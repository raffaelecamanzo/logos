import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { StatsInfo } from "../api/types.ts";
import { NAV_GROUPS, NAV_ITEMS, WORKSPACE_NAV_ITEMS } from "../nav.ts";
import { Sidebar } from "./Sidebar.tsx";
import styles from "./Sidebar.module.css";
import { WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";

// PARTIAL, not whole: the shell calls more of the router than this spec overrides
// (S-426 added `currentUrl`/`replaceUrl`, which a member switch writes the URL
// through). A whole-module mock silently under-supplies the module as it grows.
vi.mock("../router.tsx", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../router.tsx")>()),
  navigate: vi.fn(),
}));

function stats(callsTotal: number): StatsInfo {
  return {
    window_days: 7,
    calls_total: callsTotal,
    calls_by_tool: [],
    latency_p50_ms: 0,
    latency_p95_ms: 0,
    latency_p99_ms: 0,
    reads_saved_estimate: 0,
    tokens_saved_estimate: 0,
    artifact_bindings: {},
    activity_by_day: [],
    calls_by_origin: [],
    warnings: callsTotal === 0 ? ["no telemetry recorded yet"] : [],
  };
}

function stubStats(callsTotal: number) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) =>
      Promise.resolve({
        // The roster probe 404s: these specs are about the nav, so they run the
        // sidebar in the single-root shell it has always rendered in.
        ok: !url.startsWith("/api/v1/workspace/roster"),
        status: url.startsWith("/api/v1/workspace/roster") ? 404 : 200,
        json: () => Promise.resolve(stats(callsTotal)),
      } as Response),
    ),
  );
}

/** The sidebar inside the workspace context it actually renders in. The Statistics
 *  probe holds until the mode SETTLES (S-426) — it must not read a member before the
 *  shell knows which one — so a bare `<Sidebar/>` sits at the pre-probe `loading`
 *  default forever and the probe never fires. */
const sidebar = (pathname: string) => (
  <WorkspaceProvider>
    <Sidebar pathname={pathname} />
  </WorkspaceProvider>
);

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Sidebar — Statistics nav (S-235, FR-UI-27)", () => {
  it("renders a Statistics item directly above Config", () => {
    stubStats(5);
    render(<Sidebar pathname="/" />);
    const links = screen.getAllByRole("link").map((a) => a.textContent);
    const statsIdx = links.findIndex((t) => t?.includes("Statistics"));
    const configIdx = links.findIndex((t) => t?.includes("Config"));
    expect(statsIdx).toBeGreaterThanOrEqual(0);
    expect(configIdx).toBe(statsIdx + 1);
  });

  it("mutes the Statistics item when the telemetry store is empty (NFR-CC-04)", async () => {
    stubStats(0);
    render(sidebar("/"));
    const link = screen.getByRole("link", { name: /Statistics/ });
    await waitFor(() =>
      expect(link).toHaveAttribute("title", expect.stringMatching(/awaiting data/i)),
    );
  });

  it("does NOT mute the Statistics item when usage has been recorded", async () => {
    stubStats(5);
    render(sidebar("/"));
    const link = screen.getByRole("link", { name: /Statistics/ });
    // Wait until the probe has actually fired and settled — otherwise "no title"
    // could pass merely because the probe is still loading (loading ≠ populated).
    await waitFor(() => expect(fetch).toHaveBeenCalled());
    // Flush the resolved-promise microtask so the ready state has committed.
    await waitFor(() => expect(link).not.toHaveAttribute("title"));
  });
});

// ── S-250 / FR-UI-29 AC4: the workspace tab is workspace-mode ONLY ────────────

/** Render the sidebar inside a provider whose roster probe answers `probeStatus`.
 *  `callsTotal` drives the member-scoped telemetry probe: `0` is the empty store that
 *  mutes the member-scoped Statistics item. */
function mountWithMode(probeStatus: number, callsTotal = 1) {
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      const isProbe = url.startsWith("/api/v1/workspace/roster");
      return Promise.resolve({
        ok: !isProbe || probeStatus === 200,
        status: isProbe ? probeStatus : 200,
        json: () =>
          Promise.resolve(
            isProbe
              ? { workspace: "shop", default: "api", members: ["api", "web"] }
              : stats(callsTotal),
          ),
      } as Response);
    }),
  );
  return render(
    <WorkspaceProvider>
      <Sidebar pathname="/" />
    </WorkspaceProvider>,
  );
}

