// The workspace catalogues' sentences (S-613, FR-UI-39). The catalogue test holds
// every `CopyEntry` to the vocabulary rule; this holds the figure-row and absence
// sentences and the remedy table beside them, which render in the parts
// `expectWidgetCopy` reads but are not entries themselves.
import { describe, expect, it } from "vitest";

import { COVERAGE_TEXT, NOT_RESOLVED_REMEDY, remedyFor, UNLISTED_REMEDY } from "./coverage.copy.ts";
import { findUnglossedUses, plainPart } from "./text.ts";
import type { CopyText } from "./types.ts";
import { DASHBOARD_TEXT, memberRowAction } from "./workspaceDashboard.copy.ts";
import {
  BINDING_KIND_LABEL,
  evidenceRowAction,
  SERVICE_MAP_TEXT,
} from "./serviceMap.copy.ts";
import { HEALTH_TEXT } from "./workspaceHealth.copy.ts";

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
  },
  DASHBOARD_TEXT: {
    keepThem: [[0, false], [0, true], [1, false], [1, true], [7, false], [7, true]],
    keepThemBasis: [[1, 3, 3, false], [9, 2, 3, true]],
    skipped: [[["web"]], [["api", "web"]]],
    deadCount: [[1], [5]],
    membersRead: [[3, 3], [2, 3]],
  },
  HEALTH_TEXT: {
    rulesChecked: [[1, 1, 1], [2, 9, 0]],
    unknownMembers: [[1], [2]],
    incomplete: [[1], [2]],
  },
  SERVICE_MAP_TEXT: {
    bindingsShown: [[0, 1], [1, 1], [3, 12]],
    noBindings: [[0], [1], [3]],
    evidenceShown: [[0, 1], [1, 1], [2, 5]],
    declaredFigure: [[0, 0, 0], [1, 1, 1], [3, 4, 2], [1, 1, null]],
    hintFigure: [[1], [2]],
  },
};

const TABLES: Record<string, Record<string, unknown>> = { COVERAGE_TEXT, DASHBOARD_TEXT, HEALTH_TEXT, SERVICE_MAP_TEXT };

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
  const all = [
    ...sentences("COVERAGE_TEXT"),
    ...sentences("DASHBOARD_TEXT"),
    ...sentences("HEALTH_TEXT"),
    ...sentences("SERVICE_MAP_TEXT"),
    ...Object.entries(BINDING_KIND_LABEL).map(([k, label]) => [`kind ${k}`, label] as [string, CopyText]),
    ...Object.entries(NOT_RESOLVED_REMEDY).map(([k, r]) => [`remedy ${k}`, r.remedy] as [string, CopyText]),
    ["remedy (unlisted)", UNLISTED_REMEDY.remedy] as [string, CopyText],
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

  it("gives every reason the server sends a remedy, and an unknown one the listing command", () => {
    expect(Object.keys(NOT_RESOLVED_REMEDY).sort()).toEqual([
      "ambiguous",
      "base-url-runtime",
      "config-key-missing",
      "config-placeholder-value",
      "no-provider-in-workspace",
      "path-not-composed",
      "topic-not-literal",
    ]);
    expect(remedyFor("a-reason-from-a-later-arm")).toBe(UNLISTED_REMEDY);
    // Own keys only: an inherited name is not a reason.
    expect(remedyFor("constructor")).toBe(UNLISTED_REMEDY);
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
  });

  it("per-row member actions: re-index a degraded member, review deletions, else nothing", () => {
    expect(memberRowAction({ degraded: true, unusedAcross: null })).toMatchObject({
      kind: "act",
      where: "command",
      target: "logos index",
    });
    expect(memberRowAction({ degraded: false, unusedAcross: 3 })).toMatchObject({ kind: "act", where: "source code" });
    expect(memberRowAction({ degraded: false, unusedAcross: 0 })).toEqual({ kind: "none" });
    expect(memberRowAction({ degraded: false, unusedAcross: null })).toEqual({ kind: "none" });
  });

  it("gives each evidence row the action its refusal calls for, each with where (S-614, CR-203 item 7)", () => {
    const row = { member: "api", key: "billing.url", sources: [] as string[] };
    // No committed source defines it → define the key, in the member's configuration.
    expect(evidenceRowAction({ ...row, refusal: "missing-key" })).toEqual({
      kind: "act",
      where: "configuration",
      target: "billing.url",
      text: `In api, ${NOT_RESOLVED_REMEDY["config-key-missing"].remedy}.`,
    });
    // A placeholder → replace the committed value, worded as the coverage tab words it.
    expect(evidenceRowAction({ ...row, refusal: "placeholder-value" })).toEqual({
      kind: "act",
      where: "configuration",
      target: "billing.url",
      text: `In api, ${NOT_RESOLVED_REMEDY["config-placeholder-value"].remedy}.`,
    });
    // Not committed by the repository → nothing to fix there.
    expect(evidenceRowAction({ ...row, refusal: "uncommitted" })).toEqual({ kind: "none" });
    expect(SERVICE_MAP_TEXT.arrivesAtRuntime).toMatch(/^Nothing to fix in the repository/);
    // A committed value → the file named in Defining sources.
    expect(
      evidenceRowAction({ ...row, refusal: null, sources: ["application.yml", "application-docker.yml"] }),
    ).toMatchObject({ kind: "act", where: "configuration", target: "application.yml, application-docker.yml" });
    // A refusal token newer than this build is never told to correct a value
    // (its row shows none, and its sources are empty): it names the command
    // that states its reason.
    const later = evidenceRowAction({ ...row, refusal: "secret-ref" as unknown as null });
    expect(later).toMatchObject({ kind: "act", where: "command", target: "logos workspace status" });
  });
});

