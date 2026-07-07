import { afterEach, describe, expect, it } from "vitest";

import { clearHighlight, HIGHLIGHT_MARK_SELECTOR, highlightFirstMatch } from "./searchHighlight.ts";

function mount(html: string): HTMLDivElement {
  const el = document.createElement("div");
  el.innerHTML = html;
  document.body.appendChild(el);
  return el;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("highlightFirstMatch (FR-WK-28)", () => {
  it("wraps the first occurrence of the term in a <mark>, preserving surrounding text", () => {
    const container = mount("<p>Intro prose about the sandbox escape refusal.</p>");
    const mark = highlightFirstMatch(container, "sandbox escape");
    expect(mark).not.toBeNull();
    expect(mark?.tagName).toBe("MARK");
    expect(mark?.textContent).toBe("sandbox escape");
    expect(container.textContent).toBe("Intro prose about the sandbox escape refusal.");
    expect(container.querySelectorAll(HIGHLIGHT_MARK_SELECTOR).length).toBe(1);
  });

  it("matches case-insensitively", () => {
    const container = mount("<p>The Sandbox Escape is refused.</p>");
    const mark = highlightFirstMatch(container, "sandbox escape");
    expect(mark?.textContent).toBe("Sandbox Escape");
  });

  it("marks only the FIRST occurrence when the term repeats", () => {
    const container = mount("<p>escape escape escape</p>");
    highlightFirstMatch(container, "escape");
    expect(container.querySelectorAll(HIGHLIGHT_MARK_SELECTOR).length).toBe(1);
    expect(container.textContent).toBe("escape escape escape");
  });

  it("finds a match spanning a later text node, not just the first", () => {
    const container = mount("<p>Nothing here.</p><p>The needle is here.</p>");
    const mark = highlightFirstMatch(container, "needle");
    expect(mark?.textContent).toBe("needle");
  });

  it("returns null and mutates nothing when the term is absent from the body", () => {
    const container = mount("<p>No match in this body.</p>");
    const before = container.innerHTML;
    const mark = highlightFirstMatch(container, "absent-term");
    expect(mark).toBeNull();
    expect(container.innerHTML).toBe(before);
  });

  it("returns null for an empty or whitespace-only term", () => {
    const container = mount("<p>Some body text.</p>");
    expect(highlightFirstMatch(container, "")).toBeNull();
    expect(highlightFirstMatch(container, "   ")).toBeNull();
  });

  it("is idempotent — a second call is a no-op once a mark is already present", () => {
    const container = mount("<p>escape escape</p>");
    highlightFirstMatch(container, "escape");
    const after = container.innerHTML;
    const second = highlightFirstMatch(container, "escape");
    expect(second).toBeNull();
    expect(container.innerHTML).toBe(after);
  });

  it("does not find a match split across two adjacent text nodes (documented limitation)", () => {
    const container = mount("<p>sand<b>box escape</b> refusal.</p>");
    expect(highlightFirstMatch(container, "sandbox escape")).toBeNull();
  });
});

describe("clearHighlight (FR-WK-28)", () => {
  it("removes an existing mark, restoring the plain text", () => {
    const container = mount("<p>Intro prose about the sandbox escape refusal.</p>");
    highlightFirstMatch(container, "sandbox escape");
    expect(container.querySelector(HIGHLIGHT_MARK_SELECTOR)).not.toBeNull();

    clearHighlight(container);

    expect(container.querySelector(HIGHLIGHT_MARK_SELECTOR)).toBeNull();
    expect(container.textContent).toBe("Intro prose about the sandbox escape refusal.");
  });

  it("is a no-op when no mark is present", () => {
    const container = mount("<p>No highlight here.</p>");
    const before = container.innerHTML;
    clearHighlight(container);
    expect(container.innerHTML).toBe(before);
  });
});
