/*
 * The Workspace Dashboard (S-428, CR-137, FR-UI-36; frontend-design §4.17) — an
 * `app`-scoped view answering *"how coupled is this application, and how much of
 * that do we actually know?"*
 *
 * Two reads, both of them the unscoped fan-out: `workspace/status` (coverage and
 * the member table) and `workspace/reachability` (the app-wide union view S-427
 * put on the HTTP surface). Neither carries the shell's member scope — the view
 * answers for the WHOLE workspace, which is what `scope: "app"` declares in
 * `nav.ts` and what keeps the shell from remounting it on a member switch.
 *
 * It computes nothing. Every figure is the server's, displayed (ADR-01,
 * NFR-MA-02): the coverage boards are the SAME components the Workspace tab's
 * coverage panel renders (`CoverageBoards.tsx`), not a restyled copy, and the
 * headline's edge count rides in the server's composed `resolved_edges_summary`
 * line, so the count can never be shown without its rate (BR-51).
 *
 * Every widget renders through the shared `Widget` frame, in one `WidgetStack`,
 * with its words in a catalogue (S-613, CR-203): the coverage boards in
 * `copy/coverage.copy.ts`, Reachability and Members in
 * `copy/workspaceDashboard.copy.ts`.
 *
 * The one thing this view deliberately does NOT have is a headline number for the
 * workspace. The per-repo 0–10000 signal is defined against one repository's
 * baseline, so a mean across members is a number with no referent; BR-56 forbids
 * it by rule, and the member roster below reports each member's OWN signal,
 * named, instead.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { AsyncResource, useApiResource } from "../../api/index.ts";
import {
  fetchWorkspaceReachability,
  fetchWorkspaceStatus,
} from "../../api/workspaceClient.ts";
import type {
  MemberReachability,
  MemberStatus,
  WorkspaceReachabilityAnswer,
  WorkspaceStatus,
} from "../../api/types.ts";
import {
  ActionCell,
  Badge,
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  FigureNote,
  LoadingState,
  Term,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  DASHBOARD_TEXT,
  memberRowAction,
  members,
  reachability,
} from "../../copy/workspaceDashboard.copy.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { resolutionStatement } from "../dashboard/dashboardModel.ts";
import { buildCoverageDashboard } from "./coverageModel.ts";
import { CoveragePanel } from "./CoverageBoards.tsx";
import styles from "./Workspace.module.css";

/** What a cell says when the figure behind it was never read (FR-EH-04,
 *  NFR-CC-04). Never `0` and never `—`: a member whose store would not open has
 *  no tally, and printing a zero there would draw it as a service with no
 *  couplings — the failure mode FR-UI-36 names by name. */
const NOT_READ = "not read";

export function WorkspaceDashboardView() {
  const { mode, workspace, members, error } = useWorkspace();
  const status = useApiResource<WorkspaceStatus>(() => fetchWorkspaceStatus(), []);
  const reachability = useApiResource<WorkspaceReachabilityAnswer>(
    () => fetchWorkspaceReachability(),
    [],
  );

  // The probe has not answered yet: we do not know the mode, so we assert
  // neither. Claiming "not a workspace" here would flash a falsehood at every
  // real workspace on its way in (NFR-CC-04).
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

  // A hand-typed route in a plain repo gets the honest answer, not a broken fetch.
  if (mode !== "workspace") {
    return (
      <div className={styles.view}>
        <EmptyState message="Not a workspace — this serve has a single repository root. Start Logos at a directory with a logos.workspace.toml to federate members." />
      </div>
    );
  }

  // One stack for the whole view (S-613, FR-UI-40): the coverage boards are a
  // fragment and `AsyncResource` renders its children unwrapped, so every
  // widget below is a direct child of this stack, at its one gap.
  return (
    <WidgetStack>
      <Callout label="Workspace" tone="signal">
        <span>
          <span className="mono">{workspace}</span> · {members.length} service
          {members.length === 1 ? "" : "s"} · this view answers for the whole workspace, not for
          the selected member
        </span>
      </Callout>

      <AsyncResource resource={status} loadingLabel="Loading cross-service coverage…">
        {(model) => (
          <AsyncResource resource={reachability} loadingLabel="Loading cross-service reachability…">
            {(reach) => <DashboardContent status={model} answer={reach} />}
          </AsyncResource>
        )}
      </AsyncResource>
    </WidgetStack>
  );
}

function DashboardContent({
  status,
  answer,
}: {
  status: WorkspaceStatus;
  answer: WorkspaceReachabilityAnswer;
}) {
  return (
    <>
      {/* The coverage boards, rendered from the SAME components as the Workspace
          tab's coverage panel — headline, spec conformance, intake; the per-arm
          board is hidden on both through the hidden-widget register (S-612) — so
          the two surfaces cannot disagree about a figure (S-428 AC1). */}
      <CoveragePanel
        dashboard={buildCoverageDashboard(status.coverage)}
        degraded={status.degraded_rollup}
      />
      <ReachabilityCard answer={answer} />
      <MemberRoster status={status} answer={answer} />
    </>
  );
}

