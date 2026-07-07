/*
 * DataTable (S-193, FR-UI-23; frontend-design §5/§7). The accessible, semantic
 * data surface — a real <table> (never a div-grid), hairline row rules, mono for
 * identifiers/paths/numbers, sticky header, numeric headers right-aligned with
 * their cells. Sortable column headers are <button>s carrying `aria-sort`; this
 * sorts client-side over the FULL dataset. A caption that merely repeats the
 * enclosing card heading is rendered `sr-only` (still the table's accessible
 * name, §7).
 *
 * Pagination (S-188, FR-UI-11): an optional `pageSize` re-homes the legacy htmx
 * sort+paginate mechanism into the SPA. The sort is applied to the WHOLE dataset
 * first, then the current page is sliced — so a header click reorders every row,
 * not just the visible page (the htmx semantics, preserved). The pager is a pair
 * of real <button>s with an `aria-live` range announcement (WCAG 2.1 AA); with no
 * `pageSize` the table renders every row as before (back-compatible).
 */

import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";

import styles from "./DataTable.module.css";

export interface Column<Row> {
  /** Stable key for the column. */
  key: string;
  /** Header text. */
  header: ReactNode;
  /** Cell renderer. */
  cell: (row: Row) => ReactNode;
  /** Right-align the header + cells (numeric columns). */
  numeric?: boolean;
  /** Render cells in the mono face (identifiers / paths / figures). */
  mono?: boolean;
  /** When set, the column is sortable; returns the comparable value for `row`. */
  sortValue?: (row: Row) => string | number;
}

export interface DataTableProps<Row> {
  columns: Column<Row>[];
  rows: Row[];
  /** Stable key per row. */
  rowKey: (row: Row, index: number) => string;
  /** Accessible name for the table. */
  caption: string;
  /** Show the caption visually; when false it is `sr-only` (the default). */
  captionVisible?: boolean;
  /** Honest empty state when there are no rows. */
  empty?: ReactNode;
  /**
   * When set, paginate the sorted dataset at this page size (FR-UI-11). The sort
   * runs over the FULL dataset before the page is sliced. Omit to render every row.
   */
  pageSize?: number;
  className?: string;
}

type SortDir = "asc" | "desc";

export function DataTable<Row>({
  columns,
  rows,
  rowKey,
  caption,
  captionVisible = false,
  empty,
  pageSize,
  className,
}: DataTableProps<Row>) {
  const [sortKey, setSortKey] = useState<string | null>(null);
  const [sortDir, setSortDir] = useState<SortDir>("asc");
  const [page, setPage] = useState(0);

  const sorted = useMemo(() => {
    if (!sortKey) return rows;
    const col = columns.find((c) => c.key === sortKey);
    if (!col?.sortValue) return rows;
    const get = col.sortValue;
    // Copy before sort — never mutate the caller's array.
    return [...rows].sort((a, b) => {
      const va = get(a);
      const vb = get(b);
      const cmp = va < vb ? -1 : va > vb ? 1 : 0;
      return sortDir === "asc" ? cmp : -cmp;
    });
  }, [rows, columns, sortKey, sortDir]);

  // Pagination is over the SORTED full dataset (FR-UI-11). With no `pageSize` the
  // whole dataset renders (one page).
  const paginated = pageSize != null && pageSize > 0;
  const pageCount = paginated ? Math.max(1, Math.ceil(sorted.length / pageSize)) : 1;
  // Clamp the page when the dataset or sort changes (a smaller dataset, a re-sort)
  // so the current page never points past the end.
  const safePage = Math.min(page, pageCount - 1);
  useEffect(() => {
    if (page !== safePage) setPage(safePage);
  }, [page, safePage]);

  const visible = useMemo(() => {
    if (!paginated) return sorted;
    const start = safePage * (pageSize as number);
    return sorted.slice(start, start + (pageSize as number));
  }, [sorted, paginated, safePage, pageSize]);

  function onSort(col: Column<Row>) {
    if (!col.sortValue) return;
    setPage(0); // a re-sort returns to the first page (the htmx behaviour)
    if (sortKey === col.key) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(col.key);
      setSortDir("asc");
    }
  }

  if (rows.length === 0 && empty) {
    return <div className={[styles.wrap, className].filter(Boolean).join(" ")}>{empty}</div>;
  }

  // The 1-based inclusive range the current page covers, for the announcement.
  const rangeStart = paginated ? safePage * (pageSize as number) + 1 : 1;
  const rangeEnd = paginated ? rangeStart + visible.length - 1 : sorted.length;

  return (
    <div className={[styles.wrap, className].filter(Boolean).join(" ")}>
      <table className={styles.table}>
        <caption className={captionVisible ? styles.caption : "sr-only"}>{caption}</caption>
        <thead>
          <tr>
            {columns.map((col) => {
              const active = sortKey === col.key;
              const ariaSort = !col.sortValue
                ? undefined
                : active
                  ? sortDir === "asc"
                    ? "ascending"
                    : "descending"
                  : "none";
              return (
                <th
                  key={col.key}
                  scope="col"
                  className={col.numeric ? styles.num : undefined}
                  aria-sort={ariaSort}
                >
                  {col.sortValue ? (
                    <button
                      type="button"
                      className={styles.sortBtn}
                      onClick={() => onSort(col)}
                    >
                      {col.header}
                      <span aria-hidden="true" className={styles.sortGlyph}>
                        {active ? (sortDir === "asc" ? "▲" : "▼") : "↕"}
                      </span>
                    </button>
                  ) : (
                    col.header
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {visible.map((row, i) => (
            <tr key={rowKey(row, i)}>
              {columns.map((col) => (
                <td
                  key={col.key}
                  className={[col.numeric ? styles.num : "", col.mono ? styles.mono : ""]
                    .filter(Boolean)
                    .join(" ")}
                >
                  {col.cell(row)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {paginated && pageCount > 1 && (
        <nav className={styles.pager} aria-label="Table pages">
          <button
            type="button"
            className={styles.pageBtn}
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={safePage === 0}
            aria-label="Previous page"
          >
            ‹ Prev
          </button>
          <span className={styles.pageInfo} role="status" aria-live="polite">
            Showing {rangeStart}–{rangeEnd} of {sorted.length}
          </span>
          <button
            type="button"
            className={styles.pageBtn}
            onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
            disabled={safePage >= pageCount - 1}
            aria-label="Next page"
          >
            Next ›
          </button>
        </nav>
      )}
    </div>
  );
}
