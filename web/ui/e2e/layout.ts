// Computed-style layout assertions for the widget standard (S-611, FR-UI-40).
// Every later story's layout criterion calls `expectWidgetStackLayout` on a real
// view's stack; nothing here knows about any one view.
import { expect, type Locator } from "@playwright/test";

export interface StackMeasure {
  /** The stack's own computed row gap, in pixels. */
  rowGap: number;
  /** The vertical distance between each pair of consecutive widgets. */
  gaps: number[];
  /** Computed `text-align` of every widget part and copy element. */
  textAligns: { part: string; textAlign: string }[];
  /**
   * How far each block of a widget starts from its frame's left edge, in pixels.
   * `text-align` alone misses a part that is centred by flex alignment
   * (`align-items`/`justify-content`), so left alignment is also read as geometry.
   */
  leftOffsets: { part: string; offset: number }[];
  /** Per widget: the computed font size of the explanation and of the action. */
  bodySizes: { what: string; why: string; action: string; actionLine: string }[];
}

// The blocks that must start at the frame's left edge: every part, the copy
// paragraphs, the where chip, an absence statement, the title and the first
// figure. (The action TEXT is inline after its label, so it is not one of them.)
const LEFT_EDGE_BLOCKS = [
  "[data-widget-part]",
  '[data-widget-part="title"] > h3',
  '[data-widget-part="figure"] > :first-child',
  '[data-widget-copy="what"]',
  '[data-widget-copy="why"]',
  '[data-widget-copy="where"]',
  "[data-widget-absence]",
].join(", ");

/** Reads the stack's geometry and type in the browser. */
export async function measureStack(stack: Locator): Promise<StackMeasure> {
  return stack.evaluate((el, leftEdgeBlocks) => {
    const label = (node: Element) =>
      node.getAttribute("data-widget-part") ??
      node.getAttribute("data-widget-copy") ??
      (node.hasAttribute("data-widget-absence") ? "absence" : node.tagName.toLowerCase());
    const children = [...el.children] as HTMLElement[];
    const gaps = children.slice(1).map((child, i) => {
      const prev = children[i].getBoundingClientRect();
      return child.getBoundingClientRect().top - prev.bottom;
    });
    const textAligns = [
      ...el.querySelectorAll<HTMLElement>("[data-widget], [data-widget-part], [data-widget-copy], [data-widget-absence]"),
    ].map((node) => ({
      part: node.hasAttribute("data-widget") ? "widget" : label(node),
      textAlign: getComputedStyle(node).textAlign,
    }));
    const frames = [...el.querySelectorAll("[data-widget]")];
    const leftOffsets = frames.flatMap((frame) => {
      const left = frame.getBoundingClientRect().left + parseFloat(getComputedStyle(frame).paddingLeft);
      return [...frame.querySelectorAll(leftEdgeBlocks)].map((node) => ({
        part: label(node),
        offset: node.getBoundingClientRect().left - left,
      }));
    });
    const size = (node: Element | null | undefined) => (node ? getComputedStyle(node).fontSize : "missing");
    const bodySizes = frames.map((frame) => {
      const action = frame.querySelector('[data-widget-copy="action"]');
      return {
        what: size(frame.querySelector('[data-widget-copy="what"]')),
        why: size(frame.querySelector('[data-widget-copy="why"]')),
        // The action text itself, and the line that carries it with its label.
        action: size(action),
        actionLine: size(action?.closest("p")),
      };
    });
    return { rowGap: parseFloat(getComputedStyle(el).rowGap), gaps, textAligns, leftOffsets, bodySizes };
  }, LEFT_EDGE_BLOCKS);
}

/**
 * The FR-UI-40 layout rules over one rendered `WidgetStack`:
 *  - consecutive widgets are separated by ONE gap, and it is the stack's token;
 *  - every widget part computes `text-align: start` or `left`, and every block
 *    of a widget starts at its frame's left edge;
 *  - in every widget, explanation and action share one font size.
 */
export async function expectWidgetStackLayout(stack: Locator): Promise<StackMeasure> {
  const m = await measureStack(stack);
  expect(m.gaps.length, "a stack of fewer than two widgets has no gap to check").toBeGreaterThan(0);
  expect(m.rowGap, "the stack declares a gap").toBeGreaterThan(0);
  for (const [i, gap] of m.gaps.entries()) {
    expect(gap, `gap ${i + 1} of ${m.gaps.length} between consecutive widgets (stack gap ${m.rowGap}px)`).toBeCloseTo(
      m.rowGap,
      1,
    );
  }
  for (const { part, textAlign } of m.textAligns) {
    expect(["start", "left"], `text-align of the ${part} part`).toContain(textAlign);
  }
  for (const { part, offset } of m.leftOffsets) {
    expect(offset, `the ${part} block starts at its widget's left edge`).toBeCloseTo(0, 0);
  }
  for (const [i, s] of m.bodySizes.entries()) {
    expect(s.why, `widget ${i + 1}: why and what share one size`).toBe(s.what);
    expect(s.action, `widget ${i + 1}: the action text and the explanation share one size`).toBe(s.what);
    expect(s.actionLine, `widget ${i + 1}: the action line and the explanation share one size`).toBe(s.what);
  }
  return m;
}
