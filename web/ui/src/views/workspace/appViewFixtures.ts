/*
 * Fixtures for the two app-level workspace views (S-428, FR-UI-36) — the stubbed
 * `/api/v1/workspace/*` surface their specs drive.
 *
 * Kept HERE rather than folded into `src/workspace/testFixtures.ts` for two
 * reasons. The shared module answers the shell's mode-discovery contract with one
 * fixed two-member payload; these views need the member rows themselves to vary —
 * an unopenable member, a zero-denominator ratio, a workspace declaring no rules
 * are each a different member table, not a different endpoint. And it reuses that
 * module's `EMPTY_COVERAGE` rather than restating it, so the honest-empty coverage
 * shape has one author.
 *
 * Every builder takes an override bag and fills the rest, so a spec states only
 * the field it is about and a field added to a wire type later fails in ONE place.
 */

import { vi } from "vitest";

import type {
  BoundedReachability,
  CoverageRider,
  CrossServiceCoverage,
  DegradedRollup,
  MemberStatus,
  MemberTopics,
  StatusInfo,
  WarmRollup,
  WorkspaceGovernance,
  WorkspaceGovernanceAnswer,
  WorkspaceReachabilityAnswer,
  WorkspaceRoster,
  WorkspaceStatus,
} from "../../api/types.ts";
import { EMPTY_COVERAGE } from "../../workspace/testFixtures.ts";

export { EMPTY_COVERAGE };

/** The three-member roster these specs read as the workspace. */
export const ROSTER: WorkspaceRoster = {
  workspace: "shop",
  default: "api",
  members: ["api", "orders", "web"],
};

/** A unix-seconds stamp `mins` minutes before now, as the wire carries it (a
 *  string). Relative rather than fixed: `freshnessStatement` humanises the age
 *  against the clock, so a hard-coded stamp would drift into "implausibly old". */
export function minutesAgo(mins: number): string {
  return String(Math.floor(Date.now() / 1000) - mins * 60);
}

/** One member's `StatusInfo`, every field present. */
export function statusInfo(over: Partial<StatusInfo> = {}): StatusInfo {
  return {
    indexed: true,
    file_count: 120,
    node_count: 3_400,
    edge_count: 9_100,
    db_path: "/w/api/.logos/logos.db",
    db_size_bytes: 4_200_000,
    last_full_index_at: minutesAgo(30),
    last_sync_at: minutesAgo(5),
    graph_revision: 7,
    refs_total: 1_000,
    refs_resolved: 800,
    refs_unresolved: 200,
    resolution_coverage: 0.8,
    total_line_count: 20_000,
    source_line_count: 15_000,
    test_line_count: 5_000,
    freshness: "index revision 7",
    warnings: [],
    ...over,
  };
}

/** A member row that answered. */
export function memberStatus(member: string, over: Partial<MemberStatus> = {}): MemberStatus {
  return {
    member,
    result: statusInfo(),
    warm_state: "warm",
    open_state: "opened",
    ...over,
  };
}

/** A member whose store could NOT be opened (BR-45, FR-WS-16): no `result`, a
 *  verbatim diagnostic, and both degraded axes set. The shape every "drawn
 *  degraded and named" assertion drives. */
export function degradedMember(member: string): MemberStatus {
  return {
    member,
    error: "could not open member store",
    warm_state: "degraded",
    open_state: "degraded",
    degraded_cause: "host-resource-limit",
    degraded_reason: "the process ran out of file descriptors",
    degraded_diagnostic: "unable to open database file (os error 24)",
  };
}

/** A coverage summary with cross-boundary references in it — the shape the
 *  coverage boards actually draw. `EMPTY_COVERAGE` renders the awaiting-data
 *  branch instead (no cards at all), so it is the wrong default for a spec about
 *  what the boards say.
 *
 *  Both ratios are PRESENT here and both composed summary lines are the server's,
 *  so a spec asserting the denominator duty is asserting the populated path; the
 *  absent-ratio path has its own fixture in the Dashboard's spec. */
