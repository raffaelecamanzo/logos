/*
 * The Architecture view's copy catalogue (S-617, CR-203, FR-UI-39). The
 * Dependency matrix leads the view; the cycle list is hidden through the
 * hidden-widget register (S-612) and carries its entry so that removing the
 * register entry brings it back already explained.
 */

import type { CopyEntry } from "./types.ts";

export const dependencyMatrix: CopyEntry = {
  what: "How many dependencies each module has on every other module, in layer order; a cell marked ↺ points back against that order.",
  why: "A dependency against layer order closes a cycle, so the modules in it can no longer be changed, tested or reused apart.",
};

export const cycles: CopyEntry = {
  what: "Each dependency that points back against layer order, from one module to another, with how many dependencies make it.",
  why: "Each one closes a cycle, so the modules in it can no longer be changed, tested or reused apart.",
};
