// The FR-UI-39 glossary (S-611): the list is the requirement's vocabulary, and
// each term's detector matches the term and rejects its one-character near misses.
import { describe, expect, it } from "vitest";

import { findTermsInPlainText, GLOSSARY, GLOSSARY_TERMS, VOCABULARY_TERMS } from "./glossary.ts";

describe("glossary", () => {
  it("enforces every FR-UI-39 term, and defines every entry in plain words", () => {
    // A subset, not the exact set: a catalogue may also choose to enforce a
    // term of its own (a pattern makes it enforced), so the pin is that none of
    // the requirement's fourteen is ever left unenforced.
    expect(VOCABULARY_TERMS.map((t) => GLOSSARY[t].label)).toEqual(expect.arrayContaining([
      "arm",
      "intake",
      "egress",
      "residue",
      "tier",
      "union view",
      "promoted",
      "fan-out",
      "SCC",
      "LCOM4",
      "Gini",
      "baseline",
      "epsilon",
      "bound",
    ]));
    for (const term of GLOSSARY_TERMS) {
      expect(GLOSSARY[term].definition.length, term).toBeGreaterThan(20);
      // A definition that used its own term would explain nothing.
      expect(findTermsInPlainText(GLOSSARY[term].definition), term).not.toContain(term);
    }
  });

  it("enforces the Health catalogue's terms (S-615) through the vocabulary rule", () => {
    for (const term of ["brainMethod", "godContainer", "nearClone"] as const) {
      expect(VOCABULARY_TERMS, term).toContain(term);
    }
  });

  it("adds the column-header glosses (S-616) outside the vocabulary rule", () => {
    // Glossed where a table uses them as headers; ordinary English in prose, so
    // no detector: "the answered calls" in a catalogue sentence is not a violation.
    for (const term of ["coChange", "defect", "answered"] as const) {
      expect(GLOSSARY[term].definition.length, term).toBeGreaterThan(20);
      expect(VOCABULARY_TERMS, term).not.toContain(term);
    }
    expect(findTermsInPlainText("a co-change, a defect fix and the answered calls")).toEqual([]);
  });

  it.each([
    ["coverage by arm", "arm"],
    ["two arms", "arm"],
    ["split by intake", "intake"],
    ["egress sites", "egress"],
    ["the unresolved residue", "residue"],
    ["the non-gated tier", "tier"],
    ["the union view", "unionView"],
    ["the union-view answer", "unionView"],
    ["promoted edges", "promoted"],
    ["high fan-out", "fanOut"],
    ["high fan out", "fanOut"],
    ["fanout", "fanOut"],
    ["3 SCCs", "scc"],
    ["LCOM4 above 1", "lcom4"],
    ["the Gini of churn", "gini"],
    ["below the baseline", "baseline"],
    ["within epsilon", "epsilon"],
    ["12 bound, 3 unbound.", "bound"],
    ["bound and unbound", "bound"],
    ["the unbound", "bound"],
    ["3 unbound remain", "bound"],
    ["12 bound/3 unbound per service", "bound"],
    // Known limit, documented beside the pattern: a predicate adjective at a
    // sentence end is flagged too.
    ["the call is bound.", "bound"],
    ["3 brain methods", "brainMethod"],
    ["a brain-method", "brainMethod"],
    ["god containers", "godContainer"],
    ["Are containers god-objects?", "godContainer"],
    ["near-clones", "nearClone"],
    ["a near clone", "nearClone"],
  ])("detects %j as %s", (text, term) => {
    expect(findTermsInPlainText(text)).toContain(term);
  });

  it.each([
    "an alarm was raised",
    "an armed trigger",
    "the farm",
    "outbound call sites",
    "inbound calls",
    "calls bound to a route",
    "unbound calls are listed",
    "the boundary",
    "a tiered list",
    "the scc of an ordinary word",
    "intaken",
    "the god_methods threshold",
    "brainstorm methods",
    "a near copy",
  ])("does not flag the near miss %j", (text) => {
    expect(findTermsInPlainText(text)).toEqual([]);
  });
});
