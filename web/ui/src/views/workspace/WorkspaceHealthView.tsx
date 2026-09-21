/*
 * Workspace Health (S-428, CR-137, FR-UI-36) — an `app`-scoped view answering
 * *"is this workspace's picture of itself current, and which members are not
 * answering?"*
 *
 * Two reads, both unscoped: `workspace/status` (per-member freshness, the warm
 * and degraded roll-ups, the topic inventory) and `workspace/check` (the
 * governance report S-427 put on the HTTP surface).
 *
 * Three honesty rules shape every card below, and each one is an untruth this
 * product has already had to remove from another surface:
 *   - The degraded roll-up is the VERDICT and it carries its denominator — "2 of
 *     3 members answered", in words, badge text included, never colour alone
 *     (FR-WS-16, NFR-CC-04).
 *   - A member that could not be opened is a row, badged and named, with its
 *     reason; its freshness reads "not read" rather than a fabricated zero.
 *   - A workspace declaring no rules gets NO report — `governance` is `null`, and
 *     rendering that as a passing check is exactly what S-437 removed from the
 *     session readout and S-438 from the Dashboard card (ADR-56).
 *
 * It computes nothing: every roll-up here is the server's own projection of the
 * member rows beside it, so the two can never disagree (ADR-01, NFR-MA-02). In
 * particular there is no aggregate of per-member quality signals anywhere on it
 * (BR-56) — the member table reports each member's own figure, named.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { AsyncResource, useApiResource } from "../../api/index.ts";
import {
  fetchWorkspaceGovernance,
  fetchWorkspaceStatus,
} from "../../api/workspaceClient.ts";
import type {
  MemberStatus,
  MemberTopics,
  TopicSummary,
  WorkspaceGovernanceAnswer,
  WorkspaceStatus,
  WorkspaceViolation,
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
import { freshnessStatement, resolutionStatement } from "../dashboard/dashboardModel.ts";
import styles from "./Workspace.module.css";

/** What a cell says when the figure behind it was never read — a member whose
 *  store would not open has no freshness, and `0` there would be a fabrication
 *  (FR-EH-04, NFR-CC-04). */
const NOT_READ = "not read";

export function WorkspaceHealthView() {
  const { mode, workspace, members, error } = useWorkspace();
  const status = useApiResource<WorkspaceStatus>(() => fetchWorkspaceStatus(), []);
  const governance = useApiResource<WorkspaceGovernanceAnswer>(
    () => fetchWorkspaceGovernance(),
    [],
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

      <AsyncResource resource={status} loadingLabel="Reading every member…">
        {(model) => (
          <>
            <AnsweringCard status={model} />
            <MemberTable status={model} />
            <WarmCard status={model} />
            <AsyncResource resource={governance} loadingLabel="Reading the workspace rules…">
              {(answer) => <GovernanceCard answer={answer} />}
            </AsyncResource>
            <TopicsCard status={model} />
          </>
        )}
      </AsyncResource>
    </div>
  );
}

// ── The verdict (FR-WS-16) ───────────────────────────────────────────────────

/** The degraded roll-up as the view's verdict, in words and with its denominator.
 *
 *  `opened` of `members` is the figure a reader acts on, so it is a sentence
 *  rather than a bar, and the badge beside it carries the state as TEXT — colour
 *  is never the only signal (NFR-CC-04). The members that failed are NAMED: a
 *  count tells a reader that the workspace degraded but not where. */
