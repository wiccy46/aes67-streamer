# Desktop IPC Protocol

The desktop frontend communicates with the Rust host through Tauri IPC. The
local application does not run an HTTP or WebSocket server.

## Transport choices

| Traffic | Tauri primitive | Use |
| --- | --- | --- |
| Control | Commands | Create, update, delete, start, stop, and explicit snapshot reads. |
| State changes | Events or a state channel | Readiness, lifecycle, discovery, warnings, and failures. |
| Realtime telemetry | Channels | Ordered meter, clock, RTP, and health frames. |

Commands are request/response operations. Every mutation returns a typed error
or the authoritative revisioned snapshot; the frontend never assumes a local
edit succeeded before Rust confirms it.

The initial sender runtime uses `start_all`, `stop_all`, and
`get_runtime_snapshot`. Its snapshot is intentionally low-rate operational
state: lifecycle, PTP status, uptime, packet/byte totals, packet/bit rate,
late-packet count, peak/RMS, and the effective SDP for each Stream. The desktop
polls it at 2 Hz. This verifies transport health without treating polling as the
future realtime-meter protocol.

Events are suitable for small, infrequent broadcast notifications. They must
not carry audio meters or packet-by-packet data. A workflow that requires
ordered delivery should use a channel even when its data rate is low.

## Meter subscription

The frontend opens a telemetry subscription with one command and passes a
Tauri channel to Rust:

```ts
import { Channel, invoke } from "@tauri-apps/api/core";

const onTelemetry = new Channel<TelemetryEvent>();
onTelemetry.onmessage = applyTelemetryFrame;

const subscriptionId = await invoke<number>("subscribe_telemetry", {
  request: { meterHz: 25, sourceIds: [], streamIds: [] },
  onTelemetry,
});
```

Rust returns a `SubscriptionId`. `unsubscribe_telemetry` cancels its worker;
closing or reloading the window also stops delivery when the channel send
fails. A newly loaded frontend obtains a fresh `RoutingSnapshot` and creates a
new subscription instead of replaying old meter frames.

The initial frame contract should be equivalent to:

```ts
type MeterFrame = {
  schemaVersion: 1;
  sequence: number;
  monotonicNanos: number;
  points: Array<{
    kind: "source" | "stream";
    id: number;
    channels: Array<{
      peakDbfs: number;
      rmsDbfs: number;
      clipped: boolean;
    }>;
  }>;
};
```

Meter values are finite JSON numbers clamped to an agreed floor such as
`-120.0 dBFS`; `NaN` and infinity never cross IPC. `sequence` lets the frontend
detect skipped frames, while `monotonicNanos` supports timing and stale-meter
detection without relying on wall-clock time.

## Realtime rules

- The audio callback only updates lock-free meter accumulators or a bounded
  ring buffer. It never serializes JSON, invokes Tauri, allocates a telemetry
  frame, or waits for the frontend.
- A non-realtime telemetry worker samples those accumulators and publishes at
  20–30 Hz by default. This is visually smooth without sending audio-block-rate
  traffic to the webview.
- The handoff to the telemetry worker is latest-value-wins. A slow frontend may
  miss intermediate frames but must never apply backpressure to audio or RTP.
- Raw PCM and per-packet RTP data stay in Rust. Diagnostic traces are requested
  explicitly and written or exported through a separate bounded workflow.
- The frontend stores the latest meters outside the revisioned routing model
  and paints them on `requestAnimationFrame`. Meter updates must not re-render
  the entire React Flow graph.

## Type ownership

Wire DTOs live beside the engine API and derive `Serialize`/`Deserialize`.
Frontend types mirror those DTOs through one adapter module. UI components do
not call `invoke` directly.

The engine remains transport-neutral: it produces commands, snapshots, and
telemetry frames without depending on Tauri. The Tauri host adapts that API to
IPC. If remote monitoring is added later, a WebSocket adapter can expose the
same versioned DTOs without making the local desktop application depend on a
network server.

## Verification

- Pure Rust tests cover command validation, revisions, meter aggregation, and
  sequence/timestamp behavior.
- Native WebDriver E2E tests exercise the packaged webview and real Tauri
  commands.
- A future deterministic E2E meter source, enabled only by the `e2e` feature,
  will verify increasing sequences, stale-frame handling, clipping indication,
  subscription cleanup, and bounded UI update rate.
- RTP/audio correctness remains in engine loopback and soak tests; GUI tests
  verify orchestration and presentation rather than audio fidelity.
