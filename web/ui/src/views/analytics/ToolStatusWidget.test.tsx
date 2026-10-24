// The Coverage tools widget (S-739, FR-CV-15, FR-CV-11; frontend-design §4.10):
// one row per tool whose ecosystem is at the project root, each state a word —
// a tool that is not installed, or a failing suite, never reads as 0 % — with its
// version, last run and matched-of-reported files; the explanation names the
// ecosystems detected; Run coverage (Cancel while a run goes) sits on the title
// row; with no tool's ecosystem here the figure row states that absence.
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
import type { CoverageRunHandle } from "./CoverageRunPanel.tsx";
import { NO_RUN, prepareRows, progressRows, withJob } from "./coverageRunModel.ts";
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
    expect(rowOf("maven")).toEqual({ Tool: "maven", Version: "—", State: "not installed", "Last run": "—", Files: "—" });
    expect(rowOf("vitest")).toEqual({ Tool: "vitest", Version: "3.2.4", State: "never run", "Last run": "—", Files: "—" });
    // A copy outside the project reads with its version unknown: the page's read starts no tool.
    expect(rowOf("go").Version).toBe("—");
  });

  it("shows a not-installed tool's hint", () => {
    const tools = TOOLS.map((t) => (t.tool === "maven" ? { ...t, hint: "install the project's dependencies first" } : t));
    render(<ToolStatusWidget tools={tools} run={handle()} nowUnix={NOW} />);
    expect(rowOf("maven").State).toBe("not installedinstall the project's dependencies first");
  });

  it("reads a failing suite as ran — tests failed, a failed tool with its log, and each last run's age and commit", () => {
    const view = withJob(NO_RUN, COMPLETED);
    render(<ToolStatusWidget tools={TOOLS_AFTER} run={handle({ view, rows: progressRows(view, TOOLS_AFTER) })} nowUnix={NOW} />);
    expect(rowOf("vitest")).toEqual({
      Tool: "vitest",
      Version: "3.2.4",
      State: "ran — tests failed",
      "Last run": "5m ago · 4f9c2e1",
      Files: "12 of 13",
    });
    expect(rowOf("go")).toMatchObject({ State: "ran", Files: "1 of 1" });
    expect(rowOf("custom")).toMatchObject({ State: "failedWhole output: .logos/coverage/custom.failed.log", Files: "—" });
    for (const tool of ["vitest", "maven", "go", "custom"]) expect(rowOf(tool).State).not.toMatch(/\d\s*%/);
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

  it("turns the button into Cancel while a run goes, with the progress list above the table", () => {
    const view = withJob(NO_RUN, RUNNING);
    const run = handle({ view, rows: progressRows(view, TOOLS), running: true });
    render(<ToolStatusWidget tools={TOOLS} run={run} nowUnix={NOW} />);
    expect(screen.queryByRole("button", { name: COVERAGE_RUN_TEXT.run })).toBeNull();
    screen.getByRole("button", { name: COVERAGE_RUN_TEXT.cancel }).click();
    expect(run.cancel).toHaveBeenCalledTimes(1);
    const list = screen.getByRole("list", { name: "Coverage run progress" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "vitest ran — tests failed",
      "maven not installed",
      expect.stringMatching(/^go running \d+ s$/),
      "custom queued",
    ]);
  });

  it("puts a failed tool's error output and its whole-output log in a disclosure (review)", () => {
    const view = withJob(NO_RUN, COMPLETED);
    render(<ToolStatusWidget tools={TOOLS_AFTER} run={handle({ view, rows: progressRows(view, TOOLS_AFTER) })} nowUnix={NOW} />);
    const list = screen.getByRole("list", { name: "Coverage run progress" });
    const custom = within(list).getByText("custom").closest("li") as HTMLElement;
    const disclosure = custom.querySelector("details") as HTMLElement;
    expect(within(disclosure).getByText(COVERAGE_RUN_TEXT.stderr).tagName).toBe("SUMMARY");
    expect(disclosure).toHaveTextContent("make: *** No rule to make target 'cov'. Stop.");
    expect(disclosure).toHaveTextContent(COVERAGE_RUN_TEXT.log(".logos/coverage/custom.failed.log"));
    // Only the failed tool carries one.
    expect(list.querySelectorAll("details")).toHaveLength(1);
  });

  it("counts a running tool's seconds from its own start, once a second (review)", () => {
    vi.useFakeTimers();
    const goStarted = EVENTS[7].at_ms;
    const view = withJob(NO_RUN, { ...RUNNING, elapsed_ms: goStarted + 3_000 });
    const run = handle({ view, rows: progressRows(view, TOOLS), running: true });
    render(<ToolStatusWidget tools={TOOLS} run={run} nowUnix={NOW} />);
    const go = () => within(screen.getByRole("list", { name: "Coverage run progress" })).getByText("go").closest("li");
    expect(go()).toHaveTextContent(/^go running 3 s$/);
    act(() => vi.advanceTimersByTime(2_000));
    expect(go()).toHaveTextContent(/^go running 5 s$/);
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

  it("lists what the run readied above the tools, with what a cancelled addition put back", () => {
    const install = withJob(NO_RUN, INSTALL_RUNNING);
    const { unmount } = render(
      <ToolStatusWidget
        tools={PREPARABLE_TOOLS}
        run={handle({ view: install, rows: progressRows(install, PREPARABLE_TOOLS), prepared: prepareRows(install), running: true })}
        nowUnix={NOW}
      />,
    );
    const items = () => within(screen.getByRole("list", { name: "Coverage run progress" })).getAllByRole("listitem").map((li) => li.textContent);
    expect(items()).toEqual([
      "cargo-llvm-cov 0.9.1 installed",
      "llvm-tools-preview added",
      expect.stringMatching(/^cargo-llvm-cov running \d+ s$/),
    ]);
    unmount();
    const cancelled = withJob(NO_RUN, DEPENDENCY_CANCELLED);
    render(<ToolStatusWidget tools={PREPARABLE_TOOLS} run={handle({ view: cancelled, prepared: prepareRows(cancelled) })} nowUnix={NOW} />);
    expect(items()[0]).toBe(
      `pnpm add -D @vitest/coverage-v8@3 cancelled${COVERAGE_PREPARE_TEXT.putBack(["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"])}`,
    );
  });
});
