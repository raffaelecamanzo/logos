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

/**
 * `source` with its comments removed, so the guards below read CODE and not prose.
 *
 * A character scanner rather than a regex, because the two are not separable by one:
 * a `//` inside a string literal is not a comment, and a `/* … *\/` inside a template
 * literal is not either. Widening the route probe to be quote-agnostic immediately
 * produced a false positive on a backticked `/workspace` inside an explanatory
 * comment in `views/workspace/WorkspaceView.tsx` — a guard that fires on prose gets
 * switched off by the next person who trips it.
 */
function codeOf(source: string): string {
  let out = "";
  let i = 0;
  while (i < source.length) {
    const two = source.slice(i, i + 2);
    if (two === "//") {
      while (i < source.length && source[i] !== "\n") i += 1;
      continue;
    }
    if (two === "/*") {
      i += 2;
      while (i < source.length && source.slice(i, i + 2) !== "*/") i += 1;
      i += 2;
      continue;
    }
    const ch = source[i];
    if (ch === '"' || ch === "'" || ch === "`") {
      const quote = ch;
      out += ch;
      i += 1;
      while (i < source.length && source[i] !== quote) {
        if (source[i] === "\\") {
          out += source.slice(i, i + 2);
          i += 2;
          continue;
        }
        out += source[i];
        i += 1;
      }
      out += quote;
      i += 1;
      continue;
    }
    out += ch;
    i += 1;
  }
  return out;
}

describe("no second list of app-level paths exists in the tree (ADR-66 §1)", () => {
  // The defect this replaces was a hard-coded path list consulted in one place. The
  // hazard it leaves behind is a SECOND one: a `startsWith("/workspace")` in a view,
  // a `const APP_PATHS = [...]` beside a new feature. Either one has to spell the
  // route out, so the route literal is the thing to count — and it is counted by
  // reading the source, because a behavioural test cannot see a duplicate that
  // happens to agree with the registry today.
  const modules = Object.keys(SOURCES);

  it("reads the WHOLE source tree — a partial walk would pass every assertion below", () => {
    // The denominator, and it pins the walk's SHAPE rather than a floor a subset
    // clears. Found in review: `> 30` with two `.ts` sentinels passed over a walk
    // that had lost every `.tsx` file — 37 `.ts` modules clear 30, and both
    // sentinels are `.ts`. The whole component tree, including `App.tsx` where the
    // defect this guard exists for actually lived, dropped out in silence and a
    // planted second list went undetected. So both extensions are counted
    // separately and a `.tsx` sentinel is named.
    const tsx = modules.filter((m) => m.endsWith(".tsx"));
    const ts = modules.filter((m) => m.endsWith(".ts"));
    expect(ts.length).toBeGreaterThan(30);
    expect(tsx.length).toBeGreaterThan(30);
    expect(modules).toContain("nav.ts");
    expect(modules).toContain("views/index.ts");
    expect(modules).toContain("App.tsx");
    expect(modules).toContain("shell/Sidebar.tsx");
  });

  // Every app-scoped route, from EITHER registry. Nothing stops an `app` entry being
  // registered in `NAV_ITEMS` — that is how a promoted tab would land — and keying
  // this off `WORKSPACE_NAV_ITEMS` alone would exempt it from the grep half.
  const appPaths = ALL_ITEMS.filter((i) => i.scope === "app").map((i) => i.path);

  it("has app-scoped routes to check at all", () => {
    // `it.each([])` registers ZERO tests and raises nothing, so without this floor
    // the assertion AC1 names by name would VANISH rather than fail the moment no
    // entry declared `app` — a guard that disables itself exactly when the field it
    // guards is what broke. Found in review.
    expect(appPaths.length).toBeGreaterThan(0);
  });

  it.each(appPaths)(
    "spells the app-scoped route %s in the two registries and nowhere else",
    (path) => {
      // Quote-agnostic, and matched as a PREFIX. An exact `"${path}"` search was
      // evaded three ways, all reproduced in review: `"/workspace/"` (a trailing
      // slash — the natural spelling of `startsWith`) does not contain the substring
      // `"/workspace"`; and `web/ui` configures neither ESLint nor Prettier, so
      // single quotes and template literals are not hypothetical.
      const probe = new RegExp(`["'\`]${path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);
      const mentions = modules.filter((m) => probe.test(codeOf(SOURCES[m])));
      // `nav.ts` — the navigation registry that DECLARES the scope. `views/index.ts` —
      // the component registry that maps the same path to the React view it mounts;
      // it is keyed by path by construction and holds no scope. Any third file is
      // re-deriving the level from the route spelling, which ADR-66 §1 forbids.
      expect(mentions.sort()).toEqual(["nav.ts", "views/index.ts"]);
    },
  );

  it("keeps the app-level predicate itself in one module", () => {
    // `function` is not the only way to declare one. An arrow const is the dominant
    // style for small helpers in this tree, and `export const scopeForPath = (p) =>
    // p.split("/")[1] === "workspace" ? "app" : "member"` evaded BOTH this guard and
    // the route-literal one above — no `function` keyword, no route literal.
    // Reproduced in review; both matchers were widened.
    const definers = modules.filter((m) =>
      /\b(function|const|let|var)\s+(isAppLevelPath|scopeForPath)\b/.test(codeOf(SOURCES[m])),
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
