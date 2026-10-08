/*
 * HealthView (S-187, FR-UI-04, FR-UI-21) — the Health tab migrated to React over
 * `/api/v1/health`, reusing the S-186 page-integration pattern (registered in
 * `views/index.ts` for `/health`, mounted by `App.tsx`, rendered exclusively
 * through the S-193 design system).
 *
 * It preserves the server-rendered Health view's verdict-first layout
 * (web/src/views/health.rs, frontend-design §4.2) and renders it through the
 * S-611 `Widget` frame in one `WidgetStack` (S-615, CR-203, FR-UI-43): the Gate
 * widget leads, then the Quality signal widget (scope line and thresholds
 * disclosure folded in), then one widget per quality dimension — all ten, in
 * canonical order — then the Signal trend (the non-gated pointer to Files & Risk
 * that sat between them is hidden through the hidden-widget register,
 * S-612/FR-UI-41). Every widget sentence and figure text is the
 * `copy/health.copy.ts` catalogue's; this file computes each widget's state. It
 * keeps the view's honest states (a gate and a quality signal with nothing to
 * show name the step that would produce it — `logos scan` on a populated graph,
 * `logos index` on an empty one, FR-EH-04/CR-130; a populated signal the graph no
 * longer matches is labelled rather than shown as a current verdict, in one
 * structure whose sentence names which of three facts establishes that — the graph was de-indexed, or it
 * was indexed/synced after the snapshot, or the comparison could not be made at
 * all, which is the one arm that carries no date, FR-EH-04/CR-135/S-436; an ADR-21
 * metric drop-out is a muted `n/a`, never a zero; no snapshots is an honest empty
 * state). Every read is GET-only — loading the
 * view mutates no store (ADR-28); sorting the tables is client-side over the full
 * dataset.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import type { ReactNode } from "react";

import { AsyncResource, fetchHealth, useApiResource } from "../../api/index.ts";
import type {
  EvolutionPoint,
  GateResult,
  HealthModel,
  Offender,
  ScanResult,
} from "../../api/types.ts";
import {
  Badge,
  type BadgeTone,
  Callout,
  CopyTextView,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  FigureNote,
  ScoreBar,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  DIMENSION_COPY,
  DIMENSION_NOT_CURRENT,
  NO_APPLICABLE_CONSTRUCT,
  NO_SNAPSHOTS,
  OFFENDER_STATEMENT,
  READING_ABSENCE,
  THRESHOLDS_DISCLOSURE,
  type DimensionState,
  gate as gateCopy,
  gateFigureText,
  GATE_NOT_COMPARED,
  offenderBadge,
  qualitySignal as qualitySignalCopy,
  scopeLine,
  signalFigure,
  signalTrend as signalTrendCopy,
  staleNote,
} from "../../copy/health.copy.ts";
import { isWidgetHidden } from "../hiddenWidgets.ts";
import {
  aggregateSignal,
  applicableCount,
  dimensionDetails,
  gateFigure,
  isInformationalPass,
  lowestDimension,
  metricRows,
  optDelta,
  optSignal,
  passFloor,
  shortSha,
  signalAbsence,
  snapshotStaleness,
  type DimensionDetail,
  type MetricRow,
  type SignalAbsence,
  type SnapshotCurrency,
} from "./healthModel.ts";
import styles from "./Health.module.css";

// The paginated Health tables (signal trend, worst offenders) page at the shared
// `DEFAULT_TABLE_PAGE_SIZE` (FR-UI-11). The quality-metric grid stays unpaginated
// (legacy `None`, frontend-design §4.2).

export function HealthView() {
  const health = useApiResource<HealthModel>(() => fetchHealth(), []);
  return (
    <AsyncResource resource={health} loadingLabel="Loading health…">
      {(data) => <Health data={data} />}
    </AsyncResource>
  );
}

/** The verdict-first Health page over a loaded health read-model. */
function Health({ data }: { data: HealthModel }) {
  // One classification for the whole page: the gate and the quality signal gate
  // on different fields (`gate.signal` vs `scan.metrics.empty`) and must not offer
  // two different explanations for the same absence (FR-EH-04, CR-130).
  const absence = signalAbsence(data.status, data.evolution);
  // The other half of the same discipline, on the POPULATED branch: figures
  // survive both a de-index and a re-index, so one derivation decides whether
  // what both widgets show is a current reading (CR-135 §3.2, S-436). `null` =
  // the ordinary case. The clock is read here, in the view, and passed in — the
  // shape `DashboardView` uses for `freshnessStatement`, so the model stays a
  // projection of its arguments.
  const currency = snapshotStaleness(data.status, data.evolution, Math.floor(Date.now() / 1000));
  // The dimension to start with — one answer for the Gate's FAIL action and the
  // Quality signal's action, so the two cannot name different dimensions.
  const lowest = data.scan.metrics.empty ? null : lowestDimension(metricRows(data.scan.metrics));
  return (
    <WidgetStack>
      <GateWidget gate={data.gate} absence={absence} currency={currency} lowest={lowest} />
      <QualitySignalWidget scan={data.scan} absence={absence} currency={currency} lowest={lowest} />
      {!data.scan.metrics.empty &&
        dimensionDetails(data.scan).map((dim) => (
          <DimensionWidget key={dim.key} dim={dim} currency={currency} />
        ))}
      {!isWidgetHidden("non-gated-tier") && (
        <Callout label="Non-gated tier" tone="muted">
          <span>
            Per-file commit/churn/risk detail now lives in <a href="/files">Files &amp; Risk</a>.
          </span>
        </Callout>
      )}
      <SignalTrendWidget snapshots={data.evolution.snapshots} />
    </WidgetStack>
  );
}

