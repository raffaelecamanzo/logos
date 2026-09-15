/*
 * HealthView (S-187, FR-UI-04, FR-UI-21) — the Health tab migrated to React over
 * `/api/v1/health`, reusing the S-186 page-integration pattern (registered in
 * `views/index.ts` for `/health`, mounted by `App.tsx`, rendered exclusively
 * through the S-193 design system).
 *
 * It preserves the server-rendered Health view's verdict-first layout
 * (web/src/views/health.rs, frontend-design §4.2): the gate verdict band leads,
 * then the per-metric quality grid + the folded structural drill-downs, then the
 * non-gated pointer to Files & Risk, then the signal-evolution trend — and its
 * honest states (a gate and a metric grid with nothing to show name the step that
 * would produce it — `logos scan` on a populated graph, `logos index` on an empty
 * one, FR-EH-04/CR-130; an ADR-21 metric drop-out is a muted `n/a`, never a zero;
 * no snapshots is an honest empty state). Every read is GET-only — loading the
 * view mutates no store (ADR-28); sorting the tables is client-side over the full
 * dataset.
 */

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
  Callout,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ScoreBar,
  type Column,
} from "../../components/index.ts";
import {
  aggregateSignal,
  metricRows,
  optDelta,
  optSignal,
  shortSha,
  signalAbsence,
  structuralDetails,
  type MetricDetail,
  type MetricRow,
  type SignalAbsence,
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
  // One classification for the whole page: the gate band and the quality grid gate
  // on different fields (`gate.signal` vs `scan.metrics.empty`) and must not offer
  // two different explanations for the same absence (FR-EH-04, CR-130).
  const absence = signalAbsence(data.status, data.evolution);
  return (
    <div className={styles.view}>
      <GateBand gate={data.gate} absence={absence} />
      <MetricsCard scan={data.scan} absence={absence} />
      <Callout label="Non-gated tier" tone="muted">
        <span>
          Per-file commit/churn/risk detail now lives in <a href="/files">Files &amp; Risk</a>.
        </span>
      </Callout>
      <EvolutionCard snapshots={data.evolution.snapshots} />
    </div>
  );
}

/** The gate verdict band: PASS (green) / FAIL (red) + current-vs-baseline.
 *
 *  The verdict compares the **last persisted snapshot** to the baseline, so a null
 *  signal is never an empty graph on its own evidence (FR-EH-04, CR-130) — it is
 *  whichever of the three absences `signalAbsence` establishes, each naming only a
 *  step that changes it, and the unscorable case naming none. */
function GateBand({ gate, absence }: { gate: GateResult; absence: SignalAbsence }) {
  if (gate.signal === null) {
    return (
      <Callout label="Gate" tone="muted">
        <span>{gateAbsence(absence)}</span>
      </Callout>
    );
  }
  const baseline = gate.baseline_signal === null ? "no baseline" : String(gate.baseline_signal);
  return (
    <Callout label="Gate" tone={gate.passed ? "pass" : "signal"}>
      <span className={styles.gateBody}>
        <Badge tone={gate.passed ? "green" : "red"}>{gate.passed ? "PASS" : "FAIL"}</Badge>
        <span className="mono">
          current {gate.signal} vs baseline {baseline}
        </span>
      </span>
    </Callout>
  );
}

/** The gate band's muted body, one per absence — never a grid of zeroed
 *  placeholders, and never `logos index` for a figure `scan` produces. */
function gateAbsence(absence: SignalAbsence) {
  switch (absence) {
    case "unindexed":
      return (
        <>
          n/a — nothing indexed yet; run <code>logos index</code>
        </>
      );
    case "unscanned":
      return (
        <>
          n/a — no scan has been run for this project; run <code>logos scan</code>
        </>
      );
    // Indexed and scanned, but nothing in production scope to score: no command
    // changes this, so none is named (FR-EH-04).
    case "no-production-scope":
      return <>n/a — the last scan found no production functions to score</>;
  }
}

/** The quality grid's honest empty state, one per absence — the same classification
 *  the gate band renders, so the two cards cannot explain one absence two ways. */
function metricsAbsence(absence: SignalAbsence) {
  switch (absence) {
    case "unindexed":
      return <EmptyState message="Nothing indexed yet — run" command="logos index" />;
    case "unscanned":
      return <EmptyState message="No scan has been run yet — run" command="logos scan" />;
    case "no-production-scope":
      return (
        <EmptyState message="The last scan found no production functions to score — every indexed symbol is test scope (FR-QM-08), so there is no quality signal to report." />
      );
  }
}

/** The per-metric grid + aggregate, then the folded structural drill-downs. With no
 *  metrics to show, the honest empty state for whichever absence this is — the
 *  metrics are what `scan` persists, not what `index` builds (FR-EH-04, CR-130). */
