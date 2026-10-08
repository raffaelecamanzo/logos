/*
 * WorkspaceStatisticsView (S-429, CR-137, FR-UI-37, NFR-CC-04) — the `app`-scoped
 * Statistics view over `GET /api/v1/workspace/statistics`.
 *
 * It answers *"how is Logos used across this whole workspace, and what did that
 * return?"* — usage over time, top tools, the value estimate and the dev-vs-`main`
 * split, each summed over the members whose telemetry could actually be read.
 *
 * Four properties shape everything below, and each one is an untruth this project
 * has already had to remove from another surface:
 *
 *   - **Every total states its denominator.** `members_read` of `members_total`
 *     travels with each figure, in words, through ONE component
 *     ({@link DenominatorNote}) so the sentence cannot be worded two ways. A sum
 *     over an unstated population is what NFR-CC-04 forbids, and the server
 *     publishes `covers_all_members` precisely so the marker and the figures
 *     cannot be read apart.
 *   - **Every unread member is NAMED, with its reason, apart.** `absent`,
 *     `locked` and `unreadable` call for different reactions — an absent store is
 *     the normal state of a member nobody has run Logos in and is not an incident
 *     — so they are rendered as three different sentences, never as one
 *     "unavailable" (FR-UI-37, NFR-RA-05).
 *   - **The awaiting-data state is the member-scoped view's own predicate.**
 *     `isStatsEmpty` from `statsModel.ts`, keyed on `calls_total === 0`, shared
 *     rather than re-derived. It is NOT `members_read === 0`: a workspace whose
 *     members all have migrated but eventless stores reads every one of them and
 *     still has nothing to show, and rendering zeros there is exactly the
 *     "zeros read as measurements" failure.
 *   - **No quality-signal element anywhere.** This aggregates usage, not quality:
 *     the per-repository signal is defined against one repository's baseline, so a
 *     workspace-wide roll-up of it would have no referent ([BR-56], ADR-56).
 *
 * It computes no figure the server did not send. The derivations it does apply —
 * the tool ranking, the daily series, the origin split, the chart options — are the
 * member-scoped view's own pure model layer (`statsModel.ts`), called over the
 * aggregate rather than re-implemented for it: the arithmetic has one spelling in
 * the tree, and a hand-mirrored second copy of "top tools ranked by calls" is the
 * defect that shape invites.
 *
 * The member-scoped view (FR-UI-27) is untouched by this file — not its component,
 * not its stylesheet, not its `Engine::stats` pass-through. This view renders
 * through `StatisticsView.module.css` deliberately: reusing that stylesheet leaves
 * it byte-identical, whereas restating its layout here would either duplicate it or
 * edit it (and editing it rotates the served class names of a view this story
 * promises to leave alone). The two S-428 views share `Workspace.module.css` the
 * same way.
 *
 * `app`-scoped (FR-UI-35, ADR-66): declared in `nav.ts`, so the shell does not
 * remount it on a member switch — and the read below carries no member in its
 * dependency array, so nothing re-fetches when one changes. Unreachable and
 * unrendered in single-root mode.
 *
 * S-617 (CR-203, FR-UI-39/40): every figure surface is a `Widget` with its
 * entry in `copy/workspaceStatistics.copy.ts`, in one `WidgetStack` under the
 * population callout. The awaiting-data state is the Estimated value widget's
 * stated absence (the member-scoped view's S-616 shape), not a centred empty
 * state, since the view still holds widgets.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { useMemo, useState } from "react";
import type { ChangeEvent } from "react";

import {
  DEFAULT_STATISTICS_WINDOW,
  STATISTICS_WINDOWS,
  type StatisticsWindow,
} from "../../api/statisticsClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import { fetchWorkspaceStatistics } from "../../api/workspaceClient.ts";
import type { UnreadMember, UnreadReason, WorkspaceStatistics } from "../../api/types.ts";
import {
  Badge,
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  LoadingState,
  SelectField,
  Widget,
  WidgetStack,
  type BadgeTone,
  type Column,
} from "../../components/index.ts";
import {
  membersNotSummed,
  WORKSPACE_STATISTICS_ABSENCE,
  workspaceDevVsMain,
  workspaceEstimatedValue,
  workspaceTopTools,
  workspaceUsageOverTime,
} from "../../copy/workspaceStatistics.copy.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { StatChart } from "./StatChart.tsx";
import {
  activityLineOption,
  activitySeries,
  isStatsEmpty,
  originBarOption,
  originSplit,
  rankedBarOption,
  topTools,
  type ActivityPoint,
  type OriginRow,
  type ToolRow,
} from "./statsModel.ts";
import { fmtInt } from "../dashboard/dashboardModel.ts";
import styles from "./StatisticsView.module.css";

/** A right-aligned mono numeric column: one accessor drives both the formatted cell
 *  and the sort key, so they cannot drift apart. */
