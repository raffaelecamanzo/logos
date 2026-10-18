/*
 * Shared analytics cell renderers (S-188, FR-UI-11) — the small presentational
 * pieces the Files & Risk and Coverage tables reuse so the `n/a` rule and numeric
 * alignment are identical across the views. Honest by construction: a missing
 * figure is a muted `n/a`, never a fabricated `0` (NFR-RA-05); a stale coverage
 * cell is a label (a Badge), never a shifted number ([FR-CV-05]).
 */

import { Badge } from "../../components/index.ts";
import type { CoverageCell, PathFindings } from "../../api/types.ts";
import { severityTone } from "../security/securityModel.ts";
import { NA, securityHref } from "./analyticsModel.ts";

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

/** One Files & Risk findings cell (FR-SC-14), linking to the Security page
 *  filtered to the file: the count with its highest severity, `0` for a scanned
 *  file with none, and any stale findings labelled apart — they never join the
 *  count (the file changed since the scan, so they may no longer hold). */
export function FindingsCellView({ cell, path }: { cell: PathFindings; path: string }) {
  return (
    <a href={securityHref(path)}>
      {cell.count > 0 && cell.highest ? (
        <>
          <span className="mono">{cell.count}</span> <Badge tone={severityTone(cell.highest)}>{cell.highest}</Badge>
        </>
      ) : (
        <span className="muted">0</span>
      )}
      {cell.stale > 0 && (
        <>
          {" "}
          <Badge tone="orange">{cell.stale} stale</Badge>
        </>
      )}
    </a>
  );
}
