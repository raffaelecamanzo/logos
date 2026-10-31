/*
 * ToolStatusWidget (S-739, FR-CV-15, FR-CV-11; frontend-design §4.10) — the
 * Coverage page's lead widget: one row per coverage tool whose ecosystem is at the
 * project root, plus the custom row when a `refresh_cmd` is set. Run coverage and
 * Cancel sit on its title row (`RunControls`), and the run's polite live line sits
 * above the table (`RunProgress`).
 *
 * S-803 (CR-250): the table carries the run — there is no progress list. While a
 * job runs, a tool's State is its live one (`liveTools`): queued, running, its
 * outcome as it settles, or installing / adding dependency while it is readied;
 * once the run is shown ended (HF-6, after the page's reload) every row shows its
 * stored state again.
 *
 * Columns: Tool · Version · State · Elapsed · Last run · Files.
 * - Version: as detection read it, else `—` (the page's read starts no tool, so a
 *   copy outside the project reads with its version unknown).
 * - State: one word per state — not installed, disabled, never run, ran, ran —
 *   tests failed, failed, timed out, cancelled — never a 0 % (BR-77); under a
 *   not-installed state its hint.
 * - Elapsed: counts up each second while the tool readies or runs; afterwards the
 *   `duration_ms` its last run recorded (S-802), `—` when none is.
 * - Last run: how long ago, and the commit's short hash.
 * - Files: matched of reported, from this serve's last run of the tool; `—` when
 *   this serve has not run it (the run record keeps no file counts).
 *
 * Below the table, the message area (`ToolMessages`) lists each tool whose last
 * run failed or timed out (red) or ran with failing tests (orange) with its cause
 * and, when one was written, its log path, and each readying step that failed or
 * was refused with its detail; absent when there is none. A tool's entry stays
 * until it runs again.
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
  COVERAGE_PREPARE_TEXT,
  COVERAGE_RUN_TEXT,
  COVERAGE_TOOL_STATE_TEXT,
  COVERAGE_VIEW_ABSENCE,
  coverageTools,
} from "../../copy/coverageView.copy.ts";
import { humanizeAge } from "../dashboard/dashboardModel.ts";
import { RunControls, RunProgress, useJobClock, type CoverageRunHandle } from "./CoverageRunPanel.tsx";
import {
  CUSTOM_TOOL,
  detectedEcosystems,
  elapsedMs,
  filesOf,
  listedTools,
  liveTools,
  runnable,
  toolMessages,
  type LiveState,
  type LiveTool,
  type PrepareState,
  type ToolMessage,
} from "./coverageRunModel.ts";
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

/** The tone of a live state: in flight is orange, an outcome as it is stored. */
const LIVE_TONE: Record<LiveState, BadgeTone> = {
  queued: "muted",
  running: "orange",
  installing: "orange",
  adding_dependency: "orange",
  ran: "green",
  tests_failed: "orange",
  not_installed: "muted",
  failed: "red",
  timed_out: "red",
  cancelled: "muted",
};

/** The stored state, or — while the run goes — the live one; under a not-installed
 *  stored state its hint and offer. The log path is the message area's. */
function StateCell({ tool, live, run }: { tool: CoverageToolStatus; live: LiveTool | undefined; run: CoverageRunHandle }) {
  if (live !== undefined) return <Badge tone={LIVE_TONE[live.state]}>{COVERAGE_TOOL_STATE_TEXT[live.state]}</Badge>;
  return (
    <>
      <Badge tone={STATE_TONE[tool.state]}>{COVERAGE_TOOL_STATE_TEXT[tool.state]}</Badge>
      {tool.state === "not_installed" && tool.hint && <p className={`muted ${styles.fix}`}>{tool.hint}</p>}
      {tool.state === "not_installed" && tool.offer && <OfferButton tool={tool.tool} offer={tool.offer} run={run} />}
    </>
  );
}

const PREPARE_TONE: Record<PrepareState, BadgeTone> = {
  running: "orange",
  installed: "green",
  already_present: "green",
  added: "green",
  no_rustup: "orange",
  unavailable: "red",
  pending: "muted",
  refused: "orange",
  failed: "red",
  timed_out: "red",
  cancelled: "muted",
};

/** One entry: the tool, its state in words and its cause — a run's excerpt, which
 *  names the failing test, and where its whole output is; a readying step's detail. */
function MessageItem({ message }: { message: ToolMessage }) {
  if (message.kind === "prepare") {
    return (
      <li>
        <span className="mono">{message.tool}</span> <Badge tone={PREPARE_TONE[message.state]}>{COVERAGE_PREPARE_TEXT.state[message.state]}</Badge>{" "}
        <span className="muted mono">{message.what}</span>
        {message.detail !== null && <p className={`muted ${styles.fix}`}>{message.detail}</p>}
      </li>
    );
  }
  return (
    <li>
      <span className="mono">{message.tool}</span> <Badge tone={STATE_TONE[message.state]}>{COVERAGE_TOOL_STATE_TEXT[message.state]}</Badge>
      {message.cause !== null ? (
        <pre className={`mono ${styles.stderr}`}>{message.cause}</pre>
      ) : (
        <p className={`muted ${styles.fix}`}>{COVERAGE_RUN_TEXT.noCause}</p>
      )}
      {message.log && <p className={`muted ${styles.fix}`}>{COVERAGE_RUN_TEXT.log(message.log)}</p>}
    </li>
  );
}

/** The message area under the table; absent when no tool has anything to state. */
function ToolMessages({ messages }: { messages: readonly ToolMessage[] }) {
  if (messages.length === 0) return null;
  return (
    <ul className={styles.messages} aria-label={COVERAGE_RUN_TEXT.messages}>
      {messages.map((m) => (
        <MessageItem key={m.kind === "run" ? `run:${m.tool}` : m.key} message={m} />
      ))}
    </ul>
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
  const jobNow = useJobClock(run.view);
  const live = liveTools(run.view, run.rows, run.prepared);

  const columns: Column<CoverageToolStatus>[] = [
    { key: "tool", header: "Tool", mono: true, sortValue: (t) => t.tool, cell: (t) => t.tool },
    { key: "version", header: "Version", mono: true, cell: (t) => t.version ?? "—" },
    { key: "state", header: "State", sortValue: (t) => t.state, cell: (t) => <StateCell tool={t} live={live.get(t.tool)} run={run} /> },
    {
      key: "elapsed",
      header: "Elapsed",
      cell: (t) => {
        const ms = elapsedMs(t, live.get(t.tool), jobNow);
        return ms === null ? "—" : COVERAGE_RUN_TEXT.elapsed(ms);
      },
    },
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
      <ToolMessages messages={toolMessages(tools, run.prepared, live)} />
    </Widget>
  );
}
