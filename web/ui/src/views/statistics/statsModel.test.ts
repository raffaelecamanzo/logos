import { describe, expect, it } from "vitest";

import type { AttributionCoverage, StatsInfo } from "../../api/types.ts";
import {
  NEUTRAL,
  SERIES_WARM,
  activityLineOption,
  activitySeries,
  answeredLabel,
  attributionByClass,
  bySurface,
  isStatsEmpty,
  originBarOption,
  originSplit,
  rankedBarOption,
  topTools,
} from "./statsModel.ts";

/** A populated `attribution_coverage` rider — the shape the endpoint returns
 *  alongside the cross-tab; tests that don't care about the caveats reuse this
 *  rather than restating all six fields. */
function coverage(overrides: Partial<AttributionCoverage> = {}): AttributionCoverage {
  return {
    raw_events_only: true,
    requested_window_days: 7,
    covered_window_days: 7,
    truncated_by_retention: false,
    legacy_null_origin_folds_into_main: true,
    notes: ["raw events only", "CLI+MCP-only before the origin stamp"],
    ...overrides,
  };
}

/** A zeroed read-model (the shape the endpoint returns for an empty store). */
function emptyStats(overrides: Partial<StatsInfo> = {}): StatsInfo {
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
    ...overrides,
  };
}

describe("isStatsEmpty (NFR-CC-04)", () => {
  it("is true for a zeroed store", () => {
    expect(isStatsEmpty(emptyStats())).toBe(true);
  });

  it("is false as soon as any call is recorded", () => {
    expect(isStatsEmpty(emptyStats({ calls_total: 1 }))).toBe(false);
  });
});

describe("topTools", () => {
  it("sums calls across surfaces per tool and ranks by calls desc (ties by name)", () => {
    const stats = emptyStats({
      calls_total: 30,
      calls_by_tool: [
        { surface: "cli", tool: "context", calls: 5, ok_calls: 5 },
        { surface: "mcp", tool: "context", calls: 7, ok_calls: 6 },
        { surface: "cli", tool: "search", calls: 12, ok_calls: 12 },
        { surface: "cli", tool: "impact", calls: 12, ok_calls: 11 },
      ],
    });
    const { rows, truncated } = topTools(stats);
    // search (12) and impact (12) tie — impact sorts first (name asc); context = 12 too.
    expect(rows.map((r) => r.tool)).toEqual(["context", "impact", "search"]);
    expect(rows.find((r) => r.tool === "context")?.calls).toBe(12);
    expect(truncated).toBe(false);
  });

  it("caps to the limit and flags truncation", () => {
    const stats = emptyStats({
      calls_total: 100,
      calls_by_tool: Array.from({ length: 10 }, (_, i) => ({
        surface: "cli",
        tool: `t${String(i).padStart(2, "0")}`,
        calls: 10 - i,
        ok_calls: 10 - i,
      })),
    });
    const { rows, truncated } = topTools(stats, 3);
    expect(rows).toHaveLength(3);
    expect(truncated).toBe(true);
    expect(rows[0].tool).toBe("t00"); // highest calls
  });

  it("defaults to the top-8 cap (TOP_TOOLS_LIMIT)", () => {
    const stats = emptyStats({
      calls_total: 100,
      calls_by_tool: Array.from({ length: 12 }, (_, i) => ({
        surface: "cli",
        tool: `t${String(i).padStart(2, "0")}`,
        calls: 12 - i,
        ok_calls: 12 - i,
      })),
    });
    const { rows, truncated } = topTools(stats); // no explicit limit
    expect(rows).toHaveLength(8);
    expect(truncated).toBe(true);
  });
});

describe("bySurface", () => {
  it("aggregates calls per surface, ranked desc", () => {
    const stats = emptyStats({
      calls_total: 20,
      calls_by_tool: [
        { surface: "cli", tool: "a", calls: 3, ok_calls: 3 },
        { surface: "cli", tool: "b", calls: 4, ok_calls: 4 },
        { surface: "mcp", tool: "a", calls: 13, ok_calls: 13 },
      ],
    });
    expect(bySurface(stats)).toEqual([
      { surface: "mcp", calls: 13 },
      { surface: "cli", calls: 7 },
    ]);
  });

  it("breaks ties by surface name ascending", () => {
    // `bySurface` is surface-agnostic; web is filtered server-side (HF-1), so the
    // categories it ever sees are cli / mcp / watcher.
    const stats = emptyStats({
      calls_total: 10,
      calls_by_tool: [
        { surface: "watcher", tool: "a", calls: 5, ok_calls: 5 },
        { surface: "cli", tool: "a", calls: 5, ok_calls: 5 },
      ],
    });
    expect(bySurface(stats).map((s) => s.surface)).toEqual(["cli", "watcher"]);
  });
});

describe("originSplit (FR-OB-08)", () => {
  it("marks main as the baseline and ranks by calls desc", () => {
    // The backend collapses all worktree branches into a single `"dev"` bucket.
    const stats = emptyStats({
      calls_total: 40,
      calls_by_origin: [
        { origin: "main", calls: 10, ok_calls: 9 },
        { origin: "dev", calls: 25, ok_calls: 25 },
      ],
    });
    const rows = originSplit(stats);
    expect(rows[0].origin).toBe("dev");
    expect(rows[0].isMain).toBe(false);
    expect(rows[1].origin).toBe("main");
    expect(rows[1].isMain).toBe(true);
  });

  it("does not fabricate a total — it charts the split as given", () => {
    // The split sums to 35 though calls_total is 40 (rolled-up days carry no origin).
    const stats = emptyStats({
      calls_total: 40,
      calls_by_origin: [{ origin: "main", calls: 35, ok_calls: 35 }],
    });
    const rows = originSplit(stats);
    expect(rows.reduce((s, r) => s + r.calls, 0)).toBe(35);
  });
});

