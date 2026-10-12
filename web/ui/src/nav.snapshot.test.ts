/*
 * The SPA navigation, frozen as committed snapshots (S-639, CR-213, FR-DS-04).
 *
 * CR-213 moves chat and the wiki behind an extension host without changing what
 * any shipped build shows. These snapshots were taken against the navigation
 * BEFORE the host existed — single-root and workspace — and every later change
 * must keep them equal. The SPA bundle is one artifact embedded unchanged in
 * both the default and `agents` builds, so one pair covers both. The server-side
 * surfaces (route census, MCP roster, CLI help, config read-model) are frozen in
 * `cli/tests/surface_snapshots.rs`.
 *
 * S-645 asserts these very files after chat and the wiki have moved, so they are
 * a baseline, not a convenience: there is no refresh mode. Changing one means
 * editing the JSON in a reviewed commit.
 */
import { afterEach, describe, expect, it } from "vitest";

import registrations from "virtual:logos-extensions";

import type { ExtensionsInfo } from "./api/extensionsClient.ts";
import noExtension from "./extensions/fixtures/no-extension.real.json";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "./extensions/registry.ts";
import { navItemsFor, type NavItem } from "./nav.ts";
import singleRoot from "./test/surface/nav-single-root.json?raw";
import workspace from "./test/surface/nav-workspace.json?raw";
import { mountAgentsBuild, mountWiki } from "./test/wiki.ts";

afterEach(clearExtensions);

/** The chat extension's two entries (S-643): the only ones it contributes. */
const CHAT_IDS = ["chat", "workspace-chat"];
/** The wiki extension's one entry (S-644). */
const WIKI_IDS = ["wiki"];

/** `snapshot`'s sidebar less the entries of the extensions a build does not compose. */
function less(snapshot: string, ids: readonly string[]): NavItem[] {
  return (JSON.parse(snapshot) as NavItem[]).filter((i) => !ids.includes(i.id));
}

describe("the SPA navigation snapshot (S-639)", () => {
  // Since S-643/S-644 chat and the wiki are extensions, so the release (`agents`)
  // build's sidebar is the built-in one plus what the server reports active: mounted
  // here from the manifests that server actually serves (`test/chat.ts`,
  // `test/wiki.ts`), the snapshots must hold unchanged.
  it("offers exactly the snapshotted single-root sidebar with chat and the wiki active (the agents build)", () => {
    mountAgentsBuild();
    expect(navItemsFor(false)).toEqual(JSON.parse(singleRoot));
  });

  it("offers exactly the snapshotted workspace sidebar with chat and the wiki active (the agents build)", () => {
    mountAgentsBuild();
    expect(navItemsFor(true)).toEqual(JSON.parse(workspace));
  });

  // The default build composes the wiki and not chat: the same sidebar, in the same
  // order, less the Chat entries.
  it("offers the snapshotted sidebars less the chat entries with only the wiki active (the default build)", () => {
    for (const [isWorkspace, snapshot] of [
      [false, singleRoot],
      [true, workspace],
    ] as const) {
      clearExtensions();
      mountWiki();
      expect(navItemsFor(isWorkspace)).toEqual(less(snapshot, CHAT_IDS));
    }
  });

  // A build without either extension: the same sidebar, in the same order, with no
  // Chat and no Wiki entry in either mode — and nothing else missing. Booted the way
  // that build boots (S-645): the one shipped bundle registers chat's and the wiki's
  // code, and the server answers with the body the no-extension build actually serves
  // (`no-extension.real.json`, generated and guarded by
  // `cli/tests/no_extension_surface.rs`).
  it("offers the snapshotted sidebars less the chat and wiki entries when no extension is active", () => {
    registrations.forEach(registerExtensionUi);
    setActiveExtensions(noExtension as ExtensionsInfo);
    for (const [isWorkspace, snapshot] of [
      [false, singleRoot],
      [true, workspace],
    ] as const) {
      const items = navItemsFor(isWorkspace);
      expect(items).toEqual(less(snapshot, [...CHAT_IDS, ...WIKI_IDS]));
      const named = items.filter((i) => /chat|wiki/i.test(`${i.id} ${i.label} ${i.path}`));
      expect(named).toEqual([]);
    }
  });
});
