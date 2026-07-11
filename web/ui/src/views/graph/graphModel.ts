/*
 * Pure graph-canvas model (S-186, FR-UI-08) — the deterministic logic ported from
 * the legacy `web/assets/graph.js`, lifted out of the imperative canvas so it is
 * unit-testable without ECharts or a DOM. Owns: the loaded-set merge/dedup, the
 * layer/edge color + line-style palettes, node sizing, the locked-selection
 * adjacency set, the focus-depth BFS, the active-filter wire encoding, and the
 * honest "N more not shown" phrasing. No React, no ECharts, no fetch — every
 * function here is pure (NFR-RA-06 determinism).
 */

import type { GraphElementEdge, GraphElementNode, GraphLayer } from "../../api/types.ts";

/** The home-tier visible-element budget (graph.js `capForZoom(1)`) — the cap the
 *  opening snapshot and every re-fetch use. Defined once here so `GraphView`
 *  (opening fetch) and `GraphExplorer` (re-fetches) can never drift, which would
 *  desync the merge/elision counting. */
export const DEFAULT_CAP = 250;

// ── Palettes (mirror logos.css §1.2 / graph.js; ECharts paints to <canvas> and
// cannot read CSS custom properties, so the few hues it needs are reproduced). ──

/** Per-layer node fill, keyed by the wire `layer` token. */
export const LAYER_COLOR: Record<GraphLayer, string> = {
  code: "#2563eb", // --graph-layer-code
  doc: "#16a34a", // --so-green
  artifact: "#d97706", // --graph-layer-artifact
};

/** The red selection ring (focus/locked/located node) — a ring, never a fill swap. */
export const SELECTION_RING = "#da291c"; // --so-red
/** The neutral edge fallback for an un-typed (rollup) or unmapped edge. */
export const EDGE_FALLBACK = "#9ca3af";

/** Edge line style by relationship kind (solid / dashed / dotted only). */
export const EDGE_STYLE: Record<string, "solid" | "dashed" | "dotted"> = {
  calls: "solid",
  imports: "dashed",
  contains: "solid",
  references: "dotted",
  type_uses: "dotted",
  implements: "solid",
  extends: "solid",
  instantiates: "solid",
  routes_to: "dashed",
  doc_reference: "dotted",
  traces_to: "dashed",
  accesses: "dashed",
  artifact_ref: "dashed",
  artifact_binding: "dashed",
  forbidden_dependency: "solid",
  // The cross-service relation arms (S-250, CR-061) — the service map draws its
  // edges through this same canvas, so its arms live in the same palette and get
  // the same legend grammar. Additive: no intra-repo edge type carries these
  // tokens (the intra-repo route edge is `routes_to`).
  route: "dashed",
  "grpc-call": "solid",
  "broker-topic": "dotted",
  // The first-class broker edges (S-256, FR-WS-11). These are REAL graph edge kinds
  // — a `producer --publishes--> topic` in the node views, and the two hops the
  // service map now draws through a topic node — not a cross-service rollup arm.
  publishes: "solid",
  subscribes: "dashed",
};

/** Edge color by relationship kind — distinct, mutually-legible hues (CR-030). */
export const EDGE_COLOR: Record<string, string> = {
  calls: "#3d3935",
  imports: "#57534e",
  contains: "#78716c",
  references: "#a8a29e",
  type_uses: "#0891b2",
  implements: "#7c3aed",
  extends: "#c026d3",
  instantiates: "#4f46e5",
  accesses: "#0e7490",
  routes_to: "#db2777",
  doc_reference: "#9333ea",
  traces_to: "#9333ea",
  artifact_ref: "#92400e",
  artifact_binding: "#be123c",
  forbidden_dependency: "#da291c",
  // The cross-service relation arms (S-250) — distinct, mutually-legible hues in
  // the established palette, so a service map reads at a glance which boundary a
  // coupling crosses.
  route: "#db2777",
  "grpc-call": "#7c3aed",
  "broker-topic": "#0891b2",
  // The first-class broker edges (S-256) share the broker arm's teal, so a topic's
  // two hops read as one coupling: `publishes` into it, `subscribes` out of it.
  publishes: "#0891b2",
  subscribes: "#0891b2",
};

/** Node sizing — a base scaled gently by degree; the selection is bumped so it pops. */
export const NODE_BASE = 12;
export const NODE_MAX = 34;
export const NODE_FOCUS = 26;

