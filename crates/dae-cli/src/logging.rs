//! Diagnostics setup.
//!
//! Logs go to **stderr**. stdout belongs to command output that scripts parse —
//! `dae get machines -o json | jq` must never receive a log line.

use std::io::IsTerminal;

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Installs the global subscriber. Call once, at startup.
///
/// `-v` raises the level (info, debug, trace). `RUST_LOG`, when set, takes
/// precedence and accepts full filter syntax, e.g. `RUST_LOG=warn,dae=trace`.
pub(crate) fn init(verbosity: u8) {
    let default_level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    // Colour only for a human watching a terminal, and never under NO_COLOR.
    let colour = std::io::stderr().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(colour)
                .with_target(false),
        )
        .init();
}
