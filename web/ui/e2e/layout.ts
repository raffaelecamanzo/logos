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
  /** Per widget: the computed font size of the explanation and of the action. */
  bodySizes: { what: string; why: string; action: string }[];
}

/** Reads the stack's geometry and type in the browser. */
export async function measureStack(stack: Locator): Promise<StackMeasure> {
  return stack.evaluate((el) => {
    const children = [...el.children] as HTMLElement[];
    const gaps = children.slice(1).map((child, i) => {
      const prev = children[i].getBoundingClientRect();
      return child.getBoundingClientRect().top - prev.bottom;
    });
    const textAligns = [
      ...el.querySelectorAll<HTMLElement>("[data-widget], [data-widget-part], [data-widget-copy], [data-widget-absence]"),
    ].map((node) => ({
      part:
        node.getAttribute("data-widget-part") ??
        node.getAttribute("data-widget-copy") ??
        (node.hasAttribute("data-widget-absence") ? "absence" : "widget"),
      textAlign: getComputedStyle(node).textAlign,
    }));
    const size = (node: Element | null | undefined) => (node ? getComputedStyle(node).fontSize : "missing");
    const bodySizes = [...el.querySelectorAll("[data-widget]")].map((frame) => ({
      what: size(frame.querySelector('[data-widget-copy="what"]')),
      why: size(frame.querySelector('[data-widget-copy="why"]')),
      // The action SENTENCE: its paragraph, which carries the "What you can do" label too.
      action: size(frame.querySelector('[data-widget-copy="action"]')?.closest("p")),
    }));
    return { rowGap: parseFloat(getComputedStyle(el).rowGap), gaps, textAligns, bodySizes };
  });
}

/**
 * The FR-UI-40 layout rules over one rendered `WidgetStack`:
 *  - consecutive widgets are separated by ONE gap, and it is the stack's token;
 *  - every widget part computes `text-align: start` or `left`;
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
  for (const [i, s] of m.bodySizes.entries()) {
    expect(s.why, `widget ${i + 1}: why and what share one size`).toBe(s.what);
    expect(s.action, `widget ${i + 1}: action and explanation share one size`).toBe(s.what);
  }
  return m;
}
