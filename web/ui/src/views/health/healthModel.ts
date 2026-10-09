/*
 * Pure Health model (S-187, FR-UI-04) — the presentation logic ported from the
 * server-rendered Health view (web/src/views/health.rs) into framework-free,
 * unit-testable functions: the canonical metric-row projection (with the ADR-21
 * applicability drop-outs kept as `null`, never a fabricated zero), the ten
 * dimension widgets joined to their worst offenders where the payload lists them
 * (S-615, FR-UI-43), the gate's pass floor and informational-pass reading, and
 * the evolution-row formatting (signed deltas, abbreviated sha, empty-graph
 * `n/a`). No DOM, no React — every figure is a projection of a read-model field (NFR-RA-05).
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import type {
  EvolutionReport,
  GateResult,
  MetricSnapshot,
  MetricValue,
  Offender,
  ScanResult,
  StatusInfo,
  WorstOffenders,
} from "../../api/types.ts";
import { UNKNOWN_AGE_AHEAD_OF_NOW, parseSecs } from "../dashboard/dashboardModel.ts";

/**
 * Which absence a null gate signal / empty metric grid actually is.
 *
 * `gate.signal === null` and `scan.metrics.empty` are both produced by the metric
 * snapshot, and a snapshot is absent — or present but unscorable — for three
 * genuinely different reasons. A readout must name only the one its own condition
 * establishes ([FR-EH-04], CR-130), so the discriminant is derived once here rather
 * than re-guessed per widget: the Gate and Quality signal widgets gate on
 * *different* fields and must not disagree about the cause.
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
 * Why a **populated** signal is not asserted to be a current reading.
 *
 * The staleness twin of [`SignalAbsence`](#signalabsence), and deliberately a
 * *separate* question from it ([CR-135] §3.3): `signalAbsence` classifies why
 * there is **no** signal and is untouched here; this classifies a signal that
 * exists but that the payload says does not describe the graph as it now
 * stands. Conflating the two is the exact error [S-406]'s review caught.
 *
 * The figures are **labelled, not suppressed**: they are genuine history, and
 * hiding them would discard real information while adding a fourth cause to a
 * three-way absence classification just settled under review ([CR-135] §10).
 * Every arm below renders the *same* structure — chip, figures, note — so no
 * fourth Health state is added either ([CR-135] §7): only the chip and the sentence
 * differ.
 *
 * - `de-indexed` — `status.indexed` is false: the graph these figures describe
 *   is gone (S-422, [CR-135] §3.2). **Unchanged by S-436**, and evaluated
 *   first, so a de-indexed graph still reports de-indexed rather than the
 *   wording below.
 * - `moved-past` — the graph was indexed or synced **after** the snapshot was
 *   recorded. See {@link snapshotStaleness} for what that does and does not
 *   establish.
 * - `indeterminate` — the comparison cannot be made at all. Neither `current`
 *   nor a date is rendered; `detail` says which fact is missing.
 *
 * Every arm carries `date`, and the indeterminate arm's is `null` **by type**:
 * "renders no date" is then a fact the compiler holds rather than a rule the
 * renderer is trusted to follow.
 *
 * [CR-135]: ../../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
 */
export type SnapshotCurrency = DeIndexed | MovedPast | Indeterminate;

/**
 * `status.indexed` is false: the graph these figures describe is gone (S-422,
 * [CR-135] §3.2).
 *
 * **Unchanged by S-436 down to the value** — it carries a date and nothing
 * else, which is exactly what S-422's tests pin with `toEqual({ date })`. That
 * is deliberate: it makes this story's claim to have left the de-index branch
 * alone a fact the existing suite checks rather than one the notes assert. The
 * absent `cause` is therefore this arm's discriminant; adding a tag would have
 * meant editing the very tests that guard the claim.
 */
export interface DeIndexed {
  readonly date: string | null;
  readonly cause?: undefined;
  readonly detail?: undefined;
}

/** An index or sync ran **after** the snapshot was recorded (S-436). Always
 *  dated: an undated snapshot cannot be compared and is `Indeterminate`. */
