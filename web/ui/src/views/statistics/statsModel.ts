/*
 * The Statistics view's pure model layer (S-235, CR-058, FR-UI-27) — the
 * data-shaping and ECharts-option building kept **free of React and ECharts** so
 * every transform is unit-tested directly (the spa-frontend split the Graph view
 * uses: `graphOptions.ts` pure, `echarts.ts` the mockable seam). The imperative
 * mount (`StatChart.tsx`) consumes these options; the view (`StatisticsView.tsx`)
 * consumes the derived rows for the accessible data-table twins.
 *
 * Honesty (NFR-CC-04): nothing here fabricates a figure. Empty telemetry yields
 * empty series/rows the view renders as an awaiting-data state — never zeros
 * dressed up as usage. The `origin` split is charted as-is and never normalised to
 * sum to `calls_total` (the read-model warns it can be less).
 */

import type { OriginUsage, StatsInfo } from "../../api/types.ts";

// ── Brand palette (hardcoded hex mirroring the design tokens, as the Graph canvas
//    does — a canvas cannot read the CSS custom properties; the accessible truth is
//    the data-table twin, not the canvas). ──────────────────────────────────────
/** `--so-orange` — the warm data-series colour (charts are single-series red/orange;
 *  red stays signal-only, so the series uses orange). */
export const SERIES_WARM = "#e35205";
/** `--so-merlin` — brand neutral; the `main` baseline and axis text. */
export const NEUTRAL = "#3d3935";
/** `--so-muted` — secondary/axis labels. */
export const MUTED = "#716b5d";
/** A hairline gridline tuned to read on both themes. */
const GRIDLINE = "rgba(113,107,93,0.25)";

/** How many tools the ranked bar shows before it stops (the rest are aggregated
 *  away — the view notes the cap so a truncated ranking never reads as complete). */
export const TOP_TOOLS_LIMIT = 8;

/** The Statistics store is "empty/awaiting data" when the window recorded no calls
 *  at all — the single honest predicate the view (empty state) and the sidebar
 *  (muted nav item) share (NFR-CC-04). An empty store returns a zeroed model, so
 *  `calls_total === 0` is exactly the awaiting-data signal. */
export function isStatsEmpty(stats: StatsInfo): boolean {
  return stats.calls_total === 0;
}

// ── Derived rows (also the accessible data-table twins) ─────────────────────────

/** One point of the daily-activity series. */
export interface ActivityPoint {
  day: string;
  calls: number;
  ok_calls: number;
}

/** The daily-activity series, oldest day first (the read-model already orders it). */
export function activitySeries(stats: StatsInfo): ActivityPoint[] {
  return stats.activity_by_day.map((d) => ({ day: d.day, calls: d.calls, ok_calls: d.ok_calls }));
}

/** One ranked tool (calls summed across every surface it ran on). */
export interface ToolRow {
  tool: string;
  calls: number;
}

/**
 * The most-used tools, summing `calls_by_tool` across surfaces per tool, ranked by
 * calls desc (ties broken by tool name asc for a stable order), capped to
 * {@link TOP_TOOLS_LIMIT}. `truncated` is true when tools were dropped by the cap.
 */
export function topTools(stats: StatsInfo, limit: number = TOP_TOOLS_LIMIT): {
  rows: ToolRow[];
  truncated: boolean;
} {
  const byTool = new Map<string, number>();
  for (const u of stats.calls_by_tool) {
    byTool.set(u.tool, (byTool.get(u.tool) ?? 0) + u.calls);
  }
  const ranked = [...byTool.entries()]
    .map(([tool, calls]): ToolRow => ({ tool, calls }))
    .sort((a, b) => b.calls - a.calls || a.tool.localeCompare(b.tool));
  return { rows: ranked.slice(0, limit), truncated: ranked.length > limit };
}

/** One surface's total usage. */
export interface SurfaceRow {
  surface: string;
  calls: number;
}

/** Usage aggregated by recording surface (cli / mcp / watcher), ranked desc.
 *  Web-dashboard activity is filtered server-side (HF-1), so it never appears. */
export function bySurface(stats: StatsInfo): SurfaceRow[] {
  const totals = new Map<string, number>();
  for (const u of stats.calls_by_tool) {
    totals.set(u.surface, (totals.get(u.surface) ?? 0) + u.calls);
  }
  return [...totals.entries()]
    .map(([surface, calls]): SurfaceRow => ({ surface, calls }))
    .sort((a, b) => b.calls - a.calls || a.surface.localeCompare(b.surface));
}

