// The widget frame (S-611, CR-206, FR-UI-40). Vitest asserts DOM order and class
// PLACEMENT; under `css: false` a CSS-module import is a proxy that returns the
// class name, so computed style is the Playwright spec's job (web/ui/e2e/).
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { expectToolPanel } from "../copy/expectWidgetCopy.ts";
import { coverage, observe, thresholds } from "../copy/fixture.copy.ts";
import { TOOL_PANELS } from "../copy/toolPanels.ts";
import { NOTHING_TO_DO } from "../copy/types.ts";

import { Badge } from "./Badge.tsx";
import statesStyles from "./States.module.css";
import { Term } from "./Term.tsx";
import { ActionCell, FigureNote, Widget } from "./Widget.tsx";
import styles from "./Widget.module.css";
import { WidgetStack } from "./WidgetStack.tsx";
import stackStyles from "./WidgetStack.module.css";

const parts = (root: Element) =>
  [...root.querySelectorAll("[data-widget-part]")].map((el) => el.getAttribute("data-widget-part"));

describe("Widget", () => {
  it("renders title, figure, explanation and evidence in that order — no action line (CR-206)", () => {
    const { container } = render(
      <Widget
        title="Thresholds"
        badge={<Badge tone="red">fail</Badge>}
        copy={thresholds}
        figure={<span>2 of 9 measures</span>}
      >
        <table aria-label="evidence" />
      </Widget>,
    );
    expect(parts(container)).toEqual(["title", "figure", "explanation", "evidence"]);
    expect(container.querySelector('[data-widget-copy="action"], [data-widget-copy="where"]')).toBeNull();
    expect(container).not.toHaveTextContent("What you can do");
    expect(screen.getByRole("table", { name: "evidence" }).closest("[data-widget-part]")).toHaveAttribute(
      "data-widget-part",
      "evidence",
    );
  });

  it("places each part's class, and composes the Card", () => {
    const { container } = render(
      <Widget title="Observe" badge={<Badge tone="muted">info</Badge>} copy={observe} figure="12 of 40">
        <ul />
      </Widget>,
    );
    const part = (name: string) => container.querySelector(`[data-widget-part="${name}"]`)!;
    expect(container.querySelector("section")).toHaveClass(styles.widget);
    expect(container.querySelector("[data-widget]")).toHaveClass(styles.frame);
    expect(part("title")).toHaveClass(styles.titleRow);
    expect(part("title").querySelector("h3")).toHaveClass(styles.title);
    expect(part("title").lastElementChild).toHaveClass(styles.badge);
    expect(part("figure")).toHaveClass(styles.figureRow);
    expect(part("explanation")).toHaveClass(styles.explanation);
    expect(part("evidence")).toHaveClass(styles.evidence);
    // What and why carry the body class: the explanation is set at the body size (FR-UI-40).
    expect(container.querySelector('[data-widget-copy="what"]')).toHaveClass(styles.body);
    expect(container.querySelector('[data-widget-copy="why"]')).toHaveClass(styles.body);
  });

  it("omits the figure row, the badge and the evidence when it has none", () => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    expect(parts(container)).toEqual(["title", "explanation"]);
    expect(container.querySelector(`.${styles.badge}`)).toBeNull();
  });

  it.each([
    ["false", false],
    ["null", null],
    ["an empty string", ""],
    ["an empty list", []],
  ])("renders no empty part for a figure, badge or evidence of %s", (_name, nothing) => {
    const { container } = render(
      <Widget title="Observe" copy={observe} figure={nothing} badge={nothing}>
        {nothing}
      </Widget>,
    );
    expect(parts(container)).toEqual(["title", "explanation"]);
    expect(container.querySelector(`.${styles.badge}`)).toBeNull();
  });

  it("states an absence in the figure row, left-aligned, with no EmptyState", () => {
    const { container } = render(
      <Widget title="Coverage" copy={coverage} absence="No coverage ingested yet." />,
    );
    const figure = container.querySelector('[data-widget-part="figure"]')!;
    const statement = screen.getByText("No coverage ingested yet.");
    expect(figure).toContainElement(statement);
    expect(statement).toHaveClass(styles.absence);
    expect(statement).toHaveAttribute("data-widget-absence");
    // EmptyState's centred wrapper and its text never appear inside a widget.
    expect(container.querySelector(`.${statesStyles.empty}`)).toBeNull();
    expect(container.querySelector(`.${statesStyles.emptyText}`)).toBeNull();
  });

  it("renders a note in the explanation, after why, outside the catalogue copy (S-616)", () => {
    // A read-model's own caveats, rendered verbatim: payload text a catalogue
    // cannot hold, so it is not a `data-widget-copy` part.
    const { container } = render(
      <Widget title="T" copy={observe} note={<p>Raw events only.</p>} />,
    );
    const explanation = container.querySelector('[data-widget-part="explanation"]')!;
    const note = screen.getByText("Raw events only.").closest("[data-widget-note]");
    expect(explanation).toContainElement(note as HTMLElement);
    expect(note).toHaveClass(styles.note);
    expect(note?.previousElementSibling).toHaveAttribute("data-widget-copy", "why");
    expect(note?.closest("[data-widget-copy]")).toBeNull();
  });

  it("renders no note element without a note", () => {
    const { container } = render(<Widget title="T" copy={observe} />);
    expect(container.querySelector("[data-widget-note]")).toBeNull();
  });

  it("renders a glossed term through Term", () => {
    const { container } = render(<Widget title="T" copy={thresholds} />);
    const dfn = container.querySelector('[data-widget-copy="what"] dfn')!;
    expect(dfn).toHaveAttribute("data-term", "arm");
  });
});

