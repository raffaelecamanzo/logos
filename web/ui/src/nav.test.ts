import { describe, expect, it } from "vitest";

import {
  isAppLevelPath,
  NAV_ITEMS,
  NAV_SCOPE_LABELS,
  NAV_SCOPES,
  navItemMatches,
  navItemsFor,
  scopeForPath,
  WORKSPACE_NAV_ITEMS,
  type NavItem,
  type NavScope,
} from "./nav.ts";

/** Every registered entry, in both modes. */
const ALL_ITEMS: readonly NavItem[] = [...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS];

describe("navItemsFor (S-250, FR-UI-29 AC4)", () => {
  it("leaves the single-root sidebar EXACTLY as it was — no workspace item leaks in", () => {
    // The AC is "in single-root mode the UI is byte-for-byte unchanged". The sidebar is
    // the most visible half of that, so pin identity, not just absence.
    expect(navItemsFor(false)).toEqual(NAV_ITEMS);
    expect(navItemsFor(false).some((i) => i.id === "workspace")).toBe(false);
  });

  it("appends the workspace tab — and only that — in workspace mode", () => {
    expect(navItemsFor(true)).toEqual([...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS]);
    const added = navItemsFor(true).filter((i) => !NAV_ITEMS.includes(i));
    expect(added.map((i) => [i.id, i.path])).toEqual([["workspace", "/workspace"]]);
  });
});

// ── S-425 / FR-UI-35 / ADR-66: scope is a declared, required field ────────────

describe("the scope field is required, not defaulted (ADR-66 §2)", () => {
  it("every registered entry declares a scope — absence is loud, at runtime as well as in tsc", () => {
    // `tsc -b` is the first gate: `scope` is non-optional on `NavItem`, so an entry
    // written without one fails the build. This is the second, and it is not a
    // duplicate — it catches a registration that reaches runtime with the field
    // stripped (a JS caller, a cast, a merge that dropped the line) where the type
    // system is no longer watching. There is no safe default to fall back to:
    // "member" is wrong for a workspace view and "app" is wrong for the other
    // eleven, so the failure has to be the outcome.
    expect(ALL_ITEMS.length).toBeGreaterThan(0);
    const undeclared = ALL_ITEMS.filter(
      (item) => !(NAV_SCOPES as readonly string[]).includes(item.scope),
    );
    expect(undeclared.map((i) => i.id)).toEqual([]);
  });

  it("names both scopes and nothing else, each with the words the sidebar renders", () => {
    expect([...NAV_SCOPES]).toEqual(["app", "member"]);
    expect(NAV_SCOPE_LABELS).toEqual({ app: "Workspace", member: "Service" });
  });

  it("scopes each existing view as it is actually served", () => {
    // The eleven pre-existing tabs answer for ONE member through `?repo=`; the
    // Workspace tab answers for the whole workspace over the unscoped fan-out.
    expect(NAV_ITEMS.map((i) => i.scope)).toEqual(NAV_ITEMS.map(() => "member"));
    expect(WORKSPACE_NAV_ITEMS.map((i) => [i.id, i.scope])).toEqual([["workspace", "app"]]);
  });
});

describe("isAppLevelPath is a lookup over that one field (S-425, ADR-66 §3)", () => {
  it("answers every registered route from its own declaration", () => {
    for (const item of ALL_ITEMS) {
      expect({ path: item.path, app: isAppLevelPath(item.path) }).toEqual({
        path: item.path,
        app: item.scope === "app",
      });
      expect(scopeForPath(item.path)).toBe(item.scope);
    }
  });

  it("carries the declaration down a tab's own sub-routes", () => {
    // Its reads are the unscoped `workspace/*` fan-out: identical for every member, so
    // remounting it on a member switch would tear down the canvas for no new data.
    expect(isAppLevelPath("/workspace")).toBe(true);
    expect(isAppLevelPath("/workspace/anything")).toBe(true);
    // …and a member-scoped tab's sub-route stays member-scoped (the Wiki reader).
    expect(isAppLevelPath("/wiki/page/x")).toBe(false);
  });

  it("leaves every member-scoped view keyed on the member", () => {
    for (const path of ["/", "/health", "/graph", "/coverage", "/config"]) {
      expect(isAppLevelPath(path)).toBe(false);
    }
  });

  it("never lets the root Dashboard's `/` swallow another tab's route", () => {
    // `"/health".startsWith("/")` is true, so a prefix test written without the
    // trailing slash resolves every route to the Dashboard's entry — which would be
    // the right ANSWER today (both are member-scoped) and the wrong MECHANISM,
    // silently mis-scoping the first app-level view registered at a nested path.
    //
    // Proven against the mutation that matters: rewriting the prefix arm as
    // `pathname.startsWith(item.path)` fails this. Deleting the `item.path !== "/"`
    // guard does NOT, and that is correct rather than a gap — `${"/"}/` is `//`, so
    // the trailing slash already carries the property and the guard restates it.
    expect(NAV_ITEMS[0].path).toBe("/");
    for (const item of NAV_ITEMS.filter((i) => i.path !== "/")) {
      expect(navItemMatches(NAV_ITEMS[0], item.path)).toBe(false);
    }
    // …including a route that is a strict string-prefix extension of another's.
    const wiki = NAV_ITEMS.find((i) => i.id === "wiki") as NavItem;
    expect(navItemMatches(wiki, "/wikipedia")).toBe(false);
  });

  it("keys an unregistered path as member-scoped, as it always has", () => {
    expect(scopeForPath("/not-a-view")).toBe("member");
    expect(isAppLevelPath("/not-a-view")).toBe(false);
  });

  it("resolves `/` to the per-member Dashboard in BOTH modes (FR-UI-35)", () => {
    for (const isWorkspace of [false, true]) {
      const root = navItemsFor(isWorkspace).filter((i) => i.path === "/");
      expect(root.map((i) => [i.id, i.scope])).toEqual([["overview", "member"]]);
    }
    expect(scopeForPath("/")).toBe("member");
  });
});

