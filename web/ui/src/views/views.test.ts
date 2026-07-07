import { describe, expect, it } from "vitest";

import { DashboardView } from "./dashboard/DashboardView.tsx";
import { StatisticsView } from "./statistics/StatisticsView.tsx";
import { WikiView } from "./wiki/WikiView.tsx";
import { viewForPath, VIEW_REGISTRY } from "./index.ts";

describe("VIEW_REGISTRY", () => {
  it("registers the dashboard at the root route", () => {
    expect(VIEW_REGISTRY["/"]).toBe(DashboardView);
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
