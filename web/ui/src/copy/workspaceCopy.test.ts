// The workspace catalogues' sentences (S-613, FR-UI-39). The catalogue test holds
// every `CopyEntry` to the vocabulary rule; this holds the figure-row and absence
// sentences beside them, which render in the parts `expectWidgetCopy` reads but
// are not entries themselves.
import { describe, expect, it } from "vitest";

import { COVERAGE_TEXT } from "./coverage.copy.ts";
import { findUnglossedUses, plainPart } from "./text.ts";
import type { CopyText } from "./types.ts";
import { DASHBOARD_TEXT } from "./workspaceDashboard.copy.ts";
import { BINDING_KIND_LABEL, SERVICE_MAP_TEXT } from "./serviceMap.copy.ts";
import { HEALTH_TEXT } from "./workspaceHealth.copy.ts";
import { WORKSPACE_COVERAGE_TEXT } from "./workspaceCoverage.copy.ts";

/** Reference-resolution figures for the S-637 sentences: one with a ratio, one
 *  with no code reference, one all provably external, one with one of each. */
const figure = (code_references: number, provably_external: number, resolved: number) => ({
  resolved,
  internal_candidates: code_references - provably_external,
  provably_external,
  code_references,
  lower_bound: code_references > provably_external ? resolved / (code_references - provably_external) : null,
  absence: null,
});
const FIGURES = [[figure(1_000, 200, 600)], [figure(0, 0, 0)], [figure(4, 4, 0)], [figure(2, 1, 1)]];

/** The arguments each sentence function is sampled with: every branch its body
 *  takes — zero, one and many; both booleans; a one- and a two-element list. A
 *  function with no entry here fails the guard test below, so a new sentence
 *  cannot go unchecked. */
const SAMPLES: Record<string, Record<string, unknown[][]>> = {
  COVERAGE_TEXT: {
    outboundResolved: [[0, 1], [1, 1], [2, 9]],
    outboundAllOutside: [[1], [899]],
    nothingFound: [[true, 3, 3], [false, 2, 3]],
    shortfall: [[1, 1], [2, 3]],
    degradedPrefix: [[1], [2]],
    specMatched: [[1, 1], [3, 10]],
    specBreakdown: [[0, 0, 0, 0], [1, 1, 1, 1], [3, 1, 6, 2]],
    specNotMeasured: [[1], [899]],
    capturedUnresolved: [[1], [7]],
    capturedOutside: [[1], [2]],
    capturedResolves: [[0, 3], [1, 1], [2, 3]],
    declaredPairs: [[0, 0], [1, 1], [3, 2]],
    externalsMatched: [[1, 1], [2, 5]],
    notResolvedCaption: [[1], [7]],
  },
  DASHBOARD_TEXT: {
    keepThem: [[0, false], [0, true], [1, false], [1, true], [7, false], [7, true]],
    keepThemBasis: [[1, 3, 3, false], [9, 2, 3, true]],
    keptByLinks: [[1, 1], [2, 1], [3, 2]],
    skipped: [[["web"]], [["api", "web"]]],
    deadCount: [[1], [5]],
    membersRead: [[3, 3], [2, 3]],
    securityFresh: [[0, 1], [1, 1], [1, 3]],
    securityUnread: [[1], [2]],
    securityScan: [["fresh"], ["stale"], ["never_run"]],
    resolutionAbsent: FIGURES,
    ownResolution: [["75.0%", figure(1_000, 200, 600)], ["100.0%", figure(2, 1, 1)]],
    workspaceResolutionAbsent: FIGURES,
    workspacePopulation: FIGURES,
    workspaceCrossMember: [[0, 0, 0], [1, 1, 1], [12, 3, 4]],
    workspacePooled: [[1, 1], [2, 3]],
    workspaceUnread: [[1], [2]],
  },
  HEALTH_TEXT: {
    rulesChecked: [[1, 1, 1], [2, 9, 0]],
    unknownMembers: [[1], [2]],
    unmatchedRules: [[1], [2]],
    incomplete: [[1], [2]],
    answered: [[1, 1], [2, 3]],
    warm: [[1, 1], [2, 3]],
    degraded: [[0, 1], [1, 3]],
    topics: [[1, 1], [4, 2]],
  },
  SERVICE_MAP_TEXT: {
    bindingsShown: [[0, 1], [1, 1], [3, 12]],
    noBindings: [[0], [1], [3]],
    evidenceShown: [[0, 1], [1, 1], [2, 5]],
    declaredFigure: [[0, 0, 0], [1, 1, 1], [3, 4, 2], [1, 1, null]],
    hintFigure: [[1], [2]],
    assertedFigure: [[1, 1], [2, 3]],
  },
  WORKSPACE_COVERAGE_TEXT: {
    heading: [[1], [2]],
    denominator: [[0, 1, 1], [1, 1, 0], [2, 3, 1], [1, 4, 3]],
    unread: [[1], [2]],
    freshFiles: [[0, 0], [1, 1], [2, 3]],
    tools: [[0, 0, 1], [1, 0, 1], [2, 1, 3]],
  },
};

