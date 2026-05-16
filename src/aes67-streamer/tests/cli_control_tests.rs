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

#[test]
fn version_flag_exits_successfully() {
    let output = Command::new(binary())
        .arg("-V")
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains(&format!("aes67-streamer {}", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn version_file_matches_binary_package_version_and_is_valid_semver() {
    let version = fs::read_to_string(version_file()).expect("VERSION file should exist");
    let version = version.trim();

    Version::parse(version).expect("VERSION should be valid SemVer");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}
