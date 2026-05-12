use std::process::Command;
use std::time::Duration;
use std::thread;
use assert_cmd::prelude::*;
use tempfile::tempdir;

#[test]
fn test_binary_builds_and_runs() {
    let mut cmd = Command::cargo_bin("aes67-streamer").unwrap();
    cmd.arg("--help");
    cmd.assert().success();
}

#[test]
fn test_stream_playback_and_record() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempdir()?;
    let test_wav = "tests/test_clip.wav"; // short 5-10s clip recommended

    // Spawn streamer in background (in real test, use timeout or signal)
    let mut streamer = Command::cargo_bin("aes67-streamer")?
        .arg("--file").arg(test_wav)
        .arg("--address").arg("239.69.69.69") // multicast or 127.0.0.1
        .arg("--port").arg("5004")
        .arg("--duration").arg("5") // assume added flag for short test
        .spawn()?;

    // Give time to start streaming
    thread::sleep(Duration::from_secs(2));

    // Record RTP stream - use ffmpeg or rtpdump on Linux
    // For pure Rust: use rtp crate to capture UDP and decode
    println!("Recording RTP stream for comparison...");

    // Placeholder: In CI use ffmpeg to capture to WAV and compare
    // e.g. ffmpeg -i rtp://239.69.69.69:5004?localaddr=0.0.0.0 -t 5 recorded.wav

    // Kill streamer
    let _ = streamer.kill();

    // TODO: Compare PCM data from original and recorded
    // Use hound crate for WAV PCM extraction and assert_eq!

    Ok(())
}
