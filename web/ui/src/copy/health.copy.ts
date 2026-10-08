/*
 * The Health view's copy catalogue (S-615, CR-203 items 12–20, FR-UI-43).
 *
 * Every sentence Health renders lives here: the Gate, Quality signal, ten
 * dimension and Signal trend widgets (`CopyEntry`s, held to the FR-UI-39 message
 * standard), and the figure, absence and disclosure text those widgets carry.
 * `HealthView` writes no widget sentence of its own (only the "n/a" / "not
 * applicable" badges), so a wording change edits this file alone. An absent or
 * not-current reading names the command that changes it in its own sentence,
 * where one does (CR-206): the widgets carry no action line. The dimension
 * questions are the ones `docs/howto/metrics.md` asks; the threshold keys are
 * the `[metric_thresholds]` keys `docs/howto/configuration.md` documents.
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import type { DimensionKey, DimensionOffenderState, SignalAbsence, SnapshotCurrency } from "../views/health/healthModel.ts";

import { gloss, type CopyEntry, type CopyText } from "./types.ts";

// ── Shared: an absent or not-current reading ─────────────────────────────────

/** The figure-row statement of an absent reading, one per absence. Both widgets
 *  say the same sentence, so they cannot explain one absence two ways. Each
 *  names the one command that changes it, or none when no command does
 *  (FR-EH-04, CR-206). */
export const READING_ABSENCE: Readonly<Record<SignalAbsence, string>> = {
  unindexed: "n/a — nothing indexed yet; run logos index, then logos scan.",
  unscanned: "n/a — no scan has been run for this project; run logos scan.",
  "no-production-scope": "n/a — the last scan found no production functions to score: every indexed symbol is test code.",
};

/**
 * The not-current sentence both widgets render under their figures (CR-135, S-436).
 * Each claims exactly what its condition establishes: `de-indexed` knows the graph
 * is gone; `moved-past` knows only that the graph was indexed since, never that the
 * figures changed; `indeterminate` knows nothing about currency, so it carries no
 * date. Undated rather than fabricated when nothing dates the snapshot (NFR-CC-04).
 * The two arms a command changes name it; `indeterminate` names none, because no
 * command establishes currency (CR-206). A stale verdict is never acted on as a
 * FAIL — it does not describe the graph as it stands.
 */
export function staleNote(currency: SnapshotCurrency): string {
  if (currency.cause === "indeterminate") {
    return `Describes the last recorded snapshot — ${currency.detail}, so whether these figures are current cannot be established.`;
  }
  if (currency.cause === "moved-past") {
    return `Describes the snapshot of ${currency.date} — the graph has been indexed or synced since, so these figures are not established as a current reading (an index that changed nothing would read the same way). Run logos scan to record a snapshot of the graph as it stands now.`;
  }
  const subject = currency.date === null ? "the last recorded snapshot" : `the snapshot of ${currency.date}`;
  return `Describes ${subject} — the graph is no longer indexed, so these figures are not a current reading; run logos index before reading them as current.`;
}

// ── Gate (item 12) ───────────────────────────────────────────────────────────

export const gate: CopyEntry = {
  what: [
    "Whether the quality signal holds its ",
    gloss("baseline"),
    ": the gate passes while the signal stays at or above the baseline less a small tolerance, ",
    gloss("epsilon", "ε"),
    ".",
  ],
  why: "The pre-push hook and CI run this gate, so a FAIL blocks the push and fails the build.",
};

/** The line under a current FAIL naming the lowest-scoring dimension: a fact
 *  about the reading, not advice (CR-206 CRA-02). */
export function gateLowest(dimension: string): string {
  return `Lowest-scoring dimension: ${dimension}.`;
}

/**
 * The Gate figure: the verdict, the signal against the baseline, and the pass
 * condition `baseline − ε` (BR-10). Every number arrives formatted, so this is
 * words only; "baseline" and ε are glossed here because the figure is a Gate
 * widget's first use of them. A pass the gate reached without comparing (no
 * baseline, or one it could not compare) states that instead of a floor it never
 * applied — nothing is fabricated.
 */
export function gateFigureText(f: {
  readonly verdict: "PASS" | "FAIL";
  readonly signal: string;
  readonly baseline: string | null;
  readonly floor: string | null;
  readonly epsilon: string;
  readonly informational: boolean;
}): CopyText {
  const head = [`${f.verdict} · signal ${f.signal} vs `, gloss("baseline")];
  if (f.baseline === null || f.floor === null) return [...head, " n/a; informational pass"];
  if (f.informational) return [...head, ` ${f.baseline}; not compared, informational pass`];
  return [...head, ` ${f.baseline}; passes at ≥ ${f.floor} (`, gloss("epsilon", "ε"), ` = ${f.epsilon})`];
}

