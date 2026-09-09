# Development environment

Everything needed to build Daedalus is pinned in [`flake.nix`](../flake.nix).
No global installs, no version drift between your machine and CI.

## Getting in

```bash
nix develop
```

That drops you into **zsh with oh-my-zsh, the agnoster theme, and fzf**. The
first run downloads the toolchain (a few minutes); afterwards it is instant.

```
 ❄ daedalus  ~/git/daedalus   brainstorming ±✚ 
```

### Or let direnv do it

```bash
direnv allow
```

Now the toolchain is on `PATH` whenever you `cd` into the repo. direnv loads the
**bash** environment (it runs non-interactively), so it gives you the tools
without replacing your shell — run `nix develop` when you want the zsh
experience itself. Install [nix-direnv](https://github.com/nix-community/nix-direnv)
for a cached reload.

## ⚠️ Agnoster needs a Powerline font

The theme draws its segment separators with glyphs outside ASCII. Without a
patched font you get boxes or question marks instead of `` and ``.

Install a Nerd Font and select it in your terminal:

```bash
brew install --cask font-meslo-lg-nerd-font
```

Then set it as the terminal font — iTerm2: *Settings → Profiles → Text → Font*;
Ghostty: `font-family = MesloLGS Nerd Font`; Terminal.app: *Settings → Profiles
→ Text*. Nix cannot do this for you: a devShell package is not registered with
macOS's font system.

## The three shells

| Command | Shell | For |
| --- | --- | --- |
| `nix develop` | zsh + oh-my-zsh + fzf | Day-to-day work |
| `nix develop .#bash` | plain bash | If you dislike the theme |
| `nix develop .#ci` | plain bash, minimal | CI — compiler and test runner only, much smaller to realise on a cold cache |

`nix develop -c <cmd>` always stays in bash, so scripts, CI, and direnv never
land in an interactive zsh. That is what the `$-` check in the shellHook is for.

## What is included

**Rust** — the toolchain (with `rust-analyzer`, `clippy`, `rustfmt`, `rust-src`),
plus `cargo-nextest`, `bacon`, `cargo-watch`, `cargo-deny`, `cargo-audit`,
`cargo-insta`, `cargo-edit`, `cargo-expand`, `cargo-outdated`, `cargo-machete`,
`cargo-llvm-cov`, `cargo-flamegraph`, `sqlx-cli`, `just`.

**Kubernetes** (Phase 7) — `kubectl`, `helm`, `flux`, `k9s`, `kubectx`,
`stern`, `talosctl`.

**Infrastructure** (Phase 8) — `sops`, `age`, `ssh-to-age`, `graphviz` (for
`dae graph --format dot | dot -Tpng`).

**API poking** — `curl`, `xh`, `jq`, `yq`.

**Git** — `gh`, `lazygit`, `delta`, `pre-commit`.

**Linting** — `typos`, `markdownlint-cli2`, `nixpkgs-fmt`, `nil`.

**Shell** — `fzf`, `fd`, `ripgrep`, `bat`, `eza`, `zoxide`, `direnv`.

### Rust version

The flake tracks stable until you create `rust-toolchain.toml`
([Phase 0, step 0.1](plan/phase-0-foundations.md#step-01--install-the-toolchain)),
at which point it reads the pin from that file automatically — so the flake and
`rustup` can never disagree.

## Fuzzy search

| Key | Does |
| --- | --- |
| `Ctrl-R` | Search shell history |
| `Ctrl-T` | Insert a file path, with a `bat` preview |
| `Alt-C` | `cd` into a directory, with a tree preview |
| `Tab` | Completion menu through fzf (fzf-tab) |
| `↑` / `↓` | History search on what you have already typed |
| `→` | Accept the autosuggestion |

Plus two helpers: `fe [query]` fuzzy-opens a file in `$EDITOR`, and `fb`
fuzzy-checks-out a git branch with a commit preview.

`fd` is the backend, so `.gitignore` is respected and `target/` and `.direnv/`
are skipped.

## Aliases

`ls`/`ll`/`la`/`lt` → eza, `g` → git, `lg` → lazygit, `k` → kubectl, and the
cargo shorthands mirroring the justfile: `cb` build, `ct` nextest, `cc` clippy,
`cf` fmt, `cw` bacon.

`grep` and `cat` are deliberately **not** shadowed — ripgrep's flags differ from
grep's (`-E` means `--encoding`, not `--extended-regexp`) and silently swapping
them under muscle memory causes more confusion than it saves typing.

## Customising

The generated `.zshrc` lives in the read-only Nix store, so put personal
overrides in **`~/.zshrc.daedalus.local`**. It is sourced last, so it wins.

```bash
# ~/.zshrc.daedalus.local
DEFAULT_USER=$USER
alias pve='xh --verify=no https://pve.home.arpa:8006/api2/json'
```

To change the shell itself, edit [`nix/zsh.nix`](../nix/zsh.nix) and re-enter.

## Writable state

The store is read-only, so anything that needs to write goes to
`.direnv/daedalus/` (gitignored):

- `zsh_history` — history is **per-project**, not shared with your login shell
- `ohmyzsh/` — oh-my-zsh's cache and completion dump

`DATABASE_URL` points at `local/daedalus.db` for `sqlx` and the `query!` macros.

## Troubleshooting

**Boxes or `?` in the prompt** — install a Nerd Font (see above).

**`nix develop` is slow the first time** — it is fetching the Rust toolchain and
~60 tools. Subsequent entries are instant. `nix develop .#ci` is much lighter.

**Stale completions after adding a tool** — `rm -rf .direnv/daedalus/ohmyzsh`
and re-enter.

**Wrong Rust version** — check for a `rust-toolchain.toml`; the flake prefers it
over stable.

**A `cannot build on ssh://…` warning** — that is the remote builder configured
in your nix-darwin setup being unreachable. Harmless; nix falls back to local.

**Linker errors on macOS** — `libiconv` is already in the shell. If a crate
wants an Apple framework the modern SDK does not expose, add it to `systemLibs`
in `flake.nix`.

## Updating

```bash
nix flake update              # all inputs
nix flake update nixpkgs      # just one
nix flake check               # evaluate everything
nixpkgs-fmt flake.nix nix/    # format
```

Commit `flake.lock` — it is what makes the environment reproducible.
