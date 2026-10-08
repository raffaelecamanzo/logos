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
 *   - no glossary term's first use in the widget — title, figure row or copy —
 *     is outside a `Term` gloss.
 * Given the catalogue entry and the state, it also asserts the rendered parts
 * ARE that entry's parts — a test names the catalogue key, never the prose, so a
 * wording change edits the catalogue alone.
 *
 * Test-only: it imports vitest's `expect`, and nothing under src/ outside a test
 * imports it, so it never reaches the bundle.
 */

import { expect } from "vitest";

import type { GlossaryTerm } from "./glossary.ts";
import { copyTextString, findUnglossedUses, type PlainPart } from "./text.ts";
import { isToolPanelKey, TOOL_PANELS, type ToolPanelKey } from "./toolPanels.ts";
import { NOTHING_TO_DO, type CopyEntry } from "./types.ts";

/** Whitespace as a browser renders it: runs collapsed, ends trimmed. */
function collapse(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

/** Text a reader sees: tooltips (a Term's definition) excluded, whitespace
 *  collapsed. Exported so a view test reads rendered text the way this helper
 *  does, rather than keeping a twin of it. */
export function readerText(el: Element): string {
  const clone = el.cloneNode(true) as Element;
  clone.querySelectorAll('[role="tooltip"]').forEach((tip) => tip.remove());
  return collapse(clone.textContent ?? "");
}

/** Elements a browser lays out as their own block (or line): their edges are
 *  word boundaries to a reader even when no whitespace sits in the markup. */
const BLOCK_TAGS = new Set(["P", "DIV", "LI", "UL", "OL", "TABLE", "TR", "TD", "TH", "BR", "H1", "H2", "H3", "H4", "H5", "H6", "SECTION"]);

/**
 * A rendered part as the vocabulary rule reads it: the text outside every gloss,
 * and where each `<dfn>` stood — the same shape `plainPart` gives catalogue text.
 *
 * A block's edges are read as a space (S-613): two paragraphs in a figure row
 * are two runs of text, and joining them bare ("…matched" + "3 bound" →
 * "matched3 bound") would hide a term that opens the second from its pattern.
 */
function renderedPart(el: Element | null): PlainPart {
  let text = "";
  const glosses: { term: GlossaryTerm; at: number }[] = [];
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      text += node.textContent ?? "";
    } else if (node instanceof Element && node.tagName === "DFN") {
      const term = node.getAttribute("data-term");
      if (term !== null) glosses.push({ term: term as GlossaryTerm, at: text.length });
    } else {
      const block = node instanceof Element && BLOCK_TAGS.has(node.tagName);
      if (block) text += " ";
      node.childNodes.forEach(walk);
      if (block) text += " ";
    }
  };
  if (el) walk(el);
  return { text, glosses };
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
  }

  // The vocabulary rule over the whole widget, in reading order: an internal
  // term's first use — in the title, the figure row or the copy — is glossed.
  const order = ["title", "figure", "what", "why", "action"] as const;
  const parts = order.map((name) =>
    renderedPart(
      name === "title" || name === "figure"
        ? frame.querySelector(`[data-widget-part="${name}"]`)
        : part(frame, name),
    ),
  );
  const unglossed = findUnglossedUses(parts).map((u) => `${u.term} (in the ${order[u.part]})`);
  expect(unglossed, `${label}: internal vocabulary used outside a Term gloss`).toEqual([]);

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
  expect(readerText(part(frame, "what")!), `${label}: what`).toBe(collapse(copyTextString(entry.what)));
  expect(readerText(part(frame, "why")!), `${label}: why`).toBe(collapse(copyTextString(entry.why)));
  expect(kind, `${label}: action kind for this state`).toBe(action.kind);
  if (action.kind === "act") {
    expect(readerText(part(frame, "action")!), `${label}: action`).toBe(collapse(copyTextString(action.text)));
    const expectedWhere = collapse([action.where, action.target].filter(Boolean).join(" "));
    expect(readerText(where!), `${label}: where`).toBe(expectedWhere);
  }
}

/**
 * expectToolPanel (S-617) — the panel-mode twin of `expectWidgetCopy`. Asserts a
 * rendered widget is the tool panel `key`: it is registered in `TOOL_PANELS`, its
 * frame names that key, its one line IS the register's `what`, and it carries no
 * figure, why, action or where (FR-UI-39: tool panels are exempt from them). The
 * vocabulary rule still holds over its title and its line.
 */
export function expectToolPanel(widget: Element, key: ToolPanelKey): void {
  const frame = frameOf(widget);
  const label = readerText(frame.querySelector('[data-widget-part="title"]') ?? frame) || "widget";
  expect(isToolPanelKey(key), `${label}: ${key} is registered in TOOL_PANELS`).toBe(true);
  expect(frame.getAttribute("data-widget-panel"), `${label}: the panel it renders`).toBe(key);
  expect(readerText(part(frame, "what")!), `${label}: what`).toBe(collapse(copyTextString(TOOL_PANELS[key].what)));
  for (const name of ["why", "action", "where"] as const) {
    expect(part(frame, name), `${label}: a tool panel has no ${name}`).toBeNull();
  }
  expect(frame.querySelector('[data-widget-part="figure"]'), `${label}: a tool panel has no figure`).toBeNull();
  const parts = [frame.querySelector('[data-widget-part="title"]'), part(frame, "what")].map(renderedPart);
  const unglossed = findUnglossedUses(parts).map((u) => u.term);
  expect(unglossed, `${label}: internal vocabulary used outside a Term gloss`).toEqual([]);
}
