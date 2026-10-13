/*
 * The write-only key routes (FR-CF-06, NFR-SE-07): `saveSecret` (a member's
 * `POST /config/secret`) and `saveWorkspaceSecret` (the workspace root's) both post
 * through `writeSecret`, which resolves with the masked outcome only and never
 * returns or reports the reply body, so no reply from a secret route can carry key
 * material onto a SPA surface. The routes are the core's; their clients are kept
 * for the views a build composes over them.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { setScopedMember } from "../workspace/scope.ts";
import { ConfigMutateError, saveSecret } from "./configClient.ts";
import { saveWorkspaceSecret } from "./workspaceClient.ts";

const KEY = "sk-never-echo-0123456789";

/** Stub one reply whose body reads as `text`, recording each request. */
function stubOnce(status: number, text: string): { url: string; init?: RequestInit }[] {
  const calls: { url: string; init?: RequestInit }[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init?: RequestInit) => {
      calls.push({ url, init });
      return Promise.resolve({
        ok: status >= 200 && status < 300,
        status,
        text: () => Promise.resolve(text),
        json: () => Promise.resolve(JSON.parse(text)),
      } as Response);
    }),
  );
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
  setScopedMember(null);
});

const ROUTES = [
  { name: "saveSecret", save: saveSecret, url: "/config/secret?repo=api" },
  { name: "saveWorkspaceSecret", save: saveWorkspaceSecret, url: "/api/v1/workspace/config/secret" },
] as const;

describe.each(ROUTES)("$name (NFR-SE-07)", ({ save, url }) => {
  it("posts the key to its route and resolves with the masked outcome", async () => {
    setScopedMember("api");
    const outcome = { path: ".logos/secrets.toml", chat_key: { present: true, last4: "6789" } };
    const calls = stubOnce(200, JSON.stringify(outcome));
    await expect(save(KEY)).resolves.toEqual(outcome);
    expect(calls.map((c) => c.url)).toEqual([url]);
    expect(new URLSearchParams(String(calls[0].init?.body)).get("api_key")).toBe(KEY);
  });

  it("reports a rejected write with a fixed detail, never the reply body", async () => {
    stubOnce(422, `invalid key ${KEY}`);
    const err = (await save(KEY).catch((e: unknown) => e)) as ConfigMutateError;
    expect(err).toBeInstanceOf(ConfigMutateError);
    expect(err.status).toBe(422);
    expect(err.detail).toBe("the server rejected the key write");
    expect(`${err.message} ${err.detail}`).not.toContain(KEY);
  });

  it("resolves a non-JSON 2xx to null rather than surface its body", async () => {
    stubOnce(200, `saved ${KEY}`);
    await expect(save(KEY)).resolves.toBeNull();
  });
});
