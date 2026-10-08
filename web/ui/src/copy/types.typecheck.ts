/*
 * Compile-time proof that the catalogue entry type rejects an incomplete entry
 * (S-611, FR-UI-39 "Removing a part from a catalogue entry fails the type
 * check"). `tsc -b` checks this file; it is imported by nothing.
 *
 * Each `@ts-expect-error` asserts the line below it IS a type error. If the type
 * were loosened so the line compiled, the directive would be unused, and an
 * unused `@ts-expect-error` is itself a `tsc -b` error — so loosening the type
 * fails the build either way.
 */

import { noAction, type CopyEntry, type WidgetAction } from "./types.ts";

// @ts-expect-error — `what` is missing.
export const missingWhat: CopyEntry = { why: "w", action: () => noAction };

// @ts-expect-error — `why` is missing.
export const missingWhy: CopyEntry = { what: "w", action: () => noAction };

// @ts-expect-error — `action` is missing.
export const missingAction: CopyEntry = { what: "w", why: "w" };

// @ts-expect-error — an `act` action must say where.
export const actWithoutWhere: WidgetAction = { kind: "act", text: "Do it." };

// @ts-expect-error — `where` exists only on an `act` action.
export const noneWithWhere: WidgetAction = { kind: "none", where: "command" };

// @ts-expect-error — `where` is one of the four kinds of place.
export const unknownWhere: WidgetAction = { kind: "act", where: "somewhere", text: "Do it." };

// @ts-expect-error — `action` takes the entry's state type.
export const wrongState: CopyEntry<{ n: number }> = { what: "w", why: "w", action: (s: string) => (s ? noAction : noAction) };
