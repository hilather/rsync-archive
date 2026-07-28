//! rsync-archive CLI entry point.

use clap::Parser;
use rsync_archive::cli::{Cli, Command};
use rsync_archive::Error;
use tracing_subscriber::EnvFilter;

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(ExitError::Usage(msg)) => {
            eprintln!("error: {msg}");
            2
        }
        Err(ExitError::Ops(err)) => {
            eprintln!("error: {err}");
            1
        }
    };
    std::process::exit(code);
}

enum ExitError {
    Usage(String),
    Ops(Error),
}

fn run() -> std::result::Result<(), ExitError> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Create(args) => {
            args.validate().map_err(ExitError::Usage)?;
            Err(ExitError::Ops(Error::NotImplemented(
                "create (Stage 5–6b; dry-run in Stage 5, write in Stage 6/6b)",
            )))
        }
        Command::Embed(_args) => Err(ExitError::Ops(Error::NotImplemented(
            "embed (Stage 3)",
        ))),
    }
}

fn init_tracing(verbose: u8) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default));
    // Ignore double-init in tests.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
