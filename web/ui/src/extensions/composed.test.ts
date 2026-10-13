/*
 * The composed entry of this build (S-642, FR-UI-51, FR-DS-05):
 * `virtual:logos-extensions` resolves — through vitest's own copy of the alias,
 * unmocked — to an entry that registers no extension. Whatever the server reports, the
 * open-source build mounts nothing beyond the built-in shell, and nothing stands in
 * place of an extension it does not compose.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import registrations from "virtual:logos-extensions";

import { extensionsInfo, stubInfo } from "../test/extensions.tsx";
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

/** Nothing is mounted on any contribution point, and no route is an extension's. */
function expectBuiltInShellOnly(): void {
  expect(getExtensions().status).toBe("ready");
  expect(getExtensions().mounted).toEqual([]);
  expect(getExtensions().registeredPaths).toEqual([]);
  expect(mountedNavEntries()).toEqual([]);
  expect(mountedViews()).toEqual([]);
  expect(mountedConfigPanels("member")).toEqual([]);
  expect(mountedConfigPanels("workspace")).toEqual([]);
  expect(mountedDashboardCards()).toEqual([]);
}

describe("virtual:logos-extensions in this build", () => {
  it("registers no extension", () => {
    expect(registrations).toEqual([]);
  });

  it("mounts nothing for the body this build serves — no extension listed", async () => {
    // The body the binary built from this repository serves (S-645,
    // `cli/tests/no_extension_surface.rs`).
    await bootWith(noExtension);
    expectBuiltInShellOnly();
  });

  it("mounts nothing for a server that reports an extension active: this bundle carries no code for it", async () => {
    await bootWith(extensionsInfo(stubInfo()));
    expectBuiltInShellOnly();
  });
});
