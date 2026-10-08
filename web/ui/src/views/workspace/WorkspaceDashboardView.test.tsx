/*
 * The Workspace Dashboard (S-428, FR-UI-36, FR-WS-05, BR-51, BR-56).
 *
 * The view answers "how coupled is this application, and how much of that do we
 * actually know?", so every assertion here is about a figure being honest rather
 * than about a figure being present: the headline never without its rate, a ratio
 * never without its denominator, an unopenable member never as a service with no
 * couplings, and no workspace-level roll-up of per-member signals anywhere.
 */

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";

import type { CrossServiceCoverage } from "../../api/types.ts";
import statesStyles from "../../components/States.module.css";
import { COVERAGE_TEXT, resolvedEdges } from "../../copy/coverage.copy.ts";
import { expectWidgetCopy } from "../../copy/expectWidgetCopy.ts";
import { DASHBOARD_TEXT, members, reachability } from "../../copy/workspaceDashboard.copy.ts";
import { actionKind, expectOneWidgetStack, widgetTitle } from "../../test/widgetStack.ts";
import { removeHiddenWidgetEntry } from "../../test/hiddenWidgets.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { setScopedMember } from "../../workspace/scope.ts";
import {
  coverageRider,
  degradedMember,
  memberStatus,
  oneDegraded,
  POPULATED_COVERAGE,
  reachabilityAnswer,
  statusInfo,
  stubAppApi,
  workspaceStatus,
  MULTI_REASON_COVERAGE,
  WIDGET_STATES,
  type AppStubOptions,
} from "./appViewFixtures.ts";
import { buildCoverageDashboard, resolvedEdgesState } from "./coverageModel.ts";
import { WorkspaceDashboardView } from "./WorkspaceDashboardView.tsx";

/** The two reads this view issues, each with the card that read alone supplies.
 *
 *  The card is what makes the spec precise. When the INNER read fails the outer
 *  one has already succeeded, so the boards it fed are correctly still on screen
 *  — asserting "no table anywhere" would be asserting that a successful read
 *  renders nothing, which is a different (and wrong) contract. */
const READS = [
  { path: "/api/v1/workspace/status", card: /^Members$/ },
  { path: "/api/v1/workspace/reachability", card: /^Cross-service reachability$/ },
] as const;

/** The view under test, inside the provider that establishes workspace mode. */
const tree = () => (
  <WorkspaceProvider>
    <WorkspaceDashboardView />
  </WorkspaceProvider>
);

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  setScopedMember(null);
});

/** Mount the view inside the provider that establishes the workspace mode, and
 *  wait for both reads to settle. */
async function mount(opts: AppStubOptions = {}) {
  const calls = stubAppApi(opts);
  const utils = render(
    <WorkspaceProvider>
      <WorkspaceDashboardView />
    </WorkspaceProvider>,
  );
  await screen.findByRole("heading", { name: /Resolved cross-service edges/ });
  return { ...utils, calls };
}

/** The `<section>` a design-system `Card` renders for `title` — the scope every
 *  row lookup below needs, because three tables on this view legitimately carry a
 *  row named after the same member. */
function card(title: RegExp): HTMLElement {
  const section = screen.getByRole("heading", { name: title }).closest("section");
  expect(section).not.toBeNull();
  return section as HTMLElement;
}

/** A coverage summary whose spec-conformance denominator is ZERO: references
 *  exist, and every one of them is bucketed out as `no-provider-in-workspace`.
 *  The server OMITS both ratios there — a fixture carrying `1` would let a view
 *  that cannot cope with absence pass (CR-100, FR-WS-05). */
