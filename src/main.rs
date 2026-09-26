use byteshaver::Error;
use byteshaver::cli::{CliArgs, Command};
use byteshaver::config::{ConversionConfig, EncoderConfig};
use byteshaver::job::{JobHandle, JobSpec, Reporter, Session, StdoutReporter, StopFlag};
use byteshaver::metadata::policy;
use byteshaver::utils::remove_files;
use clap::Parser;
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of Ctrl+C presses, feeding the repeated-press notice.
static CTRLC_PRESSES: AtomicU64 = AtomicU64::new(0);
/// Installs the process-wide Ctrl+C handler exactly once (only one handler
/// can exist per process, even across multiple runs).
static CTRL_C_INSTALL: std::sync::Once = std::sync::Once::new();

/// Wires the Ctrl+C handler of the CLI binary to `stop` (the library never
/// installs signal handlers itself; it only consumes the stop flag).
fn install_ctrlc_handler(stop: StopFlag) {
    CTRL_C_INSTALL.call_once(|| {
        ctrlc::set_handler(move || {
            let presses = CTRLC_PRESSES.fetch_add(1, Ordering::Relaxed);
            if !stop.raised() {
                println!("received Ctrl+C, stopping further queue processing!");
                stop.raise();
            } else {
                println!(
                    "an encoding task is still active!{} processing will end afterwards.",
                    str::repeat("!", presses as usize)
                );
            }
        })
        .expect("Error setting Ctrl-C handler");
    });
}

/// Builds the event reporter: plain stdout rendering, or a tee that adds a
/// JSON-lines log when `--json-log` is set (`logs` feature).
fn build_reporter(args: &CliArgs) -> Result<Box<dyn Reporter>, Error> {
    let stdout: Box<dyn Reporter> = Box::new(StdoutReporter::new());
    #[cfg(feature = "logs")]
    if let Some(log_path) = &args.json_log {
        let jsonl = byteshaver::job::JsonlReporter::new(log_path)
            .map_err(|err| Error::from_string(format!("JSON log file: {err}")))?;
        let mut tee = byteshaver::job::TeeReporter::new();
        tee.push(Box::new(jsonl));
        tee.push(stdout);
        return Ok(Box::new(tee));
    }
    #[cfg(not(feature = "logs"))]
    let _ = args;
    Ok(stdout)
}

fn main() -> Result<(), Error> {
    // --exif-list-tags must work without the required glob pattern, so it is
    // checked before argument parsing and prints the tag table, then exits.
    if std::env::args().any(|arg| arg == "--exif-list-tags") {
        print!("{}", policy::format_recognized_tags());
        return Ok(());
    }

    let args = CliArgs::parse();
    if matches!(args.command, Command::Clean { .. }) {
        remove_files(&args.pattern)?;
        return Ok(());
    }
    warn_if_exif_feature_missing(&args);
    let conf = ConversionConfig::from_args(&args);
    // `mut` is only used when an encoder consumes the EXIF policy (oxipng/jxl)
    #[allow(unused_mut)]
    let mut enc = EncoderConfig::from_args(&args.command)
        .expect("conversion subcommand expected, clean is handled separately");
    #[cfg(all(feature = "opt-oxipng", feature = "exif"))]
    if let byteshaver::config::EncoderConfig::Oxipng(oxipng_options) = &mut enc {
        oxipng_options.exif_policy = conf.exif.clone();
    }
    #[cfg(all(feature = "jxl", feature = "exif"))]
    if let byteshaver::config::EncoderConfig::Jxl(jxl_options) = &mut enc {
        jxl_options.exif_policy = conf.exif.clone();
    }

    // conversions run through the headless job API; the binary contributes
    // the Ctrl+C handler and the stdout/JSON reporters
    let stop = StopFlag::new();
    install_ctrlc_handler(stop.clone());
    let reporter = build_reporter(&args)?;
    let spec = JobSpec::from_conversion(conf, enc);
    let session = Session::new();
    JobHandle::start(spec, reporter, stop, &session)
        .join()
        .into_stats()?;
    Ok(())
}

/// Without the `exif` feature, metadata is always stripped; warn when the
/// user explicitly requested to keep some.
#[cfg(not(feature = "exif"))]
fn warn_if_exif_feature_missing(args: &CliArgs) {
    if args.exif.is_some() || args.exif_except.is_some() || args.exif_only.is_some() {
        eprintln!(
            "Warning: this build was compiled without the \"exif\" feature; \
             EXIF metadata is always stripped regardless of --exif flags"
        );
    }
}

/// With the `exif` feature enabled (default), nothing to warn about.
#[cfg(feature = "exif")]
fn warn_if_exif_feature_missing(_args: &CliArgs) {}
