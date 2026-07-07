/*
 * The ECharts seam (S-186, FR-UI-08, spa-frontend "imperative-canvas-in-a-
 * component"). Imports ECharts **selectively** from `echarts/core` — only the
 * graph chart, tooltip, and the canvas renderer — so the bundle carries the graph
 * series and nothing else (NFR-PC-04 size budget; the legacy used a slim
 * graph-only build). ECharts is a build-time npm dependency bundled into the
 * hashed JS by Vite; it names no external origin, so the self-only CSP and offline
 * posture are unaffected (NFR-SE-01).
 *
 * This module is the single import site for ECharts, which keeps the rest of the
 * view ECharts-free and gives component tests one small module to mock (jsdom has
 * no real canvas), while the pure option-building (`graphOptions.ts`) is tested
 * directly.
 */

import * as echarts from "echarts/core";
import { GraphChart } from "echarts/charts";
import { TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

echarts.use([GraphChart, TooltipComponent, CanvasRenderer]);

/** The minimal ECharts instance surface the canvas drives (also the mock target). */
export interface GraphChartInstance {
  setOption(option: unknown, opts?: { replaceMerge?: string[] }): void;
  on(event: string, handler: (params: GraphEventParams) => void): void;
  dispatchAction(action: Record<string, unknown>): void;
  resize(): void;
  dispose(): void;
}

/** The subset of an ECharts event payload the canvas reads (click / roam). */
export interface GraphEventParams {
  dataType?: string;
  name?: string;
  data?: { id?: string; displayLabel?: string; kind?: string | null; edge_type?: string | null };
  zoom?: number;
}

/** Instantiate a canvas-rendered graph chart into `el`. */
export function createGraphChart(el: HTMLElement): GraphChartInstance {
  return echarts.init(el, null, { renderer: "canvas" }) as unknown as GraphChartInstance;
}
