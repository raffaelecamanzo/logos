/*
 * The Workspace Statistics catalogue (S-617, CR-203, FR-UI-37, FR-UI-39): usage
 * of Logos summed over the members whose telemetry could be read.
 *
 * A read failure is not a shortage of usage, so where a member could not be read
 * the awaiting-data absence names that member's store, never `logos stats`
 * (FR-UI-37, review finding A3-F1 of S-429); `WORKSPACE_STATISTICS_ABSENCE.storeRepair`
 * is that sentence.
 */

import type { CopyEntry } from "./types.ts";

export const workspaceEstimatedValue: CopyEntry = {
  // Worded apart from the figure ("12,345 tokens and 88 ad-hoc file reads …"),
  // which the view's tests locate by its own phrasing.
  what: "The estimated saving from navigating by the code graph across this workspace over the window, counted in ad-hoc file reads avoided and the tokens they would have cost, with the calls behind it.",
  why: "It shows whether Logos pays for itself here — an estimate, valued at a fixed number of tokens per avoided read, not a measured figure.",
};

export const workspaceUsageOverTime: CopyEntry = {
  what: "Calls to Logos per day across every member read, and how many of them succeeded.",
  why: "A steady line means Logos is part of the daily work across the services; a gap marks the days it went unused.",
};

export const workspaceTopTools: CopyEntry = {
  what: "The tools called most across every member read, ranked by calls.",
  why: "It shows which capabilities the work across these services relies on.",
};

export const workspaceDevVsMain: CopyEntry = {
  what: "Calls made from development worktrees, all branches combined, against calls made on main, summed across members; rolled-up days carry no origin, so this split can sum to less than total calls.",
  why: "It separates the use of Logos while a change is being built from its use on the main checkout.",
};

export const membersNotSummed: CopyEntry = {
  what: "Every member that contributed nothing to the figures above, each with its reason.",
  // The absent-store sentence is the population callout's, said only when a
  // member IS absent; this one holds in every state.
  why: "A member with no telemetry store has simply not used Logos yet, while one that could not be read leaves its usage unknown.",
};

/** The named absences each widget states in its figure row. */
export const WORKSPACE_STATISTICS_ABSENCE = {
  /** Nothing recorded, and every member was read: the command shows what each recorded. */
  nothingRecorded:
    "No member recorded any telemetry in this window — use Logos in any service and this view will fill in; logos stats in a member shows what it has recorded.",
  /** Closes the absence when a member's store could not be read: the store, not usage, is the blocker. */
  storeRepair:
    "A locked .logos/telemetry.db answers again once its writer finishes, so try again; an unreadable one needs its file permissions fixed, or the file moved aside so a new one is recorded.",
  activity: "No activity in this window.",
  tools: "No tool calls in this window.",
  origins: "No attributed usage in this window.",
} as const;
