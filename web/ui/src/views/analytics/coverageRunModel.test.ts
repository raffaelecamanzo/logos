// The Coverage page's run model (S-739, FR-CV-15, FR-CV-11): the progress list of
// a followed run, from the snapshot plus the streamed steps — a tool not started
// is queued, the one started runs, each settled one reads its outcome in words,
// and a cancel leaves every unsettled tool cancelled; tools whose ecosystem is
// absent never appear. Steps merge by `seq`, and the live line says what changed.
import { describe, expect, it } from "vitest";

import { COVERAGE_PREPARE_TEXT, COVERAGE_RUN_TEXT } from "../../copy/coverageView.copy.ts";
import {
  announcement,
  detectedEcosystems,
  filesOf,
  listedTools,
  NO_RUN,
  prepareRows,
  progressRows,
  runnable,
  withJob,
  withSteps,
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
