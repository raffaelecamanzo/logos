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
  /**
   * Per widget: the computed font size of the explanation and of the action. A
   * tool panel (S-617, `data-widget-panel`) has a what and no why or action.
   */
  bodySizes: { panel: boolean; what: string; why: string; action: string; actionLine: string }[];
  /** The page's body text size (`body`, set at `--text-base`). */
  bodyText: string;
  /**
   * Every figure-row qualifier (`FigureNote`, `data-figure-note`): its computed
   * size and weight. One class owns them on every view, at the body size.
   */
  figureNotes: { text: string; fontSize: string; fontWeight: string }[];
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
        panel: frame.hasAttribute("data-widget-panel"),
        what: size(frame.querySelector('[data-widget-copy="what"]')),
        why: size(frame.querySelector('[data-widget-copy="why"]')),
        // The action text itself, and the line that carries it with its label.
        action: size(action),
        actionLine: size(action?.closest("p")),
      };
    });
    const figureNotes = [...el.querySelectorAll<HTMLElement>("[data-figure-note]")].map((node) => ({
      text: (node.textContent ?? "").trim().slice(0, 40),
      fontSize: getComputedStyle(node).fontSize,
      fontWeight: getComputedStyle(node).fontWeight,
    }));
    return {
      rowGap: parseFloat(getComputedStyle(el).rowGap),
      gaps,
      textAligns,
      leftOffsets,
      bodySizes,
      bodyText: getComputedStyle(document.body).fontSize,
      figureNotes,
    };
  }, LEFT_EDGE_BLOCKS);
}

export interface StackLayoutOptions {
  /**
   * The view legitimately stacks ONE child (S-617: Architecture, whose cycle list
   * is hidden), so there is no gap to measure. Off by default: a stack that
   * unexpectedly lost its siblings must still fail. With it on, the stack must
   * still hold a widget, so the check never passes over nothing.
   */
  single?: boolean;
}

/**
 * The FR-UI-40 layout rules over one rendered `WidgetStack`:
 *  - consecutive widgets are separated by ONE gap, and it is the stack's token;
 *  - every widget part computes `text-align: start` or `left`, and every block
 *    of a widget starts at its frame's left edge;
 *  - in every widget, explanation and action share one font size, and every
 *    widget's explanation — a tool panel's one line included — is set at the
 *    page's body text size;
 *  - every figure-row qualifier (`FigureNote`) is set at the body size and
 *    weight, whatever its figure's size, so one role reads the same on every view.
 */
export async function expectWidgetStackLayout(stack: Locator, opts: StackLayoutOptions = {}): Promise<StackMeasure> {
  const m = await measureStack(stack);
  if (opts.single) {
    expect(m.gaps, "a single-child stack has no gap").toEqual([]);
    expect(m.bodySizes.length, "the stack holds a widget").toBeGreaterThan(0);
  } else {
    expect(m.gaps.length, "a stack of fewer than two widgets has no gap to check").toBeGreaterThan(0);
  }
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
    expect(s.what, `widget ${i + 1}: its what renders`).not.toBe("missing");
    if (s.panel) {
      // A tool panel has a what only; its size is held to the others' below.
      expect(s.why, `widget ${i + 1}: a tool panel has no why`).toBe("missing");
      continue;
    }
    expect(s.why, `widget ${i + 1}: why and what share one size`).toBe(s.what);
    expect(s.action, `widget ${i + 1}: the action text and the explanation share one size`).toBe(s.what);
    expect(s.actionLine, `widget ${i + 1}: the action line and the explanation share one size`).toBe(s.what);
  }
  // One body size across the view (S-617): every widget's explanation, a tool
  // panel's line included, is set at the page's body text size — so a view of
  // panels alone is held to it too, not only compared among themselves.
  expect([...new Set(m.bodySizes.map((s) => s.what))], "one body size across the stack's widgets").toEqual([
    m.bodyText,
  ]);
  for (const note of m.figureNotes) {
    expect(note.fontSize, `figure note "${note.text}": body size`).toBe(m.bodyText);
    expect(note.fontWeight, `figure note "${note.text}": regular weight`).toBe("400");
  }
  return m;
}
