/*
 * The Health view's copy catalogue (S-615, CR-203 items 12–20, FR-UI-43).
 *
 * Every sentence Health renders lives here: the Gate, Quality signal, ten
 * dimension and Signal trend widgets (`CopyEntry`s, held to the FR-UI-39 message
 * standard), and the figure, absence and disclosure text those widgets carry.
 * `HealthView` computes each widget's state and writes no prose of its own, so a
 * wording change edits this file alone. The dimension questions are the ones
 * `docs/howto/metrics.md` asks; the threshold keys are the `[metric_thresholds]`
 * keys `docs/howto/configuration.md` documents.
 */

import type { DimensionKey, DimensionOffenderState, SignalAbsence, SnapshotCurrency } from "../views/health/healthModel.ts";

import { gloss, noAction, type CopyEntry, type CopyText, type WidgetAction } from "./types.ts";

// ── Shared: an absent or not-current reading ─────────────────────────────────

/** Why the Gate or Quality signal has no current figure to judge — the one
 *  classification both widgets render (FR-EH-04, CR-130, CR-135). */
export type ReadingState =
  | { readonly kind: "absent"; readonly absence: SignalAbsence }
  | { readonly kind: "stale"; readonly currency: SnapshotCurrency };

/**
 * The action for an absent or not-current reading: the one command that changes
 * what is reported, or nothing when no command does (FR-EH-04). A stale verdict
 * is never acted on as a FAIL — it does not describe the graph as it stands.
 */
function readingAction(state: ReadingState): WidgetAction {
  if (state.kind === "absent") {
    switch (state.absence) {
      case "unindexed":
        return { kind: "act", where: "command", target: "logos index", text: "Index the project, then scan it." };
      case "unscanned":
        return { kind: "act", where: "command", target: "logos scan", text: "Run a scan to record the first snapshot." };
      case "no-production-scope":
        return noAction;
    }
  }
  switch (state.currency.cause) {
    case undefined:
      return {
        kind: "act",
        where: "command",
        target: "logos index",
        text: "Re-index the project before reading these figures as current.",
      };
    case "moved-past":
      return {
        kind: "act",
        where: "command",
        target: "logos scan",
        text: "Run a scan to record a snapshot of the graph as it stands now.",
      };
    case "indeterminate":
      return noAction;
  }
}

/** The figure-row statement of an absent reading, one per absence. Both widgets
 *  say the same sentence, so they cannot explain one absence two ways. */
export const READING_ABSENCE: Readonly<Record<SignalAbsence, string>> = {
  unindexed: "n/a — nothing indexed yet.",
  unscanned: "n/a — no scan has been run for this project.",
  "no-production-scope": "n/a — the last scan found no production functions to score: every indexed symbol is test code.",
};

/**
 * The not-current sentence both widgets render under their figures (CR-135, S-436).
 * Each claims exactly what its condition establishes: `de-indexed` knows the graph
 * is gone; `moved-past` knows only that the graph was indexed since, never that the
 * figures changed; `indeterminate` knows nothing about currency, so it carries no
 * date. Undated rather than fabricated when nothing dates the snapshot (NFR-CC-04).
 */
export function staleNote(currency: SnapshotCurrency): string {
  if (currency.cause === "indeterminate") {
    return `Describes the last recorded snapshot — ${currency.detail}, so whether these figures are current cannot be established.`;
  }
  if (currency.cause === "moved-past") {
    return `Describes the snapshot of ${currency.date} — the graph has been indexed or synced since, so these figures are not established as a current reading (an index that changed nothing would read the same way).`;
  }
  const subject = currency.date === null ? "the last recorded snapshot" : `the snapshot of ${currency.date}`;
  return `Describes ${subject} — the graph is no longer indexed, so these figures are not a current reading.`;
}

// ── Gate (item 12) ───────────────────────────────────────────────────────────

/** The Gate widget's state: an absent or stale reading, or a current verdict
 *  naming the dimension to start with (`null` when none is below a full score). */
