// The vocabulary rule's shared core (S-611, FR-UI-39): first use glossed, read
// the same way from catalogue text and from the rendered widget.
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Widget } from "../components/Widget.tsx";

import { expectWidgetCopy } from "./expectWidgetCopy.ts";
import { actionLiterals, copyTextString, findUnglossedTerms, findUnglossedUses, isCopyEntry, plainPart } from "./text.ts";
import { gloss, noAction, type CopyEntry } from "./types.ts";

describe("the first-use rule", () => {
  it("flags a plain term", () => {
    expect(findUnglossedTerms("Coverage by arm.")).toEqual(["arm"]);
  });

  it("accepts a glossed term", () => {
    expect(findUnglossedTerms(["Coverage by ", gloss("arm"), "."])).toEqual([]);
  });

  it("accepts a plain use after the term was glossed in an earlier part", () => {
    expect(findUnglossedTerms(["Coverage by ", gloss("arm"), "."], "An arm with no calls hides nothing.")).toEqual([]);
  });

  it("flags a plain use that comes before the gloss", () => {
    expect(findUnglossedTerms("An arm with no calls.", ["Coverage by ", gloss("arm"), "."])).toEqual(["arm"]);
    expect(findUnglossedTerms(["The arm, or ", gloss("arm"), "."])).toEqual(["arm"]);
  });

  it("names the part a violation is in", () => {
    expect(findUnglossedUses([plainPart("Fine."), plainPart("Past the baseline.")])).toEqual([
      { term: "baseline", part: 1 },
    ]);
  });
});

describe("catalogue text and rendered text read alike", () => {
  // A gloss in mid-sentence: the catalogue side and the DOM side both cut the
  // gloss out and scan the joined text, so neither can pass what the other fails.
  const what = ["3 unbound ", gloss("arm", "arms"), " were found."] as const;
  const entry: CopyEntry = { what, why: "It matters.", action: () => noAction };

  it("the catalogue side flags it", () => {
    expect(findUnglossedTerms(what)).toEqual(["bound"]);
  });

  it("the rendered side flags it too", () => {
    const { container } = render(<Widget title="T" copy={entry} />);
    expect(() => expectWidgetCopy(container.firstElementChild!)).toThrow(/bound \(in the what\)/);
  });
});

describe("action text", () => {
  it("is read from the action's string literals", () => {
    const action = (s: { n: number }) =>
      s.n > 0 ? { kind: "act" as const, where: "command" as const, text: "Split the SCC per arm." } : noAction;
    expect(findUnglossedUses([actionLiterals(action)]).map((u) => u.term)).toEqual(["arm", "scc"]);
  });

  it("does not count a term inside a gloss call", () => {
    const action = () => ({ kind: "act" as const, where: "command" as const, text: ["Check each ", gloss("arm"), "."] });
    expect(findUnglossedUses([actionLiterals(action)])).toEqual([]);
  });
});

describe("expectWidgetCopy reads the whole widget", () => {
  const entry: CopyEntry = { what: "Shows a figure.", why: "It supports a decision.", action: () => noAction };

  it("fails on a term in the title", () => {
    const { container } = render(<Widget title="Coverage by arm" copy={entry} />);
    expect(() => expectWidgetCopy(container.firstElementChild!)).toThrow(/arm \(in the title\)/);
  });

  it("fails on a term in an absence statement", () => {
    const { container } = render(<Widget title="Gate" copy={entry} absence="No baseline recorded yet." />);
    expect(() => expectWidgetCopy(container.firstElementChild!)).toThrow(/baseline \(in the figure\)/);
  });
});

describe("copyTextString", () => {
  it("reads a gloss as the word the sentence uses, else as the glossary label", () => {
    expect(copyTextString(["3 ", gloss("scc", "SCCs"), " found."])).toBe("3 SCCs found.");
    expect(copyTextString(["High ", gloss("fanOut"), "."])).toBe("High fan-out.");
    expect(copyTextString("plain")).toBe("plain");
  });

  it("is what a Widget renders for a gloss with its own wording", () => {
    const entry: CopyEntry = { what: ["3 ", gloss("scc", "SCCs"), " found."], why: "w", action: () => noAction };
    const { container } = render(<Widget title="T" copy={entry} />);
    expect(container.querySelector('dfn[data-term="scc"]')!.firstChild!.textContent).toBe("SCCs");
    expectWidgetCopy(container.firstElementChild!, entry);
  });
});

describe("isCopyEntry", () => {
  const action = () => noAction;
  it.each([
    ["string copy", { what: "a", why: "b", action }],
    ["glossed copy", { what: ["a ", gloss("arm")], why: "b", action }],
  ])("accepts %s", (_name, value) => {
    expect(isCopyEntry(value)).toBe(true);
  });

  it.each([
    ["null", null],
    ["a string", "what"],
    ["an entry with no action", { what: "a", why: "b" }],
    ["an entry with no why", { what: "a", action }],
    ["an entry whose what is a number", { what: 1, why: "b", action }],
    ["an entry whose action is not a function", { what: "a", why: "b", action: "none" }],
  ])("rejects %s", (_name, value) => {
    expect(isCopyEntry(value)).toBe(false);
  });
});
