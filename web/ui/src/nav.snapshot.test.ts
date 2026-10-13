/*
 * The SPA navigation, frozen as committed snapshots (S-639, CR-213, FR-DS-04,
 * FR-DS-05).
 *
 * These snapshots were taken against the navigation BEFORE the extension host
 * existed — single-root and workspace — and every later change kept them equal.
 * Since the open-source build composes no extension (S-651) they are that baseline
 * less exactly its chat and wiki entries, removed by hand in that reviewed commit and
 * nothing else: the sidebar this build serves. The server-side surfaces (route
 * census, MCP roster, CLI help, config read-model) are frozen in
 * `cli/tests/surface_snapshots.rs`.
 *
 * They are a baseline, not a convenience: there is no refresh mode. Changing one
 * means editing the JSON in a reviewed commit.
 */
import { afterEach, describe, expect, it } from "vitest";

import registrations from "virtual:logos-extensions";

import type { ExtensionsInfo } from "./api/extensionsClient.ts";
import noExtension from "./extensions/fixtures/no-extension.real.json";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "./extensions/registry.ts";
import { navItemsFor } from "./nav.ts";
import singleRoot from "./test/surface/nav-single-root.json?raw";
import workspace from "./test/surface/nav-workspace.json?raw";

afterEach(clearExtensions);

describe("the SPA navigation snapshot (S-639)", () => {
  // Booted the way this build boots (S-645): the shipped bundle registers what its
  // composed entry carries, and the server answers with the body this build actually
  // serves (`no-extension.real.json`, generated and guarded by
  // `cli/tests/no_extension_surface.rs`).
  it("offers exactly the snapshotted sidebars, single-root and workspace", () => {
    registrations.forEach(registerExtensionUi);
    setActiveExtensions(noExtension as ExtensionsInfo);
    expect(navItemsFor(false)).toEqual(JSON.parse(singleRoot));
    expect(navItemsFor(true)).toEqual(JSON.parse(workspace));
  });

  // Nothing stands in place of chat and the wiki (FR-DS-05): no entry in either mode
  // names them.
  it("names no chat or wiki entry in either mode", () => {
    registrations.forEach(registerExtensionUi);
    setActiveExtensions(noExtension as ExtensionsInfo);
    for (const isWorkspace of [false, true]) {
      const named = navItemsFor(isWorkspace).filter((i) => /chat|wiki/i.test(`${i.id} ${i.label} ${i.path}`));
      expect(named).toEqual([]);
    }
    for (const snapshot of [singleRoot, workspace]) {
      expect(snapshot).not.toMatch(/chat|wiki/i);
    }
  });
});
