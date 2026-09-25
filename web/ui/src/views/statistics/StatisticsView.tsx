/*
 * StatisticsView (S-235, S-306, CR-058, CR-091, FR-UI-27, FR-OB-11, FR-UI-23,
 * NFR-CC-04) — the in-app usage view over `GET /api/v1/statistics` (S-234). The
 * last read surface; it sits immediately above Config in the sidebar's last group.
 *
 * It leads with a value-estimate callout (the dogfood metric, NFR-OO-03), then a
 * daily-activity line, a top-tools & surfaces ranking, a dev-vs-`main` origin
 * split, and the tool × origin cross-tab grouped by tool class (FR-OB-11) — every
 * chart surface paired with an accessible data-table twin (WCAG 2.1 AA); the
 * cross-tab is table-only (it has no ECharts twin to pair). A 7 / 30 / 90-day
 * window selector (default 7) drives `?window=`; changing it re-queries and
 * re-renders every surface via the shared `useApiResource` cache key.
 *
 * Honesty (NFR-CC-04): an empty telemetry store degrades to an awaiting-data empty
 * state — never fabricated zeros — and the sidebar nav item is muted in step (see
 * `useStatisticsAvailability`). The value figures are labeled estimates, never
 * measured truth. The cross-tab's coverage limits — raw-events-only, the legacy
 * `NULL`-origin caveat, and the CLI+MCP-only label for the period that predates
 * it — render beside the figures from the read-model's own `attribution_coverage`
 * (FR-OB-11), not a separate help page. The answered/classified pair on each cell
 * (FR-OB-14) renders as "N of M answered" or the read-model's own named absence,
 * never a rate. Every read is GET-only; viewing the tab mutates no store and adds
 * no external origin (self-only CSP unchanged, UAT-UI-02) — and, since `stats` is
 * itself a self-referential read-model request (FR-OB-09), opening the tab does
 * not change the navigation counts it displays.
 */

import { useMemo, useState } from "react";
import type { ChangeEvent } from "react";

import {
  DEFAULT_STATISTICS_WINDOW,
  STATISTICS_WINDOWS,
  fetchStatistics,
  type StatisticsWindow,
} from "../../api/statisticsClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type { StatsInfo } from "../../api/types.ts";
import { Callout, Card, DataTable, EmptyState, SelectField } from "../../components/index.ts";
import type { Column } from "../../components/index.ts";
import { StatChart } from "./StatChart.tsx";
import {
  activityLineOption,
  activitySeries,
  attributionByClass,
  bySurface,
  isStatsEmpty,
  originBarOption,
  originSplit,
  rankedBarOption,
  topTools,
  type ActivityPoint,
  type AttributionRow,
  type OriginRow,
  type SurfaceRow,
  type ToolRow,
} from "./statsModel.ts";
import styles from "./StatisticsView.module.css";

/** Group digits for display; the read-model counts are plain integers. */
function num(n: number): string {
  return n.toLocaleString("en-US");
}

/** A right-aligned mono numeric column: one accessor drives both the formatted cell
 *  and the sort key, so they cannot drift apart. */
function numCol<R>(key: string, header: string, get: (r: R) => number): Column<R> {
  return { key, header, cell: (r) => num(get(r)), numeric: true, mono: true, sortValue: get };
}

/** A mono identifier/text column with a string sort key. */
function textCol<R>(key: string, header: string, get: (r: R) => string): Column<R> {
  return { key, header, cell: (r) => get(r), mono: true, sortValue: get };
}

// ── Window selector ─────────────────────────────────────────────────────────────

/** The 7 / 30 / 90-day control (default 7) driving `?window=` (FR-UI-27). A
 *  design-system select — changing it re-keys the resource fetch. */
function WindowSelector({
  value,
  onChange,
}: {
  value: StatisticsWindow;
  onChange: (w: StatisticsWindow) => void;
}) {
  const handle = (e: ChangeEvent<HTMLSelectElement>) =>
    onChange(Number(e.target.value) as StatisticsWindow);
  return (
    <div className={styles.window}>
      <SelectField
        label="Window"
        hint="Trailing days of telemetry to summarise."
        value={String(value)}
        onChange={handle}
      >
        {STATISTICS_WINDOWS.map((w) => (
          <option key={w} value={w}>
            Last {w} days
          </option>
        ))}
      </SelectField>
    </div>
  );
}

// ── Value estimate (the verdict-first callout) ──────────────────────────────────

/** The headline value callout (frontend-design §4.15): the window's estimated
 *  reads/tokens saved by navigation, honestly labeled as an estimate (NFR-CC-04,
 *  NFR-OO-03), with the calls/latency context beneath. */
