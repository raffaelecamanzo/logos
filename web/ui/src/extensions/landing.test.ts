/*
 * `extensionDecisionPending` (S-642, FR-UI-51): only a route an extension owns and no
 * built-in view serves waits for the boot read.
 */

import { afterEach, describe, expect, it } from "vitest";

import { extensionsInfo, STUB_ID, StubView, stubInfo } from "../test/extensions.tsx";
import { extensionDecisionPending, inactiveExtensionLanding } from "./landing.ts";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "./registry.ts";

afterEach(clearExtensions);

describe("extensionDecisionPending", () => {
  it("holds an extension's own route, and its sub-routes, while the read is in flight", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView } });
    expect(extensionDecisionPending("/stub")).toBe(true);
    expect(extensionDecisionPending("/stub/page/x")).toBe(true);
    expect(extensionDecisionPending("/stubby")).toBe(false);
    // A deep link is held, not bounced to the Dashboard, while the read is in flight.
    expect(inactiveExtensionLanding("/stub/page/x")).toBeNull();
  });

  it("never holds a route a built-in view serves, even one an extension registered over", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/health": StubView, "/stub": StubView } });
    expect(extensionDecisionPending("/health")).toBe(false);
    expect(extensionDecisionPending("/stub")).toBe(true);
  });

  it("stops holding once the read settles, and then lands an unmounted route on the Dashboard", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView } });
    setActiveExtensions(extensionsInfo());
    expect(extensionDecisionPending("/stub")).toBe(false);
    expect(inactiveExtensionLanding("/stub")).toBe("/");
    expect(inactiveExtensionLanding("/health")).toBeNull();
  });

  it("does not land a mounted extension's route", () => {
    registerExtensionUi({ id: STUB_ID, views: { "/stub": StubView } });
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(inactiveExtensionLanding("/stub")).toBeNull();
  });
});
