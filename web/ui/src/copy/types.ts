/*
 * The copy-catalogue entry type (S-611, CR-203, FR-UI-39).
 *
 * Every figure widget states four parts: what it shows, why it matters, what to
 * do, and — exactly when there is something to do — where. A view's catalogue is
 * one `*.copy.ts` module beside the view whose entries are `CopyEntry`s; the
 * `Widget` frame renders an entry for the widget's current state.
 *
 * The type is the enforcement: `what`, `why` and `action` are all required, so a
 * catalogue entry missing a part is a `tsc -b` error, and `where` exists only on
 * the `act` arm of `WidgetAction`, so "where present iff act" holds by
 * construction for every entry the type admits.
 */

import type { GlossaryTerm } from "./glossary.ts";

/**
 * A gloss: a glossary term rendered through `Term` (a `<dfn>` whose explanation
 * shows on hover and focus). `text` is the word as it reads in the sentence
 * ("arms", "SCCs"); it defaults to the glossary's own label.
 */
export interface Gloss {
  readonly term: GlossaryTerm;
  readonly text?: string;
}

/**
 * Catalogue text: a plain string, or a sentence of plain segments and glosses.
 * Internal vocabulary may appear only inside a `Gloss` — the catalogue test
 * scans the plain segments and fails on a listed term found there.
 */
export type CopyText = string | readonly (string | Gloss)[];

/** The four kinds of place a reader acts on (FR-UI-39 "Where"). */
export type WhereKind = "source code" | "documentation" | "configuration" | "command";

/** What the reader can do, as a function of the widget's state. */
export type WidgetAction =
  | {
      readonly kind: "act";
      /** Which kind of place the action happens in. */
      readonly where: WhereKind;
      /** The file, setting or command, when known (rendered in mono). */
      readonly target?: string;
      /** What to do, in one sentence. */
      readonly text: CopyText;
    }
  | { readonly kind: "none" };

/** One widget's copy. `S` is the widget's state, as the view computes it. */
export interface CopyEntry<S = void> {
  /** One sentence: what the widget shows; a share carries its denominator. */
  readonly what: CopyText;
  /** One sentence: the decision the figure supports or the failure it guards. */
  readonly why: CopyText;
  /** The action for a state: `act` with where, or `none` ("Nothing to do"). */
  readonly action: (state: S) => WidgetAction;
}

/** The fixed text a `none` action renders. One spelling, every widget. */
export const NOTHING_TO_DO = "Nothing to do — informational.";

/** Shorthand for a gloss inside catalogue text. */
export function gloss(term: GlossaryTerm, text?: string): Gloss {
  return text === undefined ? { term } : { term, text };
}

/** The word for a count: `one` when `n` is 1, `many` otherwise. One helper for
 *  every catalogue, so agreement is spelled one way. */
export function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}

/** Shorthand for an action with nothing to do. */
export const noAction: WidgetAction = { kind: "none" };
