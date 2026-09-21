import { afterEach, describe, expect, it, vi } from "vitest";

// The table as TEXT (Vite's `?raw`), not a re-typed copy: `member.rs` reads the very
// same bytes through `include_str!`.
import CASES_TABLE from "./repo-param-cases.txt?raw";

import { saveConfig } from "../api/configClient.ts";
import {
  memberFromSearch,
  normaliseMember,
  REPO_PARAM,
  scopedMember,
  setScopedMember,
  urlWithMember,
  withMemberScope,
} from "./scope.ts";

/** The `?repo=` contract table both spellings of the rule are pinned against —
 *  this one, and `web/src/member.rs`'s (see the file's own header). Read from disk
 *  rather than re-typed here: a copy would drift, which is the whole failure this
 *  table exists to make impossible. */
function sharedCases(): { query: string; expected: string | null }[] {
  return CASES_TABLE
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#"))
    .map((line) => {
      const [query, expected] = line.split("\t");
      return { query, expected: expected === "-" ? null : expected };
    });
}

afterEach(() => {
  setScopedMember(null);
  vi.unstubAllGlobals();
});

describe("the member scope (S-250, FR-UI-29)", () => {
  it("starts unscoped, so a single-root SPA never sends a member", () => {
    expect(scopedMember()).toBeNull();
    expect(withMemberScope("/config/save")).toBe("/config/save");
  });

  it("normalises a blank member to unscoped rather than sending an empty ?repo=", () => {
    setScopedMember("  ");
    expect(scopedMember()).toBeNull();
    setScopedMember("api");
    expect(scopedMember()).toBe("api");
    setScopedMember(null);
    expect(scopedMember()).toBeNull();
  });

  it("appends the member to a mutating path, URL-encoding a nested member name", () => {
    setScopedMember("services/api");
    expect(withMemberScope("/config/save")).toBe("/config/save?repo=services%2Fapi");
    expect(withMemberScope("/config/apply?file=rules")).toBe(
      "/config/apply?file=rules&repo=services%2Fapi",
    );
  });

  it("carries the member on a config WRITE — the editor never saves over another member", async () => {
    // The load-bearing case: the Config tab reads the selected member's policy, so
    // its Save must write back to THAT member, not the workspace default's file.
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        calls.push(url);
        return Promise.resolve({ ok: true, json: () => Promise.resolve({}) } as Response);
      }),
    );
    setScopedMember("web");
    await saveConfig("rules", "[rules]\n");
    expect(calls[0]).toBe("/config/save?repo=web");
  });
});

describe("the shared `?repo=` rule (S-426, FR-UI-35, NFR-RA-05)", () => {
  const cases = sharedCases();

  it("reads the shared case table — a table that read as empty would pin nothing", () => {
    // The denominator. If the fixture moved or the parser stopped matching its
    // format, every `it.each` below would vacuously pass over zero rows.
    expect(cases.length).toBeGreaterThanOrEqual(20);
    expect(cases.filter((c) => c.expected === null).length).toBeGreaterThanOrEqual(5);
    expect(cases.filter((c) => c.expected !== null).length).toBeGreaterThanOrEqual(5);
    // A count alone cannot say WHICH rows were read: a parser that silently dropped
    // the near-miss keys would still clear the floor above with rows to spare. These
    // are the rows whose loss would be invisible and would matter.
    expect(cases.map((c) => c.query)).toEqual(
      expect.arrayContaining([
        "REPO=api",
        "arepo=api",
        "repo2=api",
        "repo=first&repo=last",
        "repo=%EF%BB%BFapi",
        "repo=+",
      ]),
    );
  });

  it.each(cases)("normalises `?$query` exactly as the server does", ({ query, expected }) => {
    // The server's half of this same table is asserted in `web/src/member.rs`'s
    // tests. If the two spellings ever disagree about a row, one of the two reds.
    expect(memberFromSearch(query)).toBe(expected);
  });

  it("applies the same rule through the query reader and the transport setter", () => {
    // One rule, not two that happen to agree today: `setScopedMember` and
    // `memberFromSearch` both funnel through `normaliseMember`.
    for (const raw of ["  api  ", "", "   ", "\t\n", "services/api"]) {
      setScopedMember(raw);
      expect(scopedMember()).toBe(normaliseMember(raw));
      expect(memberFromSearch(`${REPO_PARAM}=${encodeURIComponent(raw)}`)).toBe(
        normaliseMember(raw),
      );
    }
  });

  it("accepts a leading `?`, as `window.location.search` supplies it", () => {
    expect(memberFromSearch("?repo=api")).toBe("api");
    expect(memberFromSearch("")).toBeNull();
    expect(memberFromSearch("?")).toBeNull();
  });
});

