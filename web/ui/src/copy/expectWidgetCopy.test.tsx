// expectWidgetCopy (S-611, FR-UI-39): passes over both action kinds rendered by
// the real `Widget`, and fails on each broken shape it exists to catch.
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Widget } from "../components/Widget.tsx";

import { expectWidgetCopy } from "./expectWidgetCopy.ts";
import { coverage, observe, thresholds } from "./fixture.copy.ts";
import type { WidgetAction } from "./types.ts";

const actThresholds = thresholds.action({ breached: 2 }) as Extract<WidgetAction, { kind: "act" }>;

function frame(html: string): Element {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.firstElementChild!;
}

// A hand-built frame, so each test can break exactly one thing.
function widgetHtml({
  what = "Shows a figure.",
  why = "It supports a decision.",
  kind = "none",
  action = "Nothing to do — informational.",
  where = "",
}: Partial<Record<"what" | "why" | "kind" | "action" | "where", string>>): string {
  return `<div data-widget="">
    <div data-widget-part="title"><h3>Fixture</h3></div>
    <div data-widget-part="explanation">
      <p data-widget-copy="what">${what}</p><p data-widget-copy="why">${why}</p>
    </div>
    <div data-widget-part="action" data-action-kind="${kind}">
      <p>What you can do: <span data-widget-copy="action">${action}</span></p>
      ${where ? `<p data-widget-copy="where">${where}</p>` : ""}
    </div>
  </div>`;
}

describe("expectWidgetCopy", () => {
  it("passes on an observe widget (action none) and names its catalogue entry", () => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    expectWidgetCopy(container.firstElementChild!, observe);
  });

  it("passes on an act widget, in both of the states its action distinguishes", () => {
    for (const state of [{ breached: 2 }, { breached: 0 }]) {
      const { container, unmount } = render(<Widget title="Thresholds" copy={thresholds} state={state} />);
      expectWidgetCopy(container.firstElementChild!, thresholds, state);
      unmount();
    }
  });

  it("passes on an absent widget whose action is to act", () => {
    const state = { ingested: false };
    const { container } = render(
      <Widget title="Coverage" copy={coverage} state={state} absence="No coverage ingested yet." />,
    );
    expectWidgetCopy(container.firstElementChild!, coverage, state);
  });

  it.each([
    ["an empty what", widgetHtml({ what: "" }), /what part is empty/],
    ["an empty why", widgetHtml({ why: " " }), /why part is empty/],
    ["an act action with no where", widgetHtml({ kind: "act", action: "Fix it." }), /must say where/],
    [
      "a none action that names a where",
      widgetHtml({ where: "command <code>logos scan</code>" }),
      /must not name a where/,
    ],
    ["a none action with other words", widgetHtml({ action: "Relax." }), /Nothing to do/],
    ["an unglossed arm", widgetHtml({ what: "Coverage by arm." }), /outside a Term gloss/],
    ["an element that is not a Widget", "<div><p>plain</p></div>", /not a Widget/],
    ["an unknown action kind", widgetHtml({ kind: "maybe" }), /unknown action kind/],
  ])("fails on %s", (_name, html, message) => {
    expect(() => expectWidgetCopy(frame(html))).toThrow(message);
  });

  // S-613: a figure row of two blocks reads as two runs of text. Joined with no
  // separator, "…matched" + "3 bound" read "matched3 bound", where the noun's
  // `\b\d` cannot match — so a noun "bound" opening the second block went
  // unflagged.
  it("reads a block boundary as a word boundary, so a term opening a second block is caught", () => {
    const html = widgetHtml({}).replace(
      '<div data-widget-part="explanation">',
      `<div data-widget-part="figure"><div><span>1 of 3 matched</span></div><p>3 bound · 1 ambiguous</p></div>
    <div data-widget-part="explanation">`,
    );
    expect(() => expectWidgetCopy(frame(html))).toThrow(/bound \(in the figure\)/);
  });

  it("accepts a term inside a gloss", () => {
    const html = widgetHtml({ what: 'Coverage by <dfn>arm<span role="tooltip">one way</span></dfn>.' });
    expect(() => expectWidgetCopy(frame(html))).not.toThrow();
  });

  it("compares copy as rendered, so a line break or double space in the catalogue is no mismatch", () => {
    const entry = { ...observe, what: "Shows calls\n  per service. " };
    const { container } = render(<Widget title="Observe" copy={entry} />);
    expectWidgetCopy(container.firstElementChild!, entry);
  });

  it("fails when the rendered copy is not the named entry's", () => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    const other = { ...observe, why: "A different reason." };
    expect(() => expectWidgetCopy(container.firstElementChild!, other)).toThrow(/why/);
  });

  it.each([
    ["what", { what: "Another figure." }, /what/],
    ["the action text", { action: () => ({ ...actThresholds, text: "Do something else." }) }, /action/],
    ["the where kind", { action: () => ({ ...actThresholds, where: "source code" as const }) }, /where/],
    ["the where target", { action: () => ({ ...actThresholds, target: "logos scan" }) }, /where/],
  ])("fails when %s differs from the named entry", (_name, override, message) => {
    const state = { breached: 2 };
    const { container } = render(<Widget title="Thresholds" copy={thresholds} state={state} />);
    expect(() => expectWidgetCopy(container.firstElementChild!, { ...thresholds, ...override }, state)).toThrow(
      message,
    );
  });

  it("fails when the action kind is not the one the state calls for", () => {
    const { container } = render(<Widget title="Thresholds" copy={thresholds} state={{ breached: 0 }} />);
    expect(() => expectWidgetCopy(container.firstElementChild!, thresholds, { breached: 3 })).toThrow(
      /action kind/,
    );
  });
});
