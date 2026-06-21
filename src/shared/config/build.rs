use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let version_file = manifest_dir.join("../../../VERSION");

    println!("cargo:rerun-if-changed={}", version_file.display());

    let version = fs::read_to_string(&version_file)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", version_file.display()));
    let version = version.trim();

    assert!(!version.is_empty(), "VERSION must not be empty");
    println!("cargo:rustc-env=AES67_TOOLS_VERSION={version}");
}
