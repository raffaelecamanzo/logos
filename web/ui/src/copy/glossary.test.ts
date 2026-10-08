// The FR-UI-39 glossary (S-611): the list is the requirement's vocabulary, and
// each term's detector matches the term and rejects its one-character near misses.
import { describe, expect, it } from "vitest";

import { findTermsInPlainText, FR_UI_39_TERMS, GLOSSARY, GLOSSARY_TERMS } from "./glossary.ts";

describe("glossary", () => {
  it("lists the FR-UI-39 vocabulary first, then the later stories' terms, each with a plain-words definition", () => {
    expect(GLOSSARY_TERMS.slice(0, FR_UI_39_TERMS.length)).toEqual([...FR_UI_39_TERMS]);
    expect(FR_UI_39_TERMS.map((t) => GLOSSARY[t].label)).toEqual([
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
    // S-613: the Members table's glossed headers (CR-203 §3.2 D item 3).
    expect(GLOSSARY_TERMS.slice(FR_UI_39_TERMS.length).map((t) => GLOSSARY[t].label)).toEqual([
      "Reference resolution",
      "Entry points added by other services",
      "Unused in its own graph",
      "used by another service",
      "Unused across the workspace",
    ]);
    for (const term of GLOSSARY_TERMS) {
      expect(GLOSSARY[term].definition.length, term).toBeGreaterThan(20);
      // A definition that used its own term would explain nothing.
      expect(findTermsInPlainText(GLOSSARY[term].definition), term).not.toContain(term);
    }
  });

  it.each([
    ["coverage by arm", "arm"],
    ["two arms", "arm"],
    ["split by intake", "intake"],
    ["egress sites", "egress"],
    ["outside egress_resolution", "egress"],
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
    ["its own reference resolution", "referenceResolution"],
    ["entry points added by other services", "entryPointsFromOtherServices"],
    ["one entry point added by other services", "entryPointsFromOtherServices"],
    ["unused in its own graph", "unusedInOwnGraph"],
    ["…of which used by another service", "usedByAnotherService"],
    ["callables unused across the workspace", "unusedAcrossWorkspace"],
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
    "egressive",
    "a reference that resolves",
    "entry points added by this service",
    "unused in its own service",
    "unused by another service",
    "unused across services",
  ])("does not flag the near miss %j", (text) => {
    expect(findTermsInPlainText(text)).toEqual([]);
  });
});
