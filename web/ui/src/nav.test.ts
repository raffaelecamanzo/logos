import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  isAppLevelPath,
  NAV_ITEMS,
  NAV_SCOPE_LABELS,
  NAV_SCOPES,
  navItemMatches,
  navItemsFor,
  scopeForPath,
  WORKSPACE_NAV_ITEMS,
  workspaceReplacementPath,
  type NavItem,
  type NavScope,
} from "./nav.ts";
import type { ExtensionNavEntry } from "./api/extensionsClient.ts";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "./extensions/registry.ts";
import { extensionsInfo, STUB_ID, StubAppView, StubView, stubInfo } from "./test/extensions.tsx";
import { removeHiddenWidgetEntry } from "./test/hiddenWidgets.ts";
import { viewForPath } from "./views/index.ts";

/** Routes a test withdraws the view from, over the real route list: since S-740
 *  registered the last built-in view, no built-in entry reaches S-788's "an entry
 *  no view owns is not offered" filters, so a test makes one. Cleared after each. */
const { UNREGISTERED } = vi.hoisted(() => ({ UNREGISTERED: new Set<string>() }));
vi.mock("./views/routes.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("./views/routes.ts")>();
  return { ...real, hasViewFor: (pathname: string) => !UNREGISTERED.has(pathname) && real.hasViewFor(pathname) };
});

/** A member-scoped tab that owns client sub-routes, declared here because the
 *  route-ownership rule is the subject, not which entry is registered. */
const READER_ITEM: NavItem = { id: "reader", label: "Reader", path: "/reader", group: "A", scope: "member" };

/** A stub extension's member entry, replaced in a workspace by its own app entry. */
const REPLACED: ExtensionNavEntry = {
  id: "stub",
  label: "Stub",
  path: "/stub",
  group: "A",
  scope: "member",
  replaced_in_workspace_by: "stub-app",
};
const REPLACEMENT: ExtensionNavEntry = { id: "stub-app", label: "Stub app", path: "/stub-app", group: "A", scope: "app" };

/** Mount the stub with the two entries above, a view registered for each. */
function mountReplacingStub(): void {
  registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView, "/stub-app": StubAppView } });
  setActiveExtensions(extensionsInfo(stubInfo({ ui: { nav: [REPLACED, REPLACEMENT], views: [], config_panels: [] } })));
}

/** Every registered entry, in both modes. */
const ALL_ITEMS: readonly NavItem[] = [...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS];

