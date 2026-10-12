/*
 * The sidebar lists a mounted extension's entry, with its icon, and re-renders when
 * the boot read settles after it mounted (S-642, FR-UI-51). The Sidebar is rendered
 * ALONE here — no shell above it to re-render it — so its own subscription to the
 * registry is what is under test.
 */

import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearExtensions, registerExtensionUi, setActiveExtensions } from "../extensions/registry.ts";
import { extensionsInfo, STUB_UI, stubInfo } from "../test/extensions.tsx";
import { setScopedMember } from "../workspace/scope.ts";
import { stubApi } from "../workspace/testFixtures.ts";
import { WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";
import { Sidebar } from "./Sidebar.tsx";

vi.mock("../router.tsx", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../router.tsx")>()),
  navigate: vi.fn(),
}));

afterEach(() => {
  cleanup();
  clearExtensions();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

function renderSidebar(pathname: string) {
  return render(
    <WorkspaceProvider>
      <Sidebar pathname={pathname} />
    </WorkspaceProvider>,
  );
}

describe("the Sidebar over the extension registry", () => {
  it("adds an entry, with the extension's icon, when the boot read settles after it mounted", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    renderSidebar("/stub");
    await waitFor(() => expect(screen.getByRole("link", { name: "Dashboard" })).toBeInTheDocument());
    expect(screen.queryByRole("link", { name: "Stub" })).toBeNull();
    // Let every other read the Sidebar's tree makes settle, so the update below is the
    // ONLY thing that can re-render it — and look for the entry without waiting for one.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });

    act(() => setActiveExtensions(extensionsInfo(stubInfo())));

    const link = screen.getByRole("link", { name: "Stub" });
    expect(within(link).getByTestId("stub-icon")).toBeInTheDocument();
    expect(link).toHaveAttribute("aria-current", "page");
  });

  it("gives an entry whose extension registers no icon a link without one", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi({ ...STUB_UI, icons: undefined });
    setActiveExtensions(extensionsInfo(stubInfo()));
    renderSidebar("/");

    const link = await screen.findByRole("link", { name: "Stub" });
    expect(within(link).queryByTestId("stub-icon")).toBeNull();
  });
});
