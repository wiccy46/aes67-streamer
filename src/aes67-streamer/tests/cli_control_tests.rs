use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_aes67-streamer")
}

#[test]
fn version_flag_exits_successfully() {
    let output = Command::new(binary())
        .arg("-V")
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert!(String::from_utf8_lossy(&output.stdout).contains("aes67-streamer 0.1.0"));
}
