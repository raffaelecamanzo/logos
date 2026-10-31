/*
 * The Coverage page's run model (S-739, FR-CV-15, FR-CV-11): pure functions over
 * the `coverage-run` job and the tool rows of the coverage status, so the widget
 * and the table's live State render from one reading of them.
 *
 * A job is followed as a snapshot plus the steps its event stream sent: the
 * stream's `end` carries the job without its events, so the steps are kept
 * apart and merged by `seq` (`withJob`, `withSteps`). Every state is a word — a
 * tool that is not installed, or whose suite failed, never reads as 0 % (BR-77).
 *
 * S-750 (FR-CV-17, FR-CV-18): a run the page readies first — Install, Add
 * dependency — reports the install, the `llvm-tools-preview` step or the addition
 * before any tool; `prepareRows` lists those, and the tool rows read only the
 * tools' own steps.
 *
 * S-803 (CR-250): the Coverage tools table carries the run. `liveTools` gives each
 * tool its live State while the job runs — a readied tool `installing` or `adding
 * dependency` — and `elapsedMs` its Elapsed: counting while it readies or runs,
 * else the `duration_ms` its last run recorded. `toolMessages` is the cause below
 * the table, from the stored last-run record and the readying steps that went wrong.
 */

import type {
  CoverageJob,
  CoverageJobEvent,
  CoveragePrepareProgress,
  CoverageToolRun,
  CoverageToolStatus,
} from "../../api/index.ts";
import {
  COVERAGE_PREPARE_TEXT,
  COVERAGE_RUN_TEXT,
  COVERAGE_TOOL_ECOSYSTEM,
  COVERAGE_TOOL_STATE_TEXT,
} from "../../copy/coverageView.copy.ts";

/** The registry's name for the configured `refresh_cmd`. */
export const CUSTOM_TOOL = "custom";

/** The tool states a run cannot start: nothing of it is a run. */
const NOT_RUNNABLE: ReadonlySet<CoverageToolStatus["state"]> = new Set(["not_applicable", "disabled", "not_installed"]);

/** Whether a run would start `tool` (its copy is installed, and it is enabled). */
export function runnable(tool: CoverageToolStatus): boolean {
  return !NOT_RUNNABLE.has(tool.state);
}

/** The rows the widget lists: every tool whose ecosystem is at the project root. */
export function listedTools(tools: readonly CoverageToolStatus[]): CoverageToolStatus[] {
  return tools.filter((t) => t.state !== "not_applicable");
}

/** The ecosystems the listed tools say were detected, once each, in registry order. */
export function detectedEcosystems(tools: readonly CoverageToolStatus[]): string[] {
  const found = listedTools(tools)
    .map((t) => COVERAGE_TOOL_ECOSYSTEM[t.tool])
    .filter((e): e is string => e !== undefined);
  return [...new Set(found)];
}

/** A followed job: its last snapshot, and every step seen for it, by `seq`. */
export interface RunView {
  job: CoverageJob | null;
  steps: CoverageJobEvent[];
}

export const NO_RUN: RunView = { job: null, steps: [] };

/** `steps` added to `view`, each `seq` once, in order. */
export function withSteps(view: RunView, steps: readonly CoverageJobEvent[]): RunView {
  const known = new Set(view.steps.map((s) => s.seq));
  const added = steps.filter((s) => !known.has(s.seq));
  if (added.length === 0) return view;
  return { ...view, steps: [...view.steps, ...added].sort((a, b) => a.seq - b.seq) };
}

/** `job` as the latest snapshot: another job starts over from its own events; the
 *  same job keeps the steps already seen and adds any the snapshot carries. */
export function withJob(view: RunView, job: CoverageJob | null): RunView {
  if (job === null) return view.job === null ? view : NO_RUN;
  if (view.job?.id !== job.id) return { job, steps: [...job.events].sort((a, b) => a.seq - b.seq) };
  return withSteps({ ...view, job }, job.events);
}

/** A tool's progress state in the run going. */
export type ProgressState =
  | "queued"
  | "running"
  | "ran"
  | "tests_failed"
  | "not_installed"
  | "failed"
  | "timed_out"
  | "cancelled";

export interface ProgressRow {
  tool: string;
  state: ProgressState;
  /** When it started, in milliseconds since the job did. */
  startedMs: number | null;
  /** When it settled, in milliseconds since the job started. */
  endedMs: number | null;
  /** Its settled part, once it settled. */
  run: CoverageToolRun | null;
}

/** The outcomes the run's rows leave out: the tool was never a candidate. */
const SILENT = new Set(["not_applicable", "disabled"]);

