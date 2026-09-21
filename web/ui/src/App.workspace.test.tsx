/*
 * The cache-key contract (S-250, FR-UI-29 AC1): "switching members re-fetches —
 * member is part of the cache key".
 *
 * This is the AC that is easy to *look* covered and not be. Asserting that the
 * transport scope moved, or that a manually-invoked fetch then carried `?repo=`,
 * pins `scope.ts` — not the remount. The behaviour the user actually gets is: an
 * ALREADY-MOUNTED view re-runs its reads when the member changes. That property
 * lives in one line of `App.tsx` (`<View key={cacheKey} />`), and this spec is the
 * thing that fails if that line is deleted.
 */

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App.tsx";
import { WORKSPACE_NAV_ITEMS } from "./nav.ts";
import { ThemeProvider } from "./theme/ThemeProvider.tsx";
import { scopedMember, setScopedMember } from "./workspace/scope.ts";
import { stubApi } from "./workspace/testFixtures.ts";

/** The real shell is rendered — sidebar included, which is where S-425 put the
 *  member selector — so it needs the theme context `main.tsx` provides in
 *  production. */
const app = () => (
  <ThemeProvider>
    <App />
  </ThemeProvider>
);

// Two stand-ins for real views, one per scope. Each reads exactly as the views of
// its scope do — through `useApiResource`, whose deps do NOT mention the member: the
// whole point is that the shell, not the view, owns the cache key.
//
//   member-scoped → `/api/v1/overview`, which `apiUrl` scopes with `?repo=`
//   app-scoped    → `/api/v1/workspace/status`, the unscoped fan-out, identical for
//                   every member
//
// `overview`, not `health`: the Header runs its own connectivity probe against
// `/api/v1/health`, and these specs are about the VIEW's reads.
vi.mock("./views/index.ts", async () => {
  const { useApiResource } = await import("./api/hooks.tsx");
  const { fetchOverview } = await import("./api/client.ts");
  const { fetchWorkspaceStatus } = await import("./api/workspaceClient.ts");
  function MemberProbeView() {
    const overview = useApiResource(() => fetchOverview(), []);
    return <div data-testid="view">{overview.status}</div>;
  }
  function AppProbeView() {
    const status = useApiResource(() => fetchWorkspaceStatus(), []);
    return <div data-testid="view">{status.status}</div>;
  }
  return {
    viewForPath: (path: string) => (APP_ROUTES.includes(path) ? AppProbeView : MemberProbeView),
  };
});

/** Every `app`-scoped client route, spelled out.
 *
 *  Deliberately a literal list rather than a filter over `WORKSPACE_NAV_ITEMS`:
 *  the shell decides the mount key from the SAME `scope` field that filter would
 *  read, so deriving it here would let one mutation move both sides at once. The
 *  price is that a fourth app-scoped view must be added here by hand — which the
 *  floor assertion below turns into a failing test rather than a silent gap. */
const { APP_ROUTES, pathname } = vi.hoisted(() => ({
  APP_ROUTES: ["/workspace", "/workspace-dashboard", "/workspace-health"] as string[],
  pathname: { current: "/" },
}));


