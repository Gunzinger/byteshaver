//! Measures the overhead of AVIF EXIF embedding (ravif >= 0.13).
//!
//! ravif serializes the EXIF payload as a standard HEIF `Exif` item into the
//! ISOBMFF container *after* the AV1 encode, so the expected overhead is the
//! item bytes plus a few box headers — no extra encoding work. This harness
//! verifies that by encoding the same image twice:
//!
//! 1. `plain` — no metadata (baseline)
//! 2. `+exif` — same encode with the EXIF payload attached
//!
//! Reported: output sizes (absolute and delta = metadata cost) and encode
//! times (median of the configured iterations).
//!
//! Usage:
//!   cargo run --release --example avif_exif_overhead -- [IMAGES...]
//!   (defaults to examples/jpg/*.jpg; EXIF is taken from the source file if
//!   present, otherwise a small synthetic payload is embedded;
//!   AVIF_OVERHEAD_ITERATIONS=<n> configures the runs per image)

fn main() {
    let images: Vec<std::path::PathBuf> = std::env::args()
        .skip(1)
        .map(std::path::PathBuf::from)
        .collect();
    let images = if images.is_empty() {
        glob::glob("examples/jpg/*.jpg")
            .expect("glob")
            .filter_map(Result::ok)
            .collect()
    } else {
        images
    };
    if images.is_empty() {
        eprintln!("no input images found");
        std::process::exit(1);
    }

    let encoder = byteshaver::converter::EncoderRegistry::build(
        &byteshaver::config::EncoderConfig::Avif(byteshaver::config::AvifOptions::default()),
        byteshaver::converter::ThreadBudget::global(),
    );
    println!("encoder: {}", encoder.describe());

    let iterations: usize = std::env::var("AVIF_OVERHEAD_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(3);
    println!("iterations per configuration: {iterations}");
    println!();

    println!(
        "{:<24} {:>11} {:>10} {:>10} {:>9} {:>9} | {:>8} {:>8}",
        "image", "WxH", "plain B", "+exif B", "exif B", "delta B", "t plain", "t +exif"
    );

    let mut total_plain = 0usize;
    let mut total_exif = 0usize;
    let mut total_time_plain = std::time::Duration::ZERO;
    let mut total_time_exif = std::time::Duration::ZERO;
    let mut measured = 0usize;

    for path in &images {
        let source = match byteshaver::input::load_source(path) {
            Ok(source) => source,
            Err(err) => {
                println!("{:<24} load failed: {err}", path.display());
                continue;
            }
        };
        let image = match &source.content {
            byteshaver::input::ImageContent::Still(image) => image.clone(),
            byteshaver::input::ImageContent::Animated(_) => {
                println!("{:<24} skipped (animated)", path.display());
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

        let meta_plain = byteshaver::metadata::ImageMetadata::default();
        let meta_exif = byteshaver::metadata::ImageMetadata {
            exif: Some(exif_payload.clone()),
            ..byteshaver::metadata::ImageMetadata::default()
        };

        let mut sizes = [0usize; 2];
        let mut times = [std::time::Duration::ZERO; 2];
        for (slot, meta) in [(0, &meta_plain), (1, &meta_exif)] {
            let mut runs: Vec<std::time::Duration> = Vec::with_capacity(iterations);
            let mut size = 0usize;
            for _ in 0..iterations {
                let start = std::time::Instant::now();
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
            sizes[1] > sizes[0],
            "+exif output is not larger than plain output (was the payload dropped?)"
        );

        total_plain += sizes[0];
        total_exif += sizes[1];
        total_time_plain += times[0];
        total_time_exif += times[1];
        measured += 1;

        println!(
            "{:<24} {:>11} {:>10} {:>10} {:>9} {:>9} | {:>7.2}s {:>7.2}s",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            format!("{}x{}", image.width(), image.height()),
            sizes[0],
            sizes[1],
            exif_payload.len(),
            sizes[1] - sizes[0],
            times[0].as_secs_f32(),
            times[1].as_secs_f32(),
        );
    }

    if measured == 0 {
        return;
    }
    println!();
    println!(
        "totals: plain {total_plain} B, +exif {total_exif} B (delta {} B, {:+.3}%)",
        total_exif - total_plain,
        (total_exif - total_plain) as f64 / total_plain as f64 * 100.0
    );
    let mean = |d: std::time::Duration| d.as_secs_f32() / measured as f32;
    println!(
        "mean encode time per file: plain {:.2}s, +exif {:.2}s ({:+.1}%)",
        mean(total_time_plain),
        mean(total_time_exif),
        (mean(total_time_exif) - mean(total_time_plain)) / mean(total_time_plain) * 100.0
    );
}