/** The per-tool rows of `view`'s job, in the run's own order — the status rows'
 *  (`tools`), then any other tool the job reported: each tool the run starts
 *  here, and each other tool it reported, as it stands. A tool not started yet is
 *  `queued` while the job runs, and one left unsettled when the job ended reads
 *  as the job ended (`cancelled`, else `failed`). A tool that was not applicable
 *  or switched off is not listed. */
export function progressRows(view: RunView, tools: readonly CoverageToolStatus[]): ProgressRow[] {
  const { job, steps } = view;
  if (job === null) return [];
  const expected = new Set(tools.filter(runnable).map((t) => t.tool));
  const started = new Map<string, number>();
  const ended = new Map<string, number>();
  const settled = new Map<string, CoverageToolRun>();
  const order = tools.map((t) => t.tool);
  for (const { at_ms, event } of steps) {
    // A readying step (install, component, dependency) is no tool's row.
    if (event.phase !== "started" && event.phase !== "finished") continue;
    if (!order.includes(event.tool)) order.push(event.tool);
    if (event.phase === "started") {
      started.set(event.tool, at_ms);
      continue;
    }
    const { phase: _phase, ...run } = event;
    settled.set(event.tool, run);
    ended.set(event.tool, at_ms);
  }
  const unsettled: ProgressState = job.state === "running" ? "queued" : job.state === "cancelled" ? "cancelled" : "failed";
  const rows: ProgressRow[] = [];
  for (const tool of order) {
    const run = settled.get(tool) ?? null;
    const startedMs = started.get(tool) ?? null;
    const state = rowState(job, run, startedMs, expected.has(tool), unsettled);
    if (state !== null) rows.push({ tool, state, startedMs, endedMs: ended.get(tool) ?? null, run });
  }
  return rows;
}

/** One tool's progress state, or `null` when it is not listed: a tool that was
 *  not applicable or switched off, one neither started here nor reported, and
 *  one a completed run never reported — every tool a completed run started is
 *  settled, so that one was not a candidate after all (uninstalled since the
 *  page read its state). */
function rowState(
  job: CoverageJob,
  run: CoverageToolRun | null,
  startedMs: number | null,
  expected: boolean,
  unsettled: ProgressState,
): ProgressState | null {
  if (run !== null) return SILENT.has(run.outcome) ? null : (run.outcome as ProgressState);
  if (startedMs === null && (!expected || job.state === "completed")) return null;
  if (startedMs !== null && job.state === "running") return "running";
  return unsettled;
}

/** The words of a progress state. */
export function progressText(state: ProgressState): string {
  return COVERAGE_TOOL_STATE_TEXT[state];
}

/** Matched of reported files of one settled run; `null` when it ingested nothing. */
export function filesOf(run: CoverageToolRun | null): { matched: number; reported: number } | null {
  if (run === null || !("ingests" in run) || !run.ingests || run.ingests.length === 0) return null;
  let matched = 0;
  let reported = 0;
  for (const ingest of run.ingests) {
    matched += ingest.matched_files;
    reported += ingest.matched_files + ingest.unmatched.length;
  }
  return { matched, reported };
}

/** The one sentence the polite live region holds: the end of the run once it
 *  ended, else the last tool that settled — what changed, never a clock. */
export function announcement(view: RunView, rows: readonly ProgressRow[]): string {
  const { job } = view;
  if (job === null) return "";
  if (job.state === "completed") {
    const count = (s: ProgressState) => rows.filter((r) => r.state === s).length;
    return COVERAGE_RUN_TEXT.completed(count("ran") + count("tests_failed"), count("tests_failed"), count("failed") + count("timed_out"));
  }
  if (job.state === "cancelled") {
    const finished = rows.filter((r) => r.run !== null && r.state !== "cancelled" && r.state !== "not_installed").map((r) => r.tool);
    return COVERAGE_RUN_TEXT.cancelled(finished);
  }
  if (job.state === "failed") return COVERAGE_RUN_TEXT.failed(job.error ?? "no reason was given");
  const prepared = prepareRows(view);
  for (const { event } of [...view.steps].reverse()) {
    if (event.phase === "finished" && !SILENT.has(event.outcome)) {
      const row = rows.find((r) => r.tool === event.tool);
      return COVERAGE_RUN_TEXT.toolDone(event.tool, progressText(row?.state ?? "ran"));
    }
    if (event.phase !== "started" && event.phase !== "finished") {
      const row = prepared.find((r) => r.key === prepareKey(event));
      if (row) return COVERAGE_PREPARE_TEXT.line(row.what, COVERAGE_PREPARE_TEXT.state[row.state]);
    }
  }
  return COVERAGE_RUN_TEXT.started;
}

