use semver::Version;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_aes67-streamer")
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

#[test]
fn version_flag_exits_successfully() {
    let output = Command::new(binary())
        .arg("-V")
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains(&format!("aes67-streamer {}", version_from_file())));
}

#[test]
fn version_file_is_valid_semver() {
    let version = version_from_file();
    Version::parse(&version).expect("VERSION should be valid SemVer");
}
