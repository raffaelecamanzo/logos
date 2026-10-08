/*
 * Files & Risk copy catalogue (S-616, CR-203 items 23–24, FR-UI-39). One entry
 * per widget; `FilesView` renders them through `Widget`, and its tests name
 * these keys, never the prose. An absent ranking names the command that ranks
 * the files (CR-206).
 */

import type { CopyEntry } from "./types.ts";

export const filesRankedByRisk: CopyEntry = {
  what: "Files ranked by how often they change multiplied by how complex they are, most at risk first.",
  why: "A file that changes often and is hard to follow is the likeliest place for a change to break something.",
};

export const ownershipDispersion: CopyEntry = {
  what: "For each file, the share of its changes not made by its main author (Dispersion), and how evenly its changes spread across authors (Entropy).",
  why: "A file with no clear owner has its knowledge spread thin, so its changes are reviewed by the people who know it least.",
};

/** The named absences each widget states in its figure row. */
export const filesAbsence = {
  /** Shown when the read-model carries no notice of its own. */
  unranked: "No files ranked yet.",
  filteredOut: "No ranked file matches the current filter.",
  singleAuthor: "Single-author history — every file has one author, so its ownership cannot be dispersed.",
} as const;

/**
 * A catalogue's own nothing-ranked absence — this one's or Coverage's — closed
 * with the command that ranks the files (CR-206). One spelling for both views
 * that rank files; a trailing full stop is folded into the joint so the sentence
 * reads as one. Never applied to the read-model's own `notice`: that names a
 * degraded history (not a git repository, no `git`, a shallow clone) or a first
 * mine, and running `logos hotspots` changes none of them (R1, R3 of
 * `models::quality::absence`; FR-UI-39 "an absence nothing resolves names
 * nothing").
 */
export function withRankCommand(statement: string): string {
  return `${statement.replace(/\.\s*$/, "")}; run logos hotspots to rank the files from the git history.`;
}
