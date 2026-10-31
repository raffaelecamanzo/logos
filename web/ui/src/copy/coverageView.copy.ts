/*
 * The Coverage view's copy catalogue (S-617, CR-203, FR-UI-39): test coverage
 * from ingested reports. (The workspace's cross-service coverage boards have
 * their own catalogue, `coverage.copy.ts`.) Each absence names the command that
 * fills it (CR-206).
 *
 * S-739 (FR-CV-15, FR-CV-13): the Coverage tools widget, its Run coverage job and
 * the execution-consent dialog. A tool's state is a word — never a 0 % (BR-77).
 *
 * S-750 (FR-CV-17, FR-CV-18): the widget's Install and Add dependency, their lines
 * in the one consent dialog, and the steps a readied run reports first.
 */

import { plural, type CopyEntry } from "./types.ts";

export const untestedHotspots: CopyEntry = {
  what: "The files ranked most at risk — they change often and are complex — that no fresh coverage covers.",
  why: "A risky file with no tests is where a change most likely breaks something unnoticed.",
};

export const perFileCoverage: CopyEntry = {
  what: "Each file's line coverage from the ingested reports; a file changed since its report reads STALE, not a number.",
  why: "A stale figure describes code that has since changed, so it would overstate or understate what the tests cover.",
};

/** The named absences each widget states in its figure row. */
export const COVERAGE_VIEW_ABSENCE = {
  notIngested:
    "No coverage ingested yet; run logos coverage ingest <report> on the LCOV or Cobertura report your test run writes.",
  noUntested: "No untested file among the files ranked most at risk.",
  /** Shown when the read-model carries no notice of its own. */
  notRanked: "No files ranked yet, so no untested file can be named.",
  noFiles: "No covered files in the ingested reports.",
  /** The Coverage tools widget with no tool's ecosystem at the project root. */
  noToolApplies:
    "No coverage tool applies here: the project root has none of the files that mark a Rust, JavaScript, Python, Java or Go project; run logos coverage ingest <report> on a report your own test run writes.",
} as const;

// ── The Coverage tools widget and Run coverage (S-739, FR-CV-15) ─────────────

export const coverageTools: CopyEntry = {
  what: "Each coverage tool that applies to this project: its version, its state — not installed, disabled, never run, ran, ran with failing tests, failed, timed out or cancelled; while a run goes, queued, running, installing or adding dependency — how long its last run took (counting up while it runs), when it last ran and at which commit, and how many files of its last report here matched an indexed file. Below the table, each tool that failed, timed out or had failing tests, with its cause and where its whole output is. Run coverage and Cancel sit on the title row.",
  why: "A tool that is not installed has measured nothing, and a failing suite measured a broken build, so neither reads as 0 %.",
};

/** A tool's state in words, in the table: the stored one, and the live one while
 *  a run goes. */
export const COVERAGE_TOOL_STATE_TEXT = {
  not_applicable: "not applicable",
  disabled: "disabled",
  not_installed: "not installed",
  never_run: "never run",
  queued: "queued",
  running: "running",
  /** While the tool is installed, or its `llvm-tools-preview` step goes (S-750). */
  installing: "installing",
  /** While its coverage dev dependency is added (S-750). */
  adding_dependency: "adding dependency",
  ran: "ran",
  tests_failed: "ran — tests failed",
  failed: "failed",
  timed_out: "timed out",
  cancelled: "cancelled",
} as const;

/** The ecosystem each registered tool covers, for the widget's note on what was
 *  detected at the project root. */
export const COVERAGE_TOOL_ECOSYSTEM: Readonly<Record<string, string>> = {
  "cargo-llvm-cov": "Rust",
  vitest: "JavaScript (vitest)",
  jest: "JavaScript (jest)",
  "pytest-cov": "Python",
  maven: "Java (Maven)",
  gradle: "Java (Gradle)",
  go: "Go",
};

