import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { ReactNode } from "react";

import type { StatusInfo } from "../api/types.ts";
import { apiGet } from "../intent.ts";
import { ThemeContext } from "../theme/theme.ts";
import { WorkspaceProvider } from "../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../workspace/scope.ts";
import { Header } from "./Header.tsx";

// The header reads `/api/v1/status` on mount and on navigation (S-315, FR-UI-34).
// Stub the same-origin fetch seam rather than `api/client.ts`, so the URL the header
// actually requests — including the `?repo=` member scope `apiUrl` appends — is
// asserted here rather than mocked away. `ApiError` must be part of the mock:
// `api/client.ts` re-exports it from this module.
vi.mock("../intent.ts", () => ({
  apiGet: vi.fn(),
  ApiError: class ApiError extends Error {},
}));

// The header's read waits for the workspace mode to settle (until then the member
// scope is unknown, so a request would go out unscoped and be re-issued). Each spec
// sets the probe's answer: a plain repo by default, a two-member roster for the
// member-switch case.
const { mockNavigate, pathname, mockProbe } = vi.hoisted(() => ({
  mockNavigate: vi.fn(),
  pathname: { current: "/" },
  mockProbe: vi.fn(),
}));
vi.mock("../api/workspaceClient.ts", () => ({ probeWorkspace: mockProbe }));
// Spy on the client router so we can assert the brand navigates without a reload,
// and drive the pathname the header joins to its read's dependencies.
vi.mock("../router.tsx", () => ({
  navigate: mockNavigate,
  usePathname: () => pathname.current,
}));

const get = vi.mocked(apiGet);

/** An indexed graph, as `GET /api/v1/status` serializes `StatusInfo`. */
function indexed(overrides: Partial<StatusInfo> = {}): StatusInfo {
  return {
    indexed: true,
    file_count: 118,
    node_count: 12_345,
    edge_count: 6_789,
    db_path: "/tmp/p/.logos/logos.db",
    db_size_bytes: 4_096,
    last_full_index_at: "1750000000",
    last_sync_at: "1750000600",
    graph_revision: 42,
    refs_total: 0,
    refs_resolved: 0,
    refs_unresolved: 0,
    resolution_coverage: 1,
    total_line_count: null,
    source_line_count: null,
    test_line_count: null,
    freshness: "fresh",
    warnings: [],
    ...overrides,
  };
}

/** The readout the header renders for `status` — the assertion's expected text. */
function readout(status: StatusInfo): string {
  return `rev ${status.graph_revision.toLocaleString("en-US")} · ${status.node_count.toLocaleString(
    "en-US",
  )} nodes · ${status.edge_count.toLocaleString("en-US")} edges`;
}

/** The URLs the header actually requested, in order. */
function urls(): string[] {
  return get.mock.calls.map((c) => c[0] as string);
}

// The header hosts the ThemeToggle, which reads the theme context; give it a
// static value so the toggle mounts without the full provider tree.
function withTheme(node: ReactNode) {
  return (
    <ThemeContext.Provider value={{ theme: "dark", setTheme: () => {}, toggleTheme: () => {} }}>
      {/* The header reads the workspace mode (it hosts the member selector, and its own
          read is member-scoped), so it needs the provider App gives it in production. */}
      <WorkspaceProvider>{node}</WorkspaceProvider>
    </ThemeContext.Provider>
  );
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  mockNavigate.mockClear();
  get.mockReset();
  mockProbe.mockReset();
  setScopedMember(null);
  pathname.current = "/";
});

/** The default world: a plain repo serving an indexed graph. */
function singleRoot(status: StatusInfo = indexed()) {
  mockProbe.mockResolvedValue({ mode: "single" });
  get.mockResolvedValue(status);
  return status;
}

