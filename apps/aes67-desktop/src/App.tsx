import { useCallback, useEffect, useRef, useState } from "react";
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
import {
  Broadcast,
  CheckCircle,
  FileAudio,
  PlugsConnected,
  Plus,
  Radio,
  Stop,
  Waveform,
} from "@phosphor-icons/react";
import {
  assignSource,
  createSource,
  createStream,
  getDesktopInfo,
  getRoutingSnapshot,
  isDesktopHost,
  removeRoute,
  updateStream,
} from "./desktop";
import type { DesktopInfo, RoutingSnapshot, SourceInput, StreamConfig } from "./types";

const routeStyle = {
  stroke: "#ff9d00",
  strokeWidth: 2.5,
};

type SourceNodeData = {
  name: string;
  detail: string;
  kind: string;
};

type StreamNodeData = {
  name: string;
  detail: string;
  format: string;
  gainDb: number | null;
  onGainCommit?: (gainDb: number | null) => void;
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
      <p>{data.detail}</p>
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

  return (
    <article className="module-node module-node--stream" data-testid={id}>
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
        <span className="module-node__ready">
          <CheckCircle size={15} weight="fill" aria-hidden="true" />
          Configured
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
  return { x: type === "source" ? 110 : 780, y: 104 + moduleCount * 224 };
}

function getSourcePresentation(input: SourceInput): Pick<SourceNodeData, "kind" | "detail"> {
  if ("File" in input) {
    return { kind: "Audio file", detail: input.File.path };
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

export function App() {
  const desktopHost = isDesktopHost();
  const [nodes, setNodes, onNodesChange] = useNodesState<AppNode>(initialNodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState(initialEdges);
  const [desktopInfo, setDesktopInfo] = useState<DesktopInfo | null>(null);
  const [isLive, setIsLive] = useState(false);
  const [notice, setNotice] = useState("Drag an output handle onto a stream input to create a route.");
  const sourceSequence = useRef(4);
  const streamSequence = useRef(4);
  const streamGainCommitRef = useRef(
    (_streamId: number, _config: StreamConfig, _gainDb: number | null) => {},
  );

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
            },
          };
        });
        const streamNodes: StreamFlowNode[] = snapshot.streams.map((stream, index) => {
          const id = `stream-${stream.id}`;
          return {
            id,
            type: "stream",
            position: existingPositions.get(id) ?? { x: 780, y: 104 + index * 224 },
            deletable: false,
            data: {
              name: stream.config.name,
              detail: `${stream.config.address}:${stream.config.port}`,
              format: "48 kHz · source channels",
              gainDb: stream.config.gain_db,
              onGainCommit: (gainDb) =>
                streamGainCommitRef.current(stream.id, stream.config, gainDb),
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
    if (!desktopHost) {
      return;
    }

    let cancelled = false;
    Promise.all([getDesktopInfo(), getRoutingSnapshot()])
      .then(([info, snapshot]) => {
        if (cancelled) {
          return;
        }
        setDesktopInfo(info);
        applySnapshot(snapshot);
        setNotice(`Engine model connected · revision ${snapshot.revision}`);
      })
      .catch((error) => {
        if (!cancelled) {
          setNotice(`Desktop bridge failed: ${formatError(error)}`);
        }
      });

    return () => {
      cancelled = true;
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
          setNotice("Source created. Device selection is the next editor step.");
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

  function toggleLive() {
    if (!edges.length) {
      setNotice("Create at least one source-to-stream route before starting.");
      return;
    }
    if (desktopHost && desktopInfo && !desktopInfo.liveRoutingAvailable) {
      setNotice("Routing is saved. Live transport unlocks after shared PTP runtime integration.");
      return;
    }
    setIsLive((current) => !current);
    setNotice(isLive ? "All routes are standing by." : `${edges.length} routes are now live.`);
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
          {isLive ? "Live" : "Standby"}
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
              onClick={toggleLive}
            >
              {isLive ? (
                <Stop size={18} weight="fill" aria-hidden="true" />
              ) : (
                <Waveform size={20} weight="bold" aria-hidden="true" />
              )}
              {isLive ? "Stop all" : "Start all"}
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
            onConnect={onConnect}
            onNodeClick={(_, node) =>
              setNotice(`${node.data.name} can be moved anywhere on the canvas.`)
            }
            onEdgeClick={() => setNotice("Selected route. Press Delete or Backspace to remove it.")}
            onPaneClick={() =>
              setNotice("Drag from any source output to assign or reassign a stream input.")
            }
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
    </main>
  );
}