function numCol<R>(key: string, header: string, get: (r: R) => number): Column<R> {
  return { key, header, cell: (r) => fmtInt(get(r)), numeric: true, mono: true, sortValue: get };
}

/** A mono identifier/text column with a string sort key. */
function textCol<R>(key: string, header: string, get: (r: R) => string): Column<R> {
  return { key, header, cell: (r) => get(r), mono: true, sortValue: get };
}

// ── What "unread" means, and the plural of a count ──────────────────────────────

/** `s` unless `n` is 1 — one spelling for every pluralised count on this page. */
function s(n: number): string {
  return n === 1 ? "" : "s";
}

/**
 * `unread` holds two populations that call for OPPOSITE statements, and conflating
 * them is the defect this function exists to prevent (review findings A3-F1/A3-F2).
 *
 * A member whose store is **absent** was read successfully: the server determined
 * it has no `telemetry.db` at all (`Ok(None)` — `federation/telemetry.rs`), so it
 * recorded nothing, nothing failed, and the totals here are EXACT with respect to
 * it rather than a lower bound. That is also the dominant state of a young or large
 * workspace — on the reference estate nearly every member is absent — so a caveat
 * keyed on `covers_all_members` alone would sit on this page permanently saying
 * something untrue.
 *
 * A member that is **locked** or **unreadable** is a genuine read FAILURE: its
 * usage in this window is unknown. Every total here is then a lower bound by an
 * unknown amount, and the awaiting-data state must not claim that nothing was
 * recorded anywhere — `calls_total` is the server's sum over the members it COULD
 * read, so a zero says nothing at all about these.
 *
 * An UNRECOGNISED token — a reason arm the server grows against a shipped bundle —
 * falls in with the failures. That is the safe direction: it produces a caveat that
 * may be unnecessary, rather than suppressing one that is needed. The comparison is
 * exact equality against the one token that means "read successfully, no store",
 * never a substring or a truthiness test.
 */
function splitUnread(agg: WorkspaceStatistics): {
  failed: UnreadMember[];
  absent: UnreadMember[];
} {
  return {
    failed: agg.unread.filter((u) => u.reason !== "absent"),
    absent: agg.unread.filter((u) => u.reason === "absent"),
  };
}

// ── The denominator, in one place (FR-UI-37, NFR-CC-04) ─────────────────────────

/**
 * The population sentence that accompanies every total on this page.
 *
 * One component rather than a sentence per card, because the duty is that each
 * figure states the same denominator — and three hand-written copies of "summed
 * over N of M members" is how two of them end up saying different things. It also
 * names the consequence when the aggregate is partial: a sum over fewer members
 * than the roster has is a **lower bound**, not a measurement of the workspace.
 */
function DenominatorNote({ agg }: { agg: WorkspaceStatistics }) {
  const { failed, absent } = splitUnread(agg);
  return (
    <p className={styles.capNote}>
      Summed over{" "}
      <strong>
        {agg.members_read} of {agg.members_total}
      </strong>{" "}
      workspace member{s(agg.members_total)}
      {/* A lower bound is a claim about a sum, so it is made ONLY when a read
          actually failed. `covers_all_members` is false for an absent store too,
          and calling the figures a lower bound there would be false — nothing was
          recorded, so nothing is missing. */}
      {failed.length > 0
        ? ` — a lower bound: ${failed.length} member${s(failed.length)} could not be read, so ${
            failed.length === 1 ? "its" : "their"
          } usage in this window is unknown.`
        : absent.length > 0
          ? ` — the other ${absent.length} ${
              absent.length === 1 ? "has" : "have"
            } no telemetry store yet and recorded nothing, so no recorded usage is missing.`
          : " — the whole roster."}
    </p>
  );
}

// ── Window selector ─────────────────────────────────────────────────────────────

/** The 7 / 30 / 90-day control (default 7) driving `?window=`. The window list and
 *  its default are the member-scoped client's own constants, so the two scopes
 *  cannot offer different windows. */
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
        hint="Trailing days of telemetry to summarise, across every member."
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