function AnsweringCard({ status }: { status: WorkspaceStatus }) {
  const rollup = status.degraded_rollup;
  const failed = rollup.degraded_members;
  const complete = failed.length === 0;
  return (
    <Card
      title="Members answering"
      aside={
        <Badge tone={complete ? "green" : "red"}>
          {complete ? "complete" : "incomplete — a partial fan-out"}
        </Badge>
      }
    >
      <p>
        <strong>
          {rollup.opened} of {rollup.members} members answered
        </strong>
        {rollup.not_attempted > 0 && (
          <>
            {" "}
            · {rollup.not_attempted} not attempted (nothing needed{" "}
            {rollup.not_attempted === 1 ? "it" : "them"} — outside this answer's scope, not a
            failure)
          </>
        )}
        .
      </p>
      {failed.length > 0 && (
        <p className="muted">
          {failed.length} member{failed.length === 1 ? "" : "s"} could not be opened:{" "}
          <span className="mono">{failed.join(", ")}</span>. Every other figure on this page —
          the warm roll-up, the topic inventory — is therefore computed over fewer than all
          members and is a lower bound (FR-WS-16).
        </p>
      )}
    </Card>
  );
}

// ── Per-member freshness, warm state and open state (FR-WS-15, FR-WS-16) ─────

/** One member's row. Freshness is humanised against the clock at render time by
 *  the SAME helper the per-member Dashboard uses (`freshnessStatement`), so the
 *  two surfaces cannot word one age two ways. */
interface MemberRow {
  member: string;
  freshness: string | null;
  warmState: string;
  openState: string;
  degraded: boolean;
  reason: string | null;
  /** This member's OWN reference-resolution coverage as the composed never-bare
   *  line ("40.0% (400 of 1000 refs)"), never averaged (BR-56) and never bare
   *  (CR-111). `null` when the member was not read. */
  resolution: string | null;
}

/** The warm state in words — index PRESENCE, a different axis from openability.
 *  `deferred` is honest and non-alarming: the member indexes lazily on first
 *  query (FR-IX-07), and collapsing it into `degraded` would make an un-indexed
 *  member indistinguishable from one nothing can open (FR-WS-15). */
const WARM_STATE_WORDS: Record<string, string> = {
  warm: "warm — indexed",
  warming: "warming — indexing now",
  deferred: "deferred — indexes on first query",
  degraded: "degraded — indexing failed",
};

/** The open state in words — store openability. */
const OPEN_STATE_WORDS: Record<string, string> = {
  opened: "opened",
  "not-attempted": "not attempted",
  degraded: "degraded — could not be opened",
};

function memberRow(member: MemberStatus, nowUnix: number): MemberRow {
  return {
    member: member.member,
    // `null`, not a placeholder age: a member with no result has no freshness to
    // state, and inventing one is the fabrication NFR-CC-04 forbids.
    freshness: member.result ? freshnessStatement(member.result, nowUnix) : null,
    warmState: WARM_STATE_WORDS[member.warm_state] ?? member.warm_state,
    openState: OPEN_STATE_WORDS[member.open_state] ?? member.open_state,
    degraded: member.open_state === "degraded" || member.warm_state === "degraded",
    // The classified cause's sentence when the diagnostic identified one, the
    // verbatim diagnostic when it did not, and `error` for a member that opened
    // and failed a later walk. Additive, never a replacement.
    reason: member.degraded_reason ?? member.reason ?? member.error ?? null,
    resolution: member.result ? resolutionStatement(member.result) : null,
  };
}

const MEMBER_COLUMNS: Column<MemberRow>[] = [
  { key: "member", header: "Member", mono: true, cell: (r) => r.member, sortValue: (r) => r.member },
  {
    key: "freshness",
    header: "Index freshness",
    cell: (r) => (r.freshness === null ? <span className="muted">{NOT_READ}</span> : r.freshness),
    sortValue: (r) => r.freshness ?? "",
  },
  {
    key: "warm",
    header: "Warm state",
    cell: (r) => r.warmState,
    sortValue: (r) => r.warmState,
  },
  {
    key: "open",
    header: "Open state",
    cell: (r) => (
      <>
        <Badge tone={r.degraded ? "red" : "green"}>{r.openState}</Badge>
        {r.reason && (
          <>
            <br />
            <span className="muted">{r.reason}</span>
          </>
        )}
      </>
    ),
    sortValue: (r) => r.openState,
  },
  {
    key: "resolution",
    header: "Its reference resolution",
    cell: (r) => r.resolution ?? <span className="muted">{NOT_READ}</span>,
    sortValue: (r) => r.resolution ?? "",
  },
];

