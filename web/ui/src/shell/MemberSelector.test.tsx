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

/** The Service-section heading the sidebar renders beside the control, standing in
 *  for it here: the control has no label of its own, so a suite that omitted the
 *  heading would assert against a `<select>` with no accessible name — a world the
 *  shell never renders. */
function mount() {
  return render(
    <WorkspaceProvider>
      <Mode />
      <h2 id="nav-scope-member">Service</h2>
      <MemberSelector labelledBy="nav-scope-member" />
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

  it("takes its accessible name from the section heading, and renders no label of its own", async () => {
    // The row is `SERVICE [ orders ▾ ]` (frontend-design §3). A label element here
    // would put a second word for the same thing on a 232px row — and the heading is
    // the better name, because it is the one the sidebar guarantees at every
    // breakpoint (S-425, FR-UI-35, NFR-CC-04).
    stubApi();
    const { container } = mount();
    const select = await screen.findByRole("combobox");
    expect(select).toHaveAccessibleName("Service");
    expect(container.querySelectorAll("label")).toHaveLength(0);
  });

  it("reports NOTHING about a faulted probe — that badge is `WorkspaceFault`'s", async () => {
    // A fault settles the mode to `single`, and the Service section this control
    // renders in does not exist there. A fault reported from here would be a fault
    // reported nowhere (NFR-RA-05); `WorkspaceFault.test.tsx` pins where it goes.
    stubApi({ probeStatus: 500 });
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));
    expect(screen.queryByText(/workspace status unavailable/i)).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
  });
});
