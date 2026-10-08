// expectWidgetCopy (S-611, CR-206, FR-UI-39): passes over widgets rendered by
// the real `Widget`, and fails on each broken shape it exists to catch — the
// action line CR-206 removed among them.
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Widget } from "../components/Widget.tsx";

import { expectWidgetCopy } from "./expectWidgetCopy.ts";
import { coverage, observe, thresholds } from "./fixture.copy.ts";

function frame(html: string): Element {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.firstElementChild!;
}

// A hand-built frame, so each test can break exactly one thing. `after` is
// markup placed after the explanation, where the action line used to render.
function widgetHtml({
  what = "Shows a figure.",
  why = "It supports a decision.",
  after = "",
}: Partial<Record<"what" | "why" | "after", string>>): string {
  return `<div data-widget="">
    <div data-widget-part="title"><h3>Fixture</h3></div>
    <div data-widget-part="explanation">
      <p data-widget-copy="what">${what}</p><p data-widget-copy="why">${why}</p>
    </div>
    ${after}
  </div>`;
}

describe("expectWidgetCopy", () => {
  it("passes on a widget and names its catalogue entry", () => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    expectWidgetCopy(container.firstElementChild!, observe);
  });

  it("passes on a widget whose copy glosses a term", () => {
    const { container } = render(<Widget title="Thresholds" copy={thresholds} figure="2 of 9 measures" />);
    expectWidgetCopy(container.firstElementChild!, thresholds);
  });

  it("passes on an absent widget", () => {
    const { container } = render(
      <Widget title="Coverage" copy={coverage} absence="No coverage ingested yet; run logos coverage ingest." />,
    );
    expectWidgetCopy(container.firstElementChild!, coverage);
  });

  it("passes on a row's own action column inside the evidence (row content, CR-206 §3.3)", () => {
    const html = widgetHtml({
      after: `<div data-widget-part="evidence"><table><tr><th>What you can do</th></tr><tr><td>Run logos index in this member.</td></tr></table></div>`,
    });
    expect(() => expectWidgetCopy(frame(html))).not.toThrow();
  });

  it.each([
    ["an empty what", widgetHtml({ what: "" }), /what part is empty/],
    ["an empty why", widgetHtml({ why: " " }), /why part is empty/],
    ["an unglossed arm", widgetHtml({ what: "Coverage by arm." }), /outside a Term gloss/],
    ["an element that is not a Widget", "<div><p>plain</p></div>", /not a Widget/],
    // The action line CR-206 removed: each of its pieces, alone, fails.
    [
      "an action part",
      widgetHtml({ after: `<div data-widget-part="action" data-action-kind="none"><p>Relax.</p></div>` }),
      /action line was removed .*data-widget-part="action"/,
    ],
    [
      "action copy",
      widgetHtml({ after: `<p><span data-widget-copy="action">Fix it.</span></p>` }),
      /action line was removed .*data-widget-copy="action"/,
    ],
    [
      "a where chip",
      widgetHtml({ after: `<p data-widget-copy="where">command <code>logos scan</code></p>` }),
      /action line was removed .*data-widget-copy="where"/,
    ],
    [
      "the \"What you can do\" label",
      widgetHtml({ after: `<p>What you can do: Nothing to do — informational.</p>` }),
      /action line was removed .*"What you can do"/,
    ],
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

  it.each([
    ["why", { why: "A different reason." }, /why/],
    ["what", { what: "Another figure." }, /what/],
  ])("fails when the rendered %s is not the named entry's", (_name, override, message) => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    expect(() => expectWidgetCopy(container.firstElementChild!, { ...observe, ...override })).toThrow(message);
  });
});
