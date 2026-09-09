# Phase 0 — Foundations

**Goal.** A Rust workspace you enjoy working in, with CI that catches mistakes
before you do.

**Demo.** `dae version` prints a version and a git SHA. CI is green. `just check`
runs everything.

**Estimate.** ~1 week of evenings.

**Rust you will learn.** Cargo workspaces, modules and visibility, `Result` and
`?`, `clippy` lints, `rustfmt`.

> This phase feels like throat-clearing. Do it anyway. The lint configuration and
> CI you set up here will catch hundreds of mistakes over the next six months,
> and retrofitting them into a large codebase is genuinely unpleasant.

---

## Step 0.1 — Install the toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup component add clippy rustfmt rust-analyzer
cargo install cargo-watch cargo-nextest cargo-deny just
```

Pin the toolchain so CI and your machine agree:

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.90"          # set to current stable when you start
components = ["clippy", "rustfmt"]
```

**Check:** `cargo --version` and `rustc --version` both report the pinned version.

---

## Step 0.2 — Create the workspace

```bash
cd ~/git/daedalus
cargo new --lib crates/dae-core
cargo new --bin crates/dae-cli --name dae
```

```toml
# Cargo.toml  (workspace root)
[workspace]
resolver = "3"
members = ["crates/*", "bin/*"]

[workspace.package]
version      = "0.0.1"
edition      = "2024"
license      = "AGPL-3.0-or-later"
repository   = "https://github.com/NullNUMMER24/daedalus"
rust-version = "1.90"

# Every dependency version is declared ONCE, here. Member crates use
# `serde = { workspace = true }`. This prevents version drift across
# twelve crates, which becomes a real problem surprisingly fast.
[workspace.dependencies]
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror  = "2"
anyhow     = "1"
tracing    = "0.1"
ulid       = "1"
clap       = { version = "4", features = ["derive", "env"] }

[profile.release]
lto           = "thin"
codegen-units = 1
strip         = true
```

⚠️ Check the current `resolver` and `edition` values when you start; these move.

**Check:** `cargo build` succeeds from the workspace root.

---

## Step 0.3 — Configure lints centrally

```toml
# Cargo.toml (workspace root), appended
[workspace.lints.rust]
unsafe_code            = "forbid"
missing_debug_implementations = "warn"
unreachable_pub        = "warn"

[workspace.lints.clippy]
unwrap_used  = "deny"       # ⚠️ set this NOW, not later
expect_used  = "warn"
panic        = "warn"
todo         = "warn"
pedantic     = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
missing_errors_doc      = "allow"
```

Every member crate then needs:

```toml
[lints]
workspace = true
```

💡 `unwrap_used = "deny"` is the highest-value line in this file. It forces you
to handle errors from day one, which is exactly the Rust habit worth building.
Allow it in tests with `#![cfg_attr(test, allow(clippy::unwrap_used))]`.

**Check:** add a deliberate `.unwrap()` somewhere; `cargo clippy` fails. Remove it.

---

## Step 0.4 — A task runner

```make
# justfile
default: check

check: fmt lint test

fmt:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
    cargo nextest run --workspace

fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --fix --allow-dirty

audit:
    cargo deny check

watch:
    cargo watch -x 'nextest run --workspace'

run *ARGS:
    cargo run --bin dae -- {{ARGS}}
```

**Check:** `just check` passes.

---

## Step 0.5 — Error handling foundations

Create `crates/dae-core/src/error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid resource name {name:?}: {reason}")]
    InvalidName { name: String, reason: String },

    #[error("unknown kind {0:?}")]
    UnknownKind(String),

    #[error("validation failed with {} error(s)", .0.len())]
    Validation(Vec<ValidationError>),
}

#[derive(Debug, Clone)]
pub struct ValidationError {
    pub path: String,          // "tenants/acme/machines/web-01.yaml"
    pub line: Option<usize>,
    pub message: String,
    pub hint: Option<String>,  // "did you mean `memory`?"
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
```

💡 The pattern: **`thiserror` in libraries** (callers can match on variants),
**`anyhow` in binaries** (you just want to report). Getting this right early
saves a large refactor later.

