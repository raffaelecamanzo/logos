// The Coverage page's run model (S-739, FR-CV-15, FR-CV-11): the per-tool rows of
// a followed run, from the snapshot plus the streamed steps — a tool not started
// is queued, the one started runs, each settled one reads its outcome in words,
// and a cancel leaves every unsettled tool cancelled; tools whose ecosystem is
// absent never appear. Steps merge by `seq`, and the live line says what changed.
import { describe, expect, it } from "vitest";

import type { CoverageJobEvent } from "../../api/index.ts";
import { COVERAGE_PREPARE_TEXT, COVERAGE_RUN_TEXT } from "../../copy/coverageView.copy.ts";
import {
  announcement,
  detectedEcosystems,
  elapsedMs,
  filesOf,
  listedTools,
  liveTools,
  toolMessages,
  NO_RUN,
  prepareRows,
  progressRows,
  runnable,
  withJob,
  withSteps,
  type PrepareRow,
  type RunView,
} from "./coverageRunModel.ts";
import {
  CANCELLED_END,
  COMPLETED,
  COMPLETED_END,
  DEPENDENCY_CANCELLED,
  EVENTS,
  GO_CANCELLED,
  INSTALL_RUNNING,
  PREPARABLE_TOOLS,
  RUNNING,
  TOOLS,
  TOOLS_AFTER,
} from "./fixtures/coverageRun.ts";

const EXPECTED = TOOLS.filter(runnable).map((t) => t.tool);

const states = (view: RunView) => progressRows(view, TOOLS).map((r) => `${r.tool}:${r.state}`);

describe("the tools a run starts and the widget lists", () => {
  it("runs only installed, enabled tools and lists every tool whose ecosystem is here", () => {
    expect(EXPECTED).toEqual(["vitest", "go", "custom"]);
    expect(listedTools(TOOLS).map((t) => t.tool)).toEqual(["vitest", "maven", "go", "custom"]);
    expect(detectedEcosystems(TOOLS)).toEqual(["JavaScript (vitest)", "Java (Maven)", "Go"]);
  });
});

describe("progressRows", () => {
  it("lists nothing before any run", () => {
    expect(progressRows(NO_RUN, TOOLS)).toEqual([]);
  });

  it("reads a running job: settled, running, queued — and a missing tool as not installed", () => {
    const view = withJob(NO_RUN, RUNNING);
    expect(states(view)).toEqual(["vitest:tests_failed", "maven:not_installed", "go:running", "custom:queued"]);
    const go = progressRows(view, TOOLS).find((r) => r.tool === "go");
    expect(go?.startedMs).toBe(EVENTS[7].at_ms);
  });

  it("queues every expected tool when the run has reported nothing yet", () => {
    const view = withJob(NO_RUN, { ...RUNNING, events: [] });
    expect(states(view)).toEqual(["vitest:queued", "go:queued", "custom:queued"]);
  });

  it("leaves out a tool a completed run never reported, and a tool reported by no status row is appended", () => {
    const view = withJob(NO_RUN, { ...COMPLETED, events: EVENTS.filter((e) => !("tool" in e.event) || e.event.tool !== "custom") });
    expect(states(view)).toEqual(["vitest:tests_failed", "maven:not_installed", "go:ran"]);
    const extra = withSteps(withJob(NO_RUN, RUNNING), [{ seq: 8, at_ms: 9000, event: { phase: "started", tool: "bazel" } }]);
    expect(states(extra).at(-1)).toBe("bazel:running");
  });

  it("reads a completed run's outcomes in words, never a not-applicable tool", () => {
    const view = withJob(withJob(NO_RUN, RUNNING), COMPLETED_END);
    expect(view.steps).toHaveLength(8);
    const done = withSteps(view, EVENTS);
    expect(states(done)).toEqual(["vitest:tests_failed", "maven:not_installed", "go:ran", "custom:failed"]);
  });

  it("leaves every unsettled tool cancelled once the run is cancelled", () => {
    const view = withJob(withSteps(withJob(NO_RUN, RUNNING), [GO_CANCELLED]), CANCELLED_END);
    expect(states(view)).toEqual(["vitest:tests_failed", "maven:not_installed", "go:cancelled", "custom:cancelled"]);
    // Without the cancelled step the started tool reads cancelled too.
    expect(states(withJob(withJob(NO_RUN, RUNNING), CANCELLED_END))).toContain("go:cancelled");
  });

  it("reads an unsettled tool of a failed job as failed", () => {
    const view = withJob(NO_RUN, { ...RUNNING, state: "failed", error: "the config is invalid" });
    expect(states(view)).toEqual(["vitest:tests_failed", "maven:not_installed", "go:failed", "custom:failed"]);
  });
});

