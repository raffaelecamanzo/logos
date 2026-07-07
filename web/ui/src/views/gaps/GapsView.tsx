/*
 * GapsView (S-189, FR-UI-06, FR-UI-21) — the Gaps tab (untested surface + rule
 * findings) migrated to React over `/api/v1`. Verdict-first: the counts line leads,
 * then the blast-radius-ranked test-gap table (rendered in the read-model's order
 * verbatim — never re-sorted, preserving web/MCP/CLI parity, FR-GV-17) and the
 * rule-findings panel with its three honest states (onboarding / clean / findings).
 * Consumes the shared `/api/v1` data-access layer (the S-186 pattern) and renders
 * exclusively through the S-193 design system. Every read is GET-only — loading the
 * view mutates no store (ADR-28).
 */

import { fetchGaps } from "../../api/client.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type { GapsModel, RulesReport, TestGapsReport } from "../../api/types.ts";
import { Badge, Callout, Card, DataTable, DEFAULT_TABLE_PAGE_SIZE, EmptyState } from "../../components/index.ts";
import type { BadgeTone, Column } from "../../components/index.ts";
import styles from "./GapsView.module.css";

export function GapsView() {
  const model = useApiResource<GapsModel>(() => fetchGaps(), []);
  return (
    <AsyncResource
      resource={model}
      loadingLabel="Loading the gaps…"
      isEmpty={(m) => !m.status.indexed}
      empty={<EmptyState message="No index yet — run" command="logos index" />}
    >
      {(m) => <GapsReport model={m} />}
    </AsyncResource>
  );
}

function GapsReport({ model }: { model: GapsModel }) {
  const untested = model.test_gaps.untested.length;
  const findings = model.rules.violations.length;
  const clean = untested === 0 && findings === 0;
  return (
    <div className={styles.view}>
      <Callout label="GAPS" tone={clean ? "muted" : "signal"}>
        <span>
          {untested} untested public function(s) · {findings} rule finding(s)
        </span>
      </Callout>
      <TestGapsCard report={model.test_gaps} />
      <RulesCard report={model.rules} />
    </div>
  );
}

/** A `file:line` location, or `—` when the file is absent (mirrors `views::file_line`). */
function fileLine(file: string, line: number | null): string {
  if (!file) return "—";
  return line != null ? `${file}:${line}` : file;
}

interface GapRow {
  name: string;
  location: string;
}

/** The test-gap table, led by the mandatory static-coverage caveat (BR-16) and the
 *  covered/total figure. The `untested` order is the worklist ranking — rendered
 *  verbatim (FR-GV-17), so the table is NOT made sortable here. */
function TestGapsCard({ report }: { report: TestGapsReport }) {
  const coverage =
    report.coverage_ratio != null ? (
      <>
        <span className={styles.ratio}>{report.coverage_ratio}</span>{" "}
        <span className={styles.muted}>/ 10000</span>
      </>
    ) : (
      <Badge tone="muted">n/a</Badge>
    );

  const rows: GapRow[] = report.untested.map((g) => ({
    name: g.name,
    location: fileLine(g.file, g.line),
  }));
  // No `sortValue`: the read-model arrives blast-radius-ranked and the surface
  // renders that order verbatim (a surface re-sort would diverge from MCP/CLI).
  const columns: Column<GapRow>[] = [
    { key: "fn", header: "Function", mono: true, cell: (r) => r.name },
    { key: "loc", header: "Location", mono: true, cell: (r) => r.location },
  ];

  return (
    <Card title="Test gaps">
      <p className={styles.note}>{report.caveat}</p>
      <p className={styles.coverage}>
        Coverage {coverage} — {report.covered_functions} of {report.total_functions}{" "}
        function(s) reached from a test.
      </p>
      {report.untested.length === 0 ? (
        <p className={styles.note}>No untested public functions.</p>
      ) : (
        <>
          <p className={styles.note}>
            Ordered as a worklist — by blast radius (caller fan-in × containing-file
            hotspot rank, most-urgent first) once git history has been mined,
            otherwise by file and name.
          </p>
          <DataTable
            caption="Test gaps"
            columns={columns}
            rows={rows}
            rowKey={(r, i) => `${r.name}#${i}`}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
          {report.truncated && (
            <p className={styles.truncated}>
              Showing {report.limit} of more — raise the limit with{" "}
              <code>logos test-gaps --limit N</code>.
            </p>
          )}
        </>
      )}
    </Card>
  );
}

