/*
 * FilesView (S-188, S-616, FR-UI-11, FR-UI-21, FR-UI-44) — the Files & Risk tab
 * over `/api/v1/files`: two widgets in one stack, each saying what it shows, why
 * it matters and what to do (CR-203 items 23–24; copy in `copy/files.copy.ts`).
 *
 * "Files ranked by risk" leads with the top hotspot in its figure row (or the
 * named absence when the board is empty), then the merged per-file risk table —
 * the ranked hotspot board (the spine, so the default order is the composite
 * hotspot score) joined with the per-file temporal churn/age facts. The table is
 * the shared interactive data-table: client-side sort + pagination over the FULL
 * dataset, numeric columns right-aligned, an absent churn/age rendered `n/a` —
 * never a fabricated zero (NFR-CC-04). The `--untested` filter is a React toggle
 * that re-fetches. "Ownership dispersion" follows. Both tables abbreviate long
 * paths through `PathCell`, the full path on hover and focus (FR-UI-44). Every
 * read is GET-only (ADR-28).
 */

import { useMemo, useState } from "react";

import { AsyncResource, fetchFiles, useApiResource } from "../../api/index.ts";
import type { FileTemporal, FilesModel } from "../../api/types.ts";
import {
  abbreviatePaths,
  Button,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  FigureNote,
  PathCell,
  pathColumn,
  Widget,
  WidgetStack,
  type Column,
} from "../../components/index.ts";
import { filesAbsence, filesRankedByRisk, ownershipDispersion } from "../../copy/files.copy.ts";
import { fileRiskRows, ownershipRows, pctBp, type FileRiskRow } from "./analyticsModel.ts";
import { CoverageCellView, Na } from "./cells.tsx";

