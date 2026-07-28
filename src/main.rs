//! rsync-archive CLI entry point.

use clap::Parser;
use rsync_archive::cli::{Cli, Command};
use rsync_archive::util;
use rsync_archive::Error;

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
    util::init_tracing(cli.verbose);

    match cli.command {
        Command::Create(args) => {
            args.validate().map_err(ExitError::Usage)?;
            rsync_archive::pipeline::run_create(args).map_err(ExitError::Ops)
        }
        Command::Embed(args) => {
            rsync_archive::pipeline::run_embed(args).map_err(ExitError::Ops)
        }
    }
}
