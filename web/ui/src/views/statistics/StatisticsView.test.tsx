import { cleanup, render, screen, waitFor } from "@testing-library/react";
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

import { StatisticsView } from "./StatisticsView.tsx";

/** A populated read-model. `context` calls are `100 + windowDays` (so 107 vs 130)
 *  — a unique per-surface figure that lets a test prove a data-table twin (not just
 *  the callout copy) refreshed when the window changed. */
function populated(windowDays: number): StatsInfo {
  return {
    window_days: windowDays,
    calls_total: 42,
    // No `surface:"web"` row: the endpoint filters dashboard activity out
    // server-side (HF-1), so a populated response never carries it.
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

    // The verdict-first value callout (labeled an estimate, NFR-CC-04).
    expect(await screen.findByText(/12,345/)).toBeInTheDocument();
    expect(screen.getByText(/Estimated value/i)).toBeInTheDocument();
    expect(screen.getByText(/over the last 7 days/i)).toBeInTheDocument();

    // The four surface cards.
    expect(screen.getByRole("heading", { name: "Usage over time" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Top tools & surfaces" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Dev vs main" })).toBeInTheDocument();

    // Every ECharts surface renders (activity line, tools bar, surface bar, origin bar).
    expect(screen.getAllByRole("img").length).toBeGreaterThanOrEqual(4);

    // The charts are applied with notMerge so a shrinking dataset leaves no stale marks.
    expect(setOption).toHaveBeenCalledWith(expect.anything(), { notMerge: true });

    // The accessible data-table twins carry the same figures across all surfaces.
    expect(screen.getByText("context")).toBeInTheDocument(); // top-tools twin
    expect(screen.getByText("cli")).toBeInTheDocument(); // by-surface twin
    expect(screen.getByText("mcp")).toBeInTheDocument();
    expect(screen.getByText("dev")).toBeInTheDocument(); // dev-vs-main twin

    // Web (dashboard) activity is excluded server-side, so no "web" surface row.
    expect(screen.queryByText("web")).toBeNull();
  });

  it("re-queries and updates every surface when the window changes (UAT-UI-09)", async () => {
    stubFetch(populated);
    render(<StatisticsView />);
    await screen.findByText(/over the last 7 days/i);
    // A per-surface figure that tracks the 7-day window is visible in the data-table
    // twins (the tools `context` row and the by-surface `cli` row both read 107).
    expect(screen.getAllByText("107").length).toBeGreaterThan(0);

    const before = setOption.mock.calls.length;
    await userEvent.selectOptions(screen.getByLabelText("Window"), "30");

    // The value callout re-renders against the 30-day model...
    expect(await screen.findByText(/over the last 30 days/i)).toBeInTheDocument();
    // ...AND the surface data-table twins now show the 30-day figure (proving the
    // surfaces refreshed, not just the callout text) — the old figure is gone.
    expect((await screen.findAllByText("130")).length).toBeGreaterThan(0);
    expect(screen.queryAllByText("107")).toHaveLength(0);
    // ...and the charts are re-applied with the new data (a fresh setOption batch).
    await waitFor(() => expect(setOption.mock.calls.length).toBeGreaterThan(before));
    // The selected window persists across the re-query.
    expect(screen.getByLabelText("Window")).toHaveValue("30");
  });

  it("renders an honest awaiting-data empty state, never fabricated zeros (NFR-CC-04)", async () => {
    stubFetch(empty);
    render(<StatisticsView />);

    expect(await screen.findByText(/no telemetry recorded yet/i)).toBeInTheDocument();
    // No charts and no fabricated value figure.
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(screen.queryByText(/Estimated value/i)).not.toBeInTheDocument();
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
    expect(screen.queryByText(/Estimated value/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });

  it("shows per-card empty states and the top-tools truncation note honestly", async () => {
    stubFetch(partial);
    render(<StatisticsView />);

    // Body renders (calls_total > 0), but the empty sub-series show per-card empties.
    expect(await screen.findByText(/No activity in this window/i)).toBeInTheDocument();
    expect(screen.getByText(/No attributed usage in this window/i)).toBeInTheDocument();
    // The ranked bar caps at TOP_TOOLS_LIMIT and says so.
    expect(screen.getByText(/Showing the top 8 tools/i)).toBeInTheDocument();
  });
});
