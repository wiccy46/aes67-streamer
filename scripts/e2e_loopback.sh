#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ARTIFACT_DIR="${AES67_E2E_ARTIFACT_DIR:-target/e2e-loopback}"
INPUT_WAV="$ARTIFACT_DIR/input-48k-stereo.wav"
SDP_FILE="$ARTIFACT_DIR/stream.sdp"
RECORDED_WAV="$ARTIFACT_DIR/recorded.wav"
STREAMER_LOG="$ARTIFACT_DIR/streamer.log"
FFMPEG_LOG="$ARTIFACT_DIR/ffmpeg-receiver.log"
VOLUME_LOG="$ARTIFACT_DIR/volume.log"

ADDRESS="${AES67_E2E_ADDRESS:-127.0.0.1}"
PORT="${AES67_E2E_PORT:-55004}"
INTERFACE="${AES67_E2E_INTERFACE:-127.0.0.1}"
DURATION_SECONDS="${AES67_E2E_DURATION_SECONDS:-2}"
PAYLOAD_TYPE=97

receiver_pid=""

cleanup() {
    if [[ -n "$receiver_pid" ]] && kill -0 "$receiver_pid" 2>/dev/null; then
        kill "$receiver_pid" 2>/dev/null || true
        wait "$receiver_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing required command: $1" >&2
        exit 1
    fi
}

fail_with_logs() {
    echo "$1" >&2
    echo "--- streamer log ---" >&2
    tail -n 80 "$STREAMER_LOG" 2>/dev/null >&2 || true
    echo "--- ffmpeg receiver log ---" >&2
    tail -n 120 "$FFMPEG_LOG" 2>/dev/null >&2 || true
    echo "--- volume log ---" >&2
    tail -n 80 "$VOLUME_LOG" 2>/dev/null >&2 || true
    exit 1
}

require_command cargo
require_command ffmpeg
require_command ffprobe
require_command awk

mkdir -p "$ARTIFACT_DIR"
rm -f "$INPUT_WAV" "$SDP_FILE" "$RECORDED_WAV" "$STREAMER_LOG" "$FFMPEG_LOG" "$VOLUME_LOG"

echo "Building aes67-streamer..."
cargo build -p aes67-streamer

echo "Generating deterministic test WAV..."
# Generate known-good 48 kHz stereo PCM so the receiver validation can assert
# sample rate, channels, duration, and non-silence deterministically.
ffmpeg -nostdin -y -v error \
    -f lavfi -i "sine=frequency=997:duration=${DURATION_SECONDS}:sample_rate=48000" \
    -filter:a "pan=stereo|c0=c0|c1=c0" \
    -c:a pcm_s24le \
    "$INPUT_WAV"

# ffmpeg receives RTP from SDP, so keep this in sync with the streamer's
# payload type, packet time, sample rate, channel count, address, and port.
cat > "$SDP_FILE" <<SDP
v=0
o=- 123456 123456 IN IP4 ${INTERFACE}
s=AES67 Streamer E2E
c=IN IP4 ${ADDRESS}
t=0 0
m=audio ${PORT} RTP/AVP ${PAYLOAD_TYPE}
a=rtpmap:${PAYLOAD_TYPE} L24/48000/2
a=ptime:1
a=recvonly
SDP

echo "Starting ffmpeg RTP receiver on ${ADDRESS}:${PORT}..."
# Start the receiver before the streamer to avoid dropping the first RTP burst.
# localaddr is required for multicast loopback so ffmpeg joins/listens on the
# same interface that the streamer uses.
ffmpeg -nostdin -y -v warning \
    -protocol_whitelist file,udp,rtp \
    -localaddr "$INTERFACE" \
    -i "$SDP_FILE" \
    -t "$DURATION_SECONDS" \
    -c:a pcm_s24le \
    "$RECORDED_WAV" >"$FFMPEG_LOG" 2>&1 &
receiver_pid=$!

sleep 1

echo "Running aes67-streamer for ${DURATION_SECONDS}s..."
# Run a bounded stream so this script is deterministic in CI and local smoke
# tests without requiring manual signal handling.
RUST_LOG=info target/debug/aes67-streamer \
    --file "$INPUT_WAV" \
    --address "$ADDRESS" \
    --port "$PORT" \
    --interface "$INTERFACE" \
    --duration-seconds "$DURATION_SECONDS" >"$STREAMER_LOG" 2>&1 || fail_with_logs "Streamer failed"

deadline=$((SECONDS + 10))
while kill -0 "$receiver_pid" 2>/dev/null; do
    if (( SECONDS > deadline )); then
        fail_with_logs "Timed out waiting for ffmpeg receiver"
    fi
    sleep 0.2
done

wait "$receiver_pid" || fail_with_logs "ffmpeg receiver failed"
receiver_pid=""

[[ -s "$RECORDED_WAV" ]] || fail_with_logs "Recorded WAV was not created"

# Validate the media structurally before checking loudness. ffmpeg can create a
# header-only WAV when no RTP packets are decoded; reject missing duration,
# unreadable frame counts, and empty-output receiver logs explicitly.
sample_rate="$(ffprobe -v error -select_streams a:0 -show_entries stream=sample_rate -of default=nw=1:nk=1 "$RECORDED_WAV")"
channels="$(ffprobe -v error -select_streams a:0 -show_entries stream=channels -of default=nw=1:nk=1 "$RECORDED_WAV")"
duration="$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$RECORDED_WAV")"
read_frames="$(ffprobe -v error -count_frames -select_streams a:0 -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$RECORDED_WAV")"

[[ "$sample_rate" == "48000" ]] || fail_with_logs "Expected 48000 Hz recording, got ${sample_rate}"
[[ "$channels" == "2" ]] || fail_with_logs "Expected stereo recording, got ${channels} channel(s)"
[[ -n "$duration" && "$duration" != "N/A" ]] || fail_with_logs "Recording duration was not detected"
[[ -n "$read_frames" && "$read_frames" != "N/A" && "$read_frames" != "0" ]] || fail_with_logs "Recording contains no readable audio frames"
if grep -q "Output file is empty" "$FFMPEG_LOG"; then
    fail_with_logs "ffmpeg receiver produced an empty output file"
fi
awk -v duration="$duration" 'BEGIN { exit !(duration >= 0.5) }' || fail_with_logs "Recording duration too short: ${duration}s"

# Validate that the decoded audio has samples and is not silent. volumedetect may
# print an initial zero sample count during setup, so use the final count.
ffmpeg -nostdin -v info -i "$RECORDED_WAV" -af volumedetect -f null - >"$VOLUME_LOG" 2>&1 || fail_with_logs "Volume validation failed"
volume_samples="$(awk '/n_samples:/ { samples=$NF } END { print samples }' "$VOLUME_LOG")"
[[ -n "$volume_samples" && "$volume_samples" != "0" ]] || fail_with_logs "Recorded WAV contains zero audio samples"
max_volume="$(awk '/max_volume:/ { print $NF }' "$VOLUME_LOG")"
[[ -n "$max_volume" ]] || fail_with_logs "Recorded WAV volume could not be measured"
if [[ "$max_volume" == "-inf" ]]; then
    fail_with_logs "Recorded WAV is silent"
fi

echo "E2E loopback passed"
echo "Artifacts: $ARTIFACT_DIR"
echo "Recorded: ${duration}s, ${sample_rate} Hz, ${channels} channel(s)"
