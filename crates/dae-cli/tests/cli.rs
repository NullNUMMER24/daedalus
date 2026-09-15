//! End-to-end tests against the compiled `dae` binary.

use std::process::{Command, Output};

#[expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] fns, not helpers; \
              a binary that cannot be spawned should abort the test"
)]
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
