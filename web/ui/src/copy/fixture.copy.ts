/*
 * The fixture catalogue (S-611). Not a view's catalogue: it exercises the entry
 * type, `expectWidgetCopy` and the unglossed-term test, and it is the copy the
 * Playwright harness page renders. The catalogue test discovers it like any
 * `*.copy.ts`, so it is held to the same vocabulary rule.
 */

import { gloss, type CopyEntry } from "./types.ts";

/** A plain widget. */
export const observe: CopyEntry = {
  what: "12 of 40 outbound call sites resolve to a service in this workspace.",
  why: "A call that resolves is one the service map can draw and the reachability check can follow.",
};

/** A widget whose copy glosses a term. */
export const thresholds: CopyEntry = {
  what: ["Measures that fall below their threshold, counted across every ", gloss("arm", "arm"), "."],
  why: "A breached threshold fails the quality gate on the next push.",
};

/** A widget whose figure can be absent. */
export const coverage: CopyEntry = {
  what: "Line coverage of the files this project indexes.",
  why: "Uncovered code that changes often is where a regression hides.",
};
