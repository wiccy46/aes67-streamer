# E2E Tests

This directory contains end-to-end tests for the AES67 streamer.

## Running tests

```bash
cargo test --test e2e_stream_test
```

Future improvements:
- Use loopback interface for streaming + recording
- Compare recorded PCM to input WAV using sox or hound
- GStreamer pipeline for RTP receive in CI
- CI matrix for different OS/network setups