/** The figure row of a populated reading: the figure, then — when the graph has
 *  moved on from it — the one not-current sentence both widgets render. */
function ReadingFigure({
  figure,
  currency,
  note,
}: {
  figure: ReactNode;
  currency: SnapshotCurrency | null;
  /** A further qualifying line under the figure (the Gate's not-compared line,
   *  the Quality signal's scope line). */
  note?: string;
}) {
  return (
    <div className={styles.figureLines}>
      <span>{figure}</span>
      {note !== undefined && <FigureNote block>{note}</FigureNote>}
      {currency !== null && <FigureNote block>{staleNote(currency)}</FigureNote>}
    </div>
  );
}

/** The Gate widget: PASS/FAIL against the baseline, with the pass condition from
 *  `GateResult.epsilon` (FR-UI-43, CR-203 item 12).
 *
 *  The verdict compares the **last persisted snapshot** to the baseline, so a null
 *  signal is never an empty graph on its own evidence (FR-EH-04, CR-130) — it is
 *  whichever of the three absences `signalAbsence` establishes, each naming only a
 *  step that changes it, and the unscorable case naming none.
 *
 *  A signal the graph has moved on from is neither of those: the figures are
 *  real, but they do not describe the graph as it now stands. That widget keeps
 *  every figure — PASS/FAIL among them, as plain text — and drops only what
 *  asserts they are CURRENT: the green/red pass/fail badge (CR-135 §3.2, S-436).
 *  One rendering for all three causes — the chip and the sentence differ, the
 *  structure does not, so no fourth Health state is added (CR-135 §7). */
