/*
 * Pure analytics model (S-188, FR-UI-11) — the deterministic logic the Files & Risk
 * and Coverage views render, ported from the server-side Rust (web/src/analytics.rs)
 * into pure, DOM-free TypeScript so it is unit-testable without a canvas. The React
 * views consume these; nothing here fabricates a figure — an absent value is `n/a`,
 * never a `0` (NFR-RA-05). Read-only by construction (ADR-28). CR-079 removed the
 * reachability×coverage-cross helpers along with the retired scatter view.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import type {
  FileTemporal,
  Hotspot,
  HotspotReport,
  TemporalReport,
} from "../../api/types.ts";
import { severityRank } from "../security/securityModel.ts";

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
  /** The findings cell (FR-SC-14); `undefined` when nothing was ever scanned. */
  findings: Hotspot["findings"];
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
      findings: h.findings,
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

/** Whether the board carries the findings column: only once a scan is stored —
 *  no scan shows no column rather than a column of zeros (FR-SC-14). */
export function hasFindingsColumn(rows: readonly FileRiskRow[]): boolean {
  return rows.some((r) => r.findings !== undefined);
}

/** The Security page filtered to one file (its `?path=` deep link, S-670) and,
 *  when a code scope is named, to that scope (`?scope=`, S-796): production is
 *  the page's default and is not named. */
export function securityHref(path: string, scope?: "test" | "all"): string {
  const href = `/security?path=${encodeURIComponent(path)}`;
  return scope === undefined ? href : `${href}&scope=${scope}`;
}

/** The findings column's sort key: highest severity first, then most findings;
 *  a scanned file with none sorts below every file that has one. */
export function findingsSortValue(f: NonNullable<FileRiskRow["findings"]>): number {
  return f.highest ? severityRank(f.highest) * 100_000 + f.count : 0;
}
