use byteshaver::cli::{CliArgs, Command};
use byteshaver::config::{ConversionConfig, EncoderConfig};
use byteshaver::utils::remove_files;
use byteshaver::{Error, run};
use clap::Parser;

fn main() -> Result<(), Error> {
    let args = CliArgs::parse();
    if matches!(args.command, Command::Clean { .. }) {
        remove_files(&args.pattern)?;
        return Ok(());
    }
    let conf = ConversionConfig::from_args(&args);
    let enc = EncoderConfig::from_args(&args.command)
        .expect("conversion subcommand expected, clean is handled separately");
    run(conf, enc)?;
    Ok(())
}