export interface MovedPast {
  readonly date: string;
  readonly cause: "moved-past";
  /** Never set: only the indeterminate arm has a fact to report as missing.
   *  Declared so `cause` and `detail` both read across the union without a
   *  narrowing dance at every call site. */
  readonly detail?: undefined;
}

/** The comparison cannot be made at all (S-436). `date` is `null` by type, so
 *  no date can reach the renderer; `detail` names the fact that is missing. */
export interface Indeterminate {
  readonly date: null;
  readonly cause: "indeterminate";
  readonly detail: string;
}

/** The indeterminate arm's three sentences — the missing fact, never a guess at
 *  what it would have been (NFR-CC-04).
 *
 *  The clock case borrows [S-433]'s shipped phrase verbatim by importing it
 *  rather than restating it: `last_sync_at` is a file **mtime**, a stamp ahead
 *  of now is routine on a copied tree or a restored backup, and that is the
 *  same condition S-433 already named for the Dashboard's freshness line. One
 *  product, one wording for one condition.
 *
 *  [S-433]: ../dashboard/dashboardModel.ts */
const INDETERMINATE = {
  clock: `the last index or sync is ${UNKNOWN_AGE_AHEAD_OF_NOW}`,
  noIndexTime: "no index or sync time is recorded",
  undatedSnapshot: "the last snapshot carries no date to compare against",
} as const;

/**
 * Classify a **populated** signal as not-current, or `null` for the ordinary
 * case.
 *
 * `null` means "render exactly as before": every caller branches on it, so none
 * of the wording below can reach a project whose snapshot the graph has not
 * moved past. Derived once for the whole page, like `signalAbsence`, so the
 * Gate, Quality signal and dimension widgets cannot disagree about whether what
 * they show is current.
 *
 * **The discriminant is the timestamp pair, not the counts.** Comparing
 * `MetricSnapshot.node_count` against `StatusInfo.node_count` was proposed in
 * Sprint 72's review and is **rejected**, recorded here so it is not
 * re-proposed: the snapshot's count is the **production-scoped** metric graph
 * with `is_test` vertices dropped ([FR-QM-08], pinned by
 * `logos-core/src/metrics/tests.rs`), while `StatusInfo.node_count` is the
 * **whole** graph. They differ on every project that has tests, so that
 * comparison fires everywhere and measures nothing.
 *
 * **It detects disagreement, never currency.** An index or sync that ran after
 * the snapshot may have changed nothing at all, and this reports `moved-past`
 * just the same — so it **over-reports**. That is the safe direction
 * ([NFR-RA-05] prefers it to false assurance) and it is why the rendered
 * wording claims only that the graph has moved past the snapshot, never that
 * the figures are proven stale. The test *"over-reports a re-index that changed
 * nothing — a KNOWN over-report, never a staleness proof"* pins the
 * over-report, so the label cannot later be read as precision it does not have.
 *
 * An **implausibly old** index stamp — [S-433]'s other degradation — is
 * deliberately *not* a fourth arm. It is not among the conditions this story
 * scopes, and `last_full_index_at` is written by Logos itself from the system
 * clock, so unlike the future-dated mtime it is not a shape real trees produce.
 *
 * The date comes from the evolution series' **last** point. That is the same
 * `metric_snapshots` row the gate verdict and the scan result are projected from
 * — `governance::evolution` emits the series `ORDER BY id` and keeps its tail,
 * and `latest_metric_snapshot` reads `ORDER BY id DESC LIMIT 1`. It is read
 * separately from the snapshot (`web/src/api_v1.rs`), so it dates the label
 * rather than feeding the verdict: a `scan` landing between the two reads could
 * only move the date, never the figures.
 *
 * `nowUnix` is defaulted rather than required so that the de-index arm — which
 * returns before any clock is consulted — keeps its two-argument call shape.
 * Callers that reach the arms below pass it, as `DashboardView` does for
 * `freshnessStatement`.
 *
 * [FR-QM-08]: ../../../../docs/specs/requirements/FR-QM-08.md
 * [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
 * [S-433]: ../dashboard/dashboardModel.ts
 */
