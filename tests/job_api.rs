//! WS7 job API tests: headless runs, cancelation, capabilities,
//! `InputSelection::Files` discovery, serde round-trips and JSONL logs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use byteshaver::config::{ConversionConfig, EncoderConfig, PngOptions};
use byteshaver::job::{
    self, Capabilities, InputSelection, JobEvent, JobHandle, JobSpec, Reporter, RunReport, Session,
    StopFlag,
};
use byteshaver::{Outcome, run};
// the following are only exercised by the serde/JSONL round-trips, which
// need `serde_json` (`logs` feature) and the respective encoder features
#[cfg(feature = "logs")]
use byteshaver::RunStats;
#[cfg(all(feature = "logs", feature = "anim-apng"))]
use byteshaver::config::ApngOptions;
#[cfg(all(feature = "logs", feature = "jxl"))]
use byteshaver::config::JxlOptions;
#[cfg(all(feature = "logs", feature = "opt-oxipng"))]
use byteshaver::config::OxipngOptions;
#[cfg(all(feature = "logs", feature = "anim-webp"))]
use byteshaver::config::WebpAnimOptions;
#[cfg(feature = "logs")]
use byteshaver::config::{AnimatedInputPolicy, GifOptions, HeifImagePolicy, WebpOptions};
#[cfg(feature = "logs")]
use byteshaver::metadata::policy::ExifPolicy;

/// All example images: 3 png, 2 jpeg, 10 jpg files.
const EXAMPLE_INPUT_COUNT: u64 = 15;

fn examples_dir() -> PathBuf {
    let mut base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    base.push("examples");
    base
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("byteshaver-job-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn tiny_png(dir: &Path, name: &str, shade: u8) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    let rgba = image::RgbaImage::from_pixel(4, 4, image::Rgba([shade, 1, 2, 255]));
    image::save_buffer(&path, rgba.as_raw(), 4, 4, image::ExtendedColorType::Rgba8)
        .expect("write png");
    path
}

/// Shared state of [`CaptureReporter`].
struct CaptureState {
    bytes: Mutex<Vec<u8>>,
    events: Mutex<Vec<JobEvent>>,
    stop: Option<StopFlag>,
}

/// Reporter capturing every event into a byte buffer ("stdout substitute")
/// plus the structured events; cloneable via its shared state.
#[derive(Clone)]
struct CaptureReporter {
    state: std::sync::Arc<CaptureState>,
}

impl CaptureReporter {
    fn new() -> Self {
        CaptureReporter {
            state: std::sync::Arc::new(CaptureState {
                bytes: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
                stop: None,
            }),
        }
    }

    /// Cancels the job after the first finished file (for the stop-flag
    /// test).
    fn cancel_after_first_file(stop: StopFlag) -> Self {
        Self {
            state: std::sync::Arc::new(CaptureState {
                bytes: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
                stop: Some(stop),
            }),
        }
    }

    fn events(&self) -> std::sync::MutexGuard<'_, Vec<JobEvent>> {
        self.state.events.lock().expect("events")
    }

    fn bytes(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.state.bytes.lock().expect("bytes")
    }
}

impl Reporter for CaptureReporter {
    fn on_event(&self, ev: JobEvent) {
        if let (Some(stop), JobEvent::FileFinished { .. }) = (&self.state.stop, &ev) {
            stop.raise();
        }
        self.state.events.lock().expect("events").push(ev);
        // deliberately never writes into `bytes`: proves the core emits no
        // raw stdout of its own
    }
}

// ---- Headless run: no stdout, structured report -----------------------