vi.mock("./router.tsx", () => ({
  usePathname: () => pathname.current,
  navigate: vi.fn(),
  redirect: vi.fn(),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
  pathname.current = "/";
});

/** Every read the mounted MEMBER-scoped view issued (the Header probes `status`
 *  separately, and the provider probes the roster). */
const viewCalls = (calls: string[]) => calls.filter((u) => u.startsWith("/api/v1/overview"));

/** Every read the mounted APP-scoped view issued. The provider's own probe hits
 *  `/api/v1/workspace/roster`, which this deliberately does not match. */
const appViewCalls = (calls: string[]) =>
  calls.filter((u) => u.startsWith("/api/v1/workspace/status"));

describe("the member is part of the cache key (FR-UI-29 AC1)", () => {
  it("re-fetches an already-mounted view when the member changes", async () => {
    const calls = stubApi();
    render(app());

    // The view mounts only once the probe has settled, and its FIRST read is already
    // scoped — no unscoped pre-fetch against a member the user never chose.
    await waitFor(() => expect(screen.getByTestId("view")).toBeInTheDocument());
    await waitFor(() => expect(viewCalls(calls())).toEqual(["/api/v1/overview?repo=api"]));

    // Switch the member in the shell selector.
    await userEvent.selectOptions(await screen.findByRole("combobox"), "web");

    // The view re-fetches — nobody re-invoked it; the key remounted it.
    await waitFor(() =>
      expect(viewCalls(calls())).toEqual([
        "/api/v1/overview?repo=api",
        "/api/v1/overview?repo=web",
      ]),
    );
  });

  it("never re-fetches in single-root mode — the key is constant, so nothing remounts", async () => {
    const calls = stubApi({ probeStatus: 404 });
    render(app());
    await waitFor(() => expect(screen.getByTestId("view")).toBeInTheDocument());
    await waitFor(() => expect(viewCalls(calls()).length).toBe(1));

    // The one read carries NO `?repo=` — byte-for-byte the pre-workspace request.
    expect(viewCalls(calls())).toEqual(["/api/v1/overview"]);
    // And no selector was rendered at all.
    expect(screen.queryByRole("combobox")).toBeNull();
  });
});

// ── S-425 / FR-UI-35 / ADR-66: the remount invariant reads the declared scope ──

describe("a member switch remounts the member-scoped views and no app-scoped one", () => {
  it("re-fetches the member-scoped view — counted, not inferred", async () => {
    pathname.current = "/";
    const calls = stubApi();
    render(app());
    await waitFor(() => expect(viewCalls(calls())).toEqual(["/api/v1/overview?repo=api"]));

    await userEvent.selectOptions(await screen.findByRole("combobox"), "web");

    await waitFor(() =>
      expect(viewCalls(calls())).toEqual([
        "/api/v1/overview?repo=api",
        "/api/v1/overview?repo=web",
      ]),
    );
  });

  it("has an app-scoped route registered for every one this spec drives", () => {
    // The floor: `it.each([])` registers ZERO tests and raises nothing, so an
    // empty roster would make the parameterised spec below VANISH rather than
    // fail. It also catches the reverse — a route declared `app` in `nav.ts` that
    // this spec never drives — which is how a new app-level view would silently
    // escape the remount invariant.
    expect(APP_ROUTES.length).toBeGreaterThan(0);
    expect([...APP_ROUTES].sort()).toEqual(
      WORKSPACE_NAV_ITEMS.filter((i) => i.scope === "app")
        .map((i) => i.path)
        .sort(),
    );
  });

  it.each(APP_ROUTES)(
    "does NOT re-fetch the app-scoped view at %s — its data is the same for every member",
    async (route) => {
      pathname.current = route;
      const calls = stubApi();
      render(app());
      await waitFor(() => expect(appViewCalls(calls())).toHaveLength(1));
      expect(appViewCalls(calls())).toEqual(["/api/v1/workspace/status"]);

      await userEvent.selectOptions(await screen.findByRole("combobox"), "web");

      // The switch really happened — the transport moved…
      await waitFor(() => expect(scopedMember()).toBe("web"));
      // …and a member-scoped read re-fired, so this is not a test in which
      // nothing at all re-rendered.
      await waitFor(() =>
        expect(calls().some((u) => u.includes("/api/v1/status?repo=web"))).toBe(true),
      );
      // …while the app-scoped view stayed mounted and read nothing more.
      expect(appViewCalls(calls())).toEqual(["/api/v1/workspace/status"]);
    },
  );

  it("does NOT re-fetch the app-scoped view — its data is the same for every member", async () => {
    // The whole point of the declared scope. Re-keying this view on the member would
    // tear the ECharts canvas down and re-run the fan-out every time the user clicks
    // a service in the map — which SELECTS that member — for data byte-identical to
    // what is already on screen.
    pathname.current = "/workspace";
    const calls = stubApi();
    render(app());
    await waitFor(() => expect(appViewCalls(calls())).toHaveLength(1));
    // Unscoped: an app-level read carries no `?repo=` (ADR-01, FR-UI-29).
    expect(appViewCalls(calls())).toEqual(["/api/v1/workspace/status"]);

    await userEvent.selectOptions(await screen.findByRole("combobox"), "web");

    // The switch really happened — the transport moved…
    await waitFor(() => expect(scopedMember()).toBe("web"));
    // …and the header's own member-scoped read re-fired, so this is not a test in
    // which nothing at all re-rendered.
    await waitFor(() =>
      expect(calls().some((u) => u.includes("/api/v1/status?repo=web"))).toBe(true),
    );
    // …while the app-scoped view stayed mounted and read nothing more.
    expect(appViewCalls(calls())).toEqual(["/api/v1/workspace/status"]);
  });

  it("keys `/` to the per-member Dashboard in workspace mode, not to an app-level one", async () => {
    // The route deliberately does not change meaning in a workspace (ADR-66: no
    // existing bookmark is re-homed), so `/` must still be scoped by the member —
    // shown here by the read it issues carrying the member, and by the sidebar
    // filing it under the Service section (`Sidebar.test.tsx`).
    pathname.current = "/";
    const calls = stubApi();
    render(app());
    await waitFor(() => expect(viewCalls(calls())).toEqual(["/api/v1/overview?repo=api"]));
    // Scoped to the SERVICE section: since S-428 there are two links named
    // "Dashboard" in a workspace sidebar — one per level — and the assertion this
    // spec makes is about the member-scoped one keeping `/`.
    const service = screen.getByRole("region", { name: "Service" });
    expect(await within(service).findByRole("link", { name: /^Dashboard$/ })).toHaveAttribute(
      "href",
      "/",
    );
    // …and the app-level one is a different route, so `/` did not move (ADR-66).
    const workspace = screen.getByRole("region", { name: "Workspace" });
    expect(within(workspace).getByRole("link", { name: /^Dashboard$/ })).toHaveAttribute(
      "href",
      "/workspace-dashboard",
    );
  });
});