/** Why a pass against a recorded baseline was not a comparison — the line under
 *  the Gate figure. The persisting `logos gate` re-baselines on its own (FR-GV-10). */
export const GATE_NOT_COMPARED =
  "The baseline was recorded under different metric thresholds or semantics, so it cannot be compared with this score; the next logos gate run saves the current score as the new baseline.";

// ── Quality signal (items 13 and 20) ─────────────────────────────────────────

export const qualitySignal: CopyEntry = {
  what: "One score from 0 to 10000 for the production code: the geometric mean of the dimensions below that apply to this codebase.",
  why: "It is the figure the gate compares, so the lowest-scoring dimension is where a change moves it most.",
};

/** The Quality signal figure (FR-QM-14): the score and what it is the mean of. */
export function signalFigure(signal: number, applicable: number): string {
  return `${signal} / 10000, geometric mean of the ${applicable} applicable ${applicable === 1 ? "dimension" : "dimensions"}`;
}

/** The scope line: what was scored and what was left out (FR-QM-08). */
export function scopeLine(scored: number, excluded: number): string {
  return `${scored} production ${scored === 1 ? "function" : "functions"} scored · ${excluded} test ${excluded === 1 ? "function" : "functions"} excluded`;
}

/** The thresholds disclosure (item 20): what the fingerprint is, and why a change
 *  to it is announced rather than silent — the persisting `gate` re-baselines on
 *  a thresholds-hash mismatch by itself (FR-GV-10, `governance::gate`). */
export const THRESHOLDS_DISCLOSURE: { readonly summary: string; readonly body: CopyText } = {
  summary: "Thresholds fingerprint",
  body: [
    "A fingerprint of the detection thresholds this snapshot was scored with: the defaults plus any [metric_thresholds] keys set in .logos/rules.toml. It changes when one of those keys changes; when it does, the next logos gate run saves the new score as the ",
    gloss("baseline"),
    " by itself — a change re-baselines the gate — and passes informationally, with a notice; until that run, this page shows an informational pass. Nothing to do unless the change was unintended.",
  ],
};

// ── The ten dimensions (items 14–19) ─────────────────────────────────────────

/** Where a dimension's units are found when the payload carries no list of them. */
export interface UnitPointer {
  /** The named absence: no list is recorded, and why or where to look. */
  readonly statement: string;
  /** A view that shows the units. */
  readonly view?: { readonly label: string; readonly href: string };
  /** A command that shows the units. */
  readonly command?: string;
}

/** A dimension's catalogue entry: the message standard, plus its figure text and,
 *  for the dimension with no offender list (Modularity), the named absence. */
export interface DimensionCopy extends CopyEntry {
  /** The raw value with its unit, as the figure row states it. */
  readonly raw: (raw: number) => CopyText;
  /** Present exactly for the dimension whose units the payload does not list:
   *  Modularity, which scores the directory layout as a whole (CR-209). */
  readonly unlisted?: UnitPointer;
}

/** A share of a population, to one decimal place. A share that is not zero never
 *  reads "0.0%", and one short of all never reads "100.0%": one offender among
 *  thousands of functions is "<0.1%", not a fabricated zero (NFR-CC-04). */
export function percent(ratio: number): string {
  if (ratio > 0 && ratio < 0.0005) return "<0.1%";
  if (ratio < 1 && ratio >= 0.9995) return ">99.9%";
  return `${(ratio * 100).toFixed(1)}%`;
}

export const modularity: DimensionCopy = {
  what: "Do directories form real modules? This scores how many dependencies stay inside the directory they start in.",
  why: "Directories that are real modules can be read, changed and owned one at a time.",
  raw: (raw) => `Q ${raw.toFixed(2)} (Newman's modularity, from −0.5 to 1)`,
  unlisted: {
    statement:
      "No list of units: Modularity scores the directory layout as a whole, so no single unit is responsible for it.",
  },
};

export const acyclicity: DimensionCopy = {
  what: "Are there dependency cycles? This counts the groups of units that depend on each other in a loop.",
  why: "A unit in a cycle cannot be changed, tested or released without the others in it.",
  raw: (raw) => `${raw} dependency ${raw === 1 ? "cycle" : "cycles"}`,
};

