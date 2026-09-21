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

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { App } from "./App.tsx";
import { redirect } from "./router.tsx";
import { ThemeProvider } from "./theme/ThemeProvider.tsx";
import { scopedMember, setScopedMember } from "./workspace/scope.ts";
import { ROSTER, stubApi } from "./workspace/testFixtures.ts";

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
    viewForPath: (path: string) => (path === "/workspace" ? AppProbeView : MemberProbeView),
  };
});

// A PARTIAL mock: only the three entry points that would drive the SPA's own
// navigation are stubbed, so a spec can pin the pathname without the shell
// navigating under it. `currentUrl`/`replaceUrl` are the REAL ones (S-426) — they
// are the seam the member switch writes the URL through, and a stub would make the
// URL assertions below pin the stub instead of the behaviour.
const { pathname } = vi.hoisted(() => ({ pathname: { current: "/" } }));
vi.mock("./router.tsx", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./router.tsx")>()),
  usePathname: () => pathname.current,
  navigate: vi.fn(),
  redirect: vi.fn(),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
  pathname.current = "/";
  // The URL is shared state across specs now that the shell writes to it.
  window.history.replaceState(null, "", "/");
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
    expect(await screen.findByRole("link", { name: /^Dashboard$/ })).toHaveAttribute("href", "/");
  });
});

// ── S-426 / FR-UI-35 / NFR-RA-05: the unknown member is refused on the DOM ──

/** Open the SPA on `url` — what a bookmark, a shared link or a refresh supplies. */
function openAt(url: string) {
  window.history.replaceState(null, "", url);
}

describe("the /overview migration keeps the member (S-426, S-194)", () => {
  // `redirect` is stubbed for the specs above, which is precisely why this was
  // invisible: the one suite that drives the real shell with a roster never ran the
  // real migration. These use the real one.
  const realRouter = async () => await vi.importActual<typeof import("./router.tsx")>("./router.tsx");

  it("carries a deep-linked ?repo= across /overview → /", async () => {
    const { redirect: realRedirect } = await realRouter();
    pathname.current = "/overview";
    openAt("/overview?repo=web");
    const calls = stubApi();
    vi.mocked(redirect).mockImplementation((path: string) => {
      realRedirect(path);
      pathname.current = "/";
    });
    render(app());

    await waitFor(() => expect(screen.getByTestId("view")).toBeInTheDocument());
    // The migration rewrites the PATH. Dropping the query with it would open the
    // manifest default and paint ITS figures for a URL that asked for `web`.
    expect(viewCalls(calls())).toEqual(["/api/v1/overview?repo=web"]);
    expect(scopedMember()).toBe("web");
  });

  it("carries an UNKNOWN ?repo= across it too, so the refusal is not bypassed", async () => {
    const { redirect: realRedirect } = await realRouter();
    pathname.current = "/overview";
    openAt("/overview?repo=ghost");
    const calls = stubApi();
    vi.mocked(redirect).mockImplementation((path: string) => {
      realRedirect(path);
      pathname.current = "/";
    });
    render(app());

    // Losing the member here would resolve `ghost` to the default member with
    // nothing on screen admitting it — the 200-shaped page NFR-RA-05 forbids.
    expect(await screen.findByRole("alert")).toHaveTextContent(/No workspace member/);
    expect(screen.queryByTestId("view")).toBeNull();
    expect(viewCalls(calls())).toEqual([]);
  });
});

describe("a deep-linked member scopes the FIRST read (S-426, FR-UI-35)", () => {
  it("issues exactly ONE read, already scoped to the URL's member", async () => {
    // The property, counted rather than inferred: one read, carrying `web`. A
    // provider that opened on the manifest default and then corrected itself to the
    // URL's member would remount the view and leave TWO reads here, the first of
    // them against a member the user never asked for — and it would settle in an
    // identical final state, which is why no state assertion can see it.
    openAt("/?repo=web");
    const calls = stubApi();
    render(app());

    await waitFor(() => expect(screen.getByTestId("view")).toBeInTheDocument());
    expect(viewCalls(calls())).toEqual(["/api/v1/overview?repo=web"]);
    // Nothing at all went out under the default member, from any caller.
    expect(calls().some((u) => u.includes("repo=api"))).toBe(false);
  });
});