describe("withJob and withSteps", () => {
  it("keeps each seq once, in order, whatever the arrival order", () => {
    const view = withSteps(withJob(NO_RUN, { ...RUNNING, events: [EVENTS[2]] }), [EVENTS[1], EVENTS[2], EVENTS[0]]);
    expect(view.steps.map((s) => s.seq)).toEqual([0, 1, 2]);
    expect(withSteps(view, [EVENTS[1]])).toBe(view);
  });

  it("starts over for another job, and forgets a job that is gone", () => {
    const first = withJob(NO_RUN, COMPLETED);
    const next = withJob(first, { ...RUNNING, id: 5, events: [] });
    expect(next.steps).toEqual([]);
    expect(withJob(first, null)).toEqual(NO_RUN);
  });
});

describe("filesOf", () => {
  it("counts matched of reported files over every ingested report", () => {
    const vitest = progressRows(withJob(NO_RUN, COMPLETED), TOOLS).find((r) => r.tool === "vitest");
    expect(filesOf(vitest?.run ?? null)).toEqual({ matched: 12, reported: 13 });
  });

  it("sums every report of a multi-module run (review)", () => {
    const vitest = progressRows(withJob(NO_RUN, COMPLETED), TOOLS).find((r) => r.tool === "vitest")!.run!;
    if (vitest.outcome !== "tests_failed") throw new Error("the fixture's vitest run failed its tests");
    const second = { ...vitest.ingests[0], matched_files: 1, unmatched: ["gen/a.js", "gen/b.js"] };
    expect(filesOf({ ...vitest, ingests: [vitest.ingests[0], second] })).toEqual({ matched: 13, reported: 16 });
  });

  it("has no count for a tool that ingested nothing", () => {
    const rows = progressRows(withJob(NO_RUN, COMPLETED), TOOLS);
    expect(filesOf(rows.find((r) => r.tool === "custom")?.run ?? null)).toBeNull();
    expect(filesOf(null)).toBeNull();
  });
});

describe("announcement — the polite live line", () => {
  const say = (view: RunView) => announcement(view, progressRows(view, TOOLS));

  it("says nothing before a run and that it started before a tool settled", () => {
    expect(say(NO_RUN)).toBe("");
    expect(say(withJob(NO_RUN, { ...RUNNING, events: [EVENTS[0], EVENTS[1]] }))).toBe(COVERAGE_RUN_TEXT.started);
  });

  it("names the last tool that settled, never one that does not apply", () => {
    expect(say(withJob(NO_RUN, RUNNING))).toBe("maven: not installed.");
    expect(say(withJob(NO_RUN, { ...RUNNING, events: EVENTS.slice(0, 4) }))).toBe("vitest: ran — tests failed.");
  });

  it("states the end of the run: completed, cancelled, failed", () => {
    expect(say(withJob(NO_RUN, COMPLETED))).toBe(COVERAGE_RUN_TEXT.completed(2, 1, 1));
    expect(say(withJob(NO_RUN, COMPLETED))).toBe(
      "Coverage run finished: 2 tools ran, 1 with failing tests, 1 failed. The figures below are refreshed.",
    );
    const cancelled = withJob(withSteps(withJob(NO_RUN, RUNNING), [GO_CANCELLED]), CANCELLED_END);
    expect(say(cancelled)).toBe(COVERAGE_RUN_TEXT.cancelled(["vitest"]));
    expect(say(withJob(NO_RUN, { ...RUNNING, events: [], state: "failed", error: "the config is invalid" }))).toBe(
      "The coverage run failed: the config is invalid",
    );
  });
});