function GateWidget({
  gate,
  absence,
  currency,
  lowest,
}: {
  gate: GateResult;
  absence: SignalAbsence;
  currency: SnapshotCurrency | null;
  lowest: MetricRow | null;
}) {
  if (gate.signal === null) {
    return (
      <Widget title="Gate" copy={gateCopy} state={{ kind: "absent", absence }} absence={READING_ABSENCE[absence]} />
    );
  }
  const verdict = gate.passed ? "PASS" : "FAIL";
  // A verdict over a graph that no longer exists is history, not a verdict: the
  // chip names why, and the verdict word stays as plain text in the figure. The
  // chip tones are the design system's stale tones — `Badge` documents "red —
  // fail / error / stale", every other STALE chip in the SPA is red, and orange is
  // PENDING here, not stale (CR-135 §3.2, FR-EH-04).
  const chip = currency === null ? null : currencyChip(currency);
  const badge =
    chip !== null ? (
      <Badge tone={chip.tone}>{chip.label}</Badge>
    ) : (
      <Badge tone={gate.passed ? "green" : "red"}>{verdict}</Badge>
    );
  const floor = passFloor(gate);
  // A pass reached without a comparison never shows the floor it did not apply
  // (FR-GV-10): after a `[metric_thresholds]` change the signal may sit below it.
  const informational = isInformationalPass(gate);
  const figure = (
    <span className="mono">
      <CopyTextView
        text={gateFigureText({
          verdict,
          signal: gateFigure(gate.signal),
          baseline: gate.baseline_signal === null ? null : gateFigure(gate.baseline_signal),
          floor: floor === null ? null : gateFigure(floor),
          epsilon: gateFigure(gate.epsilon),
          informational,
        })}
      />
    </span>
  );
  return (
    <Widget
      title="Gate"
      badge={badge}
      copy={gateCopy}
      state={
        currency !== null
          ? { kind: "stale", currency }
          : { kind: "verdict", passed: gate.passed, lowest: lowest?.name ?? null }
      }
      figure={
        <ReadingFigure
          figure={figure}
          currency={currency}
          note={informational && gate.baseline_signal !== null ? GATE_NOT_COMPARED : undefined}
        />
      }
    />
  );
}

/** The chip for each not-current cause — the only thing that varies between them,
 *  so the three share one rendered structure (CR-135 §7).
 *
 *  `de-indexed` keeps S-422's red STALE chip verbatim: `Badge` documents "red —
 *  fail / error / stale", and every other STALE chip in the SPA is red.
 *  `moved-past` is the same tone for the same reason — do not read these figures
 *  as current — under a chip that states the fact rather than claiming a
 *  staleness proof. `indeterminate` is muted: nothing is established, so a red
 *  chip would overclaim in the other direction. */
function currencyChip(currency: SnapshotCurrency): { tone: BadgeTone; label: string } {
  switch (currency.cause) {
    // The de-index arm is the untagged one — see `DeIndexed`, whose value shape
    // S-422's tests pin — so `undefined` is its case, not a default.
    case undefined:
      return { tone: "red", label: "STALE" };
    case "moved-past":
      return { tone: "red", label: "MOVED PAST" };
    case "indeterminate":
      return { tone: "muted", label: "UNVERIFIED" };
  }
}

/** The Quality signal widget (FR-UI-43, CR-203 items 13 and 20): the aggregate as
 *  "n / 10000, geometric mean of the k applicable dimensions", the scope line, the
 *  explained thresholds disclosure, and the per-dimension table. With no metrics,
 *  the absence `signalAbsence` establishes — the metrics are what `scan` persists,
 *  not what `index` builds (FR-EH-04, CR-130). With metrics the graph has moved on
 *  from, the Gate's own not-current sentence, so the two cannot disagree. */
function QualitySignalWidget({
  scan,
  absence,
  currency,
  lowest,
}: {
  scan: ScanResult;
  absence: SignalAbsence;
  currency: SnapshotCurrency | null;
  lowest: MetricRow | null;
}) {
  const aggregate = scan.metrics.empty ? null : aggregateSignal(scan);
  if (aggregate === null) {
    return (
      <Widget
        title="Quality signal"
        copy={qualitySignalCopy}
        state={{ kind: "absent", absence }}
        absence={READING_ABSENCE[absence]}
      />
    );
  }
  const m = scan.metrics;
  const rows = metricRows(m);
  return (
    <Widget
      title="Quality signal"
      copy={qualitySignalCopy}
      state={currency !== null ? { kind: "stale", currency } : { kind: "scored", lowest: lowest?.name ?? null }}
      figure={
        <ReadingFigure
          figure={
            <span className="num">{signalFigure(aggregate, applicableCount(rows))}</span>
          }
          note={scopeLine(m.function_count, m.test_function_count)}
          currency={currency}
        />
      }
    >
      <MetricsTable rows={rows} />
      <details className={styles.disclosure}>
        <summary>
          {THRESHOLDS_DISCLOSURE.summary} <code>{m.thresholds_hash}</code>
        </summary>
        <p className={styles.disclosureBody}>
          <CopyTextView text={THRESHOLDS_DISCLOSURE.body} />
        </p>
      </details>
    </Widget>
  );
}

/** The ten dimensions in one sortable table — the Quality signal's evidence, and
 *  the list the dimension widgets are enumerated from (`metricRows`). */
