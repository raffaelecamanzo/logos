// Coverage-run fixtures for the Coverage page's tests (S-739), shaped as the
// server sends them and as `web/tests/coverage_job.rs` pins them: a run reports
// one `finished` step per REGISTERED tool in registry order — the tools whose
// ecosystem is absent included, as `not_applicable` — then the custom command;
// an installed tool's `finished` follows its `started`; the stream's `end`
// carries the job with no events; a cancelled job has no result.
import type {
  CoverageConsentRequired,
  CoverageIngestSummary,
  CoveragePrepareRequired,
  CoverageJob,
  CoverageJobEvent,
  CoverageRunProgress,
  CoverageToolStatus,
} from "../../../api/index.ts";

/** A Go module with a Maven build and a vitest package, and a custom command:
 *  go installed (on the PATH, so its version reads unknown), vitest installed in
 *  the project, Maven not installed. */
export const TOOLS: CoverageToolStatus[] = [
  { tool: "cargo-llvm-cov", state: "not_applicable", last_run: null },
  { tool: "vitest", state: "never_run", version: "3.2.4", origin: "project", last_run: null },
  { tool: "jest", state: "not_applicable", last_run: null },
  { tool: "pytest-cov", state: "not_applicable", last_run: null },
  { tool: "maven", state: "not_installed", last_run: null },
  { tool: "gradle", state: "not_applicable", last_run: null },
  { tool: "go", state: "never_run", origin: "path", last_run: null },
  { tool: "custom", state: "never_run", last_run: null },
];

/** The same project after one run (S-802's record: each tool's `duration_ms`, and
 *  the `cause` of the two that did not pass): vitest's suite failed with its
 *  report ingested and names the failing test, go ran, the custom command failed. */
export const TOOLS_AFTER: CoverageToolStatus[] = [
  { tool: "cargo-llvm-cov", state: "not_applicable", last_run: null },
  {
    tool: "vitest",
    state: "tests_failed",
    version: "3.2.4",
    origin: "project",
    last_run: {
      outcome: "tests_failed",
      exit_code: 1,
      head_sha: "4f9c2e1d0b7a",
      finished_at: 1_761_170_000,
      log: null,
      duration_ms: 4_200,
      cause: "FAIL src/cart.test.ts > totals the cart\nAssertionError: expected 41 to be 42",
    },
  },
  { tool: "jest", state: "not_applicable", last_run: null },
  { tool: "pytest-cov", state: "not_applicable", last_run: null },
  { tool: "maven", state: "not_installed", last_run: null },
  { tool: "gradle", state: "not_applicable", last_run: null },
  {
    tool: "go",
    state: "ran",
    origin: "path",
    last_run: { outcome: "ran", exit_code: 0, head_sha: "4f9c2e1d0b7a", finished_at: 1_761_170_000, log: null, duration_ms: 1_500 },
  },
  {
    tool: "custom",
    state: "failed",
    last_run: {
      outcome: "failed",
      exit_code: 2,
      head_sha: "4f9c2e1d0b7a",
      finished_at: 1_761_170_000,
      log: ".logos/coverage/custom.failed.log",
      duration_ms: 380,
      cause: "make: *** No rule to make target 'cov'.  Stop.",
    },
  },
];

function ingest(format: string, matched: number, unmatched: string[]): CoverageIngestSummary {
  return {
    format,
    head_sha: "4f9c2e1d0b7a",
    snapshot_id: 3,
    matched_files: matched,
    unmatched,
    instrumented_lines: 40,
    covered_lines: 31,
  };
}

