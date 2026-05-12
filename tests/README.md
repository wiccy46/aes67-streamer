# E2E Tests for AES67 Streamer

## Running locally

```bash
cargo test --test e2e_stream_test -- --nocapture
```

## Requirements for full E2E
- Short test WAV file in `tests/`
- Loopback networking
- Tools: `ffmpeg`, `gst-launch-1.0` or pure Rust receiver

## CI
GitHub Actions on ubuntu-latest will run basic + full loopback tests.