/** The fill for a layer, defaulting to code for an unknown/rollup layer. */
export function layerColor(layer: GraphLayer | null): string {
  return (layer && LAYER_COLOR[layer]) || LAYER_COLOR.code;
}

/** The edge color for a wire type, defaulting to the neutral hairline. */
export function edgeColor(type: string | null): string {
  return (type && EDGE_COLOR[type]) || EDGE_FALLBACK;
}

/** The edge line style for a wire type, defaulting to solid. */
export function edgeStyle(type: string | null): "solid" | "dashed" | "dotted" {
  return (type && EDGE_STYLE[type]) || "solid";
}

/** Diameter for a node at a given visible degree, bumped to the focus size when selected. */
export function nodeSize(degree: number, selected: boolean): number {
  let size = Math.min(NODE_BASE + degree * 1.6, NODE_MAX);
  if (selected) size = Math.max(size, NODE_FOCUS);
  return size;
}

/** A human label for a wire token: `type_uses` → `type uses`. */
export function prettify(token: string): string {
  return String(token).replace(/_/g, " ");
}

// ── The loaded element set ──────────────────────────────────────────────────
// The master set of everything fetched so far, deduped by node id and by the
// (source,target,edge_type) edge key. Filters/focus/depth derive the rendered
// view from it without re-fetching, exactly as the legacy canvas does.

/** The accumulated, deduped element set the canvas renders from. */
export interface LoadedSet {
  /** id → node. */
  nodes: Record<string, GraphElementNode>;
  /** Deduped edges. */
  edges: GraphElementEdge[];
}

/** An empty loaded set. */
export function emptyLoaded(): LoadedSet {
  return { nodes: {}, edges: [] };
}

/** The dedup key for an edge: its endpoints + type. */
export function edgeKey(e: GraphElementEdge): string {
  return `${e.source}->${e.target}:${e.edge_type ?? ""}`;
}

/** How many nodes/edges a merge newly admitted. */
export interface MergeDelta {
  nodes: number;
  edges: number;
}

/**
 * Merge a fetched snapshot into `set` (mutating it), deduping nodes by id and
 * edges by {@link edgeKey}. Returns the count of newly-added elements so the
 * Expand control can draw the "N more not shown" counter down by exactly what it
 * admitted (the legacy `mergeLoaded` contract).
 */
export function mergeInto(
  set: LoadedSet,
  nodes: GraphElementNode[],
  edges: GraphElementEdge[],
): MergeDelta {
  let addedNodes = 0;
  let addedEdges = 0;
  const seenKeys = new Set(set.edges.map(edgeKey));
  for (const n of nodes) {
    if (!set.nodes[n.id]) {
      set.nodes[n.id] = n;
      addedNodes++;
    }
  }
  for (const e of edges) {
    const key = edgeKey(e);
    if (!seenKeys.has(key)) {
      seenKeys.add(key);
      set.edges.push(e);
      addedEdges++;
    }
  }
  return { nodes: addedNodes, edges: addedEdges };
}

/** A fresh loaded set from a snapshot (focus/reset replace the master set). */
export function loadedFrom(nodes: GraphElementNode[], edges: GraphElementEdge[]): LoadedSet {
  const set = emptyLoaded();
  mergeInto(set, nodes, edges);
  return set;
}

/** A shallow clone of a loaded set — so a merge produces a NEW reference React sees. */
export function cloneLoaded(set: LoadedSet): LoadedSet {
  return { nodes: { ...set.nodes }, edges: [...set.edges] };
}

// ── Derived views over the loaded set ─────────────────────────────────────────

/**
 * The node ids visible under the current focus-depth bound. The layer/edge-type
 * filters are server-side re-budgeting params (the loaded set already excludes
 * deselected kinds), so the only client-side narrowing here is the focus-mode hop
 * limit. `depth <= 0` (or no focus) means every loaded node is visible.
 */
export function visibleNodeIds(set: LoadedSet, focusId: string | null, depth: number): Set<string> {
  if (focusId && depth > 0) return nodeIdsWithinDepth(set, focusId, depth);
  return new Set(Object.keys(set.nodes));
}

/** Breadth-first node ids within `depth` undirected hops of `rootId` (inclusive). */
export function nodeIdsWithinDepth(set: LoadedSet, rootId: string, depth: number): Set<string> {
  const seen = new Set<string>([rootId]);
  let frontier = [rootId];
  let d = 0;
  while (frontier.length && d < depth) {
    const next: string[] = [];
    for (const cur of frontier) {
      for (const e of set.edges) {
        if (e.source === cur && !seen.has(e.target)) {
          seen.add(e.target);
          next.push(e.target);
        } else if (e.target === cur && !seen.has(e.source)) {
          seen.add(e.source);
          next.push(e.source);
        }
      }
    }
    frontier = next;
    d++;
  }
  return seen;
}