// ── App-wide reachability (FR-WS-12, served by S-427) ────────────────────────

/** The callables another service keeps alive, led by how many there are
 *  (CR-203 §3.2 D item 2), and the bounds the payload was projected under.
 *
 *  Every claim on this view is rendered next to `reachability.coverage` — the
 *  rider the claim is only as good as — because a promotion read apart from the
 *  coverage it rests on is the misreading BR-53 exists to remove. So the lead
 *  reads "at least" when the rider is partial, and an empty answer is stated in
 *  the figure row (never a centred `EmptyState`) with that same caveat. */
function ReachabilityCard({ answer }: { answer: WorkspaceReachabilityAnswer }) {
  const { reachability: model } = answer;
  const rider = model.coverage;
  const promotions = model.live_via_cross_service;
  // The shortfall predicate, derived from the two counts the rider carries.
  // This rider has NO `covers_all_members` flag (unlike the coverage summary and
  // the degraded roll-up), so reading one would be reading `undefined` — falsy —
  // and would stamp "lower bound" on a complete answer too (S-428 review).
  const partial = rider.members_read < rider.members_total;
  return (
    <Widget
      title="Cross-service reachability"
      badge={<Badge tone="muted">Advisory</Badge>}
      copy={reachability}
      figure={
        <div className={styles.figure}>
          <p>{DASHBOARD_TEXT.keepThem(promotions.length, partial)}</p>
          <FigureNote block>
            {DASHBOARD_TEXT.keepThemBasis(
              rider.bridge_invocation_edges,
              rider.members_read,
              rider.members_total,
              partial,
            )}
            {promotions.length === 0 && <> {DASHBOARD_TEXT.keepThemNone}</>}
          </FigureNote>
          {model.skipped_members.length > 0 && (
            <FigureNote block>
              {DASHBOARD_TEXT.skipped(model.skipped_members)}{" "}
              <span className="mono">{model.skipped_members.join(", ")}</span>.
            </FigureNote>
          )}
        </div>
      }
    >
      {promotions.length > 0 && (
        <DataTable
          caption="Callables kept in use by a call from another service"
          columns={PROMOTION_COLUMNS}
          rows={promotions}
          rowKey={(c) => `${c.member}:${c.symbol}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
      {/* The applied bounds, stated. `dead: null` is SUPPRESSED, deliberately
          distinct from `[]` ("computed, and genuinely empty") — so it is rendered
          as "not requested", never as "no dead code" (CR-084, NFR-CC-04). */}
      <p className="muted">
        {model.scope.repo ? (
          <>
            {DASHBOARD_TEXT.scopedTo} <span className="mono">{model.scope.repo}</span>.{" "}
          </>
        ) : (
          `${DASHBOARD_TEXT.unscoped} `
        )}
        {model.dead === null ? DASHBOARD_TEXT.deadSuppressed : DASHBOARD_TEXT.deadCount(model.dead.length)}
      </p>
    </Widget>
  );
}

const PROMOTION_COLUMNS: Column<{ member: string; name: string; kind: string }>[] = [
  { key: "member", header: "Member", mono: true, cell: (c) => c.member, sortValue: (c) => c.member },
  { key: "name", header: "Callable", mono: true, cell: (c) => c.name, sortValue: (c) => c.name },
  { key: "kind", header: "Kind", cell: (c) => c.kind, sortValue: (c) => c.kind },
];

// ── The member roster (BR-56) ────────────────────────────────────────────────

/** One member's own row. Every figure on it belongs to THAT member and is named
 *  with it; nothing here is summed, averaged or rolled up across the roster
 *  (BR-56) — a mean of a per-repository signal has no referent. */
interface RosterRow {
  member: string;
  /** The member's open state, in words. */
  state: string;
  /** `true` when the member could not be opened — the row is drawn degraded and
   *  its figures read {@link NOT_READ} rather than zero. */
  degraded: boolean;
  /** Why the open failed, when the fan-out said. */
  reason: string | null;
  /** This member's OWN reference-resolution coverage as the composed
   *  never-bare line ("40.0% (400 of 1000 refs)"), or `null` when unread. The
   *  denominator travels with the figure: two members on identical coverage over
   *  wildly different reference populations must not render identically
   *  (CR-111, FR-WS-05). */
  resolution: string | null;
  /** This member's own union-view tally, or `null` when the view skipped it. */
  tally: MemberReachability | null;
}

/** Join the two read-models by member name.
 *
 *  Driven by the STATUS roster, so a member the reachability view skipped is
 *  still a row: it is drawn degraded and named rather than omitted (FR-UI-36,
 *  NFR-RA-05). A member the reachability view knows and the status fan-out does
 *  not is appended rather than dropped, for the same reason — an answer that
 *  silently loses a member is the one thing neither read-model may do. */
function rosterRows(status: WorkspaceStatus, answer: WorkspaceReachabilityAnswer): RosterRow[] {
  const tallies = new Map(answer.reachability.members.map((m) => [m.member, m]));
  const rows = status.members.map((m) => rosterRow(m, tallies.get(m.member) ?? null));
  const seen = new Set(rows.map((r) => r.member));
  for (const tally of answer.reachability.members) {
    if (seen.has(tally.member)) continue;
    rows.push({
      member: tally.member,
      state: "not in the status fan-out",
      degraded: false,
      reason: null,
      resolution: null,
      tally,
    });
  }
  return rows;
}

function rosterRow(member: MemberStatus, tally: MemberReachability | null): RosterRow {
  const degraded = member.open_state === "degraded";
  return {
    member: member.member,
    state: OPEN_STATE_WORDS[member.open_state] ?? member.open_state,
    degraded,
    // `degraded_reason` is the classified cause's sentence when there was one and
    // the verbatim diagnostic when there was not; `error` is the fallback for a
    // member that opened and failed a later walk.
    reason: member.degraded_reason ?? member.error ?? null,
    // Absent for a member with no result — NOT defaulted to 0, which would read
    // as "nothing resolves here" (NFR-CC-04).
    resolution: member.result ? resolutionStatement(member.result) : null,
    tally,
  };
}

/** The open state in words, so the row is never colour or position alone
 *  (NFR-CC-04). */
const OPEN_STATE_WORDS: Record<string, string> = {
  opened: "opened",
  "not-attempted": "not attempted",
  degraded: "degraded — could not be opened",
};

/** A tally cell: the member's own figure, or {@link NOT_READ} when this member
 *  contributed no tally at all. */
function tallyCell(row: RosterRow, pick: (t: MemberReachability) => number) {
  if (!row.tally) return <span className="muted">{NOT_READ}</span>;
  return pick(row.tally);
}

/** The per-row action cell: what this member's own row asks of the reader. */
function RowAction({ row }: { row: RosterRow }) {
  return <ActionCell action={memberRowAction({ degraded: row.degraded, unusedAcross: row.tally?.dead_app_wide ?? null })} />;
}

/** The roster's columns. The figure headers are glossed (CR-203 §3.2 D item 3):
 *  plain words, each naming a precise figure the gloss defines.
 *
 *  Built on render, not at module load: the headers are elements, and an element
 *  created at import time would make every module importing this view (the app
 *  shell, its route table) need the gloss component just to load. */
const rosterColumns = (): Column<RosterRow>[] => [
  { key: "member", header: "Member", mono: true, cell: (r) => r.member, sortValue: (r) => r.member },
  {
    key: "state",
    header: "State",
    cell: (r) => (
      <>
        <Badge tone={r.degraded ? "red" : "green"}>{r.state}</Badge>
        {r.reason && (
          <>
            <br />
            <span className="muted">{r.reason}</span>
          </>
        )}
      </>
    ),
    sortValue: (r) => r.state,
  },
  {
    // This member's OWN signal, named with it. Never averaged across the roster
    // (BR-56): the figure is defined against one member's graph.
    key: "resolution",
    header: (
      <>
        <Term term="referenceResolution" /> (its own)
      </>
    ),
    cell: (r) => r.resolution ?? <span className="muted">{NOT_READ}</span>,
    sortValue: (r) => r.resolution ?? "",
  },
  {
    key: "extraRoots",
    header: <Term term="entryPointsFromOtherServices" />,
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.extra_roots),
    sortValue: (r) => r.tally?.extra_roots ?? -1,
  },
  {
    key: "deadPerRepo",
    header: <Term term="unusedInOwnGraph" />,
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.dead_per_repo),
    sortValue: (r) => r.tally?.dead_per_repo ?? -1,
  },
  {
    key: "promoted",
    header: (
      <>
        …of which <Term term="usedByAnotherService" />
      </>
    ),
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.live_via_cross_service),
    sortValue: (r) => r.tally?.live_via_cross_service ?? -1,
  },
  {
    key: "deadAppWide",
    header: <Term term="unusedAcrossWorkspace" />,
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.dead_app_wide),
    sortValue: (r) => r.tally?.dead_app_wide ?? -1,
  },
  { key: "action", header: "What you can do", cell: (r) => <RowAction row={r} /> },
];

function MemberRoster({
  status,
  answer,
}: {
  status: WorkspaceStatus;
  answer: WorkspaceReachabilityAnswer;
}) {
  const rows = rosterRows(status, answer);
  const rollup = status.degraded_rollup;
  return (
    <Widget
      title="Members"
      copy={members}
      figure={
        <div className={styles.figure}>
          <p>{DASHBOARD_TEXT.membersRead(rollup.opened, rollup.members)}</p>
        </div>
      }
    >
      <DataTable
        caption="Each member's own coupling figures — never rolled up"
        columns={rosterColumns()}
        rows={rows}
        rowKey={(r) => r.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
      <p className="muted">{DASHBOARD_TEXT.notAveraged}</p>
    </Widget>
  );
}
