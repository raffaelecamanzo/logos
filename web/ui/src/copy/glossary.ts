/*
 * The web UI glossary (S-611, CR-203, FR-UI-39 "Vocabulary").
 *
 * The internal terms a reader cannot be expected to know: the FR-UI-39 list,
 * then the terms a view's catalogue added. In catalogue text each
 * is either replaced by plain words or rendered through a `Term` gloss at its
 * first use in a widget. This module is the one list: the catalogue test and
 * `expectWidgetCopy` both apply the rule through `findUnglossedUses` (text.ts),
 * so a term added here is enforced everywhere at once.
 *
 * An entry the vocabulary rule ENFORCES carries the pattern that detects the
 * term in plain text — every FR-UI-39 internal term does, and so may any other
 * term a catalogue author decides must always be glossed. Most are a whole-word
 * match; where one needs more care, a comment beside it says why. An entry
 * WITHOUT a pattern is offered, not enforced (S-616): a label a table glosses
 * through `Term` — "Co-change", "Defect", "Answered" — that is ordinary English
 * in prose, so the rule does not police it there.
 */

export interface GlossaryEntry {
  /** The term as it is displayed by default. */
  readonly label: string;
  /** The plain-words explanation shown on hover and focus. */
  readonly definition: string;
  /**
   * Detects the term in plain catalogue text. Present on every term the
   * vocabulary rule enforces (glossed at its first use in a widget); absent on
   * a gloss that is offered but not enforced.
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
    // Not followed by a letter, so a field name a server line carries
    // ("egress_resolution") is caught too: `\b` treats `_` as part of the word.
    pattern: /\begress(?![a-z])/i,
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
  // Added by the Health catalogue (S-615): the structural dimensions' own words.
  brainMethod: {
    label: "brain method",
    definition:
      "A function that is long, has many branches and is deeply nested, all at once — too much to hold in your head.",
    pattern: /\bbrain[\s-]+methods?\b/i,
  },
  godContainer: {
    label: "god container",
    definition: "A class or struct with so many methods, or so many lines, that it gathers unrelated work.",
    // "god container" and "god-object"; the threshold keys (`god_methods`) are not the term.
    pattern: /\bgod[\s-]+(?:containers?|objects?)\b/i,
  },
  nearClone: {
    label: "near-clone",
    definition: "A function that is a near copy of another: the same structure, with different names or values.",
    pattern: /\bnear[\s-]+clones?\b/i,
  },
  // ── The Members table's column headers (S-613, CR-203 §3.2 D item 3) ──────
  // Plain words already, glossed because each names a precise figure. Each
  // pattern is the whole phrase, so the words used loosely elsewhere ("unused",
  // "another service") are not the term.
  referenceResolution: {
    label: "Reference resolution",
    definition:
      "The share of this service's references — calls, imports, type uses — that link to the code they point at. The lower it is, the more links every other figure misses.",
    pattern: /\breference resolution\b/i,
  },
  entryPointsFromOtherServices: {
    label: "Entry points added by other services",
    definition:
      "Callables in this service that another workspace service calls, so they count as reachable even when nothing inside this service calls them.",
    pattern: /\bentry points? added by other services\b/i,
  },
  unusedInOwnGraph: {
    label: "Unused in its own graph",
    definition:
      "Callables that nothing in this repository reaches from one of its entry points, judged on this repository alone.",
    pattern: /\bunused in its own graph\b/i,
  },
  usedByAnotherService: {
    label: "used by another service",
    definition:
      "Of the callables this repository never reaches on its own, the ones another workspace service calls. Keep them.",
    pattern: /\bused by another service\b/i,
  },
  unusedAcrossWorkspace: {
    label: "Unused across the workspace",
    definition:
      "Callables that nothing reaches in any service of the workspace: the candidates for deletion.",
    pattern: /\bunused across the workspace\b/i,
  },
  // ── The service map (S-614, CR-203 §3.2 D item 9) ─────────────────────────
  // Domain-driven-design jargon in the cross-context hint: enforced, so no
  // service-map sentence uses it unexplained.
  boundedContext: {
    label: "bounded context",
    definition:
      "A part of the system with its own model and vocabulary, usually owned by one team; services in different contexts should depend on each other only through agreed contracts.",
    pattern: /\bbounded[\s-]+contexts?\b/i,
  },
} as const satisfies Record<string, GlossaryEntry>;

/** The FR-UI-39 vocabulary, in the order the requirement lists it. Every other
 *  glossary entry is a later story's addition (S-613: the Members headers). */
export const FR_UI_39_TERMS = [
  "arm",
  "intake",
  "egress",
  "residue",
  "tier",
  "unionView",
  "promoted",
  "fanOut",
  "scc",
  "lcom4",
  "gini",
  "baseline",
  "epsilon",
  "bound",
] as const satisfies readonly (keyof typeof GLOSSARY)[];

export type GlossaryTerm = keyof typeof GLOSSARY;

/** Every glossary key, in declaration order. */
export const GLOSSARY_TERMS = Object.keys(GLOSSARY) as GlossaryTerm[];

/** The detector of a vocabulary term; `undefined` for a column-header gloss. */
export function vocabularyPattern(term: GlossaryTerm): RegExp | undefined {
  return (GLOSSARY[term] as GlossaryEntry).pattern;
}

/** The enforced vocabulary: the terms that carry a detector (FR-UI-39's and any other). */
export const VOCABULARY_TERMS = GLOSSARY_TERMS.filter((term) => vocabularyPattern(term) !== undefined);

/**
 * The vocabulary terms found in plain text, in glossary order (each at most
 * once). Callers pass only text that sits OUTSIDE a gloss.
 */
export function findTermsInPlainText(text: string): GlossaryTerm[] {
  return VOCABULARY_TERMS.filter((term) => vocabularyPattern(term)!.test(text));
}
