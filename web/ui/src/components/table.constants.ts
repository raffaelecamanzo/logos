/*
 * Shared table pagination constant (S-195, CR-051, FR-UI-11). The single
 * page-size default every unbounded `DataTable` paginates at — consolidating the
 * three duplicated `25` constants (Health's `TREND_PAGE_SIZE`, the graph table's
 * `TABLE_PAGE_SIZE`, the analytics `PAGE`) into one source of truth. Dropped
 * 25 → 20 by CR-051 so no table renders more than 20 rows per page. Consumed by
 * the Iteration-2 graph-traversal stories (S-197 query table, S-198 1-hop table)
 * through the same `components/index.ts` barrel.
 */

/** The default number of rows per page for every paginated table (FR-UI-11). */
export const DEFAULT_TABLE_PAGE_SIZE = 20;
