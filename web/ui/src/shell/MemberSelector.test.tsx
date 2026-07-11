import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { fetchHealth } from "../api/client.ts";
import { useWorkspace, WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";
import { scopedMember, setScopedMember } from "../workspace/scope.ts";
import { stubApi } from "../workspace/testFixtures.ts";
import { MemberSelector } from "./MemberSelector.tsx";

/** Exposes the settled mode, so a test can wait for the probe to ANSWER rather than
 *  asserting on the loading frame (where the selector is absent regardless — an
 *  assertion that would pass even if 404s were misread as workspace mode). */
function Mode() {
  return <span data-testid="mode">{useWorkspace().mode}</span>;
}

function mount() {
  return render(
    <WorkspaceProvider>
      <Mode />
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
  it("renders NO selector once single-root mode is SETTLED — the shell is unchanged", async () => {
    stubApi({ probeStatus: 404 });
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(scopedMember()).toBeNull();
  });

  it("lists every member in workspace mode and opens on the default", async () => {
    stubApi();
    mount();
    const select = await screen.findByRole("combobox");
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["api", "web"]);
    expect(select).toHaveValue("api");
  });

  it("switching members re-scopes every subsequent read", async () => {
    const calls = stubApi();
    mount();
    const select = await screen.findByRole("combobox");

    await userEvent.selectOptions(select, "web");
    expect(scopedMember()).toBe("web");

    await fetchHealth();
    expect(calls().at(-1)).toBe("/api/v1/health?repo=web");
  });

  it("states an unavailable workspace status rather than pretending it is a plain repo", async () => {
    stubApi({ probeStatus: 500 });
    mount();
    expect(await screen.findByText(/workspace status unavailable/i)).toBeInTheDocument();
  });
});