export const POPULATED_COVERAGE: CrossServiceCoverage = {
  references: [
    {
      relation: "route",
      from: { member: "web", symbol: "logos . . . web/`cart.ts`/checkout()." },
      to: { member: "api", symbol: "logos . . . api/`billing.rs`/charge()." },
      bucket: "bound",
      state: "bound",
      intake: "invocation",
      provenance: "literal" as const,
    },
    {
      relation: "route",
      from: { member: "orders", symbol: "logos . . . orders/`client.rs`/fetch()." },
      bucket: "unbound",
      state: "unbound",
      reason: "path-not-composed",
      intake: "invocation",
      provenance: "literal" as const,
    },
  ],
  bound: 12,
  ambiguous: 1,
  unbound: 3,
  no_provider_in_workspace: 40,
  by_intake: {
    contract_surface: { bound: 9, ambiguous: 0, unbound: 1, no_provider_in_workspace: 0 },
    invocation: { bound: 3, ambiguous: 1, unbound: 2, no_provider_in_workspace: 40 },
  },
  resolved_cross_service_edges: 9,
  egress_resolution: 0.5,
  egress_resolution_measured: 18,
  resolved_edges_summary:
    "9 resolved cross-service edges; egress resolution 0.500 (9 of 18 egress sites resolved)",
  spec_conformance_ratio: 0.75,
  spec_conformance_measured: 16,
  spec_conformance_summary: "0.750 (12 of 16 measured; 40 excluded as no-provider-in-workspace)",
  members_read: 3,
  members_total: 3,
  covers_all_members: true,
};

/** The coverage rider, healthy by default.
 *
 *  Note what is NOT here: `covers_all_members`. `logos-core`'s `CoverageRider`
 *  has no such field — only `members_read` / `members_total` — and this fixture
 *  used to invent one, which is exactly how a view reading it shipped a caveat
 *  that fired on every answer. A fixture that carries a field the server does not
 *  send tests a payload shape that does not exist. */
export function coverageRider(over: Partial<CoverageRider> = {}): CoverageRider {
  return {
    bound: 12,
    ambiguous: 1,
    unbound: 3,
    no_provider_in_workspace: 40,
    resolved_cross_service_edges: 9,
    bridge_invocation_edges: 9,
    egress_resolution: 0.5,
    egress_resolution_measured: 18,
    spec_conformance_ratio: 0.75,
    spec_conformance_measured: 16,
    members_read: 3,
    members_total: 3,
    ...over,
  };
}

/** `GET /api/v1/workspace/reachability`, under the promotions-only default — so
 *  `dead` is `null` (SUPPRESSED), never `[]`. */
export function reachabilityAnswer(
  over: Partial<BoundedReachability> = {},
  completeness: Partial<Omit<WorkspaceReachabilityAnswer, "reachability">> = {},
): WorkspaceReachabilityAnswer {
  const rider = over.coverage ?? coverageRider();
  return {
    reachability: {
      view: "app-wide-union",
      advisory: true,
      scope: { repo: null, promotions_only: true },
      coverage: rider,
      members: [
        {
          member: "api",
          extra_roots: 4,
          unresolved_roots: 1,
          dead_per_repo: 10,
          live_via_cross_service: 3,
          dead_app_wide: 7,
        },
        {
          member: "orders",
          extra_roots: 2,
          unresolved_roots: 0,
          dead_per_repo: 6,
          live_via_cross_service: 1,
          dead_app_wide: 5,
        },
        {
          member: "web",
          extra_roots: 0,
          unresolved_roots: 0,
          dead_per_repo: 2,
          live_via_cross_service: 0,
          dead_app_wide: 2,
        },
      ],
      skipped_members: [],
      live_via_cross_service: [
        {
          member: "orders",
          symbol: "logos . . . orders/`handler.rs`/place_order().",
          name: "place_order",
          kind: "function",
          verdict: "live-via-cross-service",
          coverage: rider,
        },
      ],
      dead: null,
      ...over,
    },
    complete: true,
    degraded_rollup: allOpened(),
    ...completeness,
  };
}

