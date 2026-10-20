import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  HOSTILE_NAME,
  paint,
  recordCanvas,
  showTooltip,
  type CanvasRecording,
} from "../../test/renderedChart.ts";
import { StatChart } from "./StatChart.tsx";
import { originBarOption, rankedBarOption, type EChartsOption } from "./statsModel.ts";

// The REAL ECharts, unmocked, drawing into a recording canvas (S-776, NFR-SE-11).
// StatisticsView.test.tsx mocks the seam and asserts what is sent; this file
// asserts what is drawn. Every Statistics chart that shows a name is a horizontal
// bar whose names are the category axis: `rankedBarOption` (top tools, by
// surface) and `originBarOption` (the dev-vs-main split, whose bucket names are
// charted as the read-model sends them). The activity line's axis shows calendar
// days, never a name.

let canvas: CanvasRecording;

beforeEach(() => {
  canvas = recordCanvas();
});
afterEach(() => {
  cleanup();
  canvas.restore();
});

/** Mount the chart and return its element once ECharts has drawn the option. */
async function mount(option: EChartsOption) {
  const { container } = render(<StatChart option={option} label="chart" />);
  const el = container.querySelector('[role="img"]')!;
  // The option is applied from an effect (memory: await effect side-effects).
  await waitFor(() => expect(paint(canvas, el).length).toBeGreaterThan(0));
  return { el, container };
}

const CHARTS: [string, EChartsOption][] = [
  ["the ranked bar (top tools, by surface)", rankedBarOption([HOSTILE_NAME, "other"], [3, 1])],
  [
    "the dev-vs-main origin bar",
    originBarOption([
      { origin: HOSTILE_NAME, calls: 3, ok_calls: 3, isMain: false },
      { origin: "main", calls: 1, ok_calls: 1, isMain: true },
    ]),
  ],
];

describe.each(CHARTS)("%s renders a name as text (S-776, NFR-SE-11)", (_, option) => {
  it("draws the category label carrying markup as that literal text", async () => {
    const { el, container } = await mount(option);
    expect(paint(canvas, el).map((d) => d.text)).toContain(HOSTILE_NAME);
    expect(container.querySelector("img")).toBeNull();
  });

  it("shows the name in the HTML tooltip as text, creating no element", async () => {
    const { el, container } = await mount(option);
    showTooltip(el, 0);
    // The axis tooltip is an HTML box ECharts appends under the chart element.
    await waitFor(() => expect(el.textContent).toContain(HOSTILE_NAME));
    expect(el.querySelector("img")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
  });
});

describe("StatChart drives the real chart (S-776, echarts 6.1)", () => {
  it("redraws a new option in place of the old one", async () => {
    const { container, rerender } = render(
      <StatChart option={rankedBarOption(["first"], [1])} label="chart" />,
    );
    const el = container.querySelector('[role="img"]')!;
    await waitFor(() => expect(paint(canvas, el).map((d) => d.text)).toContain("first"));

    rerender(<StatChart option={rankedBarOption(["second"], [2])} label="chart" />);
    await waitFor(() => expect(paint(canvas, el).map((d) => d.text)).toContain("second"));
    // The old category is no longer drawn. (This does not prove `notMerge`: each
    // builder emits one series and replaces its data whole, so a plain merge
    // draws the same frame.)
    expect(paint(canvas, el).map((d) => d.text)).not.toContain("first");
  });

  it("disposes the chart on unmount", async () => {
    const { el } = await mount(rankedBarOption(["only"], [1]));
    cleanup();
    expect(() => paint(canvas, el)).toThrow(/no ECharts instance/);
  });
});
