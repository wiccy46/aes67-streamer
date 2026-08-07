use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn version_matches_root_version_file() {
    let output = Command::new(env!("CARGO_BIN_EXE_aes67-tester"))
        .arg("--version")
        .output()
        .expect("tester should run");

    assert!(output.status.success());
    let version =
        fs::read_to_string(repo_root().join("VERSION")).expect("VERSION should be readable");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("aes67-tester {}", version.trim())
    );
}

#[test]
fn config_is_required() {
    let output = Command::new(env!("CARGO_BIN_EXE_aes67-tester"))
        .output()
        .expect("tester should run");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--config <FILE>"));
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should be reachable")
}
