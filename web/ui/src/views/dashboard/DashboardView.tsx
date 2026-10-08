/*
 * DashboardView (S-187, FR-UI-09, FR-UI-21) — the Dashboard tab migrated to React
 * over `/api/v1/overview`, reusing the S-186 page-integration pattern: registered
 * in `views/index.ts` at the `/` root route (S-194), mounted by `App.tsx` in the
 * AppShell content slot, rendering exclusively through the S-193 design system.
 *
 * It keeps the server-rendered Dashboard's verdict-first order (frontend-design
 * §4.1): a freshness statement leads, then Project Overview, Quality index,
 * Languages, Graph, Activity, Rule findings and Code coverage — and its honest
 * empty states (an un-indexed root is one empty state naming `logos index`; every
 * figure traces to a read-model field, none fabricated; NFR-RA-05, NFR-CC-04).
 *
 * S-617 (CR-203, FR-UI-39/40): every widget renders through `Widget` with its
 * catalogue entry (`copy/dashboard.copy.ts`; Rule findings shares
 * `copy/ruleFindings.copy.ts` with the Rule findings view), stacked in ONE
 * `WidgetStack`. The former equal-width pairs are one column: a stack's widgets
 * are its direct children, one gap apart. An absence inside a widget is a
 * left-aligned statement in its figure row that names the command that fills it
 * (CR-206). CR-079 retired the
 * Coverage-trust card and the reachability roll-up, promoting the architecture
 * Rule findings into that former slot. Every read is GET-only — loading the view
 * mutates no store (ADR-28).
 */

import { AsyncResource, fetchOverview, useApiResource } from "../../api/index.ts";
import type {
  CoverageStatus,
  GateResult,
  LanguageComposition,
  LanguagesInfo,
  OverviewModel,
  RulesReport,
  StatsInfo,
  StatusInfo,
  WikiPage,
} from "../../api/types.ts";
import { Badge, Callout, EmptyState, FigureNote, ScoreBar, Widget, WidgetStack } from "../../components/index.ts";
import {
  activity as activityCopy,
  codeCoverage,
  DASHBOARD_ABSENCE,
  graph as graphCopy,
  languages as languagesCopy,
  projectOverview,
  qualityIndex,
} from "../../copy/dashboard.copy.ts";
import { RULE_FINDINGS_TEXT, ruleFindings, ruleFindingsState } from "../../copy/ruleFindings.copy.ts";
import { plural } from "../../copy/types.ts";
import {
  bandOf,
  fmtInt,
  freshnessStatement,
  pctBp,
  resolutionStatement,
  snippetOf,
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
      // roll-ups (frontend-design §4.1, NFR-CC-04): a view with no widget at all.
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
    <WidgetStack>
      <Callout label="Index" tone="signal">
        <span>{freshnessStatement(data.status, nowUnix())}</span>
      </Callout>
      <ProjectOverviewCard page={data.overview_page} />
      <QualityCard gate={data.gate} />
      <LanguagesCard composition={data.composition} languages={data.languages} />
      <GraphCard status={data.status} />
      <ActivityCard stats={data.stats} />
      <RuleFindingsCard rules={data.rules} />
      <CodeCoverageCard coverage={data.coverage} />
    </WidgetStack>
  );
}

/** A same-origin GET link to the view that details a widget (CSP-safe, no JS needed). */
function DetailLink({ href, label }: { href: string; label: string }) {
  return (
    <a className={styles.cardLink} href={href}>
      {label} →
    </a>
  );
}

/** *Quality index* — the BR-34-banded signal + raw figure, with the PASS/FAIL
 *  badge on the title row.
 *
 *  The signal comes from the last persisted snapshot, which `scan` writes, so a null
 *  one is never an empty graph — the un-indexed root is already the view's own single
 *  empty state above, so `status.indexed` holds here (FR-EH-04, CR-130).
 *
 *  It states only the absence, not its cause. `OverviewModel` carries no fact that
 *  separates "no scan has ever run" from "a scan ran and scored nothing in production
 *  scope" (FR-QM-08) — the health tab has `evolution.snapshots` for that and says
 *  which it is; this widget does not, so it does not guess. `logos scan` is named as
 *  the step that records a signal, which is what the widget reports missing. */
