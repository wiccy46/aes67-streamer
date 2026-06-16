use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn version_exits_successfully_and_matches_version_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_aes67-sap"))
        .arg("--version")
        .output()
        .expect("aes67-sap --version should run");

    assert!(
        output.status.success(),
        "version should exit successfully\n{}",
        output_text(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&release_version()),
        "version output should contain root VERSION, got: {stdout}"
    );
}

#[test]
fn help_exits_successfully_and_hides_test_only_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_aes67-sap"))
        .arg("--help")
        .output()
        .expect("aes67-sap --help should run");

    assert!(
        output.status.success(),
        "help should exit successfully\n{}",
        output_text(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--interface"));
    assert!(stdout.contains("--once"));
    assert!(stdout.contains("--sdp-output-dir"));
    assert!(!stdout.contains("--test-address"));
    assert!(!stdout.contains("--test-port"));
}

fn release_version() -> String {
    fs::read_to_string(repo_root().join("VERSION"))
        .expect("VERSION file should exist")
        .trim()
        .to_string()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should be reachable")
}

fn output_text(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}
