/*
 * The cross-service coverage boards (S-250/S-376/S-377, CR-120, FR-WS-05;
 * frontend-design §4.17) — lifted out of `WorkspaceView.tsx` unchanged by S-428
 * so the app-level Workspace Dashboard renders the SAME boards rather than a
 * restyled second copy of them.
 *
 * That is the whole reason this module exists. A hand-mirrored twin would show
 * the same cards from two implementations free to diverge, and the figures
 * on them are exactly the ones this product has already had to correct twice
 * (CR-100's fabricated `1.0`, CR-127's self-contradicting headline). One
 * implementation, two call sites: `WorkspaceView`'s coverage tab and the
 * Workspace Dashboard.
 *
 * Every figure here is the SERVER's, displayed and never recomputed — including
 * the two composed lines (`resolvedEdgesSummary`, `specConformanceSummary`) that
 * make BR-51's and CR-111's pairings structural rather than remembered.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import {
  Badge,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  ScoreBar,
  type Column,
} from "../../components/index.ts";
import type { BridgeIntake, ClassificationCounts, DegradedRollup } from "../../api/types.ts";
import {
  armLabel,
  classificationTotal,
  measuredInPopulation,
  reasonLabel,
  type ArmCoverage,
  type CoverageDashboard,
} from "./coverageModel.ts";
import { isWidgetHidden } from "../hiddenWidgets.ts";
import styles from "./Workspace.module.css";


/** A percentage rendered from a 0–1 ratio, at one decimal — never rounded up to a
 *  flattering figure.
 *
 *  Takes a `number`, not `number | null`: an ABSENT ratio is not a percentage and
 *  must never be rendered as one (FR-WS-05, NFR-CC-04). Every caller narrows the
 *  absence itself and states the reason in ITS OWN words — {@link CoveragePanel}
 *  below, and the two S-428 views' member tables, where an unread member's figure
 *  reads "not read" rather than a percentage. Keeping a null arm here would be a
 *  second, divergent wording for all three. */
export function pct(ratio: number): string {
  return `${(ratio * 100).toFixed(1)}%`;
}

// ── Cross-service coverage (frontend-design §4.17) ───────────────────────────

/** One intake population's row on the coverage-by-intake board (S-377, CR-120). */
interface IntakePopulation {
  intake: BridgeIntake;
  /** What the population IS, not just its wire token — a reader who does not know
   *  the vocabulary cannot act on `contract-surface` alone. */
  label: string;
  bound: number;
  ambiguous: number;
  unbound: number;
  noProvider: number;
  /** Every reference in the population. */
  total: number;
  /** The references INSIDE the ratio's denominator — everything but
   *  `no-provider-in-workspace` (ADR-53). What a resolution-failure claim must be
   *  gated on: a population of nothing but calls that leave this workspace has
   *  not failed to resolve. */
  measured: number;
}

/** Project one intake population into its board row.
 *
 *  Every count is carried verbatim, and the row's `total` comes from
 *  {@link classificationTotal} rather than being summed again here — the model
 *  owns that arithmetic, and a copy of it in this file would be a twin printing a
 *  row total beside a sentence about that total from two implementations. */
function intakeRow(
  intake: BridgeIntake,
  label: string,
  counts: ClassificationCounts,
): IntakePopulation {
  return {
    intake,
    label,
    bound: counts.bound,
    ambiguous: counts.ambiguous,
    unbound: counts.unbound,
    noProvider: counts.no_provider_in_workspace,
    total: classificationTotal(counts),
    measured: measuredInPopulation(counts),
  };
}

const INTAKE_COLUMNS: Column<IntakePopulation>[] = [
  {
    key: "intake",
    header: "Intake",
    cell: (p) => (
      <>
        <span className="mono">{p.intake}</span>
        <br />
        <span className="muted">{p.label}</span>
      </>
    ),
    sortValue: (p) => p.intake,
  },
  { key: "bound", header: "Bound", numeric: true, cell: (p) => p.bound, sortValue: (p) => p.bound },
  {
    key: "ambiguous",
    header: "Ambiguous",
    numeric: true,
    cell: (p) => p.ambiguous,
    sortValue: (p) => p.ambiguous,
  },
  {
    key: "unbound",
    header: "Unbound",
    numeric: true,
    cell: (p) => p.unbound,
    sortValue: (p) => p.unbound,
  },
  {
    key: "noProvider",
    header: "No provider here",
    numeric: true,
    cell: (p) => p.noProvider,
    sortValue: (p) => p.noProvider,
  },
  {
    key: "total",
    header: "References",
    numeric: true,
    cell: (p) => p.total,
    sortValue: (p) => p.total,
  },
];