const PROGRESS: CoverageRunProgress[] = [
  { phase: "finished", tool: "cargo-llvm-cov", outcome: "not_applicable" },
  { phase: "started", tool: "vitest" },
  {
    phase: "finished",
    tool: "vitest",
    version: "3.2.4",
    outcome: "tests_failed",
    exit_code: 1,
    reports: ["coverage/lcov.info"],
    ingests: [ingest("lcov", 12, ["vendor/shim.js"])],
  },
  { phase: "finished", tool: "jest", outcome: "not_applicable" },
  { phase: "finished", tool: "pytest-cov", outcome: "not_applicable" },
  { phase: "finished", tool: "maven", outcome: "not_installed" },
  { phase: "finished", tool: "gradle", outcome: "not_applicable" },
  { phase: "started", tool: "go" },
  {
    phase: "finished",
    tool: "go",
    version: "go1.23.1",
    outcome: "ran",
    reports: [".logos/coverage/go.coverprofile"],
    ingests: [ingest("gocover", 1, [])],
  },
  { phase: "started", tool: "custom" },
  {
    phase: "finished",
    tool: "custom",
    outcome: "failed",
    exit_code: 2,
    stderr: "make: *** No rule to make target 'cov'.  Stop.",
    log: ".logos/coverage/custom.failed.log",
  },
];

/** Every event of the full run, `seq` from 0, a few milliseconds apart. */
export const EVENTS: CoverageJobEvent[] = PROGRESS.map((event, seq) => ({ seq, at_ms: 40 + seq * 900, event }));

/** The run as it stood while go ran: vitest settled, go started. */
export const RUNNING: CoverageJob = {
  id: 4,
  kind: "coverage-run",
  state: "running",
  elapsed_ms: EVENTS[7].at_ms,
  events: EVENTS.slice(0, 8),
  events_dropped: 0,
  result: null,
  error: null,
};

/** The run as it ended: the `end` of the stream carries no events. */
export const COMPLETED_END: CoverageJob = {
  ...RUNNING,
  state: "completed",
  elapsed_ms: EVENTS[10].at_ms + 30,
  events: [],
  result: {
    head_sha: "4f9c2e1d0b7a",
    consent_recorded: [],
    tools: PROGRESS.filter((p) => p.phase === "finished").map(({ phase: _phase, ...run }) => run),
    snapshot_id: 3,
  },
};

/** The run as `GET /api/v1/coverage/run` answers once it ended: every event. */
export const COMPLETED: CoverageJob = { ...COMPLETED_END, events: EVENTS };

/** Cancelled while go ran: go's process group killed, custom never started. */
export const CANCELLED_END: CoverageJob = {
  ...RUNNING,
  state: "cancelled",
  elapsed_ms: EVENTS[7].at_ms + 500,
  events: [],
};

/** The step a cancel adds for the tool it killed. */
export const GO_CANCELLED: CoverageJobEvent = {
  seq: 8,
  at_ms: EVENTS[7].at_ms + 480,
  event: { phase: "finished", tool: "go", version: "go1.23.1", outcome: "cancelled" },
};

/** The first run's `409`: every tool that will run, and the custom command. */
export const CONSENT_FIRST: CoverageConsentRequired = {
  error:
    "a coverage run needs consent first: it builds this project and runs its tests (vitest, go, custom), which executes the project's own code and may reach the network",
  consent_required: { tools: ["vitest", "go", "custom"], refresh_cmd: "make cov‮", command_changed: false },
  refresh_cmd_shown: "make cov\\u{202e}",
};

/** The `409` after the committed command changed: the command alone, marked changed. */
export const CONSENT_CHANGED: CoverageConsentRequired = {
  error: "a coverage run needs consent first: it builds this project and runs its tests (custom), which executes the project's own code and may reach the network",
  consent_required: { tools: ["custom"], refresh_cmd: "make coverage-all", command_changed: true },
  refresh_cmd_shown: "make coverage-all",
};

// ── Install and Add dependency (S-750), as `web/tests/coverage_install_job.rs` pins them ──

/** A Rust crate and a pnpm vitest package with no coverage tooling: Logos can
 *  install cargo-llvm-cov and add vitest's provider; Maven it cannot set up. */
