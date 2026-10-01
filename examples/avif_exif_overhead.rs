//! Measures the overhead of AVIF EXIF embedding (`enc-avif` feature).
//!
//! When EXIF survives the policy, AVIF output is routed through the native
//! libheif encoder instead of ravif (which has no metadata API). This
//! harness encodes the same image three ways and reports the differences:
//!
//! 1. `ravif`        — the default path (no metadata), baseline
//! 2. `libheif`      — libheif route without EXIF (pure encoder swap cost)
//! 3. `libheif+exif` — libheif route with the EXIF payload attached
//!
//! Size overhead = (2) - (1) container/encoder difference and (3) - (2)
//! EXIF item cost; time overhead = encode durations of (2)/(3) vs (1).
//!
//! Note the comparison is knob-equivalent, not quality-equivalent: ravif
//! 0.13 applies its own tuned speed tweaks on top of the aom `cpu-used`
//! preset, and both are given the same quality (90) and speed setting.
//!
//! Usage:
//!   cargo run --release --features enc-avif --example avif_exif_overhead -- [IMAGES...]
//!   (defaults to examples/jpg/*.jpg; EXIF is taken from the source file if
//!   present, otherwise a small synthetic payload is embedded)

fn main() {
    #[cfg(feature = "enc-avif")]
    real_main();
    #[cfg(not(feature = "enc-avif"))]
    eprintln!(
        "This example measures the libheif AVIF EXIF route; rebuild with --features enc-avif"
    );
}

#[cfg(feature = "enc-avif")]
fn real_main() {
    use byteshaver::config::AvifOptions;
    use byteshaver::converter::{EncoderRegistry, ThreadBudget};
    use byteshaver::input;
    use byteshaver::metadata::ImageMetadata;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const DEFAULT_ITERATIONS: usize = 3;
    let iterations: usize = std::env::var("AVIF_OVERHEAD_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_ITERATIONS);

    let images: Vec<PathBuf> = std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let images = if images.is_empty() {
        glob::glob("examples/jpg/*.jpg")
            .expect("glob")
            .filter_map(std::result::Result::ok)
            .collect()
    } else {
        images
    };
    if images.is_empty() {
        eprintln!("no input images found");
        std::process::exit(1);
    }

    let encoder = EncoderRegistry::build(
        &byteshaver::config::EncoderConfig::Avif(AvifOptions::default()),
        ThreadBudget::global(),
    );
    println!("encoder: {}", encoder.describe());
    println!("iterations per configuration: {iterations}");
    println!();

    println!(
        "{:<28} {:>11} {:>10} {:>10} {:>12} {:>9} | {:>8} {:>8} {:>9}",
        "image",
        "WxH",
        "ravif",
        "libheif",
        "libheif+exif",
        "exif B",
        "t ravif",
        "t heif",
        "t +exif"
    );

    let mut totals = [0usize; 3];
    let mut total_times = [Duration::ZERO; 3];

    for path in &images {
        let source = match input::load_source(path) {
            Ok(source) => source,
            Err(err) => {
                println!("{:<28} load failed: {err}", path.display());
                continue;
            }
        };
        let image = match &source.content {
            input::ImageContent::Still(image) => image.clone(),
            input::ImageContent::Animated(_) => {
                println!("{:<28} skipped (animated)", path.display());
                continue;
            }
        };
        // embed the source EXIF when present, else a small synthetic payload;
        // AVIF_OVERHEAD_EXIF_BYTES=<n> forces a ~n-byte payload to measure
        // the metadata cost at realistic camera sizes (payload stored
        // verbatim, so padding does not affect the measurement)
        let exif_payload = source.metadata.exif.clone().unwrap_or_else(|| {
            b"MM\0*\0\0\0\x08\0\x0b\x01\x12\0\x03\0\0\0\x01\0\x01\0\0\
              \x01\x1a\0\x05\0\0\0\x01\0\0\0\x1a\x01\x1b\0\x05\0\0\0\x01\0\0\0\"\
              \x8a\x29\0\x03\0\0\0\x02\0\0\0\x08\0\0\0\0\0\0\0"
                .to_vec()
        });
        let exif_payload = match std::env::var("AVIF_OVERHEAD_EXIF_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            Some(n) if n >= exif_payload.len() => {
                let mut padded = exif_payload.clone();
                padded.resize(n, b'.');
                padded
            }
            _ => exif_payload,
        };

        let meta_ravif = ImageMetadata::default();
        let meta_heif = ImageMetadata {
            exif: Some(Vec::new()), // routes through libheif, attaches nothing
            ..ImageMetadata::default()
        };
        let meta_heif_exif = ImageMetadata {
            exif: Some(exif_payload.clone()),
            ..ImageMetadata::default()
        };

        let mut sizes = [0usize; 3];
        let mut times = [Duration::ZERO; 3];
        for (slot, meta) in [(0, &meta_ravif), (1, &meta_heif), (2, &meta_heif_exif)] {
            let mut runs: Vec<Duration> = Vec::with_capacity(iterations);
            let mut size = 0usize;
            for _ in 0..iterations {
                let start = Instant::now();
                let bytes = encoder
                    .encode_still_image_with_metadata(&image, meta)
                    .expect("encode");
                runs.push(start.elapsed());
                size = bytes.len();
            }
            runs.sort();
            sizes[slot] = size;
            times[slot] = runs[runs.len() / 2];
        }

        // sanity check: the +exif run must actually carry the payload
        assert!(
            sizes[2] > sizes[1],
            "libheif+exif output is not larger than libheif output \
             (is the AV1 encoder plugin missing? see the warnings above)"
        );

        for slot in 0..3 {
            totals[slot] += sizes[slot];
            total_times[slot] += times[slot];
        }

        println!(
            "{:<28} {:>11} {:>10} {:>10} {:>12} {:>9} | {:>7.2}s {:>7.2}s {:>8.2}s",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            format!("{}x{}", image.width(), image.height()),
            sizes[0],
            sizes[1],
            sizes[2],
            exif_payload.len(),
            times[0].as_secs_f32(),
            times[1].as_secs_f32(),
            times[2].as_secs_f32(),
        );
    }

    println!();
    println!(
        "totals: ravif {} B, libheif {} B, libheif+exif {} B",
        totals[0], totals[1], totals[2]
    );
    println!(
        "mean size overhead: encoder swap {:+.1}%, exif item {:+.1}%, combined {:+.1}%",
        pct(totals[1], totals[0]),
        pct(totals[2], totals[1]),
        pct(totals[2], totals[0]),
    );
    let mean = |d: Duration| d.as_secs_f32() / images.len() as f32;
    println!(
        "mean encode time per file: ravif {:.2}s, libheif {:.2}s ({:+.0}%), libheif+exif {:.2}s ({:+.0}% vs ravif)",
        mean(total_times[0]),
        mean(total_times[1]),
        pct_time(total_times[1], total_times[0]),
        mean(total_times[2]),
        pct_time(total_times[2], total_times[0]),
    );
}

#[cfg(feature = "enc-avif")]
fn pct(part: usize, base: usize) -> f64 {
    if base == 0 {
        0.0
    } else {
        (part as f64 - base as f64) / base as f64 * 100.0
    }
}

#[cfg(feature = "enc-avif")]
fn pct_time(part: std::time::Duration, base: std::time::Duration) -> f64 {
    if base.as_secs_f64() == 0.0 {
        0.0
    } else {
        (part.as_secs_f64() - base.as_secs_f64()) / base.as_secs_f64() * 100.0
    }
}
