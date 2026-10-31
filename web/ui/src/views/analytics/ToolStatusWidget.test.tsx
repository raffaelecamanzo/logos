// The Coverage tools widget (S-739, FR-CV-15, FR-CV-11; frontend-design §4.10):
// one row per tool whose ecosystem is at the project root, each state a word —
// a tool that is not installed, or a failing suite, never reads as 0 % — with its
// version, last run and matched-of-reported files; the explanation names the
// ecosystems detected; Run coverage (Cancel while a run goes) sits on the title
// row; with no tool's ecosystem here the figure row states that absence.
// S-803 (CR-250): the table carries the run — no progress list; each tool's State
// is live while a run goes (queued, running, its outcome, installing, adding
// dependency), Elapsed counts each second and then keeps the recorded
// `duration_ms` (`—` when none), and a message area under the table lists each
// failed, timed-out or tests-failed tool with its cause and log path.
import { cleanup, render, screen, within } from "@testing-library/react";
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  COVERAGE_OFFER_TEXT,
  COVERAGE_PREPARE_TEXT,
  COVERAGE_RUN_TEXT,
  COVERAGE_TOOL_STATE_TEXT,
  COVERAGE_VIEW_ABSENCE,
  coverageTools,
} from "../../copy/coverageView.copy.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import type { CoverageJob } from "../../api/index.ts";
import type { CoverageRunHandle } from "./CoverageRunPanel.tsx";
import { NO_RUN, prepareRows, progressRows, withJob, type PrepareRow } from "./coverageRunModel.ts";
import {
  COMPLETED,
  DEPENDENCY_CANCELLED,
  EVENTS,
  INSTALL_RUNNING,
  PREPARABLE_TOOLS,
  RUNNING,
  TOOLS,
  TOOLS_AFTER,
} from "./fixtures/coverageRun.ts";
import { ToolStatusWidget } from "./ToolStatusWidget.tsx";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

/** A run handle in the given state; no request is made from it. */
function handle(over: Partial<CoverageRunHandle> = {}): CoverageRunHandle {
  return {
    view: NO_RUN,
    rows: [],
    prepared: [],
    running: false,
    joined: false,
    canRun: true,
    alert: null,
    run: vi.fn(),
    install: vi.fn(),
    addDependency: vi.fn(),
    cancel: vi.fn(),
    consent: { required: null, action: { kind: "run" }, willRun: [], consentedBefore: false, accept: vi.fn(), decline: vi.fn() },
    ...over,
  };
}

/** NOW is 5 minutes after the fixture's runs ended. */
const NOW = 1_761_170_300;

/** The cells of `tool`'s row, by column header. */
function rowOf(tool: string): Record<string, string> {
  const table = screen.getByRole("table", { name: "Coverage tools" });
  const headers = within(table)
    .getAllByRole("columnheader")
    .map((h) => (h.textContent ?? "").replace(/[↑↓↕]/g, "").trim());
  const row = within(table).getByText(tool, { selector: "td" }).closest("tr") as HTMLElement;
  const cells = [...row.querySelectorAll("td")].map((td) => td.textContent ?? "");
  return Object.fromEntries(headers.map((h, i) => [h, cells[i]]));
}

/** The class of the badge in `tool`'s State cell: its tone, through the CSS-modules proxy. */
const toneOf = (tool: string) => {
  const table = screen.getByRole("table", { name: "Coverage tools" });
  const row = within(table).getByText(tool, { selector: "td" }).closest("tr") as HTMLElement;
  return row.querySelectorAll("td")[2].querySelector("span")?.className ?? "";
};

