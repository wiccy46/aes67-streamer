#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn receive_soak_dry_run_reports_default_configuration() {
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/receive_soak_loopback.sh"))
        .arg("--dry-run")
        .env_remove("AES67_RECEIVE_SOAK_DURATION_SECONDS")
        .output()
        .expect("script should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Receive soak loopback configuration"));
    assert!(stdout.contains("Duration:    60 seconds"));
    assert!(stdout.contains("Latency:     50 ms"));
}

#[test]
fn receive_soak_dry_run_rejects_zero_duration() {
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/receive_soak_loopback.sh"))
        .arg("--dry-run")
        .env("AES67_RECEIVE_SOAK_DURATION_SECONDS", "0")
        .output()
        .expect("script should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("AES67_RECEIVE_SOAK_DURATION_SECONDS"),
        "stderr should explain invalid duration, got: {stderr}"
    );
}

#[test]
fn receive_soak_passes_clean_summary_with_expected_null_output_warning() {
    let temp = TempDir::new("clean");
    let receiver = temp.path().join("fake-receiver");
    let sender = temp.path().join("fake-sender");
    let cargo = temp.path().join("fake-cargo");

    write_fake_receiver(&receiver, CleanSummary::Pass);
    write_executable(&sender, "#!/usr/bin/env bash\nexit 0\n");
    write_executable(&cargo, "#!/usr/bin/env bash\nexit 0\n");

    let output = run_script(&temp, &cargo, &receiver, &sender);
    let logs = output_text(&output);

    assert!(
        output.status.success(),
        "clean summary should pass soak validation\n{logs}"
    );
    assert!(logs.contains("Receive soak loopback passed"));
    assert!(logs.contains("Packets decoded: 12"));
}

#[test]
fn receive_soak_fails_on_unexpected_warning() {
    let temp = TempDir::new("unexpected-warn");
    let receiver = temp.path().join("fake-receiver");
    let sender = temp.path().join("fake-sender");
    let cargo = temp.path().join("fake-cargo");

    write_fake_receiver(&receiver, CleanSummary::UnexpectedWarning);
    write_executable(&sender, "#!/usr/bin/env bash\nexit 0\n");
    write_executable(&cargo, "#!/usr/bin/env bash\nexit 0\n");

    let output = run_script(&temp, &cargo, &receiver, &sender);
    let logs = output_text(&output);

    assert!(
        !output.status.success(),
        "unexpected receiver warning should fail soak validation\n{logs}"
    );
    assert!(logs.contains("Receiver emitted unexpected warning logs"));
    assert!(logs.contains("RTP packets fell behind playout"));
}

#[test]
fn receive_soak_fails_on_summary_counter_problem() {
    let temp = TempDir::new("summary-counter");
    let receiver = temp.path().join("fake-receiver");
    let sender = temp.path().join("fake-sender");
    let cargo = temp.path().join("fake-cargo");

    write_fake_receiver(&receiver, CleanSummary::LatePacket);
    write_executable(&sender, "#!/usr/bin/env bash\nexit 0\n");
    write_executable(&cargo, "#!/usr/bin/env bash\nexit 0\n");

    let output = run_script(&temp, &cargo, &receiver, &sender);
    let logs = output_text(&output);

    assert!(
        !output.status.success(),
        "non-zero jitter counter should fail soak validation\n{logs}"
    );
    assert!(logs.contains("Jitter buffer reported 1 late packets"));
}

fn run_script(temp: &TempDir, cargo: &Path, receiver: &Path, sender: &Path) -> Output {
    Command::new("bash")
        .arg(repo_root().join("scripts/receive_soak_loopback.sh"))
        .env("AES67_RECEIVE_SOAK_CARGO", cargo)
        .env("AES67_RECEIVE_SOAK_BIN", receiver)
        .env("AES67_SEND_BIN", sender)
        .env(
            "AES67_RECEIVE_SOAK_ARTIFACT_DIR",
            temp.path().join("artifacts"),
        )
        .env("AES67_RECEIVE_SOAK_DURATION_SECONDS", "1")
        .env("AES67_RECEIVE_SOAK_LATENCY_MS", "10")
        .output()
        .expect("script should run")
}

#[derive(Clone, Copy)]
enum CleanSummary {
    Pass,
    UnexpectedWarning,
    LatePacket,
}

fn write_fake_receiver(path: &Path, mode: CleanSummary) {
    let late_packets = match mode {
        CleanSummary::LatePacket => 1,
        CleanSummary::Pass | CleanSummary::UnexpectedWarning => 0,
    };
    let unexpected_warning = match mode {
        CleanSummary::UnexpectedWarning => {
            "[2026-05-24T00:00:00Z WARN  aes67_engine::receiver::receiver] RTP packets fell behind playout\n"
        }
        CleanSummary::Pass | CleanSummary::LatePacket => "",
    };
    let contents = format!(
        "#!/usr/bin/env bash\n\
         sleep 2\n\
         cat >&2 <<'LOG'\n\
         [2026-05-24T00:00:00Z WARN  aes67_engine::receiver::receiver] Using internal null audio output for test validation\n\
         {unexpected_warning}\
         [INFO aes67_engine::receiver::receiver]   Packets decoded: 12\n\
         [INFO aes67_engine::receiver::receiver]   RTP silence frames: 0\n\
         [INFO aes67_engine::receiver::receiver]   Jitter lost packets: 0\n\
         [INFO aes67_engine::receiver::receiver]   Jitter late packets: {late_packets}\n\
         [INFO aes67_engine::receiver::receiver]   Jitter duplicate packets: 0\n\
         [INFO aes67_engine::receiver::receiver]   Jitter dropped-full packets: 0\n\
         [INFO aes67_engine::receiver::receiver]   Jitter timestamp discontinuities: 0\n\
         [INFO aes67_engine::receiver::receiver]   Output silence frames: 0\n\
         [INFO aes67_engine::receiver::receiver]   Output dropped samples: 0\n\
         LOG\n",
    );
    write_executable(path, &contents);
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should be reachable")
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("test executable should be writable");
    let mut permissions = fs::metadata(path)
        .expect("test executable metadata should be readable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("test executable should be executable");
}

fn output_text(output: &Output) -> String {
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
            "aes67-receive-soak-script-{label}-{}-{}",
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