// ── The population callout (the verdict-first statement) ────────────────────────

/** What this page is a sum over, stated before any figure on it — the workspace,
 *  the denominator, and the scope in words so the section label is not the only
 *  thing saying which level this answers for (ADR-66, NFR-CC-04). */
function PopulationCallout({ agg }: { agg: WorkspaceStatistics }) {
  const { failed, absent } = splitUnread(agg);
  // Whether any figure is on the page at all. The understated-figures claim is
  // about figures, so it is not made when the awaiting-data state has replaced
  // them — a page with no sum on it cannot be understating one.
  const figuresShown = !isStatsEmpty(agg);
  return (
    <Callout label="Population" tone="signal">
      <p className={styles.valueLead}>
        <span className="mono">{agg.workspace}</span> · this view answers for the whole workspace,
        not for the selected member.
      </p>
      <DenominatorNote agg={agg} />
      {/* Gated on the population each sentence is ABOUT, not on
          `covers_all_members`, which is true of both and so cannot word either.
          Both gates are also the predicate for the table that fulfils the promise
          these sentences make ("named below"), so the promise and the names cannot
          come apart. */}
      {failed.length > 0 && (
        <p className={styles.valueNote}>
          {failed.length} member{s(failed.length)} could not be read
          {figuresShown
            ? `, so every figure on this page is lower than the workspace's true usage by whatever ${
                failed.length === 1 ? "it" : "they"
              } recorded`
            : `, so nothing here is known about ${
                failed.length === 1 ? "its" : "their"
              } usage in this window`}
          . {failed.length === 1 ? "It is" : "They are"} named below, with{" "}
          {failed.length === 1 ? "its reason" : "their reasons"}.
        </p>
      )}
      {absent.length > 0 && (
        <p className={styles.valueNote}>
          {absent.length} member{s(absent.length)} ha{absent.length === 1 ? "s" : "ve"} no telemetry
          store yet — nobody has run Logos there, so {absent.length === 1 ? "it" : "they"} recorded
          nothing. That is the ordinary state of a new service rather than a fault, and it takes
          nothing away from the figures above. {absent.length === 1 ? "It is" : "They are"} named
          below.
        </p>
      )}
    </Callout>
  );
}

// ── Value estimate ─────────────────────────────────────────────────────────────

/** The headline value widget: the window's estimated reads/tokens saved by
 *  navigation summed across the read members, honestly labeled as an estimate
 *  in its why (NFR-CC-04, NFR-OO-03).
 *
 *  No latency percentiles, unlike its member-scoped twin. A summed p95 is a
 *  fabricated figure rather than a coarser one, so the endpoint does not carry the
 *  percentiles and this view must not fall back to the per-member shape for them. */
function ValueWidget({ agg }: { agg: WorkspaceStatistics }) {
  return (
    <Widget
      title="Estimated value"
      copy={workspaceEstimatedValue}
      figure={
        <p className={styles.valueLead}>
          <strong className={styles.valueBig}>{fmtInt(agg.tokens_saved_estimate)}</strong> tokens and{" "}
          <strong className={styles.valueBig}>{fmtInt(agg.reads_saved_estimate)}</strong> ad-hoc file
          reads estimated saved by navigation across this workspace over the last {agg.window_days}{" "}
          days.
        </p>
      }
    >
      <dl className={styles.glance}>
        <div>
          <dt>Calls</dt>
          <dd className="mono num">{fmtInt(agg.calls_total)}</dd>
        </div>
        {/* The denominator is NOT restated here as a second `N / M` figure: it is
            stated once per surface, by the one component that owns its wording
            (review finding A5-F4 — one figure in two wordings, four lines apart, is
            the shape `DenominatorNote` exists to prevent). */}
      </dl>
      <DenominatorNote agg={agg} />
    </Widget>
  );
}

// ── Surface widgets ─────────────────────────────────────────────────────────────

/** Usage over time — the daily-activity line + its accessible data-table twin.
 *  The series is the per-day sum across members, so a day on which two members
 *  each recorded once is one point reading two. */
