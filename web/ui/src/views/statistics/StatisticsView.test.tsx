import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { StatsInfo } from "../../api/types.ts";

// Mock the ECharts seam: jsdom has no real canvas. The fake instance records the
// options it is given so a test can confirm a re-query re-renders every surface.
const setOption = vi.fn();
vi.mock("./echarts.ts", () => ({
  createStatChart: () => ({
    setOption: (...args: unknown[]) => setOption(...args),
    resize: vi.fn(),
    dispose: vi.fn(),
  }),
}));

import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import {
  attributionNotesLead,
  devVsMain,
  estimatedValue,
  statisticsAbsence,
  toolAttribution,
  topToolsAndSurfaces,
  usageOverTime,
} from "../../copy/statistics.copy.ts";
import { StatisticsView } from "./StatisticsView.tsx";

/** The widget frame titled `title`. */
function widget(title: string): Element {
  const frame = screen.getByRole("heading", { name: title }).closest("[data-widget]");
  if (!frame) throw new Error(`no widget titled ${title}`);
  return frame;
}

/** The `attribution_coverage` rider (FR-OB-11): the raw-events-only limit, the
 *  legacy-`NULL`-origin caveat, and the pre-origin-stamp label, exactly as the
 *  read-model states them — asserted verbatim, not re-derived by the view. */
function coverage(): StatsInfo["attribution_coverage"] {
  return {
    raw_events_only: true,
    requested_window_days: 7,
    covered_window_days: 7,
    truncated_by_retention: false,
    legacy_null_origin_folds_into_main: true,
    notes: [
      "computed from raw events only",
      "legacy NULL origins fold into main",
      "predates the origin stamp: every surface of the time, dev/main unknown",
    ],
  };
}

/** A populated read-model. `context` calls are `100 + windowDays` (so 107 vs 130)
 *  — a unique per-surface figure that lets a test prove a data-table twin (not just
 *  the callout copy) refreshed when the window changed. */
function populated(windowDays: number): StatsInfo {
  return {
    window_days: windowDays,
    calls_total: 42,
    // This fixture carries no `surface:"web"` row; the dedicated web-row test
    // below covers a payload that does.
    calls_by_tool: [
      { surface: "cli", tool: "context", calls: 100 + windowDays, ok_calls: 50 },
      { surface: "mcp", tool: "search", calls: 20, ok_calls: 19 },
    ],
    latency_p50_ms: 3,
    latency_p95_ms: 11,
    latency_p99_ms: 30,
    reads_saved_estimate: 88,
    tokens_saved_estimate: 12345,
    artifact_bindings: {},
    activity_by_day: [
      { day: "2026-07-01", calls: 20, ok_calls: 20 },
      { day: "2026-07-02", calls: 22, ok_calls: 21 },
    ],
    calls_by_origin: [
      { origin: "main", calls: 30, ok_calls: 29 },
      { origin: "dev", calls: 12, ok_calls: 12 },
    ],
    calls_by_tool_origin: [
      {
        tool: "search",
        class: "navigation",
        origin: "dev",
        calls: 5,
        ok_calls: 5,
        answered_calls: 3,
        classified_calls: 4,
        outcome_absence: null,
      },
      {
        tool: "stats",
        class: "read-model",
        origin: "main",
        calls: 6,
        ok_calls: 6,
        answered_calls: 0,
        classified_calls: 0,
        outcome_absence: "none recorded",
      },
    ],
    calls_by_class: [
      { class: "navigation", origin: "dev", calls: 5, ok_calls: 5, answered_calls: 3, classified_calls: 4, outcome_absence: null },
      { class: "read-model", origin: "main", calls: 6, ok_calls: 6, answered_calls: 0, classified_calls: 0, outcome_absence: "none recorded" },
    ],
    attribution_coverage: coverage(),
    warnings: [],
  };
}

function empty(): StatsInfo {
  return {
    window_days: 7,
    calls_total: 0,
    calls_by_tool: [],
    latency_p50_ms: 0,
    latency_p95_ms: 0,
    latency_p99_ms: 0,
    reads_saved_estimate: 0,
    tokens_saved_estimate: 0,
    artifact_bindings: {},
    activity_by_day: [],
    calls_by_origin: [],
    calls_by_tool_origin: [],
    calls_by_class: [],
    attribution_coverage: coverage(),
    warnings: ["no telemetry recorded yet (telemetry.db not found)"],
  };
}

