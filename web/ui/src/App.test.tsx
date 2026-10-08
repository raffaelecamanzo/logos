import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

// Stub every module that would fetch the network or need a real DOM environment
// beyond jsdom. The test focuses solely on the /overview redirect and the
// effectivePath-prevents-flash guarantee.
const { mockRedirect } = vi.hoisted(() => ({ mockRedirect: vi.fn() }));
let mockPathname = "/overview";
// PARTIAL, not whole: the shell calls more of the router than this spec overrides
// (S-426 added `currentUrl`/`replaceUrl`, which a member switch writes the URL
// through). A whole-module mock silently under-supplies the module as it grows.
vi.mock("./router.tsx", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./router.tsx")>()),
  usePathname: () => mockPathname,
  navigate: vi.fn(),
  redirect: mockRedirect,
}));

// Stub the header's health-check fetch and the theme context it needs.
vi.mock("./intent.ts", () => ({ apiGet: vi.fn().mockResolvedValue({}) }));

// S-250: the shell probes the workspace roster once and mounts no view until it
// settles. Answer it as a plain repo (the honest 404), which is this spec's world.
vi.mock("./api/workspaceClient.ts", () => ({
  probeWorkspace: vi.fn().mockResolvedValue({ mode: "single" }),
}));

// Stub every view component so the test doesn't need any data-fetching hooks.
vi.mock("./views/index.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("./views/index.ts")>();
  return {
    ...real,
    // Override viewForPath to return a lightweight stub component for / and
    // /architecture (the `/dsm` redirect's destination).
    viewForPath: (path: string) =>
      path === "/"
        ? () => <div data-testid="dashboard-view" />
        : path === "/architecture"
          ? () => <div data-testid="architecture-view" />
          : real.viewForPath(path),
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
  LoadingState: () => <div data-testid="loading" />,
}));

import { App } from "./App.tsx";

afterEach(() => {
  cleanup();
  mockRedirect.mockClear();
  mockPathname = "/overview";
});

describe("App /overview redirect (S-194)", () => {
  it("renders DashboardView on the first settled frame — no blank-content flash", async () => {
    // rawPathname is /overview; effectivePath alias must resolve it to / synchronously
    // so DashboardView appears as soon as the workspace probe settles (S-250) — with no
    // intervening blank frame.
    render(<App />);
    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
  });

  it("calls redirect('/') when rawPathname is /overview", async () => {
    render(<App />);
    await waitFor(() => expect(mockRedirect).toHaveBeenCalledWith("/"));
  });

  it("does NOT call redirect when pathname is already /", async () => {
    mockPathname = "/";
    render(<App />);
    await screen.findByTestId("dashboard-view");
    expect(mockRedirect).not.toHaveBeenCalled();
  });
});

// The retired `/dsm` bookmark lands on Architecture (CR-038, frontend-design
// §Navigation). S-612 relabels the sidebar entry "Architecture"; the route and
// this redirect are unchanged by it (FR-UI-41).
describe("App /dsm redirect", () => {
  it("renders the Architecture view on the first settled frame", async () => {
    mockPathname = "/dsm";
    render(<App />);
    expect(await screen.findByTestId("architecture-view")).toBeInTheDocument();
  });

  it("calls redirect('/architecture') when rawPathname is /dsm", async () => {
    mockPathname = "/dsm";
    render(<App />);
    await waitFor(() => expect(mockRedirect).toHaveBeenCalledWith("/architecture"));
  });

  it("does NOT call redirect when pathname is already /architecture", async () => {
    mockPathname = "/architecture";
    render(<App />);
    await screen.findByTestId("architecture-view");
    expect(mockRedirect).not.toHaveBeenCalled();
  });
});
