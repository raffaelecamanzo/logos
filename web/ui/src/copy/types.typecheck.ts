/*
 * Compile-time proof that the catalogue entry type rejects an incomplete entry,
 * and one that brings back the removed action line (S-611, CR-206, FR-UI-39);
 * and that the one no-explanation exception (CR-208) cannot be borrowed.
 * `tsc -b` checks this file; it is imported by nothing.
 *
 * Each `@ts-expect-error` asserts the line below it IS a type error. If the type
 * were loosened so the line compiled, the directive would be unused, and an
 * unused `@ts-expect-error` is itself a `tsc -b` error — so loosening the type
 * fails the build either way.
 */

import type { WidgetProps } from "../components/Widget.tsx";

import type { CopyEntry, NoExplanationEntry } from "./types.ts";

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

// ── CR-208: Project Overview is the one entry with no explanation part ──

/** The exception compiles for its one widget. */
export const projectOverviewException: NoExplanationEntry = { noExplanation: "project-overview" };

// @ts-expect-error — the exception is keyed to Project Overview: another widget cannot claim it.
export const anotherException: NoExplanationEntry = { noExplanation: "quality-index" };

// @ts-expect-error — the exception carries no explanation part: a `what` beside it is refused.
export const exceptionWithWhat: NoExplanationEntry = { noExplanation: "project-overview", what: "w" };

// @ts-expect-error — claiming the exception does not excuse a `CopyEntry` its `what` and `why`.
export const entryClaimingException: CopyEntry = { noExplanation: "project-overview" };

// @ts-expect-error — the exception is not a `CopyEntry`, so it cannot stand where one is required.
export const exceptionAsEntry: CopyEntry = projectOverviewException;

// @ts-expect-error — the exception renders no explanation part, so it takes no note to put in one.
export const exceptionWithNote: WidgetProps = { title: "t", copy: projectOverviewException, note: "n" };

/** An ordinary entry still takes its note. */
export const entryWithNote: WidgetProps = { title: "t", copy: { what: "w", why: "w" }, note: "n" };
