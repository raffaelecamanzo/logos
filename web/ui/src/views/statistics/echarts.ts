/*
 * The Statistics view's ECharts seam (S-235, FR-UI-27; spa-frontend
 * "imperative-canvas-in-a-component"). Imports ECharts **selectively** from
 * `echarts/core` — only the line + bar charts, the grid + tooltip components,
 * and the canvas renderer — so the bundle carries these series and nothing else
 * (NFR-PC-04 size budget). The Graph view has its own graph-only seam; keeping this
 * one separate means neither tab pulls the other's series.
 *
 * ECharts is a build-time npm dependency bundled into the hashed JS by Vite; it
 * names no external origin, so the self-only CSP and offline posture are unaffected
 * (NFR-SE-01). This is the single import site for ECharts in this view, so the rest
 * of the view stays ECharts-free and component tests mock one small module (jsdom
 * has no real canvas); the pure option-building lives in `statsModel.ts`.
 */

import * as echarts from "echarts/core";
import { BarChart, LineChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

echarts.use([BarChart, LineChart, GridComponent, TooltipComponent, CanvasRenderer]);

/** The minimal ECharts instance surface the chart component drives (also the mock
 *  target in tests). */
export interface StatChartInstance {
  setOption(option: unknown, opts?: { notMerge?: boolean }): void;
  resize(): void;
  dispose(): void;
}

/** Instantiate a canvas-rendered chart into `el`. */
export function createStatChart(el: HTMLElement): StatChartInstance {
  return echarts.init(el, null, { renderer: "canvas" }) as unknown as StatChartInstance;
}
