/*
 * ArchitectureView (S-189, FR-UI-06, FR-UI-21) — the Architecture / Cycles (DSM)
 * tab migrated to React over `/api/v1`. Cycles-first (frontend-design §4.5): the
 * actionable back-edge list leads, then the full module heat-grid matrix is demoted
 * to a collapsible, threshold-gated disclosure. It consumes the shared `/api/v1`
 * data-access layer (the S-186 pattern): an initial async load through
 * `useApiResource`, honest loading/empty/error states through `AsyncResource`,
 * rendering exclusively through the S-193 design system. Every read is GET-only —
 * loading the view mutates no store (ADR-28).
 */

import { fetchArchitecture } from "../../api/client.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import type { ArchitectureModel, DsmReport } from "../../api/types.ts";
import { Callout, Card, DataTable, DEFAULT_TABLE_PAGE_SIZE, EmptyState } from "../../components/index.ts";
import type { Column } from "../../components/index.ts";
import { navigate } from "../../router.tsx";
import {
  backEdges,
  cellCount,
  heatBucket,
  MATRIX_MODULE_THRESHOLD,
  offDiagonalMax,
  rowName,
  type BackEdge,
} from "./dsmModel.ts";
import styles from "./ArchitectureView.module.css";

export function ArchitectureView() {
  const model = useApiResource<ArchitectureModel>(() => fetchArchitecture(), []);
  return (
    <AsyncResource
      resource={model}
      loadingLabel="Loading the dependency structure…"
      isEmpty={(m) => m.dsm.rows.length === 0}
      empty={<EmptyState message="No modules to chart — run" command="logos index" />}
    >
      {(m) => <ArchitectureReport report={m.dsm} />}
    </AsyncResource>
  );
}

/** The cycles-first report: the verdict band, the leading cycle list, then the
 *  demoted heat-grid matrix. */
function ArchitectureReport({ report }: { report: DsmReport }) {
  const edges = backEdges(report);
  return (
    <div className={styles.view}>
      <CyclesVerdict count={edges.length} />
      <CyclesCard report={report} edges={edges} />
      <MatrixCard report={report} />
    </div>
  );
}

/** The verdict band — muted when acyclic, signal-red when back-edges exist. */
function CyclesVerdict({ count }: { count: number }) {
  if (count === 0) {
    return (
      <Callout label="CYCLES" tone="muted">
        <span>No cycles detected — dependencies respect layer order.</span>
      </Callout>
    );
  }
  return (
    <Callout label="CYCLES" tone="signal">
      <span>
        {count} cycle / layering-violation edge(s) — see the list below.
      </span>
    </Callout>
  );
}

interface CycleRow {
  from: string;
  to: string;
  count: number;
}

/** The cycle list that LEADS the page (§4.5): each back-edge as a sortable
 *  `From → To` row with its dependency count. From/To are focus links into the
 *  Graph tab. An acyclic report says so honestly (NFR-CC-04). */
function CyclesCard({ report, edges }: { report: DsmReport; edges: BackEdge[] }) {
  if (edges.length === 0) {
    return (
      <Card title="Cycles">
        <p className={styles.note}>
          No cycles detected — every dependency respects layer order. The full
          dependency matrix is available below.
        </p>
      </Card>
    );
  }
  const rows: CycleRow[] = edges.map(([i, j]) => ({
    from: rowName(report, i),
    to: rowName(report, j),
    count: cellCount(report, i, j),
  }));
  const columns: Column<CycleRow>[] = [
    {
      key: "from",
      header: "From",
      mono: true,
      sortValue: (r) => r.from,
      cell: (r) => <FocusLink name={r.from} />,
    },
    {
      key: "to",
      header: "To",
      mono: true,
      sortValue: (r) => r.to,
      cell: (r) => <FocusLink name={r.to} />,
    },
    {
      key: "count",
      header: "Count",
      numeric: true,
      mono: true,
      sortValue: (r) => r.count,
      cell: (r) => r.count,
    },
  ];
  return (
    <Card title="Cycles">
      <DataTable
        caption="Cycles"
        columns={columns}
        rows={rows}
        rowKey={(r) => `${r.from} ${r.to}`}
        pageSize={DEFAULT_TABLE_PAGE_SIZE}
      />
    </Card>
  );
}

