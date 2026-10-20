import { act, cleanup, render } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { GraphElementNode } from "../../api/types.ts";
import {
  HOSTILE_NAME,
  paint,
  recordCanvas,
  showTooltip,
  type CanvasRecording,
  type DrawnText,
} from "../../test/renderedChart.ts";
import { buildServiceMap } from "../workspace/serviceMapModel.ts";
import { GraphCanvas, type GraphCanvasHandle } from "./GraphCanvas.tsx";
import { LABEL_NODE_LIMIT, type GraphSelection } from "./graphOptions.ts";
import { loadedFrom } from "./graphModel.ts";

// The REAL ECharts, unmocked, drawing into a recording canvas (S-776, NFR-SE-11).
// GraphView.test.tsx mocks the seam and asserts what is sent; this file asserts
// what is drawn. Timers are fake, so the force layout only steps when a test
// advances them, and `Math.random` is seeded, so the layout's start is the same
// on every run.

const NO_SELECTION: GraphSelection = { seed: null, focusId: null, lockedId: null, locatedId: null, depth: 0 };

const NODES: GraphElementNode[] = [
  { id: "scip::x", label: HOSTILE_NAME, kind: "function", layer: "code" },
  { id: "scip::b", label: "beta", kind: "struct", layer: "code" },
  { id: "scip::c", label: "gamma", kind: "struct", layer: "code" },
];
const LOADED = loadedFrom(NODES, [{ source: "scip::x", target: "scip::b", edge_type: "calls" }]);

let canvas: CanvasRecording;

