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
import { ThemeProvider } from "./theme/ThemeProvider.tsx";
import { setScopedMember } from "./workspace/scope.ts";
import { stubApi } from "./workspace/testFixtures.ts";

/** The real shell is rendered (the selector lives in the Header), so it needs the
 *  theme context `main.tsx` provides in production. */
const app = () => (
  <ThemeProvider>
    <App />
  </ThemeProvider>
);

// A stand-in for a real view: it reads a member-scoped read-model exactly as every
// migrated view does (through `useApiResource`, whose deps do NOT mention the member —
// the whole point is that the shell, not the view, owns the cache key).
vi.mock("./views/index.ts", async () => {
  const { useApiResource } = await import("./api/hooks.tsx");
  const { fetchOverview } = await import("./api/client.ts");
  function ProbeView() {
    // `overview`, not `health`: the Header runs its own connectivity probe against
    // `/api/v1/health`, and this spec is about the VIEW's reads.
    const overview = useApiResource(() => fetchOverview(), []);
    return <div data-testid="view">{overview.status}</div>;
  }
  return { viewForPath: () => ProbeView };
});

vi.mock("./router.tsx", () => ({
  usePathname: () => "/",
  navigate: vi.fn(),
  redirect: vi.fn(),
}));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

/** Every read the mounted VIEW issued (the Header probes `health` separately). */
const viewCalls = (calls: string[]) => calls.filter((u) => u.startsWith("/api/v1/overview"));

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
