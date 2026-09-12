import { describe, expect, it } from "vitest";

import type {
  BridgeIntake,
  ClassificationCounts,
  CrossServiceCoverage,
  ReferenceCoverage,
  UnboundReason,
} from "../../api/types.ts";
import {
  armLabel,
  buildCoverageDashboard,
  provenanceLabel,
  reasonLabel,
} from "./coverageModel.ts";

function bound(relation: string, n = 1, intake: BridgeIntake = "contract-surface"): ReferenceCoverage[] {
  return Array.from({ length: n }, (_v, i) => ({
    relation,
    from: { member: "api", symbol: `c${i}` },
    bucket: "bound" as const,
    state: "bound" as const,
    intake,
    // S-382: every row states where its target came from. These fixtures are
    // ordinary call-site literals.
    provenance: "literal" as const,
  }));
}

function unbound(
  relation: string,
  reason: UnboundReason,
  n = 1,
  intake: BridgeIntake = "contract-surface",
): ReferenceCoverage[] {
  return Array.from({ length: n }, (_v, i) => ({
    relation,
    from: { member: "api", symbol: `u-${reason}-${i}` },
    bucket: reason === "ambiguous" ? ("ambiguous" as const) : ("unbound" as const),
    state: "unbound" as const,
    reason,
    intake,
    provenance: "literal" as const,
  }));
}

/** Zero counts — the population a fixture does not exercise. */
function counts(over: Partial<ClassificationCounts> = {}): ClassificationCounts {
  return { bound: 0, ambiguous: 0, unbound: 0, no_provider_in_workspace: 0, ...over };
}

