/*
 * The navigation the mounted extensions add (S-642, FR-UI-51): composed into
 * `navItemsFor`, `scopeForPath` and `workspaceReplacementPath` from the registry at
 * call time, with the built-in lists untouched. The empty case — nothing mounted —
 * is the committed surface snapshot's (`nav.snapshot.test.ts`).
 */

import { afterEach, describe, expect, it } from "vitest";

import type { ExtensionNavEntry } from "./api/extensionsClient.ts";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "./extensions/registry.ts";
import {
  isAppLevelPath,
  NAV_ITEMS,
  navItemsFor,
  scopeForPath,
  WORKSPACE_NAV_ITEMS,
  workspaceReplacementPath,
} from "./nav.ts";
import { extensionsInfo, STUB_ID, StubAppView, StubView, stubInfo } from "./test/extensions.tsx";

afterEach(clearExtensions);

const MEMBER: ExtensionNavEntry = { id: "stub", label: "Stub", path: "/stub", group: "A", scope: "member" };
const APP: ExtensionNavEntry = { id: "stub-app", label: "Stub app", path: "/stub-app", group: "A", scope: "app" };

/** A second extension, registered ahead of the stub: a member entry it places after
 *  Graph and replaces in a workspace by its own app entry. */
const OTHER_MEMBER: ExtensionNavEntry = {
  id: "other",
  label: "Other",
  path: "/other",
  group: "A",
  scope: "member",
  replaced_in_workspace_by: "other-app",
};
const OTHER_APP: ExtensionNavEntry = { id: "other-app", label: "Other app", path: "/other-app", group: "A", scope: "app" };

/** Mount the stub with exactly `nav` as its sidebar entries (a view registered for each
 *  path), placed by `navAfter` when given — beside the second extension when `withOther`. */
function mountStub(...nav: ExtensionNavEntry[]): void {
  mountStubPlaced({}, false, ...nav);
}

function mountStubPlaced(navAfter: Record<string, string>, withOther: boolean, ...nav: ExtensionNavEntry[]): void {
  if (withOther) {
    registerExtensionUi({
      id: "other",
      views: { "/other": StubView, "/other-app": StubAppView },
      navAfter: { other: "graph" },
    });
  }
  registerExtensionUi({
    id: STUB_ID,
    views: Object.fromEntries(nav.map((n) => [n.path, n.scope === "app" ? StubAppView : StubView])),
    navAfter,
  });
  const stub = stubInfo({ ui: { nav, views: [], config_panels: [] } });
  const other = stubInfo({ id: "other", ui: { nav: [OTHER_MEMBER, OTHER_APP], views: [], config_panels: [] } });
  setActiveExtensions(extensionsInfo(...(withOther ? [other] : []), stub));
}

describe("a mounted extension's navigation", () => {
  it("is appended after the built-in entries in both modes when member-scoped", () => {
    mountStub(MEMBER);
    const stub = { ...MEMBER, group: "A", scope: "member" };
    expect(navItemsFor(false)).toEqual([...NAV_ITEMS.filter((i) => i.id !== "architecture"), stub]);
    // In a workspace it sits with the member entries, ahead of the workspace-only ones.
    const ids = navItemsFor(true).map((i) => i.id);
    expect(ids.indexOf("stub")).toBeGreaterThan(ids.indexOf("config"));
    expect(ids.indexOf("stub")).toBeLessThan(ids.indexOf(WORKSPACE_NAV_ITEMS[0].id));
  });

  it("is workspace-only when app-scoped, like WORKSPACE_NAV_ITEMS", () => {
    mountStub(APP);
    expect(navItemsFor(false).some((i) => i.id === "stub-app")).toBe(false);
    expect(navItemsFor(true).at(-1)).toEqual(APP);
    expect(navItemsFor(true).slice(0, -1).filter((i) => WORKSPACE_NAV_ITEMS.includes(i))).toHaveLength(
      WORKSPACE_NAV_ITEMS.length,
    );
  });

  it("declares its scope for the shell's remount rule", () => {
    mountStub(MEMBER, APP);
    expect(scopeForPath("/stub")).toBe("member");
    expect(scopeForPath("/stub/sub")).toBe("member");
    expect(scopeForPath("/stub-app")).toBe("app");
    expect(isAppLevelPath("/stub-app")).toBe(true);
    expect(isAppLevelPath("/stub")).toBe(false);
  });

  it("is absent again once nothing is mounted", () => {
    mountStub(MEMBER, APP);
    clearExtensions();
    expect(navItemsFor(false)).toEqual(NAV_ITEMS.filter((i) => i.id !== "architecture"));
    expect(isAppLevelPath("/stub-app")).toBe(false);
  });
});