describe("Sidebar workspace gating (S-250)", () => {
  it("renders NO Workspace item in a single-root serve — the sidebar is unchanged", async () => {
    mountWithMode(404);
    // Wait for the probe to SETTLE before asserting absence; otherwise this passes
    // merely because the shell is still loading.
    await waitFor(() => expect(screen.getByRole("link", { name: /Dashboard/ })).toBeInTheDocument());
    await waitFor(() => expect(screen.queryByRole("link", { name: /^Workspace$/ })).toBeNull());
  });

  it("renders the Workspace item in workspace mode", async () => {
    mountWithMode(200);
    expect(await screen.findByRole("link", { name: /Workspace/ })).toHaveAttribute(
      "href",
      "/workspace",
    );
  });
});

// ── S-425 / FR-UI-35 / ADR-66: the sidebar renders the declared scope ─────────
//
// Every assertion below reads the RENDERED DOM — roles, accessible names, element
// containment — and never component state. The CR-040-era regressions this suite
// exists after were all cases where the state was right and what reached the page
// was not.

/** The section for a scope, by its accessible name. A `<section>` with an
 *  accessible name is an ARIA `region`, so this is the rendered heading talking,
 *  not a class name (which `css: false` would hide from this suite anyway). */
const region = (name: string) => screen.getByRole("region", { name });

