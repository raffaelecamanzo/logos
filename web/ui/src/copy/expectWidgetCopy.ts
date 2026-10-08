/*
 * expectWidgetCopy (S-611, CR-203, CR-206, FR-UI-39) — the ONE shared test
 * helper that asserts a rendered widget carries the message standard. View tests
 * call it for each state they render (healthy, problem, absent, partial); no view
 * test keeps a hand-rolled copy of these checks.
 *
 * It asserts, over the rendered DOM:
 *   - what and why render non-empty;
 *   - no action text renders: no action part, no action or where copy, and
 *     nowhere in the widget — evidence tables included — the "What you can do"
 *     label or the "Nothing to do — informational." sentence (CR-206 removed the
 *     action line, CR-207 the per-row action columns);
 *   - no glossary term's first use in the widget — title, figure row or copy —
 *     is outside a `Term` gloss.
 * Given the catalogue entry, it also asserts the rendered parts ARE that
 * entry's parts — a test names the catalogue key, never the prose, so a wording
 * change edits the catalogue alone. Given the one no-explanation entry (CR-208,
 * Project Overview), it asserts the opposite: no explanation part renders.
 *
 * Test-only: it imports vitest's `expect`, and nothing under src/ outside a test
 * imports it, so it never reaches the bundle.
 */

import { expect } from "vitest";

import { REMOVED_ACTION_TEXT } from "../test/removedActionText.ts";

import type { GlossaryTerm } from "./glossary.ts";
import { copyTextString, findUnglossedUses, type PlainPart } from "./text.ts";
import { isToolPanelKey, TOOL_PANELS, type ToolPanelKey } from "./toolPanels.ts";
import type { CopyEntry, NoExplanationEntry } from "./types.ts";

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

/** Asserts the frame renders no action text: not the part, its copy or its
 *  where chip CR-206 removed, and not the label or the empty-row sentence
 *  anywhere in the frame. The evidence part is searched too: CR-207 removed the
 *  per-row action columns, so no table may carry either. */
function expectNoActionLine(frame: Element, label: string): void {
  for (const selector of ['[data-widget-part="action"]', '[data-widget-copy="action"]', '[data-widget-copy="where"]']) {
    expect(frame.querySelector(selector), `${label}: the action line was removed (CR-206), yet ${selector} renders`).toBeNull();
  }
  const text = readerText(frame);
  for (const removed of REMOVED_ACTION_TEXT) {
    expect(text, `${label}: action text was removed (CR-206, CR-207), yet "${removed}" renders`).not.toContain(removed);
  }
}

export function expectWidgetCopy(widget: Element, entry?: CopyEntry | NoExplanationEntry): void {
  const frame = frameOf(widget);
  const label = readerText(frame.querySelector('[data-widget-part="title"]') ?? frame) || "widget";

  if (entry !== undefined && "noExplanation" in entry) {
    expectNoExplanation(frame, label);
    return;
  }
  expect(frame.hasAttribute("data-widget-no-explanation"), `${label}: only the no-explanation entry's frame is marked as one`).toBe(false);

  for (const name of ["what", "why"] as const) {
    const el = part(frame, name);
    expect(el, `${label}: the ${name} part is missing`).not.toBeNull();
    expect(readerText(el!), `${label}: the ${name} part is empty`).not.toBe("");
  }

  // The vocabulary rule over the whole widget, in reading order: an internal
  // term's first use — in the title, the figure row or the copy — is glossed.
  const order = ["title", "figure", "what", "why"] as const;
  const parts = order.map((name) =>
    renderedPart(
      name === "title" || name === "figure"
        ? frame.querySelector(`[data-widget-part="${name}"]`)
        : part(frame, name),
    ),
  );
  const unglossed = findUnglossedUses(parts).map((u) => `${u.term} (in the ${order[u.part]})`);
  expect(unglossed, `${label}: internal vocabulary used outside a Term gloss`).toEqual([]);

  expectNoActionLine(frame, label);

  if (entry === undefined) return;
  expect(readerText(part(frame, "what")!), `${label}: what`).toBe(collapse(copyTextString(entry.what)));
  expect(readerText(part(frame, "why")!), `${label}: why`).toBe(collapse(copyTextString(entry.why)));
}

/** The one no-explanation entry's frame (CR-208): no explanation part and no
 *  what or why, while the vocabulary rule and the no-action rule still hold over
 *  what it does render — its title and its figure row. */
function expectNoExplanation(frame: Element, label: string): void {
  expect(frame.hasAttribute("data-widget-no-explanation"), `${label}: its frame declares the exception`).toBe(true);
  expect(frame.querySelector('[data-widget-part="explanation"]'), `${label}: renders no explanation part (CR-208)`).toBeNull();
  for (const name of ["what", "why"] as const) {
    expect(part(frame, name), `${label}: renders no ${name} (CR-208)`).toBeNull();
  }
  const parts = (["title", "figure"] as const).map((name) => renderedPart(frame.querySelector(`[data-widget-part="${name}"]`)));
  const unglossed = findUnglossedUses(parts).map((u) => u.term);
  expect(unglossed, `${label}: internal vocabulary used outside a Term gloss`).toEqual([]);
  expectNoActionLine(frame, label);
}

/**
 * expectToolPanel (S-617) — the panel-mode twin of `expectWidgetCopy`. Asserts a
 * rendered widget is the tool panel `key`: it is registered in `TOOL_PANELS`, its
 * frame names that key, its one line IS the register's `what`, and it carries no
 * figure or why (FR-UI-39: tool panels are exempt from them) and, like every
 * widget, no action line. The vocabulary rule still holds over its title and its
 * line.
 */
export function expectToolPanel(widget: Element, key: ToolPanelKey): void {
  const frame = frameOf(widget);
  const label = readerText(frame.querySelector('[data-widget-part="title"]') ?? frame) || "widget";
  expect(isToolPanelKey(key), `${label}: ${key} is registered in TOOL_PANELS`).toBe(true);
  expect(frame.getAttribute("data-widget-panel"), `${label}: the panel it renders`).toBe(key);
  expect(readerText(part(frame, "what")!), `${label}: what`).toBe(collapse(copyTextString(TOOL_PANELS[key].what)));
  expect(part(frame, "why"), `${label}: a tool panel has no why`).toBeNull();
  expectNoActionLine(frame, label);
  expect(frame.querySelector('[data-widget-part="figure"]'), `${label}: a tool panel has no figure`).toBeNull();
  const parts = [frame.querySelector('[data-widget-part="title"]'), part(frame, "what")].map(renderedPart);
  const unglossed = findUnglossedUses(parts).map((u) => u.term);
  expect(unglossed, `${label}: internal vocabulary used outside a Term gloss`).toEqual([]);
}
