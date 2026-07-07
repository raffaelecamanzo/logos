import { afterEach, describe, expect, it, vi } from "vitest";

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
