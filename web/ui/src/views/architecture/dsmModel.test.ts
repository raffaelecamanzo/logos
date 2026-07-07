import { describe, expect, it } from "vitest";

import type { DsmReport } from "../../api/types.ts";
import { backEdges, cellCount, heatBucket, offDiagonalMax, rowName } from "./dsmModel.ts";

/** A 3-module report with one back-edge (row 0 → col 2, above the diagonal). */
function report(): DsmReport {
  return {
    granularity: "module",
    rows: [{ name: "api", layer: null }, { name: "core", layer: null }, { name: "db", layer: null }],
    // 0→2 (5) is ABOVE the diagonal (i<j) = the sole back-edge; 1→0 (2) is a
    // forward dependency (below the diagonal, i>j), which is NOT a cycle.
    matrix: [
      [0, 0, 5],
      [2, 0, 0],
      [0, 0, 0],
    ],
    freshness: "fresh",
    warnings: [],
  };
}

describe("dsmModel projections (FR-UI-06, S-189)", () => {
  it("flags only above-diagonal non-zero cells as back-edges, in scan order", () => {
    expect(backEdges(report())).toEqual([[0, 2]]);
  });

  it("a fully lower-triangular matrix has no back-edges", () => {
    const r = report();
    r.matrix = [
      [0, 0, 0],
      [4, 0, 0],
      [1, 2, 0],
    ];
    expect(backEdges(r)).toEqual([]);
  });

  it("offDiagonalMax ignores the diagonal", () => {
    // Diagonal 9s must not become the scale; the max off-diagonal here is 5.
    const r = report();
    r.matrix = [
      [9, 3, 5],
      [2, 9, 0],
      [0, 0, 9],
    ];
    expect(offDiagonalMax(r)).toBe(5);
  });

  it("heatBucket clamps to 1..4 and collapses an empty scale to 1", () => {
    expect(heatBucket(1, 0)).toBe(1); // max 0 ⇒ bucket 1
    expect(heatBucket(1, 8)).toBe(1);
    expect(heatBucket(8, 8)).toBe(4);
    expect(heatBucket(5, 8)).toBe(3);
  });

  it("cellCount and rowName degrade out-of-range access honestly", () => {
    const r = report();
    expect(cellCount(r, 0, 2)).toBe(5);
    expect(cellCount(r, 9, 9)).toBe(0);
    expect(rowName(r, 1)).toBe("core");
    expect(rowName(r, 9)).toBe("?");
  });
});