export const depth: DimensionCopy = {
  what: "How long are dependency chains? This measures the longest chain of units that depend on one another, counting each cycle as one unit.",
  why: "In a long chain, a change at the bottom can ripple through every layer above it.",
  raw: (raw) => `longest chain of ${raw} ${raw === 1 ? "unit" : "units"}, each cycle counted as one`,
};

export const equality: DimensionCopy = {
  what: "Is complexity evenly spread? This measures how unevenly branching logic is spread across functions.",
  why: "Complexity piled into a few functions makes those functions the riskiest to change.",
  raw: (raw) => [gloss("gini"), ` ${raw.toFixed(2)} of function complexity (0 is even)`],
};

export const redundancy: DimensionCopy = {
  what: "How much code is dead or duplicated? This is the share of production functions that nothing reaches, or that repeat another function exactly.",
  why: "Dead and duplicated code is read, maintained and tested for nothing.",
  raw: (raw) => `${percent(raw)} of production functions are dead or duplicated`,
};

export const nesting: DimensionCopy = {
  what: "How deeply is control flow nested? This is the share of production functions nested at or beyond the nesting_depth threshold.",
  why: "Deeply nested code is hard to follow and easy to break when it changes.",
  raw: (raw) => `${percent(raw)} of production functions are deeply nested`,
};

export const conciseness: DimensionCopy = {
  what: [
    "How many ",
    gloss("brainMethod", "brain methods"),
    " are there? This is the share of production functions that are long, branchy and nested all at once.",
  ],
  why: "A brain method does too much to hold in your head, so it is where changes go wrong.",
  raw: (raw) => [`${percent(raw)} of production functions are `, gloss("brainMethod", "brain methods")],
};

export const cohesion: DimensionCopy = {
  what: [
    "Do classes hang together? This is the mean of 1/",
    gloss("lcom4"),
    " over production classes: 1 when every class's methods share its state.",
  ],
  why: "A class whose methods never share state is several classes in one, and is harder to change safely.",
  raw: (raw) => ["mean 1/", gloss("lcom4"), ` ${raw.toFixed(2)} over classes`],
};

export const focus: DimensionCopy = {
  what: [
    "Are containers ",
    gloss("godContainer", "god-objects"),
    "? This is the share of classes and structs at or over the god_methods or god_span threshold.",
  ],
  why: "A god container gathers unrelated work, so every change to it risks the rest.",
  raw: (raw) => [`${percent(raw)} of classes and structs are `, gloss("godContainer", "god containers")],
};

export const uniqueness: DimensionCopy = {
  what: [
    "How much code is near-duplicated? This is the share of production functions that are ",
    gloss("nearClone", "near-clones"),
    " of another one.",
  ],
  why: "A fix to near-copied code has to be made in every copy, and one is usually missed.",
  raw: (raw) => [`${percent(raw)} of production functions are `, gloss("nearClone", "near-clones")],
};

/** Every dimension's entry by key — the view looks each widget's copy up here. */
export const DIMENSION_COPY: Readonly<Record<DimensionKey, DimensionCopy>> = {
  modularity,
  acyclicity,
  depth,
  equality,
  redundancy,
  nesting,
  conciseness,
  cohesion,
  focus,
  uniqueness,
};

/** The line under a dimension's figure when the snapshot is not established as
 *  current: the Gate states why, once; each dimension points to it. */
export const DIMENSION_NOT_CURRENT =
  "From the same snapshot as the Gate, which is not established as a current reading — see the Gate for why.";

/** The figure-row statement of a dimension that dropped out with no value (ADR-21). */
export const NO_APPLICABLE_CONSTRUCT = "n/a — no applicable construct in this codebase";

/** The evidence statement for each offender state that has no table. */
export const OFFENDER_STATEMENT = {
  "none-flagged": "No offenders flagged within thresholds.",
  "not-recorded": "Offenders were not recorded for this snapshot; run logos scan to record them.",
} as const;

/** The status badge for each offender state. */
export function offenderBadge(state: DimensionOffenderState, listed: number): string | null {
  switch (state) {
    case "listed":
      return `${listed} flagged`;
    case "none-flagged":
      return "none flagged";
    case "not-recorded":
      return "not recorded";
    case "not-applicable":
      return "n/a";
    case "unlisted":
      return null;
  }
}

// ── Signal trend ─────────────────────────────────────────────────────────────

export const signalTrend: CopyEntry = {
  what: "The quality signal at each recorded snapshot, oldest first, with its change from the one before.",
  why: "A drop between two snapshots points at the change that caused it.",
};

export const NO_SNAPSHOTS = "No snapshots yet; run logos scan to record the first.";