function QualityCard({ gate }: { gate: GateResult }) {
  const signal = gate.signal;
  const link = <DetailLink href="/health" label="Health" />;
  if (signal === null) {
    return (
      <Widget
        title="Quality index"
        copy={qualityIndex}
        absence={DASHBOARD_ABSENCE.noSignal}
      >
        {link}
      </Widget>
    );
  }
  const band = bandOf(signal);
  return (
    <Widget
      title="Quality index"
      badge={<Badge tone={gate.passed ? "green" : "red"}>{gate.passed ? "PASS" : "FAIL"}</Badge>}
      copy={qualityIndex}
      figure={
        <>
          <span>{band.label}</span>
          <span className="mono num">
            {fmtInt(signal)} / {fmtInt(10_000)}
          </span>
        </>
      }
    >
      <ScoreBar value={signal} max={10_000} tone={band.tone} label={`${fmtInt(signal)} / ${fmtInt(10_000)}`} />
      {link}
    </Widget>
  );
}

/** *Code coverage* — the overall line-% as a green (never banded) bar. */
function CodeCoverageCard({ coverage }: { coverage: CoverageStatus }) {
  const bp = coverage.overall_coverage_bp;
  const link = <DetailLink href="/coverage" label="Coverage" />;
  if (bp === null) {
    return (
      <Widget title="Code coverage" copy={codeCoverage} absence={DASHBOARD_ABSENCE.noCoverage}>
        {link}
      </Widget>
    );
  }
  return (
    <Widget
      title="Code coverage"
      copy={codeCoverage}
      figure={
        <span>
          <span className="num">{pctBp(bp)}</span> <FigureNote>of lines covered</FigureNote>
        </span>
      }
    >
      <ScoreBar value={bp} max={10_000} label={pctBp(bp)} />
      {link}
    </Widget>
  );
}

/** *Rule findings* — the architecture-rules verdict projected from `overview.rules`
 *  (CR-079). Three honest states (NFR-CC-04): an absence naming the file and the
 *  evaluating command when no `.logos/rules.toml` is authored yet, or when one is authored
 *  but declares zero rules (`!rules_present || checked_rules === 0`) — the
 *  `logos init` default is not a clean check, it is nothing evaluated (CR-141,
 *  S-438); a red FAIL naming the finding count when there are findings; a green
 *  PASS only once at least one rule was actually checked. Never a fabricated
 *  figure.
 *
 *  Findings are checked FIRST, before the onboarding condition (S-354): the
 *  always-on structural/admission fold-ins fire independent of a loaded
 *  contract, so a contract-less (or a zero-rule) project can still carry real
 *  violations — those must win over the onboarding prompt, never be hidden
 *  behind it. */
function RuleFindingsCard({ rules }: { rules: RulesReport }) {
  const state = ruleFindingsState(rules);
  const { findings, checked } = state;
  const link = <DetailLink href="/gaps" label="Rule findings" />;
  if (findings === 0 && checked === 0) {
    return (
      <Widget title="Rule findings" copy={ruleFindings} absence={RULE_FINDINGS_TEXT.noRules}>
        {link}
      </Widget>
    );
  }
  return (
    <Widget
      title="Rule findings"
      badge={findings > 0 ? <Badge tone="red">FAIL</Badge> : <Badge tone="green">PASS</Badge>}
      copy={ruleFindings}
      figure={
        <span>{findings > 0 ? RULE_FINDINGS_TEXT.findings(findings, rules.checked_rules) : RULE_FINDINGS_TEXT.clean(checked)}</span>
      }
    >
      {link}
    </Widget>
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
      <Widget title="Languages" copy={languagesCopy} absence={DASHBOARD_ABSENCE.noLanguages} />
    );
  }
  const n = composition.languages.length;
  const max = Math.max(1, ...composition.languages.map((l) => l.nodes));
  const skipped = languages.skipped.length;
  return (
    <Widget
      title="Languages"
      copy={languagesCopy}
      figure={
        <span>
          {n} <FigureNote>{plural(n, "language", "languages")} indexed</FigureNote>
        </span>
      }
    >
      {composition.languages.map((l) => (
        <div className={styles.langRow} key={l.language}>
          <span className={`${styles.langName} mono`}>{l.language}</span>
          <ScoreBar value={l.nodes} max={max} tone="magnitude" label={fmtInt(l.nodes)} />
          <span className={`${styles.langCount} mono num`}>{fmtInt(l.nodes)}</span>
        </div>
      ))}
      {skipped > 0 && (
        <p className="muted">
          {skipped} {plural(skipped, "grammar", "grammars")} skipped at load
        </p>
      )}
    </Widget>
  );
}

/** *Graph (compact)* — structural counts from `status`, plus the CR-085
 *  total/source/test physical-LOC roll-up, with the reference resolution as the
 *  figure. The roll-up is computed only at full-index time ([FR-IX-12]); on a
 *  graph where it is not yet computed, all three figures are `null` in lock-step,
 *  so the rows are left out and the evidence names the full index rather than a
 *  fabricated `0` ([NFR-CC-04]). */