/** The coverage-by-intake board (S-377, CR-120).
 *
 *  Its own component so each population is built once and named — the narrative
 *  below is *about* the `invocation` row, and recovering it from `rows[1]` (or
 *  `find(...)!`) would tie a sentence to a sort order and put a non-null
 *  assertion in a render path, which this codebase's wire-type docs tell views
 *  not to do.
 *
 *  # Three states, because `invocation.bound === 0` means three different things
 *  A zero over no captured call sites at all is **honest absence**. A zero over
 *  call sites that every one of them left this workspace is **not a failure** —
 *  `no-provider-in-workspace` is deliberately outside the ratio's denominator
 *  (ADR-53) precisely because a call to a service we do not host is not a broken
 *  binding. Only a zero over call sites that WERE measured is a finding. Saying
 *  the wrong one of the three is the dishonesty NFR-CC-04 forbids, and the first
 *  cut of this card said "nothing resolves" over the second — which is the state
 *  the 84-member reference estate is actually in for its one HTTP client call. */
export function IntakeCard({ dashboard }: { dashboard: CoverageDashboard }) {
  const declared = intakeRow(
    "contract-surface",
    "Declared endpoint (OpenAPI operation)",
    dashboard.byIntake.contract_surface,
  );
  const captured = intakeRow(
    "invocation",
    "Captured call site (client call, publish/subscribe)",
    dashboard.byIntake.invocation,
  );
  return (
    <Card title="Coverage by intake">
      <DataTable
        caption="Cross-service coverage by intake population"
        columns={INTAKE_COLUMNS}
        rows={[declared, captured]}
        rowKey={(p) => p.intake}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
      {captured.measured > 0 && captured.bound === 0 && (
        <p className="muted">
          No captured call site in this workspace resolves: every one of the {captured.measured}{" "}
          <span className="mono">invocation</span> references that could bind here is ambiguous or
          unbound. The bound count above is entirely declared-contract matches.
        </p>
      )}
      {captured.measured === 0 && captured.noProvider > 0 && (
        <p className="muted">
          Every captured <span className="mono">invocation</span> reference in this workspace ({captured.noProvider}) points
          at a service outside it — reported apart, and not a broken binding. Nothing here failed to
          resolve.
        </p>
      )}
      {captured.total === 0 && (
        <p className="muted">
          No <span className="mono">invocation</span> references were captured in this workspace —
          honest absence, not a resolution failure. Its bound count says nothing about outbound call
          sites either way.
        </p>
      )}
    </Card>
  );
}

const ARM_COLUMNS: Column<ArmCoverage>[] = [
  {
    key: "relation",
    header: "Arm",
    cell: (a) => armLabel(a.relation),
    sortValue: (a) => a.relation,
  },
  { key: "bound", header: "Bound", numeric: true, cell: (a) => a.bound, sortValue: (a) => a.bound },
  {
    key: "ambiguous",
    header: "Ambiguous",
    numeric: true,
    cell: (a) => a.ambiguous,
    sortValue: (a) => a.ambiguous,
  },
  {
    key: "unbound",
    header: "Unbound",
    numeric: true,
    cell: (a) => a.unbound,
    sortValue: (a) => a.unbound,
  },
  {
    // Its own column, never folded into Unbound — so this row's figures sum to the
    // headline's above it (ADR-53).
    key: "noProvider",
    header: "No provider here",
    numeric: true,
    cell: (a) => a.noProvider,
    sortValue: (a) => a.noProvider,
  },
  {
    key: "reasons",
    // "Not bound", not "Unbound": ambiguity and no-provider are their own buckets,
    // so calling their reasons "unbound" would contradict the columns beside them.
    header: "Not bound because",
    cell: (a) =>
      a.reasons.length === 0 ? (
        <span className="muted">—</span>
      ) : (
        <ul className={styles.reasons}>
          {a.reasons.map((r) => (
            <li key={r.reason}>
              {reasonLabel(r.reason)} <Badge tone="muted">{r.count}</Badge>
            </li>
          ))}
        </ul>
      ),
    sortValue: (a) => a.reasons.length,
  },
  {
    key: "provenance",
    // ADR-64 makes this a requirement ON THE SURFACES: "an admitted value must
    // never be indistinguishable from an observed one". Without this column the
    // dashboard shows a resolved template with no statement of whether the
    // repository proved it or the call site wrote it.
    header: "Target read from",
    cell: (a) =>
      a.provenance.length === 0 ? (
        <span className="muted">—</span>
      ) : (
        <ul className={styles.reasons}>
          {a.provenance.map((p) => (
            <li key={p.label}>
              {p.label} <Badge tone="muted">{p.count}</Badge>
            </li>
          ))}
        </ul>
      ),
    sortValue: (a) => a.provenance.length,
  },
];

