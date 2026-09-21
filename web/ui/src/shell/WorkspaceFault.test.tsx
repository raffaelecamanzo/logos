/*
 * The workspace-probe fault badge (S-250 behaviour, split out of `MemberSelector`
 * in S-425 — FR-UI-29, NFR-RA-05, NFR-CC-04).
 *
 * The spec that moved: a genuine probe fault must be STATED, never passed off as
 * "this is not a workspace". S-425 made that a separate component because the
 * selector went into a sidebar section that a fault never reaches — the context
 * settles the mode to `single` and records the error — so the badge had to stay
 * somewhere rendered in every mode. This suite is what fails if it stops being.
 */

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useWorkspace, WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../workspace/scope.ts";
import { stubApi } from "../workspace/testFixtures.ts";
import { WorkspaceFault } from "./WorkspaceFault.tsx";

/** Exposes the settled mode, so a test can wait for the probe to ANSWER rather than
 *  asserting on the loading frame (where the badge is absent regardless). */
function Mode() {
  return <span data-testid="mode">{useWorkspace().mode}</span>;
}

function mount() {
  return render(
    <WorkspaceProvider>
      <Mode />
      <WorkspaceFault />
    </WorkspaceProvider>,
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("WorkspaceFault (S-250, FR-UI-29, NFR-RA-05)", () => {
  it("states an unavailable workspace status rather than pretending it is a plain repo", async () => {
    stubApi({ probeStatus: 500 });
    mount();
    expect(await screen.findByText(/workspace status unavailable/i)).toBeInTheDocument();
  });

  it("renders nothing for an HONEST 404 — that is a plain repo, not a fault", async () => {
    // The 404 IS the single-root signal. A badge here would report an error on every
    // plain repository's first paint, and would change the single-root header ADR-52
    // pins byte-for-byte.
    stubApi({ probeStatus: 404 });
    const { container } = mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("single"));
    expect(screen.queryByText(/workspace status unavailable/i)).toBeNull();
    expect(container.querySelectorAll("span[class]")).toHaveLength(0);
  });

  it("renders nothing for a healthy workspace", async () => {
    stubApi();
    mount();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("workspace"));
    expect(screen.queryByText(/workspace status unavailable/i)).toBeNull();
  });
});
