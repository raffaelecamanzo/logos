/*
 * Pure cross-service coverage model (S-250, CR-061, FR-UI-29, FR-WS-05) — the
 * dashboard's arithmetic, lifted out of the view so it is unit-testable without a
 * DOM and can never quietly drift from the server's own figures.
 *
 * It **projects**, it does not recompute. Every count comes from the read-model's
 * classified `references`; the model only groups them by arm and by reason. In
 * particular it never re-derives `bound_ratio` — that ratio deliberately excludes
 * `no-provider-in-workspace` from its denominator (a call to a service outside this
 * workspace is not a broken binding, ADR-53), and a view that recomputed it naively
 * would silently contradict the CLI and the gate-adjacent figures. The server's
 * ratio is displayed verbatim.
 *
 * Honest empties (NFR-CC-04): a workspace with no cross-boundary references at all
 * is `isEmpty` — the view then says "no cross-service references yet" rather than
 * rendering a 100%-bound score bar over nothing.
 */

import type { CrossServiceCoverage, UnboundReason } from "../../api/types.ts";

/** The human label for each unbound reason (the wire tokens are kebab-case). */
export const REASON_LABEL: Record<UnboundReason, string> = {
  "no-provider-in-workspace": "No provider in this workspace",
  "path-not-composed": "Path could not be composed",
  "base-url-runtime": "Base URL resolved at runtime",
  ambiguous: "Two or more providers (ambiguous)",
  "schema-mismatch": "Consumer / provider schema mismatch",
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
  /** Every reference on this arm — the denominator the row's counts sum to. */
  total: number;
}

/** The dashboard's model: the verbatim server summary plus the per-arm breakdown. */
export interface CoverageDashboard {
  /** `bound / (bound + ambiguous + unbound)` as the server computed it — displayed,
   *  never recomputed (see the module docs). */
  boundRatio: number;
  bound: number;
  ambiguous: number;
  unbound: number;
  /** References with no provider anywhere in this workspace — reported beside the
   *  ratio, deliberately outside its denominator (ADR-53). */
  noProviderInWorkspace: number;
  /** One row per relation arm, in arm-name order. */
  arms: ArmCoverage[];
  /** No cross-boundary reference exists at all — the honest awaiting-data state. */
  isEmpty: boolean;
}

/** Group a coverage read-model into the per-arm, per-reason dashboard model. */
export function buildCoverageDashboard(coverage: CrossServiceCoverage): CoverageDashboard {
  const byArm = new Map<string, ArmCoverage>();
  const reasonsByArm = new Map<string, Map<UnboundReason, number>>();

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
        total: 0,
      };
      byArm.set(ref.relation, arm);
      reasonsByArm.set(ref.relation, new Map());
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
  }

  for (const [relation, arm] of byArm) {
    arm.reasons = [...(reasonsByArm.get(relation) ?? new Map())]
      .map(([reason, count]) => ({ reason, count }))
      // Commonest reason first; ties broken by name so the order is deterministic.
      .sort((a, b) => b.count - a.count || a.reason.localeCompare(b.reason));
  }

  return {
    boundRatio: coverage.bound_ratio,
    bound: coverage.bound,
    ambiguous: coverage.ambiguous,
    unbound: coverage.unbound,
    noProviderInWorkspace: coverage.no_provider_in_workspace,
    arms: [...byArm.values()].sort((a, b) => a.relation.localeCompare(b.relation)),
    isEmpty: coverage.references.length === 0,
  };
}
