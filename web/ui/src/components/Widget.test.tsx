// The widget frame (S-611, FR-UI-40). Vitest asserts DOM order and class
// PLACEMENT; under `css: false` a CSS-module import is a proxy that returns the
// class name, so computed style is the Playwright spec's job (web/ui/e2e/).
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { coverage, observe, thresholds } from "../copy/fixture.copy.ts";

import { Badge } from "./Badge.tsx";
import statesStyles from "./States.module.css";
import { Term } from "./Term.tsx";
import { Widget } from "./Widget.tsx";
import styles from "./Widget.module.css";
import { WidgetStack } from "./WidgetStack.tsx";
import stackStyles from "./WidgetStack.module.css";

const parts = (root: Element) =>
  [...root.querySelectorAll("[data-widget-part]")].map((el) => el.getAttribute("data-widget-part"));

describe("Widget", () => {
  it("renders title, figure, explanation, action and evidence in that order", () => {
    const { container } = render(
      <Widget
        title="Thresholds"
        badge={<Badge tone="red">fail</Badge>}
        copy={thresholds}
        state={{ breached: 2 }}
        figure={<span>2 of 9 measures</span>}
      >
        <table aria-label="evidence" />
      </Widget>,
    );
    expect(parts(container)).toEqual(["title", "figure", "explanation", "action", "evidence"]);
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
    expect(part("action")).toHaveClass(styles.action);
    expect(part("evidence")).toHaveClass(styles.evidence);
    // Explanation and action carry the SAME body class: one body size (FR-UI-40).
    const what = container.querySelector('[data-widget-copy="what"]')!;
    const actionText = container.querySelector('[data-widget-copy="action"]')!.closest("p")!;
    expect(what).toHaveClass(styles.body);
    expect(actionText).toHaveClass(styles.body);
  });

  it("omits the figure row, the badge and the evidence when it has none", () => {
    const { container } = render(<Widget title="Observe" copy={observe} />);
    expect(parts(container)).toEqual(["title", "explanation", "action"]);
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
    expect(parts(container)).toEqual(["title", "explanation", "action"]);
    expect(container.querySelector(`.${styles.badge}`)).toBeNull();
  });

  it("states an absence in the figure row, left-aligned, with no EmptyState", () => {
    const { container } = render(
      <Widget title="Coverage" copy={coverage} state={{ ingested: false }} absence="No coverage ingested yet." />,
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

  it("labels the action line \"What you can do\"", () => {
    const { container } = render(<Widget title="T" copy={observe} />);
    expect(container.querySelector('[data-widget-part="action"]')).toHaveTextContent(/^What you can do: /);
  });

  it("renders the where chip with its target only for an act action", () => {
    const { container, rerender } = render(<Widget title="T" copy={thresholds} state={{ breached: 1 }} />);
    const where = container.querySelector('[data-widget-copy="where"]')!;
    expect(where).toHaveClass(styles.where);
    expect(where).toHaveTextContent("configuration .logos/rules.toml [metric_thresholds]");
    expect(where.querySelector("code")).toHaveClass(styles.target);
    expect(container.querySelector('[data-widget-part="action"]')).toHaveAttribute("data-action-kind", "act");

    rerender(<Widget title="T" copy={thresholds} state={{ breached: 0 }} />);
    expect(container.querySelector('[data-widget-copy="where"]')).toBeNull();
    expect(container.querySelector('[data-widget-copy="action"]')).toHaveTextContent(
      "Nothing to do — informational.",
    );
  });

  it("renders a glossed term through Term", () => {
    const { container } = render(<Widget title="T" copy={thresholds} state={{ breached: 0 }} />);
    const dfn = container.querySelector('[data-widget-copy="what"] dfn')!;
    expect(dfn).toHaveAttribute("data-term", "arm");
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
