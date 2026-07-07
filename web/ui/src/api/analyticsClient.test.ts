import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchCoverage, fetchFiles, fetchQuadrant } from "./client.ts";

// The Files/Coverage/Quadrant typed helpers (S-188) build the right same-origin
// `/api/v1` URL over a stubbed fetch — the `untested` toggle is omitted when off
// (byte-identical to the no-filter request) and present when on.
describe("analytics endpoint helpers (over a stubbed same-origin fetch)", () => {
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

  it("fetchFiles omits the untested flag when off (byte-identical no-filter)", async () => {
    const calls = stubFetch();
    await fetchFiles(false);
    expect(calls()[0]).toBe("/api/v1/files");
  });

  it("fetchFiles carries ?untested=true when on", async () => {
    const calls = stubFetch();
    await fetchFiles(true);
    expect(calls()[0]).toBe("/api/v1/files?untested=true");
  });

  it("fetchCoverage and fetchQuadrant hit their bare endpoints (GET-only reads)", async () => {
    const calls = stubFetch();
    await fetchCoverage();
    await fetchQuadrant();
    expect(calls()).toEqual(["/api/v1/coverage", "/api/v1/quadrant"]);
  });
});
