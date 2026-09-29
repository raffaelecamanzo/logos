/*
 * Shared workspace test fixtures (S-250) — one stubbed `/api/v1` surface every
 * workspace test drives, so the mode-discovery contract (roster 404 ⇒ single-root,
 * 200 ⇒ workspace) is stated once rather than re-invented per spec.
 */

import { vi } from "vitest";

import type {
  BoundExternal,
  BuildDependencyHeadline,
  CrossServiceCoverage,
  DeclaredContractRelation,
  MemberTopics,
  WorkspaceRoster,
  WorkspaceStatus,
  XserviceBuildDeps,
} from "../api/types.ts";

/** The two-member roster the shell probe answers with. */
export const ROSTER: WorkspaceRoster = {
  workspace: "shop",
  default: "api",
  members: ["api", "web"],
};

/** An empty coverage summary over a fully-read two-member workspace.
 *
 *  `spec_conformance_ratio` is **omitted**, exactly as the server omits it when nothing was
 *  measured (S-326, FR-WS-05) — a fixture carrying `1` here would let a view that
 *  cannot cope with absence pass its tests. */
export const EMPTY_COVERAGE: CrossServiceCoverage = {
  references: [],
  bound: 0,
  ambiguous: 0,
  unbound: 0,
  no_provider_in_workspace: 0,
  // Both populations empty, which is what the server sends over a workspace with
  // no cross-boundary references at all (S-377) — an empty split, never an absent
  // one, so a view that reads it needs no fallback.
  by_intake: {
    contract_surface: { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0 },
    invocation: { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0 },
  },
  spec_conformance_measured: 0,
  spec_conformance_summary: "0 of 0 measured; 0 excluded as no-provider-in-workspace",
  // The CR-120 headline in its honest-empty shape: a count of 0 with the rate
  // OMITTED (never 0 and never 1), exactly as the server sends it when no egress
  // site was captured.
  resolved_cross_service_edges: 0,
  egress_resolution_measured: 0,
  resolved_edges_summary:
    "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
  members_read: 2,
  members_total: 2,
  covers_all_members: true,
};

/** The declared-contract relation over the two members (S-461) — the reference
 *  estate's shapes at this fixture's scale:
 *
 *  - `api` holds a copy of `web`'s own spec: document identity, 3 of 3;
 *  - `api` and `web` each hold a PSS copy — ONE external, two declarers;
 *  - `web` also holds a second, unrelated document titled PSS — a SECOND
 *    external with the same name, so anything keyed by name collides here;
 *  - `pss-mock` (not a roster member) stands in for the first PSS. */
export const DECLARED_CONTRACTS: DeclaredContractRelation = {
  headline: {
    declared_contract_pairs: 4,
    to_member: 1,
    to_external: 3,
    documents: { documents: 6, own: 1, vendored: 4, partial: 0, unjudged: 0, mock: 1, documentation: 0 },
    named_externals: 2,
    identity_collisions: 0,
    resolved_ties: 0,
    summary:
      "4 declared contract pairs (1 by document identity, 3 to named externals) from 4 vendored of 6 spec documents; 2 named externals; 0 contract-surface ties resolved by document identity; declared by vendored specs, never observed calls",
  },
  contracts: [
    {
      holder: "api",
      document: "pss.yaml",
      provenance: "vendored-spec",
      target: { kind: "external", external: "api:pss.yaml", name: "PSS" },
    },
    {
      holder: "api",
      document: "specs/web.yaml",
      provenance: "vendored-spec",
      target: { kind: "member", member: "web", document: "api/openapi.yaml", shared: 3, total: 3 },
    },
    {
      holder: "web",
      document: "legacy/pss.yaml",
      provenance: "vendored-spec",
      target: { kind: "external", external: "web:legacy/pss.yaml", name: "PSS" },
    },
    {
      holder: "web",
      document: "vendor/pss-copy.yaml",
      provenance: "vendored-spec",
      target: { kind: "external", external: "api:pss.yaml", name: "PSS" },
    },
  ],
  externals: [
    {
      id: "api:pss.yaml",
      name: "PSS",
      copies: [
        { member: "api", document: "pss.yaml", title: "PSS" },
        { member: "pss-mock", document: "source.yaml", title: "PSS" },
        { member: "web", document: "vendor/pss-copy.yaml", title: "PSS" },
      ],
      declared_by: ["api", "web"],
      stand_ins: ["pss-mock"],
    },
    {
      id: "web:legacy/pss.yaml",
      name: "PSS",
      copies: [{ member: "web", document: "legacy/pss.yaml", title: "PSS" }],
      declared_by: ["web"],
      stand_ins: [],
    },
  ],
  collisions: [],
  resolved_ties: [],
};

/** The external join over {@link DECLARED_CONTRACTS} (S-459), in the estate's
 *  two base-path shapes: `api`'s call binds the first PSS under `/prov` from one
 *  deploy overlay (`pecserver-facade`), `web`'s binds the second PSS under the
 *  host-only base URL its application configuration commits in two profiles
 *  (`notification-adapter`); a third call is refused — judged, never drawn. */
