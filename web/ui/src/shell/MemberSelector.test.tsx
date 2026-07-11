import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchHealth } from "../api/client.ts";
import { WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "../workspace/scope.ts";
import { MemberSelector } from "./MemberSelector.tsx";

const STATUS = {
  workspace: "shop",
  members: [
    { member: "api", result: { indexed: true } },
    { member: "web", result: { indexed: false } },
  ],
  coverage: { references: [], bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0, bound_ratio: 1 },
};

function stubFetch(probeStatus: number): () => string[] {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      const isProbe = url.startsWith("/api/v1/workspace/status");
      return Promise.resolve({
        ok: !isProbe || probeStatus === 200,
        status: isProbe ? probeStatus : 200,
        json: () => Promise.resolve(isProbe ? STATUS : {}),
      } as Response);
    }),
  );
  return () => calls;
}

function mount() {
  return render(
    <WorkspaceProvider>
      <MemberSelector />
    </WorkspaceProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("MemberSelector (S-250, FR-UI-29)", () => {
  it("renders NO selector in single-root mode — the shell is unchanged", async () => {
    stubFetch(404);
    const { container } = mount();
    // Wait for the probe to settle, then assert the header contributed nothing.
    await waitFor(() => expect(scopedMember()).toBeNull());
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole("combobox")).toBeNull();
  });

  it("lists every member in workspace mode, labelling an un-indexed one honestly", async () => {
    stubFetch(200);
    mount();
    const select = await screen.findByRole("combobox");
    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["api", "web (awaiting index)"]);
    expect(select).toHaveValue("api");
  });

  it("switching members re-scopes every subsequent read (the cache key moves with it)", async () => {
    const calls = stubFetch(200);
    mount();
    const select = await screen.findByRole("combobox");

    await userEvent.selectOptions(select, "web");
    expect(scopedMember()).toBe("web");

    await fetchHealth();
    expect(calls().at(-1)).toBe("/api/v1/health?repo=web");
  });

  it("states an unavailable workspace status rather than pretending it is a plain repo", async () => {
    stubFetch(500);
    mount();
    expect(await screen.findByText(/workspace status unavailable/i)).toBeInTheDocument();
  });
});
