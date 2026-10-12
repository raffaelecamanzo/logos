import { describe, expect, it } from "vitest";

import {
  memberChipFacts,
  memberChips,
  workspaceChipFacts,
  workspaceChips,
  type FilesReadModel,
  type MemberChipFacts,
  type WorkspaceChipFacts,
} from "./suggestionChips.ts";

/** A `/files` bundle carrying only what the chip templates read. */
function files(paths: string[], fileCount = 120, indexed = true): FilesReadModel {
  return {
    status: { indexed, file_count: fileCount },
    hotspots: { files: paths.map((path) => ({ path })) },
  };
}

const NO_MEMBER_FACTS: MemberChipFacts = { topHotspot: null, riskiestUntested: null, fileCount: null };
const NO_WORKSPACE_FACTS: WorkspaceChipFacts = { workspace: null, memberCount: 0, member: null };

describe("memberChipFacts", () => {
  it("reads the top-ranked hotspot, the top untested file and the indexed file count", () => {
    const facts = memberChipFacts(
      files(["src/engine.rs", "src/store.rs"], 120),
      files(["src/legacy.rs"]),
    );
    expect(facts).toEqual({
      topHotspot: "src/engine.rs",
      riskiestUntested: "src/legacy.rs",
      fileCount: 120,
    });
  });

  it("reports nothing for a read that failed, was never made, or returned an empty shape", () => {
    expect(memberChipFacts(null, null)).toEqual(NO_MEMBER_FACTS);
    expect(memberChipFacts(undefined, undefined)).toEqual(NO_MEMBER_FACTS);
    // The test fixtures' unmatched `/api/v1` paths answer `{}`.
    expect(memberChipFacts({} as FilesReadModel, {} as FilesReadModel)).toEqual(NO_MEMBER_FACTS);
    expect(memberChipFacts(files([], 0, false), files([], 0, false))).toEqual(NO_MEMBER_FACTS);
  });

  it("takes a file count only from an indexed graph, never a fabricated zero", () => {
    expect(memberChipFacts(files(["a.rs"], 0), null).fileCount).toBeNull();
    expect(memberChipFacts(files(["a.rs"], 40, false), null).fileCount).toBeNull();
  });

  it("refuses a path it could not quote verbatim inside a code span", () => {
    expect(memberChipFacts(files(["a`b.rs"]), null).topHotspot).toBeNull();
    expect(memberChipFacts(files(["a\nb.rs"]), null).topHotspot).toBeNull();
    expect(memberChipFacts(files(["a\rb.rs"]), null).topHotspot).toBeNull();
    expect(memberChipFacts(files(["   "]), null).topHotspot).toBeNull();
  });

  it("reads each fact independently: a hotspot with no untested board yields only the hotspot", () => {
    const facts = memberChipFacts(files(["src/engine.rs"], 7), files([], 7));
    expect(facts.topHotspot).toBe("src/engine.rs");
    expect(facts.riskiestUntested).toBeNull();
    expect(facts.fileCount).toBe(7);
  });
});

describe("memberChips", () => {
  it("names a returned fact in every chip when every fact is returned", () => {
    const chips = memberChips({
      topHotspot: "src/engine.rs",
      riskiestUntested: "src/legacy.rs",
      fileCount: 1200,
    });
    expect(chips.map((c) => c.text)).toEqual([
      "Why is `src/engine.rs` the top hotspot, and what depends on it?",
      "What would it take to cover `src/legacy.rs`, the riskiest untested file?",
      "Give me a tour of this project's 1,200 indexed files.",
    ]);
    expect(chips.every((c) => c.grounded)).toBe(true);
  });

  it("falls back to a generic chip for each template whose fact is absent", () => {
    const chips = memberChips(NO_MEMBER_FACTS);
    expect(chips).toHaveLength(3);
    expect(chips.every((c) => !c.grounded)).toBe(true);
    for (const chip of chips) {
      expect(chip.text).not.toMatch(/`/); // no code span: nothing is named
      expect(chip.text).not.toMatch(/\d/); // no figure: nothing is counted
    }
  });

  it("mixes: only the template whose fact is missing turns generic", () => {
    const chips = memberChips({ topHotspot: "src/engine.rs", riskiestUntested: null, fileCount: null });
    expect(chips.map((c) => c.grounded)).toEqual([true, false, false]);
    expect(chips[0]?.text).toContain("`src/engine.rs`");
  });

  it("never names a path that was not in the facts", () => {
    for (const facts of [
      NO_MEMBER_FACTS,
      { topHotspot: "a.rs", riskiestUntested: null, fileCount: null },
      { topHotspot: null, riskiestUntested: "b.rs", fileCount: 3 },
    ] satisfies MemberChipFacts[]) {
      const allowed = new Set([facts.topHotspot, facts.riskiestUntested].filter(Boolean));
      for (const chip of memberChips(facts)) {
        for (const quoted of chip.text.match(/`[^`]+`/g) ?? []) {
          expect(allowed.has(quoted.slice(1, -1))).toBe(true);
        }
      }
    }
  });

  it("keeps the chip count within the 2–4 the requirement allows, with distinct keys", () => {
    for (const facts of [
      NO_MEMBER_FACTS,
      { topHotspot: "a.rs", riskiestUntested: "b.rs", fileCount: 9 },
    ] satisfies MemberChipFacts[]) {
      const chips = memberChips(facts);
      expect(chips.length).toBeGreaterThanOrEqual(2);
      expect(chips.length).toBeLessThanOrEqual(4);
      expect(new Set(chips.map((c) => c.key)).size).toBe(chips.length);
    }
  });
});

