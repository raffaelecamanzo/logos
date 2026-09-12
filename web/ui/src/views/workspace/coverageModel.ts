/*
 * Pure cross-service coverage model (S-250, CR-061, FR-UI-29, FR-WS-05) — the
 * dashboard's arithmetic, lifted out of the view so it is unit-testable without a
 * DOM and can never quietly drift from the server's own figures.
 *
 * It **projects**, it does not recompute. Every count comes from the read-model's
 * classified `references`; the model only groups them by arm and by reason. In
 * particular it never re-derives `spec_conformance_ratio` — that ratio deliberately
 * excludes `no-provider-in-workspace` from its denominator (a call to a service
 * outside this workspace is not a broken binding, ADR-53), and a view that
 * recomputed it naively would silently contradict the CLI and the gate-adjacent
 * figures. The server's ratio, its resolved-edge headline and the egress rate are
 * all displayed verbatim.
 *
 * `bound_ratio` was retired by CR-120: it pooled declared-contract matches with
 * resolved call sites in one numerator, so it read 0.287 over an estate with zero
 * caller→callee edges. The headline is now `resolvedCrossServiceEdges` with
 * `egressResolution` beside it — never one without the other (BR-51) — and the old
 * formula survives as `specConformanceRatio`.
 *
 * Honest empties (NFR-CC-04): a workspace with no cross-boundary references at all
 * is `isEmpty` — the view then says "no cross-service references yet" rather than
 * rendering a 100%-bound score bar over nothing. Since S-326 the server also sends
 * `spec_conformance_ratio` **absent** whenever nothing was measured (its denominator
 * is 0), and `egress_resolution` likewise when no egress site was captured, so both
 * are `number | null` here and the view must render "not measured"
 * rather than a bar. `coversAllMembers` is the other honesty rider: `false` means
 * every count in this model was computed over fewer than all workspace members —
 * their contract surface did not contribute, either because the member's store
 * could not be opened OR because it opened and the surface read itself failed
 * (FR-WS-16). It does NOT identify which, so a view must not name a cause from
 * it; `degraded_rollup.degraded_members` is the field that knows.
 */

import type {
  ClassificationCounts,
  CrossServiceCoverage,
  IntakeSplit,
  ReferenceCoverage,
  UnboundReason,
} from "../../api/types.ts";

/** The human label for each unbound reason (the wire tokens are kebab-case). */
export const REASON_LABEL: Record<UnboundReason, string> = {
  "no-provider-in-workspace": "No provider in this workspace",
  "path-not-composed": "Path could not be composed",
  "base-url-runtime": "Base URL resolved at runtime",
  ambiguous: "Two or more providers (ambiguous)",
  "topic-not-literal": "Broker topic is not a static literal",
  "config-key-missing": "No committed source defines the configuration key",
  "config-placeholder-value": "The committed value is itself a placeholder",
};

/** The human label for each relation arm. Unknown arms (a later M-milestone's) are
 *  shown verbatim rather than dropped — the dashboard must not hide an arm it does
 *  not yet have a pretty name for. */
export const ARM_LABEL: Record<string, string> = {
  route: "HTTP (OpenAPI ↔ route)",
  "grpc-call": "gRPC (stub ↔ service)",
  "broker-topic": "Broker (publish ↔ subscribe)",
};

/** The display name of a relation arm. */
export function armLabel(relation: string): string {
  return ARM_LABEL[relation] ?? relation;
}

/** The display name of an unbound reason. Like {@link armLabel}, an unrecognised
 *  token (a reason a later arm adds server-side) is shown VERBATIM rather than
 *  rendering as an empty label beside a count — a count with no explanation is the
 *  very thing the reason buckets exist to prevent (NFR-CC-04). The wire payload is
 *  not runtime-validated, so this is reachable without a version skew being a bug. */
export function reasonLabel(reason: string): string {
  return REASON_LABEL[reason as UnboundReason] ?? reason;
}

