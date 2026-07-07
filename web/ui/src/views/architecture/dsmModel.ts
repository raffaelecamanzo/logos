/*
 * Pure DSM (dependency-structure matrix) projections (S-189, FR-UI-06) — ported
 * from the server-rendered `web/src/views/dsm.rs` so they are unit-testable with no
 * DOM. Every function is a **projection of the `DsmReport` read-model**, never a
 * recomputed figure (FR-UI-03): a back-edge is a cell above the diagonal with a
 * non-zero count (a dependency running against layer order — a cycle participant),
 * the heat scale is bucketed against the off-diagonal max, exactly as the legacy
 * view computed them.
 */

import type { DsmReport } from "../../api/types.ts";

/** Module count above which the full matrix is unreadable (frontend-design §4.5),
 *  so its disclosure stays collapsed by default. At or below it the disclosure
 *  auto-opens. */
export const MATRIX_MODULE_THRESHOLD = 20;

/** A back-edge cell — the `[row, col]` index pair (`row` depends on `col`). */
export type BackEdge = readonly [number, number];

/**
 * The `(i, j)` cells above the diagonal with a non-zero count — back-edges
 * (dependencies running against layer order, cycle participants). A pure
 * projection of the matrix in row-major scan order (the table's default order),
 * never a recomputed figure.
 */
export function backEdges(report: DsmReport): BackEdge[] {
  const out: BackEdge[] = [];
  report.matrix.forEach((row, i) => {
    row.forEach((count, j) => {
      if (i < j && count > 0) out.push([i, j]);
    });
  });
  return out;
}

/** The dependency count at `matrix[i][j]`, or 0 when out of range (a ragged or
 *  short row degrades to an empty cell rather than throwing). */
export function cellCount(report: DsmReport, i: number, j: number): number {
  return report.matrix[i]?.[j] ?? 0;
}

/** The largest off-diagonal dependency count — the heat scale's denominator. */
export function offDiagonalMax(report: DsmReport): number {
  let max = 0;
  report.matrix.forEach((row, i) => {
    row.forEach((count, j) => {
      if (i !== j) max = Math.max(max, count);
    });
  });
  return max;
}

/** Heat bucket `1..=4` for a non-zero `count` against the off-diagonal `max`
 *  (matches the legacy `bucket`): an all-zero off-diagonal collapses to bucket 1. */
export function heatBucket(count: number, max: number): 1 | 2 | 3 | 4 {
  if (max === 0) return 1;
  const scaled = Math.ceil((count / max) * 4);
  return Math.min(4, Math.max(1, scaled)) as 1 | 2 | 3 | 4;
}

/** The module name at `index`, or `"?"` for an out-of-range index (the
 *  unresolved-row placeholder the legacy view used). */
export function rowName(report: DsmReport, index: number): string {
  return report.rows[index]?.name ?? "?";
}