describe("a section label is never the only way to tell two views apart (NFR-CC-04)", () => {
  it("never registers two same-named views in the SAME scope", () => {
    // Two views MAY share a name across scopes — ADR-66 accepts a per-member
    // "Dashboard" and a workspace "Dashboard", disambiguated by their section. Inside
    // one section there is no disambiguator left, so a repeat there is a defect.
    for (const scope of NAV_SCOPES) {
      const labels = ALL_ITEMS.filter((i) => i.scope === scope).map((i) => i.label);
      expect(labels).toEqual([...new Set(labels)]);
    }
  });
});

// ── The grep half of "no second list of app-level paths" (FR-UI-35, ADR-66 §1) ─

/**
 * Every non-test module under `src/`, as `path → source text`.
 *
 * Read through Vite's own `import.meta.glob` rather than `node:fs`: the SPA's
 * toolchain declares no Node types (`tsconfig.json` has no `@types/node`), and
 * adding a dependency to let a test read a directory would be a heavier change
 * than the assertion is worth. The glob is resolved by the bundler against this
 * file's directory, so it cannot silently point at the wrong tree the way a cwd
 * can — but the walk still asserts what it found, below.
 */
const SOURCES: Record<string, string> = Object.fromEntries(
  Object.entries(
    import.meta.glob("./**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }),
  )
    .filter(([path]) => !/\.test\.tsx?$/.test(path) && !/\.d\.ts$/.test(path))
    .map(([path, source]) => [path.replace(/^\.\//, ""), source as string]),
);

describe("no second list of app-level paths exists in the tree (ADR-66 §1)", () => {
  // The defect this replaces was a hard-coded path list consulted in one place. The
  // hazard it leaves behind is a SECOND one: a `startsWith("/workspace")` in a view,
  // a `const APP_PATHS = [...]` beside a new feature. Either one has to spell the
  // route out, so the route literal is the thing to count — and it is counted by
  // reading the source, because a behavioural test cannot see a duplicate that
  // happens to agree with the registry today.
  const modules = Object.keys(SOURCES);

  it("reads a source tree at all — an empty walk would pass every assertion below", () => {
    // The denominator. A walker that resolved to the wrong directory returns [], and
    // "no file mentions the path" is then true and worthless.
    expect(modules.length).toBeGreaterThan(30);
    expect(modules).toContain("nav.ts");
    expect(modules).toContain("views/index.ts");
  });

  it.each(WORKSPACE_NAV_ITEMS.filter((i) => i.scope === "app").map((i) => i.path))(
    "spells the app-scoped route %s in the two registries and nowhere else",
    (path) => {
      const literal = `"${path}"`;
      const mentions = modules.filter((m) => SOURCES[m].includes(literal));
      // `nav.ts` — the navigation registry that DECLARES the scope. `views/index.ts` —
      // the component registry that maps the same path to the React view it mounts;
      // it is keyed by path by construction and holds no scope. Any third file is
      // re-deriving the level from the route spelling, which ADR-66 §1 forbids.
      expect(mentions.sort()).toEqual(["nav.ts", "views/index.ts"]);
    },
  );

  it("keeps the app-level predicate itself in one module", () => {
    const definers = modules.filter((m) =>
      /(export\s+)?function\s+(isAppLevelPath|scopeForPath)\b/.test(SOURCES[m]),
    );
    expect(definers).toEqual(["nav.ts"]);
  });
});

describe("navItemMatches (S-425) — the SPA's one route-ownership rule", () => {
  it("matches a route exactly, and the sub-routes a tab owns", () => {
    const wiki = NAV_ITEMS.find((i) => i.id === "wiki") as NavItem;
    expect(navItemMatches(wiki, "/wiki")).toBe(true);
    expect(navItemMatches(wiki, "/wiki/page/x")).toBe(true);
    // A near miss: a sibling route that merely starts with the same characters.
    expect(navItemMatches(wiki, "/wikipedia")).toBe(false);
    expect(navItemMatches(wiki, "/health")).toBe(false);
  });

  it("types the scope union closed", () => {
    // A compile-time assertion, kept next to its runtime twin: widening NavScope
    // without teaching the sidebar its label fails here rather than rendering
    // `undefined` as a section heading.
    const labelled: Record<NavScope, string> = NAV_SCOPE_LABELS;
    expect(Object.keys(labelled).sort()).toEqual([...NAV_SCOPES].sort());
  });
});