export const BOUND_EXTERNAL: BoundExternal = {
  headline: {
    bound_external: 2,
    no_provider_rows: 3,
    accounting: {
      bound_external: 2,
      no_declared_external: 0,
      external_not_declared_by_member: 0,
      no_base_key: 0,
      base_path_uncommitted: 0,
      base_paths_disagree: 0,
      suffix_only: 0,
      no_match: 1,
      several_matches: 0,
    },
    summary:
      "2 of 3 invocation no-provider-in-workspace REST rows bound to a named external their own member declares (refused: 1 no match); declared by vendored specs, never a cross-service edge, and outside egress_resolution",
  },
  rows: [
    {
      from: { member: "api", symbol: "local fetch_mailbox" },
      target: "GET ${pss.uri-get-mailbox}",
      state: "bound-external",
      external: "api:pss.yaml",
      name: "PSS",
      document: "pss.yaml",
      operation: "GET /prov/domain/{}/user/{}",
      base: {
        path: "/prov",
        origin: "deploy-overlay",
        sources: [{ file: "deploy-coll/values.yaml", key: "envfrom.pssbaseurl" }],
      },
    },
    {
      from: { member: "web", symbol: "local send_legacy" },
      target: "POST ${legacy.uri-send}",
      state: "bound-external",
      external: "web:legacy/pss.yaml",
      name: "PSS",
      document: "legacy/pss.yaml",
      operation: "POST /v1/send",
      base: {
        path: "",
        origin: "application-config",
        sources: [
          { file: "src/main/resources/application.yml", key: "legacy.base-url" },
          { file: "src/test/resources/application-it.yml", key: "legacy.base-url" },
        ],
      },
    },
    {
      from: { member: "web", symbol: "local fetch_folder" },
      target: "GET /folder",
      state: "refused",
      reason: "no-match",
    },
  ],
};

/** No member has promoted a broker topic — the default, and the shape every repo
 *  that indexes no broker coupling reports (S-256). */
export const NO_TOPICS: MemberTopics[] = [];

/** A status fan-out over the two members. */
export function status(
  coverage: CrossServiceCoverage = EMPTY_COVERAGE,
  topics: MemberTopics[] = NO_TOPICS,
  degradedRollup?: WorkspaceStatus["degraded_rollup"],
  buildDependency?: BuildDependencyHeadline,
): WorkspaceStatus {
  return {
    workspace: "shop",
    members: [
      {
        member: "api",
        result: { indexed: true } as WorkspaceStatus["members"][0]["result"],
        warm_state: "warm",
        open_state: "opened",
      },
      {
        member: "web",
        result: { indexed: true } as WorkspaceStatus["members"][0]["result"],
        warm_state: "warm",
        open_state: "opened",
      },
    ],
    // Both members indexed, and no live warming signal exists — so `warming` is
    // absent rather than `0` (NFR-CC-04), exactly as the real payload omits it.
    warm_rollup: { members: 2, warm: 2, deferred: 0, degraded: 0 },
    // Both members opened, so every figure beside this roll-up covers all of them
    // (S-326, FR-WS-16).
    degraded_rollup: degradedRollup ?? {
      members: 2,
      opened: 2,
      not_attempted: 0,
      degraded_members: [],
      covers_all_members: true,
    },
    coverage,
    topics,
    // Only when given: the server omits the key over a manifest-less workspace
    // (S-463), and a fixture carrying it by default would let a view that cannot
    // cope with its absence pass.
    ...(buildDependency ? { build_dependency: buildDependency } : {}),
  };
}

/** What the stubbed surface should answer with. */
export interface StubOptions {
  /** The status of the shell's roster probe: `200` ⇒ workspace, `404` ⇒ single-root. */
  probeStatus?: number;
  coverage?: CrossServiceCoverage;
  /** The resolved cross-service bindings the service map draws. */
  providers?: unknown[];
  /** The cross-service impact payload. */
  impact?: unknown;
  /** Each member's promoted broker topics — the service map draws a node per topic. */
  topics?: MemberTopics[];
  /** Override the degraded roll-up, for the partially-opened-workspace cases
   *  (S-326, FR-WS-16). Defaults to the all-opened shape. */
  degradedRollup?: WorkspaceStatus["degraded_rollup"];
  /** The status payload's build headline (S-464); absent by default, as over a
   *  workspace with no build manifest. */
  buildDependency?: BuildDependencyHeadline;
  /** The `workspace/build-deps` answer (S-464). */
  buildDeps?: XserviceBuildDeps;
  /** Answer `workspace/build-deps` with this HTTP status instead of `buildDeps`
   *  (a failed read), or `"pending"` to never answer it (a read in flight). */
  buildDepsStatus?: number | "pending";
}

/**
 * Stub `fetch` over the `/api/v1` surface and return a recorder of every URL called.
 * Unmatched paths answer an empty `200`, so a view under test never fails on a read
 * the test does not care about.
 */
export function stubApi(opts: StubOptions = {}): () => string[] {
  const {
    probeStatus = 200,
    coverage = EMPTY_COVERAGE,
    providers = [],
    impact = {},
    topics = NO_TOPICS,
    degradedRollup,
    buildDependency,
    buildDeps,
    buildDepsStatus,
  } = opts;
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
      if (url.startsWith("/api/v1/workspace/status"))
        return json(status(coverage, topics, degradedRollup, buildDependency));
      if (url.startsWith("/api/v1/workspace/route-providers")) return json({ providers });
      if (url.startsWith("/api/v1/workspace/build-deps")) {
        if (buildDepsStatus === "pending") return new Promise<Response>(() => {});
        if (buildDepsStatus !== undefined) return json({ error: "boom" }, false, buildDepsStatus);
        return json(buildDeps ?? {});
      }
      if (url.startsWith("/api/v1/workspace/impact")) return json(impact);
      return json({});
    }),
  );
  return () => calls;
}
