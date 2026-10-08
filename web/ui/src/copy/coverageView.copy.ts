/*
 * The Coverage view's copy catalogue (S-617, CR-203, FR-UI-39): test coverage
 * from ingested reports. (The workspace's cross-service coverage boards have
 * their own catalogue, `coverage.copy.ts`.) Each absence names the command that
 * fills it (CR-206).
 */

import type { CopyEntry } from "./types.ts";

export const untestedHotspots: CopyEntry = {
  what: "The files ranked most at risk — they change often and are complex — that no fresh coverage covers.",
  why: "A risky file with no tests is where a change most likely breaks something unnoticed.",
};

export const perFileCoverage: CopyEntry = {
  what: "Each file's line coverage from the ingested reports; a file changed since its report reads STALE, not a number.",
  why: "A stale figure describes code that has since changed, so it would overstate or understate what the tests cover.",
};

/** The named absences each widget states in its figure row. */
export const COVERAGE_VIEW_ABSENCE = {
  notIngested:
    "No coverage ingested yet; run logos coverage ingest <report> on the LCOV or Cobertura report your test run writes.",
  noUntested: "No untested file among the files ranked most at risk.",
  /** Shown when the read-model carries no notice of its own. */
  notRanked: "No files ranked yet, so no untested file can be named.",
  noFiles: "No covered files in the ingested reports.",
} as const;