function ValueCallout({ stats }: { stats: StatsInfo }) {
  return (
    <Callout label="Estimated value" tone="signal">
      <p className={styles.valueLead}>
        <strong className={styles.valueBig}>{num(stats.tokens_saved_estimate)}</strong> tokens and{" "}
        <strong className={styles.valueBig}>{num(stats.reads_saved_estimate)}</strong> ad-hoc file
        reads estimated saved by navigation over the last {stats.window_days} days.
      </p>
      <p className={styles.valueNote}>
        An <em>estimate</em> — reads avoided by structural navigation, valued at the ratified
        net-tokens-per-read constant. Not a measured figure.
      </p>
      <dl className={styles.glance}>
        <div>
          <dt>Calls</dt>
          <dd className="mono num">{num(stats.calls_total)}</dd>
        </div>
        <div>
          <dt>Latency p50 / p95 / p99</dt>
          <dd className="mono num">
            {num(stats.latency_p50_ms)} / {num(stats.latency_p95_ms)} / {num(stats.latency_p99_ms)} ms
          </dd>
        </div>
      </dl>
    </Callout>
  );
}

// ── Surface cards ───────────────────────────────────────────────────────────────

/** Usage over time — the daily-activity line + its accessible data-table twin. */
function ActivityCard({ points }: { points: ActivityPoint[] }) {
  // Memoise the option so `setOption` re-fires only when the data changes, not on
  // every ancestor re-render (which would replay the entry animation).
  const option = useMemo(() => activityLineOption(points), [points]);
  return (
    <Card title="Usage over time">
      {points.length === 0 ? (
        <EmptyState message="No activity in this window." />
      ) : (
        <>
          <StatChart
            option={option}
            label={`Daily calls over the last ${points.length} recorded day${points.length === 1 ? "" : "s"} (the table below carries the same data)`}
          />
          <DataTable<ActivityPoint>
            columns={[
              textCol("day", "Day", (r) => r.day),
              numCol("calls", "Calls", (r) => r.calls),
              numCol("ok", "OK", (r) => r.ok_calls),
            ]}
            rows={points}
            rowKey={(r) => r.day}
            caption="Daily activity"
            pageSize={15}
          />
        </>
      )}
    </Card>
  );
}

/** Top tools & surfaces — two ranked bars (by tool, by surface), each with a twin. */
function ToolsCard({
  tools,
  truncated,
  surfaces,
}: {
  tools: ToolRow[];
  truncated: boolean;
  surfaces: SurfaceRow[];
}) {
  const toolsOption = useMemo(
    () => rankedBarOption(tools.map((t) => t.tool), tools.map((t) => t.calls)),
    [tools],
  );
  const surfaceOption = useMemo(
    () => rankedBarOption(surfaces.map((s) => s.surface), surfaces.map((s) => s.calls)),
    [surfaces],
  );
  return (
    <Card title="Top tools & surfaces">
      <h4 className={styles.subhead}>Most-used tools</h4>
      {tools.length === 0 ? (
        <EmptyState message="No tool calls in this window." />
      ) : (
        <>
          <StatChart
            option={toolsOption}
            label="Most-used tools ranked by calls (the table below carries the same data)"
          />
          {truncated && (
            <p className={styles.capNote}>
              Showing the top {tools.length} tools; lower-ranked tools are not charted.
            </p>
          )}
          <DataTable<ToolRow>
            columns={[textCol("tool", "Tool", (r) => r.tool), numCol("calls", "Calls", (r) => r.calls)]}
            rows={tools}
            rowKey={(r) => r.tool}
            caption="Top tools"
          />
        </>
      )}

      <h4 className={styles.subhead}>By surface</h4>
      <p className={styles.capNote}>Dashboard (web) activity is excluded — it reflects viewing, not tool use.</p>
      {surfaces.length === 0 ? (
        <EmptyState message="No surface usage in this window." />
      ) : (
        <>
          <StatChart
            option={surfaceOption}
            label="Usage by surface (cli / mcp / watcher); the table below carries the same data"
          />
          <DataTable<SurfaceRow>
            columns={[textCol("surface", "Surface", (r) => r.surface), numCol("calls", "Calls", (r) => r.calls)]}
            rows={surfaces}
            rowKey={(r) => r.surface}
            caption="Usage by surface"
          />
        </>
      )}
    </Card>
  );
}