function MetricsTable({ rows }: { rows: MetricRow[] }) {
  const columns: Column<MetricRow>[] = [
    { key: "metric", header: "Metric", cell: (r) => r.name, sortValue: (r) => r.name },
    {
      key: "score",
      header: "Score",
      numeric: true,
      cell: (r) =>
        r.value === null ? (
          <Badge tone="muted">n/a</Badge>
        ) : r.notApplicable !== null ? (
          // CR-156: computed but out of the aggregate — name why, never a score bar.
          <>
            <Badge tone="muted">not applicable</Badge> <span className="muted">{r.notApplicable}</span>
          </>
        ) : (
          <ScoreBar value={Math.round(r.value.normalized * 10_000)} max={10_000} label={r.value.normalized.toFixed(2)} />
        ),
      sortValue: (r) => (r.value === null || r.notApplicable !== null ? -1 : r.value.normalized),
    },
    {
      key: "normalized",
      header: "Normalized",
      numeric: true,
      mono: true,
      cell: (r) => (r.value === null ? <Badge tone="muted">n/a</Badge> : r.value.normalized.toFixed(2)),
      sortValue: (r) => (r.value === null ? -1 : r.value.normalized),
    },
    {
      key: "raw",
      header: "Raw",
      numeric: true,
      mono: true,
      cell: (r) => (r.value === null ? <Badge tone="muted">n/a</Badge> : r.value.raw.toFixed(2)),
      sortValue: (r) => (r.value === null ? -1 : r.value.raw),
    },
  ];
  return <DataTable columns={columns} rows={rows} rowKey={(r) => r.name} caption="Quality metrics" />;
}

/** The state a dimension's catalogue entry is evaluated at. A figure from a
 *  snapshot the graph has moved on from is the Gate's stale state, never a reading
 *  to act on (CR-135: one classification for the whole page). */
function dimensionState(dim: DimensionDetail, currency: SnapshotCurrency | null): DimensionState {
  if (dim.value === null || dim.notApplicable !== null || dim.offenderState === "not-applicable") {
    return { kind: "not-applicable" };
  }
  if (currency !== null) return { kind: "stale", currency };
  return {
    kind: "scored",
    normalized: dim.value.normalized,
    offenders: dim.offenderState,
    listed: dim.offenders.length,
  };
}

/**
 * One dimension's widget (FR-UI-43, CR-203 items 14–19): its plain question, its
 * score with the normalized and raw values, the not-applicable reason when it drops
 * out, and an action with where. The five offender-backed dimensions keep S-499's
 * three honest states, decided by `dim.offenderState` (never by list length) —
 * "not recorded" (never shown as clean, CR-162), a recorded-empty "none flagged",
 * or the worst-offender table in persisted order. The other five carry no list in
 * the payload: each states that, and points to where its units are found — never
 * an empty table (NFR-CC-04).
 */
function DimensionWidget({ dim, currency }: { dim: DimensionDetail; currency: SnapshotCurrency | null }) {
  const copy = DIMENSION_COPY[dim.key];
  const badge = offenderBadge(dim.offenderState, dim.offenders.length);
  const common = {
    title: dim.name,
    badge: badge === null ? undefined : <span className="muted">{badge}</span>,
    copy,
    state: dimensionState(dim, currency),
  };
  const evidence = <DimensionEvidence dim={dim} />;
  if (dim.value === null) {
    return (
      <Widget {...common} absence={NO_APPLICABLE_CONSTRUCT}>
        {evidence}
      </Widget>
    );
  }
  const value = dim.value;
  const figure = (
    <div className={styles.figureLines} data-dimension={dim.key}>
      {dim.notApplicable !== null ? (
        // CR-156: computed but out of the aggregate — the reason, never a score bar.
        <span className={styles.dimensionScore}>
          <Badge tone="muted">not applicable</Badge> <FigureNote>{dim.notApplicable}</FigureNote>
        </span>
      ) : (
        <span className={styles.dimensionBar}>
          <ScoreBar value={Math.round(value.normalized * 10_000)} max={10_000} label={value.normalized.toFixed(2)} />
          <span className="mono num">{value.normalized.toFixed(2)}</span>
        </span>
      )}
      <FigureNote>
        <CopyTextView text={copy.raw(value.raw)} />
      </FigureNote>
      {currency !== null && <FigureNote block>{DIMENSION_NOT_CURRENT}</FigureNote>}
    </div>
  );
  return (
    <Widget {...common} figure={figure}>
      {evidence}
    </Widget>
  );
}

