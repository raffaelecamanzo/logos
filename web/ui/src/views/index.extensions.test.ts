/*
 * `viewForPath` over the mounted extensions' views (S-642, FR-UI-51): they resolve
 * under the same rules as the built-in ones, and a built-in route always wins.
 */

import { afterEach, describe, expect, it } from "vitest";

import { clearExtensions, registerExtensionUi, setActiveExtensions } from "../extensions/registry.ts";
import { extensionsInfo, STUB_ID, STUB_NAV, StubAppView, StubView, stubInfo } from "../test/extensions.tsx";
import { HealthView } from "./health/HealthView.tsx";
import { viewForPath } from "./index.ts";

afterEach(clearExtensions);

describe("viewForPath over the mounted extensions", () => {
  it("resolves a mounted extension's route, and its sub-routes by the longest match", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView, "/stub/deep": StubAppView } });
    setActiveExtensions(
      extensionsInfo(stubInfo({ ui: { nav: [STUB_NAV], views: ["/stub/deep"], config_panels: [] } })),
    );
    expect(viewForPath("/stub")).toBe(StubView);
    expect(viewForPath("/stub/page/x")).toBe(StubView);
    expect(viewForPath("/stub/deep/x")).toBe(StubAppView);
  });

  it("gives a route two extensions both list to the first-listed extension's view", () => {
    registerExtensionUi({ id: "a", views: { "/shared": StubView } });
    registerExtensionUi({ id: "b", views: { "/shared": StubAppView } });
    const ui = { nav: [], views: ["/shared"], config_panels: [] };
    setActiveExtensions(extensionsInfo(stubInfo({ id: "a", ui }), stubInfo({ id: "b", ui })));
    expect(viewForPath("/shared")).toBe(StubView);
    expect(viewForPath("/shared/sub")).toBe(StubView);
  });

  it("resolves nothing for a registered route the server has not mounted", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView } });
    expect(viewForPath("/stub")).toBeNull();
    setActiveExtensions(extensionsInfo());
    expect(viewForPath("/stub")).toBeNull();
    expect(viewForPath("/stub/sub")).toBeNull();
  });

  it("never lets a mounted extension shadow a built-in route, exact or as a sub-route", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/health": StubView, "/health/deeper": StubAppView } });
    setActiveExtensions(
      extensionsInfo(stubInfo({ ui: { nav: [], views: ["/health", "/health/deeper"], config_panels: [] } })),
    );
    expect(viewForPath("/health")).toBe(HealthView);
    // The longer extension route is a different route and still resolves.
    expect(viewForPath("/health/deeper")).toBe(StubAppView);
  });
});
