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
 * honest states (an empty graph's gate is a muted `n/a` naming `logos index`; an
 * ADR-21 metric drop-out is a muted `n/a`, never a zero; no snapshots is an honest
 * empty state). Every read is GET-only — loading the view mutates no store
 * (ADR-28); sorting the tables is client-side over the full dataset.
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
  structuralDetails,
  type MetricDetail,
  type MetricRow,
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
  return (
    <div className={styles.view}>
      <GateBand gate={data.gate} />
      <MetricsCard scan={data.scan} />
      <Callout label="Non-gated tier" tone="muted">
        <span>
          Per-file commit/churn/risk detail now lives in <a href="/files">Files &amp; Risk</a>.
        </span>
      </Callout>
      <EvolutionCard snapshots={data.evolution.snapshots} />
    </div>
  );
}

/** The gate verdict band: PASS (green) / FAIL (red) + current-vs-baseline. An empty
 *  graph has no signal → a muted `n/a` callout naming the producing command. */
function GateBand({ gate }: { gate: GateResult }) {
  if (gate.signal === null) {
    return (
      <Callout label="Gate" tone="muted">
        <span>
          n/a — empty graph; run <code>logos index</code>
        </span>
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

/** The per-metric grid + aggregate, then the folded structural drill-downs. An
 *  empty graph renders the honest empty state, not a grid of zeroed placeholders. */
function MetricsCard({ scan }: { scan: ScanResult }) {
  if (scan.metrics.empty) {
    return (
      <Card title="Quality signal">
        <EmptyState message="No metrics yet — run" command="logos index" />
      </Card>
    );
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