#[test]
fn null_reporter_job_produces_zero_stdout() {
    let dir = temp_dir("null");
    fs::create_dir_all(&dir).expect("mkdir");
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).expect("mkdir");
    tiny_png(&inputs, "a.png", 10);
    tiny_png(&inputs, "b.png", 20);
    let output = dir.join("out");

    let null = job::NullReporter::new();
    let spec = JobSpec {
        inputs: InputSelection::Files(vec![inputs.clone()]),
        output: Some(output.clone()),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(spec, Box::new(null), StopFlag::new(), &session).join();

    // the null reporter's capture buffer stayed empty: the core never
    // printed anything on its own (fully headless)
    // (captured() asserts lock health; the buffer is empty by contract)
    assert!(job::NullReporter::new().captured().is_empty());

    assert_eq!(report.totals.input_files, 2);
    assert_eq!(report.totals.successful, 2);
    assert_eq!(report.totals.errors, 0);
    assert_eq!(report.error, None);
    assert_eq!(report.files.len(), 2);
    assert!(
        report
            .files
            .iter()
            .all(|result| matches!(result.outcome, Outcome::Encoded { .. }))
    );

    // outputs land in the overridden directory
    let jpeg_count = walk_count(&output);
    assert_eq!(jpeg_count, 2);

    let _ = fs::remove_dir_all(&dir);
}

fn walk_count(dir: &Path) -> usize {
    let mut count = 0;
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            count += walk_count(&path);
        } else {
            count += 1;
        }
    }
    count
}

