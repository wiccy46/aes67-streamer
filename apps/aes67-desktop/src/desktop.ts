import { invoke } from "@tauri-apps/api/core";
import type {
  DesktopInfo,
  RoutingRuntimeSnapshot,
  RoutingSnapshot,
  RuntimeRequest,
  SourceId,
  SourceRequest,
  StreamId,
  StreamRequest,
} from "./types";

export function isDesktopHost(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function getDesktopInfo(): Promise<DesktopInfo> {
  return invoke<DesktopInfo>("get_desktop_info");
}

export function getRoutingSnapshot(): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("get_routing_snapshot");
}

export function createSource(request: SourceRequest): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("create_source", { request });
}

export function updateSource(
  sourceId: SourceId,
  request: SourceRequest,
): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("update_source", { sourceId, request });
}

export function removeSource(sourceId: SourceId): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("remove_source", { sourceId });
}

export function createStream(request: StreamRequest): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("create_stream", { request });
}

export function updateStream(
  streamId: StreamId,
  request: StreamRequest,
): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("update_stream", { streamId, request });
}

export function removeStream(streamId: StreamId): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("remove_stream", { streamId });
}

export function assignSource(sourceId: SourceId, streamId: StreamId): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("assign_source", { sourceId, streamId });
}

export function removeRoute(streamId: StreamId): Promise<RoutingSnapshot> {
  return invoke<RoutingSnapshot>("remove_route", { streamId });
}

export function getRuntimeSnapshot(): Promise<RoutingRuntimeSnapshot> {
  return invoke<RoutingRuntimeSnapshot>("get_runtime_snapshot");
}

export function startAll(request: RuntimeRequest): Promise<RoutingRuntimeSnapshot> {
  return invoke<RoutingRuntimeSnapshot>("start_all", { request });
}

export function stopAll(): Promise<RoutingRuntimeSnapshot> {
  return invoke<RoutingRuntimeSnapshot>("stop_all");
}

export function getStreamSdp(streamId: StreamId, request: RuntimeRequest): Promise<string> {
  return invoke<string>("get_stream_sdp", { streamId, request });
}
