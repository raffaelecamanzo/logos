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
 * headline is the server's composed `resolved_edges_summary` line so the count
 * can never be shown without its rate (BR-51).
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
  Badge,
  Callout,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  LoadingState,
  type Column,
} from "../../components/index.ts";
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

  return (
    <div className={styles.view}>
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
    </div>
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
          tab's coverage panel — headline, spec conformance, intake, arms — so the
          two surfaces cannot disagree about a figure (S-428 AC1). */}
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

/** The promotions and the bounds the payload was projected under.
 *
 *  Every claim on this view is rendered next to `reachability.coverage` — the
 *  rider the claim is only as good as — because a promotion read apart from the
 *  coverage it rests on is the misreading BR-53 exists to remove. */
function ReachabilityCard({ answer }: { answer: WorkspaceReachabilityAnswer }) {
  const { reachability } = answer;
  const rider = reachability.coverage;
  const promotions = reachability.live_via_cross_service;
  // The shortfall predicate, derived from the two counts the rider carries.
  // This rider has NO `covers_all_members` flag (unlike the coverage summary and
  // the degraded roll-up), so reading one would be reading `undefined` — falsy —
  // and would stamp "lower bound" on a complete answer too (S-428 review).
  const partial = rider.members_read < rider.members_total;
  return (
    <Card
      title="Cross-service reachability"
      aside={<Badge tone="muted">advisory — never a gate input</Badge>}
    >
      <p className="muted">
        {promotions.length} callable{promotions.length === 1 ? "" : "s"} dead in its own member and
        live across the workspace union. Seeded from {rider.bridge_invocation_edges} invocation
        edge{rider.bridge_invocation_edges === 1 ? "" : "s"}; the coverage this rests on is{" "}
        {rider.members_read} of {rider.members_total} members read
        {partial ? " — every figure here is a lower bound" : ""}.
      </p>
      {/* The applied bounds, stated. `dead: null` is SUPPRESSED, deliberately
          distinct from `[]` ("computed, and genuinely empty") — so it is rendered
          as "not requested", never as "no dead code" (CR-084, NFR-CC-04). */}
      <p className="muted">
        {reachability.scope.repo ? (
          <>
            Scoped to <span className="mono">{reachability.scope.repo}</span>.{" "}
          </>
        ) : (
          "Unscoped — every member is projected. "
        )}
        {reachability.dead === null
          ? "The app-wide dead set was not requested, so it is absent from this answer rather than empty."
          : `${reachability.dead.length} callables are dead app-wide.`}
      </p>
      {reachability.skipped_members.length > 0 && (
        <p className="muted">
          {reachability.skipped_members.length} member
          {reachability.skipped_members.length === 1 ? "" : "s"} could not be read, so{" "}
          {reachability.skipped_members.length === 1 ? "it contributes" : "they contribute"} no
          roots and no claims:{" "}
          <span className="mono">{reachability.skipped_members.join(", ")}</span>.
        </p>
      )}
      {promotions.length === 0 ? (
        <EmptyState message="No callable is promoted across the workspace union — either nothing is dead per repo, or no invocation edge reaches one. Read the coverage above before taking this as an all-clear." />
      ) : (
        <DataTable
          caption="Callables promoted to live by a cross-service edge"
          columns={PROMOTION_COLUMNS}
          rows={promotions}
          rowKey={(c) => `${c.member}:${c.symbol}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
    </Card>
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

const ROSTER_COLUMNS: Column<RosterRow>[] = [
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
    header: "Its reference resolution",
    cell: (r) => r.resolution ?? <span className="muted">{NOT_READ}</span>,
    sortValue: (r) => r.resolution ?? "",
  },
  {
    key: "extraRoots",
    header: "Roots from bindings",
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.extra_roots),
    sortValue: (r) => r.tally?.extra_roots ?? -1,
  },
  {
    key: "deadPerRepo",
    header: "Dead per repo",
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.dead_per_repo),
    sortValue: (r) => r.tally?.dead_per_repo ?? -1,
  },
  {
    key: "promoted",
    header: "Live via cross-service",
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.live_via_cross_service),
    sortValue: (r) => r.tally?.live_via_cross_service ?? -1,
  },
  {
    key: "deadAppWide",
    header: "Dead app-wide",
    numeric: true,
    cell: (r) => tallyCell(r, (t) => t.dead_app_wide),
    sortValue: (r) => r.tally?.dead_app_wide ?? -1,
  },
];

function MemberRoster({
  status,
  answer,
}: {
  status: WorkspaceStatus;
  answer: WorkspaceReachabilityAnswer;
}) {
  const rows = rosterRows(status, answer);
  return (
    <Card title="Members">
      <DataTable
        caption="Each member's own coupling figures — never rolled up"
        columns={ROSTER_COLUMNS}
        rows={rows}
        rowKey={(r) => r.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
      <p className="muted">
        Each row is that member's own figure. There is deliberately no workspace-wide total or
        average here: the per-repository quality signal is defined against one repository's
        baseline, so a mean across members would be a number with no referent (BR-56).
      </p>
    </Card>
  );
}