describe("Sidebar scope sections (S-425, FR-UI-35, ADR-66)", () => {
  it("renders a Workspace section and a Service section, in that order", async () => {
    const { container } = mountWithMode(200);
    await screen.findByRole("link", { name: /Workspace/ });

    // The nav landmark keeps its name in THIS mode too. The single-root branch is
    // pinned below, and the two branches each write the `<nav>` themselves, so a
    // label or class changed on one of them would otherwise ship green (found in
    // review).
    expect(screen.getByRole("navigation", { name: "Views" })).toBeInTheDocument();
    // Disclosure is class-driven; an inline style on the NEW markup would need
    // `style-src 'unsafe-inline'` and no other spec covers this surface
    // (NFR-SE-06, found in review).
    expect(container.querySelectorAll("[style]")).toHaveLength(0);

    const regions = screen.getAllByRole("region");
    expect(regions.map((r) => r.querySelector("h2")?.textContent)).toEqual([
      "Workspace",
      "Service",
    ]);
  });

  it("files each view under the section its own scope declares", async () => {
    mountWithMode(200);
    await screen.findByRole("link", { name: /Workspace/ });

    const names = (scope: string) =>
      within(region(scope))
        .getAllByRole("link")
        .map((a) => a.textContent);

    // The app-scoped tabs, above the boundary the selector governs…
    expect(names("Workspace")).toEqual(WORKSPACE_NAV_ITEMS.map((i) => i.label));
    // …and every member-scoped tab below it. Same list, same order as the
    // single-root sidebar: the CR-042 A/B/C groups survive INSIDE the section
    // rather than being re-ordered by it.
    expect(names("Service")).toEqual(NAV_ITEMS.map((i) => i.label));
  });

  it("keeps each section's CR-042 groups as separate lists in the markup (S-454)", async () => {
    mountWithMode(200);
    await screen.findByRole("link", { name: /Workspace/ });

    // The Workspace section READS as one list, and that is the stylesheet's doing
    // alone (`.appSection .group`, guarded in `web/tests/spa_design_system.rs`). The
    // grouping itself survives: one `<ul>` per non-empty group, per section — today
    // two in Workspace and three in Service. Derived from the registry, so a new
    // view does not stale it; a component that collapsed the groups does.
    const lists = (scope: string) =>
      within(region(scope))
        .getAllByRole("list")
        .map((ul) => within(ul).getAllByRole("listitem").length);
    const groupSizes = (items: readonly { group: string }[]) =>
      NAV_GROUPS.map((g) => items.filter((i) => i.group === g).length).filter((n) => n > 0);

    expect(lists("Workspace")).toEqual(groupSizes(WORKSPACE_NAV_ITEMS));
    expect(lists("Service")).toEqual(groupSizes(NAV_ITEMS));

    // And the scoped class lands on the Workspace section and ONLY there. Under
    // `css: false` a CSS-Module import is a proxy that names every key, so this
    // reads the rendered class, not the stylesheet: whether the key is DEFINED is
    // the Rust suite's job (`every_module_style_key_a_view_uses_is_defined_in_the_…`).
    // On the Service section it would erase that section's group hairlines.
    expect(region("Workspace").classList).toContain(styles.section);
    expect(region("Workspace").classList).toContain(styles.appSection);
    expect(region("Service").classList).toContain(styles.section);
    expect(region("Service").classList).not.toContain(styles.appSection);
    expect(lists("Workspace")).toHaveLength(2);
    expect(lists("Service")).toHaveLength(3);
  });

  it("renders the member selector in the Service section header and NOWHERE else", async () => {
    mountWithMode(200);
    const select = await screen.findByRole("combobox");

    // Exactly one in the whole tree — not one per section, not a second copy left
    // behind in the app header (asserted from the header's own suite too).
    expect(screen.getAllByRole("combobox")).toHaveLength(1);
    expect(region("Service")).toContainElement(select);
    expect(region("Workspace")).not.toContainElement(select);

    // The heading above it is its accessible name — the control carries no label of
    // its own, so the section label is load-bearing twice over (FR-UI-35).
    expect(select).toHaveAccessibleName("Service");

    // In the HEADER, structurally: outside every nav list, and before the first one.
    expect(select.closest("ul")).toBeNull();
    const section = region("Service");
    const firstList = section.querySelector("ul");
    expect(firstList).not.toBeNull();
    // Exact, not a truthy mask: the list must be CONTAINED BY the section and
    // FOLLOW the select. `toBeTruthy()` on an AND stops discriminating the moment
    // someone widens the mask, and a bare `toBeDefined()` here asserted nothing at
    // all — `compareDocumentPosition` always returns a number (found in review).
    expect(section.compareDocumentPosition(firstList as Node)).toBe(
      Node.DOCUMENT_POSITION_CONTAINED_BY | Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(select.compareDocumentPosition(firstList as Node)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
  });

  it("names the scope in words, so the section is never colour or position alone", async () => {
    mountWithMode(200);
    await screen.findByRole("link", { name: /Workspace/ });

    // Rendered text, in a heading element — the label a 420px viewport still shows
    // and a screen reader still announces (NFR-CC-04). The stylesheet half (that
    // no width rung hides it) is asserted in `web/tests/spa_design_system.rs`.
    const headings = screen.getAllByRole("heading", { level: 2 });
    expect(headings.map((h) => h.textContent)).toEqual(["Workspace", "Service"]);
    for (const h of headings) expect(h).toBeVisible();
  });

  it("renders NO section, no header and no selector in single-root mode", async () => {
    mountWithMode(404);
    await waitFor(() => expect(screen.getByRole("link", { name: /Dashboard/ })).toBeInTheDocument());
    await waitFor(() => expect(screen.queryByRole("link", { name: /^Workspace$/ })).toBeNull());

    // Absent, not hidden and not empty: a plain repository has one scope, so a
    // section label there would assert an axis that does not exist (ADR-66 §5).
    expect(screen.queryAllByRole("region")).toEqual([]);
    expect(screen.queryAllByRole("heading")).toEqual([]);
    expect(screen.queryByText("Service")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
  });

  it("renders NO workspace-only nav item in single-root mode, by name", async () => {
    // The byte-for-byte snapshot below already forbids this structurally — it
    // pins every rendered <a> against NAV_ITEMS alone. This states the same
    // thing about the ITEMS, by name, because that is the clause AC6 is written
    // in ("neither nav item renders") and because a snapshot regenerated in
    // haste can absorb a leak that a named assertion cannot.
    mountWithMode(404);
    await waitFor(() => expect(screen.getByRole("link", { name: /Dashboard/ })).toBeInTheDocument());

    // Keyed on the PATH, never the label: two of these items are called
    // "Dashboard" and "Health" exactly like their member-scoped twins, which
    // ADR-66 chose deliberately — so a label query matches the member-scoped
    // link that SHOULD be there and proves nothing. The route is what separates
    // the levels.
    expect(WORKSPACE_NAV_ITEMS.length).toBeGreaterThan(0);
    const hrefs = [...document.querySelectorAll("a")].map((a) => a.getAttribute("href"));
    expect(hrefs.length).toBeGreaterThan(0);
    for (const item of WORKSPACE_NAV_ITEMS) {
      expect(hrefs).not.toContain(item.path);
    }
  });

  it("renders the single-root sidebar as the exact markup it rendered before S-425", async () => {
    // The byte-for-byte guard (ADR-52, FR-UI-29 AC4). It is a RENDERED snapshot —
    // the form that catches a leak a state-level assertion misses — and it was
    // taken by diffing this tree against the pre-S-425 component, not written from
    // the post-change output (see the implementation notes' baseline diff).
    // CSS-module class names are empty under `css: false`, so what is pinned here
    // is the element structure, the labels, the hrefs and the icons.
    const { container } = mountWithMode(404);
    await waitFor(() => expect(screen.getByRole("link", { name: /Dashboard/ })).toBeInTheDocument());

    const nav = container.querySelector("nav") as HTMLElement;
    expect(nav.getAttribute("aria-label")).toBe("Views");
    // Three group lists, direct children of the nav, and nothing else in it.
    expect([...nav.children].map((c) => c.tagName)).toEqual(NAV_GROUPS.map(() => "UL"));
    // Derived from the registry, not pinned as 6/3/2: a count written out here goes
    // stale the first time a view is added and then asserts the wrong thing quietly.
    expect([...nav.children].map((c) => c.children.length)).toEqual(
      NAV_GROUPS.map((g) => NAV_ITEMS.filter((i) => i.group === g).length),
    );
    expect(
      [...nav.querySelectorAll("a")].map((a) => [a.getAttribute("href"), a.textContent]),
    ).toEqual(NAV_ITEMS.map((i) => [i.path, i.label]));
    // Every item still carries its inline-SVG icon (CR-042).
    expect(nav.querySelectorAll("li > a > span:first-child > svg")).toHaveLength(NAV_ITEMS.length);
  });
});

// ── S-429 / FR-UI-37: the two invariants the app-scoped Statistics tab added ───
//
// Both were found by a review agent's mutations, and both were completely untested:
// nothing in the SPA tree asserted the muting behaviour at all, and the icon count
// covered only the member roster.

describe("the app-scoped Statistics tab (S-429, FR-UI-37)", () => {
  it("mutes the MEMBER-scoped Statistics item on an empty store and never the app-scoped one", async () => {
    // The muting probe reads `/api/v1/statistics` at the SELECTED MEMBER's scope, so
    // muting the workspace tab from it would assert one member's emptiness about the
    // whole workspace. `Sidebar.tsx` therefore matches `v.id === "statistics"` by
    // EXACT equality — and rewriting that as `v.id.endsWith("statistics")`, the exact
    // untruth the comment there says was avoided, left all 827 specs green.
    mountWithMode(200, 0);
    await screen.findByRole("link", { name: /^Workspace$/ });

    // Two links read "Statistics" — one per scope — so each is located by its section.
    const member = within(region("Service")).getByRole("link", { name: /Statistics/ });
    const app = within(region("Workspace")).getByRole("link", { name: /Statistics/ });

    await waitFor(() =>
      expect(member).toHaveAttribute("title", expect.stringMatching(/awaiting data/i)),
    );
    // The app-scoped tab answers for a different population, so it is not muted by
    // this probe — and the assertion is anchored on the member-scoped item above
    // having ALREADY been muted, so it cannot pass merely because nothing settled.
    expect(app).not.toHaveAttribute("title");
  });

  it("gives every Workspace-section item its inline-SVG icon, not just the member ones", async () => {
    // `NavLink` renders `{Icon && <Icon />}`, so a missing `ICONS` entry degrades to an
    // empty `<span>`. The standing icon guard counts against `NAV_ITEMS.length` — the
    // member roster only — so deleting the new `"workspace-statistics": IconStatistics`
    // entry left the suite green, as it would for either S-428 entry. This is the
    // "a story enumerates a surface a later story extends" shape, so the count is
    // derived per section rather than written out.
    mountWithMode(200);
    await screen.findByRole("link", { name: /^Workspace$/ });

    for (const [scope, roster] of [
      ["Workspace", WORKSPACE_NAV_ITEMS],
      ["Service", NAV_ITEMS],
    ] as const) {
      expect(roster.length).toBeGreaterThan(0);
      expect(
        region(scope).querySelectorAll("li > a > span:first-child > svg"),
      ).toHaveLength(roster.length);
    }
  });
});
