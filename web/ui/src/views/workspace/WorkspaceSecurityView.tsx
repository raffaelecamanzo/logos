/*
 * Workspace security (S-674, CR-217, FR-SC-16; frontend-design §4.24) — an
 * `app`-scoped view answering *"where across the workspace are the security
 * findings, and which members has nothing looked at?"*
 *
 * One unscoped read, `workspace/security`. Three honesty rules shape it:
 *   - A member never scanned reads "never run" in every figure cell, never 0
 *     (BR-66); one whose reports are all stale still counts its findings, each
 *     by its own freshness (S-760), and the figure row states how many reports
 *     are fresh as a field of its own.
 *   - A member that could not be opened or read is a row, named, with its
 *     reason, and the totals say they are over fewer than all members
 *     (FR-WS-16, NFR-CC-04).
 *   - Every total counts only fresh, unsuppressed findings, says so, and states
 *     what it left out and the members it was summed over.
 *
 * It computes nothing: every total is the server's own sum of the member rows
 * beside it (NFR-MA-02). Each member links to its own Security page, which is
 * where a scan is started — this page has no scan button (§4.24).
 */

import { useState } from "react";

import { AsyncResource, useApiResource } from "../../api/index.ts";
import { fetchWorkspaceSecurity } from "../../api/workspaceClient.ts";
import type {
  MemberResult,
  MemberSecurity,
  OwaspEditionYear,
  SecuritySeverity,
  WorkspaceSecurityAnswer,
  WorkspaceSecurityCategory,
} from "../../api/types.ts";
import {
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ErrorPanel,
  FigureNote,
  LoadingState,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  memberSecurity,
  WORKSPACE_SECURITY_ABSENCE,
  WORKSPACE_SECURITY_TEXT,
  workspaceOwasp,
} from "../../copy/workspaceSecurity.copy.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { OWASP_TEXT } from "../../copy/security.copy.ts";
import { EditionSwitch } from "../security/EditionSwitch.tsx";
import { EDITIONS } from "../security/securityModel.ts";
import { absentFigure, memberColumn } from "./memberSecurityRow.tsx";
import styles from "./Workspace.module.css";

export function WorkspaceSecurityView() {
  const { mode, workspace, members, error } = useWorkspace();
  const answer = useApiResource<WorkspaceSecurityAnswer>(() => fetchWorkspaceSecurity(), []);

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
      <Callout label="Workspace security" tone="signal">
        <span>
          <span className="mono">{workspace}</span> · {WORKSPACE_SECURITY_TEXT.heading(members.length)}
        </span>
      </Callout>
      <AsyncResource resource={answer} loadingLabel="Reading every member's security…">
        {(model) => (
          <>
            <MembersWidget answer={model} />
            <OwaspWidget answer={model} />
          </>
        )}
      </AsyncResource>
    </WidgetStack>
  );
}

// ── The member roll-up ───────────────────────────────────────────────────────

/** The severities a member row shows, in the order the columns read — every one
 *  the server counts, `info` and `unrated` included, so a member whose only
 *  findings are `info` or `unrated` never reads as a row of zeros. */
const SEVERITIES: readonly SecuritySeverity[] = ["critical", "high", "medium", "low", "info", "unrated"];

function severityColumn(severity: SecuritySeverity): Column<MemberResult<MemberSecurity>> {
  return {
    key: severity,
    header: severity[0].toUpperCase() + severity.slice(1),
    cell: (row) => {
      const counts = row.result?.findings;
      return counts ? counts[severity] : <span className="muted">{absentFigure(row)}</span>;
    },
    // A member with nothing to count sorts below every member that counted 0.
    sortValue: (row) => row.result?.findings?.[severity] ?? -1,
  };
}

const MEMBER_COLUMNS: Column<MemberResult<MemberSecurity>>[] = [
  memberColumn<MemberSecurity>(),
  {
    key: "scanners",
    header: "Scanners fresh",
    cell: (row) =>
      row.result ? (
        WORKSPACE_SECURITY_TEXT.scannersFresh(row.result.scanners_fresh, row.result.scanners_enabled)
      ) : (
        <span className="muted">{WORKSPACE_SECURITY_TEXT.notRead}</span>
      ),
    sortValue: (row) => row.result?.scanners_fresh ?? -1,
  },
  ...SEVERITIES.map(severityColumn),
];

