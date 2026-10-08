/*
 * The FR-UI-40 parentage rule, as a test helper (S-613): within a view, every
 * widget's parent is ONE `WidgetStack`, so every pair of consecutive widgets is
 * spaced by the stack's one gap token.
 *
 * vitest can only assert the DOM (its CSS modules are a proxy under
 * `css: false`), so this checks parentage and leaves the gap's computed size to
 * the Playwright spec (`e2e/layout.ts`, `expectWidgetStackLayout`).
 */

import { expect } from "vitest";

/** The rendered widgets under `root`: each `Widget`'s root (its `Card`
 *  section), in document order. */
export function widgetsIn(root: Element): HTMLElement[] {
  return [...root.querySelectorAll("[data-widget]")].map((frame) => {
    const card = frame.parentElement;
    if (!card) throw new Error("a [data-widget] frame has no Card around it");
    return card;
  });
}

/** Asserts `root` holds exactly one `WidgetStack` and that it is the parent of
 *  every widget under `root`. Returns the widgets, for further checks. */
export function expectOneWidgetStack(root: Element): HTMLElement[] {
  const stacks = root.querySelectorAll("[data-widget-stack]");
  expect(stacks, "one WidgetStack per view").toHaveLength(1);
  const widgets = widgetsIn(root);
  expect(widgets.length, "the view renders widgets").toBeGreaterThan(0);
  for (const widget of widgets) {
    const title = widget.querySelector('[data-widget-part="title"]')?.textContent ?? "widget";
    expect(widget.parentElement, `${title}: its parent is the view's WidgetStack`).toBe(stacks[0]);
  }
  return widgets;
}

/** A widget's action kind (`act` or `none`), as the frame marks it. */
export function actionKind(widget: Element): string | null {
  return widget.querySelector('[data-widget-part="action"]')?.getAttribute("data-action-kind") ?? null;
}

/** A widget's title as a reader sees it (a gloss's tooltip left out). */
export function widgetTitle(widget: Element): string {
  const title = widget.querySelector('[data-widget-part="title"] h3');
  if (!title) return "";
  const clone = title.cloneNode(true) as Element;
  clone.querySelectorAll('[role="tooltip"]').forEach((tip) => tip.remove());
  return (clone.textContent ?? "").replace(/\s+/g, " ").trim();
}

/** The one widget under `root` whose title, as a reader sees it, is `title`
 *  (asserted unique). Every view test finds its widgets through this one lookup
 *  rather than a copy of it. */
export function widgetTitled(root: Element, title: string): HTMLElement {
  const found = widgetsIn(root).filter((w) => widgetTitle(w) === title);
  expect(found, `exactly one "${title}" widget`).toHaveLength(1);
  return found[0];
}
