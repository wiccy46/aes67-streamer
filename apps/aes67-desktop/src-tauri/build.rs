use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let version_path = manifest_dir.join("../../../VERSION");
    println!("cargo:rerun-if-changed={}", version_path.display());

    let version = fs::read_to_string(&version_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", version_path.display()));
    let version = version.trim();
    let package_version = env::var("CARGO_PKG_VERSION").expect("Cargo package version");
    assert_eq!(
        version, package_version,
        "desktop Cargo version must match root VERSION"
    );
    println!("cargo:rustc-env=AES67_TOOLS_VERSION={version}");

    tauri_build::build();
}
