/*
 * The uPlot seam (S-188, FR-UI-17, spa-frontend "imperative-canvas-in-a-
 * component"). The single import site for uPlot — a build-time npm dependency
 * bundled into the hashed JS by Vite, naming no external origin (its github
 * license banner is stripped by `esbuild.legalComments: "none"`, vite.config.ts),
 * so the self-only CSP and offline posture are unaffected (NFR-SE-01, NFR-SE-06).
 *
 * It ports the legacy `web/assets/quadrant.js` render: a flipped 2×2 grid (best
 * top-right) with four shaded cell regions and per-point colour + blast-radius
 * size, drawn onto uPlot's 2D canvas. The pure point *placement* (plotX/plotY/
 * radiusFor) lives in `analyticsModel.ts` and is unit-tested directly; this module
 * holds only the uPlot wiring, which component tests mock (jsdom has no canvas) —
 * exactly as the graph view mocks `echarts.ts`.
 */

import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

import { plotX, plotY, radiusFor, type ScatterPoint } from "./analyticsModel.ts";

/** The quadrant palette (frontend-design §4.12 / CR-040 Gartner numbering): the
 *  surprising disagreements pop (Q1 red, Q2 orange) while Q4 trust (green) and Q3
 *  gap (muted) stay calm. Colour is a redundant channel — the cell labels + the
 *  accessible table carry the same classification. */
const COLORS: Record<string, string> = {
  q1: "#da291c", // false-green, worst
  q2: "#e35205", // dead/guarded edge
  q3: "#716b5d", // true gap
  q4: "#16a34a", // trust, best
};
/** Faint cell-region fills (the shaded 2×2 background). */
const CELL_FILLS = {
  tl: "rgba(218, 41, 28, 0.07)", // Q1 false-green
  tr: "rgba(22, 163, 74, 0.08)", // Q4 trust
  bl: "rgba(113, 107, 93, 0.06)", // Q3 true gap
  br: "rgba(227, 82, 5, 0.07)", // Q2 dead edge
};
const MERLIN = "#3d3935"; // axes / gridlines

/** The minimal instance surface the React component drives (also the mock target). */
export interface QuadrantChartInstance {
  setSize(size: { width: number; height: number }): void;
  destroy(): void;
}

/** Instantiate the flipped 2×2 quadrant chart for `points` into `el`. */
export function createQuadrantChart(
  el: HTMLElement,
  points: ScatterPoint[],
  width: number,
): QuadrantChartInstance {
  const xs = points.map((p, i) => plotX(p, i));
  const ys = points.map((p, i) => plotY(p, i));
  const data: uPlot.AlignedData = [xs, ys];

  function drawPoints(u: uPlot, seriesIdx: number, i0: number, i1: number): boolean {
    const ctx = u.ctx;
    ctx.save();
    for (let i = i0; i <= i1; i++) {
      const p = points[i];
      if (p == null) continue;
      const cx = u.valToPos(u.data[0][i] as number, "x", true);
      const cy = u.valToPos(u.data[seriesIdx][i] as number, "y", true);
      ctx.beginPath();
      ctx.globalAlpha = 0.72;
      ctx.fillStyle = COLORS[p.q] || MERLIN;
      ctx.arc(cx, cy, radiusFor(p.w), 0, 2 * Math.PI);
      ctx.fill();
    }
    ctx.restore();
    return false; // we drew the points; uPlot draws none of its own.
  }

  function drawCells(u: uPlot): void {
    const ctx = u.ctx;
    const left = u.bbox.left;
    const right = u.bbox.left + u.bbox.width;
    const top = u.bbox.top;
    const bot = u.bbox.top + u.bbox.height;
    const xMid = u.valToPos(0.5, "x", true);
    const yMid = u.valToPos(0.5, "y", true);
    ctx.save();
    ctx.fillStyle = CELL_FILLS.tl;
    ctx.fillRect(left, top, xMid - left, yMid - top);
    ctx.fillStyle = CELL_FILLS.tr;
    ctx.fillRect(xMid, top, right - xMid, yMid - top);
    ctx.fillStyle = CELL_FILLS.bl;
    ctx.fillRect(left, yMid, xMid - left, bot - yMid);
    ctx.fillStyle = CELL_FILLS.br;
    ctx.fillRect(xMid, yMid, right - xMid, bot - yMid);
    ctx.strokeStyle = MERLIN;
    ctx.globalAlpha = 0.35;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(xMid, top);
    ctx.lineTo(xMid, bot);
    ctx.moveTo(left, yMid);
    ctx.lineTo(right, yMid);
    ctx.stroke();
    ctx.globalAlpha = 0.85;
    ctx.font = "600 11px system-ui, sans-serif";
    const pad = 6;
    ctx.textBaseline = "top";
    ctx.textAlign = "left";
    ctx.fillStyle = COLORS.q1;
    ctx.fillText("Q1 false-green", left + pad, top + pad);
    ctx.fillStyle = COLORS.q3;
    ctx.fillText("Q3 true gap", left + pad, bot - 16);
    ctx.textAlign = "right";
    ctx.fillStyle = COLORS.q4;
    ctx.fillText("Q4 trust ★", right - pad, top + pad);
    ctx.fillStyle = COLORS.q2;
    ctx.fillText("Q2 dead edge", right - pad, bot - 16);
    ctx.restore();
  }

  const opts: uPlot.Options = {
    width: width || 640,
    height: 360,
    cursor: { y: false },
    legend: { show: false },
    scales: {
      x: { time: false, range: [0, 1] },
      y: { range: [0, 1] },
    },
    axes: [
      {
        stroke: MERLIN,
        grid: { show: false },
        ticks: { show: false },
        label: "Reachability",
        splits: () => [0.25, 0.75],
        values: (_u, vals) =>
          vals.map((v) => (v === 0.25 ? "unreachable" : v === 0.75 ? "reachable" : "")),
      },
      {
        stroke: MERLIN,
        grid: { show: false },
        ticks: { show: false },
        size: 84,
        label: "Runtime executed",
        splits: () => [0.25, 0.75],
        values: (_u, vals) =>
          vals.map((v) => (v === 0.25 ? "0% (dead)" : v === 0.75 ? "executed" : "")),
      },
    ],
    series: [
      {},
      {
        label: "symbols",
        stroke: MERLIN,
        paths: () => null,
        points: { show: drawPoints },
      },
    ],
    hooks: { draw: [drawCells] },
  };

  const chart = new uPlot(opts, data, el);
  return {
    setSize: (size) => chart.setSize(size),
    destroy: () => chart.destroy(),
  };
}
