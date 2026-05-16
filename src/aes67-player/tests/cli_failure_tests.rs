use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use semver::Version;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_aes67-player")
}

fn version_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../VERSION")
}

#[test]
fn help_exits_successfully_and_hides_test_only_output() {
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("player binary should run");
    let logs = process_output_text(&output);

    assert!(
        output.status.success(),
        "help should exit successfully\n{logs}"
    );
    assert!(logs.contains("Usage: aes67-player"));
    assert!(logs.contains("--list-devices"));
    assert!(logs.contains("--output-device"));
    assert!(
        !logs.contains("--test-null-output"),
        "test-only output flag should stay hidden from user help\n{logs}"
    );
}

#[test]
fn version_exits_successfully() {
    let output = Command::new(binary())
        .arg("--version")
        .output()
        .expect("player binary should run");
    let logs = process_output_text(&output);

    assert!(
        output.status.success(),
        "version should exit successfully\n{logs}"
    );
    assert_eq!(
        logs.trim(),
        format!("aes67-player {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn version_file_matches_player_package_version_and_is_valid_semver() {
    let version = fs::read_to_string(version_file()).expect("VERSION file should exist");
    let version = version.trim();

    Version::parse(version).expect("VERSION should be valid SemVer");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn invalid_sender_filter_exits_with_clear_error() {
    let output = Command::new(binary())
        .arg("--address")
        .arg("239.1.1.1")
        .arg("--port")
        .arg("5004")
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--sender")
        .arg("not-an-ip")
        .arg("--test-null-output")
        .arg("--duration-seconds")
        .arg("1")
        .output()
        .expect("player binary should run");

    assert_failure_contains(&output, "Invalid sender filter IPv4 address");
}

#[test]
fn missing_sdp_file_exits_with_clear_error() {
    let path = std::env::temp_dir().join(format!(
        "aes67-player-missing-sdp-{}-{}.sdp",
        std::process::id(),
        unique_suffix()
    ));
    fs::remove_file(&path).ok();

    let output = Command::new(binary())
        .arg("--sdp")
        .arg(&path)
        .arg("--test-null-output")
        .output()
        .expect("player binary should run");

    assert_failure_contains(&output, "failed to read SDP file");
}

#[test]
fn unsupported_sdp_format_exits_with_clear_error() {
    let sdp = TempFile::write(
        "unsupported",
        "v=0\r\n\
         c=IN IP4 239.1.1.1/32\r\n\
         m=audio 5004 RTP/AVP 97\r\n\
         a=rtpmap:97 L16/48000/2\r\n\
         a=ptime:1\r\n",
    );

    let output = Command::new(binary())
        .arg("--sdp")
        .arg(sdp.path())
        .arg("--test-null-output")
        .output()
        .expect("player binary should run");

    assert_failure_contains(&output, "unsupported SDP audio encoding");
}

#[test]
fn nonexistent_output_device_exits_with_clear_error() {
    let selector = format!("missing-aes67-output-device-{}", unique_suffix());
    let output = Command::new(binary())
        .arg("--address")
        .arg("239.1.1.1")
        .arg("--port")
        .arg("5004")
        .arg("--interface")
        .arg("127.0.0.1")
        .arg("--output-device")
        .arg(selector)
        .arg("--duration-seconds")
        .arg("1")
        .output()
        .expect("player binary should run");

    assert_failure_contains(&output, "Failed to create AES67 player");
}

fn assert_failure_contains(output: &Output, expected: &str) {
    let logs = process_output_text(output);
    assert!(
        !output.status.success(),
        "expected player to fail, got {}\n{logs}",
        output.status
    );
    assert!(
        logs.contains(expected),
        "expected failure output to contain {expected:?}\n{logs}"
    );
}

fn process_output_text(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn write(prefix: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "aes67-player-{prefix}-{}-{}.sdp",
            std::process::id(),
            unique_suffix()
        ));
        fs::write(&path, contents).expect("temp SDP should be writable");

        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after Unix epoch")
        .as_nanos()
}