describe("Header brand lockup", () => {
  it("renders the brand mark + wordmark as a link home to the Dashboard", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));
    expect(screen.getByText("Logos")).toBeInTheDocument();
    const link = screen.getByRole("link", { name: /go to Dashboard/i });
    // S-194: Dashboard is now at the root route.
    expect(link).toHaveAttribute("href", "/");
    // The inlined brand mark is decorative SVG inside the same link.
    expect(link.querySelector("svg")).not.toBeNull();
    // Flush the mount-time status read so its state update is acted-on.
    await screen.findByText(readout(status));
  });

  it("navigates client-side to / on click (no full reload)", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));
    await userEvent.click(screen.getByRole("link", { name: /go to Dashboard/i }));
    // S-194: brand click navigates to root.
    expect(mockNavigate).toHaveBeenCalledWith("/");
    await screen.findByText(readout(status));
  });
});

describe("Header graph-state readout (S-315, FR-UI-34, CR-097)", () => {
  it("renders rev · nodes · edges from the status read-model, thousands-separated", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));

    expect(await screen.findByText("rev 42 · 12,345 nodes · 6,789 edges")).toBeInTheDocument();
    expect(screen.getByText(readout(status))).toBeInTheDocument();
  });

  it("reads the right-sized status endpoint, never the ~1MB Health bundle", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));
    await screen.findByText(readout(status));

    expect(urls()).toEqual(["/api/v1/status"]);
    expect(urls().some((u) => u.includes("/health"))).toBe(false);
  });

  it("has retired the green connectivity badge", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));
    await screen.findByText(readout(status));

    expect(screen.queryByText(/read-model connected/i)).toBeNull();
  });

  it("is silent while idle and advances on navigation — the two halves together", async () => {
    // Fake timers are installed BEFORE mount on purpose: a poll registered during the
    // effect must be a FAKE timer for the idle advance below to be able to fire it.
    // Installing them after render would make this assertion vacuous.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const before = singleRoot();
    const { rerender } = render(withTheme(<Header />));
    expect(await screen.findByText(readout(before))).toBeInTheDocument();
    expect(urls()).toHaveLength(1);

    // The watcher syncs an edit: the server would now answer an advanced revision.
    // The header must not learn that — it does not poll.
    const after = indexed({ graph_revision: 43, node_count: 12_350, edge_count: 6_801 });
    get.mockResolvedValue(after);
    await vi.advanceTimersByTimeAsync(5 * 60_000);

    expect(urls()).toHaveLength(1);
    expect(screen.getByText(readout(before))).toBeInTheDocument();

    // …and the next navigation is what refreshes it.
    vi.useRealTimers();
    pathname.current = "/graph";
    rerender(withTheme(<Header />));

    expect(await screen.findByText(readout(after))).toBeInTheDocument();
    expect(urls()).toEqual(["/api/v1/status", "/api/v1/status"]);
  });

  it("renders the API-unavailable badge INSTEAD of the figures on a cold read fault", async () => {
    mockProbe.mockResolvedValue({ mode: "single" });
    get.mockRejectedValue(new Error("connection refused"));
    render(withTheme(<Header />));

    const badge = await screen.findByText("API unavailable");
    // No zero and no blank slot — the badge stands in for the figures (NFR-RA-05).
    expect(screen.queryByText(/nodes/)).toBeNull();
    expect(screen.queryByText(/rev /)).toBeNull();
    // The fault IS announced — it is the one state that carries the live region.
    expect(badge.closest('[role="status"]')).not.toBeNull();
  });

  it("DROPS figures it has already shown when the next read faults", async () => {
    // The retention case, which the cold fault above cannot reach: the header has
    // read successfully, the server then stops, and the next navigation must replace
    // the figures rather than leave the last-read ones standing as current — the
    // precise failure NFR-RA-05 forbids (UAT-UI-11 step 6).
    const status = singleRoot();
    const { rerender } = render(withTheme(<Header />));
    expect(await screen.findByText(readout(status))).toBeInTheDocument();

    get.mockRejectedValue(new Error("connection refused"));
    pathname.current = "/graph";
    rerender(withTheme(<Header />));

    expect(await screen.findByText("API unavailable")).toBeInTheDocument();
    expect(screen.queryByText(readout(status))).toBeNull();
    expect(screen.queryByText(/nodes/)).toBeNull();
  });

  it("renders an honest not-indexed state rather than 0 nodes · 0 edges", async () => {
    singleRoot(indexed({ indexed: false, node_count: 0, edge_count: 0, graph_revision: 0 }));
    render(withTheme(<Header />));

    expect(await screen.findByText(/not indexed/i)).toBeInTheDocument();
    expect(screen.queryByText(/nodes/)).toBeNull();
    expect(screen.queryByText(/rev /)).toBeNull();
  });

  it("does not announce the figures — only the fault state is a live region", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));
    await screen.findByText(readout(status));

    expect(screen.queryByRole("status")).toBeNull();
  });

  it("re-reads on a member switch and describes the SELECTED member", async () => {
    mockProbe.mockResolvedValue({
      mode: "workspace",
      roster: { workspace: "shop", default: "api", members: ["api", "web"] },
    });
    const api = indexed();
    const web = indexed({ graph_revision: 7, node_count: 300, edge_count: 120 });
    get.mockImplementation((url: string) =>
      Promise.resolve(url.includes("repo=web") ? web : api),
    );
    render(withTheme(<Header />));

    expect(await screen.findByText(readout(api))).toBeInTheDocument();
    expect(urls()).toEqual(["/api/v1/status?repo=api"]);

    await userEvent.selectOptions(screen.getByRole("combobox"), "web");

    expect(await screen.findByText(readout(web))).toBeInTheDocument();
    expect(urls()).toEqual(["/api/v1/status?repo=api", "/api/v1/status?repo=web"]);
    // No member's counts are ever shown beside another member's name (FR-UI-29).
    expect(screen.queryByText(readout(api))).toBeNull();
  });
});