function MembersWidget({ answer }: { answer: WorkspaceSecurityAnswer }) {
  const s = answer.security;
  const unread = s.members.filter((m) => !m.result).map((m) => m.member);
  const common = { title: "Members", copy: memberSecurity } as const;
  const evidence = (
    <>
      {unread.length > 0 && (
        <p className="muted">
          {WORKSPACE_SECURITY_TEXT.unread(unread.length)} <span className="mono">{unread.join(", ")}</span>.
        </p>
      )}
      <DataTable
        caption="Per-member scanner freshness and fresh, unsuppressed findings"
        columns={MEMBER_COLUMNS}
        rows={s.members}
        rowKey={(row) => row.member}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
      <p className="muted">{WORKSPACE_SECURITY_TEXT.noScanHere}</p>
    </>
  );
  if (s.members_read === 0) {
    return (
      <Widget {...common} absence={WORKSPACE_SECURITY_ABSENCE.noneRead}>
        {evidence}
      </Widget>
    );
  }
  if (s.members_counted === 0) {
    return (
      <Widget {...common} absence={WORKSPACE_SECURITY_ABSENCE.noneScanned}>
        {evidence}
      </Widget>
    );
  }
  const t = s.totals;
  return (
    <Widget
      {...common}
      figure={
        <div className={styles.figure}>
          <p>{WORKSPACE_SECURITY_TEXT.totals(t.critical, t.high, t.medium, t.low, t.info, t.unrated)}</p>
          <FigureNote block>
            {WORKSPACE_SECURITY_TEXT.denominator(s.members_counted, s.members_total, s.members_never_scanned)}
          </FigureNote>
          <FigureNote block>{WORKSPACE_SECURITY_TEXT.reportsFresh(s.reports_fresh, s.reports_stored)}</FigureNote>
          <FigureNote block>{WORKSPACE_SECURITY_TEXT.excluded(s.excluded.not_fresh, s.excluded.suppressed)}</FigureNote>
          <FigureNote block>{WORKSPACE_SECURITY_TEXT.testCode(s.excluded.test)}</FigureNote>
          <FigureNote block>{WORKSPACE_SECURITY_TEXT.devOnly(s.excluded.dev)}</FigureNote>
          <FigureNote block>{WORKSPACE_SECURITY_TEXT.needsReview(s.excluded.needs_review)}</FigureNote>
        </div>
      }
    >
      {evidence}
    </Widget>
  );
}

// ── The OWASP roll-up ────────────────────────────────────────────────────────

function OwaspWidget({ answer }: { answer: WorkspaceSecurityAnswer }) {
  const s = answer.security;
  const [edition, setEdition] = useState<OwaspEditionYear>(EDITIONS[0]);
  const shown = s.owasp.find((e) => e.edition === edition);
  const categories = shown?.categories ?? [];
  const unclassified = shown?.unclassified ?? 0;
  const columns: Column<WorkspaceSecurityCategory>[] = [
    { key: "id", header: "Category", mono: true, cell: (c) => c.id, sortValue: (c) => c.id },
    { key: "name", header: "Name", cell: (c) => c.name, sortValue: (c) => c.name },
    {
      key: "findings",
      header: "Findings",
      cell: (c) => c.findings ?? <span className="muted">{WORKSPACE_SECURITY_TEXT.notCovered}</span>,
      sortValue: (c) => c.findings ?? -1,
    },
    {
      key: "covered",
      header: "Coverage",
      cell: (c) => WORKSPACE_SECURITY_TEXT.coveredIn(c.covered_in, s.members_read),
      sortValue: (c) => c.covered_in,
    },
  ];
  if (s.owasp.length === 0) {
    return <Widget title="OWASP Top 10" copy={workspaceOwasp} absence={WORKSPACE_SECURITY_ABSENCE.noneRead} />;
  }
  return (
    <Widget
      title="OWASP Top 10"
      copy={workspaceOwasp}
      figure={
        <div className={styles.figure}>
          <p>{OWASP_TEXT.edition(edition)}</p>
          <FigureNote block>{WORKSPACE_SECURITY_TEXT.owaspBasis(s.reports_fresh, s.reports_stored)}</FigureNote>
          {unclassified > 0 && <FigureNote block>{OWASP_TEXT.unclassified(unclassified)}</FigureNote>}
        </div>
      }
    >
      <EditionSwitch edition={edition} onChange={setEdition} />
      <DataTable
        caption={OWASP_TEXT.edition(edition)}
        columns={columns}
        rows={categories}
        rowKey={(c) => c.id}
      />
    </Widget>
  );
}
