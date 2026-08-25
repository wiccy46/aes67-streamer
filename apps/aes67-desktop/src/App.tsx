import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  Handle,
  Position,
  ReactFlow,
  addEdge,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type NodeProps,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Broadcast,
  CheckCircle,
  Copy,
  FileAudio,
  FileCode,
  FolderOpen,
  PlugsConnected,
  Plus,
  Radio,
  Stop,
  Trash,
  Waveform,
  X,
} from "@phosphor-icons/react";
import {
  assignSource,
  createSource,
  createStream,
  getDesktopInfo,
  getLocalInterfaces,
  getRuntimeSnapshot,
  getRoutingSnapshot,
  getStreamSdp,
  isDesktopHost,
  removeBlocks,
  removeRoute,
  startAll,
  stopAll,
  updateSource,
  updateStream,
} from "./desktop";
import type {
  DesktopInfo,
  LocalInterface,
  RoutingRuntimeSnapshot,
  RoutingSnapshot,
  SourceInput,
  StreamConfig,
  StreamRuntimeStats,
} from "./types";

const routeStyle = {
  stroke: "#ff9d00",
  strokeWidth: 2.5,
};

const loopbackInterface: LocalInterface = {
  name: "loopback",
  address: "127.0.0.1",
  is_loopback: true,
};

function formatInterfaceOption(networkInterface: LocalInterface): string {
  const name = networkInterface.is_loopback
    ? `Loopback (${networkInterface.name})`
    : networkInterface.name;
  return `${name} · ${networkInterface.address}`;
}

type SourceNodeData = {
  name: string;
  detail: string;
  kind: string;
  importDisabled?: boolean;
  onImportFile?: (nodeId: string) => void;
};

type StreamNodeData = {
  name: string;
  detail: string;
  format: string;
  gainDb: number | null;
  runtime?: StreamRuntimeStats;
  onGainCommit?: (gainDb: number | null) => void;
  onSdpRequest?: (nodeId: string, action: "view" | "copy") => void;
  onOpenMenu?: (nodeId: string, x: number, y: number) => void;
};

type SourceFlowNode = Node<SourceNodeData, "source">;
type StreamFlowNode = Node<StreamNodeData, "stream">;
type AppNode = SourceFlowNode | StreamFlowNode;

const initialNodes: AppNode[] = [
  {
    id: "source-studio-a",
    type: "source",
    position: { x: 110, y: 104 },
    deletable: false,
    data: { name: "Studio A", detail: "Live input · 48 kHz", kind: "Live input" },
  },
  {
    id: "source-music-bed",
    type: "source",
    position: { x: 110, y: 328 },
    deletable: false,
    data: { name: "Music bed", detail: "music-bed.wav · stereo", kind: "Audio file" },
  },
  {
    id: "source-voiceover",
    type: "source",
    position: { x: 110, y: 552 },
    deletable: false,
    data: { name: "Voiceover", detail: "USB 04 · mono", kind: "Live input" },
  },
  {
    id: "stream-program",
    type: "stream",
    position: { x: 780, y: 104 },
    deletable: false,
    data: {
      name: "Program",
      detail: "239.69.83.1:5004",
      format: "48 kHz · 2 ch",
      gainDb: 0,
    },
  },
  {
    id: "stream-lobby",
    type: "stream",
    position: { x: 780, y: 328 },
    deletable: false,
    data: {
      name: "Lobby",
      detail: "239.69.83.2:5004",
      format: "48 kHz · 2 ch",
      gainDb: -12,
    },
  },
  {
    id: "stream-green-room",
    type: "stream",
    position: { x: 780, y: 552 },
    deletable: false,
    data: {
      name: "Green room",
      detail: "239.69.83.3:5004",
      format: "48 kHz · 2 ch",
      gainDb: -6,
    },
  },
];

const initialEdges: Edge[] = [
  buildEdge("source-studio-a", "stream-program"),
  buildEdge("source-music-bed", "stream-lobby"),
  buildEdge("source-music-bed", "stream-green-room"),
];

function SourceModule({ data, id }: NodeProps<SourceFlowNode>) {
  const hasFile = data.kind === "Audio file";

  return (
    <article className="module-node module-node--source" data-testid={id}>
      <div className="module-node__eyebrow">
        <span>
          <FileAudio size={15} weight="fill" aria-hidden="true" />
          {data.kind}
        </span>
        <span className="module-node__signal" aria-label="Source defined" />
      </div>
      <h2>{data.name}</h2>
      <div className="source-file-row">
        <p title={data.detail}>{data.detail}</p>
        <button
          className="source-file-action nodrag"
          type="button"
          disabled={data.importDisabled}
          data-testid={`${id}-import-file`}
          onClick={() => data.onImportFile?.(id)}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <FolderOpen size={14} weight="bold" aria-hidden="true" />
          {hasFile ? "Replace" : "Import file"}
        </button>
      </div>
      <div className="module-node__port-label">Output</div>
      <Handle
        id="output"
        data-testid={`${id}-output`}
        className="route-handle route-handle--output"
        type="source"
        position={Position.Right}
      />
    </article>
  );
}