// ── Progressive disclosure (S-317, FR-UI-34, UAT-UI-11) ─────────────────────
//
// Two halves, asserted in two places on purpose.
//
// The STYLESHEET half — which rung drops the readout, which drops the brand
// subtitle, and that neither is truncated rather than removed — is asserted in
// `web/tests/spa_design_system.rs`, where every other stylesheet contract in this
// SPA already lives and where the `.module.css` files are read from disk. It
// cannot be asserted here: this suite runs with `css: false` (vitest.config.ts),
// so CSS Modules are empty objects, jsdom never evaluates a media query, and a
// `toBeVisible()` would report a safety it has not checked.
//
// The MARKUP half is asserted below, and it is what makes the stylesheet half
// mean anything: the one element the narrow rung hides is the one element EVERY
// readout state renders into.
//
// The rendered/computed proof at 420/768/1024/1600px in both themes is taken
// against a live `serve --ui` and recorded in the implementation notes, which is
// where the acceptance criterion puts it.

/** The header's graph-state slot: its LAST `<span>` child. The brand lockup is an
 *  `<a>`, the spacer a `<div>`, the member selector a `<div>` and the theme toggle
 *  a `<button>`, so the slot is identified structurally rather than by a class name
 *  this suite cannot see.
 *
 *  Last, not only: the member selector renders a `Badge` — also a `<span>`, also a
 *  direct child, and rendered BEFORE the slot — when its workspace probe faults.
 *  An earlier draft asserted there was exactly one span child and would have picked
 *  that badge had the assertion been relaxed. `labelledSpanChildren` below pins the
 *  two apart so this stays a structural fact rather than an ordering accident.
 *
 *  The narrow tier hides exactly this element, so every state OF THE READOUT must
 *  render inside it: a readout badge rendered as its sibling would survive the
 *  breakpoint and stand in the dropped readout's place, which is the failure
 *  NFR-CC-04 and NFR-RA-05 forbid. The workspace-probe badge is a different signal
 *  about a different subject (the member axis, not the graph), reports a fault that
 *  genuinely occurred, and deliberately does NOT give way — see its own spec. */