interface ViolationRow {
  rule: string;
  severity: string;
  location: string;
  message: string;
}

/** The severity badge tone (mirrors the legacy `violation_row`): error → red,
 *  warning → orange, anything else → muted. Colour always carries text too (a11y). */
function severityTone(severity: string): BadgeTone {
  if (severity === "error") return "red";
  if (severity === "warning") return "orange";
  return "muted";
}

/** The rule-findings panel — three honest states (NFR-CC-04): findings table,
 *  clean "No rule findings.", or the no-rules onboarding empty state. Findings are
 *  checked first so a populated report always renders its table. */
function RulesCard({ report }: { report: RulesReport }) {
  if (report.violations.length > 0) {
    const rows: ViolationRow[] = report.violations.map((v) => ({
      rule: v.rule,
      severity: v.severity,
      location: v.file || "—",
      message: v.message,
    }));
    const columns: Column<ViolationRow>[] = [
      { key: "rule", header: "Rule", mono: true, sortValue: (r) => r.rule, cell: (r) => r.rule },
      {
        key: "sev",
        header: "Severity",
        sortValue: (r) => r.severity,
        cell: (r) => <Badge tone={severityTone(r.severity)}>{r.severity}</Badge>,
      },
      { key: "loc", header: "Location", mono: true, sortValue: (r) => r.location, cell: (r) => r.location },
      { key: "msg", header: "Message", sortValue: (r) => r.message, cell: (r) => r.message },
    ];
    return (
      <Card title="Rule findings">
        <p className={styles.note}>{report.checked_rules} rule(s) checked.</p>
        <DataTable
          caption="Rule findings"
          columns={columns}
          rows={rows}
          rowKey={(r, i) => `${r.rule}#${i}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      </Card>
    );
  }
  if (!report.rules_present) {
    return (
      <Card title="Rule findings">
        <RulesOnboarding />
      </Card>
    );
  }
  return (
    <Card title="Rule findings">
      <p className={styles.note}>{report.checked_rules} rule(s) checked.</p>
      <p className={styles.note}>No rule findings.</p>
    </Card>
  );
}

const EXAMPLE_RULES = `# .logos/rules.toml
[constraints]
max_cycles = 0            # forbid dependency cycles

[[layers]]
name  = "core"
paths = ["src/core/**"]
order = 1

[[layers]]
name  = "api"
paths = ["src/api/**"]
order = 2

[[forbidden_imports]]
from   = "src/api/**"
to     = "src/db/**"
reason = "the API layer must reach the database through core"`;

/** The no-rules onboarding empty state (NFR-CC-04, frontend-design §4.6) — explain
 *  what rules buy you, name the file to author, show a runnable example, and name
 *  the evaluating command, rather than an always-empty findings table. */
function RulesOnboarding() {
  return (
    <div className={styles.onboarding}>
      <p>
        No <code>.logos/rules.toml</code> yet — architecture rules are not
        configured. Author rules to enforce layering, ban forbidden imports, and
        require tested or documented surfaces; findings then appear here with
        severity badges.
      </p>
      <p className={styles.note}>
        Create <code>.logos/rules.toml</code>, for example:
      </p>
      <pre className={styles.example}>{EXAMPLE_RULES}</pre>
      <p className={styles.note}>
        Then run <code>logos check</code> to evaluate them.
      </p>
    </div>
  );
}
