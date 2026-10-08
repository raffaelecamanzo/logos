// Every copy catalogue glosses each internal term at its first use (S-611,
// FR-UI-39) — in what, why, and the text its action can return. Catalogues are DISCOVERED (every `*.copy.ts` under src/), so a view's
// catalogue is held to the rule the moment it exists — no hand-kept list to forget.
import { describe, expect, it } from "vitest";

import { actionLiterals, findUnglossedUses, isCopyEntry, plainPart } from "./text.ts";
import type { CopyEntry } from "./types.ts";

const modules = import.meta.glob<Record<string, unknown>>("/src/**/*.copy.ts", { eager: true });

const entries: [string, CopyEntry<never>][] = Object.entries(modules).flatMap(([path, mod]) =>
  Object.entries(mod)
    .filter(([, value]) => isCopyEntry(value))
    .map(([name, value]) => [`${path}#${name}`, value as CopyEntry<never>] as [string, CopyEntry<never>]),
);

describe("copy catalogues", () => {
  it("discovers the catalogues (a finding, not a floor)", () => {
    // The fixture catalogue is always among them, so zero means discovery broke.
    expect(entries.some(([key]) => key.startsWith("/src/copy/fixture.copy.ts#"))).toBe(true);
    console.info(`copy catalogues: ${entries.length} entries in ${Object.keys(modules).length} modules`);
  });

  it.each(entries)("%s glosses every internal term at its first use", (_key, entry) => {
    // what, then why, then every word the action can say (its string literals).
    const parts = [plainPart(entry.what), plainPart(entry.why), actionLiterals(entry.action)];
    const names = ["what", "why", "action"];
    expect(findUnglossedUses(parts).map((u) => `${u.term} (in the ${names[u.part]})`)).toEqual([]);
  });
});
