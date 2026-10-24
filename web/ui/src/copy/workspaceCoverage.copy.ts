/*
 * The Workspace coverage catalogue (S-740, CR-234, FR-CV-16, FR-UI-39): every
 * member's line coverage over its fresh files, fresh files of total, its tools'
 * outcomes and when one last ran.
 *
 * Every figure the sentences carry is the server's (NFR-MA-02); these format the
 * numbers they are handed and compute none. A member never run reads "never
 * run", never 0 % (BR-77), and the one workspace figure is a count of members
 * with its denominator (NFR-CC-04) — no percentage is averaged across members
 * (BR-56).
 */

import { plural, type CopyEntry } from "./types.ts";

export const memberCoverage: CopyEntry = {
  what: "Each member's line coverage over the files unchanged since its last coverage report, how many of its covered files are unchanged, how many of its coverage tools ran or failed, and when one last ran, linked to that member's Coverage page.",
  why: "A member nothing has measured reads never run rather than 0 %, and each percentage is that member's own, so a well-tested member never hides an untested one in an average.",
};

/** The figure-row, cell and note sentences. */
export const WORKSPACE_COVERAGE_TEXT = {
  /** The page's heading line. */
  heading: (members: number) =>
    `${members} ${plural(members, "member", "members")} · this view answers for the whole workspace, not for the selected member`,
  /** The figure row: "coverage from m of N members; k never run". */
  denominator: (fresh: number, total: number, neverRun: number) =>
    `Coverage from ${fresh} of ${total} ${plural(total, "member", "members")}; ${neverRun} ${plural(neverRun, "member", "members")} never run.`,
  /** The figure row's second line: the percentages are not combined. */
  perMember: "Each percentage is the member's own; none is summed or averaged across members.",
  /** Members that could not be opened or read. */
  unread: (n: number) =>
    `${n} ${plural(n, "member", "members")} could not be read, so the figure above is over fewer than all members:`,
  /** The Coverage cell of a member with nothing ingested. */
  neverRun: "never run",
  /** The Coverage cell of a member whose reports matched no indexed file. */
  unmatched: "no file measured — the last report matched none of this member's indexed files",
  /** The Coverage cell of a member whose every covered file changed since. */
  stale: "stale — every covered file changed since the last report",
  /** The Coverage cell of a member with fresh files and no percentage: its own
   *  `coverage status` states no figure when no line of them is covered (FR-CV-06). */
  noCoveredLine: "no figure — no line of its unchanged files is covered",
  /** A cell of a member that could not be read. */
  notRead: "not read",
  /** The Fresh files column: unchanged of covered. */
  freshFiles: (fresh: number, total: number) => `${fresh} of ${total}`,
  /** The Tools column: ran and failed of the tools that apply. */
  tools: (ran: number, failed: number, applicable: number) => `ran ${ran} · failed ${failed} of ${applicable} applicable`,
  /** The Last run cell before any tool ran. */
  noRun: "—",
  /** Why the page has no Run button. */
  noRunHere:
    "This page starts no coverage run: open a member's Coverage page to run it there, under that member's consent.",
} as const;

/** The named absences the widget states in its figure row. */
export const WORKSPACE_COVERAGE_ABSENCE = {
  /** No member could be read at all. */
  noneRead: "No member could be read, so nothing is counted; open a member's Coverage page to see why it does not answer.",
} as const;
