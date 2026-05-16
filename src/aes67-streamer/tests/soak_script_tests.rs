use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should resolve")
}

#[test]
fn soak_test_dry_run_reports_default_duration() {
    let repo_root = repo_root();
    let output = Command::new("bash")
        .arg(repo_root.join("scripts/soak_loopback.sh"))
        .arg("--dry-run")
        .env_remove("AES67_SOAK_DURATION_SECONDS")
        .current_dir(&repo_root)
        .output()
        .expect("script should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Soak test configuration"));
    assert!(stdout.contains("Duration: 300 seconds"));
    assert!(stdout.contains("scripts/e2e_loopback.sh"));
}

#[test]
fn soak_test_dry_run_rejects_zero_duration() {
    let repo_root = repo_root();
    let output = Command::new("bash")
        .arg(repo_root.join("scripts/soak_loopback.sh"))
        .arg("--dry-run")
        .env("AES67_SOAK_DURATION_SECONDS", "0")
        .current_dir(&repo_root)
        .output()
        .expect("script should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AES67_SOAK_DURATION_SECONDS"),
        "stderr should explain the invalid duration, got: {stderr}"
    );
}