const TABLES: Record<string, Record<string, unknown>> = {
  COVERAGE_TEXT,
  DASHBOARD_TEXT,
  HEALTH_TEXT,
  SERVICE_MAP_TEXT,
  WORKSPACE_COVERAGE_TEXT,
};

/** Every sentence a table can produce: fixed ones as they are (a fixed sentence
 *  may be catalogue text with glosses in it), functions over their samples. */
function sentences(name: string): [string, CopyText][] {
  return Object.entries(TABLES[name]).flatMap(([key, value]) => {
    if (typeof value !== "function") return [[`${name}.${key}`, value as CopyText] as [string, CopyText]];
    const fn = value as (...args: unknown[]) => string;
    return (SAMPLES[name][key] ?? []).map(
      (args) => [`${name}.${key}(${JSON.stringify(args).slice(1, -1)})`, fn(...args)] as [string, CopyText],
    );
  });
}

describe("workspace catalogue sentences", () => {
  // Every table, not a list of them: a table added to TABLES is checked here too.
  const all = [
    ...Object.keys(TABLES).flatMap(sentences),
    ...Object.entries(BINDING_KIND_LABEL).map(([k, label]) => [`kind ${k}`, label] as [string, CopyText]),
  ];

  it("samples every sentence function, so none goes unchecked", () => {
    const unsampled = Object.entries(TABLES).flatMap(([name, table]) =>
      Object.entries(table)
        .filter(([key, value]) => typeof value === "function" && !SAMPLES[name][key]?.length)
        .map(([key]) => `${name}.${key}`),
    );
    expect(unsampled).toEqual([]);
  });

  it("has sentences to check (a finding, not a floor)", () => {
    expect(all.length).toBeGreaterThan(20);
    console.info(`workspace catalogue sentences: ${all.length}`);
  });

  it.each(all)("%s uses no internal term outside a gloss", (_key, text) => {
    expect(findUnglossedUses([plainPart(text)])).toEqual([]);
  });

  it("states the service map's shares as n of m, numerator first (S-614)", () => {
    // FR-UI-42's own words, written out: the view tests read this sentence
    // back from the catalogue, so only a literal here pins it.
    expect(SERVICE_MAP_TEXT.bindingsShown(3, 4)).toBe("3 of 4 bindings shown");
    expect(SERVICE_MAP_TEXT.bindingsShown(0, 1)).toBe("0 of 1 binding shown");
    expect(SERVICE_MAP_TEXT.declaredFigure(4, 4, 2)).toBe(
      "4 declared contracts, from 4 documents · 2 calls matched to a named external",
    );
    // No external drawn: no count of calls matched against one.
    expect(SERVICE_MAP_TEXT.declaredFigure(1, 1, null)).toBe("1 declared contract, from 1 document");
    expect(SERVICE_MAP_TEXT.evidenceShown(0, 2)).toBe("Shown: 0 of 2 bindings not observed at a call site");
    expect(SERVICE_MAP_TEXT.evidenceShown(1, 1)).toBe("Shown: 1 of 1 binding not observed at a call site");
  });

  it("agrees verb and noun with a count of one and of many", () => {
    expect(COVERAGE_TEXT.capturedResolves(1, 3)).toBe("1 of 3 captured call sites resolves.");
    expect(COVERAGE_TEXT.capturedResolves(2, 3)).toBe("2 of 3 captured call sites resolve.");
    expect(COVERAGE_TEXT.specBreakdown(0, 0, 0, 1)).toContain("· 1 calls a service outside this workspace");
    expect(COVERAGE_TEXT.specBreakdown(0, 0, 0, 2)).toContain("· 2 call a service outside this workspace");
    expect(COVERAGE_TEXT.specNotMeasured(1)).toMatch(/^Not measured: the 1 cross-boundary reference calls /);
    expect(COVERAGE_TEXT.specNotMeasured(4)).toMatch(/^Not measured: all 4 cross-boundary references call /);
    expect(COVERAGE_TEXT.capturedUnresolved(1)).toContain("the 1 captured call that could match");
    expect(HEALTH_TEXT.unknownMembers(2)).toContain("so those rules were silently narrowed");
    expect(HEALTH_TEXT.unknownMembers(1)).toContain("so that rule was silently narrowed");
    expect(COVERAGE_TEXT.notResolvedCaption(1)).toBe("Why 1 outbound call site did not resolve, largest reason first");
    expect(COVERAGE_TEXT.notResolvedCaption(7)).toBe("Why 7 outbound call sites did not resolve, largest reason first");
    expect(WORKSPACE_COVERAGE_TEXT.heading(1)).toMatch(/^1 member · /);
    expect(WORKSPACE_COVERAGE_TEXT.heading(2)).toMatch(/^2 members · /);
    expect(WORKSPACE_COVERAGE_TEXT.denominator(1, 1, 1)).toBe("Coverage from 1 of 1 member; 1 member never run.");
    expect(WORKSPACE_COVERAGE_TEXT.denominator(1, 3, 2)).toBe("Coverage from 1 of 3 members; 2 members never run.");
    expect(WORKSPACE_COVERAGE_TEXT.unread(1)).toMatch(/^1 member could not be read/);
    expect(WORKSPACE_COVERAGE_TEXT.unread(2)).toMatch(/^2 members could not be read/);
  });
});