/** A dimension widget's evidence: the offender table, the statement of a state
 *  with no table, or — for a dimension the payload lists nothing for — the named
 *  absence and the pointer to its units. */
function DimensionEvidence({ dim }: { dim: DimensionDetail }) {
  switch (dim.offenderState) {
    case "not-applicable":
      return null;
    case "listed":
      return <OffendersTable offenders={dim.offenders} />;
    case "none-flagged":
    case "not-recorded":
      return (
        <p className={styles.statement} data-offender-state={dim.offenderState}>
          {OFFENDER_STATEMENT[dim.offenderState]}
        </p>
      );
    case "unlisted": {
      const pointer = DIMENSION_COPY[dim.key].unlisted;
      if (pointer === undefined) return null;
      return (
        <p className={styles.statement} data-offender-state="unlisted">
          {pointer.statement}
          {pointer.view !== undefined && (
            <>
              {" "}
              <a href={pointer.view.href}>{pointer.view.label}</a>
            </>
          )}
          {pointer.view !== undefined && pointer.command !== undefined && " and"}
          {pointer.command !== undefined && (
            <>
              {" "}
              <code>{pointer.command}</code>
            </>
          )}
          {(pointer.view !== undefined || pointer.command !== undefined) && "."}
        </p>
      );
    }
  }
}

/** The worst-offender table for one dimension: entity, file, line, detail. */
function OffendersTable({ offenders }: { offenders: Offender[] }) {
  const columns: Column<Offender>[] = [
    { key: "name", header: "Offender", mono: true, cell: (o) => o.name, sortValue: (o) => o.name },
    { key: "file", header: "File", mono: true, cell: (o) => o.file, sortValue: (o) => o.file },
    {
      key: "line",
      header: "Line",
      numeric: true,
      mono: true,
      cell: (o) => (o.line === null ? "" : o.line),
      sortValue: (o) => o.line ?? -1,
    },
    { key: "detail", header: "Detail", cell: (o) => o.detail, sortValue: (o) => o.detail },
  ];
  return (
    <DataTable
      columns={columns}
      rows={offenders}
      rowKey={(o, i) => `${o.file}:${o.name}:${i}`}
      caption="Worst offenders"
      captionVisible
      pageSize={DEFAULT_TABLE_PAGE_SIZE}
    />
  );
}

/** The Signal trend widget: the signal-evolution series as its accessible
 *  data-table twin, one row per snapshot oldest-first with signed movement. No
 *  snapshots → the named absence and the command that records one. */
function SignalTrendWidget({ snapshots }: { snapshots: EvolutionPoint[] }) {
  if (snapshots.length === 0) {
    return <Widget title="Signal trend" copy={signalTrendCopy} state={{ snapshots: 0 }} absence={NO_SNAPSHOTS} />;
  }
  const columns: Column<EvolutionPoint>[] = [
    { key: "snapshot", header: "Snapshot", numeric: true, mono: true, cell: (p) => p.snapshot_id, sortValue: (p) => p.snapshot_id },
    { key: "commit", header: "Commit", mono: true, cell: (p) => shortSha(p.commit_sha), sortValue: (p) => p.commit_sha ?? "" },
    { key: "signal", header: "Signal", numeric: true, mono: true, cell: (p) => optSignal(p.signal), sortValue: (p) => p.signal ?? -1 },
    { key: "delta", header: "Δ vs prev", numeric: true, mono: true, cell: (p) => optDelta(p.signal_delta), sortValue: (p) => p.signal_delta ?? 0 },
  ];
  return (
    <Widget title="Signal trend" copy={signalTrendCopy} state={{ snapshots: snapshots.length }}>
      <DataTable
        columns={columns}
        rows={snapshots}
        rowKey={(p) => String(p.snapshot_id)}
        caption="Signal evolution"
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Widget>
  );
}
