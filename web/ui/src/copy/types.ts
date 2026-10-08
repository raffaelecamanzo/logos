/*
 * The copy-catalogue entry type (S-611, CR-203, CR-206, FR-UI-39).
 *
 * Every figure widget states two parts: what it shows and why it matters. A
 * view's catalogue is one `*.copy.ts` module beside the view whose entries are
 * `CopyEntry`s; the `Widget` frame renders an entry's parts. The action line
 * ("What you can do", with its where chip) was removed end to end by CR-206: an
 * absent state names the command that fixes it in its absence sentence instead.
 *
 * The type is the enforcement: `what` and `why` are required, so a catalogue
 * entry missing either is a `tsc -b` error, and `action` is typed `never`, so an
 * entry that brings one back is a `tsc -b` error too.
 *
 * A table row's own action (`ActionCell`: the Members and Binding evidence
 * columns) is row content, not the widget's action line, and keeps its own
 * type, `RowAction`, below.
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

/** One widget's copy. */
export interface CopyEntry {
  /** One sentence: what the widget shows; a share carries its denominator. */
  readonly what: CopyText;
  /** One sentence: the decision the figure supports or the failure it guards. */
  readonly why: CopyText;
  /** Removed by CR-206: an entry carrying an action line is a type error. */
  readonly action?: never;
}

// ── A table row's own action (ActionCell) ────────────────────────────────────

/** The four kinds of place a reader acts on, as a row action names them. */
export type WhereKind = "source code" | "documentation" | "configuration" | "command";

/** What a reader can do about one table row (the Members and Binding evidence
 *  columns, rendered by `ActionCell`). */
export type RowAction =
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

/** The fixed text a row's `none` action renders by default. */
export const NOTHING_TO_DO = "Nothing to do — informational.";

/** A row with nothing to do. */
export const noRowAction: RowAction = { kind: "none" };

/** Shorthand for a gloss inside catalogue text. */
export function gloss(term: GlossaryTerm, text?: string): Gloss {
  return text === undefined ? { term } : { term, text };
}

/** The word for a count: `one` when `n` is 1, `many` otherwise. One helper for
 *  every catalogue, so agreement is spelled one way. */
export function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}