/** A cycle participant rendered as a focus link into the Graph tab. A real module
 *  name client-navigates to `/graph?seed=<name>`; the `"?"` unresolved placeholder
 *  stays inert mono text. */
function FocusLink({ name }: { name: string }) {
  if (name === "?") return <span>?</span>;
  return (
    <button
      type="button"
      className={styles.focusLink}
      onClick={() => navigate(`/graph?seed=${encodeURIComponent(name)}`)}
    >
      {name}
    </button>
  );
}

/** The demoted full matrix in a collapsible `<details>`: at/below the module-count
 *  threshold the disclosure auto-opens; above it stays collapsed with a note
 *  pointing at the Graph view (the matrix is unreadable past ~20 modules, §4.5). */
function MatrixCard({ report }: { report: DsmReport }) {
  const n = report.rows.length;
  const open = n <= MATRIX_MODULE_THRESHOLD;
  const plural = n === 1 ? "" : "s";
  const max = offDiagonalMax(report);
  return (
    <Card title="Dependency matrix">
      <details className={styles.disclosure} open={open}>
        <summary>
          Full dependency matrix · {n} module{plural}
        </summary>
        {!open && (
          <p className={styles.note}>
            {n} modules — the matrix is unreadable at this size, so it stays
            collapsed; the cycle list above is the actionable view. Function-level
            dependency exploration is the Graph view's job.
          </p>
        )}
        <div className={styles.disclosureBody}>
          <div className={styles.scroll}>
            <HeatGrid report={report} max={max} />
          </div>
        </div>
      </details>
    </Card>
  );
}

/** The heat-grid table: numbered row/column heads in mono, cell intensity bucketed
 *  against the off-diagonal max, back-edge cells outlined + glyphed (§7). */
function HeatGrid({ report, max }: { report: DsmReport; max: number }) {
  const n = report.rows.length;
  const indices = Array.from({ length: n }, (_, k) => k);
  return (
    <table className={styles.matrix}>
      <thead>
        <tr>
          <th className={styles.corner} aria-hidden="true" />
          {report.rows.map((row, idx) => (
            <th key={idx} className={styles.colHead} title={row.name} scope="col">
              {idx + 1}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {report.rows.map((row, i) => (
          <tr key={i}>
            <th className={styles.rowHead} scope="row" title={row.name}>
              {i + 1}. {row.name}
            </th>
            {indices.map((j) => (
              <HeatCell key={j} report={report} i={i} j={j} max={max} />
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** One matrix cell. The diagonal is inert; a back-edge (i < j, count > 0) is
 *  outlined with a `↺` glyph + title; other non-zero cells get a heat bucket. */
function HeatCell({
  report,
  i,
  j,
  max,
}: {
  report: DsmReport;
  i: number;
  j: number;
  max: number;
}) {
  if (i === j) return <td className={styles.diag} aria-hidden="true" />;
  const count = cellCount(report, i, j);
  if (count === 0) return <td className={styles.heat0} />;
  const backEdge = i < j;
  const bucketClass = styles[`heat${heatBucket(count, max)}` as keyof typeof styles];
  const className = [bucketClass, backEdge ? styles.cycle : ""].filter(Boolean).join(" ");
  return (
    <td
      className={className}
      title={
        backEdge ? `back-edge: ${count} dependency(ies) against layer order` : undefined
      }
    >
      <span className={styles.num}>{count}</span>
      {backEdge && (
        <span className={styles.cycleGlyph} aria-hidden="true">
          ↺
        </span>
      )}
    </td>
  );
}