/** The risk table's columns after File (which `pathColumn` builds over the rows). */
const FILE_COLUMNS: Column<FileRiskRow>[] = [
  {
    key: "commits",
    header: "Commits",
    numeric: true,
    cell: (r) => r.commits,
    sortValue: (r) => r.commits,
  },
  {
    key: "churn",
    header: "+/−",
    numeric: true,
    cell: (r) => (r.churn ? `${r.churn.added} / ${r.churn.deleted}` : <Na />),
    // n/a uses a sentinel below every real value, so n/a rows group together (at
    // the top in ascending order) — and are never sorted as a fabricated zero.
    sortValue: (r) => (r.churn ? r.churn.added + r.churn.deleted : -1),
  },
  {
    key: "age",
    header: "Age",
    numeric: true,
    cell: (r) => (r.ageDays != null ? r.ageDays : <Na />),
    sortValue: (r) => (r.ageDays != null ? r.ageDays : -1),
  },
  {
    key: "cochange",
    header: "Co-change",
    gloss: "coChange",
    numeric: true,
    cell: (r) => r.coChange,
    sortValue: (r) => r.coChange,
  },
  {
    key: "defect",
    header: "Defect",
    gloss: "defect",
    numeric: true,
    cell: (r) => r.defect,
    sortValue: (r) => r.defect,
  },
  {
    key: "complexity",
    header: "Complexity",
    numeric: true,
    cell: (r) => r.complexity,
    sortValue: (r) => r.complexity,
  },
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

/** The ownership table's columns after File. */
const OWNERSHIP_COLUMNS: Column<FileTemporal>[] = [
  {
    key: "dispersion",
    header: "Dispersion",
    numeric: true,
    cell: (r) => pctBp(r.ownership_dispersion_bp),
    sortValue: (r) => r.ownership_dispersion_bp,
  },
  {
    key: "entropy",
    header: "Entropy",
    numeric: true,
    cell: (r) => pctBp(r.change_entropy_bp),
    sortValue: (r) => r.change_entropy_bp,
  },
];

export function FilesView() {
  const [untested, setUntested] = useState(false);
  const [productionScope, setProductionScope] = useState(false);
  const files = useApiResource<FilesModel>(
    () => fetchFiles(untested, productionScope),
    [untested, productionScope],
  );

  return (
    <AsyncResource resource={files} loadingLabel="Loading files & risk…">
      {(model) => (
        <FilesContent
          model={model}
          untested={untested}
          onToggle={setUntested}
          productionScope={productionScope}
          onToggleProductionScope={setProductionScope}
        />
      )}
    </AsyncResource>
  );
}

function FilesContent({
  model,
  untested,
  onToggle,
  productionScope,
  onToggleProductionScope,
}: {
  model: FilesModel;
  untested: boolean;
  onToggle: (v: boolean) => void;
  productionScope: boolean;
  onToggleProductionScope: (v: boolean) => void;
}) {
  const { hotspots, temporal } = model;
  const rows = useMemo(() => fileRiskRows(hotspots, temporal), [hotspots, temporal]);
  const ownership = useMemo(() => ownershipRows(temporal), [temporal]);
  // One labelling for the risk table AND its figure row, so the top file reads
  // the same in both and never takes a label another row shares.
  const riskLabels = useMemo(() => abbreviatePaths(rows.map((r) => r.path)), [rows]);
  const fileColumns = useMemo(
    () => [pathColumn(rows, (r) => r.path, riskLabels), ...FILE_COLUMNS],
    [rows, riskLabels],
  );
  const ownershipColumns = useMemo(
    () => [pathColumn(ownership, (r) => r.path), ...OWNERSHIP_COLUMNS],
    [ownership],
  );

  const toggles = (
    <p className="muted">
      {untested ? (
        <>
          <Button variant="ghost" size="sm" onClick={() => onToggle(false)}>
            Show all files
          </Button>{" "}
          · <span className="muted">untested only</span>
        </>
      ) : (
        <>
          <span className="muted">all files</span> ·{" "}
          <Button variant="ghost" size="sm" onClick={() => onToggle(true)}>
            Untested only
          </Button>
        </>
      )}
      {" · "}
      {productionScope ? (
        <>
          <span className="muted">production files only</span>{" "}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onToggleProductionScope(false)}
          >
            Show test files too
          </Button>
        </>
      ) : (
        <Button variant="ghost" size="sm" onClick={() => onToggleProductionScope(true)}>
          Production files only
        </Button>
      )}
    </p>
  );
  const filtered = hotspots.untested || hotspots.production_scope;

  const top = hotspots.files[0];
  if (!top) {
    return (
      <WidgetStack>
        <Widget
          title="Files ranked by risk"
          copy={filesRankedByRisk}
          state={{ ranked: 0, filtered, coverageMissing: hotspots.coverage_basis !== "coverage" }}
          absence={filtered ? filesAbsence.filteredOut : (hotspots.notice ?? filesAbsence.unranked)}
        >
          {/* The filters stay reachable, or a filter that empties the board is a dead end. */}
          {filtered && toggles}
        </Widget>
      </WidgetStack>
    );
  }

  // "Coverage reads n/a" because no report is ingested: the read-model then ranks
  // on its static-reachability fallback. Not "every listed cell is n/a" — the
  // untested filter over an ingested report leaves exactly such a list.
  const coverageMissing = hotspots.coverage_basis !== "coverage";

  return (
    <WidgetStack>
      <Widget
        title="Files ranked by risk"
        copy={filesRankedByRisk}
        state={{ ranked: hotspots.ranked_files, filtered, coverageMissing }}
        figure={
          <>
            <span>
              {hotspots.ranked_files} <FigureNote>files ranked</FigureNote>
            </span>
            <FigureNote>
              top: <PathCell path={top.path} label={riskLabels.get(top.path)} />, score {top.score}
            </FigureNote>
          </>
        }
      >
        {toggles}
        <DataTable
          caption="Files ranked by risk"
          columns={fileColumns}
          rows={rows}
          rowKey={(r) => r.path}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
        <p className="muted">
          Defect column: {hotspots.defect_label} (commit-hygiene, not a defect measure).
        </p>
        {hotspots.coverage_label && (
          <p className="muted">
            Coverage basis: {hotspots.coverage_basis} — {hotspots.coverage_label}.
          </p>
        )}
      </Widget>
      {ownership.length === 0 ? (
        <Widget
          title="Ownership dispersion"
          copy={ownershipDispersion}
          state={{ multiAuthor: false }}
          absence={filesAbsence.singleAuthor}
        />
      ) : (
        <Widget
          title="Ownership dispersion"
          copy={ownershipDispersion}
          state={{ multiAuthor: true }}
          figure={
            <span>
              {ownership.length} <FigureNote>of {temporal.files.length} files have more than one author</FigureNote>
            </span>
          }
        >
          <DataTable
            caption="Ownership dispersion"
            columns={ownershipColumns}
            rows={ownership}
            rowKey={(r) => r.path}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        </Widget>
      )}
    </WidgetStack>
  );
}
