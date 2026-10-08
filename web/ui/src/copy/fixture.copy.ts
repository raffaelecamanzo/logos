/*
 * The fixture catalogue (S-611). Not a view's catalogue: it exercises the entry
 * type, `expectWidgetCopy` and the unglossed-term test over both action kinds,
 * and it is the copy the Playwright harness page renders. The catalogue test
 * discovers it like any `*.copy.ts`, so it is held to the same vocabulary rule.
 */

import { gloss, noAction, type CopyEntry } from "./types.ts";

/** A widget with nothing to do in any state. */
export const observe: CopyEntry = {
  what: "12 of 40 outbound call sites resolve to a service in this workspace.",
  why: "A call that resolves is one the service map can draw and the reachability check can follow.",
  action: () => noAction,
};

/** A widget whose action depends on its state. */
export const thresholds: CopyEntry<{ breached: number }> = {
  what: ["Measures that fall below their threshold, counted across every ", gloss("arm", "arm"), "."],
  why: "A breached threshold fails the quality gate on the next push.",
  action: ({ breached }) =>
    breached > 0
      ? {
          kind: "act",
          where: "configuration",
          target: ".logos/rules.toml [metric_thresholds]",
          text: "Fix the code the measure names, or relax the threshold if it no longer fits this project.",
        }
      : noAction,
};

/** A widget whose figure can be absent. */
export const coverage: CopyEntry<{ ingested: boolean }> = {
  what: "Line coverage of the files this project indexes.",
  why: "Uncovered code that changes often is where a regression hides.",
  action: ({ ingested }) =>
    ingested
      ? noAction
      : { kind: "act", where: "command", target: "logos coverage ingest", text: "Ingest a coverage report." },
};
