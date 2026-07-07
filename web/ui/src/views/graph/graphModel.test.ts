import { describe, expect, it } from "vitest";

import type { GraphElementEdge, GraphElementNode } from "../../api/types.ts";
import {
  activeEdgeTypesParam,
  activeLayersParam,
  adjacencySet,
  capNotice,
  cloneLoaded,
  edgeColor,
  edgeStyle,
  elementPhrase,
  knownEdgeTypes,
  layerColor,
  loadedFrom,
  mergeInto,
  nodeIdsWithinDepth,
  nodeSize,
  visibleNodeIds,
} from "./graphModel.ts";

const node = (id: string, layer: GraphElementNode["layer"] = "code"): GraphElementNode => ({
  id,
  label: id,
  kind: "function",
  layer,
});
const edge = (source: string, target: string, edge_type = "calls"): GraphElementEdge => ({
  source,
  target,
  edge_type,
});

describe("loaded-set merge", () => {
  it("dedups nodes by id and edges by (source,target,type) and reports the delta", () => {
    const set = loadedFrom([node("a"), node("b")], [edge("a", "b")]);
    const delta = mergeInto(set, [node("b"), node("c")], [edge("a", "b"), edge("b", "c")]);
    expect(delta).toEqual({ nodes: 1, edges: 1 }); // only c and b->c are new
    expect(Object.keys(set.nodes).sort()).toEqual(["a", "b", "c"]);
    expect(set.edges).toHaveLength(2);
  });

  it("treats edges of different types between the same nodes as distinct", () => {
    const set = loadedFrom([node("a"), node("b")], [edge("a", "b", "calls")]);
    const delta = mergeInto(set, [], [edge("a", "b", "imports")]);
    expect(delta.edges).toBe(1);
    expect(set.edges).toHaveLength(2);
  });

  it("clones to a fresh reference so a merge produces a new object", () => {
    const set = loadedFrom([node("a")], []);
    const clone = cloneLoaded(set);
    expect(clone).not.toBe(set);
    expect(clone.nodes).not.toBe(set.nodes);
    mergeInto(clone, [node("b")], []);
    expect(Object.keys(set.nodes)).toEqual(["a"]); // original untouched
  });
});

describe("derived views", () => {
  const set = loadedFrom(
    [node("a"), node("b"), node("c"), node("d")],
    [edge("a", "b"), edge("b", "c"), edge("c", "d")],
  );

  it("BFS-bounds the visible set by focus depth (undirected, inclusive)", () => {
    expect([...nodeIdsWithinDepth(set, "a", 1)].sort()).toEqual(["a", "b"]);
    expect([...nodeIdsWithinDepth(set, "a", 2)].sort()).toEqual(["a", "b", "c"]);
    // depth 0 / no focus → every node visible
    expect(visibleNodeIds(set, null, 0).size).toBe(4);
    expect(visibleNodeIds(set, "a", 0).size).toBe(4);
  });

  it("builds the locked node's adjacency set, or null when nothing is locked", () => {
    const vis = visibleNodeIds(set, null, 0);
    const adj = adjacencySet(set, "b", vis);
    expect(adj && [...adj].sort()).toEqual(["a", "b", "c"]);
    expect(adjacencySet(set, null, vis)).toBeNull();
    // a locked node filtered out of the visible set dims nothing
    expect(adjacencySet(set, "zzz", vis)).toBeNull();
  });
});

describe("active-filter wire encoding (S-122)", () => {
  it("omits the layers param when all are on, and encodes the subset otherwise", () => {
    expect(activeLayersParam({ code: true, doc: true, artifact: true })).toBeUndefined();
    expect(activeLayersParam({ code: true, doc: false, artifact: true })).toBe("code,artifact");
    expect(activeLayersParam({ code: false, doc: false, artifact: false })).toBe("");
  });

  it("omits the edge-types param until one is deselected, then sends the enabled subset", () => {
    expect(activeEdgeTypesParam({})).toBeUndefined();
    expect(activeEdgeTypesParam({ calls: true, imports: true })).toBeUndefined();
    expect(activeEdgeTypesParam({ calls: true, imports: false })).toBe("calls");
    expect(activeEdgeTypesParam({ calls: false })).toBe("");
  });

  it("keeps a deselected type's checkbox by unioning tracked + loaded types", () => {
    const set = loadedFrom([], [edge("a", "b", "imports")]);
    expect(knownEdgeTypes(set, { calls: false })).toEqual(["calls", "imports"]);
  });
});

describe("honest cap notice (NFR-CC-04)", () => {
  it("phrases elided counts and omits a zero side", () => {
    expect(elementPhrase(2, 0)).toBe("2 nodes");
    expect(elementPhrase(1, 3)).toBe("1 node and 3 edges");
    expect(capNotice(0, 0)).toBeNull();
    expect(capNotice(5, 0)).toContain("5 nodes not shown");
  });
});

describe("palettes & sizing", () => {
  it("colors nodes by layer and edges by type with safe fallbacks", () => {
    expect(layerColor("doc")).toBe("#16a34a");
    expect(layerColor(null)).toBe(layerColor("code"));
    expect(edgeColor("forbidden_dependency")).toBe("#da291c");
    expect(edgeColor(null)).toBe("#9ca3af");
    expect(edgeStyle("imports")).toBe("dashed");
    expect(edgeStyle("nonexistent")).toBe("solid");
  });

  it("bumps the selected node to at least the focus size", () => {
    expect(nodeSize(0, false)).toBe(12);
    expect(nodeSize(0, true)).toBe(26);
    expect(nodeSize(100, false)).toBe(34); // capped at NODE_MAX
  });
});