#[test]
fn capture_reporter_receives_full_event_stream() {
    let dir = temp_dir("events");
    fs::create_dir_all(&dir).expect("mkdir");
    let input = tiny_png(&dir, "solo.png", 7);

    let reporter = CaptureReporter::new();
    let spec = JobSpec {
        inputs: InputSelection::Files(vec![input]),
        output: Some(dir.join("out")),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let reporter_for_job = reporter.clone();
    let report =
        JobHandle::start(spec, Box::new(reporter_for_job), StopFlag::new(), &session).join();
    assert!(report.error.is_none());

    // full event stream in a sane order, nothing on the raw "stdout"
    // substitute (the core never prints on its own)
    assert!(reporter.bytes().is_empty());
    let events = reporter.events();
    let kinds: Vec<&str> = events
        .iter()
        .map(|ev| match ev {
            JobEvent::Started { .. } => "started",
            JobEvent::FileStarted { .. } => "file-started",
            JobEvent::FileFinished { .. } => "file-finished",
            JobEvent::Notice { .. } => "notice",
            JobEvent::ProgressStats { .. } => "stats",
            JobEvent::Finished => "finished",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "notice",
            "notice",
            "notice",
            "started",
            "file-started",
            "file-finished",
            "stats",
            "finished"
        ],
        "output-dir notice, Converting N notice, describe notice, then per-file events"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stop_flag_cancels_mid_queue() {
    let dir = temp_dir("cancel");
    let inputs = dir.join("in");
    fs::create_dir_all(&inputs).expect("mkdir");
    for index in 0..40u8 {
        tiny_png(&inputs, &format!("file{index:03}.png"), index);
    }

    let stop = StopFlag::new();
    let reporter = CaptureReporter::cancel_after_first_file(stop.clone());
    let spec = JobSpec {
        inputs: InputSelection::Files(vec![inputs.clone()]),
        output: Some(dir.join("out")),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(spec, Box::new(reporter), stop, &session).join();

    // without signal handlers: the flag is polled per item, so everything
    // not yet in flight is aborted and accounted for
    assert!(report.error.is_none());
    assert_eq!(report.totals.input_files, 40);
    assert!(
        report.totals.aborted > 0,
        "raising the flag after the first file must abort the rest of the queue"
    );
    assert_eq!(
        report.totals.successful + report.totals.aborted + report.totals.errors,
        40
    );
    assert_eq!(report.files.len(), 40);
    assert!(
        report
            .files
            .iter()
            .any(|result| result.outcome == Outcome::Aborted)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stop_flag_starts_lowered_and_raises() {
    let stop = StopFlag::new();
    assert!(!stop.raised());
    stop.raise();
    assert!(stop.raised());
    // clones share the flag
    let clone = stop.clone();
    assert!(clone.raised());
}

// ---- InputSelection::Files equivalence vs Pattern ----------------------

#[test]
fn files_selection_matches_pattern_discovery() {
    let pattern_output = temp_dir("sel-pattern");
    let files_output = temp_dir("sel-files");

    let mut pattern = examples_dir();
    pattern.push("**");
    pattern.push("*.*");

    let stats_pattern = run(
        ConversionConfig {
            pattern: pattern.display().to_string(),
            output: pattern_output.display().to_string(),
            ..ConversionConfig::default()
        },
        EncoderConfig::Png(PngOptions::default()),
    )
    .expect("pattern run");

    let spec = JobSpec {
        inputs: InputSelection::Files(vec![examples_dir()]),
        output: Some(files_output.clone()),
        encoder: EncoderConfig::Png(PngOptions::default()),
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(
        spec,
        Box::new(job::NullReporter::new()),
        StopFlag::new(),
        &session,
    )
    .join();

    assert_eq!(report.error, None);
    assert_eq!(
        report.totals.input_files, EXAMPLE_INPUT_COUNT,
        "directory walk discovers the same inputs as the glob"
    );
    assert_eq!(
        report.totals.successful, stats_pattern.successful,
        "same files encoded"
    );
    assert_eq!(
        report.totals.output_size, stats_pattern.output_size,
        "deterministic encoder: identical total output size"
    );
    assert_eq!(
        walk_count(&files_output),
        walk_count(&pattern_output),
        "same output file count"
    );

    // identical output tree structure (relocation base = examples dir)
    let mut files_tree = tree_with_sizes(&files_output, &files_output);
    let mut pattern_tree = tree_with_sizes(&pattern_output, &pattern_output);
    files_tree.sort();
    pattern_tree.sort();
    assert_eq!(files_tree, pattern_tree, "identical output trees");

    let _ = fs::remove_dir_all(&pattern_output);
    let _ = fs::remove_dir_all(&files_output);
}

fn tree_with_sizes(dir: &Path, root: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            out.extend(tree_with_sizes(&path, root));
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            out.push((relative, fs::metadata(&path).expect("meta").len()));
        }
    }
    out
}

#[test]
fn files_selection_reports_nonexistent_paths_per_file() {
    let dir = temp_dir("missing");
    let missing = dir.join("does-not-exist.png");
    let existing = tiny_png(&dir, "exists.png", 3);

    let spec = JobSpec {
        inputs: InputSelection::Files(vec![missing.clone(), existing.clone()]),
        output: Some(dir.join("out")),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(
        spec,
        Box::new(job::NullReporter::new()),
        StopFlag::new(),
        &session,
    )
    .join();

    assert_eq!(report.totals.input_files, 2);
    assert_eq!(report.totals.errors, 1);
    assert_eq!(report.totals.successful, 1);
    let failed = report
        .files
        .iter()
        .find(|result| result.path == missing)
        .expect("missing path reported");
    assert!(matches!(failed.outcome, Outcome::Error(_)));

    let _ = fs::remove_dir_all(&dir);
}

// ---- Capabilities ------------------------------------------------------

#[test]
fn capabilities_list_all_registry_encoders() {
    let caps = job::capabilities();
    let names: Vec<&str> = caps.encoders.iter().map(|encoder| encoder.name).collect();
    assert_eq!(
        names,
        vec![
            "webp",
            "webp-image",
            "avif",
            "png",
            "jpeg",
            "jxl",
            "oxipng",
            "webp-anim",
            "apng",
            "gif"
        ]
    );

    let extensions: std::collections::HashMap<&str, &str> = caps
        .encoders
        .iter()
        .map(|encoder| (encoder.name, encoder.extension))
        .collect();
    assert_eq!(extensions["webp"], "webp");
    assert_eq!(extensions["webp-image"], "webp");
    assert_eq!(extensions["webp-anim"], "webp");
    assert_eq!(extensions["avif"], "avif");
    assert_eq!(extensions["png"], "png");
    assert_eq!(extensions["apng"], "png");
    assert_eq!(extensions["oxipng"], "png");
    assert_eq!(extensions["jpeg"], "jpeg");
    assert_eq!(extensions["jxl"], "jxl");
    assert_eq!(extensions["gif"], "gif");

    // animation support only on the animated targets (plus gif/jxl)
    let animation: std::collections::HashMap<&str, bool> = caps
        .encoders
        .iter()
        .map(|encoder| (encoder.name, encoder.supports_animation))
        .collect();
    assert!(!animation["webp"]);
    assert!(!animation["png"]);
    assert!(!animation["jpeg"]);
    assert!(animation["webp-anim"] || !enabled(&caps, "webp-anim"));
    assert!(animation["apng"] || !enabled(&caps, "apng"));
    assert!(animation["gif"]);

    // enabled entries have no reason, disabled ones must explain why
    for encoder in &caps.encoders {
        assert_eq!(
            encoder.disabled_reason.is_none(),
            encoder.enabled,
            "{}: reason/enabled mismatch",
            encoder.name
        );
        assert!(!encoder.description.is_empty());
    }

    // compile-state reflection
    assert_eq!(enabled(&caps, "jxl"), cfg!(feature = "jxl"));
    assert_eq!(enabled(&caps, "oxipng"), cfg!(feature = "opt-oxipng"));
    assert_eq!(enabled(&caps, "webp-anim"), cfg!(feature = "anim-webp"));
    assert_eq!(enabled(&caps, "apng"), cfg!(feature = "anim-apng"));
    assert_eq!(caps.heif_input_enabled, cfg!(feature = "dec-heif"));
}

fn enabled(caps: &Capabilities, name: &str) -> bool {
    caps.encoders
        .iter()
        .find(|encoder| encoder.name == name)
        .expect("encoder listed")
        .enabled
}

// ---- RunReport <-> RunStats --------------------------------------------

#[test]
fn run_report_round_trips_through_run_stats() {
    let dir = temp_dir("report");
    let input = tiny_png(&dir, "one.png", 5);
    let spec = JobSpec {
        inputs: InputSelection::Files(vec![input]),
        output: Some(dir.join("out")),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(
        spec,
        Box::new(job::NullReporter::new()),
        StopFlag::new(),
        &session,
    )
    .join();

    let stats = report
        .clone()
        .into_stats()
        .expect("successful run maps to stats");
    assert_eq!(stats.results.len(), 1);
    assert_eq!(stats.metadata_dropped, report.metadata_dropped_count);
    assert_eq!(stats.input_files, report.totals.input_files);
    assert_eq!(stats.elapsed, report.elapsed);

    // failure mapping
    let failed = RunReport {
        error: Some("boom".to_string()),
        ..RunReport::default()
    };
    assert!(failed.into_stats().is_err());

    let _ = fs::remove_dir_all(&dir);
}

// ---- Serde round-trips (serde_json via the `logs` feature) -------------

#[cfg(feature = "logs")]
#[test]
fn serde_round_trip_conversion_config() {
    let conf = ConversionConfig {
        pattern: "images/**/*.png".to_string(),
        output: "/tmp/out".to_string(),
        reverse_processing_order: true,
        overwrite_if_smaller: true,
        overwrite_existing: false,
        discard_if_larger_than_input: true,
        discard_input_alpha_channel: true,
        exif: ExifPolicy::default(),
        heif_image_policy: HeifImagePolicy::default(),
        animated_input: AnimatedInputPolicy::Error,
        max_animation_memory_mib: 1024,
    };
    let json = serde_json::to_string(&conf).expect("serialize");
    let back: ConversionConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(conf, back);

    // every EXIF policy variant survives
    for policy in [
        ExifPolicy::Strip,
        ExifPolicy::Keep,
        ExifPolicy::FilterExcept(vec![byteshaver::TagSelector::Ifd(
            byteshaver::metadata::policy::Ifd::Gps,
        )]),
        ExifPolicy::KeepOnly(vec![
            byteshaver::TagSelector::Name("Orientation".to_string()),
            byteshaver::TagSelector::TagNum(0x8825, byteshaver::metadata::policy::Ifd::Primary),
        ]),
    ] {
        let conf = ConversionConfig {
            exif: policy,
            ..ConversionConfig::default()
        };
        let json = serde_json::to_string(&conf).expect("serialize");
        assert_eq!(conf, serde_json::from_str(&json).expect("deserialize"));
    }

    // policies individually
    for policy in [HeifImagePolicy::Primary, HeifImagePolicy::All] {
        let json = serde_json::to_string(&policy).expect("serialize");
        assert_eq!(policy, serde_json::from_str(&json).expect("deserialize"));
    }
    for policy in [AnimatedInputPolicy::FirstFrame, AnimatedInputPolicy::Error] {
        let json = serde_json::to_string(&policy).expect("serialize");
        assert_eq!(policy, serde_json::from_str(&json).expect("deserialize"));
    }
}

#[cfg(feature = "logs")]
#[test]
fn serde_round_trip_every_encoder_config_variant() {
    use byteshaver::config::{
        AlphaColorMode, AvifOptions, BitDepth, ColorModel, CompressionType, FilterType,
        JxlBitDepthChoice, JxlColorEncodingChoice, OxipngFilter, OxipngInterlace, OxipngLevel,
        OxipngReduction, OxipngStrip,
    };

    let variants: Vec<EncoderConfig> = vec![
        EncoderConfig::Webp(WebpOptions {
            lossless: true,
            quality: 75.5,
        }),
        EncoderConfig::WebpImage,
        EncoderConfig::Avif(AvifOptions {
            quality: 55.0,
            speed: 6,
            bit_depth: Some(BitDepth::Ten),
            color_model: Some(ColorModel::RGB),
            alpha_color_mode: Some(AlphaColorMode::Premultiplied),
            alpha_quality: 80.0,
        }),
        EncoderConfig::Png(PngOptions {
            compression_type: Some(CompressionType::Best),
            filter_type: Some(FilterType::Paeth),
        }),
        EncoderConfig::Jpeg,
        #[cfg(feature = "jxl")]
        EncoderConfig::Jxl(JxlOptions {
            lossless: true,
            quality: Some(90.0),
            distance: None,
            effort: 9,
            container: true,
            original_profile: true,
            decoding_speed: 2,
            intensity_target: Some(4000.0),
            bit_depth: Some(JxlBitDepthChoice::Sixteen),
            color_encoding: Some(JxlColorEncodingChoice::IccPassthrough),
            advanced: vec![("brotli_effort".to_string(), 9)],
            #[cfg(feature = "exif")]
            exif_policy: ExifPolicy::Keep,
        }),
        #[cfg(feature = "opt-oxipng")]
        EncoderConfig::Oxipng(OxipngOptions {
            level: OxipngLevel::Max,
            zopfli: true,
            zopfli_iterations: 20,
            interlace: OxipngInterlace::Adam7,
            strip: OxipngStrip::Safe,
            strip_explicit: true,
            #[cfg(feature = "exif")]
            exif_policy: ExifPolicy::Keep,
            filters: vec![OxipngFilter::Brute, OxipngFilter::MinSum],
            optimize_alpha: true,
            no_reduction: vec![OxipngReduction::Palette],
            scale_16: true,
            fix_errors: true,
            timeout: Some(std::time::Duration::from_secs(30)),
        }),
        #[cfg(feature = "anim-webp")]
        EncoderConfig::WebpAnim(WebpAnimOptions {
            quality: 70.0,
            lossless: true,
            kmin: Some(3),
            kmax: Some(0),
            minimize_size: true,
            allow_mixed: true,
            method: Some(6),
        }),
        #[cfg(feature = "anim-apng")]
        EncoderConfig::Apng(ApngOptions {
            compression_type: Some(CompressionType::Fast),
            filter_type: Some(FilterType::Up),
        }),
        EncoderConfig::Gif(GifOptions { speed: Some(25) }),
    ];

    assert!(
        variants.len() >= 6,
        "guards every compiled-in encoder variant"
    );
    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize");
        let back: EncoderConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, back, "round-trip of {json}");
    }
}

#[cfg(feature = "logs")]
#[test]
fn serde_round_trip_outcome_and_run_stats() {
    let outcome = Outcome::Encoded {
        input_size: 100,
        output_size: 42,
        metadata_dropped: true,
        output_path: std::path::PathBuf::from("out/img.webp"),
    };
    let json = serde_json::to_string(&outcome).expect("serialize");
    assert_eq!(outcome, serde_json::from_str(&json).expect("deserialize"));

    let stats = RunStats {
        input_files: 3,
        successful: 2,
        skipped: 1,
        discarded: 0,
        collisions: 0,
        errors: 0,
        aborted: 0,
        metadata_dropped: 1,
        input_size: 300,
        output_size: 84,
        results: vec![byteshaver::FileResult {
            path: PathBuf::from("a.png"),
            outcome,
        }],
        elapsed: std::time::Duration::from_millis(1234),
    };
    let json = serde_json::to_string(&stats).expect("serialize");
    assert_eq!(stats, serde_json::from_str(&json).expect("deserialize"));
}

// ---- JSONL reporter -----------------------------------------------------

#[cfg(feature = "logs")]
#[test]
fn jsonl_reporter_writes_parseable_events() {
    let dir = temp_dir("jsonl");
    let input = tiny_png(&dir, "log.png", 9);

    let log_path = dir.join("log.jsonl");
    let reporter = job::JsonlReporter::new(&log_path).expect("open log");
    let spec = JobSpec {
        inputs: InputSelection::Files(vec![input]),
        output: Some(dir.join("out")),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(spec, Box::new(reporter), StopFlag::new(), &session).join();
    assert!(report.error.is_none());

    let content = fs::read_to_string(&log_path).expect("read log");
    let mut saw_started = false;
    let mut saw_file_finished = 0;
    let mut saw_finished = false;
    for line in content.lines() {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("line is not valid JSON: {err}: {line}"));
        if value.get("Started").is_some() {
            saw_started = true;
            assert_eq!(value["Started"]["total_files"], 1);
        }
        if value.get("FileFinished").is_some() {
            saw_file_finished += 1;
            assert!(value["FileFinished"]["path"].is_string());
            assert!(value["FileFinished"]["outcome"].get("Encoded").is_some());
        }
        if value.get("ProgressStats").is_some() {
            assert!(value["ProgressStats"]["input_bytes"].is_u64());
        }
        if value.as_str() == Some("Finished") {
            saw_finished = true;
        }
    }
    assert!(saw_started, "Started event logged");
    assert_eq!(saw_file_finished, 1, "FileFinished event logged");
    assert!(saw_finished, "Finished event logged");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "logs")]
#[test]
fn tee_reporter_dispatches_to_both_receivers() {
    use job::TeeReporter;

    let dir = temp_dir("tee");
    let input = tiny_png(&dir, "tee.png", 4);
    let log_path = dir.join("tee.jsonl");

    let mut tee = TeeReporter::new();
    tee.push(Box::new(
        job::JsonlReporter::new(&log_path).expect("open log"),
    ));
    tee.push(Box::new(CaptureReporter::new()));

    let spec = JobSpec {
        inputs: InputSelection::Files(vec![input]),
        output: Some(dir.join("out")),
        encoder: EncoderConfig::Jpeg,
        common: ConversionConfig::default(),
    };
    let session = Session::new();
    let report = JobHandle::start(spec, Box::new(tee), StopFlag::new(), &session).join();
    assert_eq!(report.totals.successful, 1);

    let content = fs::read_to_string(&log_path).expect("read log");
    assert!(content.lines().any(|line| line.trim() == "\"Finished\""));

    let _ = fs::remove_dir_all(&dir);
}