// ── What a prepared run readies before its tools (S-750) ─────────────────────

/** A readying step's state: going, or how it ended. */
export type PrepareState = keyof typeof COVERAGE_PREPARE_TEXT.state;

/** One step a prepared run took before its tools: a tool's install, the
 *  `llvm-tools-preview` step, or a dev-dependency addition. */
export interface PrepareRow {
  key: string;
  /** The coverage tool it readies: the table row that reads `installing` or
   *  `adding dependency` while it goes. */
  tool: string;
  kind: "install" | "component" | "dependency";
  /** What it readies: `cargo-llvm-cov 0.9.1`, `llvm-tools-preview`, the command. */
  what: string;
  state: PrepareState;
  /** Why it failed, what to run by hand, or what was put back. */
  detail: string | null;
  /** When the step began, in milliseconds since the job did. */
  startedMs: number;
}

/** The tool whose install a `component_*` step completes: the `llvm-tools-preview`
 *  component is `cargo-llvm-cov`'s, the one managed tool. */
const COMPONENT_TOOL = "cargo-llvm-cov";

/** The row a readying step updates. */
function prepareKey(event: CoveragePrepareProgress): string {
  switch (event.phase) {
    case "install_started":
    case "install_finished":
      return `install:${event.tool}`;
    case "component_started":
    case "component_finished":
      return `component:${event.component}`;
    default:
      return `dependency:${event.tool}`;
  }
}

/** One readying step as its row: what it readies, its state, and the detail
 *  its outcome carries. */
function prepareRow(event: CoveragePrepareProgress): Omit<PrepareRow, "startedMs"> {
  const key = prepareKey(event);
  switch (event.phase) {
    case "install_started":
      return { key, tool: event.tool, kind: "install", what: `${event.tool} ${event.version}`, state: "running", detail: null };
    case "install_finished": {
      const detail = event.outcome === "failed" || event.outcome === "unavailable" ? event.reason : null;
      return { key, tool: event.tool, kind: "install", what: `${event.tool} ${event.version}`, state: event.outcome, detail };
    }
    case "component_started":
      return { key, tool: COMPONENT_TOOL, kind: "component", what: event.component, state: "running", detail: null };
    case "component_finished": {
      const detail =
        event.outcome === "failed"
          ? event.reason
          : event.outcome === "no_rustup"
            ? COVERAGE_PREPARE_TEXT.byHand(event.reason, event.command)
            : null;
      return { key, tool: COMPONENT_TOOL, kind: "component", what: event.component, state: event.outcome, detail };
    }
    case "dependency_started":
      return { key, tool: event.tool, kind: "dependency", what: event.command, state: "running", detail: null };
    case "dependency_finished": {
      // Why it did not end added, then what was put back: a manager that
      // changed the files and then failed — the usual failure — says both.
      const restored = event.restored ?? [];
      const why =
        event.outcome === "failed"
          ? event.stderr
          : event.outcome === "refused"
            ? COVERAGE_PREPARE_TEXT.byHand(event.reason, event.hand_command)
            : null;
      const putBack = restored.length > 0 ? COVERAGE_PREPARE_TEXT.putBack(restored) : null;
      const detail = [why, putBack].filter((part) => part !== null).join(" ") || null;
      return { key, tool: event.tool, kind: "dependency", what: event.command ?? event.package, state: event.outcome, detail };
    }
  }
}

/** The readying steps of `view`'s job, in the order they began: each one going,
 *  or as it ended. One still going when the job ended reads as the job ended
 *  (`cancelled`, else `failed`). Empty for a run that readied nothing. */
export function prepareRows(view: RunView): PrepareRow[] {
  const { job, steps } = view;
  if (job === null) return [];
  const rows = new Map<string, PrepareRow>();
  for (const { at_ms, event } of steps) {
    if (event.phase === "started" || event.phase === "finished") continue;
    const row = prepareRow(event);
    // A step keeps the moment it began, whichever event last updated it.
    rows.set(row.key, { ...row, startedMs: rows.get(row.key)?.startedMs ?? at_ms });
  }
  const unsettled: PrepareState = job.state === "cancelled" ? "cancelled" : "failed";
  return [...rows.values()].map((row) => (row.state === "running" && job.state !== "running" ? { ...row, state: unsettled } : row));
}

// ── The table carries the run (S-803, CR-250) ───────────────────────────────

/** What the State cell reads for a tool while a run goes: its progress state, or
 *  `installing` / `adding dependency` while it is readied. */
export type LiveState = ProgressState | "installing" | "adding_dependency";

