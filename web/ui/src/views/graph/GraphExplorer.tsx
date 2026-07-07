/*
 * GraphExplorer (S-186, FR-UI-08) — the stateful interactive graph: it owns the
 * accumulated element set and the canvas selection/filters, composes the imperative
 * `GraphCanvas` with the React-owned controls, query bar, Decisions panel, and the
 * accessible table, and drives every interaction through the read-only
 * `/api/v1/graph` feed. It is seeded with the first snapshot (loaded by `GraphView`
 * through the shared async hook); thereafter filter/focus/expand/reset re-fetch
 * imperatively so the merge-vs-replace + "N more not shown" honesty matches the
 * legacy canvas. No interaction mutates a store (ADR-28).
 */

import { useMemo, useRef, useState } from "react";

import { fetchGraph } from "../../api/index.ts";
import type { GraphElements, GraphGranularity, GraphLayer } from "../../api/types.ts";
import { Callout } from "../../components/index.ts";
import { GraphCanvas, type GraphCanvasHandle } from "./GraphCanvas.tsx";
import { GraphControls, type EdgeFilter } from "./GraphControls.tsx";
import { GraphQuery } from "./GraphQuery.tsx";
import { GraphTable } from "./GraphTable.tsx";
import { DecisionsPanel } from "./DecisionsPanel.tsx";
import { Legend } from "./Legend.tsx";
import {
  activeEdgeTypesParam,
  activeLayersParam,
  capNotice,
  cloneLoaded,
  DEFAULT_CAP,
  elementPhrase,
  knownEdgeTypes,
  loadedFrom,
  mergeInto,
  type LoadedSet,
} from "./graphModel.ts";
import styles from "./GraphView.module.css";

type Layers = Record<GraphLayer, boolean>;
const ALL_LAYERS_ON: Layers = { code: true, doc: true, artifact: true };

interface Meta {
  cap: number;
  elidedNodes: number;
  elidedEdges: number;
  totalNodes: number;
  totalEdges: number;
  seed: string | null;
}

function metaFrom(e: GraphElements): Meta {
  return {
    cap: e.cap,
    elidedNodes: e.elided_nodes,
    elidedEdges: e.elided_edges,
    totalNodes: e.total_nodes,
    totalEdges: e.total_edges,
    seed: e.seed,
  };
}

