/*
 * The shell mounts extension navigation and views only for extensions the server
 * reports active (S-642, FR-UI-51, CR-213).
 *
 * The REAL shell is rendered — sidebar, workspace provider, view lookup — with the
 * stub extension registered as code and the boot read's answer set directly on the
 * registry (`bootExtensions` has its own spec). Only the Dashboard component is a
 * stand-in, because the landing under test is its route, not its content.
 */

import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App.tsx";
import { clearExtensions, registerExtensionUi, setActiveExtensions } from "./extensions/registry.ts";
import { redirect } from "./router.tsx";
import { extensionsInfo, STUB_NAV, STUB_UI, stubInfo } from "./test/extensions.tsx";
import { ThemeProvider } from "./theme/ThemeProvider.tsx";
import { setScopedMember } from "./workspace/scope.ts";
import { stubApi } from "./workspace/testFixtures.ts";

const { pathname } = vi.hoisted(() => ({ pathname: { current: "/" } }));
vi.mock("./router.tsx", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./router.tsx")>()),
  usePathname: () => pathname.current,
  navigate: vi.fn(),
  redirect: vi.fn(),
}));

// The REAL lookup, with the Dashboard (the landing) swapped for a stand-in.
vi.mock("./views/index.ts", async (importOriginal) => {
  const real = await importOriginal<typeof import("./views/index.ts")>();
  return {
    ...real,
    viewForPath: (path: string) =>
      path === "/" ? () => <div data-testid="dashboard-view" /> : real.viewForPath(path),
  };
});

beforeEach(() => {
  pathname.current = "/";
});

afterEach(() => {
  cleanup();
  clearExtensions();
  vi.unstubAllGlobals();
  vi.mocked(redirect).mockClear();
  setScopedMember(null);
  window.history.replaceState(null, "", "/");
});

const app = () => (
  <ThemeProvider>
    <App />
  </ThemeProvider>
);

const sidebar = () => screen.getByRole("navigation", { name: "Views" });

describe("an extension the server reports active", () => {
  it("shows its nav entry with its icon and mounts its view at its route", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    pathname.current = "/stub";
    render(app());

    expect(await screen.findByTestId("stub-view")).toBeInTheDocument();
    const link = within(sidebar()).getByRole("link", { name: "Stub" });
    expect(link).toHaveAttribute("href", "/stub");
    expect(link).toHaveAttribute("aria-current", "page");
    expect(within(link).getByTestId("stub-icon")).toBeInTheDocument();
    expect(redirect).not.toHaveBeenCalled();
  });

  it("is listed in the sidebar of a workspace too, when member-scoped", async () => {
    stubApi();
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    render(app());

    // The sidebar element itself is replaced when the probe flips the mode, so it is
    // looked up again on every attempt rather than held.
    await waitFor(() => expect(within(sidebar()).getByRole("link", { name: "Stub" })).toBeInTheDocument());
    // And it is a workspace sidebar: the Service section holds the member entries.
    expect(within(sidebar()).getByRole("heading", { name: "Service" })).toBeInTheDocument();
  });

  it("lists an app-scoped entry only in a workspace", async () => {
    const appNav = { ...STUB_NAV, id: "stub-app", label: "Stub app", path: "/stub-app", scope: "app" };
    registerExtensionUi(STUB_UI);
    setActiveExtensions(
      extensionsInfo(stubInfo({ ui: { nav: [appNav], views: [], config_panels: [] } })),
    );

    stubApi({ probeStatus: 404 });
    const single = render(app());
    await screen.findByTestId("dashboard-view");
    expect(within(sidebar()).queryByRole("link", { name: "Stub app" })).toBeNull();
    single.unmount();

    stubApi();
    render(app());
    await waitFor(() =>
      expect(within(sidebar()).getByRole("link", { name: "Stub app" })).toBeInTheDocument(),
    );
  });
});

describe("an extension the server does not report active", () => {
  it("shows no nav entry, and its route lands on the Dashboard", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo());
    pathname.current = "/stub";
    window.history.replaceState(null, "", "/stub?q=1#top");
    render(app());

    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
    expect(screen.queryByTestId("stub-view")).toBeNull();
    expect(within(sidebar()).queryByRole("link", { name: "Stub" })).toBeNull();
    // The silent redirect a hidden view gets, query and fragment carried across.
    await waitFor(() => expect(redirect).toHaveBeenCalledWith("/?q=1#top"));
  });

  it("lands a sub-route of its view on the Dashboard too", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo());
    pathname.current = "/stub/page/x";
    render(app());

    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
    await waitFor(() => expect(redirect).toHaveBeenCalledWith("/"));
  });

  it("lands on the Dashboard when the boot read failed", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    setActiveExtensions(null);
    pathname.current = "/stub";
    render(app());

    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
    expect(screen.queryByTestId("stub-view")).toBeNull();
  });
});

describe("until the boot read settles", () => {
  it("holds an extension's route rather than mounting it or bouncing it, then mounts it when active", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    pathname.current = "/stub";
    render(app());

    expect(await screen.findByText("Starting…")).toBeInTheDocument();
    expect(screen.queryByTestId("stub-view")).toBeNull();
    expect(redirect).not.toHaveBeenCalled();

    act(() => setActiveExtensions(extensionsInfo(stubInfo())));
    expect(await screen.findByTestId("stub-view")).toBeInTheDocument();
    expect(redirect).not.toHaveBeenCalled();
  });

  it("lands on the Dashboard when the read then reports the extension inactive", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    pathname.current = "/stub";
    render(app());
    await screen.findByText("Starting…");

    act(() => setActiveExtensions(extensionsInfo()));
    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
  });

  it("never holds a built-in route", async () => {
    stubApi({ probeStatus: 404 });
    registerExtensionUi(STUB_UI);
    render(app());
    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
  });
});

describe("with no extension active — a build without extensions (S-643, S-644, S-651)", () => {
  it("leaves the shell built-in: an unowned route lands on the Dashboard, and no Chat or Wiki is offered", async () => {
    stubApi({ probeStatus: 404 });
    pathname.current = "/stub";
    render(app());

    // Not held: the read is still `loading`, and nothing waits on it — the route is
    // served by no view, so it lands where a hidden view does.
    expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
    expect(screen.queryByText("Starting…")).toBeNull();
    expect(screen.queryByTestId("stub-view")).toBeNull();
    await waitFor(() => expect(redirect).toHaveBeenCalledWith("/"));
    expect(within(sidebar()).getAllByRole("link").map((a) => a.textContent)).toEqual([
      "Dashboard",
      "Quality",
      "Security",
      "Coverage",
      "Graph",
      "Rule findings",
      "Files & Risk",
      "Statistics",
      "Config",
    ]);
  });

  // FR-UI-51 AC2: a direct visit to a route of an extension this build does not
  // compose lands on the Dashboard — an old bookmark does not open an empty page.
  for (const route of ["/chat", "/wiki", "/workspace-chat"]) {
    it(`lands a direct visit to ${route} on the Dashboard, query and fragment kept`, async () => {
      stubApi({ probeStatus: 404 });
      setActiveExtensions(extensionsInfo());
      pathname.current = route;
      window.history.replaceState(null, "", `${route}?q=1#top`);
      render(app());

      expect(await screen.findByTestId("dashboard-view")).toBeInTheDocument();
      await waitFor(() => expect(redirect).toHaveBeenCalledWith("/?q=1#top"));
    });
  }
});
