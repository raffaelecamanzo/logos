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
import { afterEach, describe, expect, it, vi } from "vitest";

import type { CrossServiceCoverage } from "../../api/types.ts";
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
  type AppStubOptions,
} from "./appViewFixtures.ts";
import { WorkspaceDashboardView } from "./WorkspaceDashboardView.tsx";

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
    expect(screen.getByText(/spec conformance not measured/)).toBeInTheDocument();
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
  it("does NOT claim a lower bound when every member was read", async () => {
    // The regression this pins: the rider carries no `covers_all_members` flag,
    // so a view consulting one reads `undefined` — falsy — and stamps "lower
    // bound" on a COMPLETE answer. Found in the S-428 review; it survived the
    // first round because the fixture had invented the field.
    await mount();
    expect(screen.getByText(/3 of 3 members read/)).toBeInTheDocument();
    expect(screen.queryByText(/lower bound/i)).toBeNull();
  });

  it("DOES claim a lower bound when a member's surface was not read", async () => {
    // The control: without it, "no caveat" and "this view has no caveat" are the
    // same observation.
    await mount({
      reachability: reachabilityAnswer({
        coverage: coverageRider({ members_read: 2, members_total: 3 }),
      }),
    });
    expect(screen.getByText(/2 of 3 members read/)).toBeInTheDocument();
    expect(screen.getByText(/lower bound/i)).toBeInTheDocument();
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
