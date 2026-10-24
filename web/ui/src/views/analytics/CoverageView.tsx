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
 *
 * S-617 (CR-203, FR-UI-39/40): Untested hotspots and Per-file coverage are
 * `Widget`s with `copy/coverageView.copy.ts` entries, in one `WidgetStack` under
 * the status callout. With no report ingested both state that absence in their
 * figure rows and name the ingest command — no centred empty state, since the
 * view still has widgets.
 *
 * S-739 (FR-CV-15, FR-CV-13, frontend-design §4.10): the Coverage tools widget
 * leads the stack — each tool's state, Run coverage and Cancel, and the run's
 * streamed progress (`ToolStatusWidget`, `CoverageRunPanel`). The empty state
 * offers Run coverage beside the ingest command while a tool here can run, and
 * says which tools would apply while none is installed. When a run this page
 * followed ends, the page reloads, so every figure shows what it ingested, and the
 * run is shown ended only once that reload has landed (HF-6).
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import { AsyncResource, fetchCoverage, useApiResource } from "../../api/index.ts";
import type { CoverageFileStatus, CoverageModel, CoverageToolStatus, Hotspot } from "../../api/types.ts";
import {
  Badge,
  Callout,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  FigureNote,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import {
  COVERAGE_RUN_TEXT,
  COVERAGE_VIEW_ABSENCE,
  perFileCoverage,
  untestedHotspots,
} from "../../copy/coverageView.copy.ts";
import { withRankCommand } from "../../copy/files.copy.ts";
import { plural } from "../../copy/types.ts";
import { pctBp } from "./analyticsModel.ts";
import { CoverageCellView, Na } from "./cells.tsx";
import { RunButton, useCoverageRun, type CoverageRunHandle } from "./CoverageRunPanel.tsx";
import { listedTools } from "./coverageRunModel.ts";
import { ToolStatusWidget } from "./ToolStatusWidget.tsx";
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
      {(model) => <CoverageContent model={model} reload={coverage.reload} />}
    </AsyncResource>
  );
}

/** The empty state's run offer: Run coverage while a tool here can run, else
 *  the tools that would apply and are not installed (none: nothing to say), and
 *  whether the widget offers to set any of them up (S-750). */
function EmptyRunOffer({ tools, run }: { tools: readonly CoverageToolStatus[]; run: CoverageRunHandle }) {
  if (run.canRun || run.running) {
    return (
      <p>
        {COVERAGE_RUN_TEXT.emptyRun} <RunButton run={run} />
      </p>
    );
  }
  const missing = listedTools(tools).filter((t) => t.state === "not_installed");
  const offered = missing.some((t) => t.offer !== undefined);
  return missing.length > 0 ? <p>{COVERAGE_RUN_TEXT.noneInstalled(missing.map((t) => t.tool), offered)}</p> : null;
}

function CoverageContent({ model, reload }: { model: CoverageModel; reload: () => Promise<void> }) {
  const { coverage, untested } = model;
  const tools = coverage.runner?.tools ?? [];
  const run = useCoverageRun({ tools, consentedBefore: (coverage.runner?.consent.consented.length ?? 0) > 0, onFinished: reload });
  // The page's read of the status carries the runner; a read without it has no
  // tool to show, so the widget is left out rather than claiming none applies.
  const toolWidget = coverage.runner ? <ToolStatusWidget tools={tools} run={run} /> : null;

  // No coverage ingested → the read-model's own `n/a` notice; each widget states
  // the absence and names the producing command ([FR-CV-06], NFR-CC-04).
  if (coverage.notice) {
    return (
      <WidgetStack>
        <Callout label="Coverage" tone="muted">
          <Na /> — no data ingested
          {coverage.runner && <EmptyRunOffer tools={tools} run={run} />}
        </Callout>
        {toolWidget}
        <Widget
          title="Untested hotspots"
          copy={untestedHotspots}
          absence={COVERAGE_VIEW_ABSENCE.notIngested}
        />
        <Widget
          title="Per-file coverage"
          copy={perFileCoverage}
          absence={COVERAGE_VIEW_ABSENCE.notIngested}
        />
      </WidgetStack>
    );
  }

  const freshness = coverage.freshness_bp != null ? pctBp(coverage.freshness_bp) : "n/a";
  const head = coverage.head_sha ?? "n/a";
  const untestedCount = untested.files.length;

  return (
    <WidgetStack>
      <Callout label="Coverage" tone="signal">
        <span>
          {coverage.fresh_files}/{coverage.total_files} files fresh · {freshness} fresh ·{" "}
          {coverage.report_count} {plural(coverage.report_count, "report", "reports")} [
          {coverage.formats.join(", ")}] · <span className="mono">HEAD {head}</span>
        </span>
      </Callout>
      {toolWidget}

      {untested.ranked_files === 0 ? (
        // Nothing was ranked (no git history, a shallow clone…): an empty board
        // here measured nothing, so it is not "no untested file" (NFR-CC-04).
        <Widget
          title="Untested hotspots"
          copy={untestedHotspots}
          absence={untested.notice ?? withRankCommand(COVERAGE_VIEW_ABSENCE.notRanked)}
        />
      ) : untestedCount === 0 ? (
        <Widget
          title="Untested hotspots"
          copy={untestedHotspots}
          absence={COVERAGE_VIEW_ABSENCE.noUntested}
        />
      ) : (
        <Widget
          title="Untested hotspots"
          copy={untestedHotspots}
          figure={
            <span>
              {untestedCount}{" "}
              <FigureNote>
                untested {plural(untestedCount, "file", "files")} among the {untested.ranked_files} ranked
              </FigureNote>
            </span>
          }
        >
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
        </Widget>
      )}

      {coverage.files.length === 0 ? (
        <Widget
          title="Per-file coverage"
          copy={perFileCoverage}
          absence={COVERAGE_VIEW_ABSENCE.noFiles}
        />
      ) : (
        <Widget
          title="Per-file coverage"
          copy={perFileCoverage}
          figure={
            <span>
              {coverage.overall_coverage_bp != null ? pctBp(coverage.overall_coverage_bp) : "n/a"}{" "}
              <FigureNote>
                of lines covered · {coverage.stale_files} of {coverage.total_files} {plural(coverage.total_files, "file", "files")}{" "}
                stale
              </FigureNote>
            </span>
          }
        >
          <DataTable
            caption="Per-file coverage"
            columns={PERFILE_COLUMNS}
            rows={coverage.files}
            rowKey={(f) => f.path}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Widget>
      )}
    </WidgetStack>
  );
}
