/*
 * Pure ECharts option builder (S-186, FR-UI-08) — ported from the legacy
 * `graph.js` `buildSeries`/`nodeOption`/`linkOption`/`seriesSkeleton`. Given the
 * loaded set and the current view selection it returns a plain ECharts `series`
 * object; it touches no chart instance and no DOM, so it is unit-testable on its
 * own. The imperative `GraphCanvas` calls this and hands the result to
 * `chart.setOption`.
 *
 * The visual grammar it reproduces verbatim from the legacy canvas:
 *   - node fill = layer color; selection is a red RING, never a fill swap (CR-030)
 *   - node size scales with visible degree; the selection is bumped + centre-pinned
 *   - locked-selection adjacency emphasis dims everything outside the neighbourhood
 *   - edges colored + line-styled by type; a forbidden edge stays heaviest.
 */

import type { GraphLayer } from "../../api/types.ts";
import {
  adjacencySet,
  degreeMap,
  edgeColor,
  edgeStyle,
  EDGE_FALLBACK,
  layerColor,
  nodeSize,
  SELECTION_RING,
  visibleNodeIds,
  type LoadedSet,
} from "./graphModel.ts";

/** Label level-of-detail: hide labels for a large set, or when zoomed far out. */
export const LABEL_NODE_LIMIT = 60;
export const LABEL_ZOOM_THRESHOLD = 0.9;

/** The current selection/scope the option builder reads. */
export interface GraphSelection {
  seed: string | null;
  focusId: string | null;
  lockedId: string | null;
  locatedId: string | null;
  depth: number;
}

/** The canvas centre in layout pixel space — the selection is pinned here. */
export type Center = readonly [number, number];

/** Whether labels should be shown given the visible count and tracked zoom. */
export function labelsVisible(visibleCount: number, zoom: number): boolean {
  return visibleCount <= LABEL_NODE_LIMIT || zoom >= LABEL_ZOOM_THRESHOLD;
}

/** The node-label style; `show` is the level-of-detail gate computed per build. */
function labelStyle(show: boolean) {
  return {
    show,
    position: "right" as const,
    distance: 4,
    fontFamily: "JetBrains Mono, ui-monospace, monospace",
    fontSize: 11,
    color: "#3d3935",
    formatter: (p: { data?: { displayLabel?: string }; name?: string }) =>
      p.data?.displayLabel ?? p.name ?? "",
  };
}

/** The static series styling (everything except data/links/label). */
export function seriesSkeleton(scaleMin: number, scaleMax: number) {
  return {
    type: "graph" as const,
    layout: "force" as const,
    roam: true,
    draggable: true,
    scaleLimit: { min: scaleMin, max: scaleMax },
    // No explicit `zoom` here. A restyle commit (`setOption` merge — selection
    // change, label LOD, or a roam-driven recommit) must NOT carry a zoom field:
    // ECharts would reset the roamed camera back to it on every merge, snapping a
    // gesture/button zoom straight back to home. Omitting it lets the merge keep
    // the current roam transform; a full `replaceMerge` (a view change) discards
    // the model and naturally re-opens at home zoom (1) — which is exactly the
    // legacy `resetZoom=true` behaviour the canvas mirrors.
    force: {
      repulsion: 420,
      edgeLength: [120, 260],
      gravity: 0.035,
      friction: 0.18,
      layoutAnimation: true,
    },
    itemStyle: {
      borderColor: "#ffffff",
      borderWidth: 1.5,
      shadowColor: "rgba(61,57,53,0.18)",
      shadowBlur: 4,
    },
    lineStyle: { color: EDGE_FALLBACK, width: 2, opacity: 0.7, curveness: 0.08 },
    emphasis: {
      focus: "none" as const,
      scale: true,
      label: { show: true, fontWeight: 600 },
      lineStyle: { width: 3, opacity: 1 },
    },
    edgeSymbol: ["none", "arrow"] as [string, string],
    edgeSymbolSize: 6,
  };
}

