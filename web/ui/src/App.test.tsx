import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

// Stub every module that would fetch the network or need a real DOM environment
// beyond jsdom. The test focuses solely on the /overview redirect and the
// effectivePath-prevents-flash guarantee.
const { mockRedirect } = vi.hoisted(() => ({ mockRedirect: vi.fn() }));
let mockPathname = "/overview";
vi.mock("./router.tsx", () => ({
  usePathname: () => mockPathname,
  navigate: vi.fn(),
  redirect: mockRedirect,
}));

// Stub the header's health-check fetch and the theme context it needs.
vi.mock("./intent.ts", () => ({ apiGet: vi.fn().mockResolvedValue({}) }));

// Stub every view component so the test doesn't need any data-fetching hooks.
vi.mock("./views/index.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("./views/index.ts")>();
  return {
    ...real,
    // Override viewForPath to return a lightweight stub component for /
    viewForPath: (path: string) =>
      path === "/" ? () => <div data-testid="dashboard-view" /> : real.viewForPath(path),
  };
});

// Stub sidebar and header to keep the render tree minimal.
vi.mock("./shell/Sidebar.tsx", () => ({
  Sidebar: () => <nav data-testid="sidebar" />,
}));
vi.mock("./shell/Header.tsx", () => ({
  Header: () => <header data-testid="header" />,
}));

// AppShell just renders children; stub it so we don't need CSS modules.
vi.mock("./components/index.ts", () => ({
  AppShell: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  ToastProvider: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import { App } from "./App.tsx";

afterEach(() => {
  cleanup();
  mockRedirect.mockClear();
  mockPathname = "/overview";
});

describe("App /overview redirect (S-194)", () => {
  it("renders DashboardView on the first frame — no blank-content flash", () => {
    // rawPathname is /overview; effectivePath alias must resolve it to / synchronously
    // so DashboardView appears without waiting for the useEffect.
    render(<App />);
    expect(screen.getByTestId("dashboard-view")).toBeInTheDocument();
  });

  it("calls redirect('/') when rawPathname is /overview", () => {
    render(<App />);
    expect(mockRedirect).toHaveBeenCalledWith("/");
  });

  it("does NOT call redirect when pathname is already /", () => {
    mockPathname = "/";
    render(<App />);
    expect(mockRedirect).not.toHaveBeenCalled();
  });
});
