/*
 * DashboardView (S-187, FR-UI-09, FR-UI-21) — the Dashboard tab migrated to React
 * over `/api/v1/overview`, reusing the S-186 page-integration pattern: registered
 * in `views/index.ts` at the `/` root route (S-194), mounted by `App.tsx` in the
 * AppShell content slot, rendering exclusively through the S-193 design system.
 *
 * It preserves the server-rendered Dashboard's verdict-first layout
 * (web/src/views/overview.rs, frontend-design §4.1): a freshness statement leads,
 * then the full-width Project Overview, then three equal-width pairs (Quality |
 * Languages, Graph | Activity, Test coverage | Code coverage), then the full-width
 * trust-score card — and its honest empty states (an un-indexed root is one card
 * naming `logos index`; every figure traces to a read-model field, none fabricated;
 * NFR-RA-05, NFR-CC-04). Every read is GET-only — loading the view mutates no store
 * (ADR-28).
 */

import { AsyncResource, fetchOverview, useApiResource } from "../../api/index.ts";
import type {
  CoverageStatus,
  CrossTotals,
  GateResult,
  LanguageComposition,
  LanguagesInfo,
  OverviewModel,
  StatsInfo,
  StatusInfo,
  TestGapsReport,
  WikiPage,
} from "../../api/types.ts";
import { Badge, Callout, Card, EmptyState, ScoreBar } from "../../components/index.ts";
import {
  bandOf,
  fileWeights,
  freshnessStatement,
  pctBp,
  snippetOf,
  trustScoreBp,
} from "./dashboardModel.ts";
import styles from "./Dashboard.module.css";

/** Current wall-clock as unix seconds — the reference the freshness line humanises
 *  against (presentation-only; reading the clock writes no store). */
function nowUnix(): number {
  return Math.floor(Date.now() / 1000);
}

export function DashboardView() {
  const overview = useApiResource<OverviewModel>(() => fetchOverview(), []);
  return (
    <AsyncResource
      resource={overview}
      loadingLabel="Loading the dashboard…"
      // An un-indexed root renders the single honest empty state, never zeroed
      // roll-ups (frontend-design §4.1, NFR-CC-04).
      isEmpty={(data) => !data.status.indexed}
      empty={<EmptyState message="No index yet — run" command="logos index" />}
    >
      {(data) => <Dashboard data={data} />}
    </AsyncResource>
  );
}

/** The verdict-first Dashboard over a loaded, indexed overview read-model. */
function Dashboard({ data }: { data: OverviewModel }) {
  return (
    <div className={styles.view}>
      <Callout label="Index" tone="signal">
        <span>{freshnessStatement(data.status, nowUnix())}</span>
      </Callout>
      <ProjectOverviewCard page={data.overview_page} />
      <div className={styles.pair}>
        <QualityCard gate={data.gate} />
        <LanguagesCard composition={data.composition} languages={data.languages} />
      </div>
      <div className={styles.pair}>
        <GraphCard status={data.status} />
        <ActivityCard stats={data.stats} />
      </div>
      <div className={styles.pair}>
        <TestCoverageCard gaps={data.gaps} />
        <CodeCoverageCard coverage={data.coverage} />
      </div>
      <TrustCard data={data} />
    </div>
  );
}

/** A same-origin GET link pinned to a card's bottom-left (CSP-safe, no JS needed). */
function DetailLink({ href, label }: { href: string; label: string }) {
  return (
    <a className={styles.cardLink} href={href}>
      {label} →
    </a>
  );
}

/** *Quality index* — the BR-34-banded signal + raw figure + PASS/FAIL badge. An
 *  empty graph has no signal → honest empty state, never a misleading bar. */
function QualityCard({ gate }: { gate: GateResult }) {
  return (
    <Card title="Quality index">
      {gate.signal === null ? (
        <EmptyState message="No quality signal yet — run" command="logos index" />
      ) : (
        <>
          <div className={styles.heroFigure}>
            <span className={styles.heroBand}>{bandOf(gate.signal).label}</span>
            <span className={`${styles.heroRaw} mono num`}>{gate.signal} / 10000</span>
            <Badge tone={gate.passed ? "green" : "red"}>{gate.passed ? "PASS" : "FAIL"}</Badge>
          </div>
          <ScoreBar value={gate.signal} max={10_000} tone={bandOf(gate.signal).tone} label={`${gate.signal} / 10000`} />
        </>
      )}
      <DetailLink href="/health" label="Health" />
    </Card>
  );
}

