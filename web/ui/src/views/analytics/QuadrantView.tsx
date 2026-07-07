/*
 * QuadrantView (S-188, FR-UI-17, FR-UI-21) — the Quadrant tab migrated to React
 * over `/api/v1/quadrant`. The SPA computes the scatter client-side from the raw
 * reachability×runtime-coverage cross read-model (`scatterPoints`), so the engine
 * cross read-model is the single source — no new core query (ADR-01). Verdict-first:
 * the architecturally-weighted Q4 trust % beside the disagreement count, or the
 * honest `n/a` empty state when there is no fresh coverage (no scatter, no
 * fabricated score — NFR-CC-04). Body: the flipped 2×2 uPlot grid (best top-right)
 * and, beside it, the urgency-ordered accessible table — a WCAG 2.1 AA affordance
 * (not a no-JS twin) carrying the same data for keyboard/screen-reader users, with
 * the `n/a` runtime rule preserved. Every read is GET-only (ADR-28).
 */

import { AsyncResource, fetchQuadrant, useApiResource } from "../../api/index.ts";
import type { QuadrantModel } from "../../api/types.ts";
import { Callout, Card, DataTable, DEFAULT_TABLE_PAGE_SIZE, EmptyState, type Column } from "../../components/index.ts";
import {
  fileLine,
  fileWeights,
  pctBp,
  rankByUrgency,
  scatterPoints,
  severity,
  trustScoreBp,
  type UrgencyRow,
} from "./analyticsModel.ts";
import { Na, QuadrantBadge, RuntimeCell } from "./cells.tsx";
import { QuadrantChart } from "./QuadrantChart.tsx";
import styles from "./AnalyticsView.module.css";

/** The cell-legend rows, worst → best, mirroring the server-rendered `cell_legend`. */
const CELL_LEGEND: ReadonlyArray<{ q: string; label: string }> = [
  { q: "q1", label: "Q1 — unreachable, executed (false-green, worst)" },
  { q: "q2", label: "Q2 — reachable, unexecuted (dead/guarded edge)" },
  { q: "q3", label: "Q3 — unreachable & unexecuted (true gap)" },
  { q: "q4", label: "Q4 — reachable & executed (trust, best)" },
];

const URGENCY_COLUMNS: Column<UrgencyRow>[] = [
  {
    key: "name",
    header: "Symbol",
    mono: true,
    cell: (r) => r.symbol.name,
    sortValue: (r) => r.symbol.name,
  },
  {
    key: "location",
    header: "Location",
    mono: true,
    cell: (r) => fileLine(r.symbol.file, r.symbol.start_line) ?? <Na />,
    sortValue: (r) => fileLine(r.symbol.file, r.symbol.start_line) ?? "",
  },
  {
    key: "quadrant",
    header: "Quadrant",
    cell: (r) => <QuadrantBadge quadrant={r.symbol.quadrant} />,
    sortValue: (r) => severity(r.symbol.quadrant),
  },
  {
    key: "reachable",
    header: "Reachable",
    cell: (r) => (r.symbol.reachable_from_test ? "yes" : "no"),
    sortValue: (r) => (r.symbol.reachable_from_test ? 1 : 0),
  },
  {
    key: "runtime",
    header: "Runtime %",
    numeric: true,
    cell: (r) => (
      <RuntimeCell pct={r.symbol.runtime_exec_bp != null ? pctBp(r.symbol.runtime_exec_bp) : null} />
    ),
    sortValue: (r) => (r.symbol.runtime_exec_bp != null ? r.symbol.runtime_exec_bp : -1),
  },
  {
    key: "urgency",
    header: "Urgency",
    numeric: true,
    cell: (r) => r.urgency,
    sortValue: (r) => r.urgency,
  },
];

export function QuadrantView() {
  const quadrant = useApiResource<QuadrantModel>(() => fetchQuadrant(), []);
  return (
    <AsyncResource resource={quadrant} loadingLabel="Loading the quadrant…">
      {(model) => <QuadrantContent model={model} />}
    </AsyncResource>
  );
}

function QuadrantContent({ model }: { model: QuadrantModel }) {
  const { cross, hotspots } = model;

  // No fresh coverage → the honest empty state (no scatter, no fabricated score).
  // `has_fresh_coverage` is false for both no-ingest and all-stale.
  if (!cross.has_fresh_coverage) {
    return (
      <div className={styles.view}>
        <Callout label="Quadrant" tone="muted">
          <span>
            <Na /> — no fresh coverage to cross
          </span>
        </Callout>
        {cross.notice ? (
          <EmptyState
            message="No coverage ingested — run"
            command="logos coverage ingest <report>"
          />
        ) : (
          <EmptyState
            message="Coverage is stale (ingested at a different HEAD) — re-run"
            command="logos coverage ingest <report>"
          />
        )}
      </div>
    );
  }

  const weights = fileWeights(hotspots);
  const trust = trustScoreBp(cross.symbols, weights);
  const disagreements = cross.totals.q1 + cross.totals.q2;
  const trustPhrase = trust != null ? `Trust ${pctBp(trust)}` : "Trust n/a";

  const points = scatterPoints(cross, weights);
  const ranked = rankByUrgency(cross, weights);

  return (
    <div className={styles.view}>
      <Callout label="Quadrant" tone="signal">
        <span>
          {trustPhrase} · {disagreements} disagreement symbol(s) (Q1 + Q2) · advisory, never gated
        </span>
      </Callout>

      <Card title="Reachability × coverage">
        <p className="muted">
          X: unreachable → reachable · Y: 0% → 100% executed · best top-right (Q4 trust)
        </p>
        <QuadrantChart points={points} />
        <ul className={styles.quadrantLegend}>
          {CELL_LEGEND.map((c) => (
            <li key={c.q}>
              <span className={`${styles.qSwatch} ${styles[`q_${c.q}`]}`} aria-hidden="true" />
              {c.label}
            </li>
          ))}
        </ul>
        <p className="muted">Point size = blast radius (hotspot weight).</p>
      </Card>

      <Card title="Symbols by urgency">
        <p className="muted">
          The same data as the chart, ordered most-dangerous-first, for keyboard and screen-reader
          access. Click a column to re-sort the full set.
        </p>
        <DataTable
          caption="Symbols by urgency"
          columns={URGENCY_COLUMNS}
          rows={ranked}
          rowKey={(r: UrgencyRow) => r.symbol.symbol}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
          empty="No symbols crossed."
        />
      </Card>
    </div>
  );
}