export type GateState = ReadingState | { readonly kind: "verdict"; readonly passed: boolean; readonly lowest: string | null };

export const gate: CopyEntry<GateState> = {
  what: [
    "Whether the quality signal holds its ",
    gloss("baseline"),
    ": the gate passes while the signal stays at or above the baseline less a small tolerance, ",
    gloss("epsilon", "ε"),
    ".",
  ],
  why: "The pre-push hook and CI run this gate, so a FAIL blocks the push and fails the build.",
  action: (state) => {
    if (state.kind !== "verdict") return readingAction(state);
    if (state.passed) return noAction;
    const start = state.lowest === null ? "the lowest-scoring dimension below" : `${state.lowest}, the lowest-scoring dimension below`;
    return {
      kind: "act",
      where: "source code",
      text: `Start with ${start}. If the drop is intended, save the new score as the baseline at release with logos gate --save (command).`,
    };
  },
};

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

export type QualitySignalState = ReadingState | { readonly kind: "scored"; readonly lowest: string | null };

export const qualitySignal: CopyEntry<QualitySignalState> = {
  what: "One score from 0 to 10000 for the production code: the geometric mean of the dimensions below that apply to this codebase.",
  why: "It is the figure the gate compares, so the lowest-scoring dimension is where a change moves it most.",
  action: (state) => {
    if (state.kind !== "scored") return readingAction(state);
    if (state.lowest === null) return noAction;
    return {
      kind: "act",
      where: "source code",
      text: `Start with ${state.lowest}, the lowest-scoring dimension; its widget below says what to change.`,
    };
  },
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
    "A fingerprint of the detection thresholds this snapshot was scored with: the defaults plus any [metric_thresholds] keys set in .logos/rules.toml. It changes when one of those keys changes, and a change re-baselines the gate: the next logos gate run saves the new score as the ",
    gloss("baseline"),
    " by itself and passes informationally, with a notice, and until then this page shows an informational pass. Nothing to do unless the change was unintended.",
  ],
};

// ── The ten dimensions (items 14–19) ─────────────────────────────────────────

/** A dimension widget's state: dropped out; scored from a snapshot not established
 *  as current (the Gate's own classification, CR-135 — one derivation per page);
 *  or scored with its offender state and the number of offenders listed. */
export type DimensionState =
  | { readonly kind: "not-applicable" }
  | { readonly kind: "stale"; readonly currency: SnapshotCurrency }
  | {
      readonly kind: "scored";
      readonly normalized: number;
      readonly offenders: Exclude<DimensionOffenderState, "not-applicable">;
      readonly listed: number;
    };

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
 *  for a dimension with no offender list, the pointer to its units. */
