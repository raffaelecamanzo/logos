/*
 * FilesView (S-188, FR-UI-11, FR-UI-21) — the Files & Risk tab migrated to React
 * over `/api/v1/files`. Verdict-first: the top hotspot (or an honest `n/a` when
 * the board is empty), then the merged per-file risk table — the ranked hotspot
 * board (the spine, so the default order is the composite hotspot score) joined
 * with the per-file temporal churn/age facts. The table is the re-homed shared
 * interactive data-table: client-side sort + pagination over the FULL dataset
 * (replacing the htmx mechanism), numeric columns right-aligned, an absent
 * churn/age rendered `n/a` — never a fabricated zero (NFR-CC-04). The `--untested`
 * filter is a React toggle that re-fetches. Every read is GET-only (ADR-28).
 */

import { useState } from "react";

import { AsyncResource, fetchFiles, useApiResource } from "../../api/index.ts";
import type { FilesModel } from "../../api/types.ts";
import {
  Button,
  Callout,
  Card,
  DataTable,
  DEFAULT_TABLE_PAGE_SIZE,
  EmptyState,
  type Column,
} from "../../components/index.ts";
import {
  fileRiskRows,
  hotspotsEmpty,
  ownershipRows,
  pctBp,
  type FileRiskRow,
} from "./analyticsModel.ts";
import { CoverageCellView, Na } from "./cells.tsx";
import type { FileTemporal } from "../../api/types.ts";
import styles from "./AnalyticsView.module.css";

const FILE_COLUMNS: Column<FileRiskRow>[] = [
  { key: "path", header: "File", mono: true, cell: (r) => r.path, sortValue: (r) => r.path },
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
    numeric: true,
    cell: (r) => r.coChange,
    sortValue: (r) => r.coChange,
  },
  {
    key: "defect",
    header: "Defect",
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

const OWNERSHIP_COLUMNS: Column<FileTemporal>[] = [
  { key: "path", header: "File", mono: true, cell: (r) => r.path, sortValue: (r) => r.path },
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
  const top = hotspots.files[0];

  const verdict = top ? (
    <Callout label="HOTSPOT" tone="signal">
      <span className="mono">{top.path}</span> <span className="muted">— score {top.score}</span>
    </Callout>
  ) : (
    <Callout label="HOTSPOT" tone="muted">
      <Na />
    </Callout>
  );

  if (hotspots.files.length === 0) {
    const { message, command } = hotspotsEmpty(hotspots);
    return (
      <div className={styles.view}>
        {verdict}
        <EmptyState message={message} command={command} />
      </div>
    );
  }

  const rows = fileRiskRows(hotspots, temporal);
  const ownership = ownershipRows(temporal);

  return (
    <div className={styles.view}>
      {verdict}
      <Card title="Files ranked by risk">
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
        <DataTable
          caption="Files ranked by risk"
          columns={FILE_COLUMNS}
          rows={rows}
          rowKey={(r) => r.path}
          pageSize={DEFAULT_TABLE_PAGE_SIZE}
        />
        <p className="muted">
          Defect column: {hotspots.defect_label} (commit-hygiene, not a defect measure). Ranked{" "}
          {hotspots.ranked_files} files.
        </p>
        {hotspots.coverage_label && (
          <p className="muted">
            Coverage basis: {hotspots.coverage_basis} — {hotspots.coverage_label}.
          </p>
        )}
      </Card>
      <Card title="Ownership dispersion">
        {ownership.length === 0 ? (
          <p className="muted">
            Single-author history — ownership dispersion needs multiple committers.
          </p>
        ) : (
          <DataTable
            caption="Ownership dispersion"
            columns={OWNERSHIP_COLUMNS}
            rows={ownership}
            rowKey={(r) => r.path}
            pageSize={DEFAULT_TABLE_PAGE_SIZE}
          />
        )}
      </Card>
    </div>
  );
}
