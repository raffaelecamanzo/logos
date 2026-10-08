// The FR-UI-39 glossary (S-611): the list is the requirement's vocabulary, and
// each term's detector matches the term and rejects its one-character near misses.
import { describe, expect, it } from "vitest";

import { findTermsInPlainText, GLOSSARY, GLOSSARY_TERMS, VOCABULARY_TERMS } from "./glossary.ts";

describe("glossary", () => {
  it("lists exactly the FR-UI-39 vocabulary, each with a plain-words definition", () => {
    expect(VOCABULARY_TERMS.map((t) => GLOSSARY[t].label)).toEqual([
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
    ]);
    for (const term of GLOSSARY_TERMS) {
      expect(GLOSSARY[term].definition.length, term).toBeGreaterThan(20);
      // A definition that used its own term would explain nothing.
      expect(findTermsInPlainText(GLOSSARY[term].definition), term).not.toContain(term);
    }
  });

  it("adds the column-header glosses (S-616) outside the vocabulary rule", () => {
    // Glossed where a table uses them as headers; ordinary English in prose, so
    // no detector: "the answered calls" in a catalogue sentence is not a violation.
    const headers = GLOSSARY_TERMS.filter((t) => !VOCABULARY_TERMS.includes(t));
    expect(headers.map((t) => GLOSSARY[t].label)).toEqual(["Co-change", "Defect", "Answered"]);
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
  ])("does not flag the near miss %j", (text) => {
    expect(findTermsInPlainText(text)).toEqual([]);
  });
});
