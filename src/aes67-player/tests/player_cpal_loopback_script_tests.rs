#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn cpal_loopback_rejects_unclocked_output_before_streaming() {
    let temp = TempDir::new("reject-unclocked");
    let player = temp.path().join("fake-player");
    let streamer = temp.path().join("fake-streamer");
    let cargo = temp.path().join("fake-cargo");
    let streamer_marker = temp.path().join("streamer-ran");

    write_executable(
        &player,
        "#!/usr/bin/env bash\n\
         echo \"[INFO aes67_player::output] Created CPAL output on 'Discard all samples (playback) or generate zero samples (capture)' at 48000 Hz, 2 channels, F32, 48000 sample buffer\" >&2\n\
         sleep 5\n",
    );
    write_executable(
        &streamer,
        &format!(
            "#!/usr/bin/env bash\n touch '{}'\n",
            streamer_marker.display()
        ),
    );
    write_executable(&cargo, "#!/usr/bin/env bash\nexit 0\n");

    let output = run_script(&temp, &cargo, &player, &streamer, &[]);
    let logs = output_text(&output);

    assert!(
        !output.status.success(),
        "script should reject unclocked output\n{logs}"
    );
    assert!(
        logs.contains("unclocked null/discard device"),
        "script should explain why the device is rejected\n{logs}"
    );
    assert!(
        logs.contains("Discard all samples"),
        "script should include captured player log output\n{logs}"
    );
    assert!(
        !streamer_marker.exists(),
        "streamer should not run after selecting an unclocked output"
    );
}

#[test]
fn cpal_loopback_startup_failure_prints_player_error() {
    let temp = TempDir::new("startup-failure");
    let player = temp.path().join("fake-player");
    let streamer = temp.path().join("fake-streamer");
    let cargo = temp.path().join("fake-cargo");

    write_executable(
        &player,
        "#!/usr/bin/env bash\n\
         echo \"[ERROR aes67_player] Failed to create AES67 player: fake device unavailable\" >&2\n\
         exit 1\n",
    );
    write_executable(&streamer, "#!/usr/bin/env bash\nexit 0\n");
    write_executable(&cargo, "#!/usr/bin/env bash\nexit 0\n");

    let output = run_script(&temp, &cargo, &player, &streamer, &[]);
    let logs = output_text(&output);

    assert!(
        !output.status.success(),
        "script should fail when player exits during startup\n{logs}"
    );
    assert!(
        logs.contains("aes67-player exited before the streamer started"),
        "script should report the startup phase\n{logs}"
    );
    assert!(
        logs.contains("fake device unavailable"),
        "script should print the captured player error\n{logs}"
    );
}

#[test]
fn cpal_loopback_can_allow_unclocked_output_for_diagnostics() {
    let temp = TempDir::new("allow-unclocked");
    let player = temp.path().join("fake-player");
    let streamer = temp.path().join("fake-streamer");
    let cargo = temp.path().join("fake-cargo");
    let streamer_marker = temp.path().join("streamer-ran");

    write_executable(
        &player,
        "#!/usr/bin/env bash\n\
         echo \"[INFO aes67_player::output] Created CPAL output on 'Discard all samples (playback) or generate zero samples (capture)' at 48000 Hz, 2 channels, F32, 48000 sample buffer\" >&2\n\
         sleep 2\n\
         cat >&2 <<'LOG'\n\
         [INFO aes67_player::player]   Packets decoded: 12\n\
         [INFO aes67_player::player]   RTP silence frames: 0\n\
         [INFO aes67_player::player]   Jitter lost packets: 0\n\
         [INFO aes67_player::player]   Jitter late packets: 0\n\
         [INFO aes67_player::player]   Jitter dropped-full packets: 0\n\
         [INFO aes67_player::player]   Jitter timestamp discontinuities: 0\n\
         [INFO aes67_player::player]   Output silence frames: 0\n\
         [INFO aes67_player::player]   Output dropped samples: 0\n\
         LOG\n",
    );
    write_executable(
        &streamer,
        &format!(
            "#!/usr/bin/env bash\n touch '{}'\n",
            streamer_marker.display()
        ),
    );
    write_executable(&cargo, "#!/usr/bin/env bash\nexit 0\n");

    let output = run_script(
        &temp,
        &cargo,
        &player,
        &streamer,
        &[("AES67_PLAYER_ALLOW_UNCLOCKED_OUTPUT", "1")],
    );
    let logs = output_text(&output);

    assert!(
        output.status.success(),
        "diagnostic override should allow the loopback to complete\n{logs}"
    );
    assert!(
        logs.contains("smoothness results are diagnostic only"),
        "script should warn that the selected device is diagnostic only\n{logs}"
    );
    assert!(
        logs.contains("CPAL player loopback passed"),
        "script should report success after clean summary counters\n{logs}"
    );
    assert!(
        streamer_marker.exists(),
        "streamer should run when the diagnostic override is enabled"
    );
}

fn run_script(
    temp: &TempDir,
    cargo: &Path,
    player: &Path,
    streamer: &Path,
    extra_env: &[(&str, &str)],
) -> Output {
    let mut command = Command::new("bash");
    command
        .arg(repo_root().join("scripts/player_cpal_loopback.sh"))
        .env("AES67_PLAYER_CARGO", cargo)
        .env("AES67_PLAYER_PLAYER_BIN", player)
        .env("AES67_PLAYER_STREAMER_BIN", streamer)
        .env("AES67_PLAYER_ARTIFACT_DIR", temp.path().join("artifacts"))
        .env("AES67_PLAYER_DURATION_SECONDS", "1")
        .env("AES67_PLAYER_LATENCY_MS", "10")
        .env("AES67_PLAYER_OUTPUT_DEVICE", "0");

    for (key, value) in extra_env {
        command.env(key, value);
    }

    command.output().expect("script should run")
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
            "aes67-player-cpal-script-{label}-{}-{}",
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