function ActivityCard({ points, agg }: { points: ActivityPoint[]; agg: WorkspaceStatistics }) {
  // Memoise the option so `setOption` re-fires only when the data changes, not on
  // every ancestor re-render (which would replay the entry animation).
  const option = useMemo(() => activityLineOption(points), [points]);
  if (points.length === 0) {
    return (
      <Widget title="Usage over time" copy={workspaceUsageOverTime} absence={WORKSPACE_STATISTICS_ABSENCE.activity}>
        <DenominatorNote agg={agg} />
      </Widget>
    );
  }
  return (
    <Widget title="Usage over time" copy={workspaceUsageOverTime}>
      <StatChart
        option={option}
        label={`Daily calls across the workspace over the last ${points.length} recorded day${points.length === 1 ? "" : "s"} (the table below carries the same data)`}
      />
      <DataTable<ActivityPoint>
        columns={[
          textCol("day", "Day", (r) => r.day),
          numCol("calls", "Calls", (r) => r.calls),
          numCol("ok", "OK", (r) => r.ok_calls),
        ]}
        rows={points}
        rowKey={(r) => r.day}
        caption="Daily activity across the workspace"
        pageSize={15}
      />
      <DenominatorNote agg={agg} />
    </Widget>
  );
}

/** Top tools — one ranked bar over the calls each tool took across every member and
 *  surface, with its accessible twin. */
function ToolsCard({
  tools,
  truncated,
  agg,
}: {
  tools: ToolRow[];
  truncated: boolean;
  agg: WorkspaceStatistics;
}) {
  const option = useMemo(
    () => rankedBarOption(tools.map((t) => t.tool), tools.map((t) => t.calls)),
    [tools],
  );
  if (tools.length === 0) {
    return (
      <Widget title="Top tools" copy={workspaceTopTools} absence={WORKSPACE_STATISTICS_ABSENCE.tools}>
        <DenominatorNote agg={agg} />
      </Widget>
    );
  }
  return (
    <Widget title="Top tools" copy={workspaceTopTools}>
      <StatChart
        option={option}
        label="Most-used tools across the workspace, ranked by calls (the table below carries the same data)"
      />
      {truncated && (
        <p className={styles.capNote}>
          Showing the top {tools.length} tools; lower-ranked tools are not charted.
        </p>
      )}
      <DataTable<ToolRow>
        columns={[
          textCol("tool", "Tool", (r) => r.tool),
          numCol("calls", "Calls", (r) => r.calls),
        ]}
        rows={tools}
        rowKey={(r) => r.tool}
        caption="Top tools across the workspace"
      />
      <DenominatorNote agg={agg} />
    </Widget>
  );
}

/** Dev vs main — the origin split bar + its accessible data-table twin. */
function OriginCard({ origins, agg }: { origins: OriginRow[]; agg: WorkspaceStatistics }) {
  const option = useMemo(() => originBarOption(origins), [origins]);
  if (origins.length === 0) {
    return (
      <Widget title="Dev vs main" copy={workspaceDevVsMain} absence={WORKSPACE_STATISTICS_ABSENCE.origins}>
        <DenominatorNote agg={agg} />
      </Widget>
    );
  }
  return (
    <Widget title="Dev vs main" copy={workspaceDevVsMain}>
      <StatChart
        option={option}
        label="Calls by event origin across the workspace — development branches combined versus main (the table below carries the same data)"
      />
      <DataTable<OriginRow>
        columns={[
          textCol("origin", "Origin", (r) => r.origin),
          numCol("calls", "Calls", (r) => r.calls),
          numCol("ok", "OK", (r) => r.ok_calls),
        ]}
        rows={origins}
        rowKey={(r) => r.origin}
        caption="Usage by origin across the workspace"
      />
      <DenominatorNote agg={agg} />
    </Widget>
  );
}

// ── The unread members (FR-UI-37, NFR-RA-05) ───────────────────────────────────

/** The three reasons in words, each saying what it means and what to do about it.
 *
 *  Kept apart rather than collapsed to "unavailable", because collapsing them
 *  reports the first as an incident and the last as routine — the state a reader
 *  acts on differently is the state the surface must distinguish. */
const UNREAD_REASON_WORDS: Readonly<Record<UnreadReason, string>> = {
  absent: "no telemetry store yet — nobody has run Logos in this member. Not a fault.",
  locked: "store busy — a writer held it past the read's timeout. Transient; try again.",
  unreadable: "store could not be read — corrupt, permission-denied, or not a database.",
};