describe("Widget in panel mode (S-617, a tool panel)", () => {
  it("renders the title, the register's one line, then the tool — no figure or why", () => {
    const { container } = render(
      <Widget panel="graphQuery" title="Query the whole graph" badge={<Badge tone="muted">idle</Badge>}>
        <form aria-label="query" />
      </Widget>,
    );
    expect(parts(container)).toEqual(["title", "explanation", "evidence"]);
    const frame = container.querySelector("[data-widget]")!;
    expect(frame).toHaveAttribute("data-widget-panel", "graphQuery");
    expect(frame.querySelector('[data-widget-copy="what"]')).toHaveTextContent(TOOL_PANELS.graphQuery.what as string);
    expect(frame.querySelector('[data-widget-copy="why"]')).toBeNull();
    expect(frame.querySelector('[data-widget-part="action"]')).toBeNull();
    expect(screen.getByRole("form", { name: "query" }).closest("[data-widget-part]")).toHaveAttribute(
      "data-widget-part",
      "evidence",
    );
    expectToolPanel(container.querySelector("section")!, "graphQuery");
  });

  it("places the same classes as a figure widget: one frame, one body size", () => {
    const { container } = render(<Widget panel="chatKey" title="Chat API key" />);
    expect(container.querySelector("section")).toHaveClass(styles.widget);
    expect(container.querySelector("[data-widget]")).toHaveClass(styles.frame);
    expect(container.querySelector('[data-widget-part="explanation"]')).toHaveClass(styles.explanation);
    expect(container.querySelector('[data-widget-copy="what"]')).toHaveClass(styles.body);
    // No tool inside: no empty evidence part to take a gap.
    expect(container.querySelector('[data-widget-part="evidence"]')).toBeNull();
  });

  it("expectToolPanel rejects a figure widget, and a panel rendered under another key", () => {
    const figure = render(<Widget title="Observe" copy={observe} />);
    expect(() => expectToolPanel(figure.container.querySelector("section")!, "graphQuery")).toThrow();
    const other = render(<Widget panel="graphTable" title="Graph nodes" />);
    expect(() => expectToolPanel(other.container.querySelector("section")!, "graphQuery")).toThrow(
      /the panel it renders/,
    );
  });
});

describe("FigureNote (the one figure-row qualifier)", () => {
  it("sits in the figure row, inline by default and as its own line with block", () => {
    const { container } = render(
      <Widget
        title="T"
        copy={observe}
        figure={
          <span>
            12 <FigureNote>files ranked</FigureNote>
            <FigureNote block>Measured before the last commit.</FigureNote>
          </span>
        }
      />,
    );
    const figure = container.querySelector('[data-widget-part="figure"]')!;
    const notes = [...figure.querySelectorAll("[data-figure-note]")];
    expect(notes.map((n) => [n.tagName, n.textContent])).toEqual([
      ["SPAN", "files ranked"],
      ["P", "Measured before the last commit."],
    ]);
    for (const note of notes) expect(note).toHaveClass(styles.figureNote);
  });
});

describe("ActionCell (a row's own action)", () => {
  it("renders an act action's text, then where and its target", () => {
    const { container } = render(
      <ActionCell action={{ kind: "act", where: "configuration", target: "billing.url", text: "Define it." }} />,
    );
    expect(container.textContent).toBe("Define it.configuration billing.url");
    expect(container.querySelector("code")?.textContent).toBe("billing.url");
  });

  it("renders a none action as the one Nothing-to-do sentence, or the catalogue's own", () => {
    const { container, rerender } = render(<ActionCell action={{ kind: "none" }} />);
    expect(container.textContent).toBe(NOTHING_TO_DO);
    rerender(<ActionCell action={{ kind: "none" }} none="Nothing to fix in the repository." />);
    expect(container.textContent).toBe("Nothing to fix in the repository.");
    expect(container.querySelector("code")).toBeNull();
  });
});

describe("WidgetStack", () => {
  it("stacks its widgets in one container carrying the stack class", () => {
    const { container } = render(
      <WidgetStack>
        <Widget title="A" copy={observe} />
        <Widget title="B" copy={observe} />
      </WidgetStack>,
    );
    const stack = container.querySelector("[data-widget-stack]")!;
    expect(stack).toHaveClass(stackStyles.stack);
    expect([...stack.children].every((child) => child.querySelector("[data-widget]"))).toBe(true);
  });
});

describe("Term", () => {
  it("is a focusable dfn described by the glossary definition", () => {
    render(<Term term="scc">SCCs</Term>);
    const dfn = screen.getByText("SCCs", { selector: "dfn" });
    expect(dfn).toHaveAttribute("tabindex", "0");
    expect(dfn).toHaveAccessibleDescription(/Strongly connected component/);
    expect(dfn.querySelector('[role="tooltip"]')).toHaveTextContent(/dependency cycle/);
  });

  it("defaults its text to the glossary label", () => {
    render(<Term term="fanOut" />);
    expect(screen.getByText("fan-out", { selector: "dfn" })).toBeInTheDocument();
  });
});
