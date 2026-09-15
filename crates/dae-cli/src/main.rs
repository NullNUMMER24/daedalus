//! `dae` — the Daedalus command-line client.

mod logging;

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
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "no command can fail yet; `validate` will in Phase 1, and this \
              expectation then goes unfulfilled and must be removed"
)]
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    logging::init(cli.verbose);
    tracing::debug!(?cli, "parsed arguments");

    match cli.command {
        Command::Version => println!("dae {VERSION}"),
    }
    Ok(())
}
