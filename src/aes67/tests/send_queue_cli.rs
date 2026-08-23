use semver::Version;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_aes67")
}

fn version_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../VERSION")
}

fn version_from_file() -> String {
    fs::read_to_string(version_file())
        .expect("VERSION file should exist")
        .trim()
        .to_string()
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("aes67-send-{name}-{}", std::process::id()));
    fs::remove_dir_all(&path).ok();
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

#[test]
fn version_flag_exits_successfully() {
    let output = Command::new(binary())
        .arg("-V")
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("aes67 {}", version_from_file()))
    );
}

#[test]
fn version_file_is_valid_semver() {
    let version = version_from_file();
    Version::parse(&version).expect("VERSION should be valid SemVer");
}

#[test]
fn send_queue_help_exits_successfully() {
    let output = Command::new(binary())
        .arg("send")
        .arg("queue")
        .arg("--help")
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AES67 Send Queue"));
    assert!(stdout.contains("queueing audio files to send"));
    assert!(!stdout.contains("--interface"));
    assert!(!stdout.contains("--address"));
    assert!(!stdout.contains("--port"));
}

#[test]
fn send_queue_opens_ui_and_persists_settings() {
    let config_dir = unique_temp_dir("queue-settings");
    let mut child = Command::new(binary())
        .arg("send")
        .arg("queue")
        .env("AES67_SEND_QUEUE_CONFIG_DIR", &config_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("binary should start");

    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(b"q\n")
        .expect("quit command should write");

    let output = child.wait_with_output().expect("binary should exit");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert!(String::from_utf8_lossy(&output.stdout).contains("AES67 Send Queue"));

    let settings_path = config_dir.join("send-queue.toml");
    let settings = fs::read_to_string(&settings_path).expect("settings should be persisted");
    assert!(settings.contains("address"));

    fs::remove_dir_all(config_dir).ok();
}