/** The badge tone per reason. Colour is never the only signal — the badge text
 *  carries the reason token and the cell beside it carries the sentence — but an
 *  absent store must not be inked like a fault.
 *
 *  The `?? "muted"` at the call site is the one fallback here that earns its keep: a
 *  token this build does not know must not be inked as a fault on the strength of
 *  not being recognised. There is deliberately no matching fallback for the WORDS —
 *  an unknown token is NAMED by the badge and explained by the server's own
 *  `detail`, and a fallback there only repeated the token in the next breath. */
const UNREAD_REASON_TONES: Readonly<Record<UnreadReason, BadgeTone>> = {
  absent: "muted",
  locked: "orange",
  unreadable: "red",
};

/**
 * The two lookups, read through accessors that tell the truth about their result.
 *
 * The maps stay keyed on the closed {@link UnreadReason} union, so a token added to
 * the wire type without words and a tone fails `tsc -b` — that exhaustiveness is
 * worth keeping. But at the READ the value is genuinely `| undefined`: the union is
 * closed only in this bundle, and a server that grows a fourth arm hands it
 * straight through. Without the widening the checker believes the opposite of the
 * runtime — it types the tone lookup as total, which makes the load-bearing
 * fallback below look like dead code a future reader may "simplify" away, while
 * typing the words lookup as a `string` when that is the one that actually yields
 * `undefined` (review finding A3-F4).
 *
 * The asymmetry between them is deliberate and proven: the tone fallback changes
 * rendered output (without it `Badge` composes no tone class at all), so an
 * unrecognised token must not be inked as a fault merely for being unrecognised.
 * A words fallback could only repeat the token the badge already carries — React
 * renders `undefined` as nothing, not as the string — and the server's own `detail`
 * in the next column is the sentence for that row.
 */
function toneFor(reason: UnreadReason): BadgeTone {
  const tone: BadgeTone | undefined = UNREAD_REASON_TONES[reason];
  return tone ?? "muted";
}

function wordsFor(reason: UnreadReason): string | undefined {
  const words: string | undefined = UNREAD_REASON_WORDS[reason];
  return words;
}

const UNREAD_COLUMNS: Column<UnreadMember>[] = [
  {
    key: "member",
    header: "Member",
    mono: true,
    cell: (u) => u.member,
    sortValue: (u) => u.member,
  },
  {
    key: "reason",
    header: "Reason",
    cell: (u) => (
      <>
        <Badge tone={toneFor(u.reason)}>{u.reason}</Badge> {wordsFor(u.reason)}
      </>
    ),
    sortValue: (u) => u.reason,
  },
  {
    key: "detail",
    header: "Diagnostic",
    cell: (u) => <span className="muted">{u.detail}</span>,
    sortValue: (u) => u.detail,
  },
];

/** Every member that contributed nothing, NAMED with its reason — so a reader can
 *  see which part of the workspace is missing from the figures above rather than
 *  only that some of it is (FR-UI-37). */
