/*
 * The Coverage view's copy catalogue (S-617, CR-203, FR-UI-39): test coverage
 * from ingested reports. (The workspace's cross-service coverage boards have
 * their own catalogue, `coverage.copy.ts`.)
 */

import { noAction, type CopyEntry } from "./types.ts";

const INGEST = {
  kind: "act",
  where: "command",
  target: "logos coverage ingest <report>",
  text: "Ingest the LCOV or Cobertura report your test run writes.",
} as const;

export interface UntestedState {
  /** Whether a coverage report has been ingested. */
  readonly ingested: boolean;
  /** Whether the hotspot board ranked any file; with none ranked (no git
   *  history, say), an empty board measured nothing. */
  readonly ranked: boolean;
  /** Files on the board. */
  readonly files: number;
}

export const untestedHotspots: CopyEntry<UntestedState> = {
  what: "The files ranked most at risk — they change often and are complex — that no fresh coverage covers.",
  why: "A risky file with no tests is where a change most likely breaks something unnoticed.",
  action: ({ ingested, ranked, files }) => {
    if (!ingested) return INGEST;
    if (!ranked) {
      return {
        kind: "act",
        where: "command",
        target: "logos hotspots",
        text: "Rank the files first: the command reads the git history and scores each file.",
      };
    }
    if (files > 0) {
      return { kind: "act", where: "source code", text: "Add tests to the listed files, starting at the top." };
    }
    return noAction;
  },
};

export interface PerFileState {
  /** Whether a coverage report has been ingested. */
  readonly ingested: boolean;
  /** Files whose report predates their last change. */
  readonly stale: number;
}

export const perFileCoverage: CopyEntry<PerFileState> = {
  what: "Each file's line coverage from the ingested reports; a file changed since its report reads STALE, not a number.",
  why: "A stale figure describes code that has since changed, so it would overstate or understate what the tests cover.",
  action: ({ ingested, stale }) => {
    if (!ingested) return INGEST;
    if (stale > 0) {
      return {
        kind: "act",
        where: "command",
        target: "logos coverage refresh",
        text: "Re-run your tests and ingest the new report, so the stale files are measured again.",
      };
    }
    return noAction;
  },
};

/** The named absences each widget states in its figure row. */
export const COVERAGE_VIEW_ABSENCE = {
  notIngested: "No coverage ingested yet.",
  noUntested: "No untested file among the files ranked most at risk.",
  /** Shown when the read-model carries no notice of its own. */
  notRanked: "No files ranked yet, so no untested file can be named.",
  noFiles: "No covered files in the ingested reports.",
} as const;
