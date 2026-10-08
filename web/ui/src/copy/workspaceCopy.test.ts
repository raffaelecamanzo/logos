// The workspace catalogues' sentences (S-613, FR-UI-39). The catalogue test holds
// every `CopyEntry` to the vocabulary rule; this holds the figure-row and absence
// sentences and the remedy table beside them, which render in the parts
// `expectWidgetCopy` reads but are not entries themselves.
import { describe, expect, it } from "vitest";

import { COVERAGE_TEXT, NOT_RESOLVED_REMEDY, remedyFor, UNLISTED_REMEDY } from "./coverage.copy.ts";
import { findUnglossedUses, plainPart } from "./text.ts";
import type { CopyText } from "./types.ts";
import { DASHBOARD_TEXT, memberRowAction } from "./workspaceDashboard.copy.ts";
import { HEALTH_TEXT } from "./workspaceHealth.copy.ts";

/** Every sentence a table can produce, over sample arguments (one and many). A
 *  fixed sentence may be catalogue text with glosses in it (`CopyText`). */
function sentences(table: Record<string, unknown>): [string, CopyText][] {
  return Object.entries(table).flatMap(([key, value]) => {
    if (typeof value === "string" || Array.isArray(value)) return [[key, value as CopyText] as [string, CopyText]];
    const fn = value as (...args: unknown[]) => string;
    return [
      [`${key}(1)`, fn(1, 1, 1, 1)],
      [`${key}(7)`, fn(7, 9, 7, 3)],
      [`${key}(partial)`, fn(false, 2, 3, true)],
      [`${key}([…])`, fn(["a", "b"], 2, 3, false)],
    ].filter(([, text]) => typeof text === "string") as [string, string][];
  });
}

describe("workspace catalogue sentences", () => {
  const all = [
    ...sentences(COVERAGE_TEXT),
    ...sentences(DASHBOARD_TEXT),
    ...sentences(HEALTH_TEXT),
    ...Object.entries(NOT_RESOLVED_REMEDY).map(([k, r]) => [`remedy ${k}`, r.remedy] as [string, CopyText]),
    ["remedy (unlisted)", UNLISTED_REMEDY.remedy] as [string, CopyText],
  ];

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
});
