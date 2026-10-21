use byteshaver::Error;
use byteshaver::cli::{CliArgs, Command};
use byteshaver::config::{ConversionConfig, EncoderConfig};
use byteshaver::job::{JobHandle, JobSpec, Reporter, Session, StdoutReporter, StopFlag};
use byteshaver::metadata::policy;
use byteshaver::utils::remove_files;
use clap::Parser;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Number of Ctrl+C presses; the second press escalates to a hard exit.
static CTRLC_PRESSES: AtomicUsize = AtomicUsize::new(0);
/// Installs the process-wide Ctrl+C handler exactly once (only one handler
/// can exist per process, even across multiple runs).
static CTRL_C_INSTALL: std::sync::Once = std::sync::Once::new();

/// Wires the Ctrl+C handler of the CLI binary to `stop` (the library never
/// installs signal handlers itself; it only consumes the stop flag).
fn install_ctrlc_handler(stop: StopFlag) {
    CTRL_C_INSTALL.call_once(|| {
        ctrlc::set_handler(move || {
            let presses = CTRLC_PRESSES.fetch_add(1, Ordering::Relaxed);
            if presses > 0 {
                // M-9: a running encode may take a long time to notice the
                // stop flag; the second press forces an immediate exit.
                eprintln!("Forcing immediate exit (unsaved progress lost)");
                std::process::exit(130);
            }
            let _ = writeln!(
                std::io::stderr(),
                "received Ctrl+C, stopping further queue processing!"
            );
            stop.raise();
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
    // Runtime CPU guard (audit M-43): this build targets x86-64-v3
    // (global `target-cpu` in .cargo/config.toml); pre-Haswell CPUs would
    // die with SIGILL — fail with a readable message and a distinct exit
    // code instead. Runs before anything else.
    if cfg!(target_arch = "x86_64") && !is_x86_feature_detected!("avx2") {
        eprintln!(
            "Error: this build targets x86-64-v3 (AVX2); your CPU lacks AVX2 — use a generic build"
        );
        std::process::exit(86);
    }

    if exif_list_tags_requested() {
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
    let enc = EncoderConfig::from_args(&args.command)
        .expect("conversion subcommand expected, clean is handled separately");

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

/// `--exif-list-tags` must work without the required glob pattern, so it
/// is checked before argument parsing (deliberate, documented clap bypass:
/// the required positional `pattern` would otherwise reject the call) and
/// prints the tag table.
fn exif_list_tags_requested() -> bool {
    std::env::args().any(|arg| arg == "--exif-list-tags")
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