function MemberTable({ status }: { status: WorkspaceStatus }) {
  const nowUnix = Math.floor(Date.now() / 1000);
  const rows = status.members.map((m) => memberRow(m, nowUnix));
  return (
    <Card title="Members">
      <DataTable
        caption="Per-member index freshness, warm state and open state"
        columns={MEMBER_COLUMNS}
        rows={rows}
        rowKey={(r) => r.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
      <p className="muted">
        Each figure is that member's own. There is no workspace-wide roll-up of them here: the
        per-repository quality signal is defined against one repository's baseline, so a mean
        across members would have no referent (BR-56).
      </p>
    </Card>
  );
}

// ── The warm roll-up (FR-WS-15) ──────────────────────────────────────────────

/** The warm roll-up, with the absent `warming` count named rather than zeroed.
 *
 *  Deriving `warming` needs a live signal from the bounded warm supervisor and
 *  NFR-CC-04 forbids inferring one, so the server OMITS the key. An absent key
 *  means "not knowable", never "none" — and a `0` here would tell a reader
 *  nothing is being indexed right now, which the payload does not say. */
function WarmCard({ status }: { status: WorkspaceStatus }) {
  const warm = status.warm_rollup;
  const partial = !status.degraded_rollup.covers_all_members;
  return (
    <Card title="Warm state across the workspace">
      <p>
        {warm.warm} of {warm.members} members are warm · {warm.deferred} deferred ·{" "}
        {warm.degraded} degraded ·{" "}
        {warm.warming === undefined
          ? "members indexing right now: not knowable (no live signal source)"
          : `${warm.warming} indexing right now`}
        .
      </p>
      {partial && (
        <p className="muted">
          Computed over fewer than all members — a member that could not be opened contributes no
          warm state, so these counts are a lower bound (FR-WS-16).
        </p>
      )}
    </Card>
  );
}

// ── Workspace governance (FR-WS-13, ADR-56) ──────────────────────────────────

const VIOLATION_COLUMNS: Column<WorkspaceViolation>[] = [
  { key: "rule", header: "Rule", mono: true, cell: (v) => v.rule, sortValue: (v) => v.rule },
  {
    key: "from",
    header: "Consumer",
    mono: true,
    cell: (v) => v.from.member,
    sortValue: (v) => v.from.member,
  },
  {
    key: "to",
    header: "Provider",
    mono: true,
    cell: (v) => v.to.member,
    sortValue: (v) => v.to.member,
  },
  { key: "relation", header: "Binding", cell: (v) => v.relation, sortValue: (v) => v.relation },
  { key: "message", header: "Finding", cell: (v) => v.message, sortValue: (v) => v.message },
];

/** The workspace rule findings — reported, never gating.
 *
 *  A violation's `severity` is `"error"` because checked-in policy is a real
 *  breach; the FAMILY is still advisory and can move neither an exit code nor any
 *  member's gated signal (ADR-56). The label says so on the surface, because a
 *  finding a reader takes for a gate verdict is the same untruth as a gate
 *  verdict a reader takes for advice. */
function GovernanceCard({ answer }: { answer: WorkspaceGovernanceAnswer }) {
  const report = answer.governance;
  return (
    <Card
      title="Workspace rules"
      aside={<Badge tone="muted">advisory — moves no exit code and no member's signal</Badge>}
    >
      {report === null ? (
        // The honest empty. NOT a passing report: nothing was checked, and a
        // green verdict over an unchecked workspace is the defect S-437 and
        // S-438 removed from the two surfaces that had it (ADR-56, NFR-CC-04).
        <EmptyState message="No workspace rules are declared, so nothing was checked — this is not a pass. Declare [[governance.boundaries]] or [[governance.no_cross_service_callers]] in logos.workspace.toml to have cross-service bindings checked." />
      ) : (
        <>
          <p className="muted">
            {report.rules_checked} rule{report.rules_checked === 1 ? "" : "s"} evaluated over{" "}
            {report.bindings_checked} bindings
            {report.bindings_checked === 0 &&
              " — nothing was bound to check, so a clean result here says nothing about this workspace"}
            .
          </p>
          {report.unknown_member_refs && report.unknown_member_refs.length > 0 && (
            <p className="muted">
              {report.unknown_member_refs.length} rule reference
              {report.unknown_member_refs.length === 1 ? "" : "s"} name a member this workspace
              does not have, so the rule was silently narrowed and can never match:{" "}
              <span className="mono">{report.unknown_member_refs.join(", ")}</span>.
            </p>
          )}
          {report.violations.length === 0 ? (
            <p>No rule was breached by the {report.bindings_checked} bindings checked.</p>
          ) : (
            <DataTable
              caption="Workspace rule findings"
              columns={VIOLATION_COLUMNS}
              rows={report.violations}
              rowKey={(v) => `${v.rule}:${v.from.symbol}:${v.to.symbol}`}
              pageSize={DEFAULT_TABLE_PAGE_SIZE}
            />
          )}
        </>
      )}
      {!answer.complete && (
        <p className="muted">
          This answer is incomplete: {answer.degraded_rollup.degraded_members.length} member
          {answer.degraded_rollup.degraded_members.length === 1 ? "" : "s"} could not be opened (
          <span className="mono">{answer.degraded_rollup.degraded_members.join(", ")}</span>), so
          a rule quantified over their bindings was quantified over fewer than all of them.
        </p>
      )}
    </Card>
  );
}

// ── The promoted topic inventory (FR-WS-11) ──────────────────────────────────

/** One topic row, repo-qualified — the identity two members meet on (FR-WS-03). */
interface TopicRow extends TopicSummary {
  member: string;
}

const TOPIC_COLUMNS: Column<TopicRow>[] = [
  { key: "member", header: "Member", mono: true, cell: (t) => t.member, sortValue: (t) => t.member },
  { key: "topic", header: "Topic", mono: true, cell: (t) => t.topic, sortValue: (t) => t.topic },
  {
    // Declarations, not call sites: a method publishing one topic twice is one
    // producer, exactly as the graph records it.
    key: "producers",
    header: "Producers",
    numeric: true,
    cell: (t) => t.producers,
    sortValue: (t) => t.producers,
  },
  {
    key: "consumers",
    header: "Consumers",
    numeric: true,
    cell: (t) => t.consumers,
    sortValue: (t) => t.consumers,
  },
];

function topicRows(topics: MemberTopics[]): TopicRow[] {
  return topics.flatMap((m) => m.topics.map((t) => ({ ...t, member: m.member })));
}

function TopicsCard({ status }: { status: WorkspaceStatus }) {
  const rows = topicRows(status.topics ?? []);
  const partial = !status.degraded_rollup.covers_all_members;
  return (
    <Card title="Promoted broker topics">
      {rows.length === 0 ? (
        <EmptyState message="No member has promoted a broker topic — honest absence, not a resolution failure. A topic appears here as soon as one member publishes or subscribes to it, before any cross-repo match." />
      ) : (
        <DataTable
          caption="Promoted broker topics by member"
          columns={TOPIC_COLUMNS}
          rows={rows}
          rowKey={(t) => `${t.member}:${t.topic}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      )}
      {partial && (
        <p className="muted">
          Read over fewer than all members — a member that could not be opened contributes no
          topics, so this inventory is a lower bound (FR-WS-16).
        </p>
      )}
    </Card>
  );
}