describe("navItemsFor (S-250, FR-UI-29 AC4)", () => {
  it("leaves the single-root sidebar EXACTLY as it was — no workspace item leaks in", () => {
    // The AC is "in single-root mode the UI is byte-for-byte unchanged". The sidebar is
    // the most visible half of that, so pin identity, not just absence — less the
    // Architecture view, which the hidden-widget register hides (CR-208).
    expect(navItemsFor(false)).toEqual(NAV_ITEMS.filter((i) => i.id !== "architecture"));
    expect(navItemsFor(false).some((i) => i.id === "workspace")).toBe(false);
  });

  it("offers the workspace-only tabs — and only those — in workspace mode", () => {
    // Less the Architecture view, which the register hides (CR-208). The Coverage
    // entry is offered since S-740 registered its view (S-788).
    expect(navItemsFor(true)).toEqual([...NAV_ITEMS.filter((i) => i.id !== "architecture"), ...WORKSPACE_NAV_ITEMS]);
  });

  it("pins both sections as literal rosters, in the order and groups they render (S-788)", () => {
    // Pinned as literals rather than derived from the lists: sessions append to these
    // ONE lists, and a parallel append to a single list is a known entry-DROPPING
    // merge. A derived expectation agrees with whatever survived the merge and
    // reports nothing. Architecture is absent while the register hides it (CR-208);
    // the workspace Coverage entry is offered since S-740 registered its view (S-788).
    const member = navItemsFor(true).filter((i) => i.scope === "member");
    expect(member.map((i) => [i.id, i.group])).toEqual([
      ["overview", "A"],
      ["health", "A"],
      ["security", "A"],
      ["coverage", "A"],
      ["graph", "B"],
      ["gaps", "B"],
      ["files", "B"],
      ["statistics", "C"],
      ["config", "C"],
    ]);
    const app = navItemsFor(true).filter((i) => i.scope === "app");
    expect(app.map((i) => [i.id, i.path, i.group])).toEqual([
      ["workspace-dashboard", "/workspace-dashboard", "A"],
      ["workspace", "/workspace", "A"],
      ["workspace-health", "/workspace-health", "A"],
      ["workspace-security", "/workspace-security", "B"],
      ["workspace-coverage", "/workspace-coverage", "B"],
      ["workspace-statistics", "/workspace-statistics", "C"],
      ["workspace-config", "/workspace-config", "C"],
    ]);
    expect(navItemsFor(false).map((i) => i.id)).toEqual(member.map((i) => i.id));
  });

  it("files the hidden Architecture entry in group B directly after Graph (S-788)", () => {
    const ids = NAV_ITEMS.map((i) => i.id);
    expect(ids.indexOf("architecture")).toBe(ids.indexOf("graph") + 1);
    expect(NAV_ITEMS.find((i) => i.id === "architecture")?.group).toBe("B");
  });

  it("keeps every id, label, route and scope the member sidebar always had (S-788)", () => {
    expect(NAV_ITEMS.map((i) => [i.id, i.label, i.path, i.scope]).sort()).toEqual(
      [
        ["overview", "Dashboard", "/", "member"],
        ["health", "Health", "/health", "member"],
        ["security", "Security", "/security", "member"],
        ["coverage", "Coverage", "/coverage", "member"],
        ["graph", "Graph", "/graph", "member"],
        ["architecture", "Architecture", "/architecture", "member"],
        ["gaps", "Rule findings", "/gaps", "member"],
        ["files", "Files & Risk", "/files", "member"],
        ["statistics", "Statistics", "/statistics", "member"],
        ["config", "Config", "/config", "member"],
      ].sort(),
    );
  });
});

describe("the workspace Coverage entry (S-788, CR-244)", () => {
  afterEach(() => {
    clearExtensions();
    UNREGISTERED.clear();
  });

  it("is not offered while no view owns its route", () => {
    UNREGISTERED.add("/workspace-coverage");
    expect(navItemsFor(true).some((i) => i.path === "/workspace-coverage")).toBe(false);
    expect(navItemsFor(true).filter((i) => i.scope === "app").map((i) => i.id)).toEqual([
      "workspace-dashboard",
      "workspace",
      "workspace-health",
      "workspace-security",
      "workspace-statistics",
      "workspace-config",
    ]);
  });

  it("is registered once, app-scoped, and offered once in a workspace only, now S-740 registers its view", () => {
    expect(WORKSPACE_NAV_ITEMS.filter((i) => i.id === "workspace-coverage")).toHaveLength(1);
    expect(WORKSPACE_NAV_ITEMS.filter((i) => i.label === "Coverage")).toHaveLength(1);
    expect(navItemsFor(true).filter((i) => i.path === "/workspace-coverage")).toHaveLength(1);
    expect(navItemsFor(false).some((i) => i.path === "/workspace-coverage")).toBe(false);
  });

  it("sits between Security and Statistics with its view registered", () => {
    // The built-in view S-740 registers (views/index.ts), not a mounted stand-in.
    // The literal workspace section: a SECOND Coverage entry — S-740 adding its own
    // beside this one — fails this roster.
    expect(navItemsFor(true).filter((i) => i.scope === "app").map((i) => [i.id, i.group])).toEqual([
      ["workspace-dashboard", "A"],
      ["workspace", "A"],
      ["workspace-health", "A"],
      ["workspace-security", "B"],
      ["workspace-coverage", "B"],
      ["workspace-statistics", "C"],
      ["workspace-config", "C"],
    ]);
    // Workspace-mode only: a single-root sidebar offers no workspace entry.
    expect(navItemsFor(false).some((i) => i.scope === "app")).toBe(false);
  });
});