const ZERO_DENOMINATOR: CrossServiceCoverage = {
  ...POPULATED_COVERAGE,
  bound: 0,
  ambiguous: 0,
  unbound: 0,
  by_intake: {
    contract_surface: { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0 },
    invocation: { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 899 },
  },
  resolved_cross_service_edges: 0,
  egress_resolution: undefined,
  spec_conformance_ratio: undefined,
  references: [
    {
      relation: "route",
      from: { member: "api", symbol: "a" },
      // `no-provider-in-workspace` arrives INSIDE the `unbound` bucket and is
      // split back out by its reason — the server's `unbound` counter excludes
      // it (ADR-53).
      bucket: "unbound",
      state: "unbound",
      reason: "no-provider-in-workspace",
      intake: "invocation",
      provenance: "literal" as const,
    },
  ],
  no_provider_in_workspace: 899,
  spec_conformance_measured: 0,
  spec_conformance_summary: "0 of 0 measured; 899 excluded as no-provider-in-workspace",
  egress_resolution_measured: 0,
  resolved_edges_summary:
    "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
};

describe("the headline is the server's composed line (BR-51, AC1)", () => {
  it("renders the edge count and the egress rate as ONE server-composed string", async () => {
    const coverage: CrossServiceCoverage = {
      ...POPULATED_COVERAGE,
      resolved_cross_service_edges: 9,
      egress_resolution: 0.5,
      egress_resolution_measured: 18,
      resolved_edges_summary:
        "9 resolved cross-service edges; egress resolution 0.500 (9 of 18 egress sites resolved)",
    };
    await mount({ status: workspaceStatus({ coverage }) });

    // The line VERBATIM, not the count and the rate recovered from two fields.
    // A view that composed them itself would be a fourth site free to forget the
    // pairing, which is the whole of BR-51.
    expect(
      screen.getByText(
        "9 resolved cross-service edges; egress resolution 0.500 (9 of 18 egress sites resolved)",
      ),
    ).toBeInTheDocument();
  });

  it("renders NO bar for an absent egress rate — 'not measured', never 0% or 100%", async () => {
    await mount({ status: workspaceStatus({ coverage: ZERO_DENOMINATOR }) });
    // Said twice, legitimately: the card's own wording AND the server's composed
    // line, which is rendered verbatim beside it.
    expect(screen.getAllByText(/egress resolution not measured/).length).toBeGreaterThan(0);
    expect(
      screen.getByText(
        "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
      ),
    ).toBeInTheDocument();
    // A 0-width bar reads "nothing resolves" and a full one "everything does";
    // the truth is that there was no denominator at all (CR-100).
    //
    // Counted as ELEMENTS, not by role: `ScoreBar` is a native `<meter>`, whose
    // implicit role is `meter` and never `progressbar` — a `queryByRole
    // ("progressbar")` here is null over a view that draws every bar it has, and
    // the assertion would pass whatever the view did (found in the falsifiability
    // sweep for this spec).
    expect(card(/^Resolved cross-service edges$/).querySelectorAll("meter")).toHaveLength(0);
    expect(card(/^Spec conformance/).querySelectorAll("meter")).toHaveLength(0);
    // Outbound calls WERE captured — all 899 call outside the workspace — so the
    // absence says that, never "no outbound call site was captured".
    expect(card(/^Resolved cross-service edges$/).querySelector("[data-widget-absence]")?.textContent).toBe(
      COVERAGE_TEXT.outboundAllOutside(899),
    );
  });

  it("DOES draw the bar when the rate is present — so the absence above is a choice", async () => {
    // The control for the assertion above: without it, "no bar" and "this view
    // draws no bars at all" are the same observation.
    await mount();
    expect(
      card(/^Resolved cross-service edges$/).querySelectorAll("meter").length,
    ).toBeGreaterThan(0);
  });
});

describe("every ratio carries its denominator and its exclusion (AC2, CR-111)", () => {
  it("renders the server's composed summary line beside the spec-conformance ratio", async () => {
    await mount({ status: workspaceStatus({ coverage: ZERO_DENOMINATOR }) });
    // The absent ratio: stated as absent, with the excluded count STILL reported.
    expect(
      screen.getByText("0 of 0 measured; 899 excluded as no-provider-in-workspace"),
    ).toBeInTheDocument();
    expect(screen.getByText(COVERAGE_TEXT.specNotMeasured(899))).toBeInTheDocument();
  });
});