describe("an unknown ?repo= renders a refusal and NO view (NFR-RA-05)", () => {
  it("names the members the workspace has, while no view renders any figures", async () => {
    // The failure being prevented is a 200-shaped page: the shell falls back to the
    // default member, the view paints ITS figures, and the URL still says `ghost`.
    // Asserted on rendered DOM, because that page passes every state assertion.
    openAt("/?repo=ghost");
    const calls = stubApi();
    render(app());

    expect(await screen.findByRole("alert")).toHaveTextContent(/No workspace member\s+ghost/);
    // The members it DOES have, named — a dead end that lists the way out.
    for (const name of ROSTER.members) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    // No view is mounted at all…
    expect(screen.queryByTestId("view")).toBeNull();
    // …so no member's figures were even fetched, under `ghost`'s name or any other.
    expect(viewCalls(calls())).toEqual([]);
    expect(scopedMember()).toBeNull();
    // Nor the header badge, which sits OUTSIDE the view subtree and reads
    // `/api/v1/status`: unscoped, that answers from the default member and would put
    // its counts inches from the name the user asked for.
    expect(screen.getByText("Connecting…")).toBeInTheDocument();
    expect(calls().some((u) => u.startsWith("/api/v1/status"))).toBe(false);
  });

  it("refuses an APP-level path too — the claim is the URL's, not the view's reads", async () => {
    // `/workspace` is app-scoped (ADR-66): its fan-out is never member-scoped, so it
    // could render correct figures here. It is refused anyway, and this pins that
    // decision — without it, narrowing the gate to member-scoped views would be a
    // silent behaviour change that no spec noticed.
    pathname.current = "/workspace";
    openAt("/workspace?repo=ghost");
    const calls = stubApi();
    render(app());

    expect(await screen.findByRole("alert")).toHaveTextContent(/No workspace member/);
    expect(screen.queryByTestId("view")).toBeNull();
    expect(appViewCalls(calls())).toEqual([]);
  });

  it("recovers: picking a named member mounts the view, scoped to THAT member", async () => {
    openAt("/?repo=ghost");
    const calls = stubApi();
    render(app());
    await screen.findByRole("alert");

    await userEvent.click(screen.getByRole("button", { name: "web" }));

    await waitFor(() => expect(screen.getByTestId("view")).toBeInTheDocument());
    expect(viewCalls(calls())).toEqual(["/api/v1/overview?repo=web"]);
    expect(window.location.search).toBe("?repo=web");
  });

  it("an unstartable member is the read's 500, a DIFFERENT state from the refusal", async () => {
    // `member.rs` refuses to conflate "no such member" (404) with "that member's
    // engine would not start" (500), and the client must not re-merge them: the two
    // send the user to different places — a typo, or a broken store. Two fixtures,
    // two rendered states; this is the second.
    openAt("/?repo=web");
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url.startsWith("/api/v1/workspace/roster")) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: () => Promise.resolve(ROSTER),
          } as Response);
        }
        // The member IS in the roster; its engine is what fails.
        return Promise.resolve({
          ok: false,
          status: 500,
          json: () =>
            Promise.resolve({ error: "workspace member `web` could not be started: locked" }),
        } as Response);
      }),
    );
    render(app());

    // The view mounted and reported the failure IT saw…
    await waitFor(() => expect(screen.getByTestId("view")).toHaveTextContent("error"));
    // …and the unknown-member refusal is nowhere on the page.
    expect(screen.queryByText(/No workspace member/)).toBeNull();
    // The member stayed selected: it exists, so it is still what the shell presents.
    expect(scopedMember()).toBe("web");
  });
});
