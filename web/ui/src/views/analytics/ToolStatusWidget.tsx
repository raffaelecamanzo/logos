/*
 * ToolStatusWidget (S-739, FR-CV-15, FR-CV-11; frontend-design §4.10) — the
 * Coverage page's lead widget: one row per coverage tool whose ecosystem is at the
 * project root, plus the custom row when a `refresh_cmd` is set. Run coverage and
 * Cancel sit on its title row (`RunControls`), and the run's progress list and its
 * polite live line sit above the table (`RunProgress`).
 *
 * Columns: Tool · Version · State · Last run · Files.
 * - Version: as detection read it, else `—` (the page's read starts no tool, so a
 *   copy outside the project reads with its version unknown).
 * - State: one word per state — not installed, disabled, never run, ran, ran —
 *   tests failed, failed, timed out, cancelled — never a 0 % (BR-77); under a
 *   not-installed state its hint, under a failed one its whole-output log.
 * - Last run: how long ago, and the commit's short hash.
 * - Files: matched of reported, from this serve's last run of the tool; `—` when
 *   this serve has not run it (the run record keeps no file counts).
 *
 * A tool whose ecosystem is absent is not listed; the explanation says which
 * ecosystems were detected.
 *
 * S-750 (FR-CV-17, FR-CV-18): a not-installed row Logos can set up carries its
 * offer under the state — **Install** for a managed tool (`cargo-llvm-cov`),
 * **Add dependency** for a vitest without its coverage provider or the project's
 * pytest without pytest-cov — which asks in the run's consent dialog, then runs
 * as the coverage job, the install or the addition first. Disabled while a run
 * goes.
 */

import type { CoverageToolStatus } from "../../api/index.ts";
import { Badge, Button, DataTable, FigureNote, Widget, type BadgeTone, type Column } from "../../components/index.ts";
import {
  COVERAGE_OFFER_TEXT,
  COVERAGE_RUN_TEXT,
  COVERAGE_TOOL_STATE_TEXT,
  COVERAGE_VIEW_ABSENCE,
  coverageTools,
} from "../../copy/coverageView.copy.ts";
import { humanizeAge } from "../dashboard/dashboardModel.ts";
import { RunControls, RunProgress, type CoverageRunHandle } from "./CoverageRunPanel.tsx";
import { CUSTOM_TOOL, detectedEcosystems, filesOf, listedTools, runnable } from "./coverageRunModel.ts";
import styles from "./AnalyticsView.module.css";

const STATE_TONE: Record<CoverageToolStatus["state"], BadgeTone> = {
  not_applicable: "muted",
  disabled: "muted",
  not_installed: "muted",
  never_run: "muted",
  ran: "green",
  tests_failed: "orange",
  failed: "red",
  timed_out: "red",
  cancelled: "muted",
};

function StateCell({ tool, run }: { tool: CoverageToolStatus; run: CoverageRunHandle }) {
  const log = tool.state === "failed" ? tool.last_run?.log : null;
  return (
    <>
      <Badge tone={STATE_TONE[tool.state]}>{COVERAGE_TOOL_STATE_TEXT[tool.state]}</Badge>
      {tool.state === "not_installed" && tool.hint && <p className={`muted ${styles.fix}`}>{tool.hint}</p>}
      {tool.state === "not_installed" && tool.offer && <OfferButton tool={tool.tool} offer={tool.offer} run={run} />}
      {log && <p className={`muted ${styles.fix}`}>{COVERAGE_RUN_TEXT.log(log)}</p>}
    </>
  );
}

/** A not-installed row's offer: Install, or Add dependency. */
function OfferButton({ tool, offer, run }: { tool: string; offer: "install" | "add_dependency"; run: CoverageRunHandle }) {
  const install = offer === "install";
  return (
    <div>
      <Button
        size="sm"
        disabled={run.running}
        aria-label={install ? COVERAGE_OFFER_TEXT.installLabel(tool) : COVERAGE_OFFER_TEXT.addDependencyLabel(tool)}
        onClick={install ? run.install : () => run.addDependency(tool)}
      >
        {install ? COVERAGE_OFFER_TEXT.install : COVERAGE_OFFER_TEXT.addDependency}
      </Button>
    </div>
  );
}

export interface ToolStatusWidgetProps {
  /** The coverage status's tool rows (`coverage.runner.tools`). */
  tools: readonly CoverageToolStatus[];
  run: CoverageRunHandle;
  /** Now, in Unix seconds: the Last run column's reference. */
  nowUnix?: number;
}

export function ToolStatusWidget({ tools, run, nowUnix = Math.floor(Date.now() / 1000) }: ToolStatusWidgetProps) {
  const rows = listedTools(tools);
  const ecosystems = detectedEcosystems(tools);
  const custom = rows.some((t) => t.tool === CUSTOM_TOOL);
  const installed = rows.filter(runnable).length;
  const disabled = rows.filter((t) => t.state === "disabled").length;
  const settled = new Map(run.rows.map((r) => [r.tool, r.run]));

  const columns: Column<CoverageToolStatus>[] = [
    { key: "tool", header: "Tool", mono: true, sortValue: (t) => t.tool, cell: (t) => t.tool },
    { key: "version", header: "Version", mono: true, cell: (t) => t.version ?? "—" },
    { key: "state", header: "State", sortValue: (t) => t.state, cell: (t) => <StateCell tool={t} run={run} /> },
    {
      key: "last",
      header: "Last run",
      sortValue: (t) => t.last_run?.finished_at ?? -1,
      cell: (t) =>
        t.last_run ? COVERAGE_RUN_TEXT.lastRun(humanizeAge(nowUnix, t.last_run.finished_at), t.last_run.head_sha) : "—",
    },
    {
      key: "files",
      header: "Files",
      cell: (t) => {
        const files = filesOf(settled.get(t.tool) ?? null);
        return files ? COVERAGE_RUN_TEXT.files(files.matched, files.reported) : "—";
      },
    },
  ];

  if (rows.length === 0) {
    return <Widget title="Coverage tools" copy={coverageTools} absence={COVERAGE_VIEW_ABSENCE.noToolApplies} />;
  }
  return (
    <Widget
      title="Coverage tools"
      copy={coverageTools}
      note={COVERAGE_RUN_TEXT.detected(ecosystems, custom)}
      badge={<RunControls run={run} />}
      figure={
        <span>
          {installed} <FigureNote>{COVERAGE_RUN_TEXT.installed(rows.length - disabled, disabled)}</FigureNote>
        </span>
      }
    >
      <RunProgress run={run} />
      <DataTable caption="Coverage tools" columns={columns} rows={rows} rowKey={(t) => t.tool} />
    </Widget>
  );
}
