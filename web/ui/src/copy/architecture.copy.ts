/*
 * The Architecture view's copy catalogue (S-617, CR-203, FR-UI-39). The
 * Dependency matrix leads the view; the cycle list is hidden through the
 * hidden-widget register (S-612) and carries its entry so that removing the
 * register entry brings it back already explained.
 */

import { noAction, type CopyEntry } from "./types.ts";

export interface CyclesState {
  /** Module pairs (cells marked ↺) whose dependencies point back against layer order. */
  readonly backEdges: number;
}

const BREAK_THEM = {
  kind: "act",
  where: "source code",
  text: "Break each dependency marked ↺: move the code both modules need into the lower one, or have the lower module depend on an interface instead.",
} as const;

export const dependencyMatrix: CopyEntry<CyclesState> = {
  what: "How many dependencies each module has on every other module, in layer order; a cell marked ↺ points back against that order.",
  why: "A dependency against layer order closes a cycle, so the modules in it can no longer be changed, tested or reused apart.",
  action: ({ backEdges }) => (backEdges > 0 ? BREAK_THEM : noAction),
};

export const cycles: CopyEntry<CyclesState> = {
  what: "Each dependency that points back against layer order, from one module to another, with how many dependencies make it.",
  why: "Each one closes a cycle, so the modules in it can no longer be changed, tested or reused apart.",
  action: ({ backEdges }) => (backEdges > 0 ? BREAK_THEM : noAction),
};
