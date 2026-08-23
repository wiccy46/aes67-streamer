use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should resolve")
}

#[test]
fn multicast_e2e_dry_run_requires_explicit_interface() {
    let repo_root = repo_root();
    let output = Command::new("bash")
        .arg(repo_root.join("scripts/e2e_multicast.sh"))
        .arg("--dry-run")
        .env_remove("AES67_E2E_INTERFACE")
        .current_dir(&repo_root)
        .output()
        .expect("script should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AES67_E2E_INTERFACE"),
        "stderr should explain the missing interface, got: {stderr}"
    );
}

#[test]
fn multicast_e2e_dry_run_reports_configuration() {
    let repo_root = repo_root();
    let output = Command::new("bash")
        .arg(repo_root.join("scripts/e2e_multicast.sh"))
        .arg("--dry-run")
        .env("AES67_E2E_INTERFACE", "127.0.0.1")
        .env("AES67_E2E_ADDRESS", "239.69.67.67")
        .env("AES67_E2E_PORT", "55044")
        .current_dir(&repo_root)
        .output()
        .expect("script should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Multicast E2E configuration"),
        "stdout should include the configuration summary, got: {stdout}"
    );
    assert!(stdout.contains("239.69.67.67"));
    assert!(stdout.contains("55044"));
    assert!(stdout.contains("127.0.0.1"));
}

#[test]
fn multicast_e2e_dry_run_rejects_non_multicast_address() {
    let repo_root = repo_root();
    let output = Command::new("bash")
        .arg(repo_root.join("scripts/e2e_multicast.sh"))
        .arg("--dry-run")
        .env("AES67_E2E_INTERFACE", "127.0.0.1")
        .env("AES67_E2E_ADDRESS", "223.255.255.255")
        .current_dir(&repo_root)
        .output()
        .expect("script should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("multicast group"),
        "stderr should explain the multicast address requirement, got: {stderr}"
    );
}
