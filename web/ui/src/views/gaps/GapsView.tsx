/*
 * GapsView → "Rule findings" (S-189, CR-079, FR-UI-06, FR-UI-21) — the architecture
 * rule-findings tab over `/api/v1/gaps` ({ status, rules }). Verdict-first: the
 * findings-count line leads, then the rule-findings panel with its three honest
 * states (onboarding when no `.logos/rules.toml` is authored / clean when rules pass
 * / a findings table when they don't). CR-079 removed the test-gaps table entirely.
 * Consumes the shared `/api/v1` data-access layer (the S-186 pattern) and renders
 * exclusively through the S-193 design system. Every read is GET-only — loading the
 * view mutates no store (ADR-28).
 *
 * S-617 (CR-203, FR-UI-39/40): the panel is a `Widget` with the catalogue entry it
 * shares with the member Dashboard (`copy/ruleFindings.copy.ts`), in one
 * `WidgetStack` under the verdict callout. A contract that declares no rule is
 * the onboarding state here too, as on the Dashboard (CR-141): a check over zero
 * rules is not a pass.
 */

import { fetchGaps } from "../../api/client.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type { GapsModel, RulesReport } from "../../api/types.ts";
import {
  Badge,
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  Widget,
  WidgetStack,
} from "../../components/index.ts";
import type { BadgeTone, Column } from "../../components/index.ts";
import { RULE_FINDINGS_TEXT, ruleFindings, ruleFindingsState } from "../../copy/ruleFindings.copy.ts";
import { plural } from "../../copy/types.ts";
import styles from "./GapsView.module.css";

export function GapsView() {
  const model = useApiResource<GapsModel>(() => fetchGaps(), []);
  return (
    <AsyncResource
      resource={model}
      loadingLabel="Loading the rule findings…"
      isEmpty={(m) => !m.status.indexed}
      empty={<EmptyState message="No index yet — run" command="logos index" />}
    >
      {(m) => <RuleFindings model={m} />}
    </AsyncResource>
  );
}

function RuleFindings({ model }: { model: GapsModel }) {
  const findings = model.rules.violations.length;
  const clean = findings === 0;
  return (
    <WidgetStack>
      <Callout label="RULE FINDINGS" tone={clean ? "muted" : "signal"}>
        <span>
          {findings} rule {plural(findings, "finding", "findings")}
        </span>
      </Callout>
      <RulesCard report={model.rules} />
    </WidgetStack>
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

/** The rule-findings widget — three honest states (NFR-CC-04): the findings
 *  table, a clean check over at least one rule, or the no-rules onboarding (no
 *  `.logos/rules.toml`, or one declaring no rule). Findings are checked first so
 *  a populated report always renders its table (S-354). */
function RulesCard({ report }: { report: RulesReport }) {
  const state = ruleFindingsState(report);
  const { findings, checked } = state;
  if (findings > 0) {
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
      <Widget
        title="Rule findings"
        badge={<Badge tone="red">FAIL</Badge>}
        copy={ruleFindings}
        state={state}
        figure={<span>{RULE_FINDINGS_TEXT.findings(findings, report.checked_rules)}</span>}
      >
        <DataTable
          caption="Rule findings"
          columns={columns}
          rows={rows}
          rowKey={(r, i) => `${r.rule}#${i}`}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
      </Widget>
    );
  }
  if (checked === 0) {
    return (
      <Widget title="Rule findings" copy={ruleFindings} state={state} absence={RULE_FINDINGS_TEXT.noRules}>
        <RulesOnboarding />
      </Widget>
    );
  }
  return (
    <Widget
      title="Rule findings"
      badge={<Badge tone="green">PASS</Badge>}
      copy={ruleFindings}
      state={state}
      figure={<span>{RULE_FINDINGS_TEXT.clean(checked)}</span>}
    />
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

/** The no-rules onboarding (NFR-CC-04, frontend-design §4.6), as the widget's
 *  evidence: what rules buy you and a runnable example contract, rather than an
 *  always-empty findings table. The file and the evaluating command are the
 *  widget's action. */
function RulesOnboarding() {
  return (
    <div className={styles.onboarding}>
      <p>
        Architecture rules enforce layering, ban forbidden imports, and require tested or
        documented surfaces; findings then appear here with severity badges.
      </p>
      <p className={styles.note}>For example:</p>
      <pre className={styles.example}>{EXAMPLE_RULES}</pre>
    </div>
  );
}