/** One ECharts node datum. */
interface NodeDatum {
  id: string;
  name: string;
  displayLabel: string;
  kind: string | null;
  layer: GraphLayer;
  symbolSize: number;
  itemStyle: { color: string; borderColor?: string; borderWidth?: number; opacity?: number };
  fixed: boolean;
  x?: number;
  y?: number;
}

/** Build one node datum (ported from graph.js `nodeOption`). */
function nodeDatum(
  node: { id: string; label: string; kind: string | null; layer: GraphLayer },
  degree: number,
  adj: Set<string> | null,
  center: Center,
  sel: GraphSelection,
): NodeDatum {
  const isFocus = node.id === sel.focusId || node.id === sel.seed;
  const isLocked = node.id === sel.lockedId;
  const isLocated = node.id === sel.locatedId;
  const isSelected = isLocked || isLocated;
  const isPinned = isFocus || isSelected;
  const itemStyle: NodeDatum["itemStyle"] = { color: layerColor(node.layer) };
  if (isFocus || isSelected) {
    // The selection draws a persistent red ring; the layer color survives under it.
    itemStyle.borderColor = SELECTION_RING;
    itemStyle.borderWidth = 3;
  }
  // Adjacency emphasis: dim any node outside the locked neighbourhood.
  if (adj && !adj.has(node.id) && !isSelected) {
    itemStyle.opacity = 0.2;
  }
  const datum: NodeDatum = {
    id: node.id,
    name: node.id, // ECharts resolves links by node name → keep it the unique id
    displayLabel: node.label,
    kind: node.kind,
    layer: node.layer,
    symbolSize: nodeSize(degree, isPinned),
    itemStyle,
    // Pin the selection to centre so a re-layout never flings it out of view.
    fixed: isPinned,
  };
  if (isPinned) {
    datum.x = center[0];
    datum.y = center[1];
  }
  return datum;
}

/** Build one edge (link) datum (ported from graph.js `linkOption`). */
function linkDatum(
  edge: { source: string; target: string; edge_type: string | null },
  adj: Set<string> | null,
  sel: GraphSelection,
) {
  const forbidden = edge.edge_type === "forbidden_dependency";
  const dimmed = !!adj && edge.source !== sel.lockedId && edge.target !== sel.lockedId;
  return {
    source: edge.source,
    target: edge.target,
    edge_type: edge.edge_type,
    lineStyle: {
      color: edgeColor(edge.edge_type),
      type: edgeStyle(edge.edge_type),
      width: forbidden ? 2.6 : 2,
      opacity: dimmed ? 0.1 : forbidden ? 0.95 : 0.82,
      curveness: 0.08,
    },
  };
}

/** The graph series {@link buildSeries} produces — a skeleton plus label/data/links. */
export type GraphSeries = ReturnType<typeof seriesSkeleton> & {
  label: ReturnType<typeof labelStyle>;
  data: NodeDatum[];
  links: ReturnType<typeof linkDatum>[];
};

/**
 * Build the full graph series from the loaded set and the current selection/scope
 * — the single source the canvas commits on any view change. Mirrors graph.js
 * `buildSeries`: visible-id derivation, degree-driven sizing, locked-adjacency
 * emphasis, selection ring + centre-pin, and the label level-of-detail gate.
 */
export function buildSeries(
  set: LoadedSet,
  sel: GraphSelection,
  center: Center,
  zoom: number,
  scaleMin: number,
  scaleMax: number,
): GraphSeries {
  const visible = visibleNodeIds(set, sel.focusId, sel.depth);
  const degree = degreeMap(set, visible);
  const adj = adjacencySet(set, sel.lockedId, visible);

  const data: NodeDatum[] = [];
  for (const id of Object.keys(set.nodes)) {
    if (visible.has(id)) data.push(nodeDatum(set.nodes[id], degree[id] || 0, adj, center, sel));
  }
  const links = set.edges
    .filter((e) => visible.has(e.source) && visible.has(e.target))
    .map((e) => linkDatum(e, adj, sel));

  const skeleton = seriesSkeleton(scaleMin, scaleMax);
  return { ...skeleton, label: labelStyle(labelsVisible(data.length, zoom)), data, links };
}
