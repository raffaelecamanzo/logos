/*
 * StatChart (S-235, FR-UI-27) — the imperative-ECharts-in-a-React-component mount,
 * the spa-frontend pattern the Graph/Quadrant canvases use. React owns the
 * surrounding layout; this component owns the ECharts instance behind a ref,
 * re-applies the option when it changes, resizes with the container, and disposes
 * on unmount.
 *
 * Accessibility (WCAG 2.1 AA, frontend-design §7): the canvas is `role="img"` with
 * a descriptive `label` — it is the picture, never the only surface. Each caller
 * pairs it with a visually-equivalent data table (the accessible twin) so
 * keyboard/screen-reader users read the same numbers.
 *
 * The ECharts import is isolated in `echarts.ts` (mocked in tests — jsdom has no
 * canvas), so this component is exercised without a headless-canvas dependency.
 */

import { useEffect, useRef } from "react";

import { createStatChart, type StatChartInstance } from "./echarts.ts";
import type { EChartsOption } from "./statsModel.ts";
import styles from "./StatisticsView.module.css";

export interface StatChartProps {
  /** The pre-built ECharts option (from `statsModel.ts`). */
  option: EChartsOption;
  /** The accessible name for the `role="img"` canvas. */
  label: string;
}

export function StatChart({ option, label }: StatChartProps) {
  const mountRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<StatChartInstance | null>(null);

  // Instantiate once; dispose on unmount. Kept separate from the option effect so a
  // window-change (new option) reuses the instance rather than tearing it down.
  useEffect(() => {
    const el = mountRef.current;
    if (!el) return;
    const chart = createStatChart(el);
    chartRef.current = chart;
    const onResize = () => chart.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.dispose();
      chartRef.current = null;
    };
  }, []);

  // Re-apply the option whenever it changes (a window-selector re-query hands a new
  // option object). `notMerge` clears the prior series so a shrinking dataset never
  // leaves stale marks behind.
  useEffect(() => {
    chartRef.current?.setOption(option, { notMerge: true });
  }, [option]);

  return <div ref={mountRef} className={styles.chart} role="img" aria-label={label} />;
}