/** A tool's part in the run going, for its State and Elapsed cells. */
export interface LiveTool {
  state: LiveState;
  /** When it started — or, readying, when the step began — in milliseconds since the job did. */
  startedMs: number | null;
  /** When it settled; `null` while it has not. */
  endedMs: number | null;
}

/** The readying outcomes after which the tool goes on to run. */
const READIED: ReadonlySet<PrepareState> = new Set(["installed", "already_present", "added"]);

/** Each tool's live State while `view`'s job runs, by tool name. A job that is
 *  not running (it ended, and the page's reload has landed — HF-6) has none: the
 *  stored state is shown again. A readied tool reads `installing` or `adding
 *  dependency` while a step goes and `queued` once they all ended well; one whose
 *  step failed, was refused or was cancelled keeps its stored state, and the step
 *  is stated below the table. */
export function liveTools(view: RunView, rows: readonly ProgressRow[], prepared: readonly PrepareRow[]): Map<string, LiveTool> {
  const live = new Map<string, LiveTool>();
  if (view.job?.state !== "running") return live;
  for (const r of rows) live.set(r.tool, { state: r.state, startedMs: r.startedMs, endedMs: r.endedMs });
  const steps = new Map<string, PrepareRow[]>();
  for (const step of prepared) steps.set(step.tool, [...(steps.get(step.tool) ?? []), step]);
  for (const [tool, own] of steps) {
    if (live.has(tool)) continue;
    const going = own.filter((s) => s.state === "running").at(-1);
    if (going) {
      live.set(tool, { state: going.kind === "dependency" ? "adding_dependency" : "installing", startedMs: going.startedMs, endedMs: null });
    } else if (own.every((s) => READIED.has(s.state))) {
      live.set(tool, { state: "queued", startedMs: null, endedMs: null });
    }
  }
  return live;
}

/** A tool's Elapsed in milliseconds: while it is live, from its start to now
 *  (`jobNow`, in milliseconds since the job started) or to its end in this run;
 *  else the `duration_ms` its last run recorded. `null` reads `—`: nothing is
 *  recorded (never run, or a record written before it was kept), or the tool has
 *  not started. */
export function elapsedMs(tool: CoverageToolStatus, live: LiveTool | undefined, jobNow: number | null): number | null {
  if (live === undefined) return tool.last_run?.duration_ms ?? null;
  if (live.startedMs === null) return null;
  const end = live.endedMs ?? jobNow;
  return end === null ? null : Math.max(0, end - live.startedMs);
}

/** A readying step that went wrong: shown below the table with its detail. */
const PREPARE_PROBLEM: ReadonlySet<PrepareState> = new Set(["failed", "unavailable", "timed_out", "refused", "no_rustup"]);

/** The last-run outcomes the message area states. */
const RUN_PROBLEM: ReadonlySet<CoverageToolStatus["state"]> = new Set(["failed", "timed_out", "tests_failed"]);

/** One entry of the message area under the table. */
export type ToolMessage =
  | {
      kind: "run";
      tool: string;
      state: "failed" | "timed_out" | "tests_failed";
      /** The masked excerpt S-802 records; `null` for a record written before it. */
      cause: string | null;
      /** The whole output's log, when one was written. */
      log: string | null;
    }
  | { kind: "prepare"; key: string; tool: string; what: string; state: PrepareState; detail: string | null };

/** The cause below the table: each listed tool whose last run failed, timed out
 *  or had failing tests — from the stored record, so it stays across a reload
 *  until that tool runs again (a tool live in the run going is leaving it) — then
 *  each readying step that failed or was refused, or was cancelled after putting
 *  files back. Empty when there is none. */
export function toolMessages(
  tools: readonly CoverageToolStatus[],
  prepared: readonly PrepareRow[],
  live: ReadonlyMap<string, LiveTool>,
): ToolMessage[] {
  const messages: ToolMessage[] = [];
  for (const t of listedTools(tools)) {
    const run = t.last_run;
    if (run === null || !RUN_PROBLEM.has(run.outcome) || live.has(t.tool)) continue;
    messages.push({
      kind: "run",
      tool: t.tool,
      state: run.outcome as "failed" | "timed_out" | "tests_failed",
      cause: run.cause ?? null,
      log: run.log,
    });
  }
  for (const step of prepared) {
    // A cancelled addition that put files back says so; a cancel that changed nothing says nothing.
    if (PREPARE_PROBLEM.has(step.state) || (step.state === "cancelled" && step.detail !== null)) {
      messages.push({ kind: "prepare", key: step.key, tool: step.tool, what: step.what, state: step.state, detail: step.detail });
    }
  }
  return messages;
}