/* The coverage shortfall rider (FR-WS-16, NFR-CC-04).
 *
 * Rendered in BOTH the empty and the populated branch, deliberately. The empty
 * branch is where it matters most: in the exact CR-100 state — 63 of 72 members
 * unopened, the 9 survivors holding no cross-boundary reference — `references`
 * is empty AND `coversAllMembers` is false, and an unqualified "none found in
 * this workspace" is then a positive claim about all 72 made from 9. That would
 * simply move the fabrication from the retired `bound_ratio: 1.0` to the empty
 * state rather than removing it.
 *
 * It says what the marker knows and no more: `covers_all_members` is
 * `members_read == members_total` over the CONTRACT-surface walk, so it is also
 * false when a member opened perfectly well and its surface read failed. Naming
 * "could not be opened" here would send an operator to `ulimit -n` for a read
 * fault. Members that genuinely could not be OPENED are named separately, from
 * `degraded_rollup` — the field that actually knows. */
export function CoverageShortfall({
  dashboard,
  degraded,
}: {
  dashboard: CoverageDashboard;
  degraded: DegradedRollup;
}) {
  if (dashboard.coversAllMembers && degraded.degraded_members.length === 0) return null;
  return (
    <p className="muted">
      {!dashboard.coversAllMembers && (
        <>
          Partial: computed over {dashboard.membersRead} of {dashboard.membersTotal} workspace
          members — the rest did not contribute (their store could not be opened, or their
          contract surface could not be read), so every figure here is a lower bound (FR-WS-16).{" "}
        </>
      )}
      {degraded.degraded_members.length > 0 && (
        <>
          {degraded.degraded_members.length} member
          {degraded.degraded_members.length === 1 ? "" : "s"} could not be opened:{" "}
          <span className="mono">{degraded.degraded_members.join(", ")}</span>.
        </>
      )}
    </p>
  );
}

