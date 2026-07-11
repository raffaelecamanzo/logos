import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchHealth } from "../api/client.ts";
import { useWorkspace, WorkspaceProvider } from "./WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "./scope.ts";
import { stubApi } from "./testFixtures.ts";

/** A probe of the context — every field the shell reads. */
function Probe() {
  const { mode, workspace, members, member, cacheKey, error } = useWorkspace();
  return (
    <div>
      <span data-testid="mode">{mode}</span>
      <span data-testid="workspace">{workspace ?? "—"}</span>
      <span data-testid="members">{members.join(",")}</span>
      <span data-testid="member">{member ?? "—"}</span>
      <span data-testid="key">{cacheKey}</span>
      <span data-testid="error">{error ? "error" : "—"}</span>
    </div>
  );
}

function mount() {
  return render(
    <WorkspaceProvider>
      <Probe />
    </WorkspaceProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("WorkspaceProvider mode discovery (S-250, FR-UI-29, FR-WS-06)", () => {
  it("reads the roster's honest 404 as single-root — and scopes NOTHING", async () => {
    stubApi({ probeStatus: 404 });
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));
    expect(screen.getByTestId("member")).toHaveTextContent("—");
    expect(screen.getByTestId("error")).toHaveTextContent("—");
    // The decisive single-root guarantee: no member scope, so no request ever carries
    // `?repo=` and every URL stays byte-for-byte the pre-workspace one.
    expect(scopedMember()).toBeNull();
    // …and the cache key never changes, so nothing remounts or re-fetches.
    expect(screen.getByTestId("key")).toHaveTextContent("single");
  });

  it("reads a 200 as workspace mode and rosters the members", async () => {
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));
    expect(screen.getByTestId("workspace")).toHaveTextContent("shop");
    expect(screen.getByTestId("members")).toHaveTextContent("api,web");
  });

  it("probes the ENGINE-FREE roster, never the all-member status fan-out (NFR-PE-10)", async () => {
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));
    // The shell probes on every page load. `workspace/status` fans out over every
    // member — probing it here would construct and watch all N engines on first paint.
    expect(calls().some((u) => u.startsWith("/api/v1/workspace/roster"))).toBe(true);
    expect(calls().some((u) => u.startsWith("/api/v1/workspace/status"))).toBe(false);
  });

  it("opens on the manifest DEFAULT member, not merely the first in the roster", async () => {
    // The roster's default is `api`; if the SPA opened on `members[0]` blindly it would
    // agree here by luck. Put the default second so the two genuinely differ.
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve({
          ok: true,
          status: 200,
          json: () =>
            Promise.resolve(
              url.startsWith("/api/v1/workspace/roster")
                ? { workspace: "shop", default: "web", members: ["api", "web"] }
                : {},
            ),
        } as Response),
      ),
    );
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("web"));
    // Unscoped requests answer from the default member, so opening anywhere else would
    // show a different member than the CLI and the plain API do.
    expect(scopedMember()).toBe("web");
  });

  it("namespaces the cache key so a member literally named `single` cannot collide", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve({
          ok: true,
          status: 200,
          json: () =>
            Promise.resolve(
              url.startsWith("/api/v1/workspace/roster")
                ? { workspace: "shop", default: "single", members: ["single"] }
                : {},
            ),
        } as Response),
      ),
    );
    mount();
    await waitFor(() => expect(screen.getByTestId("member")).toHaveTextContent("single"));
    // If the key were the bare member name it would equal the pre-probe sentinel, the
    // view would never remount on the mode flip, and it would keep the unscoped data.
    expect(screen.getByTestId("key")).toHaveTextContent("member:single");
  });

  it("scopes the transport so every existing view's read carries ?repo=<member>", async () => {
    const calls = stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));

    await fetchHealth();
    expect(calls().at(-1)).toBe("/api/v1/health?repo=api");
  });

  it("surfaces a genuine probe fault instead of passing it off as a plain repo", async () => {
    stubApi({ probeStatus: 500 });
    mount();
    await waitFor(() => expect(screen.getByTestId("error")).toHaveTextContent("error"));
    expect(scopedMember()).toBeNull();
  });
});
