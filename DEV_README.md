# AES67 Tools Development Guide

This document is for contributors and release maintainers. The user-facing
README is intentionally focused on installing and running the tools.

## Workspace Overview

This is a Cargo workspace with one shipped application, one diagnostic tool,
and shared libraries:

- `src/aes67`: the only shipped command-line application. It exposes Send and
  Receive directly through `aes67-engine`.
- `src/shared/aes67-engine`: Send (file and queue) and Receive (discovery,
  listen, device listing) workflows shared by present and future interfaces.
- `src/tools/aes67-route-test`: developer/installer route diagnostic; not a product line
  or packaged command.
- `src/shared/config`: Send, Receive, discovery, and route-diagnostic CLI parsing and Send TOML
  configuration.
- `src/shared/audio`: audio decoding, resampling, sample buffering, gain node
  processing.
- `src/shared/network`: RTP packetization/parsing, SDP/SAP, jitter buffer, UDP
  sockets.
- `src/shared/ptp`: PTP client, message parsing, clock/timestamp support.

## Send Flow

1. Parse Send File CLI arguments in `config`.
2. Load TOML config if provided.
3. Decode and resample audio to the AES67 target format.
4. Process audio through the node chain.
5. Packetize as RTP L24.
6. Send over UDP with configured packet timing.
7. Run PTP and SAP background tasks while streaming.

## Receive Flow

1. Parse Receive Listen CLI arguments in `config`.
2. Build receive session from SDP or basic CLI address/port arguments.
3. Receive RTP over UDP.
4. Reorder packets through the jitter buffer.
5. Decode L24 RTP payloads into interleaved samples.
6. Play through CPAL output or null output in tests.
7. Log final playback summary and warnings for smoothness issues.

## Receive Discovery Flow

1. Parse Receive Discover CLI arguments in `config`.
2. Resolve the selected interface name or IPv4 address.
3. Bind the SAP browser socket to `239.255.255.255:9875`.
4. Parse SAP datagrams into reusable `network::sap` message types.
5. Parse `application/sdp` payloads into AES67 session descriptions.
6. Track added, updated, removed, and expired streams.
7. Print browse-style event lines and optionally write discovered SDP files.

## Canonical Command Flow

The documented user interface is the `aes67` hierarchy:

```text
aes67 send file|queue
aes67 receive discover|listen|devices
```

The release artifact contains only `aes67`; the front door calls engine
workflows in-process, with no companion executable requirement.

## Development Setup

Install Rust stable and platform build dependencies.

On Ubuntu/Debian:

```bash
sudo apt-get update
sudo apt-get install -y ffmpeg libasound2-dev pkg-config
```

On Fedora:

```bash
sudo dnf install ffmpeg alsa-lib-devel pkgconf-pkg-config
```

On macOS, install Rust and Homebrew tooling as needed. The local media loopback
script requires `ffmpeg` and `ffprobe`.

## Common Checks

Run these before opening release-related changes:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Focused checks:

```bash
cargo test -p config
cargo test -p aes67 --test send_e2e
cargo test -p aes67 --test receive_e2e
cargo test -p aes67 --test receive_cli_failure
cargo test -p aes67 --test discovery_e2e
cargo test -p network sdp
cargo test -p network sap
cargo test -p network jitter
```

Scripted local checks:

```bash
bash scripts/e2e_loopback.sh
bash scripts/e2e_multicast.sh --dry-run
bash scripts/soak_loopback.sh --dry-run
```

The whole repository may contain pre-existing formatting drift. Prefer checking
touched files unless doing an intentional formatting-only change.

## Full Media Loopback E2E

`scripts/e2e_loopback.sh` is the Send media loopback entry point. It:

1. Builds `aes67`.
2. Generates a deterministic 48 kHz stereo WAV with `ffmpeg`.
3. Writes an SDP file for RTP L24/48000/2.
4. Starts `ffmpeg` as a receiver.
5. Runs `aes67 send file` for a bounded duration.
6. Validates the recorded WAV with `ffprobe` and volume detection.

Useful overrides:

```bash
AES67_E2E_DURATION_SECONDS=3 bash scripts/e2e_loopback.sh
AES67_E2E_PORT=55010 bash scripts/e2e_loopback.sh
AES67_E2E_ADDRESS=239.69.67.67 bash scripts/e2e_loopback.sh
```

The default address is `127.0.0.1` for CI reliability. Use multicast overrides
only when validating multicast behavior specifically.

## Versioning

The public binary version follows SemVer and is recorded in the root `VERSION`
file.

`VERSION` is the only source of truth for public release versions. The
`config` crate build script reads it for internal command parsers. The `aes67`
build script injects the same root version into the shipped command.

The release workflow validates that `VERSION` is valid SemVer and that
`CHANGELOG.md` contains a matching section.

## Release Workflow

The normal release path is the manual GitHub Actions workflow:

```text
.github/workflows/release-run.yml
```

Before running it:

