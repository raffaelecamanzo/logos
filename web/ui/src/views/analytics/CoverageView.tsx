/*
 * CoverageView (S-188, FR-UI-11, FR-UI-21) — the Coverage tab migrated to React
 * over `/api/v1/coverage`. Verdict-first: the coverage status line, or the honest
 * empty state naming the ingest command when no coverage exists ([FR-CV-06]). Body:
 * the untested-hotspots table (the join with the hotspot board, restricted to files
 * with no fresh positive coverage) over the re-homed sort+paginate data-table, and
 * the per-file coverage bars — a fresh value renders a native <meter> + percent, a
 * stale file a STALE label (never a shifted number), a never-covered file `n/a`
 * ([FR-CV-05]). The <meter> drives its fill from its `value` attribute, so no inline
 * style is needed — the self-only CSP stays intact. Every read is GET-only (ADR-28).
 */

import { AsyncResource, fetchCoverage, useApiResource } from "../../api/index.ts";
import type { CoverageFileStatus, CoverageModel, Hotspot } from "../../api/types.ts";
import {
  Badge,
  Callout,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  type Column,
} from "../../components/index.ts";
import { pctBp } from "./analyticsModel.ts";
import { CoverageCellView, Na } from "./cells.tsx";
import styles from "./AnalyticsView.module.css";

const UNTESTED_COLUMNS: Column<Hotspot>[] = [
  { key: "path", header: "File", mono: true, cell: (r) => r.path, sortValue: (r) => r.path },
  { key: "score", header: "Score", numeric: true, cell: (r) => r.score, sortValue: (r) => r.score },
  {
    key: "coverage",
    header: "Coverage",
    cell: (r) => (
      <CoverageCellView
        cell={r.coverage}
        pct={r.coverage.coverage_bp != null ? pctBp(r.coverage.coverage_bp) : null}
      />
    ),
    sortValue: (r) => (r.coverage.coverage_bp != null ? r.coverage.coverage_bp : -1),
  },
];

const PERFILE_COLUMNS: Column<CoverageFileStatus>[] = [
  { key: "path", header: "File", mono: true, cell: (f) => f.path, sortValue: (f) => f.path },
  {
    key: "coverage",
    header: "Coverage",
    // A fresh value renders a native <meter> + percent, a stale file a STALE label
    // (never a shifted number), a never-covered file `n/a` ([FR-CV-05]).
    cell: (f) =>
      f.freshness === "fresh" && f.coverage_bp != null ? (
        <span className={styles.covFigure}>
          <meter
            className={styles.covBar}
            min={0}
            max={10000}
            value={f.coverage_bp}
            aria-label={`Line coverage for ${f.path}`}
          >
            {pctBp(f.coverage_bp)}
          </meter>
          <span className="mono">{pctBp(f.coverage_bp)}</span>
        </span>
      ) : f.freshness === "fresh" ? (
        <Na />
      ) : (
        <Badge tone="red">STALE</Badge>
      ),
    sortValue: (f) => (f.coverage_bp != null ? f.coverage_bp : -1),
  },
];

export function CoverageView() {
  const coverage = useApiResource<CoverageModel>(() => fetchCoverage(), []);
  return (
    <AsyncResource resource={coverage} loadingLabel="Loading coverage…">
      {(model) => <CoverageContent model={model} />}
    </AsyncResource>
  );
}

function CoverageContent({ model }: { model: CoverageModel }) {
  const { coverage, untested } = model;

  // No coverage ingested → the read-model's own `n/a` notice, surfaced as the
  // empty state naming the producing command ([FR-CV-06], NFR-CC-04).
  if (coverage.notice) {
    return (
      <div className={styles.view}>
        <Callout label="Coverage" tone="muted">
          <Na /> — no data ingested
        </Callout>
        <EmptyState message="No coverage ingested — run" command="logos coverage ingest <report>" />
      </div>
    );
  }

  const freshness = coverage.freshness_bp != null ? pctBp(coverage.freshness_bp) : "n/a";
  const head = coverage.head_sha ?? "n/a";

  return (
    <div className={styles.view}>
      <Callout label="Coverage" tone="signal">
        <span>
          {coverage.fresh_files}/{coverage.total_files} files fresh · {freshness} fresh ·{" "}
          {coverage.report_count} report(s) [{coverage.formats.join(", ")}] ·{" "}
          <span className="mono">HEAD {head}</span>
        </span>
      </Callout>

      <Card title="Untested hotspots">
        {untested.files.length === 0 ? (
          <p className="muted">
            <Na /> untested hotspots
          </p>
        ) : (
          <>
            <DataTable
              caption="Untested hotspots"
              columns={UNTESTED_COLUMNS}
              rows={untested.files}
              rowKey={(r) => r.path}
              pageSize={DEFAULT_TABLE_PAGE_SIZE}
            />
            {untested.coverage_label && (
              <p className="muted">
                Basis: {untested.coverage_basis} — {untested.coverage_label}.
              </p>
            )}
          </>
        )}
      </Card>

      <Card title="Per-file coverage">
        {coverage.files.length === 0 ? (
          <p className="muted">
            <Na /> — no covered files
          </p>
        ) : (
          <DataTable
            caption="Per-file coverage"
            columns={PERFILE_COLUMNS}
            rows={coverage.files}
            rowKey={(f) => f.path}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        )}
      </Card>
    </div>
  );
}