describe("every offered entry has a registered view (S-788)", () => {
  it("resolves each entry offered in either mode to a view, built-in or mounted", () => {
    // The other half — an entry whose route owns no view is not offered — is "is
    // not offered while no view owns its route" above, which withdraws the Coverage
    // view (`UNREGISTERED`) and fails if the filter is dropped.
    for (const isWorkspace of [false, true]) {
      expect(navItemsFor(isWorkspace).length).toBeGreaterThan(0);
      for (const item of navItemsFor(isWorkspace)) {
        expect({ path: item.path, view: viewForPath(item.path) !== null }).toEqual({
          path: item.path,
          view: true,
        });
      }
    }
  });
});

describe("the Architecture tab (S-612, FR-UI-41)", () => {
  it("reads 'Architecture' at /architecture — the label names no hidden widget", () => {
    // The Cycles band and list are hidden through the register, so a label naming
    // them would point at nothing on the page (NFR-CC-04). The route is unchanged.
    const item = NAV_ITEMS.find((i) => i.id === "architecture");
    expect(item?.label).toBe("Architecture");
    expect(item?.path).toBe("/architecture");
    expect(ALL_ITEMS.some((i) => /cycles/i.test(i.label))).toBe(false);
  });

  it("is not offered in either mode while the register hides the view (CR-208)", () => {
    for (const isWorkspace of [false, true]) {
      expect(navItemsFor(isWorkspace).some((i) => i.path === "/architecture")).toBe(false);
    }
  });

  it("is offered again, in its registered place, once the register entry is removed (CR-208)", () => {
    const restore = removeHiddenWidgetEntry("architecture-view");
    try {
      expect(navItemsFor(false)).toEqual(NAV_ITEMS);
      expect(navItemsFor(true).filter((i) => i.scope === "member")).toEqual(NAV_ITEMS);
    } finally {
      restore();
    }
  });
});

describe("workspaceReplacementPath (S-485, ADR-71)", () => {
  it("replaces no route while no mounted entry declares a replacement", () => {
    expect(ALL_ITEMS.some((i) => i.replacedInWorkspaceBy !== undefined)).toBe(false);
    for (const item of ALL_ITEMS) {
      expect({ path: item.path, to: workspaceReplacementPath(item.path) }).toEqual({ path: item.path, to: null });
    }
  });
});

