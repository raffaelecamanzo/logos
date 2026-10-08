// Every copy catalogue glosses each internal term at its first use (S-611,
// FR-UI-39) — in what, then why. Catalogues are DISCOVERED (every `*.copy.ts`
// under src/), so a view's catalogue is held to the rule the moment it exists —
// no hand-kept list to forget.
// The walk below also reaches every OTHER text a catalogue exports (its tables,
// records and sentence functions), so a later story's table is held too.
import { describe, expect, it } from "vitest";

import { findUnglossedUses, isCopyEntry, plainPart, sentenceLiterals, type PlainPart } from "./text.ts";
import type { CopyEntry, CopyText } from "./types.ts";

const modules = import.meta.glob<Record<string, unknown>>("/src/**/*.copy.ts", { eager: true });

/** Is `value` catalogue text: a string, or an array of strings and glosses? */
function isCopyText(value: unknown): value is CopyText {
  return (
    typeof value === "string" ||
    (Array.isArray(value) && value.every((seg) => typeof seg === "string" || (typeof seg === "object" && seg !== null && "term" in seg)))
  );
}

/**
 * Walks every export of every catalogue, recursively (a record of entries such as
 * Health's `DIMENSION_COPY` included), and sorts what it finds into the entries
 * and every OTHER piece of reader text: the `*_TEXT` and `*_ABSENCE` tables, a
 * disclosure, an entry's extra fields (a dimension's `raw`), a per-row action. A
 * function is read by its string literals — so a new
 * sentence is held to the rule the moment it is exported, with no list to extend.
 */
function walk(mod: Record<string, unknown>, path: string) {
  const found: { entries: [string, CopyEntry][]; texts: [string, PlainPart][] } = { entries: [], texts: [] };
  const seen = new Set<unknown>();
  const visit = (key: string, value: unknown) => {
    if (typeof value === "object" && value !== null) {
      if (seen.has(value)) return;
      seen.add(value);
    }
    if (isCopyText(value)) {
      found.texts.push([key, plainPart(value)]);
    } else if (typeof value === "function") {
      found.texts.push([`${key}()`, sentenceLiterals(value as (...args: never[]) => unknown)]);
    } else if (isCopyEntry(value)) {
      found.entries.push([key, value]);
      for (const [field, inner] of Object.entries(value)) {
        if (field !== "what" && field !== "why") visit(`${key}.${field}`, inner);
      }
    } else if (typeof value === "object" && value !== null) {
      for (const [field, inner] of Object.entries(value)) visit(`${key}.${field}`, inner);
    }
  };
  for (const [name, value] of Object.entries(mod)) visit(`${path}#${name}`, value);
  return found;
}

const walked = Object.entries(modules).map(([path, mod]) => walk(mod, path));
const entries: [string, CopyEntry][] = walked.flatMap((w) => w.entries);
const texts: [string, PlainPart][] = walked.flatMap((w) => w.texts);

describe("copy catalogues", () => {
  it("discovers the catalogues (a finding, not a floor)", () => {
    // Every entry of the fixture catalogue is found, so a discovery that drops
    // entries of one shape (a glossed `what`, say) fails here.
    expect(entries.filter(([key]) => key.startsWith("/src/copy/fixture.copy.ts#")).map(([key]) => key)).toEqual([
      "/src/copy/fixture.copy.ts#observe",
      "/src/copy/fixture.copy.ts#thresholds",
      "/src/copy/fixture.copy.ts#coverage",
    ]);
    console.info(`copy catalogues: ${entries.length} entries in ${Object.keys(modules).length} modules`);
  });

  it.each(entries)("%s glosses every internal term at its first use", (_key, entry) => {
    const parts = [plainPart(entry.what), plainPart(entry.why)];
    const names = ["what", "why"];
    expect(findUnglossedUses(parts).map((u) => `${u.term} (in the ${names[u.part]})`)).toEqual([]);
  });
});

describe("every other sentence a catalogue exports (sprint review)", () => {
  // The entries above are not the only reader text a catalogue holds: figure-row
  // and absence tables, disclosures and per-row actions render in widgets too.
  // S-613 checked its own four tables by name (workspaceCopy.test.ts); this walk
  // holds every catalogue's, so a later story's table cannot be left out.

  /** Text that renders only AFTER its widget has glossed the term (a figure-row
   *  note under a figure that glosses it), so its first use is glossed in place.
   *  Each needs its reason; a stale exemption fails below. */
  const GLOSSED_EARLIER_IN_ITS_WIDGET: Record<string, string> = {
    "/src/copy/health.copy.ts#GATE_NOT_COMPARED":
      "the Gate's not-compared line renders under gateFigureText, whose figure glosses baseline first",
  };

  it("walks records and sentence functions, not only top-level entries", () => {
    const probe = walk(
      { RECORD: { one: { what: "W.", why: "Y." } }, TEXT: { line: (n: number) => `${n} arms` } },
      "probe",
    );
    expect(probe.entries.map(([key]) => key)).toEqual(["probe#RECORD.one"]);
    expect(probe.texts.map(([key, part]) => [key, findUnglossedUses([part]).map((u) => u.term)])).toEqual([
      ["probe#TEXT.line()", ["arm"]],
    ]);
    console.info(`copy catalogues: ${texts.length} other texts and sentence functions`);
  });

  it.each(texts.filter(([key]) => !(key in GLOSSED_EARLIER_IN_ITS_WIDGET)))(
    "%s uses no internal term outside a gloss",
    (_key, part) => {
      expect(findUnglossedUses([part]).map((u) => u.term)).toEqual([]);
    },
  );

  it("keeps no stale exemption: each still names an unglossed term", () => {
    for (const key of Object.keys(GLOSSED_EARLIER_IN_ITS_WIDGET)) {
      const part = texts.find(([k]) => k === key)?.[1];
      expect(part, `${key} is still a catalogue text`).toBeDefined();
      expect(findUnglossedUses([part!]).length, `${key} still needs its exemption`).toBeGreaterThan(0);
    }
  });
});
