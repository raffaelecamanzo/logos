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
    bound_ratio_measured: 0,
    bound_ratio_summary: "",
    members_read: 2,
    members_total: 2,
    covers_all_members: true,
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

  // ── CR-111 / FR-WS-05: the bound-ratio never travels without its scale ─────

  it("carries the server's explicit denominator and composed summary line VERBATIM", () => {
    // The exact CR-111 headline: bound: 6, ambiguous: 0, unbound: 1 (denominator 7),
    // no_provider_in_workspace: 899 — the pec-services numbers, pinned verbatim here
    // and at the core (`coverage.rs`) and `WorkspaceView.test.tsx` layers, where a
    // 906-reference fixture is likewise constructible (unlike the CLI/web-serve
    // integration tests, which drive a real indexed fixture and use its own, much
    // smaller numbers).
    const model = buildCoverageDashboard(
      coverage([...bound("route", 6), ...unbound("route", "path-not-composed", 1), ...unbound("route", "no-provider-in-workspace", 899)], {
        bound: 6,
        unbound: 1,
        no_provider_in_workspace: 899,
        bound_ratio: 6 / 7,
        bound_ratio_measured: 7,
        bound_ratio_summary: "0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)",
      }),
    );
    expect(model.boundRatioMeasured).toBe(7);
    expect(model.boundRatioSummary).toBe(
      "0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)",
    );
  });

  it("flags the ratio as dominated-by-excluded when the excluded bucket outweighs the denominator", () => {
    const dominated = buildCoverageDashboard(
      coverage([], { bound: 6, unbound: 1, no_provider_in_workspace: 899, bound_ratio_measured: 7 }),
    );
    expect(dominated.ratioDominatedByExcluded).toBe(true);

    const healthy = buildCoverageDashboard(
      coverage([], { bound: 9, ambiguous: 1, no_provider_in_workspace: 2, bound_ratio_measured: 10 }),
    );
    expect(healthy.ratioDominatedByExcluded).toBe(false);

    // The boundary: excluded EQUALS measured. "Dominates" means outweighs, not
    // ties — pins the strict `>` comparison against an accidental `>=`.
    const tied = buildCoverageDashboard(
      coverage([], { bound: 6, unbound: 1, no_provider_in_workspace: 7, bound_ratio_measured: 7 }),
    );
    expect(tied.ratioDominatedByExcluded).toBe(false);
  });

  it("still exposes the denominator and excluded count when the ratio itself is absent (S-327)", () => {
    const { bound_ratio: _omitted, ...withoutRatio } = coverage([], {
      no_provider_in_workspace: 899,
      bound_ratio_measured: 0,
      bound_ratio_summary: "0 of 0 measured; 899 excluded as no-provider-in-workspace",
    });
    const model = buildCoverageDashboard(withoutRatio as CrossServiceCoverage);

    expect(model.boundRatio).toBeNull();
    expect(model.boundRatioMeasured).toBe(0);
    expect(model.boundRatioSummary).toBe("0 of 0 measured; 899 excluded as no-provider-in-workspace");
    expect(model.ratioDominatedByExcluded).toBe(true);
  });

  it("is honestly empty when the workspace has no cross-boundary reference at all", () => {
    const model = buildCoverageDashboard(coverage([]));
    expect(model.isEmpty).toBe(true);
    expect(model.arms).toEqual([]);
  });

  // ── S-326 / FR-WS-05 / NFR-CC-04: absence is not a score ──────────────────

  it("carries an ABSENT bound_ratio through as null, never as a number", () => {
    // The server omits the key when its denominator is 0. Defaulting it to 0 or 1
    // here would reinstate exactly the fabrication CR-100 filed: `bound: 0` beside
    // a perfect ratio, over a workspace that was three-quarters unopened.
    const { bound_ratio: _omitted, ...withoutRatio } = coverage([
      ...unbound("route", "no-provider-in-workspace", 3),
    ]);
    const model = buildCoverageDashboard({
      ...withoutRatio,
      no_provider_in_workspace: 3,
    } as CrossServiceCoverage);

    expect(model.boundRatio).toBeNull();
    expect(model.noProviderInWorkspace).toBe(3);
  });

  it("passes the partial-coverage marker through so a view can label the shortfall", () => {
    const model = buildCoverageDashboard(
      coverage([...bound("route", 1)], {
        bound: 1,
        bound_ratio: 1,
        members_read: 9,
        members_total: 72,
        covers_all_members: false,
      }),
    );
    expect(model.coversAllMembers).toBe(false);
    expect([model.membersRead, model.membersTotal]).toEqual([9, 72]);
  });

  it("reports a fully-read workspace as covering all members", () => {
    // The complement of the case above, asserted on a real (non-cast) payload:
    // the three marker fields are non-optional on the wire, so there is no
    // "absent marker" case to defend against — see `coverageModel.ts`.
    const model = buildCoverageDashboard(
      coverage([...bound("route", 1)], { bound: 1, members_read: 2, members_total: 2 }),
    );

    expect(model.coversAllMembers).toBe(true);
    expect([model.membersRead, model.membersTotal]).toEqual([2, 2]);
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

  it("labels the broker arm's topic-not-literal refusal (CR-107)", () => {
    // The reason a refused broker topic now arrives under. It must read as words,
    // not as a bare wire token, because the whole point of recording the refusal is
    // that "topics: []" stopped being indistinguishable from "no broker here"
    // (NFR-CC-04) — and a token nobody can read reintroduces that.
    expect(reasonLabel("topic-not-literal")).toMatch(/not a static literal/i);
    // And it groups like any other reason, inside the unbound bucket.
    const model = buildCoverageDashboard(
      coverage(unbound("broker-topic", "topic-not-literal", 3)),
    );
    expect(model.arms[0].unbound).toBe(3);
    expect(model.arms[0].reasons).toEqual([
      { reason: "topic-not-literal", count: 3 },
    ]);
  });

  // ── CR-118: the provider-identity riders are OPTIONAL ─────────────────────

  it("builds the identical dashboard from rows with and without the CR-118 provider fields", () => {
    // The two shapes a real deployment serves: an OLDER store's rows, which
    // predate `to`/`intake`/`candidates` entirely, and a current store's rows,
    // which carry them. CR-118 §4.5 makes the fields optional precisely so the
    // view does not break on the first shape — and this asserts the stronger
    // claim the CR actually needs: the aggregation is IDENTICAL either way, so
    // the widened payload cannot silently move a count on this screen.
    const withoutFields = [...bound("route", 1), ...unbound("route", "ambiguous", 1)];
    const withFields: ReferenceCoverage[] = [
      {
        ...withoutFields[0],
        to: { member: "web", symbol: "route_get" },
        intake: "contract-surface",
      },
      {
        ...withoutFields[1],
        candidates: {
          disposition: "tied-between",
          providers: [
            { member: "mailbox-aggregator-api", symbol: "route_agg" },
            { member: "mailbox-core", symbol: "route_core" },
          ],
          total: 2,
          omitted: 0,
          summary: "2 tied providers, all listed; none bound",
        },
      },
    ];
    const summary = { bound: 1, ambiguous: 1, bound_ratio: 0.5, bound_ratio_measured: 2 };

    const older = buildCoverageDashboard(coverage(withoutFields, summary));
    const current = buildCoverageDashboard(coverage(withFields, summary));

    expect(older).toEqual(current);
    // And the old shape still produces the figures it always did — the guard is
    // not merely "the two agree", which two identically-broken models would also
    // satisfy.
    expect(older.arms[0].bound).toBe(1);
    expect(older.arms[0].ambiguous).toBe(1);
    expect(older.arms[0].total).toBe(2);
  });
});
