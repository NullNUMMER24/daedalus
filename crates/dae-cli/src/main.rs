//! `dae` — the Daedalus command-line client.

mod logging;
mod validate;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// The crate version and the commit it was built from: `0.0.1 (3f9c2a1)`.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("DAEDALUS_GIT_SHA"),
    ")"
);

/// Daedalus homelab control plane.
#[derive(Debug, Parser)]
#[command(name = "dae", version = VERSION)]
struct Cli {
    /// Increase log verbosity: -v info, -vv debug, -vvv trace
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print version information
    Version,
    /// Check a repository's manifests, without touching any infrastructure
    Validate(validate::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    logging::init(cli.verbose);
    tracing::debug!(?cli, "parsed arguments");

    match run(cli.command) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Version => {
            println!("dae {VERSION}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Validate(args) => validate::run(&args),
    }
}
