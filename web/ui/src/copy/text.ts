/*
 * Pure helpers over catalogue text (S-611, FR-UI-39). No React here: the
 * catalogue test and `expectWidgetCopy` use these to read text the way the
 * `Widget` frame renders it.
 */

import { findTermsInPlainText, GLOSSARY, type GlossaryTerm } from "./glossary.ts";
import type { CopyEntry, CopyText } from "./types.ts";

/** The plain (unglossed) segments of catalogue text. */
export function plainSegments(text: CopyText): string[] {
  if (typeof text === "string") return [text];
  return text.filter((seg): seg is string => typeof seg === "string");
}

/** The text as a reader sees it: plain segments and gloss words, joined. */
export function copyTextString(text: CopyText): string {
  if (typeof text === "string") return text;
  return text.map((seg) => (typeof seg === "string" ? seg : (seg.text ?? GLOSSARY[seg.term].label))).join("");
}

/** Glossary terms used in the plain segments of `text`, i.e. without a gloss. */
export function findUnglossedTerms(text: CopyText): GlossaryTerm[] {
  const found = new Set<GlossaryTerm>();
  for (const seg of plainSegments(text)) {
    for (const term of findTermsInPlainText(seg)) found.add(term);
  }
  return [...found];
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
