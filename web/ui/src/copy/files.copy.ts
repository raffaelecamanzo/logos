/*
 * Files & Risk copy catalogue (S-616, CR-203 items 23–24, FR-UI-39). One entry
 * per widget; `FilesView` renders them through `Widget`, and its tests name
 * these keys, never the prose.
 */

import { noAction, type CopyEntry } from "./types.ts";

/** Files ranked by risk: what the board holds decides what to do about it. */
export interface RiskState {
  /** Files on the board; 0 when the history has not been ranked, or a filter left none. */
  readonly ranked: number;
  /** A filter (untested only, production files only) is narrowing the board. */
  readonly filtered: boolean;
  /** No coverage report is ingested, so every Coverage cell reads n/a. */
  readonly coverageMissing: boolean;
}

export const filesRankedByRisk: CopyEntry<RiskState> = {
  what: "Files ranked by how often they change multiplied by how complex they are, most at risk first.",
  why: "A file that changes often and is hard to follow is the likeliest place for a change to break something.",
  action: ({ ranked, filtered, coverageMissing }) => {
    // An empty board under a filter is the filter's answer, not a missing ranking.
    if (ranked === 0 && filtered) return noAction;
    if (ranked === 0) {
      return {
        kind: "act",
        where: "command",
        target: "logos hotspots",
        text: "Rank the files: the command reads the git history and scores each file.",
      };
    }
    if (coverageMissing) {
      return {
        kind: "act",
        where: "command",
        target: "logos coverage ingest",
        text: "Ingest a coverage report, so the Coverage column shows which of the top files lack tests.",
      };
    }
    return {
      kind: "act",
      where: "source code",
      text: "Add tests to the files at the top of the list, or split them into smaller files.",
    };
  },
};

/** Ownership dispersion: more than one author is what makes it actionable. */
export interface OwnershipState {
  /** Some file has changes by more than one author. */
  readonly multiAuthor: boolean;
}

export const ownershipDispersion: CopyEntry<OwnershipState> = {
  what: "For each file, the share of its changes not made by its main author (Dispersion), and how evenly its changes spread across authors (Entropy).",
  why: "A file with no clear owner has its knowledge spread thin, so its changes are reviewed by the people who know it least.",
  action: ({ multiAuthor }) =>
    multiAuthor
      ? {
          kind: "act",
          where: "documentation",
          target: "CODEOWNERS",
          text: "Name an owner for the most dispersed files, for example in CODEOWNERS.",
        }
      : noAction,
};

/** The named absences each widget states in its figure row. */
export const filesAbsence = {
  /** Shown when the read-model carries no notice of its own. */
  unranked: "No files ranked yet.",
  filteredOut: "No ranked file matches the current filter.",
  singleAuthor: "Single-author history — every file has one author, so its ownership cannot be dispersed.",
} as const;