/** How one coverage row's target was obtained, in words (S-382, ADR-64,
 *  NFR-CC-04).
 *
 *  ADR-64 requires an admitted value never to be indistinguishable from an
 *  observed one, and this is where the dashboard honours that: a row read from
 *  committed configuration says so, names the key it was read from, and — when
 *  the overlays disagree — says that EVERY one of its values is carried, rather
 *  than showing one of them as though it were the value.
 *
 *  A row whose `provenance` this build does not recognise renders as the bare
 *  token rather than as an empty string, for the same reason {@link reasonLabel}
 *  does: the wire payload is not runtime-validated, and a target shown with no
 *  statement of where it came from is the indistinguishability ADR-64 forbids. */
export function provenanceLabel(ref: ReferenceCoverage): string {
  switch (ref.provenance) {
    case "literal":
      return "Written at the call site";
    case "config-bound": {
      // `bound` is one entry PER KEY the target names, and each entry's `values`
      // is one entry per distinct committed value of that key. Both are read:
      // rendering only the first key would hide a second key's evidence, and
      // rendering only the first value would show an overlay divergence as
      // though the repository proved one value (ADR-64).
      const keys = ref.bound ?? [];
      const profiles = [
        ...new Set(keys.flatMap((b) => b.values.flatMap((v) => v.profiles))),
      ].sort();
      const where = profiles.length > 0 ? ` (${profiles.join(", ")})` : "";
      const names = keys.map((b) => `\`${b.key}\``).join(", ");
      const values = keys.reduce((n, b) => n + b.values.length, 0);
      if (keys.length === 0) {
        return "Read from committed configuration";
      }
      return values > keys.length
        ? `Read from ${names} — ${values} values, one per overlay${where}`
        : `Read from ${names}${where}`;
    }
    case "config-unresolved":
      return `Names \`${ref.keys.join("`, `")}\`, which the committed sources do not admit`;
    default:
      return (ref as { provenance: string }).provenance;
  }
}

/** One value provenance and how many of an arm's references carry it (S-382).
 *
 *  Keyed by the rendered {@link provenanceLabel} rather than by the raw
 *  `provenance` token, because two `config-bound` rows reading different keys
 *  are different statements and collapsing them to one count would hide exactly
 *  what ADR-64 requires the surface to show. */
export interface ProvenanceCount {
  label: string;
  count: number;
}

/** One unbound reason and how many references carry it. */
export interface ReasonCount {
  reason: UnboundReason;
  count: number;
}

/** The wire reason that is bucketed out of the ratio's denominator (ADR-53) — a
 *  reference to a service outside this workspace is not a *broken* binding. */
const NO_PROVIDER: UnboundReason = "no-provider-in-workspace";

/** Coverage for one relation arm — the dashboard's per-arm row (FR-UI-29 AC3).
 *
 * The four count fields partition the arm's references exactly as the server's own
 * summary counters partition the workspace's: `bound + ambiguous + unbound +
 * noProvider === total`, and each column SUMS ACROSS ARMS to its summary
 * counterpart. That reconciliation is the point — the arm board and the headline
 * sit on the same screen, so a row that folded `no-provider-in-workspace` into
 * `unbound` (the wire `bucket` does; the server's `unbound` counter does not) would
 * state a different figure for the same quantity, inches apart. */
export interface ArmCoverage {
  /** The relation arm (`route`, `grpc-call`, `broker-topic`, …). */
  relation: string;
  bound: number;
  ambiguous: number;
  /** Unbound for a reason other than ambiguity or no-provider — each of which is
   *  its own bucket, exactly as in the server's summary (ADR-53). */
  unbound: number;
  /** No provider anywhere in this workspace — reported apart, and excluded from the
   *  bound ratio's denominator (ADR-53). */
  noProvider: number;
  /** Every reason present on this arm's non-bound references, commonest first. */
  reasons: ReasonCount[];
  /** Where this arm's reference targets came from, commonest first (S-382,
   *  ADR-64).
   *
   *  This is the dashboard's half of *"an admitted value must never be
   *  indistinguishable from an observed one"*. An arm whose targets are all
   *  written at the call site reads one way; an arm resolving through committed
   *  configuration reads another, and names the keys it read. Empty only when
   *  the arm has no references at all. */
  provenance: ProvenanceCount[];
  /** Every reference on this arm — the denominator the row's counts sum to. */
  total: number;
}