describe("workspaceChipFacts", () => {
  it("reads the workspace name, the roster's size and the selected member from the roster alone", () => {
    expect(workspaceChipFacts("acme", ["api", "web"], "web")).toEqual({
      workspace: "acme",
      memberCount: 2,
      member: "web",
    });
  });

  it("reports no member for a selection the roster does not list, or none at all", () => {
    expect(workspaceChipFacts("acme", ["api", "web"], "ghost").member).toBeNull();
    expect(workspaceChipFacts("acme", ["api", "web"], null).member).toBeNull();
    expect(workspaceChipFacts("acme", [], "api").member).toBeNull();
  });

  it("reports nothing for an unnamed workspace and an empty roster", () => {
    expect(workspaceChipFacts(null, [], null)).toEqual(NO_WORKSPACE_FACTS);
  });

  it("refuses a name it could not quote verbatim inside a code span", () => {
    const facts = workspaceChipFacts("ac`me", ["a`b", "web"], "a`b");
    expect(facts.workspace).toBeNull();
    expect(facts.member).toBeNull();
    expect(facts.memberCount).toBe(2);
    expect(workspaceChipFacts("ac\r\nme", ["a", "b"], "a\rb").workspace).toBeNull();
  });
});

describe("workspaceChips", () => {
  it("names a returned fact in the chips that carry a slot when every fact is returned", () => {
    const chips = workspaceChips({ workspace: "acme", memberCount: 3, member: "web" });
    expect(chips.map((c) => c.text)).toEqual([
      "How do the 3 members of `acme` depend on each other?",
      "Which services talk to each other over a message broker?",
      "What does `web` do, and which other members depend on it?",
    ]);
    // The broker chip has no slot, so it is a fixed question and never "grounded".
    expect(chips.map((c) => c.grounded)).toEqual([true, false, true]);
  });

  it("falls back to generic chips with no facts, naming nothing", () => {
    const chips = workspaceChips(NO_WORKSPACE_FACTS);
    expect(chips).toHaveLength(3);
    expect(chips.every((c) => !c.grounded)).toBe(true);
    for (const chip of chips) {
      expect(chip.text).not.toMatch(/`/);
      expect(chip.text).not.toMatch(/\d/);
    }
  });

  it("does not say 'depend on each other' of a single member, nor name a lone member", () => {
    const chips = workspaceChips({ workspace: "acme", memberCount: 1, member: "api" });
    expect(chips[0]?.grounded).toBe(false);
    expect(chips[0]?.text).not.toContain("1 members");
    expect(chips[2]?.grounded).toBe(false);
    expect(chips[2]?.text).not.toContain("api");
  });

  it("states the member count without the workspace name when the name is unusable", () => {
    const chips = workspaceChips({ workspace: null, memberCount: 4, member: null });
    expect(chips[0]).toMatchObject({ grounded: true, text: "How do the 4 members of this workspace depend on each other?" });
    expect(chips[2]?.grounded).toBe(false);
  });

  it("keeps distinct keys — in every fact combination, and apart from the member chat's", () => {
    const memberKeys = [
      ...memberChips(NO_MEMBER_FACTS),
      ...memberChips({ topHotspot: "a.rs", riskiestUntested: "b.rs", fileCount: 9 }),
    ].map((c) => c.key);
    for (const facts of [
      NO_WORKSPACE_FACTS,
      { workspace: "acme", memberCount: 3, member: "web" },
    ] satisfies WorkspaceChipFacts[]) {
      const keys = workspaceChips(facts).map((c) => c.key);
      expect(new Set(keys).size).toBe(keys.length);
      expect(keys.filter((k) => memberKeys.includes(k))).toEqual([]);
    }
  });
});
