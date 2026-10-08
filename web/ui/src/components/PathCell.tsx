/*
 * PathCell (S-616, CR-203 §3.2 E, FR-UI-44). A repository-relative path in a
 * table, abbreviated when it is longer than a character budget to its first
 * segment, an ellipsis and its last two segments (`logos-core/…/resolve/binder.rs`),
 * so a path column stops setting the width of the whole table.
 *
 * Abbreviation never hides which file a row is (NFR-CC-04):
 *  - the full path is the `title` (hover) and the cell's accessible name — the
 *    abbreviation is `aria-hidden`, the full path is read instead;
 *  - an abbreviated path is focusable, and its full path shows on focus;
 *  - sorting and row keys use the full path (`pathColumn`);
 *  - when two rows of one table would abbreviate identically, both keep more
 *    trailing segments until they differ (`abbreviatePaths`, over the WHOLE table,
 *    so rows on different pages never share a label).
 */

import type { Column } from "./DataTable.tsx";
import styles from "./PathCell.module.css";

/** Paths up to this many characters render whole. */
export const PATH_BUDGET = 40;

/** Trailing segments an abbreviation keeps before any collision. */
const TRAILING = 2;

/**
 * The label for each distinct path of one table: the path itself when it fits
 * the budget (or has no middle segment to drop), else `first/…/<trailing>`. A
 * label shared by two paths grows by one trailing segment for each of them,
 * repeatedly, until every label is distinct — at worst a path ends whole.
 */
export function abbreviatePaths(paths: Iterable<string>, budget = PATH_BUDGET): Map<string, string> {
  const unique = [...new Set(paths)];
  const keep = new Map(unique.map((p) => [p, TRAILING]));

  const label = (path: string): string => {
    if (path.length <= budget) return path;
    const segments = path.split("/");
    const trailing = keep.get(path)!;
    // first + trailing must leave at least one segment out, or it IS the path.
    if (segments.length - 1 <= trailing) return path;
    return `${segments[0]}/…/${segments.slice(-trailing).join("/")}`;
  };

  for (;;) {
    const byLabel = new Map<string, string[]>();
    for (const path of unique) {
      const l = label(path);
      byLabel.set(l, [...(byLabel.get(l) ?? []), path]);
    }
    let grew = false;
    for (const group of byLabel.values()) {
      if (group.length < 2) continue;
      for (const path of group) {
        if (label(path) === path) continue; // already whole
        keep.set(path, keep.get(path)! + 1);
        grew = true;
      }
    }
    if (!grew) break;
  }
  return new Map(unique.map((p) => [p, label(p)]));
}

export interface PathCellProps {
  /** The full repository-relative path. */
  path: string;
  /** The label `abbreviatePaths` gave it in its table; defaults to the path alone. */
  label?: string;
}

export function PathCell({ path, label = abbreviatePaths([path]).get(path)! }: PathCellProps) {
  if (label === path) {
    return (
      <span className={styles.path} title={path} data-path={path}>
        {path}
      </span>
    );
  }
  return (
    <span className={styles.path} title={path} data-path={path} tabIndex={0}>
      <span aria-hidden="true">{label}</span>
      <span className={styles.full}>{path}</span>
    </span>
  );
}

/**
 * A "File" column over `rows`: each cell a `PathCell` labelled against every
 * row of the table, sorted by the full path. Build it where the rows are known
 * (a `useMemo` over the rows), not as a module constant.
 */
export function pathColumn<R>(rows: readonly R[], getPath: (row: R) => string, header = "File"): Column<R> {
  const labels = abbreviatePaths(rows.map(getPath));
  return {
    key: "path",
    header,
    mono: true,
    cell: (r) => <PathCell path={getPath(r)} label={labels.get(getPath(r))} />,
    sortValue: getPath,
  };
}