/** Visible-degree of each node (edges with both endpoints visible) — drives sizing. */
export function degreeMap(set: LoadedSet, visible: Set<string>): Record<string, number> {
  const deg: Record<string, number> = {};
  for (const e of set.edges) {
    if (visible.has(e.source) && visible.has(e.target)) {
      deg[e.source] = (deg[e.source] || 0) + 1;
      deg[e.target] = (deg[e.target] || 0) + 1;
    }
  }
  return deg;
}

/**
 * The locked node plus its directly-connected visible neighbours — the set kept
 * bright while the rest dims (adjacency emphasis, S-119). Always contains the
 * locked id itself. `null` when nothing is locked or the locked node is filtered
 * out, so nothing dims.
 */
export function adjacencySet(
  set: LoadedSet,
  lockedId: string | null,
  visible: Set<string>,
): Set<string> | null {
  if (!lockedId || !visible.has(lockedId)) return null;
  const adj = new Set<string>([lockedId]);
  for (const e of set.edges) {
    if (!visible.has(e.source) || !visible.has(e.target)) continue;
    if (e.source === lockedId) adj.add(e.target);
    else if (e.target === lockedId) adj.add(e.source);
  }
  return adj;
}

// ── Active-filter wire encoding (S-122 server-side re-budgeting) ──────────────

const ALL_LAYERS: readonly GraphLayer[] = ["code", "doc", "artifact"];

/**
 * The enabled layers as a comma-joined wire list, or `undefined` when every layer
 * is on (so the request omits the filter and the server returns the whole scope).
 * All three deselected encodes as `""` (the honest empty graph), distinct from
 * `undefined`.
 */
export function activeLayersParam(layers: Record<GraphLayer, boolean>): string | undefined {
  const on = ALL_LAYERS.filter((l) => layers[l] !== false);
  return on.length === ALL_LAYERS.length ? undefined : on.join(",");
}

/**
 * The enabled edge types as a comma-joined wire list, or `undefined` when none are
 * known yet or every known type is on (omit the filter). Once at least one type is
 * deselected the enabled subset is sent — possibly `""` ("hide every edge").
 */
export function activeEdgeTypesParam(edgeTypes: Record<string, boolean>): string | undefined {
  const keys = Object.keys(edgeTypes);
  if (keys.length === 0) return undefined;
  const anyOff = keys.some((t) => edgeTypes[t] === false);
  if (!anyOff) return undefined;
  return keys.filter((t) => edgeTypes[t] !== false).join(",");
}

/**
 * The union of edge types in the loaded set and those already tracked, each
 * defaulting to shown — so a deselected type (which vanishes from the loaded set
 * under server-side filtering) keeps its checkbox and can be re-enabled. Sorted
 * for a stable control order.
 */
export function knownEdgeTypes(set: LoadedSet, tracked: Record<string, boolean>): string[] {
  const types = new Set<string>(Object.keys(tracked));
  for (const e of set.edges) {
    if (e.edge_type) types.add(e.edge_type);
  }
  return [...types].sort();
}

// ── Honest "N more not shown" phrasing (NFR-CC-04) ────────────────────────────

/** "X node(s) and Y edge(s)" for the cap notice; omits a zero side. Empty when both 0. */
export function elementPhrase(nodes: number, edges: number): string {
  const parts: string[] = [];
  if (nodes > 0) parts.push(`${nodes} ${nodes === 1 ? "node" : "nodes"}`);
  if (edges > 0) parts.push(`${edges} ${edges === 1 ? "edge" : "edges"}`);
  return parts.join(" and ");
}

/** The "N more not shown" notice, or `null` when nothing was elided. */
export function capNotice(elidedNodes: number, elidedEdges: number): string | null {
  if (elidedNodes === 0 && elidedEdges === 0) return null;
  return `${elementPhrase(elidedNodes, elidedEdges)} not shown — lock a node and use “Expand neighbours” to reveal more.`;
}

/** The human layer label (color is never the only signal — frontend-design §7). */
export function layerLabel(layer: GraphLayer | null): string {
  return layer ?? "cluster";
}
