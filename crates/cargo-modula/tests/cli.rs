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