export function snapshotStaleness(
  status: StatusInfo,
  evolution: EvolutionReport,
  nowUnix: number = Math.floor(Date.now() / 1000),
): SnapshotCurrency | null {
  // S-422's condition, unchanged and evaluated FIRST: the two conditions
  // compose, and a de-indexed graph reports de-indexed ([CR-135] §3.2).
  if (!status.indexed) return { date: snapshotDate(evolution) };
  const snapshot = lastSnapshot(evolution);
  if (snapshot === null) return { date: null, cause: "indeterminate", detail: INDETERMINATE.undatedSnapshot };
  const basis = indexBasis(status, nowUnix);
  if (basis.at === null) return { date: null, cause: "indeterminate", detail: basis.detail };
  if (basis.at > snapshot.secs) return { date: snapshot.date, cause: "moved-past" };
  return null;
}

type IndexBasis = { readonly at: number } | { readonly at: null; readonly detail: string };

/**
 * The instant to compare the snapshot against — the **later** of the last full
 * index and the last incremental sync — or the indeterminate sentence saying
 * why there is none.
 *
 * The later of the two, because either one running after the snapshot moves the
 * graph past it. A field present but not unix seconds is treated as absent,
 * which is `parseSecs`'s own contract and what `freshnessStatement` already
 * does with it — not a second reading of the same field.
 *
 * A stamp ahead of `nowUnix` makes the whole comparison indeterminate rather
 * than falling back to the other field: a clock that cannot be trusted for one
 * stamp cannot be trusted for its sibling either, and the alternative is to
 * pick whichever reading happens to be reassuring.
 */
function indexBasis(status: StatusInfo, nowUnix: number): IndexBasis {
  let latest: number | null = null;
  for (const field of [status.last_full_index_at, status.last_sync_at]) {
    const secs = parseSecs(field);
    if (secs === null) continue;
    if (secs > nowUnix) return { at: null, detail: INDETERMINATE.clock };
    if (latest === null || secs > latest) latest = secs;
  }
  return latest === null ? { at: null, detail: INDETERMINATE.noIndexTime } : { at: latest };
}

/** The last recorded snapshot's instant and its UTC calendar date, or `null`
 *  when the series is empty or the stored instant is not a representable
 *  calendar date. Module-private: the seams onto this fact are
 *  `snapshotStaleness` and `snapshotDate`.
 *
 *  Both guards are load-bearing, and the second is not the first. `getTime()`
 *  is `NaN` only outside the ±8.64e15 ms range; *inside* it, a year outside
 *  0000–9999 makes `toISOString()` emit ECMAScript's **expanded** form
 *  (`±YYYYYY-MM-DD…`), and a `slice(0, 10)` written for the ordinary 10-char
 *  prefix then returns a garbled fragment — `"+010000-01"` — rather than a
 *  date. That is a fabricated figure, which is exactly what this seam promises
 *  never to produce ([NFR-CC-04]), so the year is checked before the slice. */
function lastSnapshot(evolution: EvolutionReport): { secs: number; date: string } | null {
  const last = evolution.snapshots[evolution.snapshots.length - 1];
  if (last === undefined) return null;
  const at = new Date(last.created_at * 1000);
  if (Number.isNaN(at.getTime())) return null;
  const year = at.getUTCFullYear();
  if (year < 0 || year > 9999) return null;
  return { secs: last.created_at, date: at.toISOString().slice(0, 10) };
}

/** The last recorded snapshot's UTC calendar date, or `null` when there is no
 *  point to take one from — an undated label, never a fabricated date. One
 *  projection of {@link lastSnapshot}, so the de-index arm's date and the
 *  moved-past arm's date cannot be derived two different ways. */
function snapshotDate(evolution: EvolutionReport): string | null {
  return lastSnapshot(evolution)?.date ?? null;
}

/** The ten quality dimensions, by key. */
export type DimensionKey =
  | "modularity"
  | "acyclicity"
  | "depth"
  | "equality"
  | "redundancy"
  | "nesting"
  | "conciseness"
  | "cohesion"
  | "focus"
  | "uniqueness";

