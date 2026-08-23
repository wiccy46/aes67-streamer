use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_aes67")
}

#[test]
fn root_help_lists_the_two_product_lines() {
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("aes67 --help should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("send"));
    assert!(stdout.contains("receive"));
    assert!(!stdout.contains("verify"));
    assert!(!stdout.contains("player"));
}

#[test]
fn receive_listen_help_uses_canonical_language() {
    let output = Command::new(binary())
        .args(["receive", "listen", "--help"])
        .output()
        .expect("receive listen help should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AES67 Receive Listen"));
    assert!(stdout.contains("--output-device"));
    assert!(!stdout.contains("player"));
}

#[test]
fn version_matches_the_root_version_file() {
    let output = Command::new(binary())
        .arg("--version")
        .output()
        .expect("aes67 --version should run");

    assert!(output.status.success());
    let version =
        fs::read_to_string(repo_root().join("VERSION")).expect("VERSION file should be readable");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("aes67 {}", version.trim())
    );
}

#[test]
fn invalid_product_command_fails_with_help_pointer() {
    let output = Command::new(binary())
        .args(["receive", "play"])
        .output()
        .expect("invalid command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"));
    assert!(stderr.contains("aes67 receive --help"));
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should be reachable")
}
