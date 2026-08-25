export type SourceId = number;
export type StreamId = number;

export type SourceInput =
  | { File: { path: string } }
  | { LiveInput: { device: string } };

export interface SourceConfig {
  name: string;
  input: SourceInput;
}

export interface StreamConfig {
  name: string;
  address: string;
  port: number;
  gain_db: number | null;
}

export interface RoutingSource {
  id: SourceId;
  config: SourceConfig;
}

export interface RoutingStream {
  id: StreamId;
  config: StreamConfig;
}

export interface RouteAssignment {
  source_id: SourceId;
  stream_id: StreamId;
}

export interface RoutingSnapshot {
  revision: number;
  sources: RoutingSource[];
  streams: RoutingStream[];
  routes: RouteAssignment[];
}

export interface DesktopInfo {
  productName: string;
  version: string;
  liveRoutingAvailable: boolean;
}

export interface LocalInterface {
  name: string;
  address: string;
  is_loopback: boolean;
}

export interface SourceRequest {
  name: string;
  inputKind: "file" | "liveInput";
  location: string;
}

export interface StreamRequest {
  name: string;
  address: string;
  port: number;
  gainDb: number | null;
}

export interface RuntimeRequest {
  interface: string;
  ptpDomain: number;
}

export type RoutingRuntimeLifecycle = "stopped" | "starting" | "running" | "failed";
export type StreamRuntimeLifecycle = "starting" | "live" | "stopped" | "failed";

export interface StreamRuntimeStats {
  stream_id: StreamId;
  lifecycle: StreamRuntimeLifecycle;
  packets_sent: number;
  bytes_sent: number;
  packets_per_second: number;
  megabits_per_second: number;
  peak_dbfs: number;
  rms_dbfs: number;
  late_packets: number;
  sdp: string;
}

export interface RoutingRuntimeSnapshot {
  lifecycle: RoutingRuntimeLifecycle;
  interface: string | null;
  uptime_seconds: number;
  ptp: {
    state: string;
    offset_ns: number;
    master_identity: string | null;
  };
  streams: StreamRuntimeStats[];
  error: string | null;
}
