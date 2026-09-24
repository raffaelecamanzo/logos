import { afterEach, describe, expect, it, vi } from "vitest";

import { setScopedMember } from "../workspace/scope.ts";
import { ApiError } from "../intent.ts";
import {
  fetchWorkspaceGovernance,
  fetchWorkspaceImpact,
  fetchWorkspaceReachability,
  fetchWorkspaceManifest,
  fetchWorkspaceRoster,
  probeWorkspace,
  saveWorkspaceManifest,
} from "./workspaceClient.ts";
import { ConfigMutateError } from "./configClient.ts";

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

describe("the S-427 reads (FR-WS-28)", () => {
  it("reads reachability and governance on their own app-level routes", async () => {
    const calls = stubFetch();
    setScopedMember("api");
    await fetchWorkspaceReachability();
    await fetchWorkspaceGovernance();
    // Unscoped, like every other `workspace/*` read: the ambient member must not
    // narrow an answer the views present as workspace-wide (S-250).
    expect(calls()).toEqual([
      "/api/v1/workspace/reachability",
      "/api/v1/workspace/check",
    ]);
  });

  it("leaves the promotions-only default in place — it never sends ?all", async () => {
    // The bound is the SERVER's default and the payload states it in
    // `reachability.scope`. A client that opted out here would pull the whole
    // per-repo dead set (~500 KB on a large workspace) into a view that renders
    // the promotions (CR-084, NFR-PE-10).
    const calls = stubFetch();
    await fetchWorkspaceReachability();
    expect(calls()[0]).not.toContain("all");
  });
});

describe("the S-430 manifest read and save (FR-UI-38)", () => {
  /** Stub one response with a body readable as JSON and as text. */
  function stubOnce(status: number, body: unknown): { url: string; init?: RequestInit }[] {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        calls.push({ url, init });
        return Promise.resolve({
          ok: status >= 200 && status < 300,
          status,
          json: () => Promise.resolve(body),
          text: () => Promise.resolve(typeof body === "string" ? body : JSON.stringify(body)),
        } as Response);
      }),
    );
    return calls;
  }

  it("reads and saves on the app-level routes, never carrying the ambient member", async () => {
    setScopedMember("api");
    const calls = stubOnce(200, { outcome: "unchanged", path: "logos.workspace.toml", fingerprint: "f" });
    await fetchWorkspaceManifest();
    await saveWorkspaceManifest("[workspace]\nname = \"a\"\n", "f");
    expect(calls.map((c) => c.url)).toEqual([
      "/api/v1/workspace/manifest",
      "/api/v1/workspace/manifest/save",
    ]);
    const form = new URLSearchParams(String(calls[1].init?.body));
    expect(form.get("content")).toBe('[workspace]\nname = "a"\n');
    expect(form.get("fingerprint")).toBe("f");
  });

  it("resolves a 409 conflict as an outcome — it is the server declining to clobber, not a fault", async () => {
    const conflict = {
      outcome: "conflict",
      path: "logos.workspace.toml",
      loaded_fingerprint: "a",
      disk_fingerprint: "b",
      disk_content: "x",
    };
    stubOnce(409, conflict);
    await expect(saveWorkspaceManifest("c", "a")).resolves.toEqual(conflict);
  });

  it("throws a 422 with the family's JSON error message, not its raw body", async () => {
    stubOnce(422, { error: "unknown field `membrs`" });
    const err = await saveWorkspaceManifest("c", "a").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ConfigMutateError);
    expect((err as ConfigMutateError).status).toBe(422);
    expect((err as ConfigMutateError).detail).toBe("unknown field `membrs`");
  });

  it("keeps a non-JSON error body verbatim", async () => {
    stubOnce(403, "missing or invalid intent token");
    const err = (await saveWorkspaceManifest("c", "a").catch((e: unknown) => e)) as ConfigMutateError;
    expect(err.detail).toBe("missing or invalid intent token");
  });
});
