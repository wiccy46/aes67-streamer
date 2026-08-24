import { invoke } from "@tauri-apps/api/core";
import type {
  DesktopInfo,
  RoutingSnapshot,
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
