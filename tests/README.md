# Tests for AES67 Streamer

## Running locally

```bash
cargo test --workspace
bash scripts/e2e_loopback.sh
```

## Full Media Loopback E2E

`scripts/e2e_loopback.sh` is the local and CI entry point for end-to-end media testing. It:

- Builds `aes67-streamer`.
- Generates a short 48 kHz stereo WAV fixture.
- Starts `ffmpeg` as an RTP receiver from an SDP file.
- Runs the streamer for a bounded duration on loopback.
- Validates the recorded WAV with `ffprobe`.

Requirements:

- `ffmpeg`
- `ffprobe`
- Local UDP loopback networking

## CI

GitHub Actions on `ubuntu-latest` runs `cargo test --workspace` and `bash scripts/e2e_loopback.sh`.