describe("a member that could not be opened is drawn degraded and named (AC4)", () => {
  it("keeps it in the roster, badged, with its tallies read as unread rather than zero", async () => {
    await mount({
      status: workspaceStatus({
        members: [memberStatus("api"), memberStatus("orders"), degradedMember("web")],
        degraded_rollup: oneDegraded("web"),
      }),
      reachability: reachabilityAnswer({
        // The view could not read it, so it carries no tally at all.
        members: [
          {
            member: "api",
            extra_roots: 4,
            unresolved_roots: 1,
            dead_per_repo: 10,
            live_via_cross_service: 3,
            dead_app_wide: 7,
          },
        ],
        skipped_members: ["web"],
        coverage: coverageRider({ members_read: 2 }),
      }),
    });

    const row = within(card(/^Members$/)).getByRole("row", { name: /web/ });
    // Named and present — the omission FR-UI-36 calls the failure mode a service
    // map is most prone to.
    expect(row).toBeInTheDocument();
    expect(within(row).getByText(/degraded/i)).toBeInTheDocument();
    // And NOT drawn as a healthy service with no couplings: the tallies read
    // "not read", so a reader cannot mistake an unread member for an uncoupled
    // one (NFR-RA-05).
    expect(within(row).getAllByText(/not read/i).length).toBeGreaterThan(0);
    expect(within(row).queryByText("0")).toBeNull();
  });

  it("names every skipped member beside the reachability claims", async () => {
    await mount({
      status: workspaceStatus({
        members: [memberStatus("api"), memberStatus("orders"), degradedMember("web")],
        degraded_rollup: oneDegraded("web"),
      }),
      reachability: reachabilityAnswer({ skipped_members: ["web"] }),
    });
    expect(screen.getByText(/could not be read/i)).toHaveTextContent("web");
  });
});

describe("the reachability rider's shortfall caveat (FR-WS-16, NFR-CC-04)", () => {
  it("does NOT claim a minimum (a lower bound) when every member was read", async () => {
    // The regression this pins: the rider carries no `covers_all_members` flag,
    // so a view consulting one reads `undefined` — falsy — and stamps "lower
    // bound" on a COMPLETE answer. Found in the S-428 review; it survived the
    // first round because the fixture had invented the field.
    await mount();
    expect(within(card(/^Cross-service reachability$/)).getByText(/3 of 3 members read/)).toBeInTheDocument();
    expect(screen.queryByText(/is a minimum/i)).toBeNull();
  });

  it("DOES claim a minimum (a lower bound) when a member's surface was not read", async () => {
    // The control: without it, "no caveat" and "this view has no caveat" are the
    // same observation.
    await mount({
      reachability: reachabilityAnswer({
        coverage: coverageRider({ members_read: 2, members_total: 3 }),
      }),
    });
    expect(within(card(/^Cross-service reachability$/)).getByText(/2 of 3 members read/)).toBeInTheDocument();
    expect(screen.getByText(/is a minimum/i)).toBeInTheDocument();
  });
});