/** *Code coverage* — the overall line-% as a green (never banded) bar. */
function CodeCoverageCard({ coverage }: { coverage: CoverageStatus }) {
  const bp = coverage.overall_coverage_bp;
  return (
    <Card title="Code coverage">
      {bp === null ? (
        <EmptyState message="No coverage ingested — run" command="logos coverage ingest <report>" />
      ) : (
        <>
          <div className={styles.heroFigure}>
            <span className={`${styles.heroRaw} num`}>{pctBp(bp)}</span>
          </div>
          <ScoreBar value={bp} max={10_000} label={pctBp(bp)} />
        </>
      )}
      <DetailLink href="/coverage" label="Coverage" />
    </Card>
  );
}

/** *Test coverage* — the test-reachability ratio as a green (never banded) bar,
 *  with the descriptor that it is reachability, not line coverage. */
function TestCoverageCard({ gaps }: { gaps: TestGapsReport }) {
  const ratio = gaps.coverage_ratio;
  return (
    <Card title="Test coverage">
      {ratio === null ? (
        <p className="muted">n/a — no functions to cover.</p>
      ) : (
        <>
          <div className={styles.heroFigure}>
            <span className={`${styles.heroRaw} num`}>{pctBp(ratio)}</span>
          </div>
          <ScoreBar value={ratio} max={10_000} label={pctBp(ratio)} />
          <p className="muted">reachable from a test</p>
        </>
      )}
      <DetailLink href="/gaps" label="Gaps" />
    </Card>
  );
}

/** *Languages (project-only)* — a magnitude bar list sized by node count. */
function LanguagesCard({
  composition,
  languages,
}: {
  composition: LanguageComposition;
  languages: LanguagesInfo;
}) {
  if (composition.languages.length === 0) {
    return (
      <Card title="Languages">
        <EmptyState message="No languages indexed — run" command="logos index" />
      </Card>
    );
  }
  const max = Math.max(1, ...composition.languages.map((l) => l.nodes));
  return (
    <Card title="Languages">
      <p className="muted">Counts are indexed symbols, not files.</p>
      {composition.languages.map((l) => (
        <div className={styles.langRow} key={l.language}>
          <span className={`${styles.langName} mono`}>{l.language}</span>
          <ScoreBar value={l.nodes} max={max} tone="magnitude" label={String(l.nodes)} />
          <span className={`${styles.langCount} mono num`}>{l.nodes}</span>
        </div>
      ))}
      {languages.skipped.length > 0 && (
        <p className="muted">{languages.skipped.length} grammar(s) skipped at load</p>
      )}
    </Card>
  );
}

/** *Graph (compact)* — structural counts + resolution coverage from `status`. */
function GraphCard({ status }: { status: StatusInfo }) {
  const resolution = `${(status.resolution_coverage * 100).toFixed(1)}% (${status.refs_resolved} of ${status.refs_total} refs)`;
  return (
    <Card title="Graph">
      <dl className={styles.statList}>
        <dt>Files</dt>
        <dd className="mono num">{status.file_count}</dd>
        <dt>Nodes</dt>
        <dd className="mono num">{status.node_count}</dd>
        <dt>Edges</dt>
        <dd className="mono num">{status.edge_count}</dd>
        <dt>Resolution</dt>
        <dd className="mono">{resolution}</dd>
      </dl>
      <p className="muted">A partial resolution figure is expected — many references resolve lazily.</p>
    </Card>
  );
}

/** *Activity (compact)* — usage telemetry from `stats`; no telemetry → honest note. */
function ActivityCard({ stats }: { stats: StatsInfo }) {
  if (stats.calls_total === 0) {
    return (
      <Card title="Activity">
        <EmptyState message="No telemetry yet — populate it by running" command="logos <command>" />
      </Card>
    );
  }
  return (
    <Card title="Activity">
      <dl className={styles.statList}>
        <dt>Window</dt>
        <dd className="mono">{stats.window_days} days</dd>
        <dt>Calls</dt>
        <dd className="mono num">{stats.calls_total}</dd>
        <dt>Latency p50/p95</dt>
        <dd className="mono num">{stats.latency_p50_ms} / {stats.latency_p95_ms} ms</dd>
        <dt>Reads saved (est)</dt>
        <dd className="mono num">{stats.reads_saved_estimate}</dd>
      </dl>
    </Card>
  );
}

