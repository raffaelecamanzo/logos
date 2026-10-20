/*
 * Rendered-output checks for the ECharts seams (S-776, NFR-SE-11). jsdom has no
 * canvas, so the component tests mock `echarts.ts` and assert the option object.
 * That proves what was ASKED for, not what was DRAWN: NFR-SE-11 wants the literal
 * name asserted on the rendered output. This helper lets a test run the REAL
 * ECharts against a recording 2D context instead.
 *
 * - `recordCanvas()` stubs `HTMLCanvasElement.getContext` with a context that
 *   draws nothing but records every `fillText` / `strokeText` string and where it
 *   landed, and gives every element an 800×600 layout size so ECharts lays the
 *   chart out.
 * - `paint(canvas, el)` draws one complete frame of the chart mounted in `el` and
 *   returns it.
 * - `showTooltip(el, dataIndex)` opens the tooltip on one data item, as a hover
 *   would. An HTML tooltip lands in the DOM under `el`; a rich-text one is drawn
 *   on the canvas and shows up in the next frame.
 *
 * The ECharts import here is the same `echarts/core` module the seams register
 * their charts on, so `getInstanceByDom` finds the instance a component created.
 */

import { getInstanceByDom } from "echarts/core";

/** A fixture name carrying markup: rendered as text, it creates no element. */
export const HOSTILE_NAME = "<img src=x onerror=alert(1)>";

/** One drawn string and where it landed, in canvas pixels. */
export interface DrawnText {
  text: string;
  x: number;
  y: number;
}

export interface CanvasRecording {
  /** Every string drawn with `fillText` / `strokeText`, in draw order. */
  drawn: DrawnText[];
  /** The strings alone, in draw order. */
  texts(): string[];
  /** Forget what was drawn so far (the next `paint` starts a fresh frame). */
  clear(): void;
  /** Undo the stubs. */
  restore(): void;
}

/** A 2D affine transform `[a, b, c, d, e, f]`, as `setTransform` takes it. */
type Matrix = [number, number, number, number, number, number];
const IDENTITY: Matrix = [1, 0, 0, 1, 0, 0];

function multiply(m: Matrix, n: Matrix): Matrix {
  return [
    m[0] * n[0] + m[2] * n[1],
    m[1] * n[0] + m[3] * n[1],
    m[0] * n[2] + m[2] * n[3],
    m[1] * n[2] + m[3] * n[3],
    m[0] * n[4] + m[2] * n[5] + m[4],
    m[1] * n[4] + m[3] * n[5] + m[5],
  ];
}

const WIDTH = 800;
const HEIGHT = 600;

/** A 2D context that draws nothing and records each drawn string with its
 *  position under the current transform (which it tracks through `save` /
 *  `restore`). Any member ECharts sets is stored; any it calls is a no-op, except
 *  the few whose return value it reads. */
function recordingContext(canvas: HTMLCanvasElement, drawn: DrawnText[]): CanvasRenderingContext2D {
  const state: Record<string | symbol, unknown> = { canvas };
  let matrix: Matrix = IDENTITY;
  const stack: Matrix[] = [];
  const nums = (args: unknown[]) => args.map(Number) as Matrix;
  const record = (text: unknown, x: unknown, y: unknown) => {
    const [px, py] = [Number(x), Number(y)];
    drawn.push({
      text: String(text),
      x: matrix[0] * px + matrix[2] * py + matrix[4],
      y: matrix[1] * px + matrix[3] * py + matrix[5],
    });
  };
  const methods: Record<string, (...args: unknown[]) => unknown> = {
    fillText: record,
    strokeText: record,
    save: () => void stack.push(matrix),
    restore: () => void (matrix = stack.pop() ?? IDENTITY),
    setTransform: (...args) => void (matrix = nums(args)),
    resetTransform: () => void (matrix = IDENTITY),
    transform: (...args) => void (matrix = multiply(matrix, nums(args))),
    translate: (x, y) => void (matrix = multiply(matrix, [1, 0, 0, 1, Number(x), Number(y)])),
    scale: (x, y) => void (matrix = multiply(matrix, [Number(x), 0, 0, Number(y), 0, 0])),
    rotate: (r) => {
      const [cos, sin] = [Math.cos(Number(r)), Math.sin(Number(r))];
      matrix = multiply(matrix, [cos, sin, -sin, cos, 0, 0]);
    },
    measureText: (text) => ({ width: String(text).length * 6 }),
    createLinearGradient: () => ({ addColorStop() {} }),
    createRadialGradient: () => ({ addColorStop() {} }),
    createPattern: () => ({}),
    getImageData: () => ({ data: new Uint8ClampedArray(4) }),
    getLineDash: () => [],
    isPointInPath: () => false,
    isPointInStroke: () => false,
  };
  return new Proxy(state, {
    get: (target, key) => {
      if (key in target) return target[key];
      if (typeof key === "string" && key in methods) return methods[key];
      return () => {};
    },
    set: (target, key, value) => {
      target[key] = value;
      return true;
    },
  }) as unknown as CanvasRenderingContext2D;
}

/** Install the recording canvas and a layout size for every element. */
export function recordCanvas(): CanvasRecording {
  const drawn: DrawnText[] = [];
  const proto = HTMLCanvasElement.prototype;
  const originalGetContext = proto.getContext;
  // jsdom defines the size getters on `Element.prototype` (always 0); the stub
  // shadows them on `HTMLElement.prototype`, so restoring is deleting the shadow.
  const sizes = (["clientWidth", "clientHeight"] as const).map((key) => ({
    key,
    own: Object.getOwnPropertyDescriptor(HTMLElement.prototype, key),
  }));
  proto.getContext = function getContext(this: HTMLCanvasElement) {
    return recordingContext(this, drawn);
  } as unknown as typeof proto.getContext;
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => WIDTH });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => HEIGHT });
  return {
    drawn,
    texts: () => drawn.map((d) => d.text),
    clear: () => void drawn.splice(0),
    restore() {
      proto.getContext = originalGetContext;
      for (const { key, own } of sizes) {
        if (own) Object.defineProperty(HTMLElement.prototype, key, own);
        else delete (HTMLElement.prototype as unknown as Record<string, unknown>)[key];
      }
    },
  };
}

/** The ECharts instance mounted in `el` (throws if there is none). */
function chartIn(el: Element) {
  const chart = getInstanceByDom(el as HTMLElement);
  if (!chart) throw new Error("no ECharts instance is mounted in this element");
  return chart;
}

/** Draw one complete frame of the chart in `el` and return what it drew; the
 *  recording then holds exactly that frame. Pending updates are applied first
 *  (zrender only redraws what changed), then everything on screen is drawn. */
export function paint(canvas: CanvasRecording, el: Element): DrawnText[] {
  const zr = chartIn(el).getZr();
  zr.flush();
  canvas.clear();
  zr.painter.refresh({ paintAll: true });
  return canvas.drawn.slice();
}

/** Open the tooltip on data item `dataIndex` of series 0, as a hover would. */
export function showTooltip(el: Element, dataIndex: number): void {
  chartIn(el).dispatchAction({ type: "showTip", seriesIndex: 0, dataIndex });
}
