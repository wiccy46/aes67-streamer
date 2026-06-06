use std::fs;
use std::path::PathBuf;

#[test]
fn homebrew_formula_points_to_release_archives_for_supported_targets() {
    let formula = read_formula();

    for target in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ] {
        let archive = format!("aes67-tools-#{{version}}-{target}.tar.gz");
        assert!(
            formula.contains(&archive),
            "formula should reference release archive {archive}"
        );
    }

    for placeholder in [
        "REPLACE_WITH_AARCH64_APPLE_DARWIN_SHA256",
        "REPLACE_WITH_X86_64_APPLE_DARWIN_SHA256",
        "REPLACE_WITH_AARCH64_UNKNOWN_LINUX_GNU_SHA256",
        "REPLACE_WITH_X86_64_UNKNOWN_LINUX_GNU_SHA256",
    ] {
        assert!(
            formula.contains(placeholder),
            "formula should keep replacement placeholder {placeholder}"
        );
    }
}

#[test]
fn homebrew_formula_installs_binaries_docs_and_examples() {
    let formula = read_formula();

    for expected in [
        "license \"GPL-3.0-only\"",
        "bin.install \"bin/aes67-streamer\"",
        "bin.install \"bin/aes67-player\"",
        "doc.install \"README.md\"",
        "doc.install \"LICENSE\"",
        "pkgshare.install \"examples\"",
    ] {
        assert!(
            formula.contains(expected),
            "formula should include `{expected}`"
        );
    }
}

#[test]
fn homebrew_formula_smoke_tests_public_binaries() {
    let formula = read_formula();

    assert!(formula.contains("test do"));
    assert!(formula.contains("shell_output(\"#{bin}/aes67-streamer --version\")"));
    assert!(formula.contains("shell_output(\"#{bin}/aes67-player --version\")"));
}

fn read_formula() -> String {
    fs::read_to_string(repo_root().join("packaging/homebrew/aes67-tools.rb"))
        .expect("Homebrew formula should be readable")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root should be reachable")
}
