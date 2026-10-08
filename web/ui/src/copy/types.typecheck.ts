/*
 * Compile-time proof that the catalogue entry type rejects an incomplete entry,
 * and one that brings back the removed action line (S-611, CR-206, FR-UI-39).
 * `tsc -b` checks this file; it is imported by nothing.
 *
 * Each `@ts-expect-error` asserts the line below it IS a type error. If the type
 * were loosened so the line compiled, the directive would be unused, and an
 * unused `@ts-expect-error` is itself a `tsc -b` error — so loosening the type
 * fails the build either way.
 */

import type { CopyEntry, RowAction } from "./types.ts";

// @ts-expect-error — `what` is missing.
export const missingWhat: CopyEntry = { why: "w" };

// @ts-expect-error — `why` is missing.
export const missingWhy: CopyEntry = { what: "w" };

// @ts-expect-error — the action line was removed (CR-206): an entry may not carry one.
export const withAction: CopyEntry = { what: "w", why: "w", action: () => ({ kind: "none" }) };

/** Built apart from any annotation, so the excess-property check cannot be what
 *  rejects it: the `never` field is. */
const built = { what: "w", why: "w", action: () => ({ kind: "none" as const }) };
// @ts-expect-error — an entry built elsewhere that carries an action is still refused.
export const builtWithAction: CopyEntry = built;

// @ts-expect-error — a row action must say where.
export const actWithoutWhere: RowAction = { kind: "act", text: "Do it." };

// @ts-expect-error — `where` exists only on an `act` row action.
export const noneWithWhere: RowAction = { kind: "none", where: "command" };