/** Mount two extensions, `a` then `b`, each with the given entries (a view per path). */
function mountTwo(a: ExtensionNavEntry[], b: ExtensionNavEntry[]): void {
  for (const [id, nav] of [["a", a], ["b", b]] as const) {
    registerExtensionUi({ id, views: Object.fromEntries(nav.map((n) => [n.path, StubView])) });
  }
  setActiveExtensions(
    extensionsInfo(
      stubInfo({ id: "a", ui: { nav: a, views: [], config_panels: [] } }),
      stubInfo({ id: "b", ui: { nav: b, views: [], config_panels: [] } }),
    ),
  );
}

describe("a mounted extension's replacement in workspace mode", () => {
  it("accepts a replacement by a BUILT-IN app entry", () => {
    mountStub({ ...MEMBER, replaced_in_workspace_by: "workspace-health" });
    expect(navItemsFor(true).some((i) => i.id === "stub")).toBe(false);
    expect(workspaceReplacementPath("/stub")).toBe("/workspace-health");
  });

  it("clears a replacement that names a MEMBER-scoped entry — only an app entry can replace", () => {
    mountTwo(
      [{ ...MEMBER, id: "a", path: "/a", replaced_in_workspace_by: "b" }],
      [{ ...MEMBER, id: "b", path: "/b" }],
    );
    expect(() => workspaceReplacementPath("/a")).not.toThrow();
    expect(workspaceReplacementPath("/a")).toBeNull();
    expect(navItemsFor(true).map((i) => i.id)).toEqual(expect.arrayContaining(["a", "b"]));
  });

  it("drops the member entry in a workspace and sends its route to the app entry", () => {
    mountStub({ ...MEMBER, replaced_in_workspace_by: "stub-app" }, APP);
    expect(navItemsFor(false).some((i) => i.id === "stub")).toBe(true);
    expect(navItemsFor(true).some((i) => i.id === "stub")).toBe(false);
    expect(navItemsFor(true).some((i) => i.id === "stub-app")).toBe(true);
    expect(workspaceReplacementPath("/stub")).toBe("/stub-app");
    expect(workspaceReplacementPath("/stub/sub")).toBe("/stub-app");
    expect(workspaceReplacementPath("/stub-app")).toBeNull();
  });

  it("keeps another extension's replacement beside it", () => {
    mountStubPlaced({}, true, { ...MEMBER, replaced_in_workspace_by: "stub-app" }, APP);
    expect(workspaceReplacementPath("/other")).toBe("/other-app");
    expect(workspaceReplacementPath("/stub")).toBe("/stub-app");
  });

  it("clears a replacement that names no offered app entry — nothing is dropped for nothing", () => {
    mountStub({ ...MEMBER, replaced_in_workspace_by: "no-such-entry" });
    expect(navItemsFor(true).some((i) => i.id === "stub")).toBe(true);
    expect(workspaceReplacementPath("/stub")).toBeNull();
  });
});

describe("a navigation entry the shell cannot place", () => {
  it("is skipped when its group or scope is not one the shell knows", () => {
    mountStub(
      { ...MEMBER, id: "bad-group", path: "/stub", group: "D" },
      { ...MEMBER, id: "bad-scope", path: "/stub-app", scope: "global" },
    );
    expect(navItemsFor(false)).toEqual(NAV_ITEMS.filter((i) => i.id !== "architecture"));
    expect(navItemsFor(true).some((i) => i.id.startsWith("bad-"))).toBe(false);
  });

  it("is placed in every group and scope the shell knows", () => {
    mountStub({ ...MEMBER, group: "C" }, { ...APP, group: "B" });
    expect(navItemsFor(true).filter((i) => i.id.startsWith("stub")).map((i) => [i.id, i.group, i.scope])).toEqual([
      ["stub", "C", "member"],
      ["stub-app", "B", "app"],
    ]);
  });
});