describe("urlWithMember — the browser-URL write (S-426, ADR-52)", () => {
  it("appends the member to a URL that carries no query", () => {
    expect(urlWithMember("/health", "api")).toBe("/health?repo=api");
  });

  it("replaces an existing repo rather than appending a second one", () => {
    expect(urlWithMember("/health?repo=api", "web")).toBe("/health?repo=web");
    // …including one that arrived percent-encoded or blank.
    expect(urlWithMember("/health?repo=%20api%20", "web")).toBe("/health?repo=web");
    expect(urlWithMember("/health?repo=", "web")).toBe("/health?repo=web");
  });

  it("carries every other pair across byte-for-byte, and keeps the fragment", () => {
    // Not a round-trip through URLSearchParams: that would rewrite `%20` as `+` in
    // a value this function has no business touching.
    expect(urlWithMember("/graph?seed=a%20b&cap=50", "web")).toBe(
      "/graph?seed=a%20b&cap=50&repo=web",
    );
    expect(urlWithMember("/graph?seed=x#node-3", "web")).toBe("/graph?seed=x&repo=web#node-3");
  });

  it("URL-encodes a nested member name", () => {
    expect(urlWithMember("/health", "services/api")).toBe("/health?repo=services%2Fapi");
  });

  it("does NOT mistake a near-miss param for the member's own", () => {
    // `notrepo`/`arepo`/`repo2` are not `repo`; dropping one would silently lose a
    // view's own state on every member switch. These are the UNDER-capture direction
    // — keys that are longer than `repo`.
    expect(urlWithMember("/graph?notrepo=a&arepo=b&repo2=c", "web")).toBe(
      "/graph?notrepo=a&arepo=b&repo2=c&repo=web",
    );
  });

  it("does NOT capture a key that only becomes `repo` under a transformation", () => {
    // The OVER-capture direction, which a longer-key case cannot reach: keys that
    // equal `repo` once you fold case, trim, or decode twice. Each of those is a
    // one-word edit to `pairKey`, and each would silently delete somebody else's
    // param on every member switch. `REPO` in particular is pinned as NOT the member
    // by the shared table (`REPO=api` → unscoped) and by the server, so capturing it
    // here would contradict a rule asserted two files away.
    // Both spellings of "padded key" are here on purpose, because they catch
    // DIFFERENT edits: ` repo` (a literal space) catches a trim applied before the
    // decode, `%20repo` catches one applied after it.
    expect(urlWithMember("/graph?REPO=a& repo=b&%20repo=c&%2572epo=d&repo%00=e", "web")).toBe(
      "/graph?REPO=a& repo=b&%20repo=c&%2572epo=d&repo%00=e&repo=web",
    );
  });

  it("replaces an ENCODED spelling of the key, and survives a malformed one", () => {
    // A URL that arrives from outside (hand-typed, or pasted from somewhere that
    // escaped it) can spell the key `%72epo`. It is the same param, so it must be
    // REPLACED — appending beside it would leave two `repo` pairs and let the stale
    // one win on the server's last-wins decode.
    expect(urlWithMember("/graph?%72epo=old", "web")).toBe("/graph?repo=web");
    expect(urlWithMember("/graph?re%70o=old&seed=x", "web")).toBe("/graph?seed=x&repo=web");
    // …and a key that is not decodable at all is carried through rather than thrown
    // on: a malformed URL must not take the navigation down with it.
    expect(urlWithMember("/graph?%zz=1", "web")).toBe("/graph?%zz=1&repo=web");
  });

  it("leaves the URL untouched when unscoped — including a HAND-TYPED ?repo=", () => {
    // The single-root guarantee (ADR-52): single-root never has a scope, so no
    // navigation writes a `?repo=` — and a hand-typed one stays exactly where the
    // user put it, inert. Stripping it would be this mode noticing a param it is
    // supposed to be blind to.
    expect(urlWithMember("/health", null)).toBe("/health");
    expect(urlWithMember("/health?repo=web", null)).toBe("/health?repo=web");
    expect(urlWithMember("/graph?seed=a%20b#x", null)).toBe("/graph?seed=a%20b#x");
  });
});
