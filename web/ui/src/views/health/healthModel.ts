/*
 * Pure Health model (S-187, FR-UI-04) — the presentation logic ported from the
 * server-rendered Health view (web/src/views/health.rs) into framework-free,
 * unit-testable functions: the canonical metric-row projection (with the ADR-21
 * applicability drop-outs kept as `null`, never a fabricated zero), the five CR-005
 * structural drill-down dimensions joined to their worst offenders, and the
 * evolution-row formatting (signed deltas, abbreviated sha, empty-graph `n/a`). No
 * DOM, no React — every figure is a projection of a read-model field (NFR-RA-05).
 */

import type { MetricSnapshot, MetricValue, Offender, ScanResult } from "../../api/types.ts";

/** One row of the quality-signal grid: a metric name and its value, or `null` for
 *  an applicability drop-out (Cohesion/Focus with no applicable construct). */
export interface MetricRow {
  name: string;
  /** `null` renders a muted `n/a`, never a zero (ADR-21, NFR-CC-04). */
  value: MetricValue | null;
}

/**
 * The ten quality metrics in canonical order (matching the grid + the Dashboard
 * roll-up). `cohesion`/`focus` are `Option` drop-outs carried through as `null`.
 */
export function metricRows(m: MetricSnapshot): MetricRow[] {
  return [
    { name: "Modularity", value: m.modularity },
    { name: "Acyclicity", value: m.acyclicity },
    { name: "Depth", value: m.depth },
    { name: "Equality", value: m.equality },
    { name: "Redundancy", value: m.redundancy },
    { name: "Nesting", value: m.nesting },
    { name: "Conciseness", value: m.conciseness },
    { name: "Cohesion", value: m.cohesion },
    { name: "Focus", value: m.focus },
    { name: "Uniqueness", value: m.uniqueness },
  ];
}

/** One structural dimension's drill-down source, projected from the scan. */
export interface MetricDetail {
  name: string;
  definition: string;
  /** `null` for an applicability drop-out — rendered muted, never a zero/table. */
  value: MetricValue | null;
  offenders: Offender[];
}

/**
 * The five CR-005 structural dimensions (FR-QM-09..FR-QM-13) joined to their worst
 * offenders — the only dimensions carrying per-symbol offenders. Canonical order,
 * matching the metric grid.
 */
export function structuralDetails(scan: ScanResult): MetricDetail[] {
  const m = scan.metrics;
  const w = scan.worst_offenders;
  return [
    { name: "Nesting", definition: "1 − deep-nesting ratio (FR-QM-09)", value: m.nesting, offenders: w.nesting },
    { name: "Conciseness", definition: "1 − brain-method ratio (FR-QM-10)", value: m.conciseness, offenders: w.conciseness },
    { name: "Cohesion", definition: "mean 1/LCOM4 over classes (FR-QM-11)", value: m.cohesion, offenders: w.cohesion },
    { name: "Focus", definition: "1 − god-container ratio (FR-QM-12)", value: m.focus, offenders: w.focus },
    { name: "Uniqueness", definition: "1 − near-clone ratio (FR-QM-13)", value: m.uniqueness, offenders: w.uniqueness },
  ];
}

/** The aggregate quality signal: the scan signal, else the snapshot aggregate,
 *  else `null` (an empty graph — rendered as a muted `n/a`, never a zero). */
export function aggregateSignal(scan: ScanResult): number | null {
  return scan.signal ?? scan.metrics.aggregate_signal;
}

/** Render an optional signal as a figure or the empty-graph `n/a` sentinel. */
export function optSignal(value: number | null): string {
  return value === null ? "n/a" : String(value);
}

/** Render a signed signal delta, or `—` for the first point / an n/a edge. */
export function optDelta(value: number | null): string {
  if (value === null) return "—";
  return value > 0 ? `+${value}` : String(value);
}

/** First 9 chars of a commit sha (abbreviated; the full sha is in the read-model);
 *  `—` for a snapshot recorded without a commit. */
export function shortSha(sha: string | null): string {
  return sha === null ? "—" : [...sha].slice(0, 9).join("");
}
