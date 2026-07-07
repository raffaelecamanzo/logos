import { describe, expect, it } from "vitest";

import type { GraphElementEdge, GraphElementNode } from "../../api/types.ts";
import { loadedFrom } from "./graphModel.ts";
import { buildSeries, labelsVisible, type GraphSelection } from "./graphOptions.ts";

const NO_SELECTION: GraphSelection = {
  seed: null,
  focusId: null,
  lockedId: null,
  locatedId: null,
  depth: 0,
};
const n = (id: string, layer: GraphElementNode["layer"] = "code"): GraphElementNode => ({
  id,
  label: `label-${id}`,
  kind: "function",
  layer,
});
const e = (source: string, target: string, edge_type = "calls"): GraphElementEdge => ({
  source,
  target,
  edge_type,
});

const CENTER = [100, 100] as const;

describe("buildSeries", () => {
  it("fills each node with its layer color and never swaps the fill on selection", () => {
    const set = loadedFrom([n("a", "doc")], []);
    const series = buildSeries(set, NO_SELECTION, CENTER, 1, 0.5, 5);
    expect(series.data[0].itemStyle.color).toBe("#16a34a"); // doc green, not a selection fill
  });

  it("draws a red ring + centre-pins the locked selection, leaving its layer fill", () => {
    const set = loadedFrom([n("a", "code"), n("b")], [e("a", "b")]);
    const sel = { ...NO_SELECTION, lockedId: "a" };
    const series = buildSeries(set, sel, CENTER, 1, 0.5, 5);
    const locked = series.data.find((d) => d.id === "a")!;
    expect(locked.itemStyle.color).toBe("#2563eb"); // layer fill survives
    expect(locked.itemStyle.borderColor).toBe("#da291c"); // red ring
    expect(locked.fixed).toBe(true);
    expect(locked.x).toBe(CENTER[0]);
  });

  it("dims nodes outside the locked node's adjacency neighbourhood", () => {
    const set = loadedFrom([n("a"), n("b"), n("c")], [e("a", "b")]);
    const sel = { ...NO_SELECTION, lockedId: "a" };
    const series = buildSeries(set, sel, CENTER, 1, 0.5, 5);
    const neighbour = series.data.find((d) => d.id === "b")!;
    const stranger = series.data.find((d) => d.id === "c")!;
    expect(neighbour.itemStyle.opacity).toBeUndefined(); // adjacent → bright
    expect(stranger.itemStyle.opacity).toBe(0.2); // outside → dimmed
  });

  it("styles edges by type and keeps a forbidden edge heaviest/most opaque", () => {
    const set = loadedFrom([n("a"), n("b"), n("c")], [e("a", "b", "imports"), e("b", "c", "forbidden_dependency")]);
    const series = buildSeries(set, NO_SELECTION, CENTER, 1, 0.5, 5);
    const imports = series.links.find((l) => l.edge_type === "imports")!;
    const forbidden = series.links.find((l) => l.edge_type === "forbidden_dependency")!;
    expect(imports.lineStyle.type).toBe("dashed");
    expect(forbidden.lineStyle.color).toBe("#da291c");
    expect(forbidden.lineStyle.width).toBeGreaterThan(imports.lineStyle.width);
  });

  it("honors the focus-depth bound when building the visible series", () => {
    const set = loadedFrom([n("a"), n("b"), n("c")], [e("a", "b"), e("b", "c")]);
    const sel = { ...NO_SELECTION, focusId: "a", depth: 1 };
    const series = buildSeries(set, sel, CENTER, 1, 0.5, 5);
    expect(series.data.map((d) => d.id).sort()).toEqual(["a", "b"]); // c is 2 hops away
  });

  it("carries no explicit `zoom`, so a restyle merge never resets the roamed camera", () => {
    // Regression: a hardcoded `zoom: 1` made every restyle `setOption` snap a
    // gesture/button zoom back to home. The series must omit `zoom` entirely — the
    // camera survives a merge; a full replaceMerge re-opens at home on its own.
    const set = loadedFrom([n("a"), n("b")], [e("a", "b")]);
    const series = buildSeries(set, NO_SELECTION, CENTER, 1, 0.5, 5);
    expect("zoom" in series).toBe(false);
    expect(series.scaleLimit).toEqual({ min: 0.5, max: 5 }); // bounds still applied
  });
});

describe("label level-of-detail", () => {
  it("shows labels for a small set or once zoomed in, hides them for a large zoomed-out set", () => {
    expect(labelsVisible(10, 0.5)).toBe(true); // small set
    expect(labelsVisible(200, 1.5)).toBe(true); // zoomed in
    expect(labelsVisible(200, 0.5)).toBe(false); // large + zoomed out
  });
});
