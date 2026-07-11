import { afterEach, describe, expect, it, vi } from "vitest";

import { setScopedMember } from "../workspace/scope.ts";
import { ApiError } from "../intent.ts";
import { fetchWorkspaceImpact, fetchWorkspaceRoster, probeWorkspace } from "./workspaceClient.ts";

/** Stub `fetch` with a fixed status, recording the URLs requested. */
function stubFetch(status = 200, body: unknown = {}): () => string[] {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      return Promise.resolve({
        ok: status >= 200 && status < 300,
        status,
        json: () => Promise.resolve(body),
      } as Response);
    }),
  );
  return () => calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("probeWorkspace (S-250, FR-WS-06)", () => {
  it("reads the surface's honest 404 as single-root — not as a failure", async () => {
    stubFetch(404);
    await expect(probeWorkspace()).resolves.toEqual({ mode: "single" });
  });

  it("RETHROWS any other failure — a broken read must never masquerade as a plain repo", async () => {
    // Downgrading a 500 to "single-root" would silently hide the whole workspace UI
    // and state a mode that was never established (NFR-RA-05).
    stubFetch(500);
    await expect(probeWorkspace()).rejects.toBeInstanceOf(ApiError);
  });

  it("probes the engine-free roster endpoint", async () => {
    const calls = stubFetch(200, { workspace: "shop", default: "api", members: ["api", "web"] });
    await expect(probeWorkspace()).resolves.toEqual({
      mode: "workspace",
      roster: { workspace: "shop", default: "api", members: ["api", "web"] },
    });
    expect(calls()[0]).toBe("/api/v1/workspace/roster");
  });
});

describe("the workspace fan-out is app-level (S-250)", () => {
  it("carries NO ambient member scope — the fan-out is a view of every member", async () => {
    const calls = stubFetch();
    setScopedMember("api");
    await fetchWorkspaceRoster();
    await fetchWorkspaceImpact("get_user");
    // `?repo=` on a fan-out NARROWS it; auto-applying the shell's scope would quietly
    // reduce the app-level views to one member's slice.
    expect(calls()).toEqual(["/api/v1/workspace/roster", "/api/v1/workspace/impact?symbol=get_user"]);
  });

  it("scopes the impact seed only when a member is passed EXPLICITLY", async () => {
    const calls = stubFetch();
    setScopedMember("api");
    await fetchWorkspaceImpact("get_user", "web");
    expect(calls()[0]).toBe("/api/v1/workspace/impact?symbol=get_user&repo=web");
  });
});