describe("ToolStatusWidget (S-739)", () => {
  it("lists every tool whose ecosystem is here, never a not-applicable one", () => {
    const { container } = render(<ToolStatusWidget tools={TOOLS} run={handle()} nowUnix={NOW} />);
    const table = screen.getByRole("table", { name: "Coverage tools" });
    const names = within(table)
      .getAllByRole("row")
      .slice(1)
      .map((r) => r.querySelector("td")?.textContent);
    expect(names).toEqual(["vitest", "maven", "go", "custom"]);
    expectWidgetCopy(container.querySelector("[data-widget]")!, coverageTools);
  });

  it("states the tools installed of those listed, and the ecosystems detected", () => {
    const { container } = render(<ToolStatusWidget tools={TOOLS} run={handle()} nowUnix={NOW} />);
    expect(container.querySelector('[data-widget-part="figure"]')).toHaveTextContent("3 of 4 tools installed");
    expect(container).toHaveTextContent(
      "Detected at the project root: JavaScript (vitest), Java (Maven), Go. A custom command is configured as well.",
    );
  });

  it("states a disabled tool apart from the installed figure — nothing was looked for, so it is not 'not installed'", () => {
    const tools = TOOLS.map((t) => (t.tool === "go" ? { ...t, state: "disabled" as const, version: undefined } : t));
    const { container } = render(<ToolStatusWidget tools={tools} run={handle()} nowUnix={NOW} />);
    expect(container.querySelector('[data-widget-part="figure"]')).toHaveTextContent("2 of 3 enabled tools installed · 1 disabled");
    expect(rowOf("go").State).toBe(COVERAGE_TOOL_STATE_TEXT.disabled);
  });

  it("reads a missing tool as not installed and a never-run one as never run — never a figure", () => {
    render(<ToolStatusWidget tools={TOOLS} run={handle()} nowUnix={NOW} />);
    expect(rowOf("maven")).toEqual({ Tool: "maven", Version: "—", State: "not installed", Elapsed: "—", "Last run": "—", Files: "—" });
    expect(rowOf("vitest")).toEqual({ Tool: "vitest", Version: "3.2.4", State: "never run", Elapsed: "—", "Last run": "—", Files: "—" });
    // A copy outside the project reads with its version unknown: the page's read starts no tool.
    expect(rowOf("go").Version).toBe("—");
  });

  it("shows a not-installed tool's hint", () => {
    const tools = TOOLS.map((t) => (t.tool === "maven" ? { ...t, hint: "install the project's dependencies first" } : t));
    render(<ToolStatusWidget tools={tools} run={handle()} nowUnix={NOW} />);
    expect(rowOf("maven").State).toBe("not installedinstall the project's dependencies first");
  });

  it("reads a failing suite as ran — tests failed, each last run's recorded duration, age and commit — and no log path in State", () => {
    const view = withJob(NO_RUN, COMPLETED);
    render(<ToolStatusWidget tools={TOOLS_AFTER} run={handle({ view, rows: progressRows(view, TOOLS_AFTER) })} nowUnix={NOW} />);
    expect(rowOf("vitest")).toEqual({
      Tool: "vitest",
      Version: "3.2.4",
      State: "ran — tests failed",
      Elapsed: "4 s",
      "Last run": "5m ago · 4f9c2e1",
      Files: "12 of 13",
    });
    expect(rowOf("go")).toMatchObject({ State: "ran", Elapsed: "1 s", Files: "1 of 1" });
    expect(rowOf("custom")).toMatchObject({ State: "failed", Elapsed: "<1 s", Files: "—" });
    for (const tool of ["vitest", "maven", "go", "custom"]) {
      expect(rowOf(tool).State).not.toMatch(/\d\s*%/);
      expect(rowOf(tool).State).not.toContain("Whole output");
    }
  });

  it("shows — for a tool with no duration recorded: never run, or a record written before it was kept (S-802)", () => {
    const old = TOOLS_AFTER.map((t) =>
      t.last_run ? { ...t, last_run: { ...t.last_run, duration_ms: undefined, cause: undefined } } : t,
    );
    render(<ToolStatusWidget tools={old} run={handle()} nowUnix={NOW} />);
    expect(rowOf("go")).toMatchObject({ State: "ran", Elapsed: "—" });
    expect(rowOf("vitest").Elapsed).toBe("—");
    expect(rowOf("maven").Elapsed).toBe("—");
  });

  it("puts Run coverage on the title row, disabled while no tool here can run", () => {
    const run = handle();
    const { unmount } = render(<ToolStatusWidget tools={TOOLS} run={run} nowUnix={NOW} />);
    const button = screen.getByRole("button", { name: COVERAGE_RUN_TEXT.run });
    expect(button.closest('[data-widget-part="title"]')).not.toBeNull();
    button.click();
    expect(run.run).toHaveBeenCalledTimes(1);
    unmount();
    render(<ToolStatusWidget tools={TOOLS} run={handle({ canRun: false })} nowUnix={NOW} />);
    const disabled = screen.getByRole("button", { name: COVERAGE_RUN_TEXT.run });
    expect(disabled).toBeDisabled();
    expect(disabled).toHaveAttribute("title", COVERAGE_RUN_TEXT.noToolRunnable);
  });

  it("turns the button into Cancel while a run goes, with no progress list: the table's State carries the run", () => {
    const view = withJob(NO_RUN, RUNNING);
    const run = handle({ view, rows: progressRows(view, TOOLS), running: true });
    render(<ToolStatusWidget tools={TOOLS} run={run} nowUnix={NOW} />);
    expect(screen.queryByRole("button", { name: COVERAGE_RUN_TEXT.run })).toBeNull();
    screen.getByRole("button", { name: COVERAGE_RUN_TEXT.cancel }).click();
    expect(run.cancel).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("list", { name: "Coverage run progress" })).toBeNull();
    expect(["vitest", "maven", "go", "custom"].map((t) => rowOf(t).State)).toEqual([
      "ran — tests failed",
      "not installed",
      "running",
      "queued",
    ]);
  });

  it("keeps the polite live line above the table, announcing each tool's completion and the run's end", () => {
    const view = withJob(NO_RUN, RUNNING);
    const { container, rerender } = render(
      <ToolStatusWidget tools={TOOLS} run={handle({ view, rows: progressRows(view, TOOLS), running: true })} nowUnix={NOW} />,
    );
    const live = container.querySelector("[aria-live='polite']") as HTMLElement;
    expect(live).toHaveTextContent("maven: not installed.");
    const table = screen.getByRole("table", { name: "Coverage tools" });
    expect(live.compareDocumentPosition(table) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const done = withJob(NO_RUN, COMPLETED);
    rerender(<ToolStatusWidget tools={TOOLS_AFTER} run={handle({ view: done, rows: progressRows(done, TOOLS_AFTER) })} nowUnix={NOW} />);
    expect(container.querySelector("[aria-live='polite']")).toHaveTextContent(COVERAGE_RUN_TEXT.completed(2, 1, 1));
  });

  it("counts a running tool's Elapsed from its own start, once a second", () => {
    vi.useFakeTimers();
    const goStarted = EVENTS[7].at_ms;
    const view = withJob(NO_RUN, { ...RUNNING, elapsed_ms: goStarted + 3_000 });
    const run = handle({ view, rows: progressRows(view, TOOLS), running: true });
    render(<ToolStatusWidget tools={TOOLS} run={run} nowUnix={NOW} />);
    expect(rowOf("go").Elapsed).toBe("3 s");
    act(() => vi.advanceTimersByTime(2_000));
    expect(rowOf("go").Elapsed).toBe("5 s");
    // A queued tool has not started: nothing to count.
    expect(rowOf("custom")).toMatchObject({ State: "queued", Elapsed: "—" });
  });

  it("keeps a tool's own elapsed once it settles in this run, and shows its live outcome", () => {
    const view = withJob(NO_RUN, RUNNING);
    render(<ToolStatusWidget tools={TOOLS} run={handle({ view, rows: progressRows(view, TOOLS), running: true })} nowUnix={NOW} />);
    // vitest started at EVENTS[1] and settled at EVENTS[2]: 900 ms apart.
    expect(rowOf("vitest")).toMatchObject({ State: "ran — tests failed", Elapsed: "<1 s" });
  });

  it("shows the stored state again, and the recorded duration, once the run is shown ended (HF-6)", () => {
    const done = withJob(NO_RUN, COMPLETED);
    render(<ToolStatusWidget tools={TOOLS_AFTER} run={handle({ view: done, rows: progressRows(done, TOOLS_AFTER) })} nowUnix={NOW} />);
    expect(rowOf("go")).toMatchObject({ State: "ran", Elapsed: "1 s" });
  });

  it("lists each failed and tests-failed tool below the table with its cause and log path, red and orange", () => {
    render(<ToolStatusWidget tools={TOOLS_AFTER} run={handle()} nowUnix={NOW} />);
    const table = screen.getByRole("table", { name: "Coverage tools" });
    const area = screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages });
    expect(table.compareDocumentPosition(area) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const items = within(area).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    const [vitest, custom] = items;
    expect(vitest).toHaveTextContent("vitest");
    expect(vitest).toHaveTextContent("FAIL src/cart.test.ts > totals the cart");
    expect(vitest).toHaveTextContent("AssertionError: expected 41 to be 42");
    // A tests-failed run wrote no log.
    expect(vitest).not.toHaveTextContent("Whole output");
    expect(custom).toHaveTextContent("custom");
    expect(custom).toHaveTextContent("make: *** No rule to make target 'cov'. Stop.");
    expect(custom).toHaveTextContent(COVERAGE_RUN_TEXT.log(".logos/coverage/custom.failed.log"));
    expect(within(vitest).getByText("ran — tests failed").className).toMatch(/orange/);
    expect(within(custom).getByText("failed").className).toMatch(/red/);
    // The ran tool has no entry.
    expect(items.map((li) => li.querySelector(".mono")?.textContent)).toEqual(["vitest", "custom"]);
  });

  it("lists a timed out tool in red with the limit it hit", () => {
    const tools = TOOLS_AFTER.map((t) =>
      t.tool === "go"
        ? { ...t, state: "timed_out" as const, last_run: { ...t.last_run!, outcome: "timed_out" as const, cause: "timed out after 1800 s" } }
        : t,
    );
    render(<ToolStatusWidget tools={tools} run={handle()} nowUnix={NOW} />);
    const area = screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages });
    const go = within(area).getAllByRole("listitem").find((li) => li.textContent?.startsWith("go"));
    expect(go).toHaveTextContent("timed out");
    expect(go).toHaveTextContent("timed out after 1800 s");
    expect(within(go as HTMLElement).getByText("timed out").className).toMatch(/red/);
  });

  it("states a record with no cause as not recorded, not as an empty cause (NFR-CC-04)", () => {
    const tools = TOOLS_AFTER.map((t) => (t.last_run ? { ...t, last_run: { ...t.last_run, cause: undefined } } : t));
    render(<ToolStatusWidget tools={tools} run={handle()} nowUnix={NOW} />);
    const area = screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages });
    expect(within(area).getAllByRole("listitem")[0]).toHaveTextContent(COVERAGE_RUN_TEXT.noCause);
  });

  it("has no message area when no tool failed, timed out or had failing tests — cancelled and ran show nothing", () => {
    const { rerender } = render(<ToolStatusWidget tools={TOOLS} run={handle()} nowUnix={NOW} />);
    expect(screen.queryByRole("list", { name: COVERAGE_RUN_TEXT.messages })).toBeNull();
    const calm = TOOLS_AFTER.map((t) =>
      t.tool === "vitest" || t.tool === "custom"
        ? { ...t, state: "cancelled" as const, last_run: { ...t.last_run!, outcome: "cancelled" as const } }
        : t,
    );
    rerender(<ToolStatusWidget tools={calm} run={handle()} nowUnix={NOW} />);
    expect(screen.queryByRole("list", { name: COVERAGE_RUN_TEXT.messages })).toBeNull();
  });

  it("keeps a tool's entry until it runs again: a tool outside the run keeps it, one queued or settled in the run does not", () => {
    // jest is switched off, so the run leaves it out; its last run failed before it was.
    const tools = TOOLS_AFTER.map((t) =>
      t.tool === "jest"
        ? { ...t, state: "disabled" as const, last_run: { outcome: "failed" as const, exit_code: 1, head_sha: null, finished_at: 1_761_169_000, log: null, cause: "BUILD FAILURE" } }
        : t,
    );
    const view = withJob(NO_RUN, RUNNING);
    render(<ToolStatusWidget tools={tools} run={handle({ view, rows: progressRows(view, tools), running: true })} nowUnix={NOW} />);
    const area = screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages });
    // vitest settled in this run and custom is queued behind it: neither shows the run before.
    expect(within(area).getAllByRole("listitem").map((li) => li.textContent)).toEqual([expect.stringMatching(/^jest.*BUILD FAILURE/)]);
  });

  it("tones a live State: in flight orange, an outcome as it is stored, queued and not installed muted", () => {
    const timedOut = EVENTS.map((e) =>
      e.event.phase === "finished" && e.event.tool === "custom" ? { ...e, event: { phase: "finished" as const, tool: "custom", outcome: "timed_out" as const, after_secs: 30 } } : e,
    );
    const live = (events: typeof EVENTS) => {
      const view = withJob(NO_RUN, { ...RUNNING, events });
      return handle({ view, rows: progressRows(view, TOOLS), running: true });
    };
    const { unmount } = render(<ToolStatusWidget tools={TOOLS} run={live(EVENTS.slice(0, 8))} nowUnix={NOW} />);
    expect(["vitest", "maven", "go", "custom"].map((t) => /orange|muted|green|red/.exec(toneOf(t))?.[0])).toEqual(["orange", "muted", "orange", "muted"]);
    unmount();
    const ended = render(<ToolStatusWidget tools={TOOLS} run={live(EVENTS.slice(0, 10))} nowUnix={NOW} />);
    expect(["go", "custom"].map((t) => /orange|muted|green|red/.exec(toneOf(t))?.[0])).toEqual(["green", "orange"]);
    ended.unmount();
    render(<ToolStatusWidget tools={TOOLS} run={live(EVENTS)} nowUnix={NOW} />);
    expect(toneOf("custom")).toMatch(/red/);
    cleanup();
    render(<ToolStatusWidget tools={TOOLS} run={live(timedOut)} nowUnix={NOW} />);
    expect(toneOf("custom")).toMatch(/red/);
  });

  it("tones a stored State: ran green, tests failed orange, failed red, never run muted", () => {
    render(<ToolStatusWidget tools={TOOLS_AFTER} run={handle()} nowUnix={NOW} />);
    expect(toneOf("go")).toMatch(/green/);
    expect(toneOf("vitest")).toMatch(/orange/);
    expect(toneOf("custom")).toMatch(/red/);
    cleanup();
    render(<ToolStatusWidget tools={TOOLS} run={handle()} nowUnix={NOW} />);
    expect(toneOf("vitest")).toMatch(/muted/);
  });

  it("states that no tool applies, naming the ingest command, with no table and no Run", () => {
    const none = TOOLS.map((t) => ({ ...t, state: "not_applicable" as const }));
    const { container } = render(<ToolStatusWidget tools={none} run={handle({ canRun: false })} nowUnix={NOW} />);
    expect(container.querySelector("[data-widget-absence]")?.textContent).toBe(COVERAGE_VIEW_ABSENCE.noToolApplies);
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("ToolStatusWidget's Install and Add dependency (S-750)", () => {
  it("offers Install on a managed tool's row and Add dependency on a provider-less vitest, never on a tool it cannot set up", () => {
    const run = handle({ canRun: false });
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={run} nowUnix={NOW} />);
    screen.getByRole("button", { name: COVERAGE_OFFER_TEXT.installLabel("cargo-llvm-cov") }).click();
    expect(run.install).toHaveBeenCalledTimes(1);
    screen.getByRole("button", { name: COVERAGE_OFFER_TEXT.addDependencyLabel("vitest") }).click();
    expect(run.addDependency).toHaveBeenCalledWith("vitest");
    expect(rowOf("cargo-llvm-cov").State).toContain(COVERAGE_OFFER_TEXT.install);
    expect(rowOf("vitest").State).toContain(COVERAGE_OFFER_TEXT.addDependency);
    // Maven is not installed and Logos cannot set it up: its row offers nothing.
    const table = screen.getByRole("table", { name: "Coverage tools" });
    const maven = within(table).getByText("maven", { selector: "td" }).closest("tr") as HTMLElement;
    expect(within(maven).queryByRole("button")).toBeNull();
    // An offer only on a row that reads not installed.
    expect(screen.getAllByRole("button", { name: /then run coverage$/ })).toHaveLength(2);
  });

  it("disables the offers while a run goes", () => {
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={handle({ running: true })} nowUnix={NOW} />);
    for (const name of [COVERAGE_OFFER_TEXT.installLabel("cargo-llvm-cov"), COVERAGE_OFFER_TEXT.addDependencyLabel("vitest")]) {
      expect(screen.getByRole("button", { name })).toBeDisabled();
    }
  });

  /** The widget over `job`'s steps for PREPARABLE_TOOLS, as the hook hands them. */
  const readying = (job: CoverageJob, over: Partial<CoverageRunHandle> = {}) => {
    const view = withJob(NO_RUN, job);
    return handle({ view, rows: progressRows(view, PREPARABLE_TOOLS), prepared: prepareRows(view), running: job.state === "running", ...over });
  };

  it("reads installing while the tool installs or its llvm-tools-preview step goes, counting from that step's start", () => {
    vi.useFakeTimers();
    const [started, finished, componentStarted] = INSTALL_RUNNING.events;
    const install = { ...INSTALL_RUNNING, elapsed_ms: started.at_ms + 4_000, events: [started] };
    const { unmount } = render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(install)} nowUnix={NOW} />);
    expect(rowOf("cargo-llvm-cov")).toMatchObject({ State: expect.stringContaining("installing"), Elapsed: "4 s" });
    expect(toneOf("cargo-llvm-cov")).toMatch(/orange/);
    unmount();
    // Installed, then the component step: the tool is still being readied.
    const component = { ...INSTALL_RUNNING, elapsed_ms: componentStarted.at_ms + 2_000, events: [started, finished, componentStarted] };
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(component)} nowUnix={NOW} />);
    expect(rowOf("cargo-llvm-cov")).toMatchObject({ State: expect.stringContaining("installing"), Elapsed: "2 s" });
    act(() => vi.advanceTimersByTime(1_000));
    expect(rowOf("cargo-llvm-cov").Elapsed).toBe("3 s");
  });

  it("reads queued once the tool is readied and running once it starts", () => {
    const [started, finished, componentStarted, componentFinished] = INSTALL_RUNNING.events;
    const readied = { ...INSTALL_RUNNING, events: [started, finished, componentStarted, componentFinished] };
    const { unmount } = render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(readied)} nowUnix={NOW} />);
    expect(rowOf("cargo-llvm-cov")).toMatchObject({ State: "queued", Elapsed: "—" });
    expect(toneOf("cargo-llvm-cov")).toMatch(/muted/);
    unmount();
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(INSTALL_RUNNING)} nowUnix={NOW} />);
    expect(rowOf("cargo-llvm-cov").State).toBe("running");
  });

  it("reads adding dependency while a provider is added, and the run's other rows stay as they were", () => {
    const adding = { ...DEPENDENCY_CANCELLED, state: "running" as const, elapsed_ms: 3_010, events: DEPENDENCY_CANCELLED.events.slice(0, 1) };
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(adding)} nowUnix={NOW} />);
    expect(rowOf("vitest").State).toContain("adding dependency");
    expect(toneOf("vitest")).toMatch(/orange/);
    expect(rowOf("vitest").Elapsed).toBe("3 s");
    expect(rowOf("cargo-llvm-cov").State).toContain("not installed");
  });

  it("lists a failed or refused readying step below the table with its detail, until the next run replaces it", () => {
    const failed: CoverageJob = {
      ...DEPENDENCY_CANCELLED,
      state: "completed",
      events: [
        DEPENDENCY_CANCELLED.events[0],
        {
          seq: 1,
          at_ms: 900,
          event: {
            phase: "dependency_finished",
            tool: "vitest",
            package: "@vitest/coverage-v8@3",
            manager: "pnpm",
            command: "pnpm add -D @vitest/coverage-v8@3",
            outcome: "failed",
            exit_code: 1,
            stderr: "ERR_PNPM_FETCH_404 not found",
            log: null,
          },
        },
        { seq: 2, at_ms: 950, event: { phase: "component_started", component: "llvm-tools-preview", command: "rustup component add llvm-tools-preview" } },
        { seq: 3, at_ms: 990, event: { phase: "component_finished", component: "llvm-tools-preview", outcome: "no_rustup", reason: "rustup was not found", command: "rustup component add llvm-tools-preview" } },
      ],
    };
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(failed)} nowUnix={NOW} />);
    const area = screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages });
    const [dependency, component] = within(area).getAllByRole("listitem");
    expect(dependency).toHaveTextContent("pnpm add -D @vitest/coverage-v8@3");
    expect(dependency).toHaveTextContent("ERR_PNPM_FETCH_404 not found");
    expect(within(dependency).getByText(COVERAGE_PREPARE_TEXT.state.failed).className).toMatch(/red/);
    expect(component).toHaveTextContent(COVERAGE_PREPARE_TEXT.byHand("rustup was not found", "rustup component add llvm-tools-preview"));
    expect(within(component).getByText(COVERAGE_PREPARE_TEXT.state.no_rustup).className).toMatch(/orange/);
    // A cancelled addition names what it put back; a cancel that changed nothing says nothing.
    cleanup();
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(DEPENDENCY_CANCELLED)} nowUnix={NOW} />);
    expect(screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages })).toHaveTextContent(
      COVERAGE_PREPARE_TEXT.putBack(["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"]),
    );
    cleanup();
    const untouched = { ...INSTALL_RUNNING, state: "cancelled" as const, events: INSTALL_RUNNING.events.slice(0, 1) };
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={readying(untouched)} nowUnix={NOW} />);
    expect(screen.queryByRole("list", { name: COVERAGE_RUN_TEXT.messages })).toBeNull();
  });

  it("tones each readying step the message area states: failures red, refusals orange, a cancel muted", () => {
    const step = (state: PrepareRow["state"], detail: string | null = null): PrepareRow => ({
      key: `install:${state}`, tool: "cargo-llvm-cov", kind: "install", what: "cargo-llvm-cov 0.9.1", state, detail, startedMs: 0,
    });
    const prepared = [step("unavailable", "no release"), step("timed_out"), step("refused", "by hand"), step("cancelled", "Put back as it was: package.json.")];
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={handle({ prepared })} nowUnix={NOW} />);
    const items = within(screen.getByRole("list", { name: COVERAGE_RUN_TEXT.messages })).getAllByRole("listitem");
    const tone = (i: number, state: PrepareRow["state"]) => within(items[i]).getByText(COVERAGE_PREPARE_TEXT.state[state]).className;
    expect(items).toHaveLength(4);
    expect(tone(0, "unavailable")).toMatch(/red/);
    expect(tone(1, "timed_out")).toMatch(/red/);
    expect(tone(2, "refused")).toMatch(/orange/);
    expect(tone(3, "cancelled")).toMatch(/muted/);
  });
});
