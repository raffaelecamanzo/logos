/*
 * expectWidgetCopy (S-611, CR-203, FR-UI-39) — the ONE shared test helper that
 * asserts a rendered widget carries the message standard. View tests call it for
 * each state they render (healthy, problem, absent, partial); no view test keeps
 * a hand-rolled copy of these checks.
 *
 * It asserts, over the rendered DOM:
 *   - what, why and action render non-empty;
 *   - where renders if and only if the action is `act`, and a `none` action
 *     reads the one fixed "Nothing to do" sentence;
 *   - no glossary term appears outside a `Term` gloss.
 * Given the catalogue entry and the state, it also asserts the rendered parts
 * ARE that entry's parts — a test names the catalogue key, never the prose, so a
 * wording change edits the catalogue alone.
 *
 * Test-only: it imports vitest's `expect`, and nothing under src/ outside a test
 * imports it, so it never reaches the bundle.
 */

import { expect } from "vitest";

import { findTermsInPlainText } from "./glossary.ts";
import { copyTextString } from "./text.ts";
import { NOTHING_TO_DO, type CopyEntry } from "./types.ts";

/** Text a reader sees: tooltips (a Term's definition) excluded. */
function readerText(el: Element): string {
  const clone = el.cloneNode(true) as Element;
  clone.querySelectorAll('[role="tooltip"]').forEach((tip) => tip.remove());
  return (clone.textContent ?? "").replace(/\s+/g, " ").trim();
}

/** Text outside every gloss: what must carry no internal vocabulary. */
function unglossedText(el: Element): string {
  const clone = el.cloneNode(true) as Element;
  clone.querySelectorAll("dfn").forEach((dfn) => dfn.remove());
  return clone.textContent ?? "";
}

function part(frame: Element, name: string): Element | null {
  return frame.querySelector(`[data-widget-copy="${name}"]`);
}

/** Resolves the `[data-widget]` frame from the widget root or the frame itself. */
function frameOf(widget: Element): Element {
  const frame = widget.matches("[data-widget]") ? widget : widget.querySelector("[data-widget]");
  if (!frame) throw new Error("expectWidgetCopy: the element is not a Widget (no [data-widget] frame)");
  return frame;
}

export function expectWidgetCopy<S>(widget: Element, entry?: CopyEntry<S>, state?: S): void {
  const frame = frameOf(widget);
  const label = readerText(frame.querySelector('[data-widget-part="title"]') ?? frame) || "widget";

  for (const name of ["what", "why", "action"] as const) {
    const el = part(frame, name);
    expect(el, `${label}: the ${name} part is missing`).not.toBeNull();
    expect(readerText(el!), `${label}: the ${name} part is empty`).not.toBe("");
    expect(
      findTermsInPlainText(unglossedText(el!)),
      `${label}: the ${name} part uses internal vocabulary outside a Term gloss`,
    ).toEqual([]);
  }

  const actionLine = frame.querySelector('[data-widget-part="action"]');
  const kind = actionLine?.getAttribute("data-action-kind");
  expect(["act", "none"], `${label}: unknown action kind ${String(kind)}`).toContain(kind);
  const where = part(frame, "where");
  if (kind === "act") {
    expect(where, `${label}: an "act" action must say where`).not.toBeNull();
    expect(readerText(where!), `${label}: the where chip is empty`).not.toBe("");
  } else {
    expect(where, `${label}: a "none" action must not name a where`).toBeNull();
    expect(readerText(part(frame, "action")!)).toBe(NOTHING_TO_DO);
  }

  if (entry === undefined) return;
  const action = entry.action(state as S);
  expect(readerText(part(frame, "what")!), `${label}: what`).toBe(copyTextString(entry.what));
  expect(readerText(part(frame, "why")!), `${label}: why`).toBe(copyTextString(entry.why));
  expect(kind, `${label}: action kind for this state`).toBe(action.kind);
  if (action.kind === "act") {
    expect(readerText(part(frame, "action")!), `${label}: action`).toBe(copyTextString(action.text));
    const expectedWhere = [action.where, action.target].filter(Boolean).join(" ");
    expect(readerText(where!), `${label}: where`).toBe(expectedWhere);
  }
}