describe("activitySeries", () => {
  it("preserves the read-model's oldest-first order and carries calls/ok through", () => {
    const stats = emptyStats({
      calls_total: 5,
      activity_by_day: [
        { day: "2026-07-01", calls: 2, ok_calls: 1 },
        { day: "2026-07-02", calls: 3, ok_calls: 3 },
      ],
    });
    expect(activitySeries(stats)).toEqual([
      { day: "2026-07-01", calls: 2, ok_calls: 1 },
      { day: "2026-07-02", calls: 3, ok_calls: 3 },
    ]);
  });
});

describe("answeredLabel (FR-OB-14)", () => {
  it("renders the answered pair, never a rate, once anything is classified", () => {
    expect(
      answeredLabel({ answered_calls: 2, classified_calls: 4, outcome_absence: null }),
    ).toBe("2 of 4 answered");
  });

  it("renders 0-of-N as a figure, not an absence", () => {
    expect(
      answeredLabel({ answered_calls: 0, classified_calls: 4, outcome_absence: null }),
    ).toBe("0 of 4 answered");
  });

  it("renders the read-model's own named absence when nothing is classified", () => {
    expect(
      answeredLabel({ answered_calls: 0, classified_calls: 0, outcome_absence: "none recorded" }),
    ).toBe("none recorded");
  });

  it("falls back to the figure, never blank, if the absence invariant is ever violated", () => {
    // The backend derives `outcome_absence` from `classified_calls` at
    // serialisation (`OutcomeCounts::absence`), so the two can never actually
    // disagree — but nothing in this type pins that here, so a defensive input
    // that pairs `classified_calls: 0` with a `null` absence is still pinned to
    // an honest answer ("0 of 0 answered") rather than an empty string or a
    // silently different rendering.
    expect(
      answeredLabel({ answered_calls: 0, classified_calls: 0, outcome_absence: null }),
    ).toBe("0 of 0 answered");
  });
});

describe("attributionByClass (FR-OB-11)", () => {
  it("groups the cross-tab by class, classes sorted by name", () => {
    const stats = emptyStats({
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
          tool: "config_write",
          class: "engine-internal",
          origin: "main",
          calls: 2,
          ok_calls: 2,
          answered_calls: 0,
          classified_calls: 0,
          outcome_absence: "none recorded",
        },
        {
          tool: "impact",
          class: "navigation",
          origin: "main",
          calls: 1,
          ok_calls: 1,
          answered_calls: 1,
          classified_calls: 1,
          outcome_absence: null,
        },
      ],
    });
    const groups = attributionByClass(stats);
    expect(groups.map((g) => g.class)).toEqual(["engine-internal", "navigation"]);
    const navigation = groups.find((g) => g.class === "navigation");
    // Row order follows the read-model's own (tool, origin) order — not re-sorted.
    expect(navigation?.rows).toEqual([
      { tool: "search", origin: "dev", calls: 5, ok_calls: 5, answered: "3 of 4 answered" },
      { tool: "impact", origin: "main", calls: 1, ok_calls: 1, answered: "1 of 1 answered" },
    ]);
    const engineInternal = groups.find((g) => g.class === "engine-internal");
    expect(engineInternal?.rows).toEqual([
      { tool: "config_write", origin: "main", calls: 2, ok_calls: 2, answered: "none recorded" },
    ]);
  });

  it("is empty when the cross-tab is empty (raw-events-only, honestly)", () => {
    expect(attributionByClass(emptyStats())).toEqual([]);
  });
});

describe("ECharts option builders", () => {
  it("activityLineOption is a warm single line over the days", () => {
    const opt = activityLineOption([
      { day: "2026-07-01", calls: 2, ok_calls: 2 },
      { day: "2026-07-02", calls: 3, ok_calls: 3 },
    ]);
    const series = (opt.series as Array<Record<string, unknown>>)[0];
    expect(series.type).toBe("line");
    expect(series.data).toEqual([2, 3]);
    expect((series.lineStyle as { color: string }).color).toBe(SERIES_WARM);
    expect((opt.xAxis as { data: string[] }).data).toEqual(["2026-07-01", "2026-07-02"]);
  });

  it("rankedBarOption is a horizontal bar (category y, value x)", () => {
    const opt = rankedBarOption(["search", "impact"], [12, 5]);
    expect((opt.yAxis as { type: string; data: string[] }).type).toBe("category");
    expect((opt.yAxis as { data: string[] }).data).toEqual(["search", "impact"]);
    expect((opt.xAxis as { type: string }).type).toBe("value");
    expect((opt.series as Array<{ data: number[] }>)[0].data).toEqual([12, 5]);
  });

  it("originBarOption colours main neutral and the dev bucket warm", () => {
    const opt = originBarOption([
      { origin: "dev", calls: 25, ok_calls: 25, isMain: false },
      { origin: "main", calls: 10, ok_calls: 9, isMain: true },
    ]);
    const data = (opt.series as Array<{ data: Array<{ itemStyle: { color: string } }> }>)[0].data;
    expect(data[0].itemStyle.color).toBe(SERIES_WARM); // dev bucket
    expect(data[1].itemStyle.color).toBe(NEUTRAL); // main baseline
  });
});