function UnreadCard({ agg }: { agg: WorkspaceStatistics }) {
  return (
    <Widget
      title="Members not summed"
      badge={
        <Badge tone="muted">
          {agg.unread.length} of {agg.members_total} contributed nothing
        </Badge>
      }
      copy={membersNotSummed}
    >
      <DataTable
        // Not "could not be read": an absent store WAS read, successfully, and found
        // to be absent. The caption has to cover both populations in the table, and
        // what they share is that neither contributed to the figures above.
        caption="Members that contributed nothing to the figures above, each with the reason"
        columns={UNREAD_COLUMNS}
        rows={agg.unread}
        rowKey={(u) => u.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Widget>
  );
}

// ── Body / empty ───────────────────────────────────────────────────────────────

/** The three charted surfaces plus the value estimate. The derivations are memoised
 *  on the aggregate so each card receives a stable array reference — the option
 *  `useMemo`s downstream then re-fire only on an actual window re-query. */
function Surfaces({ agg }: { agg: WorkspaceStatistics }) {
  const points = useMemo(() => activitySeries(agg), [agg]);
  const { rows: tools, truncated } = useMemo(() => topTools(agg), [agg]);
  const origins = useMemo(() => originSplit(agg), [agg]);
  return (
    <>
      <ValueWidget agg={agg} />
      <ActivityCard points={points} agg={agg} />
      <ToolsCard tools={tools} truncated={truncated} agg={agg} />
      <OriginCard origins={origins} agg={agg} />
    </>
  );
}

/**
 * The honest awaiting-data state (NFR-CC-04): nothing was recorded, so the
 * Estimated value widget states that absence rather than render a grid of zeros
 * — zeros here would read as a measured "nobody uses Logos", which is not what
 * an empty store says.
 *
 * It takes the aggregate because the sentence it is entitled to say depends on WHY
 * the sum is zero (review finding A3-F1). `calls_total` is the server's sum over
 * the members it could READ, so with a locked or unreadable member in the roster a
 * zero is not evidence that nothing was recorded anywhere — and `logos stats` is
 * then the wrong remedy, because the blocker is the store rather than a shortage of
 * usage: that branch names the store instead (CR-206).
 */
function AwaitingData({ agg }: { agg: WorkspaceStatistics }) {
  const { failed } = splitUnread(agg);
  const absence =
    failed.length === 0 ? (
      // Every member either has a store that recorded nothing, or has no store at
      // all. Both mean nothing was recorded, so the universal claim is earned.
      WORKSPACE_STATISTICS_ABSENCE.nothingRecorded
    ) : (
      <>
        {agg.members_read > 0 &&
          `None of the ${agg.members_read} member${s(agg.members_read)} whose telemetry could be read recorded anything in this window. `}
        {failed.length} member{s(failed.length)} could not be read at all, so this view knows
        nothing about {failed.length === 1 ? "its" : "their"} usage —{" "}
        {failed.length === 1 ? "it is" : "they are"} named below, with the reason.{" "}
        {WORKSPACE_STATISTICS_ABSENCE.storeRepair}
      </>
    );
  return (
    <Widget
      title="Estimated value"
      copy={workspaceEstimatedValue}
      absence={absence}
    />
  );
}

// ── The view ───────────────────────────────────────────────────────────────────

export function WorkspaceStatisticsView() {
  const { mode, error } = useWorkspace();
  const [window, setWindow] = useState<StatisticsWindow>(DEFAULT_STATISTICS_WINDOW);
  // `[window]` is the WHOLE dependency array, and that is the app-scoped contract
  // (FR-UI-35, ADR-66): the aggregate is the same answer for every member, so a
  // member switch must not re-issue it. The shell also declines to remount this
  // view on a switch, through the `scope: "app"` declaration in `nav.ts` — two
  // halves of one property, neither sufficient alone.
  const stats = useApiResource<WorkspaceStatistics>(
    () => fetchWorkspaceStatistics(window),
    [window],
  );

  if (mode === "loading") {
    return (
      <div className={styles.view}>
        <LoadingState label="Reading the workspace…" />
      </div>
    );
  }

  if (error) {
    return (
      <div className={styles.view}>
        <ErrorPanel>The workspace could not be read: {error.message}</ErrorPanel>
      </div>
    );
  }

  if (mode !== "workspace") {
    return (
      <div className={styles.view}>
        <EmptyState message="Not a workspace — this serve has a single repository root. The per-service Statistics view answers for it; start Logos at a directory with a logos.workspace.toml to federate members." />
      </div>
    );
  }

  return (
    <div className={styles.view}>
      <header className={styles.head}>
        <div>
          <h1 className={styles.title}>Statistics</h1>
          {/* "Summed over N of M" is {@link DenominatorNote}'s sentence and ONLY its
              sentence — the lead deliberately words the same idea differently, so
              that phrase identifies a denominator statement wherever it appears
              (which is how the spec counts that every total carries one). */}
          <p className={styles.lead}>
            How Logos is used across this workspace, and the value it returns — aggregated across
            the members whose telemetry could be read.
          </p>
        </div>
        <WindowSelector value={window} onChange={setWindow} />
      </header>

      {/* `AsyncResource`'s own `isEmpty`/`empty` slots are deliberately NOT used
          here. They replace the whole body, and this view owes two statements that
          survive an empty aggregate: the population it summed over, and the name of
          every member it could not read. A workspace where nothing has telemetry is
          exactly the case where all three members are `absent` — hiding their names
          behind the empty state would drop the criterion FR-UI-37 states
          unconditionally. So only the FIGURE surfaces are gated on the predicate. */}
      <AsyncResource resource={stats} loadingLabel="Reading every member's telemetry…">
        {(agg) => (
          <WidgetStack>
            <PopulationCallout agg={agg} />
            {isStatsEmpty(agg) ? <AwaitingData agg={agg} /> : <Surfaces agg={agg} />}
            {agg.unread.length > 0 && <UnreadCard agg={agg} />}
          </WidgetStack>
        )}
      </AsyncResource>
    </div>
  );
}