/** `GET /api/v1/workspace/check` carrying a report. */
export function governanceAnswer(
  report: WorkspaceGovernance | null,
  completeness: Partial<Omit<WorkspaceGovernanceAnswer, "governance">> = {},
): WorkspaceGovernanceAnswer {
  return {
    governance: report,
    complete: true,
    degraded_rollup: allOpened(),
    ...completeness,
  };
}

/** A governance report with one breach. */
export function governanceReport(over: Partial<WorkspaceGovernance> = {}): WorkspaceGovernance {
  return {
    workspace: "shop",
    rules_checked: 2,
    bindings_checked: 9,
    violations: [
      {
        rule: "workspace-boundary:web->api",
        rule_type: "workspace-boundary",
        severity: "error",
        relation: "route",
        from: { member: "web", symbol: "logos . . . web/`cart.ts`/checkout()." },
        to: { member: "api", symbol: "logos . . . api/`billing.rs`/charge()." },
        message: "web must not call api directly — route through orders",
      },
    ],
    ...over,
  };
}

/** Every roster member opened. */
export function allOpened(): DegradedRollup {
  return {
    members: 3,
    opened: 3,
    not_attempted: 0,
    degraded_members: [],
    covers_all_members: true,
  };
}

/** One roster member attempted and failed — the partial fan-out (FR-WS-16). */
export function oneDegraded(member = "web"): DegradedRollup {
  return {
    members: 3,
    opened: 2,
    not_attempted: 0,
    degraded_members: [member],
    covers_all_members: false,
  };
}

/** The warm roll-up. `warming` is OMITTED by default, exactly as the server omits
 *  it when no trustworthy live signal exists — absent means "not knowable", never
 *  "none" (NFR-CC-04). */
export function warmRollup(over: Partial<WarmRollup> = {}): WarmRollup {
  return { members: 3, warm: 3, deferred: 0, degraded: 0, ...over };
}

/** `GET /api/v1/workspace/status` over the three-member roster. */
export function workspaceStatus(over: Partial<WorkspaceStatus> = {}): WorkspaceStatus {
  return {
    workspace: "shop",
    members: [memberStatus("api"), memberStatus("orders"), memberStatus("web")],
    warm_rollup: warmRollup(),
    degraded_rollup: allOpened(),
    coverage: POPULATED_COVERAGE,
    topics: [],
    ...over,
  };
}

/** What the stubbed surface answers with. */
export interface AppStubOptions {
  /** `200` ⇒ workspace, `404` ⇒ single-root (the shell's mode probe). */
  probeStatus?: number;
  status?: WorkspaceStatus;
  reachability?: WorkspaceReachabilityAnswer;
  governance?: WorkspaceGovernanceAnswer;
  coverage?: CrossServiceCoverage;
  topics?: MemberTopics[];
}

/**
 * Stub `fetch` over the `/api/v1` surface the two app-level views read, and return
 * a recorder of every URL called. An unmatched path answers an empty `200`, so a
 * view never fails on a read its spec does not care about.
 */
export function stubAppApi(opts: AppStubOptions = {}): () => string[] {
  const { probeStatus = 200, coverage, topics } = opts;
  const model =
    opts.status ??
    workspaceStatus({
      ...(coverage ? { coverage } : {}),
      ...(topics ? { topics } : {}),
    });
  const reach = opts.reachability ?? reachabilityAnswer();
  const gov = opts.governance ?? governanceAnswer(governanceReport());

  const calls: string[] = [];
  const json = (body: unknown, ok = true, code = 200) =>
    Promise.resolve({ ok, status: code, json: () => Promise.resolve(body) } as Response);

  vi.stubGlobal(
    "fetch",
    vi.fn((url: string) => {
      calls.push(url);
      if (url.startsWith("/api/v1/workspace/roster")) {
        return json(ROSTER, probeStatus === 200, probeStatus);
      }
      if (url.startsWith("/api/v1/workspace/status")) return json(model);
      if (url.startsWith("/api/v1/workspace/reachability")) return json(reach);
      if (url.startsWith("/api/v1/workspace/check")) return json(gov);
      return json({});
    }),
  );
  return () => calls;
}
