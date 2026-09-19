/*
 * Pure Health model (S-187, FR-UI-04) — the presentation logic ported from the
 * server-rendered Health view (web/src/views/health.rs) into framework-free,
 * unit-testable functions: the canonical metric-row projection (with the ADR-21
 * applicability drop-outs kept as `null`, never a fabricated zero), the five CR-005
 * structural drill-down dimensions joined to their worst offenders, and the
 * evolution-row formatting (signed deltas, abbreviated sha, empty-graph `n/a`). No
 * DOM, no React — every figure is a projection of a read-model field (NFR-RA-05).
 */

import type {
  EvolutionReport,
  MetricSnapshot,
  MetricValue,
  Offender,
  ScanResult,
  StatusInfo,
} from "../../api/types.ts";

/**
 * Which absence a null gate signal / empty metric grid actually is.
 *
 * `gate.signal === null` and `scan.metrics.empty` are both produced by the metric
 * snapshot, and a snapshot is absent — or present but unscorable — for three
 * genuinely different reasons. A readout must name only the one its own condition
 * establishes ([FR-EH-04], CR-130), so the discriminant is derived once here rather
 * than re-guessed per card: the gate band and the quality grid gate on *different*
 * fields and must not disagree about the cause.
 *
 * - `unindexed` — the graph holds no file and no node. `logos index` is the step.
 * - `unscanned` — indexed, but `metric_snapshots` is empty, so no `scan` has ever
 *   recorded one. `logos scan` is the step.
 * - `no-production-scope` — indexed AND scanned, but the snapshot scored nothing:
 *   the production metric graph is empty because every indexed symbol is test
 *   scope or otherwise excluded ([FR-QM-08]). **No command changes this**, so a
 *   readout in this state names none.
 *
 * [FR-EH-04]: ../../../../docs/specs/requirements/FR-EH-04.md
 * [FR-QM-08]: ../../../../docs/specs/requirements/FR-QM-08.md
 */
export type SignalAbsence = "unindexed" | "unscanned" | "no-production-scope";

/**
 * Classify the absence behind a null signal or an empty metric grid.
 *
 * `status.indexed` is a whole-graph fact (`files > 0 || nodes > 0`, test nodes
 * included); the metric snapshot's emptiness is a *production*-subgraph fact. The
 * two are different questions, so `indexed` alone cannot tell `unscanned` from
 * `no-production-scope` — `evolution.snapshots` supplies the missing bit, being
 * non-empty exactly when a `metric_snapshots` row exists.
 */
export function signalAbsence(status: StatusInfo, evolution: EvolutionReport): SignalAbsence {
  if (!status.indexed) return "unindexed";
  if (evolution.snapshots.length === 0) return "unscanned";
  return "no-production-scope";
}

/**
 * A populated signal that describes a graph which is **no longer indexed**.
 *
 * The staleness twin of [`SignalAbsence`](#signalabsence), and deliberately a
 * *separate* question from it ([CR-135] §3.3): `signalAbsence` classifies why
 * there is **no** signal and is untouched here; this classifies a signal that
 * exists but that `status.indexed` says describes a graph that has since been
 * de-indexed. Conflating the two is the exact error [S-406]'s review caught —
 * `status.indexed` is a whole-graph fact, and it answers staleness, never cause.
 *
 * The figures are **labelled, not suppressed**: they are genuine history, and
 * hiding them would discard real information while adding a fourth cause to a
 * three-way absence classification just settled under review ([CR-135] §10).
 *
 * [CR-135]: ../../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
 */
export interface StaleSnapshot {
  /** The snapshot's UTC calendar date, `YYYY-MM-DD`; `null` when the payload
   *  carries no dated point to take it from — an undated label, never a
   *  fabricated date (NFR-CC-04). */
  date: string | null;
}

/**
 * Classify a **populated** signal as describing a graph that is no longer
 * indexed, or `null` for the ordinary case.
 *
 * `null` means "render exactly as before": every caller branches on it, so the
 * staleness wording cannot reach an indexed project. Derived once for the whole
 * page, like `signalAbsence`, so the gate band and the quality grid cannot
 * disagree about whether what they show is current.
 *
 * The date comes from the evolution series' **last** point. That is the same
 * `metric_snapshots` row the gate verdict and the scan result are projected from
 * — `governance::evolution` emits the series `ORDER BY id` and keeps its tail,
 * and `latest_metric_snapshot` reads `ORDER BY id DESC LIMIT 1`. It is read
 * separately from the snapshot (`web/src/api_v1.rs`), so it dates the label
 * rather than feeding the verdict: a `scan` landing between the two reads could
 * only move the date, never the figures.
 */
export function snapshotStaleness(status: StatusInfo, evolution: EvolutionReport): StaleSnapshot | null {
  if (status.indexed) return null;
  return { date: snapshotDate(evolution) };
}

/** The last recorded snapshot's UTC calendar date, or `null` when the series is
 *  empty or the stored instant is not a representable calendar date.
 *  Module-private: the one seam onto this fact is `snapshotStaleness`.
 *
 *  Both guards are load-bearing, and the second is not the first. `getTime()`
 *  is `NaN` only outside the ±8.64e15 ms range; *inside* it, a year outside
 *  0000–9999 makes `toISOString()` emit ECMAScript's **expanded** form
 *  (`±YYYYYY-MM-DD…`), and a `slice(0, 10)` written for the ordinary 10-char
 *  prefix then returns a garbled fragment — `"+010000-01"` — rather than a
 *  date. That is a fabricated figure, which is exactly what this seam promises
 *  never to produce ([NFR-CC-04]), so the year is checked before the slice. */
function snapshotDate(evolution: EvolutionReport): string | null {
  const last = evolution.snapshots[evolution.snapshots.length - 1];
  if (last === undefined) return null;
  const at = new Date(last.created_at * 1000);
  if (Number.isNaN(at.getTime())) return null;
  const year = at.getUTCFullYear();
  if (year < 0 || year > 9999) return null;
  return at.toISOString().slice(0, 10);
}

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
 *  else `null` (nothing recorded to report — rendered as a muted `n/a`, never a
 *  zero, and never described as an empty graph: this cannot tell the two apart,
 *  FR-EH-04). */
export function aggregateSignal(scan: ScanResult): number | null {
  return scan.signal ?? scan.metrics.aggregate_signal;
}

/** Render an optional signal as a figure or the honest `n/a` sentinel. */
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