export const PREPARABLE_TOOLS: CoverageToolStatus[] = [
  {
    tool: "cargo-llvm-cov",
    state: "not_installed",
    hint: "not on PATH: `logos coverage install --consent` installs it, with the llvm-tools-preview component",
    offer: "install",
    last_run: null,
  },
  {
    tool: "vitest",
    state: "not_installed",
    hint: "no vitest coverage provider (@vitest/coverage-v8 or @vitest/coverage-istanbul) is installed",
    offer: "add_dependency",
    last_run: null,
  },
  { tool: "jest", state: "not_applicable", last_run: null },
  { tool: "pytest-cov", state: "not_applicable", last_run: null },
  { tool: "maven", state: "not_installed", last_run: null },
  { tool: "gradle", state: "not_applicable", last_run: null },
  { tool: "go", state: "not_applicable", last_run: null },
];

/** Add dependency's first `409`: the exact command and files, and the tool it enables. */
export const DEPENDENCY_FIRST: CoveragePrepareRequired = {
  error: "this needs your consent first: each line below is downloaded and run, or changes the project, on this machine",
  consent_required: { tools: ["vitest"], refresh_cmd: null, command_changed: false },
  refresh_cmd_shown: null,
  install: [],
  dependencies: [
    {
      tool: "vitest",
      package: "@vitest/coverage-v8@3",
      manager: "pnpm",
      command: "pnpm add -D @vitest/coverage-v8@3",
      files: ["package.json", "pnpm-lock.yaml"],
    },
  ],
};

/** Install's first `409`: the download and the rustup step, and the tool it enables. */
export const INSTALL_FIRST: CoveragePrepareRequired = {
  error: "this needs your consent first: each line below is downloaded and run, or changes the project, on this machine",
  consent_required: { tools: ["cargo-llvm-cov"], refresh_cmd: null, command_changed: false },
  refresh_cmd_shown: null,
  install: [
    {
      line: "tool",
      tool: "cargo-llvm-cov",
      version: "0.9.1",
      source: "github.com",
      size: 1_488_900,
      text: "cargo-llvm-cov 0.9.1 from github.com (1.5 MB)",
    },
    {
      line: "toolchain",
      command: "rustup component add llvm-tools-preview",
      text: "`rustup component add llvm-tools-preview` on your active Rust toolchain, run only if the component is missing",
    },
  ],
  dependencies: [],
};

const INSTALL_PROGRESS: CoverageRunProgress[] = [
  { phase: "install_started", tool: "cargo-llvm-cov", version: "0.9.1" },
  { phase: "install_finished", tool: "cargo-llvm-cov", version: "0.9.1", outcome: "installed", binary: "/tools/cargo-llvm-cov/0.9.1/cargo-llvm-cov" },
  { phase: "component_started", component: "llvm-tools-preview", command: "rustup component add llvm-tools-preview" },
  { phase: "component_finished", component: "llvm-tools-preview", outcome: "added" },
  { phase: "started", tool: "cargo-llvm-cov" },
];

/** An Install's run while cargo-llvm-cov runs: installed, the component added. */
export const INSTALL_RUNNING: CoverageJob = {
  ...RUNNING,
  id: 6,
  events: INSTALL_PROGRESS.map((event, seq) => ({ seq, at_ms: 30 + seq * 700, event })),
};

/** An Add dependency cancelled while pnpm ran: the files put back. */
export const DEPENDENCY_CANCELLED: CoverageJob = {
  ...RUNNING,
  id: 7,
  state: "cancelled",
  events: [
    { seq: 0, at_ms: 10, event: { phase: "dependency_started", tool: "vitest", command: "pnpm add -D @vitest/coverage-v8@3" } },
    {
      seq: 1,
      at_ms: 900,
      event: {
        phase: "dependency_finished",
        tool: "vitest",
        package: "@vitest/coverage-v8@3",
        manager: "pnpm",
        command: "pnpm add -D @vitest/coverage-v8@3",
        files: ["package.json", "pnpm-lock.yaml"],
        restored: ["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"],
        outcome: "cancelled",
      },
    },
  ],
};