function coverage(references: ReferenceCoverage[], summary: Partial<CrossServiceCoverage> = {}): CrossServiceCoverage {
  return {
    references,
    bound: 0,
    ambiguous: 0,
    unbound: 0,
    no_provider_in_workspace: 0,
    by_intake: { contract_surface: counts(), invocation: counts() },
    spec_conformance_ratio: 1,
    spec_conformance_measured: 0,
    spec_conformance_summary: "",
    // The CR-120 headline, in its degenerate shape by default: a count of 0 with
    // `egress_resolution` OMITTED, exactly as the server sends it when no egress
    // site was captured. Individual cases override it through `summary`.
    resolved_cross_service_edges: 0,
    egress_resolution_measured: 0,
    resolved_edges_summary:
      "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
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
        { bound: 1, ambiguous: 1, unbound: 1, no_provider_in_workspace: 3, spec_conformance_ratio: 0.5 },
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

  it("displays the server's spec_conformance_ratio VERBATIM — it never recomputes it", () => {
    // The server excludes `no-provider-in-workspace` from the denominator (ADR-53):
    // 1 bound, 1 unbound, 3 no-provider → 1/2 = 0.5, NOT 1/5. A view that recomputed
    // naively would report 20% and silently contradict the CLI.
    const model = buildCoverageDashboard(
      coverage(
        [...bound("route", 1), ...unbound("route", "path-not-composed", 1), ...unbound("route", "no-provider-in-workspace", 3)],
        { bound: 1, unbound: 1, no_provider_in_workspace: 3, spec_conformance_ratio: 0.5 },
      ),
    );
    expect(model.specConformanceRatio).toBe(0.5);
    expect(model.noProviderInWorkspace).toBe(3);
  });

  // ── CR-111 / FR-WS-05: the ratio never travels without its scale ──────────

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
        spec_conformance_ratio: 6 / 7,
        spec_conformance_measured: 7,
        spec_conformance_summary: "0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)",
      }),
    );
    expect(model.specConformanceMeasured).toBe(7);
    expect(model.specConformanceSummary).toBe(
      "0.857 (6 of 7 measured; 899 excluded as no-provider-in-workspace)",
    );
  });

  // ── S-376 / CR-120 / BR-51: the headline is a resolved-edge count ──────────

  it("carries the resolved-edge headline and its egress rate VERBATIM", () => {
    // The reference estate's shape in miniature: `bound: 1` (a declared endpoint
    // matched to a controller) beside 0 resolved edges over 2 captured egress
    // sites. The model must not conflate them — that conflation is CR-120's defect.
    const model = buildCoverageDashboard(
      coverage([...bound("route", 1), ...unbound("route", "base-url-runtime", 2, "invocation")], {
        bound: 1,
        unbound: 2,
        by_intake: {
          contract_surface: counts({ bound: 1 }),
          invocation: counts({ unbound: 2 }),
        },
        resolved_cross_service_edges: 0,
        egress_resolution: 0,
        egress_resolution_measured: 2,
        resolved_edges_summary:
          "0 resolved cross-service edges; egress resolution 0.000 (0 of 2 egress sites resolved)",
      }),
    );
    expect(model.resolvedCrossServiceEdges).toBe(0);
    expect(model.egressResolution).toBe(0);
    expect(model.egressResolutionMeasured).toBe(2);
    expect(model.resolvedEdgesSummary).toBe(
      "0 resolved cross-service edges; egress resolution 0.000 (0 of 2 egress sites resolved)",
    );
    // …and the pooled ratio still reads healthy over the same data, which is why
    // the headline moved: 1 bound of 3 measured is 0.333, and none of it is a
    // resolved call.
    expect(model.bound).toBe(1);
  });

  it("carries an ABSENT egress resolution through as null, never as a number", () => {
    // The CR-100 rule on the successor figure. `0` would claim every captured call
    // failed to resolve and `1` that every one succeeded; the truth is that none
    // was captured, so the rate has no denominator.
    const { egress_resolution: _omitted, ...withoutRate } = coverage([...bound("route", 1)], {
      bound: 1,
      egress_resolution: 0,
      egress_resolution_measured: 0,
      resolved_edges_summary:
        "0 resolved cross-service edges; egress resolution not measured (0 of 0 egress sites)",
    });
    const model = buildCoverageDashboard(withoutRate as CrossServiceCoverage);

    expect(model.egressResolution).toBeNull();
    expect(model.egressResolutionMeasured).toBe(0);
    expect(model.resolvedCrossServiceEdges).toBe(0);
    expect(model.resolvedEdgesSummary).toContain("not measured");
  });

  it("flags the ratio as dominated-by-excluded when the excluded bucket outweighs the denominator", () => {
    const dominated = buildCoverageDashboard(
      coverage([], { bound: 6, unbound: 1, no_provider_in_workspace: 899, spec_conformance_measured: 7 }),
    );
    expect(dominated.ratioDominatedByExcluded).toBe(true);

    const healthy = buildCoverageDashboard(
      coverage([], { bound: 9, ambiguous: 1, no_provider_in_workspace: 2, spec_conformance_measured: 10 }),
    );
    expect(healthy.ratioDominatedByExcluded).toBe(false);

    // The boundary: excluded EQUALS measured. "Dominates" means outweighs, not
    // ties — pins the strict `>` comparison against an accidental `>=`.
    const tied = buildCoverageDashboard(
      coverage([], { bound: 6, unbound: 1, no_provider_in_workspace: 7, spec_conformance_measured: 7 }),
    );
    expect(tied.ratioDominatedByExcluded).toBe(false);
  });

  it("still exposes the denominator and excluded count when the ratio itself is absent (S-327)", () => {
    const { spec_conformance_ratio: _omitted, ...withoutRatio } = coverage([], {
      no_provider_in_workspace: 899,
      spec_conformance_measured: 0,
      spec_conformance_summary: "0 of 0 measured; 899 excluded as no-provider-in-workspace",
    });
    const model = buildCoverageDashboard(withoutRatio as CrossServiceCoverage);

    expect(model.specConformanceRatio).toBeNull();
    expect(model.specConformanceMeasured).toBe(0);
    expect(model.specConformanceSummary).toBe("0 of 0 measured; 899 excluded as no-provider-in-workspace");
    expect(model.ratioDominatedByExcluded).toBe(true);
  });

  it("is honestly empty when the workspace has no cross-boundary reference at all", () => {
    const model = buildCoverageDashboard(coverage([]));
    expect(model.isEmpty).toBe(true);
    expect(model.arms).toEqual([]);
  });

  // ── S-326 / FR-WS-05 / NFR-CC-04: absence is not a score ──────────────────

  it("carries an ABSENT spec_conformance_ratio through as null, never as a number", () => {
    // The server omits the key when its denominator is 0. Defaulting it to 0 or 1
    // here would reinstate exactly the fabrication CR-100 filed: `bound: 0` beside
    // a perfect ratio, over a workspace that was three-quarters unopened.
    const { spec_conformance_ratio: _omitted, ...withoutRatio } = coverage([
      ...unbound("route", "no-provider-in-workspace", 3),
    ]);
    const model = buildCoverageDashboard({
      ...withoutRatio,
      no_provider_in_workspace: 3,
    } as CrossServiceCoverage);

    expect(model.specConformanceRatio).toBeNull();
    expect(model.noProviderInWorkspace).toBe(3);
  });

  it("passes the partial-coverage marker through so a view can label the shortfall", () => {
    const model = buildCoverageDashboard(
      coverage([...bound("route", 1)], {
        bound: 1,
        spec_conformance_ratio: 1,
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
    const summary = { bound: 1, ambiguous: 1, spec_conformance_ratio: 0.5, spec_conformance_measured: 2 };

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

  // ── S-377 / CR-120: the intake split ──────────────────────────────────────

  it("carries the server's intake split VERBATIM, never regrouped from the rows", () => {
    // Deliberately inconsistent with the rows below: the server DERIVES its
    // headline from `by_intake`, so a model that regrouped the references here
    // would be a second implementation of that arithmetic, free to disagree with
    // the headline inches away on the same screen. The only way to catch that is
    // to hand it a split the rows do not imply and require the split to win.
    const split = {
      contract_surface: counts({ bound: 7, no_provider_in_workspace: 1 }),
      invocation: counts({ unbound: 3 }),
    };
    const model = buildCoverageDashboard(
      coverage([...bound("route", 1, "invocation")], { by_intake: split }),
    );
    expect(model.byIntake).toEqual(split);
  });

  it("tells an unresolved invocation population apart from an absent one", () => {
    // The reference workspace's shape: call sites exist and none of them binds.
    const unresolved = buildCoverageDashboard(
      coverage([...bound("route", 81), ...unbound("route", "base-url-runtime", 5, "invocation")], {
        by_intake: {
          contract_surface: counts({ bound: 81 }),
          invocation: counts({ unbound: 5 }),
        },
      }),
    );
    expect(unresolved.hasInvocationReferences).toBe(true);
    expect(unresolved.byIntake.invocation.bound).toBe(0);

    // The same zero over an estate with no captured call site at all — honest
    // absence. A view must not report the two the same way (NFR-CC-04), so the
    // model has to distinguish them and a bare `invocation.bound === 0` cannot.
    const absent = buildCoverageDashboard(
      coverage([...bound("route", 81)], {
        by_intake: { contract_surface: counts({ bound: 81 }), invocation: counts() },
      }),
    );
    expect(absent.hasInvocationReferences).toBe(false);
    expect(absent.byIntake.invocation.bound).toBe(0);
  });

  it("counts an invocation reference in EVERY bucket toward the population's presence", () => {
    // Not just `bound`: a population whose every reference is ambiguous, unbound
    // or no-provider is still present, and reporting it as absent would call a
    // total resolution failure "no call sites here" — the worse of the two
    // misreadings, because it makes the failure invisible.
    for (const bucket of ["bound", "ambiguous", "unbound", "no_provider_in_workspace"] as const) {
      const model = buildCoverageDashboard(
        coverage([], {
          by_intake: { contract_surface: counts(), invocation: counts({ [bucket]: 1 }) },
        }),
      );
      expect(model.hasInvocationReferences, `an invocation row in ${bucket} counts`).toBe(true);
    }
  });
});

describe("provenanceLabel (S-382, ADR-64)", () => {
  const row = (extra: object): ReferenceCoverage =>
    ({
      relation: "route",
      from: { member: "web", symbol: "fetch_order" },
      bucket: "bound",
      state: "bound",
      intake: "invocation",
      ...extra,
    }) as ReferenceCoverage;

  it("tells an observed literal from an admitted configuration value", () => {
    expect(provenanceLabel(row({ provenance: "literal" }))).toBe("Written at the call site");
    expect(
      provenanceLabel(
        row({
          provenance: "config-bound",
          bound: [
            {
              key: "orders.base",
              source: "placeholder",
              values: [
                { value: "/orders", profiles: ["docker"], unprofiled: false, sources: ["a.yml"] },
              ],
            },
          ],
        }),
      ),
    ).toBe("Read from `orders.base` (docker)");
  });

  it("says that EVERY overlay value is carried, never showing one as the value", () => {
    // ADR-64 decision point 3: a divergent key is admitted with all of its
    // values, so the dashboard must not render the first as though it were THE
    // value. The count and the profile set are what make that visible.
    const label = provenanceLabel(
      row({
        provenance: "config-bound",
        bound: [
          {
            key: "orders.base",
            source: "properties",
            values: [
              { value: "/orders", profiles: [], unprofiled: true, sources: ["application.yml"] },
              {
                value: "/orders-it",
                profiles: ["it"],
                unprofiled: false,
                sources: ["application-it.yml"],
              },
            ],
          },
        ],
      }),
    );
    expect(label).toBe("Read from `orders.base` — 2 values, one per overlay (it)");
    expect(label).not.toContain("/orders-it");
  });

  it("names the keys a refused configuration reference proved only an indirection to", () => {
    expect(
      provenanceLabel(
        row({
          provenance: "config-unresolved",
          keys: ["orders.base", "orders.version"],
          refusal: "missing-key",
        }),
      ),
    ).toBe(
      "Names `orders.base`, `orders.version`, which the committed sources do not admit",
    );
  });

  it("renders an unrecognised provenance verbatim rather than as an empty statement", () => {
    // The wire payload is not runtime-validated and a later story may add a
    // state. A target shown with NO statement of where it came from is the
    // indistinguishability ADR-64 forbids, so the token stands in for the label.
    expect(provenanceLabel(row({ provenance: "read-from-the-future" }))).toBe(
      "read-from-the-future",
    );
  });

  it("labels both configuration reasons, so neither count is shown bare", () => {
    expect(reasonLabel("config-key-missing")).toBe(
      "No committed source defines the configuration key",
    );
    expect(reasonLabel("config-placeholder-value")).toBe(
      "The committed value is itself a placeholder",
    );
  });
});

describe("per-arm provenance breakdown (S-382, ADR-64)", () => {
  it("counts every row's provenance, so an arm that reads no configuration says so", () => {
    const model = buildCoverageDashboard(coverage(bound("route", 2)));
    expect(model.arms[0].provenance).toEqual([
      { label: "Written at the call site", count: 2 },
    ]);
  });

  it("names the key an admitted row was read from, beside the literal rows", () => {
    const literal = bound("route", 1);
    const admitted: ReferenceCoverage[] = [
      {
        relation: "route",
        from: { member: "web", symbol: "fetch" },
        bucket: "bound",
        state: "bound",
        intake: "invocation",
        provenance: "config-bound",
        bound: [
          {
            key: "orders.base",
            source: "placeholder",
            values: [
              { value: "/orders", profiles: ["docker"], unprofiled: false, sources: ["a.yml"] },
            ],
          },
        ],
      },
    ];
    const model = buildCoverageDashboard(coverage([...literal, ...admitted]));
    expect(model.arms[0].provenance).toEqual([
      { label: "Read from `orders.base` (docker)", count: 1 },
      { label: "Written at the call site", count: 1 },
    ]);
  });

  it("keeps two different keys apart rather than collapsing them to one count", () => {
    // Two `config-bound` rows reading DIFFERENT keys are different statements.
    // Counting them as one "config-bound: 2" would hide exactly what ADR-64
    // requires the surface to show.
    const rows: ReferenceCoverage[] = ["orders.base", "users.base"].map((key, i) => ({
      relation: "route",
      from: { member: "web", symbol: `fetch${i}` },
      bucket: "bound",
      state: "bound",
      intake: "invocation",
      provenance: "config-bound",
      bound: [
        {
          key,
          source: "placeholder",
          values: [{ value: "/x", profiles: [], unprofiled: true, sources: ["a.yml"] }],
        },
      ],
    }));
    const model = buildCoverageDashboard(coverage(rows));
    expect(model.arms[0].provenance.map((p) => p.label)).toEqual([
      "Read from `orders.base`",
      "Read from `users.base`",
    ]);
  });
});