describe("what a prepared run readies first (S-750)", () => {
  const rows = (view: RunView) => prepareRows(view).map((r) => `${r.what}:${r.state}`);

  it("lists the install and the llvm-tools-preview step as they end, and the tool rows read only the tools", () => {
    const view = withJob(NO_RUN, INSTALL_RUNNING);
    expect(rows(view)).toEqual(["cargo-llvm-cov 0.9.1:installed", "llvm-tools-preview:added"]);
    expect(progressRows(view, PREPARABLE_TOOLS).map((r) => `${r.tool}:${r.state}`)).toEqual(["cargo-llvm-cov:running"]);
    expect(announcement(view, progressRows(view, PREPARABLE_TOOLS))).toBe(COVERAGE_PREPARE_TEXT.line("llvm-tools-preview", "added"));
  });

  it("reads a step still going as the job ended, and a run that readied nothing as none", () => {
    const going = { ...INSTALL_RUNNING, events: INSTALL_RUNNING.events.slice(0, 1) };
    expect(rows(withJob(NO_RUN, going))).toEqual(["cargo-llvm-cov 0.9.1:running"]);
    expect(announcement(withJob(NO_RUN, going), [])).toBe(COVERAGE_PREPARE_TEXT.line("cargo-llvm-cov 0.9.1", "in progress"));
    expect(rows(withJob(NO_RUN, { ...going, state: "cancelled" }))).toEqual(["cargo-llvm-cov 0.9.1:cancelled"]);
    expect(rows(withJob(NO_RUN, { ...going, state: "failed" }))).toEqual(["cargo-llvm-cov 0.9.1:failed"]);
    expect(prepareRows(withJob(NO_RUN, COMPLETED))).toEqual([]);
    expect(prepareRows(NO_RUN)).toEqual([]);
  });

  it("names the command of a cancelled addition and what was put back", () => {
    const [row] = prepareRows(withJob(NO_RUN, DEPENDENCY_CANCELLED));
    expect(row).toMatchObject({ what: "pnpm add -D @vitest/coverage-v8@3", state: "cancelled" });
    expect(row.detail).toBe(COVERAGE_PREPARE_TEXT.putBack(["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"]));
  });

  it("says why an addition failed and what it put back, and why the rustup step failed (review)", () => {
    const failed = {
      ...RUNNING,
      events: [
        {
          seq: 0,
          at_ms: 1,
          event: {
            phase: "dependency_finished" as const,
            tool: "vitest",
            package: "@vitest/coverage-v8@3",
            command: "pnpm add -D @vitest/coverage-v8@3",
            restored: ["package.json", "pnpm-lock.yaml"],
            outcome: "failed" as const,
            exit_code: 1,
            stderr: "ERR_PNPM_FETCH_404",
            log: ".logos/coverage/vitest.dependency.failed.log",
          },
        },
        { seq: 1, at_ms: 2, event: { phase: "component_finished" as const, component: "llvm-tools-preview", outcome: "failed" as const, reason: "toolchain is not installed" } },
      ],
    };
    expect(prepareRows(withJob(NO_RUN, failed)).map((r) => r.detail)).toEqual([
      `ERR_PNPM_FETCH_404 ${COVERAGE_PREPARE_TEXT.putBack(["package.json", "pnpm-lock.yaml"])}`,
      "toolchain is not installed",
    ]);
  });

  it("carries why a step failed, and the command to run by hand", () => {
    const failed = {
      ...RUNNING,
      events: [
        { seq: 0, at_ms: 1, event: { phase: "install_finished" as const, tool: "cargo-llvm-cov", version: "0.9.1", outcome: "failed" as const, stage: "checksum", reason: "the SHA-256 does not match" } },
        { seq: 1, at_ms: 2, event: { phase: "component_finished" as const, component: "llvm-tools-preview", outcome: "no_rustup" as const, reason: "no rustup on the PATH", command: "rustup component add llvm-tools-preview" } },
      ],
    };
    expect(prepareRows(withJob(NO_RUN, failed)).map((r) => r.detail)).toEqual([
      "the SHA-256 does not match",
      COVERAGE_PREPARE_TEXT.byHand("no rustup on the PATH", "rustup component add llvm-tools-preview"),
    ]);
  });
});