/** A non-empty store whose sub-series are individually empty / oversized — for the
 *  per-card empty states and the top-tools truncation note. */
function partial(): StatsInfo {
  return {
    ...empty(),
    calls_total: 50,
    // 10 tools > TOP_TOOLS_LIMIT (8) → the ranked bar truncates.
    calls_by_tool: Array.from({ length: 10 }, (_, i) => ({
      surface: "cli",
      tool: `tool-${String(i).padStart(2, "0")}`,
      calls: 10 - i,
      ok_calls: 10 - i,
    })),
    activity_by_day: [], // activity card → empty
    calls_by_origin: [], // origin card → empty
    calls_by_tool_origin: [], // attribution card → empty
    warnings: [],
  };
}

/** Stub global fetch, echoing the requested `?window=` into the read-model. */
function stubFetch(build: (windowDays: number) => StatsInfo) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const url = new URL(input, "http://localhost");
      const windowDays = Number(url.searchParams.get("window") ?? "7");
      return Promise.resolve({ ok: true, json: () => Promise.resolve(build(windowDays)) } as Response);
    }),
  );
}

/** Stub a failing read (non-2xx) — the honest error path, distinct from empty. */
function stubFetchError(status = 500) {
  vi.stubGlobal("fetch", vi.fn(() => Promise.resolve({ ok: false, status } as Response)));
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setOption.mockClear();
});