⚠️ `Validation` holds a `Vec` on purpose. Collecting all errors rather than
failing on the first is the difference between a tool people like and one they
tolerate. Design for it from the start.

**Check:** `cargo test -p dae-core` passes with a test that constructs each variant.

---

## Step 0.6 — Structured logging

```rust
// crates/dae-cli/src/logging.rs
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn init(verbosity: u8) {
    let default = match verbosity {
        0 => "warn,dae=info",
        1 => "info,dae=debug",
        2 => "debug",
        _ => "trace",
    };
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(default)))
        .with(fmt::layer().with_target(false))
        .init();
}
```

💡 Use `tracing`, not `log`. Spans — not just lines — are what make a
reconciliation run comprehensible later: you will want to see every action nested
under its apply run, with the correlation ID attached.

**Check:** `RUST_LOG=debug cargo run --bin dae -- version` prints debug output.

---

## Step 0.7 — CLI skeleton

```rust
// crates/dae-cli/src/main.rs
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dae", version, about = "Daedalus homelab control plane")]
struct Cli {
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[arg(long, global = true, env = "DAEDALUS_CONTEXT")]
    context: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print version information
    Version,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    crate::logging::init(cli.verbose);

    match cli.command {
        Command::Version => {
            println!("dae {} ({})",
                env!("CARGO_PKG_VERSION"),
                option_env!("DAEDALUS_GIT_SHA").unwrap_or("unknown"));
        }
    }
    Ok(())
}
```

Add a `build.rs` to capture the git SHA:

```rust
fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=DAEDALUS_GIT_SHA={}", sha.trim());
    println!("cargo:rerun-if-changed=.git/HEAD");
}
```

**Check:** `cargo run --bin dae -- version` prints a version and a real SHA.

---

## Step 0.8 — CI

```yaml
# .github/workflows/ci.yml
name: CI
on:
  push:    { branches: [main] }
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-D warnings"

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: rustfmt, clippy }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets --all-features
      - uses: taiki-e/install-action@nextest
      - run: cargo nextest run --workspace

  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v2

  core-purity:
    name: dae-core has no I/O dependencies
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Fail if dae-core depends on tokio, sqlx or reqwest
        run: |
          if cargo tree -p dae-core --prefix none \
             | grep -qE '^(tokio|sqlx|reqwest|axum) '; then
            echo "::error::dae-core gained an I/O dependency — see D-012"
            exit 1
          fi
```

⚠️ That last job enforces [D-012](../decisions.md#d-012--dae-core-has-no-io-dependencies).
It is three lines and it will preserve the architecture for six months.

**Check:** push a branch; all three jobs pass.

---

## Step 0.9 — Repository hygiene

```bash
# .gitignore
/target
*.db
*.db-shm
*.db-wal
.env
/local/
```

Add `deny.toml` (start from `cargo deny init`), a `CONTRIBUTING.md` stub, and
`LICENSE`.

💡 Pick a licence now. AGPL-3.0 if you want self-hosted forks to stay open;
Apache-2.0 if you want the widest adoption. This is much easier to decide before
anyone else contributes.

**Check:** `git status` is clean after a full build.

---

## Definition of done

- [ ] `just check` passes locally
- [ ] CI green on all three jobs
- [ ] `dae version` prints version + git SHA
- [ ] `cargo clippy` denies `unwrap()` in non-test code
- [ ] `RUST_LOG=debug` changes the output
- [ ] Workspace has `dae-core` and `dae-cli`, both linted from the workspace

## Pitfalls

- **Skipping `unwrap_used = "deny"` "for now".** Retrofitting it into 10k lines
  means several hundred call sites. Ask how anyone knows this.
- **Not pinning the toolchain.** "Works locally, fails in CI" is nearly always a
  version difference.
- **Over-engineering the workspace.** Two crates is right for Phase 0. Do not
  create all twelve up front — you will guess the boundaries wrong.
- **Fighting `clippy::pedantic`.** If a pedantic lint is genuinely noisy for your
  style, `allow` it in the workspace config and move on. Do not let it become a
  reason to disable clippy entirely.