// ── The table carries the run (S-803, CR-250) ────────────────────────────────

describe("liveTools — each tool's live State while a run goes", () => {
  const live = (job: Parameters<typeof withJob>[1], tools = TOOLS) => {
    const view = withJob(NO_RUN, job);
    return liveTools(view, progressRows(view, tools), prepareRows(view));
  };
  const words = (m: ReturnType<typeof live>) => [...m].map(([tool, l]) => `${tool}:${l.state}`);

  it("is each tool's progress state while the job runs: queued, running, or its outcome", () => {
    expect(words(live(RUNNING))).toEqual(["vitest:tests_failed", "maven:not_installed", "go:running", "custom:queued"]);
  });

  it("is empty once the job is not running — the stored state is shown again (HF-6)", () => {
    for (const job of [COMPLETED, CANCELLED_END, { ...RUNNING, state: "failed" as const }]) expect(live(job).size).toBe(0);
    expect(liveTools(NO_RUN, [], []).size).toBe(0);
  });

  it("gives a tool being readied installing or adding dependency, then queued once readied, then its own run", () => {
    const [started, finished, componentStarted, componentFinished] = INSTALL_RUNNING.events;
    const at = (events: typeof INSTALL_RUNNING.events) => live({ ...INSTALL_RUNNING, events }, PREPARABLE_TOOLS).get("cargo-llvm-cov");
    expect(at([started])).toEqual({ state: "installing", startedMs: started.at_ms, endedMs: null });
    expect(at([started, finished])?.state).toBe("queued");
    // The component step belongs to the tool it prepares.
    expect(at([started, finished, componentStarted])).toEqual({ state: "installing", startedMs: componentStarted.at_ms, endedMs: null });
    expect(at([started, finished, componentStarted, componentFinished])?.state).toBe("queued");
    expect(at(INSTALL_RUNNING.events)?.state).toBe("running");
    const adding = live({ ...DEPENDENCY_CANCELLED, state: "running", events: DEPENDENCY_CANCELLED.events.slice(0, 1) }, PREPARABLE_TOOLS);
    expect(adding.get("vitest")?.state).toBe("adding_dependency");
  });

  it("leaves a tool whose readying step did not succeed to its stored state", () => {
    const [started] = INSTALL_RUNNING.events;
    const failed = {
      seq: 1,
      at_ms: 50,
      event: { phase: "install_finished" as const, tool: "cargo-llvm-cov", version: "0.9.1", outcome: "failed" as const, stage: "download", reason: "offline" },
    };
    expect(live({ ...INSTALL_RUNNING, events: [started, failed] }, PREPARABLE_TOOLS).has("cargo-llvm-cov")).toBe(false);
  });

  it("keeps when a tool started and ended in the job, from the steps", () => {
    const go = live(RUNNING).get("vitest");
    expect(go).toEqual({ state: "tests_failed", startedMs: EVENTS[1].at_ms, endedMs: EVENTS[2].at_ms });
  });
});

