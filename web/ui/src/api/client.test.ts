import { afterEach, describe, expect, it, vi } from "vitest";

import { setScopedMember } from "../workspace/scope.ts";
import { apiUrl, fetchGraph, runQuery } from "./client.ts";

describe("apiUrl", () => {
  it("prefixes /api/v1 and omits empty/false/null params (byte-identical no-filter)", () => {
    expect(apiUrl("graph")).toBe("/api/v1/graph");
    expect(apiUrl("graph", { cap: 250, seed: "", intent: false, layers: undefined })).toBe(
      "/api/v1/graph?cap=250",
    );
  });

  it("URL-encodes values and keeps a truthy flag", () => {
    expect(apiUrl("query", { q: "a b", intent: true })).toBe("/api/v1/query?q=a+b&intent=true");
  });
});

describe("typed endpoint helpers (over a stubbed same-origin fetch)", () => {
  afterEach(() => vi.unstubAllGlobals());

  function stubFetch(): () => string[] {
    const calls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        calls.push(url);
        return Promise.resolve({ ok: true, json: () => Promise.resolve({}) } as Response);
      }),
    );
    return () => calls;
  }

  it("fetchGraph drops the default `symbol` granularity so the request stays byte-identical", async () => {
    const calls = stubFetch();
    await fetchGraph({ cap: 250, granularity: "symbol" });
    expect(calls()[0]).toBe("/api/v1/graph?cap=250");
  });

  it("fetchGraph carries a non-default tier and the re-budgeting filters", async () => {
    const calls = stubFetch();
    await fetchGraph({ cap: 250, granularity: "file", layers: "code,doc" });
    expect(calls()[0]).toBe("/api/v1/graph?cap=250&granularity=file&layers=code%2Cdoc");
  });

  it("runQuery sends a relational verb + target", async () => {
    const calls = stubFetch();
    await runQuery({ verb: "impact-of", target: "Engine" });
    expect(calls()[0]).toBe("/api/v1/query?verb=impact-of&target=Engine");
  });
});

// ── The workspace member scope (S-250, CR-061, FR-UI-29) ─────────────────────
// The selector reaches every existing view through this one builder: the active
// member rides as `?repo=`. Single-root leaves the scope null, so every URL in the
// blocks above stays byte-for-byte what it was.

describe("the member scope on /api/v1 reads", () => {
  afterEach(() => setScopedMember(null));

  it("appends the active member to every ordinary read", () => {
    setScopedMember("api");
    expect(apiUrl("health")).toBe("/api/v1/health?repo=api");
    expect(apiUrl("graph", { cap: 250 })).toBe("/api/v1/graph?repo=api&cap=250");
  });

  it("appends NOTHING when unscoped — the single-root URL is byte-identical", () => {
    setScopedMember(null);
    expect(apiUrl("health")).toBe("/api/v1/health");
    expect(apiUrl("graph", { cap: 250 })).toBe("/api/v1/graph?cap=250");
  });

  it("treats a blank member as unscoped rather than sending an empty ?repo=", () => {
    setScopedMember("   ");
    expect(apiUrl("health")).toBe("/api/v1/health");
  });

  it("never auto-scopes the workspace fan-out — the service map is app-level by design", () => {
    setScopedMember("api");
    // `?repo=` on a fan-out NARROWS it; auto-applying the shell scope there would
    // silently reduce the app-level service map to one member's slice.
    expect(apiUrl("workspace/status")).toBe("/api/v1/workspace/status");
    expect(apiUrl("workspace/route-providers")).toBe("/api/v1/workspace/route-providers");
    // …but an explicitly-passed member still wins (the impact view scopes its seed).
    expect(apiUrl("workspace/impact", { symbol: "f", repo: "web" })).toBe(
      "/api/v1/workspace/impact?symbol=f&repo=web",
    );
  });

  it("an explicit repo param overrides the ambient scope", () => {
    setScopedMember("api");
    expect(apiUrl("node", { symbol: "f", repo: "web" })).toBe("/api/v1/node?repo=web&symbol=f");
  });
});
