/*
 * Term (S-611, CR-203, FR-UI-39 "Vocabulary"). Renders one glossary term as a
 * `<dfn>` whose plain-words explanation shows on hover AND on keyboard focus —
 * a `title` attribute alone would serve the mouse only. The explanation is the
 * glossary's, so one wording serves every widget.
 *
 * The tip is always in the DOM (shown by CSS, never by inline style — self-only
 * CSP) and is the `<dfn>`'s accessible description, so a screen reader hears
 * the term and then its meaning.
 */

import { useId, type ReactNode } from "react";

import { GLOSSARY, type GlossaryTerm } from "../copy/glossary.ts";

import styles from "./Term.module.css";

export interface TermProps {
  /** The glossary key. */
  term: GlossaryTerm;
  /** The word as it reads in the sentence; defaults to the glossary label. */
  children?: ReactNode;
}

export function Term({ term, children }: TermProps) {
  const tipId = useId();
  const entry = GLOSSARY[term];
  return (
    <dfn className={styles.term} tabIndex={0} aria-describedby={tipId} data-term={term}>
      {children ?? entry.label}
      <span id={tipId} role="tooltip" className={styles.tip}>
        {entry.definition}
      </span>
    </dfn>
  );
}
