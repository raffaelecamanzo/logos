/*
 * Pure analytics model (S-188, FR-UI-11) — the deterministic logic the Files & Risk
 * and Coverage views render, ported from the server-side Rust (web/src/analytics.rs)
 * into pure, DOM-free TypeScript so it is unit-testable without a canvas. The React
 * views consume these; nothing here fabricates a figure — an absent value is `n/a`,
 * never a `0` (NFR-RA-05). Read-only by construction (ADR-28). CR-079 removed the
 * reachability×coverage-cross helpers along with the retired scatter view.
 */

import type {
  FileTemporal,
  Hotspot,
  HotspotReport,
  TemporalReport,
} from "../../api/types.ts";

/** The muted `n/a` token — the single honest placeholder for a missing figure. */
export const NA = "n/a";

/**
 * Render a basis-point figure (0–10000) as a one-decimal percent — the exact
 * reprojection the Rust `pct_bp` does (`{:.1}%` of clamped bp/100), so the SPA and
 * the server-rendered view agree byte-for-byte. Clamped so a stray out-of-range
 * value can never overflow a bar.
 */
export function pctBp(bp: number): string {
  const clamped = Math.min(10_000, Math.max(0, bp));
  return `${(clamped / 100).toFixed(1)}%`;
}

/**
 * `file:line`, `file`, or `null` (an unbound symbol) — never a fabricated `:0`
 * (mirrors the Rust `file_line`, NFR-RA-05).
 */
export function fileLine(file: string | null, line: number | null): string | null {
  if (file && line != null) return `${file}:${line}`;
  if (file) return file;
  return null;
}

// ── Files & Risk merged-row join (mirrors web/src/analytics.rs `render_files`) ─

/** One merged Files & Risk row: the hotspot board spine joined with the per-file
 *  temporal facts by path. An absent temporal join renders `n/a` (`null` here),
 *  never a fabricated `0`. */
export interface FileRiskRow {
  path: string;
  /** Churn — in-window commit count (the hotspot's own figure). */
  commits: number;
  /** `lines_added / lines_deleted`, or `null` when temporal is absent. */
  churn: { added: number; deleted: number } | null;
  /** Code-age in days, or `null` when temporal is absent. */
  ageDays: number | null;
  coChange: number;
  /** Defect-history heuristic count. */
  defect: number;
  complexity: number;
  /** The per-file coverage cell. */
  coverage: Hotspot["coverage"];
}

/**
 * Join the ranked hotspot board (the spine, so the default order is the composite
 * hotspot score) with the temporal report by path. A ranked file absent from the
 * temporal set renders `n/a` for churn/age — never a zero (NFR-CC-04).
 */
export function fileRiskRows(report: HotspotReport, temporal: TemporalReport): FileRiskRow[] {
  const byPath = new Map<string, FileTemporal>();
  for (const f of temporal.files) byPath.set(f.path, f);
  return report.files.map((h) => {
    const ft = byPath.get(h.path);
    return {
      path: h.path,
      commits: h.churn_commits,
      churn: ft ? { added: ft.lines_added, deleted: ft.lines_deleted } : null,
      ageDays: ft ? ft.last_change_age_days : null,
      coChange: h.co_change_count,
      defect: h.defect_commits,
      complexity: h.complexity,
      coverage: h.coverage,
    };
  });
}

/** Files with a meaningful ownership/entropy signal (both meaningful only with
 *  multiple committers — a single-author history is an honest empty state). */
export function ownershipRows(temporal: TemporalReport): FileTemporal[] {
  return temporal.files.filter(
    (f) => f.ownership_dispersion_bp > 0 || f.change_entropy_bp > 0,
  );
}

/** The honest empty-state message + producing command for an empty hotspot board,
 *  preferring the read-model's own degraded/first-mine notice. */
export function hotspotsEmpty(report: HotspotReport): { message: string; command: string } {
  return {
    message: report.notice ?? "No hotspots ranked yet — run",
    command: "logos hotspots",
  };
}