function graphStateSlot(): HTMLElement {
  const header = document.querySelector("header");
  expect(header).not.toBeNull();
  const spans = [...header!.children].filter((el): el is HTMLElement => el.tagName === "SPAN");
  expect(spans.length).toBeGreaterThan(0);
  return spans[spans.length - 1];
}

describe("Header progressive disclosure (S-317, FR-UI-34, UAT-UI-11)", () => {
  it("renders the figures inside the one element the narrow tier drops", async () => {
    const status = singleRoot();
    render(withTheme(<Header />));
    const figures = await screen.findByText(readout(status));

    expect(graphStateSlot()).toContainElement(figures);
  });

  it("renders the fault badge inside that SAME element — never beside it", async () => {
    mockProbe.mockResolvedValue({ mode: "single" });
    get.mockRejectedValue(new Error("connection refused"));
    render(withTheme(<Header />));
    const badge = await screen.findByText("API unavailable");

    // Dropping the readout therefore drops the badge with it: a fault badge left
    // standing in the readout's place would report an error that did not occur.
    expect(graphStateSlot()).toContainElement(badge);
  });

  it("renders the connecting state inside it too", async () => {
    mockProbe.mockResolvedValue({ mode: "single" });
    // A read that never settles, so the loading state is observable.
    get.mockReturnValue(new Promise(() => {}));
    render(withTheme(<Header />));

    expect(graphStateSlot()).toContainElement(await screen.findByText("Connecting…"));
  });

  it("renders the not-indexed state inside it too", async () => {
    singleRoot(indexed({ indexed: false, node_count: 0, edge_count: 0, graph_revision: 0 }));
    render(withTheme(<Header />));

    expect(graphStateSlot()).toContainElement(await screen.findByText(/not indexed/i));
  });

  it("keeps the member selector OUT of the dropped slot in workspace mode", async () => {
    mockProbe.mockResolvedValue({
      mode: "workspace",
      roster: { workspace: "shop", default: "api", members: ["api", "web"] },
    });
    const status = indexed();
    get.mockResolvedValue(status);
    render(withTheme(<Header />));
    await screen.findByText(readout(status));

    const selector = screen.getByRole("combobox");
    expect(graphStateSlot()).not.toContainElement(selector);
  });

  it("keeps the workspace-probe fault OUT of the slot — it is a different signal", async () => {
    // Found in review. `MemberSelector` renders its probe fault as a `Badge` — a
    // `<span>`, a DIRECT child of the header, sitting before the slot — so the
    // 1023px rung does not drop it and it stands where the readout was. That is
    // correct and deliberate: it reports a fault that genuinely occurred, about the
    // MEMBER AXIS rather than the graph, and a workspace whose roster could not be
    // read must say so at every width (FR-UI-29, NFR-RA-05). What it must not do is
    // overflow the row, which it did — measured at 254px and 29px of overflow at
    // 420px, pushing the theme toggle off-screen — so it now carries its own width
    // concession. This spec pins the structure the CSS rule depends on.
    mockProbe.mockRejectedValue(new Error("probe exploded"));
    get.mockResolvedValue(indexed());
    const { container } = render(withTheme(<Header />));

    const fault = await screen.findByText("Workspace status unavailable");
    const header = container.querySelector("header");
    // A direct child of the header, not inside the graph-state slot…
    expect(fault.parentElement).toBe(header);
    expect(graphStateSlot()).not.toContainElement(fault);
    // …and it precedes the slot, which is why the slot is the LAST span child.
    const spans = [...header!.children].filter((el) => el.tagName === "SPAN");
    expect(spans.indexOf(fault)).toBeLessThan(spans.indexOf(graphStateSlot()));
  });

  it("carries no inline style — disclosure is class-driven, the CSP is untouched", async () => {
    const status = singleRoot();
    const { container } = render(withTheme(<Header />));
    await screen.findByText(readout(status));

    // `style="…"` would need `style-src 'unsafe-inline'`; the self-only CSP
    // (NFR-SE-06) does not grant it, so the header must carry none.
    expect(container.querySelectorAll("[style]")).toHaveLength(0);
  });
});
