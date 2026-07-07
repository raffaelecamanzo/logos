/*
 * QuadrantChart (S-188, FR-UI-17) — the imperative-uPlot-in-a-React-component
 * mount, the spa-frontend pattern for re-homing the 2×2 quadrant canvas. React
 * owns the surrounding layout; this component owns the uPlot instance behind a ref
 * and re-creates it when the points or the container width change, tidying up on
 * unmount. The canvas is `role="img"` with a descriptive label, and the accessible
 * urgency table beside it carries the same data for keyboard/screen-reader users
 * (WCAG 2.1 AA) — the canvas is the picture, never the only surface.
 *
 * The uPlot import is isolated in `uplot.ts` (mocked in tests — jsdom has no
 * canvas), so this component is exercised without a headless-canvas dependency.
 */

import { useEffect, useRef } from "react";

import type { ScatterPoint } from "./analyticsModel.ts";
import { createQuadrantChart, type QuadrantChartInstance } from "./uplot.ts";
import styles from "./AnalyticsView.module.css";

export interface QuadrantChartProps {
  /** The placed scatter points (n/a symbols already excluded by `scatterPoints`). */
  points: ScatterPoint[];
}

export function QuadrantChart({ points }: QuadrantChartProps) {
  const mountRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<QuadrantChartInstance | null>(null);

  useEffect(() => {
    const el = mountRef.current;
    if (!el) return;
    const chart = createQuadrantChart(el, points, el.clientWidth || 640);
    chartRef.current = chart;
    const onResize = () => chart.setSize({ width: el.clientWidth || 640, height: 360 });
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      chart.destroy();
      chartRef.current = null;
    };
  }, [points]);

  return (
    <div
      ref={mountRef}
      className={styles.quadrantChart}
      role="img"
      aria-label="Reachability by runtime-coverage 2×2 grid, best top-right (the urgency table below carries the same data)"
    />
  );
}
