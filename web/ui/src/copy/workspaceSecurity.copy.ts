/*
 * The Workspace security catalogue (S-674, CR-217, FR-SC-16, FR-UI-39): every
 * member's scanner states and fresh, unsuppressed findings, and the OWASP Top 10
 * summed over the members read.
 *
 * Every figure the sentences carry is the server's (NFR-MA-02); these format the
 * numbers they are handed and compute none. A member never scanned reads "never
 * run", never 0 (BR-66), and every total says what it counts and over how many
 * members (NFR-CC-04).
 */

import { plural, type CopyEntry } from "./types.ts";

export const memberSecurity: CopyEntry = {
  what: "Each member's scanners with a fresh scan at its current commit, and its fresh, unsuppressed findings by severity, linked to that member's Security page.",
  why: "A member nothing has scanned has looked at nothing, so it reads never run rather than a clean zero; a stale or suppressed finding is not in these counts.",
};

export const workspaceOwasp: CopyEntry = {
  what: "The fresh, unsuppressed findings of every member read, grouped by OWASP Top 10 category for the chosen edition, with how many members a fresh scan covers each category in.",
  why: "A category no member's fresh scan can detect is a blind spot across the workspace, which a count of zero would hide.",
};

/** The figure-row, cell and note sentences. */
export const WORKSPACE_SECURITY_TEXT = {
  /** The page's heading line. */
  heading: (members: number) =>
    `${members} ${plural(members, "member", "members")} · this view answers for the whole workspace, not for the selected member`,
  /** The roll-up's denominator: "findings from m of N members; k never scanned". */
  denominator: (counted: number, total: number, neverScanned: number) =>
    `Findings from ${counted} of ${total} ${plural(total, "member", "members")}; ${neverScanned} ${plural(neverScanned, "member", "members")} never scanned.`,
  /** The totals in words, with what they count. */
  totals: (critical: number, high: number, medium: number, low: number, info: number) =>
    `${critical} critical · ${high} high · ${medium} medium · ${low} low · ${info} info — fresh, unsuppressed findings only`,
  /** What the totals leave out, and why. */
  excluded: (notFresh: number, suppressed: number) =>
    `Left out: ${notFresh} ${plural(notFresh, "finding", "findings")} from reports that are not fresh (stale, or from a scanner now disabled), and ${suppressed} suppressed.`,
  /** Members that could not be opened or read. */
  unread: (n: number) =>
    `${n} ${plural(n, "member", "members")} could not be read, so every total here is over fewer than all members:`,
  /** A figure cell of a member never scanned. */
  neverRun: "never run",
  /** A figure cell of a member whose stored reports are all stale. */
  stale: "no fresh scan",
  /** A cell of a member that could not be read. */
  notRead: "not read",
  /** The scanners column: fresh of enabled. */
  scannersFresh: (fresh: number, enabled: number) => `${fresh} of ${enabled} enabled`,
  /** A category's coverage across the members read. */
  coveredIn: (covered: number, read: number) =>
    `covered in ${covered} of ${read} ${plural(read, "member", "members")} read`,
  /** A category no member covers. */
  notCovered: "not covered: no member has a fresh scan from a scanner that can detect it",
  /** Why the page has no scan button. */
  noScanHere:
    "This page starts no scan: open a member's Security page to scan it, or run logos security scan in that member.",
} as const;

/** The named absences each widget states in its figure row. */
export const WORKSPACE_SECURITY_ABSENCE = {
  /** No member could be read at all. */
  noneRead: "No member could be read, so nothing is summed; open a member's Security page to see why it does not answer.",
  /** Members read, and none has a fresh scan: nothing current to count. */
  noneFresh:
    "No member has a fresh scan, so there is nothing current to count — never run is not zero; run logos security scan in a member to fill this in.",
} as const;