function MetricsCard({ scan, absence }: { scan: ScanResult; absence: SignalAbsence }) {
  if (scan.metrics.empty) {
    return <Card title="Quality signal">{metricsAbsence(absence)}</Card>;
  }
  const aggregate = aggregateSignal(scan);
  const rows = metricRows(scan.metrics);
  const columns: Column<MetricRow>[] = [
    { key: "metric", header: "Metric", cell: (r) => r.name, sortValue: (r) => r.name },
    {
      key: "score",
      header: "Score",
      numeric: true,
      cell: (r) =>
        r.value === null ? (
          <Badge tone="muted">n/a</Badge>
        ) : (
          <ScoreBar value={Math.round(r.value.normalized * 10_000)} max={10_000} label={r.value.normalized.toFixed(2)} />
        ),
      sortValue: (r) => (r.value === null ? -1 : r.value.normalized),
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
  return (
    <>
      <Card title="Quality signal">
        <p className={styles.aggregate}>
          Aggregate{" "}
          {aggregate === null ? (
            <Badge tone="muted">n/a</Badge>
          ) : (
            <>
              <span className="mono num">{optSignal(aggregate)}</span> <span className="muted">/ 10000</span>
            </>
          )}
        </p>
        <DataTable columns={columns} rows={rows} rowKey={(r) => r.name} caption="Quality metrics" />
      </Card>
      <section className={styles.details}>
        {structuralDetails(scan).map((dim) => (
          <Drilldown key={dim.name} dim={dim} />
        ))}
        <AggregateScope scan={scan} />
      </section>
    </>
  );
}

/** One dimension's drill-down, rendered open for the no-JS reader. Three honest
 *  states: an n/a drop-out (no table), an applicable-but-unflagged note, or the
 *  worst-offender table. */
function Drilldown({ dim }: { dim: MetricDetail }) {
  let tag;
  let body;
  if (dim.value === null) {
    tag = <Badge tone="muted">n/a</Badge>;
    body = (
      <>
        <p className={styles.definition}>{dim.definition}</p>
        <p className="muted">n/a — no applicable construct in this codebase</p>
      </>
    );
  } else if (dim.offenders.length === 0) {
    tag = <span className="muted">none flagged</span>;
    body = (
      <>
        <p className={styles.definition}>{dim.definition}</p>
        <p className="muted">No offenders flagged within thresholds.</p>
      </>
    );
  } else {
    tag = <span className="muted">{dim.offenders.length} flagged</span>;
    body = (
      <>
        <p className={styles.definition}>{dim.definition}</p>
        <OffendersTable offenders={dim.offenders} />
      </>
    );
  }
  return (
    <details open className={styles.detail}>
      <summary>
        <span className={styles.detailName}>{dim.name}</span> {tag}
      </summary>
      {body}
    </details>
  );
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

/** The extended-aggregate provenance (FR-QM-14): the production scope the run was
 *  scored under and the effective-thresholds hash — figures straight from the scan. */
function AggregateScope({ scan }: { scan: ScanResult }) {
  const m = scan.metrics;
  return (
    <Card title="Aggregate scope" className={styles.scope}>
      <p className="muted">
        {m.node_count} nodes · {m.edge_count} edges · {m.function_count} production functions ·{" "}
        {m.test_function_count} test functions excluded
      </p>
      <p className="muted mono">thresholds {m.thresholds_hash}</p>
    </Card>
  );
}

/** The signal-evolution trend as its accessible data-table twin, one row per
 *  snapshot oldest-first with signed movement. No snapshots → an honest empty state. */
function EvolutionCard({ snapshots }: { snapshots: EvolutionPoint[] }) {
  if (snapshots.length === 0) {
    return (
      <Card title="Signal trend">
        <EmptyState message="No snapshots yet — run" command="logos scan" />
      </Card>
    );
  }
  const columns: Column<EvolutionPoint>[] = [
    { key: "snapshot", header: "Snapshot", numeric: true, mono: true, cell: (p) => p.snapshot_id, sortValue: (p) => p.snapshot_id },
    { key: "commit", header: "Commit", mono: true, cell: (p) => shortSha(p.commit_sha), sortValue: (p) => p.commit_sha ?? "" },
    { key: "signal", header: "Signal", numeric: true, mono: true, cell: (p) => optSignal(p.signal), sortValue: (p) => p.signal ?? -1 },
    { key: "delta", header: "Δ vs prev", numeric: true, mono: true, cell: (p) => optDelta(p.signal_delta), sortValue: (p) => p.signal_delta ?? 0 },
  ];
  return (
    <Card title="Signal trend">
      <DataTable
        columns={columns}
        rows={snapshots}
        rowKey={(p) => String(p.snapshot_id)}
        caption="Signal evolution"
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Card>
  );
}