/** Dev vs main — the origin split bar + its accessible data-table twin. */
function OriginCard({ origins }: { origins: OriginRow[] }) {
  const option = useMemo(() => originBarOption(origins), [origins]);
  return (
    <Card title="Dev vs main">
      {origins.length === 0 ? (
        <EmptyState message="No attributed usage in this window." />
      ) : (
        <>
          <p className={styles.capNote}>
            Usage during development increments (all worktree branches combined, warm) versus{" "}
            <code>main</code> (neutral). Rolled-up days carry no origin, so this split can sum to
            less than total calls.
          </p>
          <StatChart
            option={option}
            label="Calls by event origin — development branches combined versus main (the table below carries the same data)"
          />
          <DataTable<OriginRow>
            columns={[
              textCol("origin", "Origin", (r) => r.origin),
              numCol("calls", "Calls", (r) => r.calls),
              numCol("ok", "OK", (r) => r.ok_calls),
            ]}
            rows={origins}
            rowKey={(r) => r.origin}
            caption="Usage by origin"
          />
        </>
      )}
    </Card>
  );
}

/** Tool attribution by class (FR-OB-11) — the tool × origin cross-tab grouped by
 *  tool class, with its coverage limits rendered beside the figures rather than
 *  in a separate help page (NFR-CC-04): raw-events-only, the legacy-`NULL`-origin
 *  caveat, and the CLI+MCP-only label for the period that predates it — the
 *  read-model's own `attribution_coverage.notes`, rendered verbatim so the tab
 *  and `logos stats --json` can never disagree. */
function AttributionCard({ stats }: { stats: StatsInfo }) {
  const groups = useMemo(() => attributionByClass(stats), [stats]);
  const { notes } = stats.attribution_coverage;
  return (
    <Card title="Tool attribution by class">
      {notes.map((note) => (
        <p key={note} className={styles.capNote}>
          {note}
        </p>
      ))}
      {groups.length === 0 ? (
        <EmptyState message="No attributed tool calls in this window." />
      ) : (
        groups.map((group) => (
          <div key={group.class}>
            <h4 className={styles.subhead}>{group.class}</h4>
            <DataTable<AttributionRow>
              columns={[
                textCol("tool", "Tool", (r) => r.tool),
                textCol("origin", "Origin", (r) => r.origin),
                numCol("calls", "Calls", (r) => r.calls),
                numCol("ok", "OK", (r) => r.ok_calls),
                textCol("answered", "Answered", (r) => r.answered),
              ]}
              rows={group.rows}
              rowKey={(r) => `${r.tool}:${r.origin}`}
              caption={`${group.class} tools by origin`}
            />
          </div>
        ))
      )}
    </Card>
  );
}

/** The five surfaces over a non-empty read-model. The derivations are memoised on
 *  `stats` so each card receives a stable array reference — the option `useMemo`s
 *  downstream then re-fire only on an actual window re-query, not on every render. */
function StatisticsBody({ stats }: { stats: StatsInfo }) {
  const points = useMemo(() => activitySeries(stats), [stats]);
  const { rows: tools, truncated } = useMemo(() => topTools(stats), [stats]);
  const surfaces = useMemo(() => bySurface(stats), [stats]);
  const origins = useMemo(() => originSplit(stats), [stats]);
  return (
    <div className={styles.surfaces}>
      <ValueCallout stats={stats} />
      <ActivityCard points={points} />
      <ToolsCard tools={tools} truncated={truncated} surfaces={surfaces} />
      <OriginCard origins={origins} />
      <AttributionCard stats={stats} />
    </div>
  );
}

/** The honest awaiting-data state (NFR-CC-04): the store holds no events yet, so the
 *  tab names the fact plainly rather than render fabricated zeros. */
function AwaitingData() {
  return (
    <EmptyState
      message="No telemetry recorded yet — use Logos and this view will fill in. Try"
      command="logos stats"
    />
  );
}

/** The Statistics tab (FR-UI-27). The window selector persists across load/empty so
 *  a re-query never unmounts the control; the surfaces load beneath it. */
export function StatisticsView() {
  const [window, setWindow] = useState<StatisticsWindow>(DEFAULT_STATISTICS_WINDOW);
  const stats = useApiResource<StatsInfo>(() => fetchStatistics(window), [window]);

  return (
    <div className={styles.view}>
      <header className={styles.head}>
        <div>
          <h1 className={styles.title}>Statistics</h1>
          <p className={styles.lead}>How Logos is used here, and the value it returns.</p>
        </div>
        <WindowSelector value={window} onChange={setWindow} />
      </header>

      <AsyncResource
        resource={stats}
        loadingLabel="Loading usage statistics…"
        isEmpty={isStatsEmpty}
        empty={<AwaitingData />}
      >
        {(s) => <StatisticsBody stats={s} />}
      </AsyncResource>
    </div>
  );
}
