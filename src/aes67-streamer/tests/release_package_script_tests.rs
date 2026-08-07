#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn package_release_dry_run_reports_package_layout() {
    let temp = TempDir::new("dry-run");
    let target = "x86_64-unknown-linux-gnu";
    let package_name = format!("aes67-tools-{}-{target}", release_version());
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/package_release.sh"))
        .arg("--dry-run")
        .env("AES67_RELEASE_OUTPUT_DIR", temp.path().join("out"))
        .env("AES67_RELEASE_TARGET", target)
        .output()
        .expect("script should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Release package configuration"));
    assert!(stdout.contains(&format!("Package:     {package_name}")));
    assert!(stdout.contains("bin/aes67-streamer"));
    assert!(stdout.contains("bin/aes67-player"));
    assert!(stdout.contains("bin/aes67-sap"));
    assert!(stdout.contains("bin/aes67-tester"));
    assert!(stdout.contains("examples/streamer.toml"));
    assert!(stdout.contains("examples/aes67-tester.toml"));
    assert!(stdout.contains("examples/example.sdp"));
    assert!(stdout.contains(".tar.gz"));
    assert!(stdout.contains(".sha256"));
}

#[test]
fn package_release_creates_tarball_with_expected_layout() {
    let temp = TempDir::new("package");
    let bin_dir = temp.path().join("bin");
    let target = "x86_64-unknown-linux-gnu";
    let package_name = format!("aes67-tools-{}-{target}", release_version());
    fs::create_dir_all(&bin_dir).expect("test bin dir should be writable");
    write_executable(&bin_dir.join("aes67-streamer"));
    write_executable(&bin_dir.join("aes67-player"));
    write_executable(&bin_dir.join("aes67-sap"));
    write_executable(&bin_dir.join("aes67-tester"));

    let output = Command::new("bash")
        .arg(repo_root().join("scripts/package_release.sh"))
        .env("AES67_RELEASE_OUTPUT_DIR", temp.path().join("out"))
        .env("AES67_RELEASE_BINARY_DIR", &bin_dir)
        .env("AES67_RELEASE_SKIP_BUILD", "1")
        .env("AES67_RELEASE_TARGET", target)
        .output()
        .expect("script should run");

    assert!(
        output.status.success(),
        "package script should succeed\n{}",
        output_text(&output)
    );

    let archive = temp.path().join(format!("out/{package_name}.tar.gz"));
    let checksum = temp
        .path()
        .join(format!("out/{package_name}.tar.gz.sha256"));
    assert!(
        archive.exists(),
        "archive should exist at {}",
        archive.display()
    );
    assert!(
        checksum.exists(),
        "checksum should exist at {}",
        checksum.display()
    );

    let tar_output = Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()
        .expect("tar should list archive contents");
    assert!(
        tar_output.status.success(),
        "archive should be readable\n{}",
        output_text(&tar_output)
    );
    let contents = String::from_utf8_lossy(&tar_output.stdout);
    for path in [
        "bin/aes67-streamer",
        "bin/aes67-player",
        "bin/aes67-sap",
        "bin/aes67-tester",
        "README.md",
        "LICENSE",
        "VERSION",
        "examples/streamer.toml",
        "examples/aes67-tester.toml",
        "examples/example.sdp",
    ] {
        let expected = format!("{package_name}/{path}");
        assert!(
            contents.lines().any(|line| line == expected),
            "archive should contain {expected}\n{contents}"
        );
    }
}

#[test]
fn package_release_rejects_invalid_version() {
    let temp = TempDir::new("bad-version");
    let version_file = temp.path().join("VERSION");
    fs::write(&version_file, "not-semver\n").expect("test version file should be writable");

    let output = Command::new("bash")
        .arg(repo_root().join("scripts/package_release.sh"))
        .arg("--dry-run")
        .env("AES67_RELEASE_VERSION_FILE", version_file)
        .env("AES67_RELEASE_TARGET", "x86_64-unknown-linux-gnu")
        .output()
        .expect("script should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("valid SemVer"),
        "stderr should explain invalid version, got: {stderr}"
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should be reachable")
}

fn release_version() -> String {
    fs::read_to_string(repo_root().join("VERSION"))
        .expect("VERSION file should exist")
        .trim()
        .to_string()
}

fn write_executable(path: &Path) {
    fs::write(path, "#!/usr/bin/env bash\nexit 0\n").expect("test binary should be writable");
    let mut permissions = fs::metadata(path)
        .expect("test binary metadata should be readable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("test binary should be executable");
}

fn output_text(output: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aes67-release-package-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&path).expect("temp dir should be writable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos()
}