describe("no aggregate of per-member signals is rendered (BR-56, AC5)", () => {
  it("shows each member's OWN resolution coverage, named — and no mean of them", async () => {
    // Three deliberately distinct per-member signals whose mean is a distinctive
    // string, over three DIFFERENT denominators — so this spec pins the CR-111
    // duty (the ratio is never bare) and the BR-56 one (it is never averaged) at
    // the same time.
    await mount({
      status: workspaceStatus({
        members: [
          memberStatus("api", {
            result: statusInfo({ resolution_coverage: 0.4, refs_resolved: 40, refs_total: 100 }),
          }),
          memberStatus("orders", {
            result: statusInfo({ resolution_coverage: 0.6, refs_resolved: 600, refs_total: 1000 }),
          }),
          memberStatus("web", {
            result: statusInfo({
              resolution_coverage: 0.8,
              refs_resolved: 8_000,
              refs_total: 10_000,
            }),
          }),
        ],
      }),
    });

    for (const [member, own] of [
      ["api", "40.0% (40 of 100 refs)"],
      ["orders", "60.0% (600 of 1,000 refs)"],
      ["web", "80.0% (8,000 of 10,000 refs)"],
    ] as const) {
      const row = within(card(/^Members$/)).getByRole("row", { name: new RegExp(member) });
      expect(within(row).getByText(own)).toBeInTheDocument();
    }
    // The mean of 0.4/0.6/0.8 is 0.6, which `orders` legitimately owns — so the
    // roll-up is pinned by COUNTING the percentage, not by its absence.
    expect(within(card(/^Members$/)).getAllByText(/60\.0%/)).toHaveLength(1);
  });

  it("renders no element announcing itself as a workspace-wide quality signal", async () => {
    await mount();
    // BR-56 is about rendering an AGGREGATE AS A SIGNAL, so the guard reads the
    // places a figure announces itself — card titles and column headers — and not
    // page prose. Matching all text instead fires on this view's own disclaimer,
    // which contains the words "quality signal" and "mean" precisely because it is
    // explaining that neither is computed here.
    //
    // The vocabulary is the SIBLING per-member Dashboard's own: its card is titled
    // "Quality index" and its empty state says "quality signal"
    // (`views/dashboard/DashboardView.tsx`). An earlier spelling matched only
    // /workspace (signal|score)/, which an aggregate added in that idiom would
    // have walked straight past (found in the S-428 review).
    const labels = [
      ...screen.getAllByRole("heading").map((h) => h.textContent ?? ""),
      ...screen.getAllByRole("columnheader").map((c) => c.textContent ?? ""),
    ];
    // The floor: an empty `labels` would make every assertion below vanish rather
    // than fail — a guard that switches itself off exactly when the surface it
    // guards disappears.
    expect(labels.length).toBeGreaterThan(5);
    for (const forbidden of [
      /quality (index|signal)/i,
      /workspace (signal|score)/i,
      /overall (signal|score|health)/i,
      /\baverage\b|\bmean\b|\btotal signal\b/i,
    ]) {
      expect(labels.filter((l) => forbidden.test(l))).toEqual([]);
    }
  });
});

describe("the view is app-level and honest about its mode", () => {
  it("reads the two unscoped fan-outs and carries no ?repo= from the shell", async () => {
    setScopedMember("api");
    const { calls } = await mount();
    const reads = calls().filter((u) => !u.startsWith("/api/v1/workspace/roster"));
    expect(reads).toEqual(["/api/v1/workspace/status", "/api/v1/workspace/reachability"]);
  });

  it("states honestly that a single-root serve is not a workspace", async () => {
    stubAppApi({ probeStatus: 404 });
    render(
      <WorkspaceProvider>
        <WorkspaceDashboardView />
      </WorkspaceProvider>,
    );
    await waitFor(() => expect(screen.getByText(/Not a workspace/)).toBeInTheDocument());
    expect(screen.queryByRole("heading", { name: /Resolved cross-service edges/ })).toBeNull();
  });
});

describe("the reachability view is labelled advisory and states its bounds", () => {
  it("says the dead set was suppressed rather than rendering it as empty", async () => {
    await mount();
    // `dead: null` is SUPPRESSED, deliberately distinct from `[]` ("computed, and
    // genuinely empty"). Rendering it as "no dead code" is the misreading CR-084
    // added the bound to prevent.
    expect(screen.getByText(/not requested/i)).toBeInTheDocument();
    expect(screen.queryByText(/no dead code/i)).toBeNull();
  });

  it("labels the reachability card advisory — it is never a gate input (ADR-56)", async () => {
    await mount();
    const card = screen.getByRole("heading", { name: /reachability/i }).closest("section");
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByText(/advisory/i)).toBeInTheDocument();
  });
});