function StreamModule({ data, id }: NodeProps<StreamFlowNode>) {
  const [gainDraft, setGainDraft] = useState(formatGain(data.gainDb));

  useEffect(() => {
    setGainDraft(formatGain(data.gainDb));
  }, [data.gainDb]);

  function commitGain() {
    const gainDb = normalizeGain(gainDraft);
    setGainDraft(formatGain(gainDb));
    if (gainDb !== data.gainDb) {
      data.onGainCommit?.(gainDb);
    }
  }

  const lifecycle = data.runtime?.lifecycle ?? "stopped";
  const stateLabel =
    lifecycle === "live"
      ? "Live"
      : lifecycle === "starting"
        ? "Starting"
        : lifecycle === "failed"
          ? "Error"
          : "Configured";

  return (
    <article
      className={`module-node module-node--stream module-node--${lifecycle}`}
      data-testid={id}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        data.onOpenMenu?.(id, event.clientX, event.clientY);
      }}
    >
      <Handle
        id="input"
        data-testid={`${id}-input`}
        className="route-handle route-handle--input"
        type="target"
        position={Position.Left}
      />
      <div className="module-node__eyebrow">
        <span>
          <Broadcast size={15} weight="fill" aria-hidden="true" />
          AES67 stream
        </span>
        <span className={`module-node__ready is-${lifecycle}`}>
          <CheckCircle size={15} weight="fill" aria-hidden="true" />
          {stateLabel}
        </span>
      </div>
      <h2>{data.name}</h2>
      <p className="is-address">{data.detail}</p>
      <div className="module-node__stream-settings">
        <span className="module-node__format">{data.format}</span>
        <label className="gain-control nodrag nowheel">
          <span className="gain-control__label">Gain</span>
          <span className={`gain-control__value ${data.gainDb === null ? "is-muted" : ""}`}>
            <input
              type="number"
              max={0}
              step={0.5}
              value={gainDraft}
              placeholder="−∞"
              aria-label={`${data.name} gain in decibels`}
              data-testid={`${id}-gain`}
              onChange={(event) => setGainDraft(event.target.value)}
              onBlur={commitGain}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.currentTarget.blur();
                }
              }}
            />
            <span>dB</span>
          </span>
        </label>
        <button
          className="sdp-action nodrag"
          type="button"
          data-testid={`${id}-sdp`}
          onClick={() => data.onSdpRequest?.(id, "view")}
          onMouseDown={(event) => event.stopPropagation()}
        >
          SDP
        </button>
      </div>
      <div className="stream-metrics" aria-label={`${data.name} stream metrics`}>
        <span>
          <small>Packets</small>
          <strong>{formatPacketCount(data.runtime?.packets_sent)}</strong>
        </span>
        <span>
          <small>Rate</small>
          <strong>{formatRate(data.runtime?.megabits_per_second)}</strong>
        </span>
        <span>
          <small>Peak</small>
          <strong>{formatPeak(data.runtime?.peak_dbfs)}</strong>
        </span>
      </div>
      <div className="module-node__port-label">Input</div>
    </article>
  );
}

const nodeTypes: NodeTypes = {
  source: SourceModule,
  stream: StreamModule,
};

const MIN_GAIN_DB = -120;
const MAX_GAIN_DB = 0;

function formatGain(gainDb: number | null): string {
  return gainDb === null ? "" : String(gainDb);
}

function normalizeGain(value: string): number | null {
  if (value.trim() === "") {
    return null;
  }
  const gainDb = Number(value);
  if (!Number.isFinite(gainDb) || gainDb < MIN_GAIN_DB) {
    return null;
  }
  return Math.min(gainDb, MAX_GAIN_DB);
}

function formatPacketCount(value?: number): string {
  if (value === undefined) {
    return "—";
  }
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(1)}m`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(1)}k`;
  }
  return String(value);
}

function formatRate(value?: number): string {
  return value === undefined ? "—" : `${value.toFixed(2)} Mb/s`;
}

function formatPeak(value?: number): string {
  return value === undefined ? "—" : value <= -120 ? "−∞" : `${value.toFixed(1)} dB`;
}

function buildEdge(source: string, target: string): Edge {
  return {
    id: `${source}-${target}`,
    source,
    sourceHandle: "output",
    target,
    targetHandle: "input",
    type: "smoothstep",
    animated: true,
    style: routeStyle,
  };
}

function getModulePosition(items: AppNode[], type: "source" | "stream") {
  const moduleCount = items.filter((item) => item.type === type).length;
  return { x: type === "source" ? 110 : 780, y: 104 + moduleCount * 248 };
}

function getSourcePresentation(input: SourceInput): Pick<SourceNodeData, "kind" | "detail"> {
  if ("File" in input) {
    return {
      kind: "Audio file",
      detail: input.File.path.split(/[\\/]/).at(-1) ?? input.File.path,
    };
  }
  return { kind: "Live input", detail: input.LiveInput.device };
}