/** The dashboard's model: the verbatim server summary plus the per-arm breakdown. */
export interface CoverageDashboard {
  /** `bound / (bound + ambiguous + unbound)` as the server computed it — displayed,
   *  never recomputed (see the module docs).
   *
   *  `null` when the server measured nothing (a zero denominator). Render that as
   *  "not measured": a `0` bar would claim nothing bound and a full bar would claim
   *  everything did, and the truth is that there was nothing to bind (NFR-CC-04). */
  specConformanceRatio: number | null;
  bound: number;
  ambiguous: number;
  unbound: number;
  /** References with no provider anywhere in this workspace — reported beside the
   *  ratio, deliberately outside its denominator (ADR-53). */
  noProviderInWorkspace: number;
  /** The denominator `specConformanceRatio` was computed over (`bound + ambiguous +
   *  unbound`), carried verbatim from the server's explicit field rather than
   *  summed here — the same "displayed, never recomputed" discipline as
   *  `specConformanceRatio` itself (CR-111). Present even when it is `null`. */
  specConformanceMeasured: number;
  /** The spec-conformance ratio's server-composed "never bare" line — its value
   *  (when present) plus the denominator and excluded count, verbatim (CR-111). */
  specConformanceSummary: string;
  /** **The headline** (CR-120): cross-service edges resolved from a captured
   *  invocation — a caller→callee call, a producer→consumer publish. Not `bound`,
   *  which also counts declared-contract matches, and not a count of sites: one
   *  fan-out publish is several edges. */
  resolvedCrossServiceEdges: number;
  /** The rate at which captured egress sites resolved at all, verbatim from the
   *  server. `null` when no egress site was captured — render "not measured",
   *  never a bar (BR-51, CR-100). */
  egressResolution: number | null;
  /** The denominator `egressResolution` was computed over, verbatim. Present even
   *  when the rate is `null`, where `0` *is* the finding. */
  egressResolutionMeasured: number;
  /** The server-composed line carrying `resolvedCrossServiceEdges` AND
   *  `egressResolution` together. The structural form of BR-51: rendering this
   *  line cannot render the count without the rate. */
  resolvedEdgesSummary: string;
  /** Whether the excluded (`no-provider-in-workspace`) bucket dominates the
   *  measured denominator — the score bar renders muted rather than a
   *  confident fill when this is true, so a ratio computed over a sliver of
   *  the workspace never LOOKS like a healthy score (CR-111, frontend-design.md
   *  §4.17). `false` whenever nothing is excluded, even over a zero
   *  denominator — muting is about the excluded bucket's WEIGHT, not about
   *  whether a bar is drawn at all (that's `specConformanceRatio === null`). */
  ratioDominatedByExcluded: boolean;
  /** One row per relation arm, in arm-name order. */
  arms: ArmCoverage[];
  /** The four counts above, split by intake population — carried VERBATIM from
   *  the server's own `by_intake`, never regrouped here (CR-120, FR-WS-05).
   *
   *  This is the field that stops the board reading two populations as one: a
   *  `contract-surface` reference is a *declared* endpoint matched to a
   *  controller, an `invocation` reference is a captured *call site*, and the
   *  reference workspace's headline `bound: 81` is 81 of the first and **0** of
   *  the second. The per-arm rows above do not answer this — `route` carries both
   *  populations, because an OpenAPI operation and an HTTP client call are the
   *  same arm (CR-120 §3.1, NFR-CC-04).
   *
   *  Its two keys are `contract_surface` and `invocation` — snake-cased, because
   *  they are struct fields server-side, while a row's own `intake` carries the
   *  kebab-case token (`contract-surface` / `invocation`). Joining a row to its
   *  population means translating one into the other, and a rename of either
   *  without the other makes that join silently match nothing. */
  byIntake: IntakeSplit;
  /** Whether any `invocation`-intake reference exists at all.
   *
   *  The distinction a view must make before it says anything about the
   *  invocation half: `invocation.bound === 0` over an estate with call sites is a
   *  finding, and over one with none it is honest absence. Two different
   *  statements from the same zero, which is exactly what NFR-CC-04 asks a
   *  surface to keep apart. */
  hasInvocationReferences: boolean;
  /** No cross-boundary reference exists at all — the honest awaiting-data state. */
  isEmpty: boolean;
  /** Whether every workspace member's contract surface contributed to the counts
   *  above (FR-WS-16). `false` means the model is a partial picture and must be
   *  labelled one — but it does not say *why* a member did not contribute (an
   *  unopenable store and a failed surface read both reduce it), so a view must
   *  not attribute a cause from this field alone. */
  coversAllMembers: boolean;
  /** Members that contributed, out of the roster — the shortfall, stated. */
  membersRead: number;
  membersTotal: number;
}