describe("a failed read is stated, never papered over (NFR-RA-05)", () => {
  /** Answer `path` with `code` and everything else normally — so the spec can
   *  fail ONE of the two reads and see what the view does with the other. */
  function failing(path: string, code = 500) {
    const calls = stubAppApi();
    const ok = globalThis.fetch as unknown as (u: string) => Promise<Response>;
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        url.startsWith(path)
          ? Promise.resolve({
              ok: false,
              status: code,
              json: () => Promise.resolve({ error: "boom" }),
            } as Response)
          : ok(url),
      ),
    );
    return calls;
  }

  it("has a read to fail for every read this view issues", () => {
    // The floor: `it.each([])` registers zero tests silently, so an empty roster
    // would delete the coverage below rather than fail it.
    expect(READS.length).toBe(2);
  });

  it.each(READS)("states the failure when $path fails", async ({ path, card }) => {
    // Both views wire TWO resources into nested `AsyncResource` frames. The
    // generic hook has its own specs, but those prove the primitive works in
    // isolation — not that THIS view passed the right resource into the right
    // frame. A swap or a shared reference would leave one failure invisible,
    // which is the state this spec exists to rule out.
    failing(path);
    render(tree());
    const panel = await screen.findByText(
      new RegExp(`${path.replace("/api/v1/", "")}.*failed`, "i"),
    );
    expect(panel).toBeInTheDocument();
    // …and the card that read alone supplies is ABSENT rather than filled with
    // a fabricated figure (NFR-CC-04).
    expect(screen.queryByRole("heading", { name: card })).toBeNull();
  });

  it("states a failed mode probe as a failure, not as a plain repo", async () => {
    // The THIRD error source: the roster probe behind `useWorkspace()`. A 500
    // there must not be read as single-root — that would silently hide the
    // workspace UI (NFR-RA-05), which is why `probeWorkspace` rethrows.
    failing("/api/v1/workspace/roster");
    render(tree());
    await waitFor(() =>
      expect(screen.getByText(/could not be read/i)).toBeInTheDocument(),
    );
    expect(screen.queryByText(/Not a workspace/)).toBeNull();
  });
});

describe("the per-arm coverage board is hidden through the register (S-612, FR-UI-41)", () => {
  it("renders the coverage boards without the per-arm board", async () => {
    await mount();
    // The title glosses "intake", so its accessible name carries the gloss after it.
    expect(card(/^Coverage by intake/)).toBeInTheDocument();
    // A prefix: the title glosses "arm", so its accessible name carries the gloss after it.
    expect(screen.queryByRole("heading", { name: /^Coverage by relation arm/ })).toBeNull();
    expect(screen.queryByRole("table", { name: /by relation arm/i })).toBeNull();
  });

  it("renders the per-arm board again when its register entry is removed", async () => {
    onTestFinished(removeHiddenWidgetEntry("coverage-by-relation-arm"));
    await mount();
    const board = card(/^Coverage by relation arm/);
    expect(within(board).getByRole("table", { name: /by relation arm/i })).toBeInTheDocument();
  });
});

// ── S-613 (CR-203 §3.2 D items 2–4 and 10, FR-UI-39, FR-UI-40) ────────────────

/** The dashboard's widgets, top to bottom. The per-arm board is hidden (S-612). */
const DASHBOARD_WIDGETS = [
  "Resolved cross-service edges",
  "Spec conformance (declared endpoints vs controllers)",
  "Coverage by intake",
  "Cross-service reachability",
  "Members",
];

/** The one widget titled `title`. */
function widget(container: HTMLElement, title: string): HTMLElement {
  const found = expectOneWidgetStack(container).filter((w) => widgetTitle(w) === title);
  expect(found, title).toHaveLength(1);
  return found[0];
}

/** A widget's figure row, as a reader sees it. */
function figureText(w: HTMLElement): string {
  return (w.querySelector('[data-widget-part="figure"]')?.textContent ?? "").replace(/\s+/g, " ").trim();
}

function actionText(w: HTMLElement): string {
  return w.querySelector('[data-widget-copy="action"]')?.textContent ?? "";
}