function parseEngineId(nodeId: string, prefix: "source" | "stream"): number | null {
  if (!nodeId.startsWith(`${prefix}-`)) {
    return null;
  }
  const id = Number(nodeId.slice(prefix.length + 1));
  return Number.isSafeInteger(id) && id > 0 ? id : null;
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function matchesActiveRuntime(lifecycle: RoutingRuntimeSnapshot["lifecycle"]): boolean {
  return lifecycle === "starting" || lifecycle === "running";
}

function buildPreviewSdp(name: string, destination: string, interfaceName: string): string {
  const [address = "239.69.83.1", port = "5004"] = destination.split(":");
  return [
    "v=0",
    `o=- 1 1 IN IP4 ${interfaceName}`,
    `s=${name}`,
    `c=IN IP4 ${address}/32`,
    "t=0 0",
    `m=audio ${port} RTP/AVP 97`,
    "a=rtpmap:97 L24/48000/2",
    "a=ptime:1",
    "a=mediaclk:direct=0",
    "a=sendonly",
    "",
  ].join("\r\n");
}

async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // The fallback supports webviews that do not grant Clipboard API access.
    }
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) {
    throw new Error("clipboard access is unavailable");
  }
}

const stoppedRuntime: RoutingRuntimeSnapshot = {
  lifecycle: "stopped",
  interface: null,
  uptime_seconds: 0,
  ptp: { state: "stopped", offset_ns: 0, master_identity: null },
  streams: [],
  error: null,
};

type StreamMenuState = {
  nodeId: string;
  name: string;
  x: number;
  y: number;
};

type SdpDialogState = {
  name: string;
  sdp: string;
};

type DeleteBlockTarget = {
  nodeId: string;
  name: string;
  kind: "source" | "stream";
};

type DeleteDialogState = {
  blocks: DeleteBlockTarget[];
};

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return (
    target.isContentEditable ||
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.tagName === "SELECT"
  );
}

function formatSelectionSummary(blocks: DeleteBlockTarget[]): string {
  if (blocks.length === 1) {
    return blocks[0].name;
  }
  if (blocks.length === 2) {
    return `${blocks[0].name} and ${blocks[1].name}`;
  }
  return `${blocks[0].name}, ${blocks[1].name} and ${blocks.length - 2} more`;
}

