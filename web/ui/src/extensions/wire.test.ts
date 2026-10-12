/*
 * The server's half and the SPA's half of the extension manifest meet here (S-641 →
 * S-642, FR-UI-51). `fixtures/extensions.real.json` is the body `GET /api/v1/extensions`
 * actually serialises — `web/tests/extensions_endpoint.rs` is its generator and its
 * guard — and this suite mounts it through the real registry and `nav.ts`. The wire
 * types in `api/extensionsClient.ts` are hand-written, so without a body both sides read,
 * a field renamed on either side would leave every other test green while the SPA
 * silently mounted nothing.
 */

import { afterEach, describe, expect, it } from "vitest";

import type { ExtensionsInfo } from "../api/extensionsClient.ts";
import { navItemsFor, scopeForPath, workspaceReplacementPath } from "../nav.ts";
import served from "./fixtures/extensions.real.json";
import {
  clearExtensions,
  getExtensions,
  mountedConfigPanels,
  mountedIcon,
  mountedViews,
  registerExtensionUi,
  setActiveExtensions,
} from "./registry.ts";

afterEach(clearExtensions);

const View = () => null;
const Icon = () => null;
const Panel = () => null;

/** The code the probe's bundle would register, keyed by the ids and routes the
 *  served manifest uses — the only join between the two halves. */
function registerProbe(): void {
  registerExtensionUi({
    id: "wire-probe",
    icons: { "wire-probe": Icon, "wire-probe-workspace": Icon },
    views: { "/wire-probe": View, "/wire-probe-workspace": View, "/wire-probe-detail": View },
    configPanels: { "wire-probe-config": { member: Panel, workspace: Panel } },
  });
}

describe("the served GET /api/v1/extensions body", () => {
  it("is the extension list and nothing else", () => {
    expect(Object.keys(served)).toEqual(["extensions"]);
  });

  it("mounts every navigation entry, view, icon and config panel it lists", () => {
    registerProbe();
    setActiveExtensions(served as ExtensionsInfo);

    expect(getExtensions().mounted.map((m) => m.id)).toEqual(["wire-probe"]);
    expect(mountedViews().map(([route]) => route)).toEqual([
      "/wire-probe",
      "/wire-probe-workspace",
      "/wire-probe-detail",
    ]);
    expect(mountedIcon("wire-probe")).toBe(Icon);
    expect(mountedConfigPanels("member").map((p) => p.panel)).toEqual([
      { id: "wire-probe-config", label: "Wire probe", table: "wire_probe" },
    ]);
    expect(mountedConfigPanels("workspace")).toHaveLength(1);
  });

  it("speaks the group, scope and replacement vocabulary nav.ts narrows to", () => {
    registerProbe();
    setActiveExtensions(served as ExtensionsInfo);

    const single = navItemsFor(false);
    expect(single.find((i) => i.id === "wire-probe")).toEqual({
      id: "wire-probe",
      label: "Wire probe",
      path: "/wire-probe",
      group: "B",
      scope: "member",
      replacedInWorkspaceBy: "wire-probe-workspace",
    });
    expect(single.some((i) => i.id === "wire-probe-workspace")).toBe(false);

    const workspace = navItemsFor(true).map((i) => i.id);
    expect(workspace).toContain("wire-probe-workspace");
    expect(workspace).not.toContain("wire-probe");
    expect(workspaceReplacementPath("/wire-probe")).toBe("/wire-probe-workspace");
    expect(scopeForPath("/wire-probe-workspace")).toBe("app");
  });

  it("lists an active extension without a manifest, which mounts nothing", () => {
    expect(served.extensions.map((e) => e.id)).toContain("wire-headless");
    expect(served.extensions.find((e) => e.id === "wire-headless")?.ui).toBeNull();
  });
});
