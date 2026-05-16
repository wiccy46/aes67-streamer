#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: AES67_E2E_INTERFACE=<local-ip> bash scripts/e2e_multicast.sh [--dry-run]

Optional multicast interoperability test for local/pro network validation.
This script is intentionally not part of CI because multicast routing and
interface behavior depends on the host and network.

Environment:
  AES67_E2E_INTERFACE         Required local interface IPv4 address for ffmpeg localaddr and streamer --interface
  AES67_E2E_ADDRESS           Multicast group, default 239.69.67.67
  AES67_E2E_PORT              RTP port, default 55004
  AES67_E2E_DURATION_SECONDS  Stream and receive duration, default 2
  AES67_E2E_ARTIFACT_DIR      Artifact directory, default target/e2e-multicast
USAGE
}

DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --dry-run)
            DRY_RUN=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            usage >&2
            exit 2
            ;;
    esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ARTIFACT_DIR="${AES67_E2E_ARTIFACT_DIR:-target/e2e-multicast}"
INPUT_WAV="$ARTIFACT_DIR/input-48k-stereo.wav"
SDP_FILE="$ARTIFACT_DIR/stream.sdp"
RECORDED_WAV="$ARTIFACT_DIR/recorded.wav"
STREAMER_LOG="$ARTIFACT_DIR/streamer.log"
FFMPEG_LOG="$ARTIFACT_DIR/ffmpeg-receiver.log"
VOLUME_LOG="$ARTIFACT_DIR/volume.log"

ADDRESS="${AES67_E2E_ADDRESS:-239.69.67.67}"
PORT="${AES67_E2E_PORT:-55004}"
INTERFACE="${AES67_E2E_INTERFACE:-}"
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

validate_ipv4() {
    local value="$1"
    local label="$2"

    if ! [[ "$value" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; then
        echo "$label must be an IPv4 address, got: $value" >&2
        exit 2
    fi

    IFS='.' read -r o1 o2 o3 o4 <<<"$value"
    for octet in "$o1" "$o2" "$o3" "$o4"; do
        if (( octet < 0 || octet > 255 )); then
            echo "$label contains an invalid IPv4 octet: $value" >&2
            exit 2
        fi
    done
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

if [[ -z "$INTERFACE" ]]; then
    echo "AES67_E2E_INTERFACE is required and must be the local interface IPv4 address." >&2
    echo "Example: AES67_E2E_INTERFACE=192.168.1.20 bash scripts/e2e_multicast.sh" >&2
    exit 2
fi

validate_ipv4 "$INTERFACE" "AES67_E2E_INTERFACE"
validate_ipv4 "$ADDRESS" "AES67_E2E_ADDRESS"

address_first_octet="${ADDRESS%%.*}"
if (( address_first_octet < 224 || address_first_octet > 239 )); then
    echo "AES67_E2E_ADDRESS must be an IPv4 multicast group, got: $ADDRESS" >&2
    exit 2
fi

cat <<CONFIG
Multicast E2E configuration
  Address:   $ADDRESS
  Port:      $PORT
  Interface: $INTERFACE
  Duration:  $DURATION_SECONDS seconds
  Artifacts: $ARTIFACT_DIR
CONFIG

if (( DRY_RUN )); then
    exit 0
fi

require_command cargo
require_command ffmpeg
require_command ffprobe
require_command awk

mkdir -p "$ARTIFACT_DIR"
rm -f "$INPUT_WAV" "$SDP_FILE" "$RECORDED_WAV" "$STREAMER_LOG" "$FFMPEG_LOG" "$VOLUME_LOG"

echo "Building aes67-streamer..."
cargo build -p aes67-streamer

echo "Generating deterministic test WAV..."
ffmpeg -nostdin -y -v error \
    -f lavfi -i "sine=frequency=997:duration=${DURATION_SECONDS}:sample_rate=48000" \
    -filter:a "pan=stereo|c0=c0|c1=c0" \
    -c:a pcm_s24le \
    "$INPUT_WAV"

# The receiver joins the configured multicast group from SDP. localaddr selects
# the interface used to join the group, which is the most common source of
# false failures in multicast testing.
cat > "$SDP_FILE" <<SDP
v=0
o=- 123456 123456 IN IP4 ${INTERFACE}
s=AES67 Streamer Multicast E2E
c=IN IP4 ${ADDRESS}/32
t=0 0
m=audio ${PORT} RTP/AVP ${PAYLOAD_TYPE}
a=rtpmap:${PAYLOAD_TYPE} L24/48000/2
a=ptime:1
a=recvonly
SDP

echo "Starting ffmpeg multicast RTP receiver on ${ADDRESS}:${PORT} via ${INTERFACE}..."
ffmpeg -nostdin -y -v warning \
    -protocol_whitelist file,udp,rtp \
    -localaddr "$INTERFACE" \
    -i "$SDP_FILE" \
    -t "$DURATION_SECONDS" \
    -c:a pcm_s24le \
    "$RECORDED_WAV" >"$FFMPEG_LOG" 2>&1 &
receiver_pid=$!

sleep 1

echo "Running aes67-streamer multicast send for ${DURATION_SECONDS}s..."
RUST_LOG=info target/debug/aes67-streamer \
    --file "$INPUT_WAV" \
    --address "$ADDRESS" \
    --port "$PORT" \
    --interface "$INTERFACE" \
    --duration-seconds "$DURATION_SECONDS" >"$STREAMER_LOG" 2>&1 || fail_with_logs "Streamer failed"

deadline=$((SECONDS + 10))
while kill -0 "$receiver_pid" 2>/dev/null; do
    if (( SECONDS > deadline )); then
        fail_with_logs "Timed out waiting for ffmpeg receiver. Check that multicast routing and interface ${INTERFACE} are valid."
    fi
    sleep 0.2
done

wait "$receiver_pid" || fail_with_logs "ffmpeg multicast receiver failed. Check interface selection, multicast group join support, and local firewall rules."
receiver_pid=""

[[ -s "$RECORDED_WAV" ]] || fail_with_logs "Recorded WAV was not created"

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

ffmpeg -nostdin -v info -i "$RECORDED_WAV" -af volumedetect -f null - >"$VOLUME_LOG" 2>&1 || fail_with_logs "Volume validation failed"
volume_samples="$(awk '/n_samples:/ { samples=$NF } END { print samples }' "$VOLUME_LOG")"
[[ -n "$volume_samples" && "$volume_samples" != "0" ]] || fail_with_logs "Recorded WAV contains zero audio samples"
max_volume="$(awk '/max_volume:/ { print $NF }' "$VOLUME_LOG")"
[[ -n "$max_volume" ]] || fail_with_logs "Recorded WAV volume could not be measured"
if [[ "$max_volume" == "-inf" ]]; then
    fail_with_logs "Recorded WAV is silent"
fi

echo "Multicast E2E passed"
echo "Artifacts: $ARTIFACT_DIR"
echo "Recorded: ${duration}s, ${sample_rate} Hz, ${channels} channel(s)"
