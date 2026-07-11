import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchHealth } from "../api/client.ts";
import { useWorkspace, WorkspaceProvider } from "./WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "./scope.ts";

/** A two-member workspace status, as `/api/v1/workspace/status` serves it. */
const STATUS = {
  workspace: "shop",
  members: [
    { member: "api", result: { indexed: true, file_count: 3 } },
    { member: "web", result: { indexed: false, file_count: 0 } },
  ],
  coverage: { references: [], bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0, bound_ratio: 1 },
};

/** Stub `fetch`, answering the workspace probe with `probe` and recording every URL. */
function stubFetch(probe: { status: number; body?: unknown }): () => string[] {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      if (url.startsWith("/api/v1/workspace/status")) {
        return Promise.resolve({
          ok: probe.status === 200,
          status: probe.status,
          json: () => Promise.resolve(probe.body ?? {}),
        } as Response);
      }
      return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve({}) } as Response);
    }),
  );
  return () => calls;
}

/** A probe of the context, plus a read that shows whether the scope reached the wire. */
function Probe() {
  const { mode, workspace, members, member, cacheKey, error } = useWorkspace();
  return (
    <div>
      <span data-testid="mode">{mode}</span>
      <span data-testid="workspace">{workspace ?? "—"}</span>
      <span data-testid="members">{members.map((m) => `${m.name}:${m.indexed}`).join(",")}</span>
      <span data-testid="member">{member ?? "—"}</span>
      <span data-testid="key">{cacheKey}</span>
      <span data-testid="error">{error ? "error" : "—"}</span>
    </div>
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("WorkspaceProvider mode discovery (S-250, FR-UI-29, FR-WS-06)", () => {
  it("reads the fan-out's honest 404 as single-root — and scopes NOTHING", async () => {
    stubFetch({ status: 404 });
    render(
      <WorkspaceProvider>
        <Probe />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));
    expect(screen.getByTestId("member")).toHaveTextContent("—");
    expect(screen.getByTestId("error")).toHaveTextContent("—");
    // The decisive single-root guarantee: no member scope, so no request ever
    // carries `?repo=` and every URL stays byte-for-byte the pre-workspace one.
    expect(scopedMember()).toBeNull();
    // …and the cache key is constant, so nothing ever remounts/re-fetches.
    expect(screen.getByTestId("key")).toHaveTextContent("single");
  });

  it("reads a 200 as workspace mode, rosters the members, and selects the first", async () => {
    stubFetch({ status: 200, body: STATUS });
    render(
      <WorkspaceProvider>
        <Probe />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));
    expect(screen.getByTestId("workspace")).toHaveTextContent("shop");
    // The un-indexed member is rostered as awaiting-index, never dropped.
    expect(screen.getByTestId("members")).toHaveTextContent("api:true,web:false");
    expect(screen.getByTestId("member")).toHaveTextContent("api");
    // The member IS the cache key — the shell keys the view subtree on it.
    expect(screen.getByTestId("key")).toHaveTextContent("api");
    expect(scopedMember()).toBe("api");
  });

  it("scopes the transport so every existing view's read carries ?repo=<member>", async () => {
    const calls = stubFetch({ status: 200, body: STATUS });
    render(
      <WorkspaceProvider>
        <Probe />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));

    await fetchHealth();
    expect(calls().at(-1)).toBe("/api/v1/health?repo=api");
  });

  it("surfaces a genuine probe fault instead of passing it off as a plain repo", async () => {
    stubFetch({ status: 500 });
    render(
      <WorkspaceProvider>
        <Probe />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("error")).toHaveTextContent("error"));
    // There is no roster to render, so the layout degrades — but the fault is
    // recorded, and the header states it (see MemberSelector).
    expect(scopedMember()).toBeNull();
  });
});