export function CoveragePanel({
  dashboard,
  degraded,
}: {
  dashboard: CoverageDashboard;
  degraded: DegradedRollup;
}) {
  if (dashboard.isEmpty) {
    return (
      <div className={styles.panel}>
        <EmptyState
          message={
            dashboard.coversAllMembers
              ? "No cross-boundary references found in this workspace — nothing to bind, so no coverage is reported (never a fabricated 100%)."
              : `No cross-boundary references found among the ${dashboard.membersRead} of ${dashboard.membersTotal} workspace members that could be read — this is NOT a statement about the whole workspace.`
          }
        />
        <CoverageShortfall dashboard={dashboard} degraded={degraded} />
      </div>
    );
  }

  return (
    <div className={styles.panel}>
      {/* S-376/CR-120: the HEADLINE is the resolved-edge count, and it is drawn
          first because it is the figure a reader takes away. `bound_ratio` used to
          sit here and read 0.287 over an estate with zero caller→callee edges.

          The count and the rate are rendered from the server's composed
          `resolvedEdgesSummary` line, not assembled here from two fields: BR-51
          says the count is never published without the rate beside it, and a view
          that composed them itself would be a fourth place that could forget. */}
      <Card title="Resolved cross-service edges">
        <div className={styles.ratio}>
          {dashboard.egressResolution === null ? (
            <span className="mono">
              egress resolution not measured — no outbound call site was captured in this
              workspace, so the rate has no denominator
            </span>
          ) : (
            <>
              <ScoreBar
                value={dashboard.egressResolution}
                max={1}
                tone="default"
                label={pct(dashboard.egressResolution)}
              />
              <span className="mono">{pct(dashboard.egressResolution)} of egress sites resolve</span>
            </>
          )}
        </div>
        <p className="muted mono">{dashboard.resolvedEdgesSummary}</p>
        <p className="muted">
          Edges resolved from a captured <span className="mono">invocation</span> — a caller→callee
          call, a producer→consumer publish. Not the same as <span className="mono">bound</span>{" "}
          below, which also counts declared-contract matches, and not a count of sites: one fan-out
          publish binds every cross-member subscriber and is several edges. Advisory: never a
          quality-gate input.
        </p>
      </Card>

      <Card title="Spec conformance (declared endpoints vs controllers)">
        {/* Advisory only — never a gate input (ADR-53). The ratio is the server's,
            displayed verbatim: `no-provider-in-workspace` is deliberately outside its
            denominator, so recomputing it here would contradict the CLI.

            An ABSENT ratio gets no bar at all (FR-WS-05, NFR-CC-04): a bar is a
            quantity, and there is no quantity here — a 0-width bar would read "nothing
            bound" and a full one "everything bound", when the truth is that nothing
            was measured.

            The reason is stated precisely rather than as "nothing to bind": the
            `isEmpty` branch above already took the no-references case, so reaching
            here means references DO exist and every one of them is bucketed out of
            the denominator. Saying "nothing to bind" would replace a fabricated
            number with a fabricated explanation. */}
        <div className={styles.ratio}>
          {dashboard.specConformanceRatio === null ? (
            <span className="mono">
              spec conformance not measured — all {dashboard.noProviderInWorkspace} cross-boundary
              references have no provider in this workspace, so the ratio has no denominator
            </span>
          ) : (
            <>
              <ScoreBar
                value={dashboard.specConformanceRatio}
                max={1}
                tone={dashboard.ratioDominatedByExcluded ? "muted" : "default"}
                label={pct(dashboard.specConformanceRatio)}
              />
              <span className="mono">{pct(dashboard.specConformanceRatio)} bound</span>
            </>
          )}
        </div>
        {/* CR-111: the ratio is never presented without its denominator and excluded
            count — the server's own composed line, adjacent to the bar, verbatim
            (the same "displayed, never recomputed" discipline as the ratio itself). */}
        <p className="muted mono">{dashboard.specConformanceSummary}</p>
        <CoverageShortfall dashboard={dashboard} degraded={degraded} />
        <p className="muted">
          {dashboard.bound} bound · {dashboard.ambiguous} ambiguous · {dashboard.unbound} unbound ·{" "}
          {dashboard.noProviderInWorkspace} with no provider in this workspace (reported apart, and
          excluded from the ratio — a call to a service outside this workspace is not a broken
          binding). This ratio is dominated by declared-contract matches and is not a measure of
          cross-service coupling. Advisory: this figure is never a quality-gate input.
        </p>
      </Card>

      {/* S-377/CR-120: the headline above counts TWO populations as one. A declared
          endpoint matched to a controller and a resolved outbound call site are
          different claims, and the reference workspace's `bound: 81` is 81 of the
          first and 0 of the second — which a bare headline reads as healthy.

          The per-arm board (hidden through the hidden-widget register, S-612)
          does not answer this and cannot: `route` carries both populations,
          because an OpenAPI operation and an HTTP client call are the same arm. So
          the split is its own board, adjacent to the headline it decomposes. Counts
          are the server's, displayed verbatim. */}
      <IntakeCard dashboard={dashboard} />

      {/* S-612 (FR-UI-41): hidden through the register on both call sites. Its
          data stays on `GET /api/v1/workspace/status`, `logos workspace status`
          and MCP `workspace_status`. */}
      {!isWidgetHidden("coverage-by-relation-arm") && (
        <Card title="Coverage by relation arm">
          <DataTable
            caption="Cross-service coverage by relation arm"
            columns={ARM_COLUMNS}
            rows={dashboard.arms}
            rowKey={(a) => a.relation}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Card>
      )}
    </div>
  );
}
