/*
 * Shared analytics cell renderers (S-188, FR-UI-11) — the small presentational
 * pieces the Files & Risk, Coverage, and Quadrant tables reuse so the `n/a` rule
 * and numeric alignment are identical across the three views. Honest by
 * construction: a missing figure is a muted `n/a`, never a fabricated `0`
 * (NFR-RA-05); a stale coverage cell is a label (a Badge), never a shifted number
 * ([FR-CV-05]).
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

/** A quadrant badge carrying its label **and** colour (a11y: colour never alone).
 *  Q4 trust green, Q2 dead-edge orange, Q1 false-green red, Q3 gap muted; a symbol
 *  with no runtime axis renders a muted `n/a` (mirrors the Rust `quadrant_badge`,
 *  CR-040 Gartner numbering). */
export function QuadrantBadge({ quadrant }: { quadrant: import("../../api/types.ts").QuadrantTag | null }) {
  switch (quadrant) {
    case "q1":
      return <Badge tone="red">Q1</Badge>;
    case "q2":
      return <Badge tone="orange">Q2</Badge>;
    case "q3":
      return <Badge tone="muted">Q3</Badge>;
    case "q4":
      return <Badge tone="green">Q4</Badge>;
    default:
      return <Na />;
  }
}

/** The runtime-execution cell: a percent when the symbol carries a real fraction,
 *  muted `n/a` otherwise — never a fabricated zero (mirrors `runtime_cell`). */
export function RuntimeCell({ pct }: { pct: string | null }) {
  return pct != null ? <span className="mono">{pct}</span> : <Na />;
}