/** One row of the quality-signal grid: a metric name and its value, or `null` for
 *  an applicability drop-out (Cohesion/Focus with no applicable construct). */
export interface MetricRow {
  key: DimensionKey;
  name: string;
  /** `null` renders a muted `n/a`, never a zero (ADR-21, NFR-CC-04). */
  value: MetricValue | null;
  /** The server's reason when the dimension is computed but not applicable
   *  (CR-156: Modularity on too few edges) — rendered in place of a score, with
   *  the computed `value` still shown beside it; `null` when it applies. */
  notApplicable: string | null;
}

/**
 * The ten quality metrics in canonical order (matching the grid + the Dashboard
 * roll-up). `cohesion`/`focus` are `Option` drop-outs carried through as `null`.
 * The ONE list: the Quality signal table and the ten dimension widgets are both
 * enumerated from it (FR-UI-43), so neither can show a dimension the other lacks.
 */
export function metricRows(m: MetricSnapshot): MetricRow[] {
  const rows: Omit<MetricRow, "notApplicable">[] = [
    { key: "modularity", name: "Modularity", value: m.modularity },
    { key: "acyclicity", name: "Acyclicity", value: m.acyclicity },
    { key: "depth", name: "Depth", value: m.depth },
    { key: "equality", name: "Equality", value: m.equality },
    { key: "redundancy", name: "Redundancy", value: m.redundancy },
    { key: "nesting", name: "Nesting", value: m.nesting },
    { key: "conciseness", name: "Conciseness", value: m.conciseness },
    { key: "cohesion", name: "Cohesion", value: m.cohesion },
    { key: "focus", name: "Focus", value: m.focus },
    { key: "uniqueness", name: "Uniqueness", value: m.uniqueness },
  ];
  return rows.map((r) => ({
    ...r,
    notApplicable: r.key === "modularity" ? (m.modularity_not_applicable?.reason ?? null) : null,
  }));
}

/** Whether a row is in the geometric mean: it has a value and no drop-out reason. */
function isApplicable(row: MetricRow): row is MetricRow & { value: MetricValue } {
  return row.value !== null && row.notApplicable === null;
}

/** `k` in "the geometric mean of the k applicable dimensions" (FR-QM-14): the
 *  rows the aggregate is taken over — ADR-21 and CR-156 drop-outs excluded. */
export function applicableCount(rows: MetricRow[]): number {
  return rows.filter(isApplicable).length;
}

/**
 * The applicable dimension with the lowest normalized score — the one the Gate and
 * Quality signal widgets tell the reader to start with — or `null` when none
 * applies or the lowest already scores a full 1 (nothing to start with). A tie
 * keeps the first in canonical order, so the answer is stable.
 */
export function lowestDimension(rows: MetricRow[]): (MetricRow & { value: MetricValue }) | null {
  let lowest: (MetricRow & { value: MetricValue }) | null = null;
  for (const row of rows) {
    if (isApplicable(row) && (lowest === null || row.value.normalized < lowest.value.normalized)) lowest = row;
  }
  return lowest !== null && lowest.value.normalized < 1 ? lowest : null;
}

/**
 * The lowest signal that still passes the gate, `baseline − ε` (BR-10: the gate
 * fails iff `current < baseline − ε`), or `null` with no baseline to compare
 * against. ε is `GateResult.epsilon`, read from the payload, never restated.
 */
export function passFloor(gate: Pick<GateResult, "baseline_signal" | "epsilon">): number | null {
  return gate.baseline_signal === null ? null : gate.baseline_signal - gate.epsilon;
}

/**
 * Whether the gate passed WITHOUT comparing the signal to the baseline — an
 * informational pass (FR-GV-05, FR-GV-10). The read-only verdict Health gets
 * (`gate_from_snapshot`) passes informationally when there is no baseline, when
 * the baseline or the signal has no figure, and when the baseline was recorded
 * under other metric semantics or other `[metric_thresholds]` (an incomparable
 * anchor the persisting `gate` re-baselines on its next run). `GateResult` carries
 * no flag for this: the server's one marker is its message, every informational
 * arm of which says "informational pass" and no comparing arm does. A missing
 * baseline is informational whatever the message says.
 */
