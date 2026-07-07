/*
 * Pure analytics model (S-188, FR-UI-11, FR-UI-17) — the deterministic logic the
 * Files & Risk, Coverage, and Quadrant views render, ported from the server-side
 * Rust (web/src/analytics.rs, web/src/views/quadrant.rs, web/assets/quadrant.js)
 * into pure, DOM-free TypeScript so it is unit-testable without a canvas. The
 * imperative uPlot mount (`uplot.ts`/`QuadrantChart.tsx`) and the React views
 * consume these; nothing here fabricates a figure — an absent value is `n/a`,
 * never a `0` (NFR-RA-05). Read-only by construction (ADR-28).
 */

import type {
  CoverageCrossReport,
  CrossSymbol,
  FileTemporal,
  Hotspot,
  HotspotReport,
  QuadrantTag,
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

// ── Quadrant severity / weight / trust (mirrors web/src/views/quadrant.rs) ─────

/**
 * One quadrant's **urgency** rank for table ordering ([FR-UI-17]). The surprising
 * disagreements lead: Q1 false-green (4) > Q2 dead edge (3) > Q3 true gap (2) > Q4
 * trust (1); a symbol with no runtime axis has no disagreement to rank (0). The
 * inverse of the Gartner desirability order — the worst cell leads the table.
 */
export function severity(quadrant: QuadrantTag | null): number {
  switch (quadrant) {
    case "q1":
      return 4;
    case "q2":
      return 3;
    case "q3":
      return 2;
    case "q4":
      return 1;
    default:
      return 0;
  }
}

/** The per-file architectural weight map (hotspot score by path, [FR-GH-06]); an
 *  absent/degraded report yields an empty map. */
export function fileWeights(hotspots: HotspotReport | null | undefined): Map<string, number> {
  const weights = new Map<string, number>();
  if (hotspots) {
    for (const file of hotspots.files) weights.set(file.path, file.score);
  }
  return weights;
}

/** A symbol's architectural weight: its file's hotspot score, floored at `1` so a
 *  symbol in an unranked file still counts (never weighted to zero). */
export function weightOf(weights: Map<string, number>, file: string): number {
  return Math.max(1, weights.get(file) ?? 0);
}

/**
 * The architecturally-weighted **Q4 trust share** in basis points (0–10000) over
 * the *placed* symbols — the headline trust score ([FR-UI-17]). A symbol with no
 * runtime axis cannot be placed and is excluded from the denominator. `null` when
 * no symbol is placed (no fresh coverage) — the honest empty state, never `0%`.
 */
export function trustScoreBp(symbols: CrossSymbol[], weights: Map<string, number>): number | null {
  let denominator = 0;
  let trust = 0;
  for (const symbol of symbols) {
    if (symbol.quadrant) {
      const weight = weightOf(weights, symbol.file);
      denominator += weight;
      if (symbol.quadrant === "q4") trust += weight;
    }
  }
  if (denominator <= 0) return null;
  // Integer basis points, matching the Rust `saturating_mul(10_000) / denominator`.
  return Math.trunc((trust * 10_000) / denominator);
}

// ── Scatter payload (mirrors web/src/views/quadrant.rs `scatter_points` +
//    the plot placement in web/assets/quadrant.js) ──────────────────────────────

/** One scatter point for the client-side uPlot render ([FR-UI-17]). */
export interface ScatterPoint {
  /** Reachability: 1.0 reachable (right column), 0.0 not (left). */
  x: number;
  /** Runtime-executed line fraction, 0.0–1.0 (the Y axis, bottom → top). */
  y: number;
  /** Architectural weight (hotspot score, floored at 1) — drives point size. */
  w: number;
  /** Quadrant tag — drives point color. */
  q: QuadrantTag;
  /** Symbol name (tooltip). */
  name: string;
  /** `file:line` location (tooltip). */
  loc: string;
}

/**
 * The scatter payload: one point per **placed** symbol (a symbol with no runtime
 * axis cannot be plotted — the `n/a` rule). Built from the same cross read-model +
 * weights the trust score uses, so the surfaces agree byte-for-byte.
 */
export function scatterPoints(
  report: CoverageCrossReport,
  weights: Map<string, number>,
): ScatterPoint[] {
  const out: ScatterPoint[] = [];
  for (const s of report.symbols) {
    if (s.runtime_exec_bp == null || s.quadrant == null) continue; // n/a — not placed
    out.push({
      x: s.reachable_from_test ? 1.0 : 0.0,
      y: s.runtime_exec_bp / 10_000.0,
      w: weightOf(weights, s.file),
      q: s.quadrant,
      name: s.name,
      loc: fileLine(s.file, s.start_line) ?? s.file,
    });
  }
  return out;
}

/** Point radius scales with architectural weight (blast radius, [FR-GH-06]): a
 *  sqrt scale, bounded so a point is always selectable and never swamps a
 *  neighbour. Ported from `radiusFor` in quadrant.js. */
export function radiusFor(weight: number): number {
  const w = weight > 0 ? weight : 1;
  const r = 3 + Math.sqrt(w);
  return Math.max(3, Math.min(r, 14));
}

/** A small deterministic spread in [-0.5, 0.5) from a point's index, so symbols
 *  sharing a cell separate without `Math.random` (the table twin stays the
 *  deterministic surface). Knuth multiplicative hash in 32-bit. */
export function jitter(i: number): number {
  const h = ((i * 2654435761) >>> 0) % 1000;
  return h / 1000 - 0.5;
}

/** Plotted X: reachability drives the column (left unreachable / right reachable),
 *  jittered within the column half. Ported from `plotX` in quadrant.js. */
export function plotX(p: ScatterPoint, i: number): number {
  const center = p.x >= 0.5 ? 0.75 : 0.25;
  return center + jitter(i) * 0.36;
}

/** Plotted Y: executed symbols track the runtime fraction in the top band;
 *  unexecuted (a measured 0%) sit in the bottom band, jittered. Ported from
 *  `plotY` in quadrant.js. */
export function plotY(p: ScatterPoint, i: number): number {
  if (p.y > 0) return 0.55 + p.y * 0.4; // executed: 0.55 → 0.95
  return 0.27 + jitter(i) * 0.34; // unexecuted band: ~0.10 → 0.44
}

// ── Urgency ranking (mirrors the `urgency_card` ordering) ──────────────────────

/** One symbol paired with its computed urgency, for the accessible table. */
export interface UrgencyRow {
  symbol: CrossSymbol;
  /** Architectural weight × quadrant severity — most dangerous leads. */
  urgency: number;
}

/**
 * Pair each symbol with its urgency and order urgency-descending — the most
 * dangerous disagreements lead ([FR-UI-17]). The cross read-model is already in
 * canonical order, and JS `Array.sort` is stable, so that order is the
 * within-urgency tiebreak (deterministic, mirrors the Rust stable sort).
 */
export function rankByUrgency(
  report: CoverageCrossReport,
  weights: Map<string, number>,
): UrgencyRow[] {
  return report.symbols
    .map((symbol) => ({
      symbol,
      urgency: weightOf(weights, symbol.file) * severity(symbol.quadrant),
    }))
    .sort((a, b) => b.urgency - a.urgency);
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