describe("StatisticsView (S-235, FR-UI-27)", () => {
  it("renders the value estimate and all four surfaces on a populated store", async () => {
    stubFetch(populated);
    render(<StatisticsView />);

    // The lead value widget (labeled an estimate, NFR-CC-04).
    expect(await screen.findByText(/12,345/)).toBeInTheDocument();
    expect(within(widget("Estimated value") as HTMLElement).getByText(/over the last 7 days/i)).toBeInTheDocument();

    // The four surface widgets.
    expect(screen.getByRole("heading", { name: "Usage over time" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Top tools & surfaces" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Dev vs main" })).toBeInTheDocument();

    // Every ECharts surface renders (activity line, tools bar, surface bar, origin bar).
    expect(screen.getAllByRole("img").length).toBeGreaterThanOrEqual(4);

    // The charts are applied with notMerge so a shrinking dataset leaves no stale marks.
    //
    // AWAITED, not asserted synchronously: setOption is called from a useEffect,
    // which React flushes after the commit — so the role="img" node above is
    // queryable strictly before the effect has necessarily run. A bare
    // expect() here passes on an idle machine and fails under load, which is
    // exactly how it behaved (green alone, red while a full cargo test run
    // saturated the box). The re-query assertion below already used waitFor for
    // this same spy; this is the same reason.
    await waitFor(() =>
      expect(setOption).toHaveBeenCalledWith(expect.anything(), { notMerge: true }),
    );

    // The accessible data-table twins carry the same figures across all surfaces.
    expect(within(screen.getByRole("table", { name: "Top tools" })).getByText("context")).toBeInTheDocument();
    // ...and the figure row names the most-used tool with its calls.
    expect(widget("Top tools & surfaces").querySelector('[data-widget-part="figure"]')).toHaveTextContent(
      "context most used, 107 calls",
    );
    expect(screen.getByText("cli")).toBeInTheDocument(); // by-surface twin
    expect(screen.getByText("mcp")).toBeInTheDocument();
    // "dev" appears in both the dev-vs-main twin and the attribution cross-tab.
    expect(screen.getAllByText("dev").length).toBeGreaterThanOrEqual(2);

    // This fixture carries no `web` row (see the dedicated web-row test below).
    expect(screen.queryByText("web")).toBeNull();
  });

  it("states the per-event self-referential exclusion and renders a web row (CR-146)", async () => {
    stubFetch((windowDays) => ({
      ...populated(windowDays),
      calls_by_tool: [
        { surface: "cli", tool: "context", calls: 100 + windowDays, ok_calls: 50 },
        { surface: "mcp", tool: "search", calls: 20, ok_calls: 19 },
        { surface: "web", tool: "context", calls: 7, ok_calls: 7 },
      ],
    }));
    render(<StatisticsView />);
    await screen.findByRole("heading", { name: "Top tools & surfaces" });

    // The caption states the true per-event rule, matching docs/howto/usage.md,
    // and never the retired blanket exclusion.
    expect(screen.queryByText(/dashboard \(web\) activity is excluded/i)).toBeNull();
    expect(screen.getByText(/self-referential reads/i)).toBeInTheDocument();
    expect(screen.getByText(/the tab's own stats request/i)).toBeInTheDocument();
    expect(screen.getByText(/the shell's status readout/i)).toBeInTheDocument();

    // The chart's accessible label carries no hard-coded surface list.
    expect(screen.queryByRole("img", { name: /cli \/ mcp \/ watcher/i })).toBeNull();
    expect(screen.getByRole("img", { name: /usage by surface/i })).toBeInTheDocument();

    // A `web` row present in the payload renders in the by-surface data table.
    expect(screen.getByText("web")).toBeInTheDocument();
  });

  it("re-queries and updates every surface when the window changes (UAT-UI-09)", async () => {
    stubFetch(populated);
    render(<StatisticsView />);
    await screen.findAllByText(/over the last 7 days/i);
    // A per-surface figure that tracks the 7-day window is visible in the data-table
    // twins (the tools `context` row and the by-surface `cli` row both read 107).
    expect(screen.getAllByText("107").length).toBeGreaterThan(0);

    const before = setOption.mock.calls.length;
    await userEvent.selectOptions(screen.getByLabelText("Window"), "30");

    // The value widget re-renders against the 30-day model...
    await screen.findAllByText(/over the last 30 days/i);
    expect(within(widget("Estimated value") as HTMLElement).getByText(/over the last 30 days/i)).toBeInTheDocument();
    // ...AND the surface data-table twins now show the 30-day figure (proving the
    // surfaces refreshed, not just the callout text) — the old figure is gone.
    expect((await screen.findAllByText("130")).length).toBeGreaterThan(0);
    expect(screen.queryAllByText("107")).toHaveLength(0);
    // ...and the charts are re-applied with the new data (a fresh setOption batch).
    await waitFor(() => expect(setOption.mock.calls.length).toBeGreaterThan(before));
    // The selected window persists across the re-query.
    expect(screen.getByLabelText("Window")).toHaveValue("30");
  });

  it("renders an honest awaiting-data state naming logos stats, never fabricated zeros (NFR-CC-04)", async () => {
    stubFetch(empty);
    render(<StatisticsView />);

    const absence = await screen.findByText(statisticsAbsence.awaiting);
    // The absence is stated in the value widget's figure row, and its action is
    // the command — the one Statistics state with something to do.
    expect(absence).toHaveAttribute("data-widget-absence");
    const w = widget("Estimated value");
    expectWidgetCopy(w, estimatedValue, { recorded: false });
    expect(w.querySelector('[data-widget-copy="where"]')).toHaveTextContent("command logos stats");
    // No charts, no other widget, and no fabricated value figure.
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(document.querySelectorAll("[data-widget]")).toHaveLength(1);
    expect(w.querySelector('[data-widget-part="figure"]')?.textContent).toBe(statisticsAbsence.awaiting);
    // The window selector still renders so the user can widen the window.
    expect(screen.getByLabelText("Window")).toBeInTheDocument();
  });

  it("renders a failed read as an honest error, distinct from the empty state (NFR-RA-05)", async () => {
    stubFetchError(500);
    render(<StatisticsView />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/500/);
    // A fault is NEVER shown as awaiting-data or as fabricated zeros.
    expect(screen.queryByText(/no telemetry recorded yet/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Estimated value" })).not.toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });

  it("shows per-card empty states and the top-tools truncation note honestly", async () => {
    stubFetch(partial);
    render(<StatisticsView />);

    // Body renders (calls_total > 0), but each empty sub-series is a named absence
    // in its widget's figure row — never the centred view-level empty state.
    for (const text of [statisticsAbsence.activity, statisticsAbsence.origins, statisticsAbsence.attribution]) {
      expect(await screen.findByText(text)).toHaveAttribute("data-widget-absence");
    }
    expectWidgetCopy(widget("Usage over time"), usageOverTime);
    expectWidgetCopy(widget("Dev vs main"), devVsMain);
    expectWidgetCopy(widget("Tool attribution by class"), toolAttribution);
    // An empty cross-tab is when "raw events only" explains the emptiness: the
    // read-model's notes still render in the explanation.
    const notes = [...widget("Tool attribution by class").querySelectorAll("[data-widget-note] p")].map(
      (p) => p.textContent,
    );
    expect(notes.slice(1)).toEqual(coverage().notes);
    // The ranked bar caps at TOP_TOOLS_LIMIT and says so.
    expect(screen.getByText(/Showing the top 8 tools/i)).toBeInTheDocument();
  });

  it("renders the tool × origin cross-tab as one table with a Class column, grouped by class then calls (CR-203 item 25)", async () => {
    stubFetch((w) => ({
      ...populated(w),
      calls_by_tool_origin: [
        ...populated(w).calls_by_tool_origin,
        {
          tool: "impact",
          class: "navigation",
          origin: "main",
          calls: 9,
          ok_calls: 9,
          answered_calls: 9,
          classified_calls: 9,
          outcome_absence: null,
        },
      ],
    }));
    render(<StatisticsView />);
    await screen.findByRole("heading", { name: "Tool attribution by class" });
    const w = widget("Tool attribution by class");

    // ONE table; the class is a column, not a heading per class.
    const tables = within(w as HTMLElement).getAllByRole("table");
    expect(tables).toHaveLength(1);
    expect(within(w as HTMLElement).queryByRole("heading", { name: "navigation" })).toBeNull();
    const headers = within(tables[0]).getAllByRole("columnheader").map((h) => h.querySelector("button")?.textContent);
    expect(headers.map((h) => h?.replace(/[↕▲▼]/g, ""))).toEqual(["Class", "Tool", "Origin", "Calls", "OK", "Answered"]);
    const rows = within(tables[0])
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell").slice(0, 2).map((c) => c.textContent));
    // navigation (impact 9 calls, then search 5), then read-model.
    expect(rows).toEqual([
      ["navigation", "impact"],
      ["navigation", "search"],
      ["read-model", "stats"],
    ]);

    // "Answered" is glossed, in the header.
    expect(tables[0].querySelector("th dfn[data-term='answered']")).not.toBeNull();
    // The answered/classified pair renders as prose, never a rate.
    expect(screen.getByText("3 of 4 answered")).toBeInTheDocument();
    // A cell with nothing classified renders the read-model's own named absence.
    expect(screen.getByText("none recorded")).toBeInTheDocument();
    expect(screen.queryByText(/%/)).not.toBeInTheDocument();
  });

  it("renders attribution_coverage.notes verbatim in the attribution widget's explanation (FR-OB-11)", async () => {
    stubFetch(populated);
    render(<StatisticsView />);
    await screen.findByRole("heading", { name: "Tool attribution by class" });
    const explanation = widget("Tool attribution by class").querySelector('[data-widget-part="explanation"]')!;
    const notes = [...explanation.querySelectorAll("[data-widget-note] p")].map((p) => p.textContent);
    // Exactly the read-model's own notes, verbatim and in order, after the lead.
    expect(notes).toEqual([attributionNotesLead, ...coverage().notes]);
  });

  it("renders no note, and no lead, when the read-model states no attribution limits", async () => {
    stubFetch((w) => ({ ...populated(w), attribution_coverage: { ...coverage(), notes: [] } }));
    render(<StatisticsView />);
    await screen.findByRole("heading", { name: "Tool attribution by class" });
    expect(widget("Tool attribution by class").querySelector("[data-widget-note]")).toBeNull();
    expect(screen.queryByText(attributionNotesLead)).toBeNull();
  });

  it("every widget explains itself, informational on a populated store (FR-UI-39)", async () => {
    stubFetch(populated);
    render(<StatisticsView />);
    await screen.findByRole("heading", { name: "Tool attribution by class" });
    expectWidgetCopy(widget("Estimated value"), estimatedValue, { recorded: true });
    expectWidgetCopy(widget("Usage over time"), usageOverTime);
    expectWidgetCopy(widget("Top tools & surfaces"), topToolsAndSurfaces);
    expectWidgetCopy(widget("Dev vs main"), devVsMain);
    expectWidgetCopy(widget("Tool attribution by class"), toolAttribution);
    for (const frame of document.querySelectorAll("[data-widget]")) {
      expect(frame.querySelector('[data-widget-part="action"]')).toHaveAttribute("data-action-kind", "none");
    }
    // One stack, five widgets, in reading order.
    const stacks = document.querySelectorAll("[data-widget-stack]");
    expect(stacks).toHaveLength(1);
    expect([...stacks[0].children].map((c) => c.querySelector("h3")?.textContent)).toEqual([
      "Estimated value",
      "Usage over time",
      "Top tools & surfaces",
      "Dev vs main",
      "Tool attribution by class",
    ]);
  });
});
