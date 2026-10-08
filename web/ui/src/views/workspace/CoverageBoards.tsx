/*
 * The cross-service coverage boards (S-250/S-376/S-377, CR-120, FR-WS-05;
 * frontend-design §4.17) — lifted out of `WorkspaceView.tsx` unchanged by S-428
 * so the app-level Workspace Dashboard renders the SAME boards rather than a
 * restyled second copy of them.
 *
 * That is the whole reason this module exists. A hand-mirrored twin would show
 * the same widgets from two implementations free to diverge, and the figures
 * on them are exactly the ones this product has already had to correct twice
 * (CR-100's fabricated `1.0`, CR-127's self-contradicting headline). One
 * implementation, two call sites: `WorkspaceView`'s coverage tab and the
 * Workspace Dashboard.
 *
 * Every figure here is the SERVER's, displayed and never recomputed — including
 * the two composed lines (`resolvedEdgesSummary`, `specConformanceSummary`) that
 * make BR-51's and CR-111's pairings structural rather than remembered.
 *
 * Since S-613 (CR-203) each board is a `Widget` whose words come from the shared
 * catalogue `copy/coverage.copy.ts`, and an absence is stated in the widget's
 * figure row rather than as a centred empty state.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import {
  Badge,
  CopyTextView,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  ScoreBar,
  Term,
  Widget,
  type Column,
} from "../../components/index.ts";
import type { BridgeIntake, ClassificationCounts, DegradedRollup } from "../../api/types.ts";
import {
  COVERAGE_TEXT,
  coverageByArm,
  coverageByIntake,
  resolvedEdges,
  specConformance,
  type IntakeFinding,
} from "../../copy/coverage.copy.ts";
import {
  armLabel,
  classificationTotal,
  measuredInPopulation,
  reasonLabel,
  resolvedEdgesState,
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

/** The captured-call half of the split, as one of its four findings. */
function intakeFinding(captured: IntakePopulation): IntakeFinding {
  if (captured.total === 0) return "captured-absent";
  if (captured.measured === 0) return "captured-outside";
  return captured.bound === 0 ? "captured-unresolved" : "captured-resolves";
}

/** The coverage-by-intake board (S-377, CR-120).
 *
 *  Its own component so each population is built once and named — the finding
 *  below is *about* the `invocation` row, and recovering it from `rows[1]` (or
 *  `find(...)!`) would tie a sentence to a sort order and put a non-null
 *  assertion in a render path, which this codebase's wire-type docs tell views
 *  not to do.
 *
 *  # Four findings, because `invocation.bound === 0` means three different things
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
  const finding = intakeFinding(captured);
  const title = (
    <>
      Coverage by <Term term="intake" />
    </>
  );
  if (dashboard.isEmpty) {
    return (
      <Widget
        title={title}
        copy={coverageByIntake}
        state={{ finding }}
        absence={COVERAGE_TEXT.nothingFound(dashboard.coversAllMembers, dashboard.membersRead, dashboard.membersTotal)}
      />
    );
  }
  return (
    <Widget title={title} copy={coverageByIntake} state={{ finding }} figure={<IntakeFigure finding={finding} captured={captured} />}>
      <DataTable
        caption="Cross-service coverage by intake population"
        columns={INTAKE_COLUMNS}
        rows={[declared, captured]}
        rowKey={(p) => p.intake}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Widget>
  );
}

/** The captured-call finding, in words — never left to be read off two zeros. */
function IntakeFigure({ finding, captured }: { finding: IntakeFinding; captured: IntakePopulation }) {
  return (
    <div className={styles.figure}>
      <p className={styles.statement}>{intakeStatement(finding, captured)}</p>
    </div>
  );
}

