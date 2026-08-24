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
