---
tags:
  - linting
  - workspace
  - setup
created: 2026-09-28
last-updated: 2026-09-28
---

# Linter
To configure a linter following can be added to the `Cargo.toml` which is in the project root:
```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"          # drop this if it's too noisy early on
unused_qualifications = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
# turn off the pedantic lints you find annoying:
module_name_repetitions = "allow"
missing_errors_doc = "allow"
# a few useful restriction lints:
unwrap_used = "warn"
dbg_macro = "warn"
todo = "warn"
```
Just look at the `Cargo.toml` to get the idea of how it got used.

To use the linter just run:
```bash
cargo clippy --all-targets --all-features
```

## Crates
Also for linting in the crates add following lines to `crates/*/Cargo.toml`:
```toml
[lints]
workspace = true
```
>[!INFO]
> Without this, a crate ignores the workspace lints.