describe("elapsedMs — a tool's Elapsed", () => {
  const tool = TOOLS_AFTER.find((t) => t.tool === "vitest")!;

  it("is the recorded duration when the tool is not live, and none when nothing is recorded", () => {
    expect(elapsedMs(tool, undefined, null)).toBe(4_200);
    expect(elapsedMs(TOOLS_AFTER.find((t) => t.tool === "maven")!, undefined, 9_000)).toBeNull();
    const old = { ...tool, last_run: { ...tool.last_run!, duration_ms: undefined } };
    expect(elapsedMs(old, undefined, null)).toBeNull();
  });

  it("counts from the start while live, stops at the end once it settled, and is none before it starts", () => {
    expect(elapsedMs(tool, { state: "running", startedMs: 1_000, endedMs: null }, 4_500)).toBe(3_500);
    expect(elapsedMs(tool, { state: "ran", startedMs: 1_000, endedMs: 2_500 }, 9_000)).toBe(1_500);
    expect(elapsedMs(tool, { state: "queued", startedMs: null, endedMs: null }, 9_000)).toBeNull();
    expect(elapsedMs(tool, { state: "running", startedMs: 1_000, endedMs: null }, null)).toBeNull();
    // The clock never runs it backwards.
    expect(elapsedMs(tool, { state: "running", startedMs: 5_000, endedMs: null }, 4_000)).toBe(0);
  });
});

describe("toolMessages — the cause below the table", () => {
  const kinds = (m: ReturnType<typeof toolMessages>) => m.map((x) => `${x.tool}:${x.state}`);

  it("holds the failed, timed-out and tests-failed tools of the stored record, in table order, nothing else", () => {
    expect(kinds(toolMessages(TOOLS_AFTER, [], new Map()))).toEqual(["vitest:tests_failed", "custom:failed"]);
    expect(toolMessages(TOOLS, [], new Map())).toEqual([]);
    const [vitest] = toolMessages(TOOLS_AFTER, [], new Map());
    expect(vitest).toMatchObject({ kind: "run", cause: expect.stringContaining("totals the cart"), log: null });
  });

  it("carries a missing cause as null and a missing log as null, never an empty text", () => {
    const bare = TOOLS_AFTER.map((t) => (t.last_run ? { ...t, last_run: { ...t.last_run, cause: undefined, log: null } } : t));
    expect(toolMessages(bare, [], new Map()).map((m) => (m.kind === "run" ? [m.cause, m.log] : null))).toEqual([[null, null], [null, null]]);
  });

  it("leaves out a tool live in the run going, and adds a readying step that failed or was refused", () => {
    const view = withJob(NO_RUN, RUNNING);
    const live = liveTools(view, progressRows(view, TOOLS_AFTER), prepareRows(view));
    expect(toolMessages(TOOLS_AFTER, [], live)).toEqual([]);
    const refused = {
      key: "dependency:vitest", tool: "vitest", kind: "dependency" as const, what: "pnpm add", state: "refused" as const, detail: "run by hand", startedMs: 0,
    };
    // A cancel that changed nothing says nothing; one that put files back names them.
    const quiet = { ...refused, key: "x", state: "cancelled" as const, detail: null };
    const putBack = { ...quiet, key: "y", detail: "Put back as it was: package.json." };
    expect(toolMessages([], [refused, quiet, putBack], new Map()).map((m) => (m.kind === "prepare" ? `${m.key}:${m.state}` : m.tool))).toEqual([
      "dependency:vitest:refused",
      "y:cancelled",
    ]);
  });
});

describe("COVERAGE_RUN_TEXT.elapsed — the Elapsed cell's words (S-803)", () => {
  it.each([
    [0, "<1 s"],
    [999, "<1 s"],
    [1_000, "1 s"],
    [4_200, "4 s"],
    [59_999, "59 s"],
    [60_000, "1 min 0 s"],
    [125_000, "2 min 5 s"],
  ])("reads %i ms as %s", (ms, words) => {
    expect(COVERAGE_RUN_TEXT.elapsed(ms)).toBe(words);
  });
});

