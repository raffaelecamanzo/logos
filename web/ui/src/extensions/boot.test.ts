/*
 * The extension boot (S-642, FR-UI-51): code enters through `virtual:logos-extensions`,
 * and one read of `GET /api/v1/extensions` decides what is mounted.
 *
 * The virtual module is replaced here by a test registration — the stub — which is
 * what a composing build's entry would be.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import registrations from "virtual:logos-extensions";

import { extensionsInfo, STUB_UI, stubInfo } from "../test/extensions.tsx";
import { bootExtensions } from "./boot.ts";
import {
  clearExtensions,
  getExtensions,
  mountedConfigPanels,
  mountedNavEntries,
  mountedViews,
} from "./registry.ts";

vi.mock("virtual:logos-extensions", async () => ({
  default: [(await import("../test/extensions.tsx")).STUB_UI],
}));

afterEach(() => {
  clearExtensions();
  vi.unstubAllGlobals();
});

/** Stub `fetch` to answer `/api/v1/extensions` with `body` (or fail), and record the URLs. */
function stubExtensionsEndpoint(answer: { ok: boolean; status: number; body?: unknown } | "network-error") {
  const urls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      urls.push(url);
      if (answer === "network-error") throw new TypeError("fetch failed");
      return { ok: answer.ok, status: answer.status, json: async () => answer.body } as Response;
    }),
  );
  return urls;
}

describe("a registration through virtual:logos-extensions", () => {
  it("adds a nav item, a view and a config panel once the endpoint reports the extension active", async () => {
    const urls = stubExtensionsEndpoint({ ok: true, status: 200, body: extensionsInfo(stubInfo()) });
    await bootExtensions();

    expect(urls).toEqual(["/api/v1/extensions"]);
    expect(mountedNavEntries().map((n) => n.id)).toEqual(["stub"]);
    expect(mountedViews().map(([route]) => route)).toEqual(["/stub"]);
    expect(mountedConfigPanels("member").map((p) => p.panel.id)).toEqual(["stub-config"]);
  });

  it("mounts none of them when the endpoint does not report the extension active", async () => {
    stubExtensionsEndpoint({ ok: true, status: 200, body: extensionsInfo() });
    await bootExtensions();

    expect(getExtensions().status).toBe("ready");
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedViews()).toEqual([]);
    expect(mountedConfigPanels("member")).toEqual([]);
  });

  it("mounts none of them when the read fails — an extension the server did not vouch for is not mounted", async () => {
    stubExtensionsEndpoint({ ok: false, status: 500 });
    await bootExtensions();
    expect(getExtensions().status).toBe("ready");
    expect(mountedNavEntries()).toEqual([]);

    clearExtensions();
    stubExtensionsEndpoint("network-error");
    await bootExtensions();
    expect(getExtensions().status).toBe("ready");
    expect(mountedNavEntries()).toEqual([]);
  });

  it("registers the code before the read settles, so a deep link can be told from an unknown route", async () => {
    stubExtensionsEndpoint({ ok: true, status: 200, body: extensionsInfo(stubInfo()) });
    const booting = bootExtensions();
    expect(getExtensions().status).toBe("loading");
    expect(getExtensions().registeredPaths).toEqual(Object.keys(STUB_UI.views ?? {}));
    await booting;
  });

  it("settles, mounting nothing, and still throws when a registration is refused", async () => {
    const urls = stubExtensionsEndpoint({ ok: true, status: 200, body: extensionsInfo(stubInfo()) });
    // The mocked entry is `[STUB_UI]`; a second copy is the one id twice.
    (registrations as unknown[]).push(STUB_UI);
    try {
      await expect(bootExtensions()).rejects.toThrow('"stub" is registered twice');
    } finally {
      (registrations as unknown[]).pop();
    }

    expect(getExtensions().status).toBe("ready");
    expect(mountedNavEntries()).toEqual([]);
    expect(urls).toEqual([]);
  });
});
