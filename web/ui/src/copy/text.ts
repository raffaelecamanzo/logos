/*
 * Pure helpers over catalogue text (S-611, FR-UI-39). No React here: the
 * catalogue test and `expectWidgetCopy` use these to read text the way the
 * `Widget` frame renders it.
 */

import { GLOSSARY, VOCABULARY_TERMS, vocabularyPattern, type GlossaryTerm } from "./glossary.ts";
import type { CopyEntry, CopyText } from "./types.ts";

/** The text as a reader sees it: plain segments and gloss words, joined. */
export function copyTextString(text: CopyText): string {
  if (typeof text === "string") return text;
  return text.map((seg) => (typeof seg === "string" ? seg : (seg.text ?? GLOSSARY[seg.term].label))).join("");
}

/**
 * One part of a widget's text as the vocabulary rule reads it: the plain text,
 * with every gloss cut out, and where each gloss stood in it. The DOM side
 * (`expectWidgetCopy`, which removes each `<dfn>`) and the catalogue side
 * (`plainPart`) build the SAME string for the same copy, so the two checks
 * cannot disagree at a gloss boundary.
 */
export interface PlainPart {
  readonly text: string;
  readonly glosses: readonly { readonly term: GlossaryTerm; readonly at: number }[];
}

/** Catalogue text as a `PlainPart`. */
export function plainPart(text: CopyText): PlainPart {
  if (typeof text === "string") return { text, glosses: [] };
  let joined = "";
  const glosses: { term: GlossaryTerm; at: number }[] = [];
  for (const seg of text) {
    if (typeof seg === "string") joined += seg;
    else glosses.push({ term: seg.term, at: joined.length });
  }
  return { text: joined, glosses };
}

/** An internal term used without a gloss, and the index of the part it is in. */
export interface UnglossedUse {
  readonly term: GlossaryTerm;
  readonly part: number;
}

/**
 * The FR-UI-39 vocabulary rule over a widget's parts, in reading order: a
 * vocabulary term must be glossed at its FIRST use in the widget. A plain use is
 * a violation unless the same term was glossed before it — in an earlier part,
 * or earlier in the same part.
 */
export function findUnglossedUses(parts: readonly PlainPart[]): UnglossedUse[] {
  const glossed = new Set<GlossaryTerm>();
  const uses: UnglossedUse[] = [];
  parts.forEach((part, index) => {
    for (const term of VOCABULARY_TERMS) {
      const at = part.text.search(vocabularyPattern(term)!);
      if (at < 0 || glossed.has(term)) continue;
      if (!part.glosses.some((g) => g.term === term && g.at <= at)) uses.push({ term, part: index });
    }
    for (const g of part.glosses) glossed.add(g.term);
  });
  return uses;
}

/** Vocabulary terms whose first use across `texts` (in order) has no gloss. */
export function findUnglossedTerms(...texts: CopyText[]): GlossaryTerm[] {
  return [...new Set(findUnglossedUses(texts.map(plainPart)).map((u) => u.term))];
}

/**
 * The text an action can return, read from its source: every string literal in
 * `action`, with `gloss(…)` calls removed first, joined into one plain part.
 * `action(state)` cannot be enumerated over its states, but the words it can
 * say are all literals in its body — so the catalogue test holds action text to
 * the vocabulary rule without each catalogue declaring sample states.
 */
export function actionLiterals(action: (...args: never[]) => unknown): PlainPart {
  // `gloss)(` too: a test transform may call it as `(0, module.gloss)("arm")`.
  const source = action.toString().replace(/\bgloss\)?\(\s*(["'`])[^"'`]*\1(?:\s*,\s*(["'`])[^"'`]*\2)?\s*\)/g, "");
  const literals = [...source.matchAll(/(["'`])((?:\\.|(?!\1)[^\\])*)\1/g)].map((m) => m[2]);
  return { text: literals.join(" "), glosses: [] };
}

/**
 * Narrows an unknown module export to a catalogue entry: an object carrying
 * `what`, `why` and an `action` function. Lets the catalogue test discover every
 * entry in every `*.copy.ts` module without a hand-kept list.
 */
export function isCopyEntry(value: unknown): value is CopyEntry<never> {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  const isText = (t: unknown) => typeof t === "string" || Array.isArray(t);
  return isText(v.what) && isText(v.why) && typeof v.action === "function";
}