export const COVERAGE_RUN_TEXT = {
  run: "Run coverage",
  cancel: "Cancel",
  /** The Run button's tooltip while no tool here can run: each is missing or
   *  switched off. */
  noToolRunnable: "No coverage tool here is both installed and enabled.",
  /** The widget's note: the ecosystems found, and a configured custom command. */
  detected: (ecosystems: readonly string[], custom: boolean) =>
    `Detected at the project root: ${ecosystems.length > 0 ? ecosystems.join(", ") : "no tool's ecosystem"}.${
      custom ? " A custom command is configured as well." : ""
    }`,
  /** The widget's figure note: installed of the listed tools a run may start;
   *  a disabled one is stated apart, since nothing was looked for. */
  installed: (enabled: number, disabled: number) =>
    `of ${enabled} ${disabled > 0 ? "enabled " : ""}${plural(enabled, "tool", "tools")} installed${
      disabled > 0 ? ` · ${disabled} disabled` : ""
    }`,
  /** The Files cell: matched of reported, from this serve's last run. */
  files: (matched: number, reported: number) => `${matched} of ${reported}`,
  /** The Last run cell: when, and the commit's short hash. */
  lastRun: (age: string, head: string | null) => (head ? `${age} · ${head.slice(0, 7)}` : age),
  log: (path: string) => `Whole output: ${path}`,
  joined: "A coverage run was already going, so this click joined it.",
  started: "Coverage run started.",
  /** The live region, as each tool settles. */
  toolDone: (tool: string, state: string) => `${tool}: ${state}.`,
  completed: (ran: number, failing: number, failed: number) =>
    `Coverage run finished: ${ran} ${plural(ran, "tool", "tools")} ran${
      failing > 0 ? `, ${failing} with failing tests` : ""
    }${failed > 0 ? `, ${failed} failed` : ""}. The figures below are refreshed.`,
  /** `finished` names the tools that settled before the cancel. */
  cancelled: (finished: readonly string[]) =>
    `Coverage run cancelled: every tool process was stopped, and nothing from the cancelled tool is counted.${
      finished.length > 0 ? ` Finished before the cancel: ${finished.join(", ")}.` : ""
    }`,
  failed: (error: string) => `The coverage run failed: ${error}`,
  requestFailed: (detail: string) => `The coverage request failed: ${detail}`,
  lost: "The run's progress stream broke; reading the run again.",
  /** The Elapsed cell: a tool's time in whole seconds — under a second as such,
   *  from a minute on in minutes and seconds. */
  elapsed: (ms: number) => {
    const secs = Math.floor(ms / 1000);
    if (secs < 1) return "<1 s";
    return secs < 60 ? `${secs} s` : `${Math.floor(secs / 60)} min ${secs % 60} s`;
  },
  /** The message area under the table: its name, and a failed run whose record
   *  holds no cause (one written before it was kept). */
  messages: "Coverage tool messages",
  noCause: "No cause was recorded for this run.",
  /** The empty state's offer beside the ingest command. */
  emptyRun: "Or run the project's coverage tools from here:",
  /** The empty state with no tool installed: what would apply, and whether the
   *  widget offers to set any of them up (`offered`). */
  noneInstalled: (tools: readonly string[], offered: boolean) =>
    `No coverage tool is installed: ${tools.join(", ")} would apply here.${
      offered
        ? " Install or Add dependency in the Coverage tools widget sets up the ones Logos can; install the others yourself."
        : " Logos cannot set these up for you: install them yourself."
    }`,
} as const;

/** The execution-consent dialog (FR-CV-13), modelled on the scan's. */
export const COVERAGE_CONSENT_TEXT = {
  title: "Build this project and run its tests?",
  label: "EXECUTES CODE",
  lead: "Logos will build this project and run its tests. This runs the project's own code (build scripts, tests) and may download dependencies.",
  tools: "Tools that will run",
  /** The mark on a tool the recorded consent does not name yet. */
  added: "newly enabled",
  command: "The custom command, exactly as it will run:",
  commandChanged: "This command changed since you last allowed it.",
  where:
    "Your answer is recorded once for this project in your user-level Logos config, never in the repository, and later runs do not ask again. A tool enabled later, or a changed command, is asked about again.",
  accept: "Allow and run",
  decline: "Cancel",
} as const;

// ── Install and Add dependency (S-750, FR-CV-17, FR-CV-18) ──────────────────

/** The widget's two offers, on a not-installed tool's row. */
export const COVERAGE_OFFER_TEXT = {
  install: "Install",
  addDependency: "Add dependency",
  /** The accessible names, which say what each does for which tool. */
  installLabel: (tool: string) => `Install ${tool}, then run coverage`,
  addDependencyLabel: (tool: string) => `Add ${tool}'s coverage dependency, then run coverage`,
} as const;

/** The consent dialog's install and dependency sections. */
export const COVERAGE_PREPARE_CONSENT_TEXT = {
  installTitle: "Install the coverage tool, then run coverage?",
  dependencyTitle: "Add the coverage dependency, then run coverage?",
  installLead: "Logos will download, verify and install, then run coverage:",
  installLines: "What will be installed",
  installWhere:
    "Each download is checked against the SHA-256 pinned in this Logos release before anything from it runs, and goes into your user-level tools directory. llvm-tools-preview is added by your own rustup. Your answer is recorded once for your user, never in the repository, and covers these versions in every project.",
  dependencyLead: "Logos will run this command in the project, exactly as shown, then run coverage:",
  command: "The command",
  files: (files: readonly string[]) => `It changes ${files.join(", ")}.`,
  scripts:
    "The package manager may run dependency lifecycle scripts (install scripts), which execute code from the packages it fetches, and it may reach the network.",
  scriptsLabel: "RUNS SCRIPTS",
  dependencyWhere:
    "Logos never commits: the change stays in your working tree for you to review. Your answer is recorded once for this project in your user-level Logos config, never in the repository; a different command is asked about again.",
  acceptInstall: "Install and run",
  acceptDependency: "Add and run",
} as const;

/** The steps a readied run reports before its tools. */
export const COVERAGE_PREPARE_TEXT = {
  state: {
    running: "in progress",
    installed: "installed",
    already_present: "already present",
    added: "added",
    no_rustup: "no rustup",
    unavailable: "unavailable",
    pending: "awaiting consent",
    refused: "refused",
    failed: "failed",
    timed_out: "timed out",
    cancelled: "cancelled",
  },
  /** The live line, as a step changes. */
  line: (what: string, state: string) => `${what}: ${state}.`,
  putBack: (files: readonly string[]) => `Put back as it was: ${files.join(", ")}.`,
  byHand: (reason: string, command: string) => `${reason} — run by hand: ${command}`,
} as const;