function GraphCard({ status }: { status: StatusInfo }) {
  const resolution = resolutionStatement(status);
  // Bind the LOC roll-up as a narrowed object so the figures are `number` (not
  // `number | null`) at the render site — the three fields are null in lock-step
  // ([FR-IX-12]), so either all three are present or the rows are left out.
  const loc =
    status.total_line_count !== null &&
    status.source_line_count !== null &&
    status.test_line_count !== null
      ? {
          total: status.total_line_count,
          source: status.source_line_count,
          test: status.test_line_count,
        }
      : null;
  return (
    <Widget
      title="Graph"
      copy={graphCopy}
      figure={
        <span>
          <span className="mono">{resolution}</span> <FigureNote>of references resolved</FigureNote>
        </span>
      }
    >
      <dl className={styles.statList}>
        <dt>Files</dt>
        <dd className="mono num">{fmtInt(status.file_count)}</dd>
        {loc !== null && (
          <>
            <dt>
              Total Lines of Code <sup className={styles.footnoteMark}>*</sup>
            </dt>
            <dd className="mono num">{fmtInt(loc.total)}</dd>
            <dt>
              Source Lines of Code <sup className={styles.footnoteMark}>*</sup>
            </dt>
            <dd className="mono num">{fmtInt(loc.source)}</dd>
            <dt>
              Test Lines of Code <sup className={styles.footnoteMark}>*</sup>
            </dt>
            <dd className="mono num">{fmtInt(loc.test)}</dd>
          </>
        )}
        <dt>Nodes</dt>
        <dd className="mono num">{fmtInt(status.node_count)}</dd>
        <dt>Edges</dt>
        <dd className="mono num">{fmtInt(status.edge_count)}</dd>
      </dl>
      <div className={styles.footnotes}>
        {loc !== null ? <p>* LOC figures reflect the last full index</p> : <p>{DASHBOARD_ABSENCE.noLines}</p>}
        <p>A partial resolution figure is expected: many references resolve lazily.</p>
      </div>
    </Widget>
  );
}

/** *Activity (compact)* — usage telemetry from `stats`; no telemetry → an honest absence. */
function ActivityCard({ stats }: { stats: StatsInfo }) {
  if (stats.calls_total === 0) {
    return <Widget title="Activity" copy={activityCopy} absence={DASHBOARD_ABSENCE.noTelemetry} />;
  }
  return (
    <Widget
      title="Activity"
      copy={activityCopy}
      figure={
        <span>
          <span className="num">{fmtInt(stats.calls_total)}</span>{" "}
          <FigureNote>
            {plural(stats.calls_total, "call", "calls")} in the last {stats.window_days} days
          </FigureNote>
        </span>
      }
    >
      <dl className={styles.statList}>
        <dt>Window</dt>
        <dd className="mono">{stats.window_days} days</dd>
        <dt>Calls</dt>
        <dd className="mono num">{fmtInt(stats.calls_total)}</dd>
        <dt>Latency p50/p95</dt>
        <dd className="mono num">{fmtInt(stats.latency_p50_ms)} / {fmtInt(stats.latency_p95_ms)} ms</dd>
        <dt>Reads saved (est)</dt>
        <dd className="mono num">{fmtInt(stats.reads_saved_estimate)}</dd>
      </dl>
    </Widget>
  );
}

/** *Project Overview* — a prose snippet of the agent wiki page, or an honest
 *  "not yet generated" absence naming the producing command.
 *
 *  The page is agent-authored prose the binary never writes itself (ADR-57), and
 *  `wiki status` is a pure read that only *lists* the work — running it leaves the
 *  page just as absent. So the absence names the write that ends it (FR-EH-04,
 *  CR-130, CR-206), which is also the command `wiki status` hands out for this slug. */
function ProjectOverviewCard({ page }: { page: WikiPage | null }) {
  if (page === null) {
    return (
      <Widget
        title="Project Overview"
        copy={projectOverview}
        absence={DASHBOARD_ABSENCE.noOverview}
      />
    );
  }
  const snippet = snippetOf(page.body);
  return (
    <Widget title="Project Overview" copy={projectOverview}>
      {snippet === "" ? (
        <p className="muted">A project overview is available in the wiki.</p>
      ) : (
        <p className={styles.snippet}>{snippet}</p>
      )}
      <DetailLink href="/wiki" label="Open wiki" />
    </Widget>
  );
}