1. Finish and merge the code change.
2. Update `CHANGELOG.md`.
3. Bump `VERSION`.
4. Make sure the GitHub secret `HOMEBREW_TAP_TOKEN` has write access to
   `wiccy46/homebrew-aes67`.

Run it from GitHub:

```text
Actions -> Release Run -> Run workflow
```

Use `dry_run=true` first. A dry run validates metadata, runs tests and clippy,
and builds both release packages without tagging, creating a GitHub release, or
pushing the Homebrew tap.

For a real release, leave `update_homebrew=true`. That is the default release
path: the workflow creates or updates the GitHub release, uploads the archives,
then commits the generated formula to `wiccy46/homebrew-aes67`. Set
`update_homebrew=false` only for an emergency release where the tap will be
updated separately.

One-time Homebrew automation setup:

1. Create a fine-grained GitHub token with access to `wiccy46/homebrew-aes67`.
2. Grant `Contents: Read and write` permission.
3. Add it to `wiccy46/aes67-tools` as an Actions repository secret named
   `HOMEBREW_TAP_TOKEN`.

The workflow validates this secret before publishing a non-dry-run release when
`update_homebrew=true`, so a missing token fails before a GitHub release is
created.

The workflow builds and publishes:

- `aes67-tools-<version>-aarch64-apple-darwin.tar.gz`
- `aes67-tools-<version>-aarch64-apple-darwin.tar.gz.sha256`
- `aes67-tools-<version>-x86_64-unknown-linux-gnu.tar.gz`
- `aes67-tools-<version>-x86_64-unknown-linux-gnu.tar.gz.sha256`

Then it creates or updates GitHub release `v<VERSION>`. If `update_homebrew` is
true, it also updates the Homebrew tap formula in `wiccy46/homebrew-aes67`.

## Local Release Debugging

Check release metadata locally:

```bash
bash scripts/check_release_ready.sh
```

Build a local package for the host target:

```bash
bash scripts/package_release.sh
```

Dry-run specific targets:

```bash
AES67_RELEASE_TARGET=aarch64-apple-darwin bash scripts/package_release.sh --dry-run
AES67_RELEASE_TARGET=x86_64-unknown-linux-gnu bash scripts/package_release.sh --dry-run
```

Update a checked-out Homebrew formula from local checksum files:

```bash
bash scripts/update_homebrew_formula.sh \
  /Users/jiajun.yang/dev/homebrew-aes67/Formula/aes67-tools.rb \
  0.1.0 \
  target/release-packages/aes67-tools-0.1.0-aarch64-apple-darwin.tar.gz.sha256 \
  target/release-packages/aes67-tools-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
```

## Release Package Layout

The tarball contains:

- `bin/aes67`
- `README.md`
- `LICENSE`
- `VERSION`
- `examples/send-file.toml`
- `examples/example.sdp`

The package script writes archives and checksums under:

```text
target/release-packages/
```

## CI

GitHub Actions workflow:

```text
.github/workflows/ci.yml
```

CI runs on Ubuntu, installs ALSA development headers and `ffmpeg`, builds the
workspace, runs tests, and runs the full media loopback E2E script.

## Networking Notes

- Tests that bind UDP sockets may need elevated sandbox permissions in Codex.
- Loopback E2E uses unicast `127.0.0.1` by default.
- Multicast tests validate RTP packet receipt over multicast loopback where
  supported.
- SAP announcements are sent to `239.255.255.255:9875`.
- PTP sockets use ports `319` and `320`.
- DSCP marking sets packet fields only; the network must be configured to honor
  DSCP for actual prioritization.

## Files To Know

- Canonical product CLI: `src/aes67/src/main.rs`
- Send/Receive product workflows: `src/shared/aes67-engine/src/`
- Send/Receive/discovery CLI parsing: `src/shared/config/src/args.rs`
- Send orchestration: `src/shared/aes67-engine/src/sender/engine.rs`
- Receive orchestration: `src/shared/aes67-engine/src/receiver/session.rs`
- Local output adapter: `src/shared/aes67-engine/src/receiver/output.rs`
- RTP packetizer/parser: `src/shared/network/src/rtp.rs`
- SDP parser: `src/shared/network/src/sdp.rs`
- Jitter buffer: `src/shared/network/src/jitter.rs`
- UDP socket wrappers: `src/shared/network/src/socket.rs`
- SAP announcer: `src/shared/network/src/sap.rs`
- PTP client: `src/shared/ptp/src/client.rs`
- Release package script: `scripts/package_release.sh`
- Release workflow: `.github/workflows/release-run.yml`
- Homebrew formula template: `packaging/homebrew/aes67-tools.rb`

## Development Guidelines

- Prefer small, focused changes.
- Add or update tests before changing behavior where practical.
- Keep changes scoped to the relevant crate.
- Preserve existing workspace structure.
- Use structured parsing and existing helper APIs instead of ad hoc string
  handling.
- Avoid unrelated formatting churn.
- Use clear errors and logs for network/setup failures.
- Be precise with AES67, RTP, SDP, SAP, QoS, and PTP terminology.