/** *Project Overview* — a prose snippet of the agent wiki page, or an honest
 *  "not yet generated" empty state naming the producing path. */
function ProjectOverviewCard({ page }: { page: WikiPage | null }) {
  if (page === null) {
    return (
      <Card title="Project Overview">
        <EmptyState
          message="No project overview generated yet — it is written off the work-list of"
          command="logos wiki status"
        />
      </Card>
    );
  }
  const snippet = snippetOf(page.body);
  return (
    <Card title="Project Overview">
      {snippet === "" ? (
        <p className="muted">A project overview is available in the wiki.</p>
      ) : (
        <p className={styles.snippet}>{snippet}</p>
      )}
      <DetailLink href="/wiki" label="Open wiki" />
    </Card>
  );
}

/** *Coverage trust* — the architecturally-weighted Q4 trust share + mini-quadrant,
 *  advisory and never gated. Degrades to the honest ingest/refresh empty states. */
function TrustCard({ data }: { data: OverviewModel }) {
  const { cross, hotspots } = data;
  let body;
  if (!cross.has_fresh_coverage) {
    body =
      cross.notice !== null ? (
        <EmptyState
          message="No coverage ingested — run"
          command="logos coverage ingest <report> or logos coverage refresh"
        />
      ) : (
        <EmptyState message="Coverage is stale — run" command="logos coverage refresh" />
      );
  } else {
    const bp = trustScoreBp(cross.symbols, fileWeights(hotspots));
    body =
      bp === null ? (
        <EmptyState
          message="Coverage ingested, but no symbol spans could be placed — refresh with"
          command="logos coverage refresh"
        />
      ) : (
        <>
          <div className={styles.heroFigure}>
            <span className={`${styles.heroRaw} num`}>{pctBp(bp)}</span>
          </div>
          <ScoreBar value={bp} max={10_000} label={pctBp(bp)} />
          <p className="muted">
            architecturally-weighted Q4 (reachable &amp; executed) share — advisory, never gated
          </p>
          <MiniQuadrant totals={cross.totals} />
        </>
      );
  }
  return (
    <Card title="Coverage trust" className={styles.trust}>
      {body}
      <DetailLink href="/quadrant" label="Open quadrant" />
    </Card>
  );
}

/** The mini 2×2 quadrant (CR-040 flipped layout: executed on top, reachable to the
 *  right, so Q4 trust sits top-right). Each cell carries its tag AND count so the
 *  grid reads without hover, and an always-visible compact legend names each cell's
 *  meaning (mirroring the server-rendered `mini_legend`) — colour is never the only
 *  channel and the semantics are not hidden behind a hover tooltip (WCAG 2.1 AA, §7). */
function MiniQuadrant({ totals }: { totals: CrossTotals }) {
  const cells: { cls: string; tag: string; title: string; count: number }[] = [
    { cls: styles.mqQ1, tag: "Q1", title: "Q1 — unreachable, executed (false-green)", count: totals.q1 },
    { cls: styles.mqQ4, tag: "Q4", title: "Q4 — reachable, executed (trust)", count: totals.q4 },
    { cls: styles.mqQ3, tag: "Q3", title: "Q3 — unreachable, unexecuted (true gap)", count: totals.q3 },
    { cls: styles.mqQ2, tag: "Q2", title: "Q2 — reachable, unexecuted (dead edge)", count: totals.q2 },
  ];
  // The compact legend, ordered worst → best (Q1 → Q4) to match the full Quadrant
  // view's legend and the urgency-table lead.
  const legend: { tag: string; meaning: string }[] = [
    { tag: "Q1", meaning: "false-green" },
    { tag: "Q2", meaning: "dead edge" },
    { tag: "Q3", meaning: "true gap" },
    { tag: "Q4", meaning: "trust" },
  ];
  return (
    <div className={styles.miniQuadrantWrap}>
      <a className={styles.miniQuadrant} href="/quadrant" aria-label="Coverage quadrant — open the full view">
        {cells.map((c) => (
          <span className={`${styles.mqCell} ${c.cls}`} key={c.tag} title={c.title}>
            <span className={styles.mqTag}>{c.tag}</span>
            <span className={`${styles.mqCount} num`}>{c.count}</span>
          </span>
        ))}
      </a>
      <ul className={styles.mqLegend}>
        {legend.map((l) => (
          <li key={l.tag}>
            <span className={styles.mqTag}>{l.tag}</span> — {l.meaning}
          </li>
        ))}
      </ul>
    </div>
  );
}
