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

export const EMPTY_COVERAGE: CrossServiceCoverage = {
  references: [],
  bound: 0,
  ambiguous: 0,
  unbound: 0,
  no_provider_in_workspace: 0,
  bound_ratio: 1,
};

/** No member has promoted a broker topic — the default, and the shape every repo
 *  that indexes no broker coupling reports (S-256). */
export const NO_TOPICS: MemberTopics[] = [];

/** A status fan-out over the two members. */
export function status(
  coverage: CrossServiceCoverage = EMPTY_COVERAGE,
  topics: MemberTopics[] = NO_TOPICS,
): WorkspaceStatus {
  return {
    workspace: "shop",
    members: [
      { member: "api", result: { indexed: true } as WorkspaceStatus["members"][0]["result"] },
      { member: "web", result: { indexed: true } as WorkspaceStatus["members"][0]["result"] },
    ],
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
      if (url.startsWith("/api/v1/workspace/status")) return json(status(coverage, topics));
      if (url.startsWith("/api/v1/workspace/route-providers")) return json({ providers });
      if (url.startsWith("/api/v1/workspace/impact")) return json(impact);
      return json({});
    }),
  );
  return () => calls;
}