beforeEach(() => {
  vi.useFakeTimers();
  let seed = 42;
  vi.spyOn(Math, "random").mockImplementation(() => {
    seed = (seed * 16807) % 2147483647;
    return seed / 2147483647;
  });
  canvas = recordCanvas();
});
afterEach(() => {
  cleanup();
  canvas.restore();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function mount(selection: GraphSelection = NO_SELECTION, loaded = LOADED) {
  const ref = createRef<GraphCanvasHandle>();
  const view = render(<GraphCanvas ref={ref} loaded={loaded} selection={selection} onNodeClick={() => {}} />);
  const el = view.container.querySelector('[role="application"]')!;
  const rerender = (next: GraphSelection) =>
    act(() => view.rerender(<GraphCanvas ref={ref} loaded={loaded} selection={next} onNodeClick={() => {}} />));
  /** One complete frame of what the canvas shows now. */
  const frame = () => paint(canvas, el);
  return { el, ref, rerender, frame, container: view.container };
}

/** Where `text` is drawn in `frame` (throws if it is not). */
function at(frame: DrawnText[], text: string): DrawnText {
  const hit = frame.find((d) => d.text === text);
  if (!hit) throw new Error(`"${text}" is not drawn; drawn: ${JSON.stringify(frame.map((d) => d.text))}`);
  return hit;
}

const count = (frame: DrawnText[], text: string) => frame.filter((d) => d.text === text).length;

describe("the graph canvas draws a repository name as text (S-776, NFR-SE-11)", () => {
  it("draws a node label carrying markup as that literal text, and creates no element", () => {
    const { frame, container } = mount();
    expect(count(frame(), HOSTILE_NAME)).toBe(1);
    expect(container.querySelector("img")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
  });

  it("draws the node's tooltip as literal text on the canvas, never as HTML", () => {
    const { el, frame, container } = mount();
    expect(count(frame(), HOSTILE_NAME), "before the hover, only the label").toBe(1);

    showTooltip(el, 0);
    const hovered = frame();
    // The rich-text tooltip is drawn on the canvas: the name, then its kind line.
    expect(count(hovered, HOSTILE_NAME), "the hover adds the tooltip's copy of the name").toBe(2);
    const texts = hovered.map((d) => d.text);
    expect(texts[texts.lastIndexOf(HOSTILE_NAME) + 1]).toBe("function");
    // No HTML tooltip: an HTML one would put the name in the DOM under the mount.
    expect(el.textContent).toBe("");
    expect(container.querySelector("img")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
  });
});

// The canvas tooltip is ECharts rich text, which reads `{style|text}` as styling
// markup. A name such as a templated doc heading must still be drawn whole.
describe("the graph tooltip draws rich-text markup in a name as text (S-776, NFR-SE-11)", () => {
  const RICH = "Render {{title|upper}} and {a|b}";

  it("draws the name's own characters, with none taken as styling", () => {
    const loaded = loadedFrom([{ id: "scip::t", label: RICH, kind: "doc_section", layer: "doc" }], []);
    const { el, frame } = mount(NO_SELECTION, loaded);
    expect(count(frame(), RICH), "the label is plain text").toBe(1);

    showTooltip(el, 0);
    // A zero-width space may separate characters; what is drawn reads as the name.
    const visible = frame().map((d) => d.text.replaceAll("\u200b", ""));
    expect(visible.filter((t) => t === RICH), "the tooltip draws the whole name").toHaveLength(2);
  });
});

// The Workspace service map is this same GraphCanvas over a set the service-map
// model builds; member and topic names come from the workspace's repositories.
describe("the Workspace service map draws member and topic names as text (S-776, NFR-SE-11)", () => {
  const TOPIC = "<svg onload=alert(2)>";
  const map = buildServiceMap(
    [
      { name: HOSTILE_NAME, warmState: "warm", indexed: true, error: null },
      { name: "billing", warmState: "warm", indexed: true, error: null },
    ],
    [],
    [
      { member: HOSTILE_NAME, topics: [{ topic: TOPIC, producers: 1, consumers: 0 }] },
      { member: "billing", topics: [{ topic: TOPIC, producers: 0, consumers: 1 }] },
    ],
  );

  it("draws each name as its literal label, and the hovered member's tooltip as text", () => {
    expect(Object.values(map.loaded.nodes).map((n) => n.label)).toEqual(
      expect.arrayContaining([HOSTILE_NAME, TOPIC]),
    );
    const { el, frame } = mount(NO_SELECTION, map.loaded);
    const before = frame();
    expect(count(before, HOSTILE_NAME)).toBe(1);
    expect(count(before, TOPIC)).toBe(1);

    showTooltip(el, Object.values(map.loaded.nodes).findIndex((n) => n.label === HOSTILE_NAME));
    const texts = frame().map((d) => d.text);
    expect(texts.filter((t) => t === HOSTILE_NAME)).toHaveLength(2);
    expect(texts[texts.lastIndexOf(HOSTILE_NAME) + 1]).toBe("service");
    expect(el.textContent).toBe("");
    expect(document.querySelector("img, svg")).toBeNull();
  });
});

describe("the graph canvas rules hold on rendered state (S-776, echarts 6.1)", () => {
  it("keeps a pinned node still while the force layout moves the others", () => {
    const { frame } = mount({ ...NO_SELECTION, lockedId: "scip::b" });
    const before = frame();

    act(() => void vi.advanceTimersByTime(1000)); // run the force layout
    const after = frame();
    expect(at(after, "beta"), "the locked node is fixed").toEqual(at(before, "beta"));
    expect(at(after, "gamma"), "an unpinned node is free to move").not.toEqual(at(before, "gamma"));
  });

  it("restyles a selection-only change in place: the layout is kept, not re-run", () => {
    const { frame, rerender } = mount();
    const before = at(frame(), "gamma");

    rerender({ ...NO_SELECTION, lockedId: "scip::b" });
    // A `replaceMerge` would restart the force layout from fresh random positions
    // (hundreds of pixels away); a plain merge keeps gamma where it was drawn.
    const after = at(frame(), "gamma");
    expect(Math.hypot(after.x - before.x, after.y - before.y)).toBeLessThan(10);
  });

  it("hides labels on a large set when zoomed out, and draws them again when zoomed back in", () => {
    const many = loadedFrom(
      [
        ...NODES,
        ...Array.from({ length: LABEL_NODE_LIMIT }, (_, k): GraphElementNode => ({
          id: `scip::n${k}`,
          label: `n${k}`,
          kind: "function",
          layer: "code",
        })),
      ],
      [],
    );
    const { frame, ref } = mount(NO_SELECTION, many);
    expect(count(frame(), HOSTILE_NAME), "home zoom shows labels").toBe(1);

    act(() => ref.current!.zoomOut());
    expect(count(frame(), HOSTILE_NAME), "zoomed out past the threshold, a large set draws no label").toBe(0);

    act(() => ref.current!.zoomIn());
    expect(count(frame(), HOSTILE_NAME), "back at home zoom, the labels return").toBe(1);
  });
});
