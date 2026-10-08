/*
 * The web UI glossary (S-611, CR-203, FR-UI-39 "Vocabulary").
 *
 * The internal terms a reader cannot be expected to know. In catalogue text each
 * is either replaced by plain words or rendered through a `Term` gloss at its
 * first use in a widget. This module is the one list: the catalogue test and
 * `expectWidgetCopy` both apply the rule through `findUnglossedUses` (text.ts),
 * so a term added here is enforced everywhere at once.
 *
 * A vocabulary entry carries the pattern that detects the term in plain text.
 * Most are a whole-word match; where one needs more care, a comment beside it
 * says why. An entry WITHOUT a pattern is a column-header gloss (S-616): a
 * label a table defines through `Term` — "Co-change", "Defect", "Answered" —
 * that is ordinary English in prose, so the vocabulary rule does not police it.
 */

export interface GlossaryEntry {
  /** The term as it is displayed by default. */
  readonly label: string;
  /** The plain-words explanation shown on hover and focus. */
  readonly definition: string;
  /**
   * Detects the term in plain catalogue text: present exactly on the FR-UI-39
   * internal vocabulary, which must be glossed at its first use in a widget.
   */
  readonly pattern?: RegExp;
}

export const GLOSSARY = {
  arm: {
    label: "arm",
    definition:
      "One way a call between services is recognised — for example a REST route or a Kafka topic. Coverage is reported for each way separately.",
    // Whole word only: "alarm", "armed" and "farm" are not the term.
    pattern: /\barms?\b/i,
  },
  intake: {
    label: "intake",
    definition:
      "Where a cross-service reference was read from: a call site in the code, or a declared contract such as an API document.",
    pattern: /\bintakes?\b/i,
  },
  egress: {
    label: "egress",
    definition: "Outbound: calls that leave this service for another one.",
    pattern: /\begress\b/i,
  },
  residue: {
    label: "residue",
    definition: "What is still unresolved after every matching step has run.",
    pattern: /\bresidues?\b/i,
  },
  tier: {
    label: "tier",
    definition:
      "A group of checks treated the same way — for example the measures the quality gate enforces, as opposed to those it only reports.",
    pattern: /\btiers?\b/i,
  },
  unionView: {
    label: "union view",
    definition:
      "The answer computed over every workspace member together, rather than over each repository alone.",
    pattern: /\bunion[\s-]+views?\b/i,
  },
  promoted: {
    label: "promoted",
    definition:
      "Raised from a fact about one repository to a fact about the workspace, once the link to another member is confirmed.",
    pattern: /\bpromoted\b/i,
  },
  fanOut: {
    label: "fan-out",
    definition: "How many other units a unit calls or depends on.",
    // "fan-out", "fan out" and "fanout" are all the term.
    pattern: /\bfan[\s-]?outs?\b/i,
  },
  scc: {
    label: "SCC",
    definition:
      "Strongly connected component: a group of modules that all depend on each other, directly or indirectly — a dependency cycle.",
    // Case-sensitive: the acronym, not a substring of an ordinary word.
    pattern: /\bSCCs?\b/,
  },
  lcom4: {
    label: "LCOM4",
    definition:
      "Lack of cohesion of methods: how many unrelated groups of methods one type contains. 1 means the type is cohesive.",
    pattern: /\bLCOM4\b/i,
  },
  gini: {
    label: "Gini",
    definition:
      "How unevenly something is spread across files or people, from 0 (evenly) to 1 (all in one place).",
    pattern: /\bGini\b/i,
  },
  baseline: {
    label: "baseline",
    definition:
      "The recorded score the quality gate compares the current score against; it is set at release time.",
    pattern: /\bbaselines?\b/i,
  },
  epsilon: {
    label: "epsilon",
    definition:
      "The tolerance the gate allows: a drop below the baseline smaller than this does not count as a regression.",
    pattern: /\bepsilon\b/i,
  },
  bound: {
    label: "bound",
    definition:
      "As a noun: a bound call is one linked to the code it reaches; an unbound call is one that could not be linked.",
    // Noun use only (FR-UI-39 "bound/unbound used as nouns"). The adjective
    // ("unbound calls") and the verb ("bound to a route") are plain English, so
    // the pattern matches the word as a counted noun ("3 unbound remain") or
    // where it ends a noun phrase: before punctuation, a slash, the end of the
    // text, or a conjunction ("12 bound and 3 unbound", "bound vs unbound").
    // Known limit: a predicate adjective at a sentence end ("the call is
    // bound.") is flagged too; write "linked" there.
    pattern: /\b\d[\d,]*\s+(?:un)?bound\b|\b(?:un)?bound\b(?=\s*(?:$|[.,;:!?)/]|(?:and|or|vs\.?|versus)\b))/i,
  },
  // Column-header glosses (S-616, CR-203 items 23 and 25): no pattern.
  coChange: {
    label: "Co-change",
    definition:
      "How many other files repeatedly changed in the same commits as this one. A file that always changes with others is coupled to them.",
  },
  defect: {
    label: "Defect",
    definition:
      "Commits to this file whose message looks like a fix. A commit-hygiene heuristic read from commit messages, not a count of real defects.",
  },
  answered: {
    label: "Answered",
    definition:
      "Calls that returned a non-empty answer, out of the calls whose answer could be classified.",
  },
} as const satisfies Record<string, GlossaryEntry>;

export type GlossaryTerm = keyof typeof GLOSSARY;

/** Every glossary key, in declaration order. */
export const GLOSSARY_TERMS = Object.keys(GLOSSARY) as GlossaryTerm[];

/** The detector of a vocabulary term; `undefined` for a column-header gloss. */
export function vocabularyPattern(term: GlossaryTerm): RegExp | undefined {
  return (GLOSSARY[term] as GlossaryEntry).pattern;
}

/** The FR-UI-39 internal vocabulary: the terms that carry a detector. */
export const VOCABULARY_TERMS = GLOSSARY_TERMS.filter((term) => vocabularyPattern(term) !== undefined);

/**
 * The vocabulary terms found in plain text, in glossary order (each at most
 * once). Callers pass only text that sits OUTSIDE a gloss.
 */
export function findTermsInPlainText(text: string): GlossaryTerm[] {
  return VOCABULARY_TERMS.filter((term) => vocabularyPattern(term)!.test(text));
}