/** Every reference in one intake population — the denominator that tells an
 *  `invocation.bound === 0` finding apart from an estate with no call sites at
 *  all (NFR-CC-04).
 *
 *  Exported because the coverage view needs the same sum for its per-population
 *  **References** column, and a second copy of it there was the hand-mirrored
 *  twin this module exists to prevent: the two would print a row total and a
 *  sentence about that total, side by side in one card, from two implementations
 *  free to diverge the moment `ClassificationCounts` gains a bucket. */
export function classificationTotal(counts: ClassificationCounts): number {
  return counts.bound + counts.ambiguous + counts.unbound + counts.no_provider_in_workspace;
}

/** References in one population that are INSIDE the ratio's denominator — bound,
 *  ambiguous or unbound, i.e. everything but `no-provider-in-workspace`
 *  (ADR-53).
 *
 *  The distinction {@link classificationTotal} cannot make: a population made
 *  entirely of calls to services outside this workspace is not a *broken*
 *  binding, so "nothing here resolves" would be a fabricated finding over it. A
 *  view that reports a resolution failure must gate on this, never on the total. */
export function measuredInPopulation(counts: ClassificationCounts): number {
  return counts.bound + counts.ambiguous + counts.unbound;
}

/** Group a coverage read-model into the per-arm, per-reason dashboard model. */
export function buildCoverageDashboard(coverage: CrossServiceCoverage): CoverageDashboard {
  const byArm = new Map<string, ArmCoverage>();
  const reasonsByArm = new Map<string, Map<UnboundReason, number>>();
  const provenanceByArm = new Map<string, Map<string, number>>();

  for (const ref of coverage.references) {
    let arm = byArm.get(ref.relation);
    if (!arm) {
      arm = {
        relation: ref.relation,
        bound: 0,
        ambiguous: 0,
        unbound: 0,
        noProvider: 0,
        reasons: [],
        provenance: [],
        total: 0,
      };
      byArm.set(ref.relation, arm);
      reasonsByArm.set(ref.relation, new Map());
      provenanceByArm.set(ref.relation, new Map());
    }
    arm.total += 1;
    // The wire `bucket` is the server's own 3-state classification, read verbatim —
    // except that `no-provider-in-workspace` arrives inside the `unbound` bucket
    // while the server's `unbound` COUNTER excludes it (ADR-53). Split it back out
    // here so each column reconciles with its summary counterpart.
    if (ref.bucket === "bound") arm.bound += 1;
    else if (ref.bucket === "ambiguous") arm.ambiguous += 1;
    else if (ref.reason === NO_PROVIDER) arm.noProvider += 1;
    else arm.unbound += 1;

    if (ref.reason) {
      const reasons = reasonsByArm.get(ref.relation)!;
      reasons.set(ref.reason, (reasons.get(ref.reason) ?? 0) + 1);
    }

    // EVERY row, not only the configuration-bound ones: an arm that reads
    // nothing from configuration must say so, because "no config-bound rows
    // here" and "this build does not render provenance" look identical
    // otherwise (NFR-CC-04).
    const provenances = provenanceByArm.get(ref.relation)!;
    const label = provenanceLabel(ref);
    provenances.set(label, (provenances.get(label) ?? 0) + 1);
  }

  for (const [relation, arm] of byArm) {
    arm.reasons = [...(reasonsByArm.get(relation) ?? new Map())]
      .map(([reason, count]) => ({ reason, count }))
      // Commonest reason first; ties broken by name so the order is deterministic.
      .sort((a, b) => b.count - a.count || a.reason.localeCompare(b.reason));
    arm.provenance = [...(provenanceByArm.get(relation) ?? new Map())]
      .map(([label, count]) => ({ label, count }))
      // Commonest first; ties broken by label so the order is deterministic.
      .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  }

  return {
    // `?? null` rather than a numeric default: an absent ratio is "not measured",
    // and defaulting it to 0 or 1 is exactly the fabrication FR-WS-05 forbids.
    specConformanceRatio: coverage.spec_conformance_ratio ?? null,
    bound: coverage.bound,
    ambiguous: coverage.ambiguous,
    unbound: coverage.unbound,
    noProviderInWorkspace: coverage.no_provider_in_workspace,
    specConformanceMeasured: coverage.spec_conformance_measured,
    specConformanceSummary: coverage.spec_conformance_summary,
    // The headline, verbatim. `?? null` on the rate for the same reason as the
    // ratio above: absent is "not measured", and defaulting it to 0 or 1 is the
    // fabrication FR-WS-05 forbids — here it would claim either that every
    // captured call fails to resolve or that every one succeeds.
    resolvedCrossServiceEdges: coverage.resolved_cross_service_edges,
    egressResolution: coverage.egress_resolution ?? null,
    egressResolutionMeasured: coverage.egress_resolution_measured,
    resolvedEdgesSummary: coverage.resolved_edges_summary,
    // "Dominates" = the excluded bucket outweighs what was actually measured —
    // the pec-services shape this CR fixes (7 measured, 899 excluded). Not the
    // zero-denominator case alone: a workspace with 0 measured and 0 excluded
    // (nothing to bind at all) is `isEmpty`, not a dominated ratio.
    ratioDominatedByExcluded:
      coverage.no_provider_in_workspace > coverage.spec_conformance_measured,
    arms: [...byArm.values()].sort((a, b) => a.relation.localeCompare(b.relation)),
    // Verbatim, on the same "projects, never recomputes" discipline as
    // `specConformanceRatio` — and for a sharper reason here: the server DERIVES its four
    // headline counters from this split, so a regrouping performed in the view
    // would be a second implementation of the arithmetic the headline already
    // rests on, free to disagree with it inches away on the same screen.
    byIntake: coverage.by_intake,
    hasInvocationReferences: classificationTotal(coverage.by_intake.invocation) > 0,
    isEmpty: coverage.references.length === 0,
    // No `??` fallbacks: these three are non-optional in `CrossServiceCoverage`
    // and the SPA ships inside the same binary that serves them, so there is no
    // version skew to defend against. A fallback the type says can never fire is
    // a claim about the wire that contradicts the type — one or the other has to
    // be wrong. (`spec_conformance_ratio` and `egress_resolution` ARE optional,
    // which is why their `?? null` above is real and type-checked.)
    coversAllMembers: coverage.covers_all_members,
    membersRead: coverage.members_read,
    membersTotal: coverage.members_total,
  };
}