export interface DimensionCopy extends CopyEntry<DimensionState> {
  /** The raw value with its unit, as the figure row states it. */
  readonly raw: (raw: number) => CopyText;
  /** Present exactly for the five dimensions whose units the payload does not list. */
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

/** "baseline" in a dimension's tuning clause: glossed, as no earlier part of a
 *  dimension widget uses it. */
const resetsBaseline = gloss("baseline");

/** The offender-backed action: refactor the listed units (source code), naming the
 *  threshold keys that tune the detection (configuration). Not recorded → scan. */
function offenderAction(state: DimensionState, refactor: (listed: number) => string, keys: string | null): WidgetAction {
  if (state.kind === "stale") return readingAction(state);
  if (state.kind === "not-applicable") return noAction;
  if (state.offenders === "not-recorded") {
    return {
      kind: "act",
      where: "command",
      target: "logos scan",
      text: "Run a scan to record this dimension's offenders; this snapshot did not record them.",
    };
  }
  if (state.offenders !== "listed") return noAction;
  if (keys === null) return { kind: "act", where: "source code", text: `${refactor(state.listed)}.` };
  return {
    kind: "act",
    where: "source code",
    text: [
      `${refactor(state.listed)}, or raise ${keys} under [metric_thresholds] in .logos/rules.toml (configuration) if the threshold does not fit this project; that resets the gate's `,
      resetsBaseline,
      ".",
    ],
  };
}

/** The action for a dimension with no list: act through its pointer while it is
 *  below a full score; nothing to do at a full score or when it drops out. A
 *  snapshot not established as current takes the Gate's remedy instead. */
function unlistedAction(state: DimensionState, act: Extract<WidgetAction, { kind: "act" }>): WidgetAction {
  if (state.kind === "stale") return readingAction(state);
  return state.kind === "scored" && state.normalized < 1 ? act : noAction;
}

const ARCHITECTURE = { label: "the Architecture dependency matrix", href: "/architecture" } as const;

export const modularity: DimensionCopy = {
  what: "Do directories form real modules? This scores how many dependencies stay inside the directory they start in.",
  why: "Directories that are real modules can be read, changed and owned one at a time.",
  action: (state) =>
    unlistedAction(state, {
      kind: "act",
      where: "source code",
      text: "Move code so that each directory depends mostly on itself: merge directories that always change together, and split ones that reach into many others.",
    }),
  raw: (raw) => `Q ${raw.toFixed(2)} (Newman's modularity, from −0.5 to 1)`,
  unlisted: {
    statement:
      "No list of units: Modularity scores the directory layout as a whole, so no single unit is responsible for it.",
  },
};

export const acyclicity: DimensionCopy = {
  what: "Are there dependency cycles? This counts the groups of units that depend on each other in a loop.",
  why: "A unit in a cycle cannot be changed, tested or released without the others in it.",
  action: (state) =>
    unlistedAction(state, {
      kind: "act",
      where: "command",
      target: "logos dsm",
      text: "Find the cycles in the Architecture dependency matrix, or with logos dsm, and break each one by removing or inverting one dependency.",
    }),
  raw: (raw) => `${raw} dependency ${raw === 1 ? "cycle" : "cycles"}`,
  unlisted: {
    statement: "No list of the cycles is recorded with this snapshot. They are shown in",
    view: ARCHITECTURE,
    command: "logos dsm",
  },
};

export const depth: DimensionCopy = {
  what: "How long are dependency chains? This measures the longest chain of units that depend on one another, counting each cycle as one unit.",
  why: "In a long chain, a change at the bottom can ripple through every layer above it.",
  action: (state) =>
    unlistedAction(state, {
      kind: "act",
      where: "command",
      target: "logos dsm",
      text: "Find the longest chains in the Architecture dependency matrix, or with logos dsm, and shorten them by removing layers that only pass calls through.",
    }),
  raw: (raw) => `longest chain ${raw}`,
  unlisted: {
    statement: "No list of the longest chains is recorded with this snapshot. They are shown in",
    view: ARCHITECTURE,
    command: "logos dsm",
  },
};

export const equality: DimensionCopy = {
  what: "Is complexity evenly spread? This measures how unevenly branching logic is spread across functions.",
  why: "Complexity piled into a few functions makes those functions the riskiest to change.",
  action: (state) =>
    unlistedAction(state, {
      kind: "act",
      where: "command",
      target: "logos hotspots",
      text: "Find the most complex functions in the Complexity column of Files & Risk, or with logos hotspots, and split them.",
    }),
  raw: (raw) => [gloss("gini"), ` ${raw.toFixed(2)} of function complexity (0 is even)`],
  unlisted: {
    statement: "No list of the most complex functions is recorded with this snapshot. They are in the Complexity column of",
    view: { label: "Files & Risk", href: "/files" },
    command: "logos hotspots",
  },
};

export const redundancy: DimensionCopy = {
  what: "How much code is dead or duplicated? This is the share of production functions that nothing reaches, or that repeat another function exactly.",
  why: "Dead and duplicated code is read, maintained and tested for nothing.",
  action: (state) =>
    unlistedAction(state, {
      kind: "act",
      where: "command",
      target: "logos node <symbol>",
      text: "Check a suspect symbol's dead and duplicate flags with logos node, then delete the dead code and merge the duplicates.",
    }),
  raw: (raw) => `${percent(raw)} of production functions are dead or duplicated`,
  unlisted: {
    statement: "No list of the dead or duplicated functions is recorded with this snapshot. Each symbol's dead and duplicate flags are shown by",
    command: "logos node <symbol>",
  },
};

export const nesting: DimensionCopy = {
  what: "How deeply is control flow nested? This is the share of production functions nested at or beyond the nesting_depth threshold.",
  why: "Deeply nested code is hard to follow and easy to break when it changes.",
  action: (state) =>
    offenderAction(
      state,
      (n) => `Flatten the ${n} listed ${n === 1 ? "function" : "functions"} with early returns or extracted helpers`,
      "nesting_depth",
    ),
  raw: (raw) => `${percent(raw)} of production functions are deeply nested`,
};

export const conciseness: DimensionCopy = {
  what: [
    "How many ",
    gloss("brainMethod", "brain methods"),
    " are there? This is the share of production functions that are long, branchy and nested all at once.",
  ],
  why: "A brain method does too much to hold in your head, so it is where changes go wrong.",
  action: (state) =>
    offenderAction(
      state,
      (n) => `Split the ${n} listed ${n === 1 ? "function" : "functions"} into smaller ones`,
      "brain_complexity, brain_lines or brain_nesting",
    ),
  raw: (raw) => [`${percent(raw)} of production functions are `, gloss("brainMethod", "brain methods")],
};

export const cohesion: DimensionCopy = {
  what: [
    "Do classes hang together? This is the mean of 1/",
    gloss("lcom4"),
    " over production classes: 1 when every class's methods share its state.",
  ],
  why: "A class whose methods never share state is several classes in one, and is harder to change safely.",
  // Cohesion has no detection threshold to tune: LCOM4 is a count, not a cut-off.
  action: (state) =>
    offenderAction(
      state,
      (n) => `Split the ${n} listed ${n === 1 ? "class" : "classes"} along the groups of methods that share state`,
      null,
    ),
  raw: (raw) => ["mean 1/", gloss("lcom4"), ` ${raw.toFixed(2)} over classes`],
};

export const focus: DimensionCopy = {
  what: [
    "Are containers ",
    gloss("godContainer", "god-objects"),
    "? This is the share of classes and structs at or over the god_methods or god_span threshold.",
  ],
  why: "A god container gathers unrelated work, so every change to it risks the rest.",
  action: (state) =>
    offenderAction(
      state,
      (n) => `Split the ${n} listed ${n === 1 ? "container" : "containers"} by responsibility`,
      "god_methods or god_span",
    ),
  raw: (raw) => [`${percent(raw)} of classes and structs are `, gloss("godContainer", "god containers")],
};

export const uniqueness: DimensionCopy = {
  what: [
    "How much code is near-duplicated? This is the share of production functions that are ",
    gloss("nearClone", "near-clones"),
    " of another one.",
  ],
  why: "A fix to near-copied code has to be made in every copy, and one is usually missed.",
  action: (state) =>
    offenderAction(
      state,
      (n) => `Merge the ${n} listed near-${n === 1 ? "clone" : "clones"} into shared functions`,
      "clone_similarity or clone_min_tokens",
    ),
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
  "not-recorded": "Offenders were not recorded for this snapshot.",
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

export const signalTrend: CopyEntry<{ readonly snapshots: number }> = {
  what: "The quality signal at each recorded snapshot, oldest first, with its change from the one before.",
  why: "A drop between two snapshots points at the change that caused it.",
  action: ({ snapshots }) =>
    snapshots === 0
      ? { kind: "act", where: "command", target: "logos scan", text: "Run a scan to record the first snapshot." }
      : noAction,
};

export const NO_SNAPSHOTS = "No snapshots yet.";
