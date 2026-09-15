//! Embeds the commit the binary was built from as `DAEDALUS_GIT_SHA`.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=DAEDALUS_GIT_SHA");

    // Packaging without a .git directory (a future `nix build`, a source
    // tarball) can pass the commit in explicitly.
    let sha = std::env::var("DAEDALUS_GIT_SHA")
        .ok()
        .filter(|sha| !sha.is_empty())
        .or_else(|| {
            watch_head();
            git(&["rev-parse", "--short", "HEAD"])
        })
        .unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=DAEDALUS_GIT_SHA={sha}");
}

/// Re-runs this script when HEAD moves. A checkout rewrites HEAD itself; a
/// commit rewrites the branch ref HEAD points at, which may be a loose file or
/// a line in packed-refs. `--git-path` resolves each of these correctly from a
/// linked worktree, where `.git` is a file, and relative to this crate's
/// directory, which is where cargo resolves the paths.
fn watch_head() {
    for path in ["HEAD", "packed-refs"] {
        if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(resolved) = git(&["rev-parse", "--git-path", &branch])
    {
        println!("cargo:rerun-if-changed={resolved}");
    }
}

/// Runs git, returning trimmed stdout, or `None` if git is missing, fails, or
/// prints nothing.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
