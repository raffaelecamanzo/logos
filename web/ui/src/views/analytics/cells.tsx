/*
 * Shared analytics cell renderers (S-188, FR-UI-11) — the small presentational
 * pieces the Files & Risk and Coverage tables reuse so the `n/a` rule and numeric
 * alignment are identical across the views. Honest by construction: a missing
 * figure is a muted `n/a`, never a fabricated `0` (NFR-RA-05); a stale coverage
 * cell is a label (a Badge), never a shifted number ([FR-CV-05]).
 */

import { Badge } from "../../components/index.ts";
import type { CoverageCell } from "../../api/types.ts";
import { NA } from "./analyticsModel.ts";

/** A muted `n/a` for an absent figure (right-aligns inside a numeric column). */
export function Na() {
  return <span className="muted">{NA}</span>;
}

/** One hotspot/coverage cell: a fresh percent, a `STALE` badge, or muted `n/a`
 *  — never a fabricated zero (mirrors the Rust `coverage_cell`, [FR-CV-05]). */
export function CoverageCellView({ cell, pct }: { cell: CoverageCell; pct: string | null }) {
  if (cell.state === "fresh" && cell.coverage_bp != null && pct != null) {
    return <span className="mono">{pct}</span>;
  }
  if (cell.state === "stale") return <Badge tone="red">STALE</Badge>;
  return <Na />;
}
