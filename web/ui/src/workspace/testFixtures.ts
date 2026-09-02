/*
 * Shared workspace test fixtures (S-250) — one stubbed `/api/v1` surface every
 * workspace test drives, so the mode-discovery contract (roster 404 ⇒ single-root,
 * 200 ⇒ workspace) is stated once rather than re-invented per spec.
 */

import { vi } from "vitest";

import type {
  CrossServiceCoverage,
  MemberTopics,
  WorkspaceRoster,
  WorkspaceStatus,
} from "../api/types.ts";

/** The two-member roster the shell probe answers with. */
export const ROSTER: WorkspaceRoster = {
  workspace: "shop",
  default: "api",
  members: ["api", "web"],
};

/** An empty coverage summary over a fully-read two-member workspace.
 *
 *  `bound_ratio` is **omitted**, exactly as the server omits it when nothing was
 *  measured (S-326, FR-WS-05) — a fixture carrying `1` here would let a view that
 *  cannot cope with absence pass its tests. */
export const EMPTY_COVERAGE: CrossServiceCoverage = {
  references: [],
  bound: 0,
  ambiguous: 0,
  unbound: 0,
  no_provider_in_workspace: 0,
  members_read: 2,
  members_total: 2,
  covers_all_members: true,
};

/** No member has promoted a broker topic — the default, and the shape every repo
 *  that indexes no broker coupling reports (S-256). */
export const NO_TOPICS: MemberTopics[] = [];

/** A status fan-out over the two members. */
export function status(
  coverage: CrossServiceCoverage = EMPTY_COVERAGE,
  topics: MemberTopics[] = NO_TOPICS,
  degradedRollup?: WorkspaceStatus["degraded_rollup"],
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
        return json(status(coverage, topics, degradedRollup));
      if (url.startsWith("/api/v1/workspace/route-providers")) return json({ providers });
      if (url.startsWith("/api/v1/workspace/impact")) return json(impact);
      return json({});
    }),
  );
  return () => calls;
}
