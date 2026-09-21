import { describe, expect, it } from "vitest";

import { DashboardView } from "./dashboard/DashboardView.tsx";
import { StatisticsView } from "./statistics/StatisticsView.tsx";
import { WikiView } from "./wiki/WikiView.tsx";
import { WorkspaceDashboardView } from "./workspace/WorkspaceDashboardView.tsx";
import { WorkspaceHealthView } from "./workspace/WorkspaceHealthView.tsx";
import { NAV_ITEMS, WORKSPACE_NAV_ITEMS } from "../nav.ts";
import { viewForPath, VIEW_REGISTRY } from "./index.ts";

describe("VIEW_REGISTRY", () => {
  it("registers the dashboard at the root route", () => {
    expect(VIEW_REGISTRY["/"]).toBe(DashboardView);
  });

  it("registers no view the navigation registry has not scoped (S-425, ADR-66 §2)", () => {
    // The shell mounts `viewForPath(pathname)` while keying the mount on
    // `isAppLevelPath(pathname)` — two registries, read by one component. A route
    // present HERE but absent from `nav.ts` resolves to a real view and to
    // `scopeForPath`'s unregistered-path fallback of "member": the silent wrong
    // default ADR-66 §2 exists to make impossible, displaced one level. Found in
    // review, while the two registries still agreed by coincidence; three later
    // stories add entries to both by hand, which is when a coincidence stops
    // holding.
    const scoped = new Set([...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS].map((i) => i.path));
    expect(Object.keys(VIEW_REGISTRY).length).toBeGreaterThan(0);
    expect(Object.keys(VIEW_REGISTRY).filter((path) => !scoped.has(path))).toEqual([]);
  });

  it("registers the Statistics view at /statistics (S-235)", () => {
    expect(VIEW_REGISTRY["/statistics"]).toBe(StatisticsView);
  });

  it("does NOT register /overview (that route is retired)", () => {
    expect(VIEW_REGISTRY["/overview"]).toBeUndefined();
  });
});

describe("viewForPath", () => {
  it("returns DashboardView for /", () => {
    expect(viewForPath("/")).toBe(DashboardView);
  });

  it("returns null for /overview (redirect handled separately)", () => {
    expect(viewForPath("/overview")).toBeNull();
  });

  it("matches a sub-route to its owning view (wiki sub-routes)", () => {
    expect(viewForPath("/wiki/page/foo")).toBe(WikiView);
    expect(viewForPath("/wiki/search")).toBe(WikiView);
  });

  it("does NOT treat / as a prefix for all paths", () => {
    // The root registration must not shadow every other route.
    // viewForPath("/health") must return HealthView, not DashboardView.
    const result = viewForPath("/health");
    expect(result).not.toBeNull();
    expect(result).not.toBe(DashboardView);
  });
});

describe("the workspace route (S-250, FR-UI-29)", () => {
  it("registers /workspace so a hand-typed URL resolves (its NAV item is workspace-only)", () => {
    expect(viewForPath("/workspace")).toBe(VIEW_REGISTRY["/workspace"]);
    expect(VIEW_REGISTRY["/workspace"]).toBeDefined();
  });
});

describe("the app-level workspace views (S-428, FR-UI-36)", () => {
  it("registers both so a hand-typed URL resolves to the honest not-a-workspace state", () => {
    expect(VIEW_REGISTRY["/workspace-dashboard"]).toBe(WorkspaceDashboardView);
    expect(VIEW_REGISTRY["/workspace-health"]).toBe(WorkspaceHealthView);
  });

  it("does NOT let /workspace shadow either of them", () => {
    // They are siblings of `/workspace`, not children: `viewForPath` resolves a
    // sub-path to its longest registered prefix, so a route UNDER `/workspace`
    // would have been claimable by the Workspace tab.
    expect(viewForPath("/workspace-dashboard")).toBe(WorkspaceDashboardView);
    expect(viewForPath("/workspace-health")).toBe(WorkspaceHealthView);
    expect(viewForPath("/workspace")).not.toBe(WorkspaceDashboardView);
  });
});
