import { afterEach, describe, expect, it } from "vitest";

import { CoverageView } from "./analytics/CoverageView.tsx";
import { ArchitectureView } from "./architecture/ArchitectureView.tsx";
import { DashboardView } from "./dashboard/DashboardView.tsx";
import { SecurityView } from "./security/SecurityView.tsx";
import { StatisticsView } from "./statistics/StatisticsView.tsx";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "../extensions/registry.ts";
import { extensionsInfo, STUB_UI, StubView, stubInfo } from "../test/extensions.tsx";
import { WorkspaceDashboardView } from "./workspace/WorkspaceDashboardView.tsx";
import { WorkspaceHealthView } from "./workspace/WorkspaceHealthView.tsx";
import { WorkspaceCoverageView } from "./workspace/WorkspaceCoverageView.tsx";
import { WorkspaceSecurityView } from "./workspace/WorkspaceSecurityView.tsx";
import { WorkspaceStatisticsView } from "./statistics/WorkspaceStatisticsView.tsx";
import { NAV_ITEMS, WORKSPACE_NAV_ITEMS } from "../nav.ts";
import { viewForPath, VIEW_REGISTRY } from "./index.ts";
import { BUILT_IN_VIEW_ROUTES, hasViewFor } from "./routes.ts";

afterEach(clearExtensions);

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

  it("registers the member-scoped Security view at /security, in group A (S-670, FR-SC-09, S-788)", () => {
    expect(VIEW_REGISTRY["/security"]).toBe(SecurityView);
    expect(viewForPath("/security")).toBe(SecurityView);
    expect(NAV_ITEMS.find((i) => i.path === "/security")).toMatchObject({ id: "security", group: "A", scope: "member" });
  });

  it("does NOT register /overview (that route is retired)", () => {
    expect(VIEW_REGISTRY["/overview"]).toBeUndefined();
  });

  it("registers the Architecture view at /architecture, and nothing at the retired /dsm (S-612)", () => {
    // `/dsm` resolves through the shell's redirect to `/architecture`, never as a
    // second registration of the same view.
    expect(VIEW_REGISTRY["/architecture"]).toBe(ArchitectureView);
    expect(VIEW_REGISTRY["/dsm"]).toBeUndefined();
  });
});

describe("viewForPath", () => {
  it("returns DashboardView for /", () => {
    expect(viewForPath("/")).toBe(DashboardView);
  });

  it("returns null for /overview (redirect handled separately)", () => {
    expect(viewForPath("/overview")).toBeNull();
  });

  it("matches a sub-route to its owning view — a mounted extension's included (S-642)", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(viewForPath("/stub/page/foo")).toBe(StubView);
    expect(viewForPath("/stub/search")).toBe(StubView);
    expect(viewForPath("/workspace/anything")).toBe(VIEW_REGISTRY["/workspace"]);
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

describe("the app-level workspace views (S-428 FR-UI-36, S-429 FR-UI-37)", () => {
  /** Every `app`-scoped route, read from the navigation registry rather than written
   *  out here. The two specs below used to enumerate S-428's pair by hand, and
   *  S-429's `/workspace-statistics` escaped both of them in silence — a hand-kept
   *  roster in a test is a roster that stops covering the surface the moment the
   *  surface grows. Derived, it cannot. */
  //
  // Every one: S-788's `workspace-coverage` entry resolves since S-740 registered
  // its view.
  const appRoutes = WORKSPACE_NAV_ITEMS.filter((i) => i.scope === "app").map((i) => i.path);

  it("has app-scoped routes to check at all", () => {
    // `it.each([])` registers zero tests and raises nothing, so without this floor
    // the two specs below would VANISH rather than fail if the filter ever matched
    // nothing.
    expect(appRoutes.length).toBeGreaterThan(0);
  });

  it.each(appRoutes)(
    "registers %s so a hand-typed URL resolves to the honest not-a-workspace state",
    (route) => {
      expect(VIEW_REGISTRY[route]).toBeDefined();
    },
  );

  it.each(appRoutes.filter((r) => r !== "/workspace"))(
    "does NOT let /workspace shadow %s",
    (route) => {
      // They are siblings of `/workspace`, not children: `viewForPath` resolves a
      // sub-path to its longest registered prefix, so a route UNDER `/workspace`
      // would have been claimable by the Workspace tab — and would then render the
      // service map under another tab's name.
      expect(viewForPath(route)).toBe(VIEW_REGISTRY[route]);
      expect(viewForPath(route)).not.toBe(VIEW_REGISTRY["/workspace"]);
    },
  );

  it("maps each route to the component it is NAMED for, not merely to some view", () => {
    // The derived specs above prove every app route resolves; they cannot prove it
    // resolves to the RIGHT view, because a registry that mapped all three to one
    // component would satisfy them. So the identities are pinned by name.
    expect(VIEW_REGISTRY["/workspace-dashboard"]).toBe(WorkspaceDashboardView);
    expect(VIEW_REGISTRY["/workspace-health"]).toBe(WorkspaceHealthView);
    expect(VIEW_REGISTRY["/workspace-statistics"]).toBe(WorkspaceStatisticsView);
    // S-674: the app-level Security roll-up, never the member-scoped Security page.
    expect(VIEW_REGISTRY["/workspace-security"]).toBe(WorkspaceSecurityView);
    expect(VIEW_REGISTRY["/workspace-security"]).not.toBe(SecurityView);
    // S-740: the app-level Coverage roll-up, never the member-scoped Coverage page.
    expect(VIEW_REGISTRY["/workspace-coverage"]).toBe(WorkspaceCoverageView);
    expect(VIEW_REGISTRY["/workspace-coverage"]).not.toBe(CoverageView);
    // …and none of them is the member-scoped Statistics view, which stays at
    // `/statistics` (FR-UI-37: the member-scoped view is unchanged).
    expect(VIEW_REGISTRY["/statistics"]).toBe(StatisticsView);
    expect(VIEW_REGISTRY["/workspace-statistics"]).not.toBe(StatisticsView);
  });
});

describe("the component-free route list (S-788)", () => {
  it("names exactly the routes VIEW_REGISTRY maps — `tsc -b` guards it, this guards a stripped type", () => {
    expect([...BUILT_IN_VIEW_ROUTES].sort()).toEqual(Object.keys(VIEW_REGISTRY).sort());
  });

  it("answers as viewForPath does, near misses included", () => {
    for (const path of [
      "/",
      "/health",
      "/health/sub/page",
      "/workspace",
      "/workspace-security",
      "/workspace-coverage",
      "/workspace-coveragex",
      "/healthz",
      "/overview",
      "/stub",
      "/stub/page",
    ]) {
      expect({ path, owned: hasViewFor(path) }).toEqual({ path, owned: viewForPath(path) !== null });
    }
    expect(hasViewFor("/workspace-coverage")).toBe(true);
    expect(hasViewFor("/workspace-coveragex")).toBe(false);
    expect(hasViewFor("/healthz")).toBe(false);
  });

  it("counts a mounted extension's route as owned, exactly or as a sub-route", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(hasViewFor("/stub")).toBe(true);
    expect(hasViewFor("/stub/page")).toBe(true);
    expect(hasViewFor("/stubby")).toBe(false);
  });
});