describe("which readying outcomes let a tool go on, and which the message area states (S-803)", () => {
  const readyingJob = (events: CoverageJobEvent["event"][]) => ({
    ...INSTALL_RUNNING,
    events: events.map((event, seq) => ({ seq, at_ms: 100 + seq * 100, event })),
  });
  const liveOf = (events: CoverageJobEvent["event"][]) => {
    const view = withJob(NO_RUN, readyingJob(events));
    return liveTools(view, progressRows(view, PREPARABLE_TOOLS), prepareRows(view)).get("cargo-llvm-cov")?.state;
  };
  const install = { phase: "install_started", tool: "cargo-llvm-cov", version: "0.9.1" } as const;
  const component = { phase: "component_started", component: "llvm-tools-preview", command: "rustup component add llvm-tools-preview" } as const;

  it("reads queued after an install that found the tool already present", () => {
    expect(
      liveOf([install, { phase: "install_finished", tool: "cargo-llvm-cov", version: "0.9.1", outcome: "already_present", binary: "/tools/cargo-llvm-cov" }]),
    ).toBe("queued");
  });

  it("keeps the stored state of a tool with one step done and another not: every step must have ended well", () => {
    const installed = { phase: "install_finished", tool: "cargo-llvm-cov", version: "0.9.1", outcome: "installed", binary: "/tools/cargo-llvm-cov" } as const;
    const noRustup = { phase: "component_finished", component: "llvm-tools-preview", outcome: "no_rustup", reason: "rustup was not found", command: "rustup component add llvm-tools-preview" } as const;
    expect(liveOf([install, installed, component, noRustup])).toBeUndefined();
  });

  it("states a readying step that did not end well, and no step that did or still goes", () => {
    const step = (state: PrepareRow["state"]): PrepareRow => ({
      key: `install:${state}`, tool: "cargo-llvm-cov", kind: "install", what: "cargo-llvm-cov 0.9.1", state, detail: null, startedMs: 0,
    });
    const stated = (states: PrepareRow["state"][]) =>
      toolMessages([], states.map(step), new Map()).map((m) => (m.kind === "prepare" ? m.state : "run"));
    expect(stated(["failed", "unavailable", "timed_out", "refused", "no_rustup"])).toEqual(["failed", "unavailable", "timed_out", "refused", "no_rustup"]);
    expect(stated(["installed", "already_present", "added", "running", "pending"])).toEqual([]);
  });
});

describe("toolMessages lists only the tools the table lists (S-803)", () => {
  it("leaves out a tool whose ecosystem is not here, whatever its stored record says", () => {
    const tools = TOOLS_AFTER.map((t) =>
      t.tool === "jest" ? { ...t, last_run: { outcome: "failed" as const, exit_code: 1, head_sha: null, finished_at: 1, log: null, cause: "stale" } } : t,
    );
    expect(tools.find((t) => t.tool === "jest")?.state).toBe("not_applicable");
    expect(toolMessages(tools, [], new Map()).map((m) => (m.kind === "run" ? m.tool : m.key))).toEqual(["vitest", "custom"]);
  });
});

describe("the detail a readying step carries (S-803)", () => {
  const detailsOf = (events: CoverageJobEvent["event"][]) =>
    prepareRows(withJob(NO_RUN, { ...INSTALL_RUNNING, state: "completed", events: events.map((event, seq) => ({ seq, at_ms: 10 + seq, event })) })).map(
      (r) => r.detail,
    );

  it("gives an install that was unavailable its reason, and a refused addition the command to run by hand", () => {
    expect(
      detailsOf([{ phase: "install_finished", tool: "cargo-llvm-cov", version: "0.9.1", outcome: "unavailable", reason: "no release for this platform" }]),
    ).toEqual(["no release for this platform"]);
    expect(
      detailsOf([
        {
          phase: "dependency_finished",
          tool: "vitest",
          package: "@vitest/coverage-v8@3",
          outcome: "refused",
          reason: "no lockfile names the package manager",
          hand_command: "npm install --save-dev @vitest/coverage-v8@3",
        },
      ]),
    ).toEqual([COVERAGE_PREPARE_TEXT.byHand("no lockfile names the package manager", "npm install --save-dev @vitest/coverage-v8@3")]);
  });
});
