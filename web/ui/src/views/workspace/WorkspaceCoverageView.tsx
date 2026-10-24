/*
 * Workspace coverage (S-740, CR-234, FR-CV-16; frontend-design §4.25) — an
 * `app`-scoped view answering *"which members are measured, how well, and which
 * has nothing measured at all?"*
 *
 * One unscoped read, `workspace/coverage`. Three honesty rules shape it:
 *   - A member with nothing ingested reads "never run", never 0 % (BR-77).
 *   - A member that could not be opened or read is a row, named, with its
 *     reason, and the figure row says it is over fewer than all members
 *     (FR-WS-16, NFR-CC-04).
 *   - The one workspace figure is a count of members with its denominator; no
 *     percentage is summed or averaged across members (BR-56).
 *
 * It computes nothing: every figure is the server's own, from the member's own
 * Coverage page read (NFR-MA-02). Each member links to its own Coverage page,
 * which is where a run is started under that member's consent — this page has
 * no Run button (§4.25).
 */

// The types come through the `api/index.ts` barrel, not `api/types.ts` itself,
// whose fan-in sits at the architecture rule's limit (`max_fan_in`).
import {
  AsyncResource,
  useApiResource,
  type MemberCoverage,
  type MemberResult,
  type WorkspaceCoverageAnswer,
} from "../../api/index.ts";
import { fetchWorkspaceCoverage } from "../../api/workspaceClient.ts";
import {
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  FigureNote,
  LoadingState,
  ScoreBar,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  memberCoverage,
  WORKSPACE_COVERAGE_ABSENCE,
  WORKSPACE_COVERAGE_TEXT,
} from "../../copy/workspaceCoverage.copy.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { urlWithMember } from "../../workspace/scope.ts";
import { humanizeAge, pctBp } from "../dashboard/dashboardModel.ts";
import { memberColumn } from "./memberSecurityRow.tsx";
import styles from "./Workspace.module.css";

type Row = MemberResult<MemberCoverage>;

export interface WorkspaceCoverageViewProps {
  /** The "now" the Last run ages are read against, in Unix seconds; a test pins it. */
  nowUnix?: number;
}

export function WorkspaceCoverageView({ nowUnix }: WorkspaceCoverageViewProps = {}) {
  const { mode, workspace, members, error } = useWorkspace();
  const answer = useApiResource<WorkspaceCoverageAnswer>(() => fetchWorkspaceCoverage(), []);

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
    <WidgetStack>
      <Callout label="Workspace coverage" tone="signal">
        <span>
          <span className="mono">{workspace}</span> · {WORKSPACE_COVERAGE_TEXT.heading(members.length)}
        </span>
      </Callout>
      <AsyncResource resource={answer} loadingLabel="Reading every member's coverage…">
        {(model) => <MembersWidget answer={model} nowUnix={nowUnix ?? Math.floor(Date.now() / 1000)} />}
      </AsyncResource>
    </WidgetStack>
  );
}

/** The member-scoped Coverage page for `member`: its route with the member in
 *  `?repo=`, the one way a URL names a member (S-426). */
export function memberCoverageHref(member: string): string {
  return urlWithMember("/coverage", member);
}

/** A figure cell of a member with no row to read it from. */
function notRead() {
  return <span className="muted">{WORKSPACE_COVERAGE_TEXT.notRead}</span>;
}

/** The Coverage cell: the member's own percentage as a score bar, or the state
 *  in words when there is none to show — never a 0 % bar. */
function coverageCell(row: Row) {
  const c = row.result;
  if (!c) return notRead();
  if (c.coverage === "never_run") return <span className="muted">{WORKSPACE_COVERAGE_TEXT.neverRun}</span>;
  if (c.coverage === "unmatched") return <span className="muted">{WORKSPACE_COVERAGE_TEXT.unmatched}</span>;
  if (c.coverage === "stale") return <span className="muted">{WORKSPACE_COVERAGE_TEXT.stale}</span>;
  if (c.overall_coverage_bp === null) return <span className="muted">{WORKSPACE_COVERAGE_TEXT.noCoveredLine}</span>;
  // The bar's label is only its fallback text: the figure is printed beside it,
  // as every score bar shows its raw value alongside (frontend-design §5).
  const pct = pctBp(c.overall_coverage_bp);
  return (
    <span className={styles.scoreCell}>
      <ScoreBar value={c.overall_coverage_bp} max={10_000} label={pct} />
      <span className="mono num">{pct}</span>
    </span>
  );
}

function columns(nowUnix: number): Column<Row>[] {
  return [
    memberColumn<MemberCoverage>(memberCoverageHref),
    {
      key: "coverage",
      header: "Coverage",
      cell: coverageCell,
      // A member with no percentage sorts below every member that has one.
      sortValue: (row) => row.result?.overall_coverage_bp ?? -1,
    },
    {
      key: "fresh",
      header: "Fresh files",
      cell: (row) =>
        row.result ? WORKSPACE_COVERAGE_TEXT.freshFiles(row.result.fresh_files, row.result.total_files) : notRead(),
      sortValue: (row) => row.result?.fresh_files ?? -1,
    },
    {
      key: "tools",
      header: "Tools",
      cell: (row) =>
        row.result
          ? WORKSPACE_COVERAGE_TEXT.tools(row.result.tools_ran, row.result.tools_failed, row.result.tools_applicable)
          : notRead(),
      sortValue: (row) => row.result?.tools_ran ?? -1,
    },
    {
      key: "last-run",
      header: "Last run",
      cell: (row) => {
        if (!row.result) return notRead();
        const at = row.result.last_run;
        return at === null ? <span className="muted">{WORKSPACE_COVERAGE_TEXT.noRun}</span> : humanizeAge(nowUnix, at);
      },
      sortValue: (row) => row.result?.last_run ?? -1,
    },
  ];
}

function MembersWidget({ answer, nowUnix }: { answer: WorkspaceCoverageAnswer; nowUnix: number }) {
  const c = answer.coverage;
  const unread = c.members.filter((m) => !m.result).map((m) => m.member);
  const common = { title: "Members", copy: memberCoverage } as const;
  const evidence = (
    <>
      {unread.length > 0 && (
        <p className="muted">
          {WORKSPACE_COVERAGE_TEXT.unread(unread.length)} <span className="mono">{unread.join(", ")}</span>.
        </p>
      )}
      <DataTable
        caption="Per-member line coverage, fresh files, tool outcomes and last run"
        columns={columns(nowUnix)}
        rows={c.members}
        rowKey={(row) => row.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
      <p className="muted">{WORKSPACE_COVERAGE_TEXT.noRunHere}</p>
    </>
  );
  if (c.members_read === 0) {
    return (
      <Widget {...common} absence={WORKSPACE_COVERAGE_ABSENCE.noneRead}>
        {evidence}
      </Widget>
    );
  }
  return (
    <Widget
      {...common}
      figure={
        <div className={styles.figure}>
          <p>{WORKSPACE_COVERAGE_TEXT.denominator(c.members_fresh, c.members_total, c.members_never_run)}</p>
          <FigureNote block>{WORKSPACE_COVERAGE_TEXT.perMember}</FigureNote>
        </div>
      }
    >
      {evidence}
    </Widget>
  );
}