/** One dev-vs-`main` bucket row; `isMain` distinguishes the baseline. */
export interface OriginRow {
  origin: string;
  calls: number;
  ok_calls: number;
  /** True for the `"main"` baseline (primary checkout + legacy rows); false for
   *  the `"dev"` bucket (all worktree branches combined). */
  isMain: boolean;
}

/** The dev-vs-`main` split — at most two rows (`"dev"`, `"main"`), ranked by calls
 *  desc (ties by origin asc). Rendered as charted (no normalisation): the read-model
 *  warns the split can sum to less than `calls_total` because rolled-up days carry
 *  no `origin` ([FR-OB-08]). */
export function originSplit(stats: StatsInfo): OriginRow[] {
  return stats.calls_by_origin
    .map((o: OriginUsage): OriginRow => ({
      origin: o.origin,
      calls: o.calls,
      ok_calls: o.ok_calls,
      isMain: o.origin === "main",
    }))
    .sort((a, b) => b.calls - a.calls || a.origin.localeCompare(b.origin));
}

// ── ECharts option builders (pure objects; the seam renders them) ───────────────

/** The loose ECharts option shape — kept structural so this module needs no
 *  ECharts import (the seam owns the dependency; jsdom tests need neither). */
export type EChartsOption = Record<string, unknown>;

const AXIS_LABEL = { color: MUTED, fontSize: 11 };
const SPLIT_LINE = { lineStyle: { color: GRIDLINE } };
const NO_LOAD_ANIMATION = { animationDuration: 200 };

/** The daily-activity line: a single warm series over the calendar days, with a
 *  soft area fill; merlin/muted axes and a light gridline (frontend-design §5). */
export function activityLineOption(points: ActivityPoint[]): EChartsOption {
  return {
    ...NO_LOAD_ANIMATION,
    grid: { left: 44, right: 16, top: 16, bottom: 28 },
    tooltip: { trigger: "axis" },
    xAxis: {
      type: "category",
      data: points.map((p) => p.day),
      axisLabel: { ...AXIS_LABEL },
      axisLine: { lineStyle: { color: MUTED } },
    },
    yAxis: {
      type: "value",
      minInterval: 1,
      axisLabel: { ...AXIS_LABEL },
      splitLine: { ...SPLIT_LINE },
    },
    series: [
      {
        type: "line",
        name: "calls",
        data: points.map((p) => p.calls),
        smooth: false,
        showSymbol: points.length <= 31,
        lineStyle: { color: SERIES_WARM, width: 2 },
        itemStyle: { color: SERIES_WARM },
        areaStyle: { color: SERIES_WARM, opacity: 0.12 },
      },
    ],
  };
}

const BAR_RADIUS = [0, 2, 2, 0];

/** The shared horizontal-bar skeleton (categories down the y-axis, highest at the
 *  top via `inverse`, values along x). Callers supply the category labels and the
 *  series `data` (plain numbers for a uniform-colour bar, or `{value,itemStyle}`
 *  objects for per-bar colour). An optional series-level `itemStyle` colours a
 *  uniform bar without per-item objects. */
function horizontalBarOption(
  labels: string[],
  data: unknown[],
  seriesItemStyle?: Record<string, unknown>,
): EChartsOption {
  const series: Record<string, unknown> = { type: "bar", data, barMaxWidth: 22 };
  if (seriesItemStyle) series.itemStyle = seriesItemStyle;
  return {
    ...NO_LOAD_ANIMATION,
    grid: { left: 8, right: 24, top: 12, bottom: 24, containLabel: true },
    tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
    xAxis: {
      type: "value",
      minInterval: 1,
      axisLabel: { ...AXIS_LABEL },
      splitLine: { ...SPLIT_LINE },
    },
    yAxis: {
      type: "category",
      data: labels,
      inverse: true,
      axisLabel: { ...AXIS_LABEL },
      axisLine: { lineStyle: { color: MUTED } },
    },
    series: [series],
  };
}

/** A horizontal ranked bar (top-tools / by-surface): one warm value series. */
export function rankedBarOption(labels: string[], values: number[]): EChartsOption {
  return horizontalBarOption(labels, values, { color: SERIES_WARM, borderRadius: BAR_RADIUS });
}

/** The dev-vs-`main` split: two horizontal bars, `main` in neutral merlin (the
 *  baseline) and the `dev` bucket (all worktree branches combined) in warm orange,
 *  so the colour itself reads "work done during increments vs on main" ([FR-OB-08]). */
export function originBarOption(rows: OriginRow[]): EChartsOption {
  return horizontalBarOption(
    rows.map((r) => r.origin),
    rows.map((r) => ({
      value: r.calls,
      itemStyle: { color: r.isMain ? NEUTRAL : SERIES_WARM, borderRadius: BAR_RADIUS },
    })),
  );
}