describe("every widget explains itself, in one stack (S-613)", () => {
  it("has a state to check for each of the four the story names", () => {
    // The floor: `it.each` over an empty table registers nothing, silently.
    expect(Object.keys(WIDGET_STATES)).toEqual(["healthy", "partial coverage", "nothing measured", "degraded member"]);
  });

  /** Each widget's action kind in each state, written out rather than read back
   *  from the catalogue: `expectWidgetCopy` derives the expected kind from the
   *  same catalogue the view renders, so only a table stated here can catch a
   *  wrong branch in the catalogue or a wrong state in the view. Order follows
   *  `DASHBOARD_WIDGETS`. */
  const EXPECTED_ACTIONS: Record<keyof typeof WIDGET_STATES, string[]> = {
    // Every call resolves, every endpoint matches, nothing to keep or delete.
    healthy: ["none", "none", "none", "none", "none"],
    // 7 calls unresolved, 7 references unmatched, captured calls do resolve
    // (2), 1 callable to keep, members with callables unused everywhere.
    "partial coverage": ["act", "act", "none", "act", "act"],
    // Nothing captured, nothing declared, nothing promoted, no tallies.
    "nothing measured": ["none", "none", "none", "none", "none"],
    // As partial, and the Members action is the degraded member's re-index.
    "degraded member": ["act", "act", "none", "act", "act"],
  };

  it.each(Object.entries(WIDGET_STATES))(
    "%s: every widget is a Widget under the view's one WidgetStack and carries the message standard",
    async (state, { status, reachability: reach }) => {
      const { container } = await mount({ status, reachability: reach });
      const widgets = expectOneWidgetStack(container);
      expect(widgets.map(widgetTitle)).toEqual(DASHBOARD_WIDGETS);
      for (const w of widgets) expectWidgetCopy(w);
      expect(widgets.map(actionKind)).toEqual(EXPECTED_ACTIONS[state as keyof typeof WIDGET_STATES]);
    },
  );

  it("Members asks for deletion review when no member is degraded and some have callables unused everywhere", async () => {
    // The default roster: no degraded member; unused-everywhere tallies 7, 5, 2.
    const { container } = await mount();
    const w = widget(container, "Members");
    expectWidgetCopy(w, members, { degraded: 0, withUnused: 3 });
    // The phrase is glossed, so the tooltip follows it in the raw text.
    expect(actionText(w)).toMatch(/^Review the callables unused across the workspace/);
    expect(w.querySelector('[data-widget-copy="action"] dfn')?.getAttribute("data-term")).toBe("unusedAcrossWorkspace");
    expect(w.querySelector('[data-widget-copy="where"]')?.textContent).toBe("source code");
  });

  it("Members asks for a re-index first when a member is degraded", async () => {
    const { container } = await mount(WIDGET_STATES["degraded member"]);
    const w = widget(container, "Members");
    expect(actionText(w)).toMatch(/^Run logos index in each member marked degraded/);
    expect(w.querySelector('[data-widget-copy="where"]')?.textContent).toBe("command logos index");
  });

  it("Resolved cross-service edges: r of s resolved, and below 100% the reasons largest first, summing to the unresolved count", async () => {
    const { container } = await mount({ status: workspaceStatus({ coverage: MULTI_REASON_COVERAGE }) });
    const w = widget(container, "Resolved cross-service edges");
    const state = resolvedEdgesState(buildCoverageDashboard(MULTI_REASON_COVERAGE));
    expectWidgetCopy(w, resolvedEdges, state);

    expect(figureText(w)).toContain("2 of 9 outbound call sites resolved");
    // The edge count rides beside it in the server's composed line (CR-203 D4,
    // BR-51), in the figure row and verbatim.
    expect(figureText(w)).toContain(MULTI_REASON_COVERAGE.resolved_edges_summary);
    const [, resolved, measured] = figureText(w).match(/(\d+) of (\d+) outbound call sites resolved/)!.map(Number);
    const listed = [...actionText(w).matchAll(/(\d+) × ([^:]+): [^;(]+\(([^)]+)\)/g)].map((m) => ({
      count: Number(m[1]),
      label: m[2],
      where: m[3],
    }));
    // Largest first, across three arms; the 1–1 tie broken by reason name.
    expect(listed.map((l) => [l.count, l.label])).toEqual([
      [3, "Base URL resolved at runtime"],
      [2, "Broker topic is not a static literal"],
      [1, "Two or more providers (ambiguous)"],
      [1, "Path could not be composed"],
    ]);
    expect(listed.map((l) => l.where)).toEqual([
      "configuration",
      "source code or configuration",
      "source code",
      "source code",
    ]);
    // The listed counts sum to the unresolved count the figure shows.
    expect(listed.reduce((n, l) => n + l.count, 0)).toBe(measured - resolved);
    // The call to a service outside the workspace is named apart, never in the sum.
    expect(actionText(w)).toMatch(/A further 1 call site calls a service outside this workspace and is not counted above/);
  });

  it("Resolved cross-service edges: nothing to do when every captured call resolves", async () => {
    const { container } = await mount({ status: WIDGET_STATES.healthy.status });
    const w = widget(container, "Resolved cross-service edges");
    expect(w.querySelector('[data-widget-part="action"]')?.getAttribute("data-action-kind")).toBe("none");
  });

  it("Cross-service reachability leads with the keep-them count, with no EmptyState", async () => {
    const { container } = await mount();
    const w = widget(container, "Cross-service reachability");
    expectWidgetCopy(w, reachability, { keep: 1 });
    expect(w.querySelector('[data-widget-part="figure"] p')?.textContent).toBe(DASHBOARD_TEXT.keepThem(1, false));
    expect(w.querySelector('[data-widget-part="figure"] p')?.textContent).toMatch(
      /^1 callable unused in its own service but called from another — keep it\.$/,
    );
  });

  it("Cross-service reachability reads 'at least' when its coverage is partial", async () => {
    const { container } = await mount({ reachability: WIDGET_STATES["partial coverage"].reachability });
    const w = widget(container, "Cross-service reachability");
    expect(w.querySelector('[data-widget-part="figure"] p')?.textContent).toMatch(/^At least 1 callable unused/);
  });

  it("Cross-service reachability states an empty answer in its figure row, never as an EmptyState", async () => {
    const { container } = await mount(WIDGET_STATES["nothing measured"]);
    const w = widget(container, "Cross-service reachability");
    expectWidgetCopy(w, reachability, { keep: 0 });
    expect(figureText(w)).toContain(DASHBOARD_TEXT.keepThem(0, false));
    expect(figureText(w)).toContain(DASHBOARD_TEXT.keepThemNone);
    expect(w.querySelector(`.${statesStyles.empty}`)).toBeNull();
    expect(w.querySelector("table")).toBeNull();
  });

  it("Members glosses its figure headers", async () => {
    const { container } = await mount();
    const w = widget(container, "Members");
    const glossed = [...w.querySelectorAll("th dfn[data-term]")].map((d) => d.getAttribute("data-term"));
    expect(glossed).toEqual([
      "referenceResolution",
      "entryPointsFromOtherServices",
      "unusedInOwnGraph",
      "usedByAnotherService",
      "unusedAcrossWorkspace",
    ]);
  });

  it("Members gives each row its own action: re-index a degraded member, review deletions, or nothing", async () => {
    const { container } = await mount({
      status: WIDGET_STATES["degraded member"].status,
      reachability: reachabilityAnswer({
        members: [
          { member: "api", extra_roots: 4, unresolved_roots: 1, dead_per_repo: 10, live_via_cross_service: 3, dead_app_wide: 7 },
          { member: "orders", extra_roots: 0, unresolved_roots: 0, dead_per_repo: 0, live_via_cross_service: 0, dead_app_wide: 0 },
        ],
        skipped_members: ["web"],
      }),
    });
    const w = widget(container, "Members");
    expectWidgetCopy(w, members, { degraded: 1, withUnused: 1 });
    const actionCell = (member: string) => {
      const row = within(w).getByRole("row", { name: new RegExp(`^${member}\\b`) });
      return [...row.querySelectorAll("td")].at(-1)?.textContent ?? "";
    };
    expect(actionCell("web")).toMatch(/^Run logos index in this member\.command logos index$/);
    expect(actionCell("api")).toMatch(/^Review 7 callables for deletion\.source code$/);
    expect(actionCell("orders")).toBe("Nothing to do — informational.");
  });

  it("Members states BR-56 as one plain sentence", async () => {
    const { container } = await mount();
    expect(within(widget(container, "Members")).getByText(DASHBOARD_TEXT.notAveraged)).toBeInTheDocument();
  });
});
