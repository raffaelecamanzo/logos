import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ReactNode } from "react";

import { ThemeContext } from "../theme/theme.ts";
import { WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";
import { Header } from "./Header.tsx";

// The header probes /api/v1/health on mount; stub it so the connectivity check
// resolves without a real network (jsdom has no fetch). `ApiError` must be part of the
// mock: the header now builds its URL through `api/client.ts` (so the probe carries the
// workspace member scope, S-250), and that module imports the error type from here.
vi.mock("../intent.ts", () => ({
  apiGet: vi.fn().mockResolvedValue({}),
  ApiError: class ApiError extends Error {},
}));

// S-250: the header's probe now waits for the workspace mode to settle (until then the
// member scope is unknown, so a request would go out unscoped and be re-issued). Answer
// the roster probe as a plain repo — this spec's world — so the header proceeds.
vi.mock("../api/workspaceClient.ts", () => ({
  probeWorkspace: vi.fn().mockResolvedValue({ mode: "single" }),
}));
// Spy on the client router so we can assert the brand navigates without a reload.
const { mockNavigate } = vi.hoisted(() => ({ mockNavigate: vi.fn() }));
vi.mock("../router.tsx", () => ({ navigate: mockNavigate }));

// The header hosts the ThemeToggle, which reads the theme context; give it a
// static value so the toggle mounts without the full provider tree.
function withTheme(node: ReactNode) {
  return (
    <ThemeContext.Provider value={{ theme: "dark", setTheme: () => {}, toggleTheme: () => {} }}>
      {/* The header reads the workspace mode (it hosts the member selector, and its own
          probe is member-scoped), so it needs the provider App gives it in production. */}
      <WorkspaceProvider>{node}</WorkspaceProvider>
    </ThemeContext.Provider>
  );
}

afterEach(() => {
  cleanup();
  mockNavigate.mockClear();
});

describe("Header brand lockup", () => {
  it("renders the brand mark + wordmark as a link home to the Dashboard", async () => {
    render(withTheme(<Header />));
    expect(screen.getByText("Logos")).toBeInTheDocument();
    const link = screen.getByRole("link", { name: /go to Dashboard/i });
    // S-194: Dashboard is now at the root route.
    expect(link).toHaveAttribute("href", "/");
    // The inlined brand mark is decorative SVG inside the same link.
    expect(link.querySelector("svg")).not.toBeNull();
    // Flush the mount-time connectivity probe so its state update is acted-on.
    await screen.findByText("Read-model connected");
  });

  it("navigates client-side to / on click (no full reload)", async () => {
    render(withTheme(<Header />));
    await userEvent.click(screen.getByRole("link", { name: /go to Dashboard/i }));
    // S-194: brand click navigates to root.
    expect(mockNavigate).toHaveBeenCalledWith("/");
    await screen.findByText("Read-model connected");
  });
});
