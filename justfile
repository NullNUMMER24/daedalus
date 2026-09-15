# Daedalus task runner. Run `just --list` for recipes.
#
# CI runs these same recipes inside `nix develop .#ci`, so a green
# `just check` locally means a green pipeline.

set shell := ["bash", "-euo", "pipefail", "-c"]

# CI sets DAE_CARGO_FLAGS=--locked, so a Cargo.lock that is out of date fails
# the build instead of being silently regenerated. Locally it stays empty, so
# adding a dependency does not make every recipe fail until you rebuild.
cargo_flags := env("DAE_CARGO_FLAGS", "")

# Everything CI checks for Rust code
default: check

# fmt, clippy, tests, and the dae-core purity rule
check: fmt lint test core-purity

# Fail if anything is unformatted (`just fix` formats)
fmt:
    cargo fmt --all -- --check

# Clippy across every crate and target, warnings as errors
lint:
    cargo clippy {{cargo_flags}} --workspace --all-targets --all-features -- -D warnings

# Run the test suite with nextest
test:
    cargo nextest run {{cargo_flags}} --workspace --all-features

# Format, and apply clippy's machine-applicable fixes
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged

# D-012: dae-core must not depend on anything that does I/O
core-purity:
    #!/usr/bin/env bash
    set -euo pipefail
    # Normal and build edges only: a dev-dependency used by tests is fine.
    offenders=$(cargo tree {{cargo_flags}} -p dae-core -e normal,build --prefix none \
        | grep -E '^(tokio|async-std|smol|mio|sqlx|rusqlite|reqwest|hyper|axum|tonic|kube|git2|gix) ' \
        | sort -u || true)
    if [[ -n "$offenders" ]]; then
        echo "error: dae-core depends on crates that do I/O (docs/decisions.md, D-012):" >&2
        echo "$offenders" | sed 's/^/  /' >&2
        exit 1
    fi
    echo "dae-core has no I/O dependencies"

# Dependency licences, security advisories and sources
deny:
    cargo deny check

# Spelling across the repo, and markdown lint for the docs
docs:
    typos
    markdownlint-cli2

# Re-run the tests on every save
watch:
    bacon nextest

# Run the CLI, e.g. `just run version`
run *args:
    cargo run --quiet --bin dae -- {{args}}
