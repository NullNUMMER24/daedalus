//! End-to-end tests against the compiled `dae` binary.

#![allow(
    clippy::expect_used,
    reason = "test helpers: a binary that cannot be spawned, or a scratch file that cannot be \
              written, should fail the test loudly"
)]

use std::process::{Command, Output};

fn dae(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dae"));
    cmd.args(args).env_remove("RUST_LOG").env_remove("NO_COLOR");
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("failed to run the dae binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_prints_version_and_commit() {
    let out = dae(&["version"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let expected_prefix = format!("dae {} (", env!("CARGO_PKG_VERSION"));
    let line = stdout(&out);
    assert!(line.starts_with(&expected_prefix), "got {line:?}");
    assert!(line.ends_with(")\n"), "got {line:?}");

    let sha = &line[expected_prefix.len()..line.len() - 2];
    assert!(!sha.is_empty(), "commit must never be blank, got {line:?}");
}

#[test]
fn version_flag_agrees_with_version_subcommand() {
    assert_eq!(
        stdout(&dae(&["--version"], &[])),
        stdout(&dae(&["version"], &[]))
    );
}

#[test]
fn logs_go_to_stderr_and_never_to_stdout() {
    let quiet = dae(&["version"], &[]);
    let debug = dae(&["version"], &[("RUST_LOG", "debug")]);

    assert_eq!(
        stdout(&quiet),
        stdout(&debug),
        "logging must not change stdout"
    );
    assert!(
        stderr(&debug).contains("parsed arguments"),
        "expected a debug log on stderr, got {:?}",
        stderr(&debug)
    );
    assert!(
        stderr(&quiet).is_empty(),
        "default verbosity must be silent, got {:?}",
        stderr(&quiet)
    );
}

#[test]
fn verbose_flag_enables_logging() {
    let out = dae(&["-vv", "version"], &[]);
    assert!(stderr(&out).contains("parsed arguments"));
}

#[test]
fn logs_are_not_coloured_when_stderr_is_not_a_terminal() {
    let out = dae(&["version"], &[("RUST_LOG", "debug")]);
    assert!(
        !stderr(&out).contains('\u{1b}'),
        "ANSI escapes in piped stderr: {:?}",
        stderr(&out)
    );
}

/// docs/cli.md reserves exit code 2 for usage errors.
#[test]
fn usage_errors_exit_with_code_2() {
    assert_eq!(dae(&[], &[]).status.code(), Some(2), "missing subcommand");
    assert_eq!(
        dae(&["frobnicate"], &[]).status.code(),
        Some(2),
        "unknown subcommand"
    );
}

// --------------------------------------------------------------- validate --

fn example(name: &str) -> String {
    format!("{}/../../examples/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A fresh, empty directory for one test.
fn scratch(test: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn write(root: &std::path::Path, path: &str, contents: impl AsRef<[u8]>) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().expect("has a parent")).expect("create dirs");
    std::fs::write(path, contents).expect("write file");
}

#[test]
fn validate_accepts_the_example_lab() {
    let out = dae(&["validate", &example("lab")], &[]);
    assert!(out.status.success(), "stderr:\n{}", stderr(&out));
    assert!(stderr(&out).is_empty());
    insta::assert_snapshot!(stdout(&out));
}

/// The snapshot is the documentation of what `dae validate` says: review
/// changes to it as carefully as changes to the code.
#[test]
fn validate_reports_each_mistake_in_the_broken_example_once() {
    let out = dae(&["validate", &example("broken")], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty(), "diagnostics go to stderr");
    insta::assert_snapshot!(stderr(&out));
}

#[test]
fn validate_explains_a_path_that_does_not_exist() {
    let out = dae(&["validate", "/nonexistent/daedalus"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with("error: cannot read `/nonexistent/daedalus`"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn validate_explains_a_directory_without_manifests() {
    let dir = scratch("validate-empty");
    let out = dae(&["validate", dir.to_str().expect("utf-8 path")], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no manifests in"), "{}", stderr(&out));
}

#[test]
fn validate_reports_files_that_are_not_utf8() {
    let dir = scratch("validate-not-utf8");
    write(&dir, "catalog/images/broken.yaml", [0xff, 0xfe, b'\n']);
    let out = dae(&["validate", dir.to_str().expect("utf-8 path")], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("`catalog/images/broken.yaml` is not valid UTF-8"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn validate_skips_hidden_files_and_other_extensions() {
    let dir = scratch("validate-skips");
    let image =
        "apiVersion: daedalus.io/v1alpha1\nkind: Image\nmetadata:\n  name: debian-12\nspec: {}\n";
    write(&dir, "catalog/images/debian-12.yml", image);
    write(
        &dir,
        "catalog/images/.debian-12.yaml.swp.yaml",
        "not: [valid",
    );
    write(&dir, "catalog/images/NOTES.md", "# not a manifest");
    write(&dir, "catalog/.git-crypt/key.yaml", "not: [valid");
    write(&dir, "README.yaml", "not part of the layout, never read");

    let out = dae(&["validate", dir.to_str().expect("utf-8 path")], &[]);
    assert!(out.status.success(), "stderr:\n{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("✓ 1 resource in 0 tenants, 0 environments"),
        "{}",
        stdout(&out)
    );
}
