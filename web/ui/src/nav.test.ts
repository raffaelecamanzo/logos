import { describe, expect, it } from "vitest";

import { isAppLevelPath, NAV_ITEMS, navItemsFor, WORKSPACE_NAV_ITEMS } from "./nav.ts";

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

describe("isAppLevelPath (S-250)", () => {
  it("marks the workspace routes app-level, so the shell never re-keys them per member", () => {
    // Its reads are the unscoped `workspace/*` fan-out: identical for every member, so
    // remounting it on a member switch would tear down the canvas for no new data.
    expect(isAppLevelPath("/workspace")).toBe(true);
    expect(isAppLevelPath("/workspace/anything")).toBe(true);
  });

  it("leaves every member-scoped view keyed on the member", () => {
    for (const path of ["/", "/health", "/graph", "/coverage", "/config"]) {
      expect(isAppLevelPath(path)).toBe(false);
    }
  });
});
