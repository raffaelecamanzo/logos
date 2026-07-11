import { describe, expect, it } from "vitest";

import type { CrossServiceCoverage, ReferenceCoverage, UnboundReason } from "../../api/types.ts";
import { armLabel, buildCoverageDashboard, reasonLabel } from "./coverageModel.ts";

function bound(relation: string, n = 1): ReferenceCoverage[] {
  return Array.from({ length: n }, (_v, i) => ({
    relation,
    from: { member: "api", symbol: `c${i}` },
    bucket: "bound" as const,
    state: "bound" as const,
  }));
}

function unbound(relation: string, reason: UnboundReason, n = 1): ReferenceCoverage[] {
  return Array.from({ length: n }, (_v, i) => ({
    relation,
    from: { member: "api", symbol: `u-${reason}-${i}` },
    bucket: reason === "ambiguous" ? ("ambiguous" as const) : ("unbound" as const),
    state: "unbound" as const,
    reason,
  }));
}

function coverage(references: ReferenceCoverage[], summary: Partial<CrossServiceCoverage> = {}): CrossServiceCoverage {
  return {
    references,
    bound: 0,
    ambiguous: 0,
    unbound: 0,
    no_provider_in_workspace: 0,
    bound_ratio: 1,
    ...summary,
  };
}

describe("buildCoverageDashboard (S-250, FR-UI-29, FR-WS-05)", () => {
  it("groups references into per-arm bound / ambiguous / unbound rows", () => {
    const model = buildCoverageDashboard(
      coverage([
        ...bound("route", 3),
        ...unbound("route", "ambiguous", 1),
        ...unbound("route", "path-not-composed", 2),
        ...bound("grpc-call", 1),
      ]),
    );
    const route = model.arms.find((a) => a.relation === "route")!;
    expect(route).toMatchObject({ bound: 3, ambiguous: 1, unbound: 2, noProvider: 0, total: 6 });
    const grpc = model.arms.find((a) => a.relation === "grpc-call")!;
    expect(grpc).toMatchObject({ bound: 1, ambiguous: 0, unbound: 0, total: 1 });
    // Arms sort by relation, so the board is stable across runs (NFR-RA-06).
    expect(model.arms.map((a) => a.relation)).toEqual(["grpc-call", "route"]);
  });

  it("keeps `ambiguous` as its own bucket — never folded into unbound", () => {
    const model = buildCoverageDashboard(coverage(unbound("route", "ambiguous", 2)));
    const route = model.arms[0];
    expect(route.ambiguous).toBe(2);
    expect(route.unbound).toBe(0);
    // …but it IS still a reason on the row, so the "why" is never lost.
    expect(route.reasons).toEqual([{ reason: "ambiguous", count: 2 }]);
  });

  it("groups unbound references by reason, commonest first (ties by name — deterministic)", () => {
    const model = buildCoverageDashboard(
      coverage([
        ...unbound("route", "base-url-runtime", 1),
        ...unbound("route", "no-provider-in-workspace", 3),
        ...unbound("route", "path-not-composed", 1),
      ]),
    );
    expect(model.arms[0].reasons).toEqual([
      { reason: "no-provider-in-workspace", count: 3 },
      { reason: "base-url-runtime", count: 1 },
      { reason: "path-not-composed", count: 1 },
    ]);
  });

  it("splits no-provider out of the arm's unbound, so the rows RECONCILE with the summary", () => {
    // The wire `bucket` is "unbound" for a no-provider reference, but the server's
    // `unbound` COUNTER excludes it (ADR-53). An arm row that folded them in would
    // print a different figure for the same quantity, inches from the headline.
    const model = buildCoverageDashboard(
      coverage(
        [
          ...bound("route", 1),
          ...unbound("route", "path-not-composed", 1),
          ...unbound("route", "no-provider-in-workspace", 3),
          ...unbound("grpc-call", "ambiguous", 1),
        ],
        { bound: 1, ambiguous: 1, unbound: 1, no_provider_in_workspace: 3, bound_ratio: 0.5 },
      ),
    );
    const sum = (f: (a: (typeof model.arms)[number]) => number) =>
      model.arms.reduce((n, a) => n + f(a), 0);
    expect(sum((a) => a.bound)).toBe(model.bound);
    expect(sum((a) => a.ambiguous)).toBe(model.ambiguous);
    expect(sum((a) => a.unbound)).toBe(model.unbound);
    expect(sum((a) => a.noProvider)).toBe(model.noProviderInWorkspace);

    const route = model.arms.find((a) => a.relation === "route")!;
    expect(route).toMatchObject({ bound: 1, ambiguous: 0, unbound: 1, noProvider: 3, total: 5 });
  });

  it("displays the server's bound_ratio VERBATIM — it never recomputes it", () => {
    // The server excludes `no-provider-in-workspace` from the denominator (ADR-53):
    // 1 bound, 1 unbound, 3 no-provider → 1/2 = 0.5, NOT 1/5. A view that recomputed
    // naively would report 20% and silently contradict the CLI.
    const model = buildCoverageDashboard(
      coverage(
        [...bound("route", 1), ...unbound("route", "path-not-composed", 1), ...unbound("route", "no-provider-in-workspace", 3)],
        { bound: 1, unbound: 1, no_provider_in_workspace: 3, bound_ratio: 0.5 },
      ),
    );
    expect(model.boundRatio).toBe(0.5);
    expect(model.noProviderInWorkspace).toBe(3);
  });

  it("is honestly empty when the workspace has no cross-boundary reference at all", () => {
    const model = buildCoverageDashboard(coverage([]));
    expect(model.isEmpty).toBe(true);
    expect(model.arms).toEqual([]);
  });

  it("shows an unknown (future-arm) relation verbatim rather than dropping it", () => {
    const model = buildCoverageDashboard(coverage(bound("topic-v2", 1)));
    expect(model.arms[0].relation).toBe("topic-v2");
    expect(armLabel("topic-v2")).toBe("topic-v2");
    expect(armLabel("route")).toMatch(/HTTP/);
  });

  it("shows an unknown (future) unbound reason verbatim — a count with no explanation is the bug", () => {
    // The wire payload is not runtime-validated, so a reason this build does not know
    // must still read as something. An empty label beside a count is exactly the
    // unexplained figure the reason buckets exist to prevent (NFR-CC-04).
    expect(reasonLabel("some-new-reason")).toBe("some-new-reason");
    expect(reasonLabel("ambiguous")).toMatch(/Two or more providers/);
  });
});