function intakeStatement(finding: IntakeFinding, captured: IntakePopulation): string {
  switch (finding) {
    case "captured-absent":
      return COVERAGE_TEXT.capturedAbsent;
    case "captured-outside":
      return COVERAGE_TEXT.capturedOutside(captured.noProvider);
    case "captured-unresolved":
      return COVERAGE_TEXT.capturedUnresolved(captured.measured);
    case "captured-resolves":
      return COVERAGE_TEXT.capturedResolves(captured.bound, captured.measured);
  }
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
 * `degraded_rollup` — the field that actually knows.
 *
 * Inline content (spans), so it can sit in the figure row's paragraph or in an
 * absence statement alike; `null` when there is nothing to say. */
export function CoverageShortfall({
  dashboard,
  degraded,
}: {
  dashboard: CoverageDashboard;
  degraded: DegradedRollup;
}) {
  if (dashboard.coversAllMembers && degraded.degraded_members.length === 0) return null;
  return (
    <>
      {!dashboard.coversAllMembers && (
        <span>{COVERAGE_TEXT.shortfall(dashboard.membersRead, dashboard.membersTotal)} </span>
      )}
      {degraded.degraded_members.length > 0 && (
        <span>
          {COVERAGE_TEXT.degradedPrefix(degraded.degraded_members.length)}{" "}
          <span className="mono">{degraded.degraded_members.join(", ")}</span>.
        </span>
      )}
    </>
  );
}

/** The server's composed edge line, verbatim, after a lead that glosses the
 *  internal word it uses. The line is the edge count AND the rate in one string
 *  (BR-51), so rendering it cannot show the count without the rate. */
function EdgeLine({ line }: { line: string }) {
  return (
    <p className={styles.note}>
      <CopyTextView text={COVERAGE_TEXT.edgeLineLead} /> <span className="mono">{line}</span>
    </p>
  );
}

/** The headline (S-376/CR-120, CR-203 §3.2 D item 4): resolved outbound call
 *  sites of those captured, drawn first because it is the figure a reader takes
 *  away. `bound_ratio` used to sit here and read 0.287 over an estate with zero
 *  caller→callee edges.
 *
 *  The figure is two server fields (the rate's own numerator and denominator),
 *  never recomputed, and beside it the edge count in the server's composed
 *  `resolvedEdgesSummary` line, so BR-51's pairing — the count never without
 *  the rate — is the server's, not a fourth place that could forget it. Below
 *  100% the action lists the not-resolved reasons across every relation arm,
 *  largest first, with their remedies. The coverage shortfall is stated once,
 *  on Spec conformance, the board computed over the walk it describes. */
function ResolvedEdgesWidget({ dashboard }: { dashboard: CoverageDashboard }) {
  const state = resolvedEdgesState(dashboard);
  const common = {
    title: "Resolved cross-service edges",
    badge: <Badge tone="muted">Advisory</Badge>,
    copy: resolvedEdges,
    state,
  } as const;
  if (dashboard.isEmpty || dashboard.egressResolution === null) {
    return (
      <Widget
        {...common}
        absence={
          dashboard.isEmpty
            ? COVERAGE_TEXT.nothingFound(dashboard.coversAllMembers, dashboard.membersRead, dashboard.membersTotal)
            : state.outside > 0
              ? COVERAGE_TEXT.outboundAllOutside(state.outside)
              : COVERAGE_TEXT.outboundNotMeasured
        }
      >
        <EdgeLine line={dashboard.resolvedEdgesSummary} />
      </Widget>
    );
  }
  return (
    <Widget
      {...common}
      figure={
        <div className={styles.figure}>
          <div className={styles.ratio}>
            <ScoreBar value={dashboard.egressResolution} max={1} tone="default" label={pct(dashboard.egressResolution)} />
            <span>{COVERAGE_TEXT.outboundResolved(state.resolved, state.measured)}</span>
          </div>
          <EdgeLine line={dashboard.resolvedEdgesSummary} />
        </div>
      }
    />
  );
}

/** Spec conformance (CR-203 §3.2 D item 10). Advisory only — never a gate input
 *  (ADR-53). The ratio is the server's, displayed verbatim:
 *  `no-provider-in-workspace` is deliberately outside its denominator, so
 *  recomputing it here would contradict the CLI.
 *
 *  An ABSENT ratio gets no bar at all (FR-WS-05, NFR-CC-04): a bar is a
 *  quantity, and there is no quantity here — a 0-width bar would read "nothing
 *  bound" and a full one "everything bound", when the truth is that nothing was
 *  measured. The reason is stated precisely: references DO exist and every one
 *  of them is bucketed out of the denominator, so "nothing to bind" would
 *  replace a fabricated number with a fabricated explanation.
 *
 *  CR-111: the ratio is never presented without its denominator and excluded
 *  count — the figure row states both from the server's fields, and the
 *  server's own composed line rides beside it in the evidence. The coverage
 *  shortfall rides in the figure row too, its one place among the boards:
 *  `covers_all_members` is about the contract-surface walk, the very
 *  population this ratio is computed over. */
function SpecConformanceWidget({
  dashboard,
  degraded,
}: {
  dashboard: CoverageDashboard;
  degraded: DegradedRollup;
}) {
  const partial = !dashboard.coversAllMembers || degraded.degraded_members.length > 0;
  const shortfall = <CoverageShortfall dashboard={dashboard} degraded={degraded} />;
  const common = {
    title: "Spec conformance (declared endpoints vs controllers)",
    badge: <Badge tone="muted">Advisory</Badge>,
    copy: specConformance,
    state: { notMatched: dashboard.ambiguous + dashboard.unbound },
  } as const;
  const evidence = <p className="muted mono">{dashboard.specConformanceSummary}</p>;
  if (dashboard.isEmpty || dashboard.specConformanceRatio === null) {
    return (
      <Widget
        {...common}
        absence={
          <>
            {dashboard.isEmpty
              ? COVERAGE_TEXT.nothingFound(dashboard.coversAllMembers, dashboard.membersRead, dashboard.membersTotal)
              : COVERAGE_TEXT.specNotMeasured(dashboard.noProviderInWorkspace)}{" "}
            {shortfall}
          </>
        }
      >
        {evidence}
      </Widget>
    );
  }
  return (
    <Widget
      {...common}
      figure={
        <div className={styles.figure}>
          <div className={styles.ratio}>
            <ScoreBar
              value={dashboard.specConformanceRatio}
              max={1}
              tone={dashboard.ratioDominatedByExcluded ? "muted" : "default"}
              label={pct(dashboard.specConformanceRatio)}
            />
            <span>
              {pct(dashboard.specConformanceRatio)} ·{" "}
              {COVERAGE_TEXT.specMatched(dashboard.bound, dashboard.specConformanceMeasured)}
            </span>
          </div>
          <p className={styles.note}>
            {COVERAGE_TEXT.specBreakdown(
              dashboard.bound,
              dashboard.ambiguous,
              dashboard.unbound,
              dashboard.noProviderInWorkspace,
            )}
          </p>
          {partial && <p className={styles.note}>{shortfall}</p>}
        </div>
      }
    >
      {evidence}
    </Widget>
  );
}

/** The coverage boards, as sibling widgets. A FRAGMENT, never a wrapper: each
 *  call site renders them inside its own one `WidgetStack`, so every widget's
 *  parent is that stack and every gap is its one token (FR-UI-40). The old
 *  `.panel` wrapper had its own smaller gap, and the coverage tab's two relation
 *  cards sat outside it with none (CR-203 §3.1 item 10). */
export function CoveragePanel({
  dashboard,
  degraded,
}: {
  dashboard: CoverageDashboard;
  degraded: DegradedRollup;
}) {
  return (
    <>
      <ResolvedEdgesWidget dashboard={dashboard} />
      <SpecConformanceWidget dashboard={dashboard} degraded={degraded} />

      {/* S-377/CR-120: the headline counts TWO populations as one. A declared
          endpoint matched to a controller and a resolved outbound call site are
          different claims, and the reference workspace's `bound: 81` is 81 of
          the first and 0 of the second — which a bare headline reads as healthy.
          `route` carries both populations, because an OpenAPI operation and an
          HTTP client call are the same arm, so the split is its own board,
          adjacent to the headline it decomposes. Counts are the server's. */}
      <IntakeCard dashboard={dashboard} />

      {/* S-612 (FR-UI-41): hidden through the register on both call sites. Its
          data stays on `GET /api/v1/workspace/status`, `logos workspace status`
          and MCP `workspace_status`. */}
      {!isWidgetHidden("coverage-by-relation-arm") && !dashboard.isEmpty && (
        <Widget
          title={
            <>
              Coverage by relation <Term term="arm">arm</Term>
            </>
          }
          copy={coverageByArm}
        >
          <DataTable
            caption="Cross-service coverage by relation arm"
            columns={ARM_COLUMNS}
            rows={dashboard.arms}
            rowKey={(a) => a.relation}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Widget>
      )}
    </>
  );
}