export function isInformationalPass(gate: Pick<GateResult, "passed" | "baseline_signal" | "message">): boolean {
  return gate.passed && (gate.baseline_signal === null || /\binformational pass\b/.test(gate.message));
}

/** A gate figure (signal, floor, ε) as text: an integer as-is, otherwise to two
 *  decimal places at most — ε is a float on the wire (≈1.0). */
export function gateFigure(n: number): string {
  return Number.isInteger(n) ? String(n) : String(Math.round(n * 100) / 100);
}

/**
 * What an offender-backed dimension widget says about its worst offenders
 * (S-499, CR-162):
 *  - `not-applicable` — the dimension dropped out (ADR-21); no offender concept;
 *  - `not-recorded`   — the snapshot never recorded offenders (FR-QM-15), so its
 *    empty lists mean nothing and are never shown as a clean result (NFR-CC-04);
 *  - `none-flagged`   — recorded, and nothing crossed a threshold;
 *  - `listed`         — recorded, with entries to tabulate in persisted order.
 */
export type OffenderState = "not-applicable" | "not-recorded" | "none-flagged" | "listed";

/**
 * Any dimension widget's offender state: an [`OffenderState`], or `unlisted` —
 * the payload carries no offender list for this dimension at all (Modularity,
 * FR-UI-43): a named absence, never `[]`.
 */
export type DimensionOffenderState = OffenderState | "unlisted";

/**
 * Decide an offender-backed dimension's offender state. `recorded` is read FIRST and is the only
 * thing that can make an empty list mean "none flagged": an absent or `false` flag
 * is "not recorded" whatever the list holds, and a list's length is consulted only
 * once the snapshot is known to have recorded it. Never infer the state from
 * `offenders.length` alone.
 */
export function offenderState(
  value: MetricValue | null,
  worst: Pick<WorstOffenders, "recorded" | "unrecorded">,
  key: OffenderListKey,
  offenders: Offender[],
): OffenderState {
  if (value === null) return "not-applicable";
  // A snapshot recorded before CR-209 holds no list for the four dimensions it
  // added; the server names each one it cannot vouch for (`unrecorded`).
  if (worst.recorded !== true || worst.unrecorded?.includes(key) === true) return "not-recorded";
  return offenders.length === 0 ? "none-flagged" : "listed";
}

/** The nine dimensions whose worst offenders the payload carries (`WorstOffenders`,
 *  `DIMENSIONS` in logos-core/src/models/quality.rs). Modularity scores the
 *  directory layout as a whole, so it has no list. */
const OFFENDER_LISTS = [
  "nesting",
  "conciseness",
  "cohesion",
  "focus",
  "uniqueness",
  "acyclicity",
  "depth",
  "equality",
  "redundancy",
] as const;
type OffenderListKey = (typeof OFFENDER_LISTS)[number];

function hasOffenderList(key: DimensionKey): key is OffenderListKey {
  return (OFFENDER_LISTS as readonly DimensionKey[]).includes(key);
}

/** One dimension widget's source, projected from the scan. */
export interface DimensionDetail extends MetricRow {
  /** The persisted worst offenders; `[]` for a dimension with no list. */
  offenders: Offender[];
  /** Which offender state this is (see [`DimensionOffenderState`]). */
  offenderState: DimensionOffenderState;
}

/**
 * All ten dimensions, in canonical order, each joined to its worst offenders where
 * the payload carries a list (FR-UI-43). Enumerated from `metricRows`, the list the
 * Quality signal table renders, so the widgets and the table cannot disagree about
 * which dimensions exist or in what order.
 */
export function dimensionDetails(scan: ScanResult): DimensionDetail[] {
  const w = scan.worst_offenders;
  return metricRows(scan.metrics).map((row) => {
    if (!hasOffenderList(row.key)) return { ...row, offenders: [], offenderState: "unlisted" };
    const offenders = w[row.key];
    return { ...row, offenders, offenderState: offenderState(row.value, w, row.key, offenders) };
  });
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