export function App() {
  const desktopHost = isDesktopHost();
  const [nodes, setNodes, onNodesChange] = useNodesState<AppNode>(initialNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);
  const [desktopInfo, setDesktopInfo] = useState<DesktopInfo | null>(null);
  const [runtime, setRuntime] = useState<RoutingRuntimeSnapshot>(stoppedRuntime);
  const [browserLive, setBrowserLive] = useState(false);
  const [localInterfaces, setLocalInterfaces] = useState<LocalInterface[]>([
    loopbackInterface,
  ]);
  const [interfaceAddress, setInterfaceAddress] = useState(loopbackInterface.address);
  const [selectedNodeIds, setSelectedNodeIds] = useState<string[]>([]);
  const [streamMenu, setStreamMenu] = useState<StreamMenuState | null>(null);
  const [sdpDialog, setSdpDialog] = useState<SdpDialogState | null>(null);
  const [deleteDialog, setDeleteDialog] = useState<DeleteDialogState | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);
  const [notice, setNotice] = useState(
    "Drag an output handle onto a stream input to create a route.",
  );
  const sourceSequence = useRef(4);
  const streamSequence = useRef(4);
  const runtimeRef = useRef(runtime);
  const sourceFileImportRef = useRef((_nodeId: string) => {});
  const streamGainCommitRef = useRef(
    (_streamId: number, _config: StreamConfig, _gainDb: number | null) => {},
  );
  const streamSdpRequestRef = useRef((_nodeId: string, _action: "view" | "copy") => {});
  const isLive = desktopHost ? runtime.lifecycle === "running" : browserLive;
  const isStarting = desktopHost && runtime.lifecycle === "starting";
  const selectedBlocks = useMemo(() => {
    const selectedIds = new Set(selectedNodeIds);
    return nodes
      .filter((node) => selectedIds.has(node.id))
      .map<DeleteBlockTarget>((node) => ({
        nodeId: node.id,
        name: node.data.name,
        kind: node.type,
      }));
  }, [nodes, selectedNodeIds]);

  const onSelectionChange = useCallback(
    ({ nodes: selectedNodes }: { nodes: AppNode[] }) =>
      setSelectedNodeIds(selectedNodes.map((node) => node.id)),
    [],
  );

  runtimeRef.current = runtime;

  const applySnapshot = useCallback(
    (snapshot: RoutingSnapshot) => {
      setNodes((currentNodes) => {
        const existingPositions = new Map(currentNodes.map((node) => [node.id, node.position]));
        const sourceNodes: SourceFlowNode[] = snapshot.sources.map((source, index) => {
          const id = `source-${source.id}`;
          return {
            id,
            type: "source",
            position: existingPositions.get(id) ?? { x: 110, y: 104 + index * 224 },
            deletable: false,
            data: {
              name: source.config.name,
              ...getSourcePresentation(source.config.input),
              importDisabled: matchesActiveRuntime(runtimeRef.current.lifecycle),
              onImportFile: (nodeId) => sourceFileImportRef.current(nodeId),
            },
          };
        });
        const streamNodes: StreamFlowNode[] = snapshot.streams.map((stream, index) => {
          const id = `stream-${stream.id}`;
          const runtimeStats = runtimeRef.current.streams.find(
            (stats) => stats.stream_id === stream.id,
          );
          return {
            id,
            type: "stream",
            position: existingPositions.get(id) ?? { x: 780, y: 104 + index * 248 },
            deletable: false,
            data: {
              name: stream.config.name,
              detail: `${stream.config.address}:${stream.config.port}`,
              format: "48 kHz · source channels",
              gainDb: stream.config.gain_db,
              runtime: runtimeStats,
              onGainCommit: (gainDb) =>
                streamGainCommitRef.current(stream.id, stream.config, gainDb),
              onSdpRequest: (nodeId, action) =>
                streamSdpRequestRef.current(nodeId, action),
              onOpenMenu: (nodeId, x, y) =>
                setStreamMenu({ nodeId, name: stream.config.name, x, y }),
            },
          };
        });
        return [...sourceNodes, ...streamNodes];
      });
      setEdges(
        snapshot.routes.map((route) =>
          buildEdge(`source-${route.source_id}`, `stream-${route.stream_id}`),
        ),
      );
      sourceSequence.current = Math.max(4, snapshot.sources.length + 1);
      streamSequence.current = Math.max(4, snapshot.streams.length + 1);
    },
    [setEdges, setNodes],
  );

  streamGainCommitRef.current = (streamId, config, gainDb) => {
    if (!desktopHost) {
      return;
    }
    setNotice("Saving stream gain…");
    void updateStream(streamId, {
      name: config.name,
      address: config.address,
      port: config.port,
      gainDb,
    })
      .then((snapshot) => {
        applySnapshot(snapshot);
        setNotice(
          `Stream gain saved at ${gainDb === null ? "−∞" : `${gainDb} dB`} · revision ${snapshot.revision}`,
        );
      })
      .catch((error) => setNotice(`Could not save stream gain: ${formatError(error)}`));
  };

  useEffect(() => {
    setNodes((currentNodes) =>
      currentNodes.map((node) => {
        if (node.type === "source") {
          return {
            ...node,
            data: {
              ...node.data,
              importDisabled: matchesActiveRuntime(runtime.lifecycle),
              onImportFile: (nodeId) => sourceFileImportRef.current(nodeId),
            },
          };
        }
        if (node.type !== "stream") {
          return node;
        }
        const streamId = parseEngineId(node.id, "stream");
        return {
          ...node,
          data: {
            ...node.data,
            runtime:
              streamId === null
                ? node.data.runtime
                : runtime.streams.find((stats) => stats.stream_id === streamId),
            onSdpRequest: (nodeId, action) =>
              streamSdpRequestRef.current(nodeId, action),
            onOpenMenu: (nodeId, x, y) =>
              setStreamMenu({ nodeId, name: node.data.name, x, y }),
          },
        };
      }),
    );
  }, [runtime, setNodes]);

  sourceFileImportRef.current = (nodeId) => {
    if (matchesActiveRuntime(runtime.lifecycle)) {
      setNotice("Stop all streams before changing a source file.");
      return;
    }

    const node = nodes.find((candidate) => candidate.id === nodeId);
    if (!node || node.type !== "source") {
      setNotice("Could not find that source.");
      return;
    }

    if (!desktopHost) {
      const picker = document.createElement("input");
      picker.type = "file";
      picker.accept = ".wav,.flac,.mp3,.aiff,.aif,audio/*";
      picker.addEventListener(
        "change",
        () => {
          const file = picker.files?.[0];
          if (!file) {
            return;
          }
          setNodes((currentNodes) =>
            currentNodes.map((currentNode) =>
              currentNode.id === nodeId && currentNode.type === "source"
                ? {
                    ...currentNode,
                    data: {
                      ...currentNode.data,
                      kind: "Audio file",
                      detail: file.name,
                    },
                  }
                : currentNode,
            ),
          );
          setNotice(`${file.name} selected for ${node.data.name} in browser preview.`);
        },
        { once: true },
      );
      picker.click();
      return;
    }

    const sourceId = parseEngineId(nodeId, "source");
    if (sourceId === null) {
      setNotice("This source does not contain a valid engine identifier.");
      return;
    }

    void open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: "Audio files",
          extensions: ["wav", "flac", "mp3", "aiff", "aif"],
        },
      ],
    })
      .then((selection) => {
        if (selection === null) {
          return null;
        }
        const path = Array.isArray(selection) ? selection[0] : selection;
        if (!path) {
          return null;
        }
        setNotice(`Importing ${path.split(/[\\/]/).at(-1) ?? path}…`);
        return updateSource(sourceId, {
          name: node.data.name,
          inputKind: "file",
          location: path,
        });
      })
      .then((snapshot) => {
        if (!snapshot) {
          return;
        }
        applySnapshot(snapshot);
        setNotice(`${node.data.name} audio file updated · revision ${snapshot.revision}`);
      })
      .catch((error) => setNotice(`Could not import audio file: ${formatError(error)}`));
  };

  streamSdpRequestRef.current = (nodeId, action) => {
    const node = nodes.find((candidate) => candidate.id === nodeId);
    if (!node || node.type !== "stream") {
      setNotice("Could not find that stream.");
      return;
    }

    setStreamMenu(null);
    setNotice(action === "copy" ? "Preparing SDP to copy…" : "Preparing SDP…");
    const streamId = parseEngineId(nodeId, "stream");
    const sdpPromise =
      desktopHost && streamId !== null
        ? getStreamSdp(streamId, { interface: interfaceAddress, ptpDomain: 0 })
        : Promise.resolve(buildPreviewSdp(node.data.name, node.data.detail, interfaceAddress));

    void sdpPromise
      .then(async (sdp) => {
        if (action === "copy") {
          await copyText(sdp);
          setNotice(`${node.data.name} SDP copied.`);
        } else {
          setSdpDialog({ name: node.data.name, sdp });
          setNotice(`${node.data.name} SDP is ready.`);
        }
      })
      .catch((error) => setNotice(`Could not get SDP: ${formatError(error)}`));
  };

  useEffect(() => {
    if (!desktopHost) {
      return;
    }

    let cancelled = false;
    let pollTimer: number | undefined;

    const pollRuntime = () => {
      void getRuntimeSnapshot()
        .then((snapshot) => {
          if (!cancelled) {
            setRuntime(snapshot);
          }
        })
        .finally(() => {
          if (!cancelled) {
            pollTimer = window.setTimeout(pollRuntime, 500);
          }
        });
    };

    const interfaceDiscovery = getLocalInterfaces()
      .then((interfaces) => ({ interfaces, error: null }))
      .catch((error: unknown) => ({
        interfaces: [loopbackInterface],
        error: formatError(error),
      }));

    Promise.all([
      getDesktopInfo(),
      getRoutingSnapshot(),
      getRuntimeSnapshot(),
      interfaceDiscovery,
    ])
      .then(([info, snapshot, runtimeSnapshot, discovered]) => {
        if (cancelled) {
          return;
        }
        const activeInterface = matchesActiveRuntime(runtimeSnapshot.lifecycle)
          ? runtimeSnapshot.interface
          : null;
        const interfaces =
          activeInterface &&
          !discovered.interfaces.some((item) => item.address === activeInterface)
            ? [
                ...discovered.interfaces,
                { name: "active", address: activeInterface, is_loopback: false },
              ]
            : discovered.interfaces;
        setDesktopInfo(info);
        setRuntime(runtimeSnapshot);
        setLocalInterfaces(interfaces.length ? interfaces : [loopbackInterface]);
        setInterfaceAddress(activeInterface ?? loopbackInterface.address);
        applySnapshot(snapshot);
        setNotice(
          discovered.error
            ? `Engine model connected · Interface discovery unavailable; using Loopback.`
            : `Engine model connected · ${interfaces.length} network ${interfaces.length === 1 ? "interface" : "interfaces"} found.`,
        );
        pollTimer = window.setTimeout(pollRuntime, 500);
      })
      .catch((error) => {
        if (!cancelled) {
          setNotice(`Desktop bridge failed: ${formatError(error)}`);
        }
      });

    return () => {
      cancelled = true;
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
      }
    };
  }, [applySnapshot, desktopHost]);

  const isValidConnection = useCallback(
    (connection: Connection | Edge) =>
      connection.source?.startsWith("source-") && connection.target?.startsWith("stream-"),
    [],
  );

  const onConnect = useCallback(
    (connection: Connection) => {
      if (!isValidConnection(connection) || !connection.source || !connection.target) {
        setNotice("Routes run from a source output to a stream input.");
        return;
      }

      if (!desktopHost) {
        setEdges((currentEdges) => {
          const routesWithoutPreviousInput = currentEdges.filter(
            (edge) => edge.target !== connection.target,
          );
          return addEdge(
            {
              ...connection,
              id: `${connection.source}-${connection.target}`,
              type: "smoothstep",
              animated: true,
              style: routeStyle,
            },
            routesWithoutPreviousInput,
          );
        });
        setNotice("Route updated. A source can feed as many streams as you need.");
        return;
      }

      const sourceId = parseEngineId(connection.source, "source");
      const streamId = parseEngineId(connection.target, "stream");
      if (sourceId === null || streamId === null) {
        setNotice("This route does not contain valid engine identifiers.");
        return;
      }

      setNotice("Saving route…");
      void assignSource(sourceId, streamId)
        .then((snapshot) => {
          applySnapshot(snapshot);
          setNotice(`Route saved · revision ${snapshot.revision}`);
        })
        .catch((error) => setNotice(`Route failed: ${formatError(error)}`));
    },
    [applySnapshot, desktopHost, isValidConnection, setEdges],
  );

  const onEdgesDelete = useCallback(
    (deletedEdges: Edge[]) => {
      if (!desktopHost) {
        setNotice(`${deletedEdges.length} route${deletedEdges.length === 1 ? "" : "s"} removed.`);
        return;
      }

      void (async () => {
        try {
          let latestSnapshot: RoutingSnapshot | null = null;
          for (const edge of deletedEdges) {
            const streamId = parseEngineId(edge.target, "stream");
            if (streamId !== null) {
              latestSnapshot = await removeRoute(streamId);
            }
          }
          if (latestSnapshot) {
            applySnapshot(latestSnapshot);
            setNotice(`Route removed · revision ${latestSnapshot.revision}`);
          }
        } catch (error) {
          setNotice(`Could not remove route: ${formatError(error)}`);
          const snapshot = await getRoutingSnapshot();
          applySnapshot(snapshot);
        }
      })();
    },
    [applySnapshot, desktopHost],
  );

  const requestDeleteSelection = useCallback(() => {
    if (!selectedBlocks.length) {
      setNotice("Select a Source or Stream block first.");
      return;
    }
    if (isLive || isStarting) {
      setNotice("Stop all streams before deleting blocks.");
      return;
    }
    setDeleteDialog({ blocks: selectedBlocks });
  }, [isLive, isStarting, selectedBlocks]);

  async function confirmDeleteSelection() {
    if (!deleteDialog || isDeleting) {
      return;
    }

    const blocks = deleteDialog.blocks;
    setIsDeleting(true);
    setNotice(`Deleting ${blocks.length} selected ${blocks.length === 1 ? "block" : "blocks"}...`);

    try {
      if (desktopHost) {
        const sourceIds: number[] = [];
        const streamIds: number[] = [];
        for (const block of blocks) {
          if (block.kind === "source") {
            const sourceId = parseEngineId(block.nodeId, "source");
            if (sourceId === null) {
              throw new Error(`${block.name} does not contain a valid Source identifier.`);
            }
            sourceIds.push(sourceId);
          } else {
            const streamId = parseEngineId(block.nodeId, "stream");
            if (streamId === null) {
              throw new Error(`${block.name} does not contain a valid Stream identifier.`);
            }
            streamIds.push(streamId);
          }
        }
        const snapshot = await removeBlocks({ sourceIds, streamIds });
        applySnapshot(snapshot);
      } else {
        const deletedIds = new Set(blocks.map((block) => block.nodeId));
        setNodes((currentNodes) =>
          currentNodes.filter((node) => !deletedIds.has(node.id)),
        );
        setEdges((currentEdges) =>
          currentEdges.filter(
            (edge) => !deletedIds.has(edge.source) && !deletedIds.has(edge.target),
          ),
        );
      }

      setSelectedNodeIds([]);
      setDeleteDialog(null);
      setNotice(
        `${blocks.length} ${blocks.length === 1 ? "block" : "blocks"} deleted with connected routes.`,
      );
    } catch (error) {
      setNotice(`Could not delete selection: ${formatError(error)}`);
      if (desktopHost) {
        const snapshot = await getRoutingSnapshot().catch(() => null);
        if (snapshot) {
          applySnapshot(snapshot);
        }
      }
    } finally {
      setIsDeleting(false);
    }
  }

  useEffect(() => {
    const handleDeleteShortcut = (event: KeyboardEvent) => {
      if (event.key === "Escape" && deleteDialog && !isDeleting) {
        event.preventDefault();
        setDeleteDialog(null);
        return;
      }
      if (
        (event.key !== "Backspace" && event.key !== "Delete") ||
        event.repeat ||
        deleteDialog ||
        isEditableTarget(event.target) ||
        !selectedBlocks.length
      ) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      requestDeleteSelection();
    };

    window.addEventListener("keydown", handleDeleteShortcut, true);
    return () => window.removeEventListener("keydown", handleDeleteShortcut, true);
  }, [deleteDialog, isDeleting, requestDeleteSelection, selectedBlocks.length]);

  function addSource() {
    const sequence = sourceSequence.current++;
    if (desktopHost) {
      setNotice("Creating source…");
      void createSource({
        name: `Source ${sequence}`,
        inputKind: "liveInput",
        location: "Input not selected",
      })
        .then((snapshot) => {
          applySnapshot(snapshot);
          setNotice("Source created. Import an audio file from its Source block.");
        })
        .catch((error) => setNotice(`Could not create source: ${formatError(error)}`));
      return;
    }

    setNodes((currentNodes) => [
      ...currentNodes,
      {
        id: `source-${sequence}`,
        type: "source",
        position: getModulePosition(currentNodes, "source"),
        deletable: false,
        data: {
          name: `Source ${sequence}`,
          detail: "Choose an input or file",
          kind: "New source",
        },
      },
    ]);
    setNotice("Source added. Drag its output onto a stream input when it is ready.");
  }

  function addStream() {
    const sequence = streamSequence.current++;
    if (desktopHost) {
      setNotice("Creating stream…");
      void createStream({
        name: `Stream ${sequence}`,
        address: `239.69.83.${Math.min(sequence, 254)}`,
        port: 5004,
        gainDb: 0,
      })
        .then((snapshot) => {
          applySnapshot(snapshot);
          setNotice("Stream created. Select it to edit its destination next.");
        })
        .catch((error) => setNotice(`Could not create stream: ${formatError(error)}`));
      return;
    }

    setNodes((currentNodes) => [
      ...currentNodes,
      {
        id: `stream-${sequence}`,
        type: "stream",
        position: getModulePosition(currentNodes, "stream"),
        deletable: false,
        data: {
          name: `Stream ${sequence}`,
          detail: "Set multicast address",
          format: "48 kHz · 2 ch",
          gainDb: 0,
        },
      },
    ]);
    setNotice("Stream added. Connect one source now, or leave it unassigned.");
  }

  async function toggleLive() {
    if (!edges.length) {
      setNotice("Create at least one source-to-stream route before starting.");
      return;
    }

    if (!desktopHost) {
      setBrowserLive((current) => !current);
      setNotice(
        browserLive ? "All routes are standing by." : `${edges.length} preview routes are live.`,
      );
      return;
    }

    try {
      if (runtime.lifecycle === "running") {
        setNotice("Stopping all streams…");
        const snapshot = await stopAll();
        setRuntime(snapshot);
        setNotice("All streams stopped.");
      } else {
        setNotice("Starting PTP and routed streams…");
        setRuntime((current) => ({ ...current, lifecycle: "starting", error: null }));
        const snapshot = await startAll({ interface: interfaceAddress, ptpDomain: 0 });
        setRuntime(snapshot);
        setNotice(`${snapshot.streams.length} streams are sending RTP.`);
      }
    } catch (error) {
      const snapshot = await getRuntimeSnapshot().catch(() => null);
      if (snapshot) {
        setRuntime(snapshot);
      }
      setNotice(`Could not ${isLive ? "stop" : "start"} streams: ${formatError(error)}`);
    }
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="wordmark" aria-label="aes67">
          aes<span>67</span>
        </div>
        <nav className="mode-switch" aria-label="Product mode">
          <button
            className="is-active"
            type="button"
            aria-current="page"
            data-testid="mode-send"
          >
            Send
          </button>
          <button
            type="button"
            data-testid="mode-receive"
            onClick={() =>
              setNotice("Receive uses the same module system and will follow the Send editor.")
            }
          >
            Receive
          </button>
        </nav>
        <div className={`header-state ${isLive ? "is-live" : ""}`}>
          <span />
          {isStarting
            ? "Starting"
            : isLive
              ? `Live · PTP ${runtime.ptp.state}`
              : runtime.lifecycle === "failed"
                ? "Runtime error"
                : "Standby"}
        </div>
      </header>

      <section className="routing-workspace" aria-label="Send routing workspace">
        <div className="routing-toolbar">
          <div className="routing-heading">
            <div className="routing-kicker">
              <Radio size={17} weight="fill" aria-hidden="true" />
              Send routing
            </div>
            <p>Sources can fan out to multiple AES67 streams.</p>
          </div>

          <div className="routing-actions">
            <label className="interface-control">
              <span>Interface</span>
              <select
                value={interfaceAddress}
                disabled={isLive || isStarting}
                data-testid="send-interface"
                onChange={(event) => setInterfaceAddress(event.target.value)}
                aria-label="Send network interface"
              >
                {localInterfaces.map((networkInterface) => (
                  <option
                    key={`${networkInterface.name}-${networkInterface.address}`}
                    value={networkInterface.address}
                  >
                    {formatInterfaceOption(networkInterface)}
                  </option>
                ))}
              </select>
            </label>
            <button
              className="toolbar-button"
              type="button"
              data-testid="add-source"
              onClick={addSource}
            >
              <Plus size={18} weight="bold" aria-hidden="true" />
              Add source
            </button>
            <button
              className="toolbar-button"
              type="button"
              data-testid="add-stream"
              onClick={addStream}
            >
              <Plus size={18} weight="bold" aria-hidden="true" />
              Add stream
            </button>
            <button
              className={`live-action ${isLive ? "is-live" : ""}`}
              type="button"
              data-testid="start-all"
              disabled={isStarting}
              onClick={() => void toggleLive()}
            >
              {isLive ? (
                <Stop size={18} weight="fill" aria-hidden="true" />
              ) : (
                <Waveform size={20} weight="bold" aria-hidden="true" />
              )}
              {isStarting ? "Starting…" : isLive ? "Stop all" : "Start all"}
            </button>
          </div>
        </div>

        <div className="routing-canvas" data-testid="routing-canvas">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onEdgesDelete={onEdgesDelete}
            onSelectionChange={onSelectionChange}
            onConnect={onConnect}
            onNodeClick={(_, node) =>
              setNotice(`${node.data.name} selected. Use the Delete control or keyboard shortcut.`)
            }
            onEdgeClick={() => setNotice("Selected route. Press Delete or Backspace to remove it.")}
            onPaneClick={() => {
              setStreamMenu(null);
              setNotice("Drag from any source output to assign or reassign a stream input.");
            }}
            isValidConnection={isValidConnection}
            deleteKeyCode={["Backspace", "Delete"]}
            defaultEdgeOptions={{ animated: true, style: routeStyle, type: "smoothstep" }}
            fitView
            fitViewOptions={{ padding: 0.18, maxZoom: 1 }}
            minZoom={0.55}
            maxZoom={1.35}
            proOptions={{ hideAttribution: true }}
          >
            <Background color="#1a2530" gap={25} size={1} />
          </ReactFlow>

          {selectedBlocks.length ? (
            <div className="selection-actions" data-testid="selection-actions">
              <div className="selection-actions__summary">
                <span>
                  {selectedBlocks.length} {selectedBlocks.length === 1 ? "block" : "blocks"} selected
                </span>
                <strong title={selectedBlocks.map((block) => block.name).join(", ")}>
                  {formatSelectionSummary(selectedBlocks)}
                </strong>
              </div>
              <span className="selection-actions__shortcut">Backspace or Delete</span>
              <button
                type="button"
                disabled={isLive || isStarting}
                title={
                  isLive || isStarting
                    ? "Stop all streams before deleting blocks"
                    : "Delete selected blocks"
                }
                data-testid="delete-selected"
                onClick={requestDeleteSelection}
              >
                <Trash size={16} weight="bold" aria-hidden="true" />
                Delete
              </button>
            </div>
          ) : null}

          <aside className="canvas-legend" aria-live="polite">
            <span className="canvas-legend__line" aria-hidden="true" />
            <strong data-testid="route-count">
              {edges.length} {isLive ? "active" : "configured"} routes
            </strong>
            <p data-testid="routing-notice">{notice}</p>
          </aside>
          <div
            className="canvas-status"
            aria-label="Desktop engine status"
            data-testid="engine-status"
          >
            <PlugsConnected size={18} weight="fill" aria-hidden="true" />
            <span>
              {desktopHost
                ? desktopInfo
                  ? `Engine ${desktopInfo.version}`
                  : "Connecting to engine"
                : "Browser preview"}
            </span>
          </div>
        </div>
      </section>

      {streamMenu ? (
        <div
          className="stream-menu"
          style={{ left: streamMenu.x, top: streamMenu.y }}
          role="menu"
          data-testid="stream-context-menu"
        >
          <div className="stream-menu__title">{streamMenu.name}</div>
          <button
            type="button"
            role="menuitem"
            onClick={() => streamSdpRequestRef.current(streamMenu.nodeId, "view")}
          >
            <FileCode size={16} aria-hidden="true" />
            View SDP
          </button>
          <button
            type="button"
            role="menuitem"
            data-testid="copy-stream-sdp"
            onClick={() => streamSdpRequestRef.current(streamMenu.nodeId, "copy")}
          >
            <Copy size={16} aria-hidden="true" />
            Copy SDP
          </button>
        </div>
      ) : null}

      {sdpDialog ? (
        <div className="sdp-backdrop" role="presentation" onMouseDown={() => setSdpDialog(null)}>
          <section
            className="sdp-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="sdp-title"
            data-testid="sdp-dialog"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header>
              <div>
                <span>Session description</span>
                <h2 id="sdp-title">{sdpDialog.name}</h2>
              </div>
              <button type="button" aria-label="Close SDP" onClick={() => setSdpDialog(null)}>
                <X size={18} aria-hidden="true" />
              </button>
            </header>
            <pre>{sdpDialog.sdp}</pre>
            <footer>
              <span>Generated from the authoritative stream configuration.</span>
              <button
                type="button"
                onClick={() =>
                  void copyText(sdpDialog.sdp)
                    .then(() => setNotice(`${sdpDialog.name} SDP copied.`))
                    .catch((error) => setNotice(`Could not copy SDP: ${formatError(error)}`))
                }
              >
                <Copy size={16} aria-hidden="true" />
                Copy SDP
              </button>
            </footer>
          </section>
        </div>
      ) : null}

      {deleteDialog ? (
        <div
          className="delete-backdrop"
          role="presentation"
          onMouseDown={() => {
            if (!isDeleting) {
              setDeleteDialog(null);
            }
          }}
        >
          <section
            className="delete-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-dialog-title"
            aria-describedby="delete-dialog-description"
            data-testid="delete-dialog"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header>
              <Trash size={22} weight="bold" aria-hidden="true" />
              <div>
                <span>Confirm deletion</span>
                <h2 id="delete-dialog-title">
                  Delete {deleteDialog.blocks.length === 1 ? "this block" : "these blocks"}?
                </h2>
              </div>
            </header>
            <p id="delete-dialog-description">
              Connected routes will also be removed. This change cannot be undone.
            </p>
            <div className="delete-dialog__targets">
              {deleteDialog.blocks.map((block) => (
                <span key={block.nodeId}>
                  <small>{block.kind === "source" ? "Source" : "Stream"}</small>
                  {block.name}
                </span>
              ))}
            </div>
            <footer>
              <button
                className="delete-dialog__cancel"
                type="button"
                disabled={isDeleting}
                autoFocus
                onClick={() => setDeleteDialog(null)}
              >
                Cancel
              </button>
              <button
                className="delete-dialog__confirm"
                type="button"
                disabled={isDeleting}
                data-testid="confirm-delete"
                onClick={() => void confirmDeleteSelection()}
              >
                <Trash size={16} weight="bold" aria-hidden="true" />
                {isDeleting ? "Deleting..." : "Delete"}
              </button>
            </footer>
          </section>
        </div>
      ) : null}
    </main>
  );
}
