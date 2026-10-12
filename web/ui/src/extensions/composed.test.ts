/*
 * The composed entry of this build (S-642, FR-UI-51, S-643, S-644):
 * `virtual:logos-extensions` resolves — through vitest's own copy of the alias,
 * unmocked — to the code of the `chat` and `wiki` extensions and nothing else.
 * Registered code is inert until the server reports its extension active, so the boot
 * read decides: a `logos` binary composing neither gets exactly the built-in shell,
 * and one composing an extension gets its navigation, views, config panels and cards.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import registrations from "virtual:logos-extensions";

import { CHAT_ACTIVE } from "../test/chat.ts";
import { extensionsInfo, stubInfo } from "../test/extensions.tsx";
import { WIKI_ACTIVE } from "../test/wiki.ts";
import { CHAT_EXTENSION_UI } from "../views/chat/extension.tsx";
import { WIKI_EXTENSION_UI } from "../views/wiki/extension.tsx";
import { bootExtensions } from "./boot.ts";
import noExtension from "./fixtures/no-extension.real.json";
import {
  clearExtensions,
  getExtensions,
  mountedConfigPanels,
  mountedDashboardCards,
  mountedNavEntries,
  mountedViews,
} from "./registry.ts";

afterEach(() => {
  clearExtensions();
  vi.unstubAllGlobals();
});

/** Boot with the server answering `GET /api/v1/extensions` with `body`. */
async function bootWith(body: unknown): Promise<void> {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => ({ ok: true, status: 200, json: async () => body }) as Response),
  );
  await bootExtensions();
}

describe("virtual:logos-extensions in this build", () => {
  it("resolves to the chat and wiki extensions' registrations, and only those", () => {
    expect(registrations).toEqual([CHAT_EXTENSION_UI, WIKI_EXTENSION_UI]);
  });

  it("mounts nothing for a server that reports no extension — a build without extensions", async () => {
    // The body the no-extension build serves (S-645, `cli/tests/no_extension_surface.rs`).
    await bootWith(noExtension);
    expect(getExtensions().status).toBe("ready");
    expect(getExtensions().mounted).toEqual([]);
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedViews()).toEqual([]);
    expect(mountedConfigPanels("member")).toEqual([]);
    expect(mountedConfigPanels("workspace")).toEqual([]);
    expect(mountedDashboardCards()).toEqual([]);
    // The chat and wiki routes are still known as an extension's, so a deep link to
    // one is routed by the inactive-extension rule rather than taken for a built-in
    // path.
    expect(getExtensions().registeredPaths).toEqual(["/chat", "/workspace-chat", "/wiki"]);
  });

  it("leaves an extension the server reports active with no code here inert", async () => {
    await bootWith(extensionsInfo(stubInfo()));
    expect(mountedNavEntries()).toEqual([]);
  });

  it("mounts the wiki's entry, view, config panels and Dashboard card for a server that reports it active", async () => {
    await bootWith(WIKI_ACTIVE);
    expect(mountedNavEntries().map((n) => n.id)).toEqual(["wiki"]);
    expect(mountedViews().map(([route]) => route)).toEqual(["/wiki"]);
    expect(mountedConfigPanels("member").map((p) => p.panel.table)).toEqual(["wiki"]);
    expect(mountedConfigPanels("workspace").map((p) => p.panel.table)).toEqual(["wiki"]);
    expect(mountedDashboardCards().map(({ card }) => card.id)).toEqual(["project-overview"]);
  });

  it("mounts chat's entries, views and both config panels for a server that reports it active", async () => {
    await bootWith(CHAT_ACTIVE);
    expect(mountedNavEntries().map((n) => n.id)).toEqual(["chat", "workspace-chat"]);
    expect(mountedViews().map(([route]) => route)).toEqual(["/chat", "/workspace-chat"]);
    expect(mountedConfigPanels("member").map((p) => p.panel.table)).toEqual(["chat"]);
    expect(mountedConfigPanels("workspace").map((p) => p.panel.table)).toEqual(["chat"]);
  });
});