export function GraphExplorer({ initial }: { initial: GraphElements }) {
  const [loaded, setLoaded] = useState<LoadedSet>(() => loadedFrom(initial.nodes, initial.edges));
  const [meta, setMeta] = useState<Meta>(() => metaFrom(initial));
  const [layers, setLayers] = useState<Layers>(ALL_LAYERS_ON);
  const [edgeTypes, setEdgeTypes] = useState<Record<string, boolean>>({});
  const [intent, setIntent] = useState(false);
  const [granularity, setGranularity] = useState<GraphGranularity>("symbol");
  const [depth, setDepth] = useState(0);
  const [focusId, setFocusId] = useState<string | null>(null);
  const [lockedId, setLockedId] = useState<string | null>(null);
  const [locatedId, setLocatedId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  // The accessibility table's own view: null = show the full loaded set (no
  // selection); non-null = the fetched 1-hop neighbourhood of the locked node.
  const [tableSet, setTableSet] = useState<LoadedSet | null>(null);
  const canvasRef = useRef<GraphCanvasHandle>(null);
  // A monotonic guard for the imperative re-fetches (filter/focus/expand/reset):
  // only the latest issued fetch may commit, so two quick toggles can't let a slow
  // earlier response clobber a newer one (mirrors the `useApiResource` guard).
  const fetchGen = useRef(0);
  // Independent generation counter for the table's neighbourhood fetch — the table
  // and the canvas re-fetch concurrently without coupling.
  const tableFetchGen = useRef(0);
  // The freshest loaded set, so a merge (expand) bases off current state rather
  // than the closure captured when the async fetch was issued.
  const loadedRef = useRef(loaded);
  loadedRef.current = loaded;

  const selection = useMemo(
    () => ({ seed: meta.seed, focusId, lockedId, locatedId, depth }),
    [meta.seed, focusId, lockedId, locatedId, depth],
  );

  const edgeFilters: EdgeFilter[] = knownEdgeTypes(loaded, edgeTypes).map((type) => ({
    type,
    on: edgeTypes[type] !== false,
  }));

  /** Build the `/api/v1/graph` params from the current filters, honoring overrides. */
  function paramsFrom(over: {
    layers?: Layers;
    edgeTypes?: Record<string, boolean>;
    granularity?: GraphGranularity;
    intent?: boolean;
  }) {
    return {
      cap: DEFAULT_CAP,
      layers: activeLayersParam(over.layers ?? layers),
      edge_types: activeEdgeTypesParam(over.edgeTypes ?? edgeTypes),
      granularity: over.granularity ?? granularity,
      intent: over.intent ?? intent,
    };
  }

  /** Commit a fetched snapshot, replacing or merging the loaded set + meta. */
  function commit(data: GraphElements, mode: "replace" | "merge") {
    const base = mode === "merge" ? cloneLoaded(loadedRef.current) : loadedFrom([], []);
    const delta = mergeInto(base, data.nodes, data.edges);
    setLoaded(base);
    if (mode === "merge") {
      // The admitted neighbours came from the elided set — draw the counter down.
      setMeta((m) => ({
        ...m,
        elidedNodes: Math.max(0, m.elidedNodes - delta.nodes),
        elidedEdges: Math.max(0, m.elidedEdges - delta.edges),
      }));
    } else {
      setMeta(metaFrom(data));
    }
    // Newly-seen edge types default to shown; tracked (deselected) ones persist.
    setEdgeTypes((prev) => {
      const next = { ...prev };
      for (const t of knownEdgeTypes(base, prev)) {
        if (next[t] === undefined) next[t] = true;
      }
      return next;
    });
    return delta;
  }

  /**
   * Reload the accessibility table to the 1-hop neighbourhood of `id` via the
   * existing `?seed=<id>` graph-elements load (FR-UI-08, S-198). Passing `null`
   * clears the neighbourhood view so the table falls back to the full loaded set.
   * Uses its own generation counter so it never races with the canvas re-fetch.
   */
  async function reloadTable(id: string | null): Promise<void> {
    if (!id) {
      setTableSet(null);
      return;
    }
    const gen = ++tableFetchGen.current;
    try {
      const data = await fetchGraph({ seed: id, ...paramsFrom({}) });
      if (gen !== tableFetchGen.current) return;
      setTableSet(loadedFrom(data.nodes, data.edges));
    } catch {
      // On error the table stays on the full loaded set; this is silent — the
      // canvas already has its own error path and the table is a secondary view.
      if (gen === tableFetchGen.current) setTableSet(null);
    }
  }

  /** Fetch a scope and commit it; `over` carries any just-changed filter value. */
  async function reload(
    scope: string,
    mode: "replace" | "merge",
    over: Parameters<typeof paramsFrom>[0] = {},
  ): Promise<void> {
    const gen = ++fetchGen.current;
    setBusy(true);
    try {
      const data = await fetchGraph({ seed: scope || undefined, ...paramsFrom(over) });
      if (gen !== fetchGen.current) return; // a newer re-fetch superseded this one
      commit(data, mode);
    } catch {
      if (gen === fetchGen.current) setNotice("Could not load the graph elements.");
    } finally {
      if (gen === fetchGen.current) setBusy(false);
    }
  }

  const scope = () => focusId ?? "";
  const labelFor = (id: string) => loaded.nodes[id]?.label ?? id;

  // ── Interaction handlers ────────────────────────────────────────────────────

  function onNodeClick(id: string) {
    if (lockedId === id) {
      setLockedId(null);
      setLocatedId(null);
      tableFetchGen.current++; // cancel any in-flight neighbourhood fetch
      setTableSet(null);
      setNotice("Selection unlocked.");
    } else {
      setLockedId(id);
      setNotice(`Locked ${labelFor(id)} — Expand to grow it, or click it again to unlock.`);
      void reloadTable(id);
    }
  }

  async function focusOn(id: string) {
    if (!id) return;
    setFocusId(id);
    setLockedId(id);
    setLocatedId(id);
    setGranularity("symbol");
    tableFetchGen.current++; // cancel any in-flight table neighbourhood fetch
    const gen = ++fetchGen.current;
    setBusy(true);
    try {
      const data = await fetchGraph({ seed: id, ...paramsFrom({ granularity: "symbol" }) });
      if (gen !== fetchGen.current) return;
      commit(data, "replace");
      // Reuse the already-fetched neighbourhood for the accessibility table —
      // no second request needed; the seed-scoped response is the 1-hop view.
      setTableSet(loadedFrom(data.nodes, data.edges));
      setNotice(`Focused on ${data.nodes.find((n) => n.id === id)?.label ?? id} — Reset to return.`);
    } catch {
      if (gen === fetchGen.current) {
        setNotice("Could not focus on that node.");
        setFocusId(null);
        setLockedId(null);
        setLocatedId(null);
      }
    } finally {
      if (gen === fetchGen.current) setBusy(false);
    }
  }

  async function expand() {
    if (!lockedId) {
      setNotice("Lock a node first, then expand its neighbours.");
      return;
    }
    const gen = ++fetchGen.current;
    setBusy(true);
    try {
      const data = await fetchGraph({ seed: lockedId, ...paramsFrom({}) });
      if (gen !== fetchGen.current) return;
      if (focusId) setFocusId(lockedId);
      const delta = commit(data, "merge");
      if (delta.nodes > 0 || delta.edges > 0) {
        setNotice(`Expanded ${labelFor(lockedId)} — added ${elementPhrase(delta.nodes, delta.edges)}.`);
      } else {
        setNotice(`“${labelFor(lockedId)}” is already fully expanded — no further neighbours.`);
      }
    } catch {
      if (gen === fetchGen.current) setNotice("Could not expand the neighbours.");
    } finally {
      if (gen === fetchGen.current) setBusy(false);
    }
  }

  function toggleLayer(layer: GraphLayer) {
    const next = { ...layers, [layer]: !layers[layer] };
    setLayers(next);
    void reload(scope(), "replace", { layers: next });
  }

  function toggleEdge(type: string) {
    const next = { ...edgeTypes, [type]: edgeTypes[type] === false };
    setEdgeTypes(next);
    void reload(scope(), "replace", { edgeTypes: next });
  }

  function toggleIntent(on: boolean) {
    setIntent(on);
    void reload(scope(), "replace", { intent: on });
    setNotice(on ? "Intent / governing docs layer shown." : "Intent / governing docs layer hidden.");
  }

  function changeGranularity(tier: GraphGranularity) {
    if (tier === granularity) return;
    setGranularity(tier);
    // A tier change swaps the node id-space — clear scope/selection cleanly.
    setFocusId(null);
    setLockedId(null);
    setLocatedId(null);
    setEdgeTypes({});
    tableFetchGen.current++; // cancel any in-flight table neighbourhood fetch
    setTableSet(null);
    void reload("", "replace", { granularity: tier, edgeTypes: {} });
  }

  function reset() {
    setLayers(ALL_LAYERS_ON);
    setEdgeTypes({});
    setIntent(false);
    setGranularity("symbol");
    setDepth(0);
    setFocusId(null);
    setLockedId(null);
    setLocatedId(null);
    tableFetchGen.current++; // cancel any in-flight table neighbourhood fetch
    setTableSet(null);
    setNotice("Showing the whole graph.");
    void reload("", "replace", {
      layers: ALL_LAYERS_ON,
      edgeTypes: {},
      intent: false,
      granularity: "symbol",
    });
  }

  const notShown = capNotice(meta.elidedNodes, meta.elidedEdges);
  const scopeText = meta.seed
    ? `${meta.seed} — neighbourhood`
    : "Whole graph";

  return (
    <div className={styles.view}>
      <Callout label="Graph" tone="signal">
        <span>
          {scopeText}: {meta.totalNodes} node(s), {meta.totalEdges} edge(s)
        </span>
      </Callout>

      <GraphQuery onSelect={focusOn} />

      <div className={styles.canvasArea}>
        <GraphControls
          layers={layers}
          onToggleLayer={toggleLayer}
          intent={intent}
          onToggleIntent={toggleIntent}
          granularity={granularity}
          onGranularity={changeGranularity}
          depth={depth}
          onDepth={setDepth}
          edgeFilters={edgeFilters}
          onToggleEdge={toggleEdge}
          canExpand={lockedId !== null}
          onExpand={() => void expand()}
          onReset={reset}
          onZoomIn={() => canvasRef.current?.zoomIn()}
          onZoomOut={() => canvasRef.current?.zoomOut()}
          notice={notice}
        />
        <div className={styles.canvasStage}>
          <GraphCanvas ref={canvasRef} loaded={loaded} selection={selection} onNodeClick={onNodeClick} busy={busy} />
          <Legend />
        </div>
        {notShown && (
          <p className={styles.capNotice} role="status" aria-live="polite">
            {notShown}
          </p>
        )}
      </div>

      <DecisionsPanel seed={lockedId} onFocus={focusOn} />

      <GraphTable
        loaded={tableSet ?? loaded}
        hoodOf={lockedId ? labelFor(lockedId) : undefined}
      />
    </div>
  );
}