describe("a built-in entry always wins a clash", () => {
  it("drops an extension entry that reuses a built-in id", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView } });
    setActiveExtensions(
      extensionsInfo(stubInfo({ ui: { nav: [{ ...MEMBER, id: "health" }], views: [], config_panels: [] } })),
    );
    expect(navItemsFor(false)).toEqual(NAV_ITEMS.filter((i) => i.id !== "architecture"));
  });

  it("drops an extension entry that reuses a built-in route", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/health": StubView } });
    setActiveExtensions(
      extensionsInfo(
        stubInfo({ ui: { nav: [{ ...MEMBER, path: "/health" }], views: [], config_panels: [] } }),
      ),
    );
    expect(navItemsFor(false)).toEqual(NAV_ITEMS.filter((i) => i.id !== "architecture"));
    expect(scopeForPath("/health")).toBe("member");
  });

  it("drops an extension entry that reuses a WORKSPACE built-in route", () => {
    registerExtensionUi({ id: STUB_ID, views: { [WORKSPACE_NAV_ITEMS[0].path]: StubView } });
    setActiveExtensions(
      extensionsInfo(
        stubInfo({
          ui: { nav: [{ ...APP, path: WORKSPACE_NAV_ITEMS[0].path }], views: [], config_panels: [] },
        }),
      ),
    );
    expect(navItemsFor(true).filter((i) => i.path === WORKSPACE_NAV_ITEMS[0].path)).toHaveLength(1);
  });

  it("drops an extension entry that reuses a WORKSPACE built-in id under a free route", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/free": StubView } });
    setActiveExtensions(
      extensionsInfo(
        stubInfo({
          ui: { nav: [{ ...APP, id: WORKSPACE_NAV_ITEMS[0].id, path: "/free" }], views: [], config_panels: [] },
        }),
      ),
    );
    expect(navItemsFor(true).filter((i) => i.id === WORKSPACE_NAV_ITEMS[0].id)).toHaveLength(1);
    expect(navItemsFor(true).some((i) => i.path === "/free")).toBe(false);
  });

  it("keeps the first of two extensions' entries on one route, whatever their ids", () => {
    mountTwo(
      [{ ...MEMBER, id: "a", path: "/shared" }],
      [{ ...MEMBER, id: "b", path: "/shared" }],
    );
    expect(navItemsFor(false).filter((i) => i.path === "/shared").map((i) => i.id)).toEqual(["a"]);
  });

  it("lists the entries of several extensions in the server's order", () => {
    mountTwo([{ ...MEMBER, id: "a", path: "/a" }], [{ ...MEMBER, id: "b", path: "/b" }]);
    expect(navItemsFor(false).slice(-2).map((i) => i.id)).toEqual(["a", "b"]);
  });

  it("keeps the first of two extension entries that claim one id", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView, "/other": StubView } });
    setActiveExtensions(
      extensionsInfo(
        stubInfo({
          ui: { nav: [MEMBER, { ...MEMBER, label: "Second", path: "/other" }], views: [], config_panels: [] },
        }),
      ),
    );
    expect(navItemsFor(false).filter((i) => i.id === "stub").map((i) => i.label)).toEqual(["Stub"]);
  });
});

describe("an entry its extension places (navAfter, S-643)", () => {
  it("sits directly after its anchor in its own section, in both modes", () => {
    mountStubPlaced({ stub: "health", "stub-app": "workspace-dashboard" }, false, MEMBER, APP);
    expect(navItemsFor(false).map((i) => i.id).slice(0, 4)).toEqual(["overview", "health", "stub", "graph"]);
    const app = navItemsFor(true).filter((i) => i.scope === "app").map((i) => i.id);
    expect(app.slice(0, 3)).toEqual(["workspace-dashboard", "stub-app", "workspace-health"]);
  });

  it("follows an entry another extension placed after the same anchor, in registration order", () => {
    // The other extension is anchored after Graph; a later extension anchored there
    // too follows it, so the two keep the order the composition registered them in.
    mountStubPlaced({ stub: "graph" }, true, MEMBER);
    expect(navItemsFor(false).map((i) => i.id).slice(2, 6)).toEqual(["graph", "other", "stub", "files"]);
  });

  it("is appended when its anchor is not offered — never placed into another section", () => {
    // `workspace` is an app entry: a member entry anchored to it is not moved across
    // sections, and an anchor nothing offers places nothing.
    mountStubPlaced({ stub: "workspace", "stub-app": "no-such-entry" }, false, MEMBER, APP);
    expect(navItemsFor(false).at(-1)?.id).toBe("stub");
    expect(navItemsFor(true).at(-1)?.id).toBe("stub-app");
    expect(navItemsFor(true).filter((i) => i.scope === "member").at(-1)?.id).toBe("stub");
  });
});
