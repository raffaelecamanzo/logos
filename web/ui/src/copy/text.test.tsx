// The vocabulary rule's shared core (S-611, FR-UI-39): first use glossed, read
// the same way from catalogue text and from the rendered widget.
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Widget } from "../components/Widget.tsx";

import { expectWidgetCopy } from "./expectWidgetCopy.ts";
import {
  copyTextString,
  findUnglossedTerms,
  findUnglossedUses,
  isCopyEntry,
  isNoExplanationEntry,
  plainPart,
  sentenceLiterals,
} from "./text.ts";
import { gloss, type CopyEntry } from "./types.ts";

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
  const entry: CopyEntry = { what, why: "It matters." };

  it("the catalogue side flags it", () => {
    expect(findUnglossedTerms(what)).toEqual(["bound"]);
  });

  it("the rendered side flags it too", () => {
    const { container } = render(<Widget title="T" copy={entry} />);
    expect(() => expectWidgetCopy(container.firstElementChild!)).toThrow(/bound \(in the what\)/);
  });
});

describe("sentence function text", () => {
  it("is read from the function's string literals", () => {
    const sentence = (s: { n: number }) => (s.n > 0 ? "Split the SCC per arm." : "Nothing found.");
    expect(findUnglossedUses([sentenceLiterals(sentence)]).map((u) => u.term)).toEqual(["arm", "scc"]);
  });

  it("reads a template's literal text, never the code inside ${…} (sprint review)", () => {
    const sentence = (f: { baseline: number; n: number }) =>
      `Compare with ${f.baseline}, then split the ${f.n === 1 ? "arm" : "SCC"}.`;
    // `f.baseline` is an expression; "arm" and "SCC" are words the function can say.
    expect(findUnglossedUses([sentenceLiterals(sentence)]).map((u) => u.term)).toEqual(["arm", "scc"]);
  });

  it("does not count a term inside a gloss call", () => {
    const sentence = () => ["Check each ", gloss("arm"), "."];
    expect(findUnglossedUses([sentenceLiterals(sentence)])).toEqual([]);
  });
});

describe("expectWidgetCopy reads the whole widget", () => {
  const entry: CopyEntry = { what: "Shows a figure.", why: "It supports a decision." };

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
    const entry: CopyEntry = { what: ["3 ", gloss("scc", "SCCs"), " found."], why: "w" };
    const { container } = render(<Widget title="T" copy={entry} />);
    expect(container.querySelector('dfn[data-term="scc"]')!.firstChild!.textContent).toBe("SCCs");
    expectWidgetCopy(container.firstElementChild!, entry);
  });
});

describe("isCopyEntry", () => {
  it.each([
    ["string copy", { what: "a", why: "b" }],
    ["glossed copy", { what: ["a ", gloss("arm")], why: "b" }],
  ])("accepts %s", (_name, value) => {
    expect(isCopyEntry(value)).toBe(true);
  });

  it.each([
    ["null", null],
    ["a string", "what"],
    ["an entry with no why", { what: "a" }],
    ["an entry with no what", { why: "b" }],
    ["an entry whose what is a number", { what: 1, why: "b" }],
    ["a disclosure, whose text is not what and why", { summary: "a", body: "b" }],
  ])("rejects %s", (_name, value) => {
    expect(isCopyEntry(value)).toBe(false);
  });
});

// CR-208: the one no-explanation entry is told apart by its marker AND by carrying
// neither part — so an entry that claims the marker beside a what is not one.
describe("isNoExplanationEntry", () => {
  it("accepts the exception", () => {
    expect(isNoExplanationEntry({ noExplanation: "project-overview" })).toBe(true);
  });

  it.each([
    ["a marker beside a what", { noExplanation: "project-overview", what: "W." }],
    ["a marker beside a why", { noExplanation: "project-overview", why: "Y." }],
    ["a marker that is not a string", { noExplanation: true }],
    ["an ordinary entry", { what: "W.", why: "Y." }],
    ["null", null],
    ["a string", "project-overview"],
  ])("refuses %s", (_name, value) => {
    expect(isNoExplanationEntry(value)).toBe(false);
  });
});
