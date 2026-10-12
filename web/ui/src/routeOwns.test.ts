import { describe, expect, it } from "vitest";

import { routeOwns } from "./routeOwns.ts";

describe("routeOwns (S-642) — the SPA's one route-ownership rule", () => {
  it("owns the route itself and the whole-segment sub-routes under it", () => {
    expect(routeOwns("/wiki", "/wiki")).toBe(true);
    expect(routeOwns("/wiki", "/wiki/page/x")).toBe(true);
  });

  it("does not own a sibling that merely starts with the same characters, or a parent", () => {
    expect(routeOwns("/wiki", "/wikipedia")).toBe(false);
    expect(routeOwns("/wiki", "/wik")).toBe(false);
    expect(routeOwns("/wiki/page", "/wiki")).toBe(false);
  });

  it("lets the root own only itself — never every route in the SPA", () => {
    expect(routeOwns("/", "/")).toBe(true);
    expect(routeOwns("/", "/health")).toBe(false);
    expect(routeOwns("/", "//")).toBe(false);
  });
});
