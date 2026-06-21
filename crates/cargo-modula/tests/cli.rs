//! Smoke tests for the `cargo modula` binary. Marked `#[ignore]` because they
//! run the full rust-analyzer extractor and need a toolchain.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "modula-extract",
        "tests",
        "fixtures",
        name,
    ]
    .iter()
    .collect()
}

fn spike_fixture() -> PathBuf {
    fixture("spike")
}

#[test]
#[ignore = "runs the full extractor; needs a toolchain"]
fn prints_a_report_and_succeeds() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(spike_fixture())
        .output()
        .expect("run cargo-modula");
    assert!(output.status.success(), "expected success exit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Headline score"), "stdout: {stdout}");
}

#[test]
#[ignore = "runs the full extractor; needs a toolchain"]
fn unmeetable_gate_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(spike_fixture())
        .args(["--min-headline", "1.01"])
        .output()
        .expect("run cargo-modula");
    assert!(!output.status.success(), "expected gate failure exit");
}

#[test]
#[ignore = "runs the full extractor; needs a toolchain"]
fn package_and_workspace_flags_work() {
    // `--package` selects a named member; the fixture has a lib plus an
    // integration test, and the test target must be excluded.
    let by_package = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(fixture("targets"))
        .args(["--package", "targets"])
        .output()
        .expect("run cargo-modula");
    assert!(by_package.status.success());

    let by_workspace = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(fixture("targets"))
        .arg("--workspace")
        .output()
        .expect("run cargo-modula");
    assert!(by_workspace.status.success());
}

#[test]
#[ignore = "runs the full extractor; needs a toolchain"]
fn emits_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(spike_fixture())
        .arg("--json")
        .output()
        .expect("run cargo-modula");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"headline\""), "stdout: {stdout}");
}

// The following two tests fail in `resolve_manifest`, before the extractor
// runs, so they need no toolchain and are not ignored.

#[test]
fn nonexistent_path_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg("/no/such/modula/path/zzz")
        .output()
        .expect("run cargo-modula");
    assert!(!output.status.success(), "expected failure exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("path does not exist"), "stderr: {stderr}");
}

#[test]
fn directory_without_manifest_errors() {
    // A real directory that is not a project of any supported language.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("no_manifest_here");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let _ = std::fs::remove_file(dir.join("Cargo.toml"));
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(&dir)
        .output()
        .expect("run cargo-modula");
    assert!(!output.status.success(), "expected failure exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not detect a supported project"),
        "stderr: {stderr}"
    );
}

#[test]
#[ignore = "runs the full extractor; needs a toolchain"]
fn passing_gates_print_pass_and_succeed() {
    // spike is acyclic, so all three gates pass; this drives the gate-printing
    // PASS / [ok] branch the failing-gate test does not reach.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(spike_fixture())
        .args([
            "--min-headline",
            "0.0",
            "--require-acyclic",
            "--max-overexposed",
            "1.0",
        ])
        .output()
        .expect("run cargo-modula");
    assert!(output.status.success(), "expected passing gates");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Gates: PASS"), "stderr: {stderr}");
    assert!(stderr.contains("[ok]"), "stderr: {stderr}");
}

#[test]
#[ignore = "runs the full extractor; needs a toolchain"]
fn json_mode_gates_exit_code_without_printing_gates() {
    // In JSON mode the gate block is not printed, but gates still set the exit.
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-modula"))
        .arg("modula")
        .arg(spike_fixture())
        .args(["--json", "--min-headline", "1.01"])
        .output()
        .expect("run cargo-modula");
    assert!(!output.status.success(), "expected gate failure exit");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"headline\""), "stdout: {stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Gates:"),
        "gate block must be suppressed in JSON mode"
    );
}
