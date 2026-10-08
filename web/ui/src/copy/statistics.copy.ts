/*
 * Statistics copy catalogue (S-616, CR-203 item 25, FR-UI-27, FR-UI-39). The
 * awaiting-data absence names the command that shows what is recorded (CR-206).
 */

import { gloss, type CopyEntry } from "./types.ts";

export const estimatedValue: CopyEntry = {
  what: "Tokens and ad-hoc file reads that navigating by the code graph is estimated to have saved over the selected window, with the calls and their latency.",
  why: "It shows whether Logos pays for itself here — an estimate, valued at a fixed number of tokens per avoided read, not a measured figure.",
};

export const usageOverTime: CopyEntry = {
  what: "Calls to Logos per day over the selected window, and how many of them succeeded.",
  why: "A steady line means Logos is part of the daily workflow; a gap marks the days it went unused.",
};

export const topToolsAndSurfaces: CopyEntry = {
  what: "The most-called tools, and how the calls split across surfaces: the command line, MCP and this web UI.",
  why: "It shows which capabilities the work relies on, and which surface it goes through.",
};

export const devVsMain: CopyEntry = {
  what: "Calls made from development worktrees, all branches combined, against calls made on main.",
  why: "It separates the use of Logos while a change is being built from its use on the main checkout.",
};

export const toolAttribution: CopyEntry = {
  what: [
    "Calls per tool and origin, grouped by tool class and then by calls, with how many were ",
    gloss("answered", "answered"),
    ".",
  ],
  why: "It shows which kinds of tool return answers, and whether they are used while building a change or on main.",
};

/** Leads the read-model's own attribution caveats, which follow it verbatim. */
export const attributionNotesLead = "The limits of this attribution, as the read-model states them:";

/** The named absences each widget states in its figure row. */
export const statisticsAbsence = {
  awaiting:
    "No telemetry recorded yet — use Logos from your agent or the command line, and this view fills in as calls are recorded; logos stats shows what has been recorded so far.",
  activity: "No activity in this window.",
  tools: "No tool calls in this window.",
  surfaces: "No surface usage in this window.",
  origins: "No attributed usage in this window.",
  attribution: "No attributed tool calls in this window.",
} as const;
