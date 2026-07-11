import { afterEach, describe, expect, it, vi } from "vitest";

import { saveConfig } from "../api/configClient.ts";
import { scopedMember, setScopedMember, withMemberScope } from "./scope.ts";

afterEach(() => {
  setScopedMember(null);
  vi.unstubAllGlobals();
});

describe("the member scope (S-250, FR-UI-29)", () => {
  it("starts unscoped, so a single-root SPA never sends a member", () => {
    expect(scopedMember()).toBeNull();
    expect(withMemberScope("/config/save")).toBe("/config/save");
  });

  it("normalises a blank member to unscoped rather than sending an empty ?repo=", () => {
    setScopedMember("  ");
    expect(scopedMember()).toBeNull();
    setScopedMember("api");
    expect(scopedMember()).toBe("api");
    setScopedMember(null);
    expect(scopedMember()).toBeNull();
  });

  it("appends the member to a mutating path, URL-encoding a nested member name", () => {
    setScopedMember("services/api");
    expect(withMemberScope("/config/save")).toBe("/config/save?repo=services%2Fapi");
    expect(withMemberScope("/config/apply?file=rules")).toBe(
      "/config/apply?file=rules&repo=services%2Fapi",
    );
  });

  it("carries the member on a config WRITE — the editor never saves over another member", async () => {
    // The load-bearing case: the Config tab reads the selected member's policy, so
    // its Save must write back to THAT member, not the workspace default's file.
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        calls.push(url);
        return Promise.resolve({ ok: true, json: () => Promise.resolve({}) } as Response);
      }),
    );
    setScopedMember("web");
    await saveConfig("rules", "[rules]\n");
    expect(calls[0]).toBe("/config/save?repo=web");
  });
});