describe("workspaceReplacementPath with a mounted entry that declares one (S-485, ADR-71)", () => {
  beforeEach(mountReplacingStub);
  afterEach(clearExtensions);

  it("lands the replaced route — and its sub-routes — on the app entry it names", () => {
    expect(workspaceReplacementPath("/stub")).toBe("/stub-app");
    expect(workspaceReplacementPath("/stub/anything")).toBe("/stub-app");
  });

  it("leaves every other route where it is, near misses included", () => {
    for (const path of ["/", "/stubby", "/stub-app", "/workspace", "/health", "/reader/page/x"]) {
      expect({ path, to: workspaceReplacementPath(path) }).toEqual({ path, to: null });
    }
  });

  it("names a registered workspace view for every replacement it declares", () => {
    const offered = [...navItemsFor(false), ...navItemsFor(true)];
    const replaced = offered.filter((i) => i.replacedInWorkspaceBy !== undefined);
    expect(replaced.map((i) => [i.id, i.replacedInWorkspaceBy])).toEqual([["stub", "stub-app"]]);
    expect(NAV_ITEMS.some((i) => i.replacedInWorkspaceBy !== undefined)).toBe(false);
    for (const item of replaced) {
      const target = navItemsFor(true).find((w) => w.id === item.replacedInWorkspaceBy);
      expect(target?.scope).toBe("app");
      expect(workspaceReplacementPath(item.path)).toBe(target?.path);
    }
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
    expect(WORKSPACE_NAV_ITEMS.map((i) => [i.id, i.scope])).toEqual([
      ["workspace-dashboard", "app"],
      ["workspace", "app"],
      ["workspace-health", "app"],
      ["workspace-security", "app"],
      ["workspace-coverage", "app"],
      ["workspace-statistics", "app"],
      ["workspace-config", "app"],
    ]);
    // A mounted extension's entries declare theirs on the server (S-642).
    mountReplacingStub();
    try {
      expect(navItemsFor(true).filter((i) => i.id.startsWith("stub")).map((i) => [i.id, i.scope])).toEqual([
        ["stub-app", "app"],
      ]);
      expect(navItemsFor(false).filter((i) => i.id.startsWith("stub")).map((i) => [i.id, i.scope])).toEqual([
        ["stub", "member"],
      ]);
    } finally {
      clearExtensions();
    }
  });

  it("gives the two S-428 views the SAME labels as their member-scoped twins", () => {
    // ADR-66 accepted this deliberately: `/` stays the per-member Dashboard so no
    // bookmark is re-homed, and the two levels are told apart by the section
    // label the sidebar renders — never by renaming one of them.
    // Keyed off the ids that HAVE a member-scoped twin, not off "everything except
    // /workspace". The exclusion spelling silently swept in S-429's app-scoped
    // Statistics entry the moment it was registered, and would sweep in the next
    // one too — an expectation that grows a new obligation every time the list does
    // is asserting the list, not the property.
    const twinned = [
      "workspace-dashboard",
      "workspace-health",
      "workspace-security",
      "workspace-coverage",
      "workspace-statistics",
      "workspace-config",
    ];
    const appLabels = WORKSPACE_NAV_ITEMS.filter((i) => twinned.includes(i.id)).map((i) => i.label);
    expect(appLabels).toEqual(["Dashboard", "Health", "Security", "Coverage", "Statistics", "Config"]);
    for (const label of appLabels) {
      expect(NAV_ITEMS.map((i) => i.label)).toContain(label);
    }
  });

  it("files the app-scoped Statistics tab in its member-scoped twin's CR-042 group (S-429)", () => {
    // The group says WHAT a tab answers, and this one answers the same question as
    // the per-member Statistics tab, one scope up — so it is filed with it rather
    // than beside the three cross-service surfaces, which answer a different one.
    // Asserted as the equality rather than as the literal "C": the day CR-042 moves
    // Statistics to another group, the two must move together or the sidebar starts
    // grouping one question two ways.
    const member = NAV_ITEMS.find((i) => i.id === "statistics") as NavItem;
    const app = WORKSPACE_NAV_ITEMS.find((i) => i.id === "workspace-statistics") as NavItem;
    expect(member).toBeDefined();
    expect(app).toBeDefined();
    expect(app.group).toBe(member.group);
    // …and the three cross-service tabs are NOT in that group, which is what makes
    // the grouping visible in the rendered section at all.
    //
    // Keyed on the ids that ARE the cross-service surfaces, not on "everything except
    // workspace-statistics". The exclusion spelling is the same false obligation this
    // file fixes 25 lines above — it would fail the day a second group-C app entry is
    // registered, with nothing wrong.
    const crossService = WORKSPACE_NAV_ITEMS.filter((i) =>
      ["workspace-dashboard", "workspace-health", "workspace"].includes(i.id),
    );
    expect(crossService.length).toBeGreaterThan(0);
    for (const item of crossService) {
      expect(item.group).not.toBe(app.group);
    }
  });

  it("files the app-scoped Config tab in its member-scoped twin's group, after Statistics (S-430)", () => {
    // The same reasoning as the Statistics pair above, asserted as the equality:
    // the workspace Config editor answers "what is configured" one scope up, so it
    // sits with the policy editor it twins — and after the app Statistics tab, the
    // order the member section already uses.
    const member = NAV_ITEMS.find((i) => i.id === "config") as NavItem;
    const app = WORKSPACE_NAV_ITEMS.find((i) => i.id === "workspace-config") as NavItem;
    expect(member).toBeDefined();
    expect(app).toBeDefined();
    expect(app.group).toBe(member.group);
    const ids = WORKSPACE_NAV_ITEMS.map((i) => i.id);
    expect(ids.indexOf("workspace-config")).toBeGreaterThan(ids.indexOf("workspace-statistics"));
  });

  it("files the app-scoped Security and Coverage tabs in group B, after Health (S-674, S-788, FR-UI-55)", () => {
    // frontend-design §3 (CR-217, CR-244): the Workspace section reads Dashboard,
    // Workspace, Health | Security, Coverage | Statistics, Config. Unlike Statistics
    // and Config these two are NOT filed with their member twins (group A): FR-UI-55
    // files them in the Workspace section's group B, so the group is pinned literally.
    const group = (id: string) => WORKSPACE_NAV_ITEMS.find((i) => i.id === id)?.group;
    expect(group("workspace-security")).toBe("B");
    expect(group("workspace-coverage")).toBe("B");
    const ids = WORKSPACE_NAV_ITEMS.map((i) => i.id);
    expect(ids.indexOf("workspace-security")).toBe(ids.indexOf("workspace-health") + 1);
    expect(ids.indexOf("workspace-coverage")).toBe(ids.indexOf("workspace-security") + 1);
  });

  it("keeps every app-scoped route a SIBLING of /workspace, never a child of it", () => {
    // `navItemMatches` treats a path under a tab's route as belonging to that tab
    // — the rule that keeps a tab lit on the client sub-routes it owns. A route
    // under `/workspace` would therefore highlight TWO sidebar items at once.
    const workspaceTab = WORKSPACE_NAV_ITEMS.find((i) => i.id === "workspace");
    expect(workspaceTab).toBeDefined();
    for (const item of ALL_ITEMS) {
      if (item.id === "workspace") continue;
      expect(navItemMatches(workspaceTab as NavItem, item.path)).toBe(false);
    }
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
    // …and a member-scoped tab's sub-route stays member-scoped.
    expect(isAppLevelPath("/health/page/x")).toBe(false);
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
    expect(navItemMatches(READER_ITEM, "/readers")).toBe(false);
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

  // The modules that may spell an app-scoped route: `nav.ts` — the navigation
  // registry that DECLARES the scope; and `views/index.ts` — the component registry
  // that maps a built-in path to the React view it mounts. (An extension's
  // registration is the same component registry for the views IT mounts, keyed by
  // path by construction and holding no scope; this build composes none.) Any other
  // file is re-deriving the level from the route spelling, which ADR-66 §1 forbids.
  // `views/routes.ts` (S-788) is the component registry's route list without the
  // components, so `nav.ts` can ask whether a view owns a route without importing
  // stylesheets; it holds routes and no scope, and `views/index.ts` cannot compile
  // without exactly its routes.
  const ROUTE_REGISTRIES = ["nav.ts", "views/index.ts", "views/routes.ts"];

  it("has app-scoped routes to check at all", () => {
    // `it.each([])` registers ZERO tests and raises nothing, so without this floor
    // the assertion AC1 names by name would VANISH rather than fail the moment no
    // entry declared `app` — a guard that disables itself exactly when the field it
    // guards is what broke. Found in review.
    expect(appPaths.length).toBeGreaterThan(0);
  });

  it.each(appPaths)(
    "spells the app-scoped route %s in the route registries and nowhere else",
    (path) => {
      // Quote-agnostic, and matched as a PREFIX. An exact `"${path}"` search was
      // evaded three ways, all reproduced in review: `"/workspace/"` (a trailing
      // slash — the natural spelling of `startsWith`) does not contain the substring
      // `"/workspace"`; and `web/ui` configures neither ESLint nor Prettier, so
      // single quotes and template literals are not hypothetical.
      const probe = new RegExp(`["'\`]${path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`);
      const mentions = modules.filter((m) => probe.test(codeOf(SOURCES[m])));
      expect(mentions.length).toBeGreaterThan(0);
      expect(mentions.filter((m) => !ROUTE_REGISTRIES.includes(m))).toEqual([]);
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
    expect(navItemMatches(READER_ITEM, "/reader")).toBe(true);
    expect(navItemMatches(READER_ITEM, "/reader/page/x")).toBe(true);
    // A near miss: a sibling route that merely starts with the same characters.
    expect(navItemMatches(READER_ITEM, "/readers")).toBe(false);
    expect(navItemMatches(READER_ITEM, "/health")).toBe(false);
  });

  it("types the scope union closed", () => {
    // A compile-time assertion, kept next to its runtime twin: widening NavScope
    // without teaching the sidebar its label fails here rather than rendering
    // `undefined` as a section heading.
    const labelled: Record<NavScope, string> = NAV_SCOPE_LABELS;
    expect(Object.keys(labelled).sort()).toEqual([...NAV_SCOPES].sort());
  });
});

