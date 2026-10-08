// expectWidgetCopy (S-611, FR-UI-39): passes over both action kinds rendered by
// the real `Widget`, and fails on each broken shape it exists to catch.
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
  ])("fails on %s", (_name, html, message) => {
    expect(() => expectWidgetCopy(frame(html))).toThrow(message);
  });

  it("accepts a term inside a gloss", () => {
    const html = widgetHtml({ what: 'Coverage by <dfn>arm<span role="tooltip">one way</span></dfn>.' });
    expect(() => expectWidgetCopy(frame(html))).not.toThrow();
  });

  it("fails when the rendered copy is not the named entry's", () => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    const other = { ...observe, why: "A different reason." };
    expect(() => expectWidgetCopy(container.firstElementChild!, other)).toThrow(/why/);
  });

  it("fails when the action kind is not the one the state calls for", () => {
    const { container } = render(<Widget title="Thresholds" copy={thresholds} state={{ breached: 0 }} />);
    expect(() => expectWidgetCopy(container.firstElementChild!, thresholds, { breached: 3 })).toThrow(
      /action kind/,
    );
  });
});
