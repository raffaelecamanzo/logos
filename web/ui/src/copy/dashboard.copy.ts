/*
 * The member Dashboard's copy catalogue (S-617, CR-203, FR-UI-39). One entry per
 * widget; the Rule findings widget shares `ruleFindings.copy.ts` with the Rule
 * findings view, since both render the one rules report.
 *
 * Every figure is the read-model's own (NFR-CC-04); these sentences name what a
 * reader does about it and where.
 */

import { gloss, noAction, type CopyEntry } from "./types.ts";

export interface QualityIndexState {
  /** Whether a scan has recorded a quality signal. */
  readonly recorded: boolean;
  /** The gate verdict on that signal. */
  readonly passed: boolean;
}

export const qualityIndex: CopyEntry<QualityIndexState> = {
  what: ["The quality signal from the last scan, out of 10,000, and whether it holds its ", gloss("baseline"), "."],
  why: "The quality gate compares this figure, so a drop below the floor fails a push or a CI run.",
  action: ({ recorded, passed }) => {
    if (!recorded) {
      return {
        kind: "act",
        where: "command",
        target: "logos scan",
        text: "Run a scan to record a signal.",
      };
    }
    if (!passed) {
      return {
        kind: "act",
        where: "source code",
        text: "Open Health and start with its lowest-scoring dimension.",
      };
    }
    return noAction;
  },
};

export interface CodeCoverageState {
  /** Whether a coverage report has been ingested. */
  readonly ingested: boolean;
}

export const codeCoverage: CopyEntry<CodeCoverageState> = {
  what: "The share of lines your tests cover, from the last coverage report ingested.",
  why: "Uncovered code is code a change can break without a test noticing.",
  action: ({ ingested }) =>
    ingested
      ? noAction
      : {
          kind: "act",
          where: "command",
          target: "logos coverage ingest <report>",
          text: "Ingest the LCOV or Cobertura report your test run writes.",
        },
};

export interface IndexedState {
  /** Whether the index holds what this widget reports. */
  readonly indexed: boolean;
}

export const languages: CopyEntry<IndexedState> = {
  what: "The languages indexed here, sized by how many symbols each holds — symbols, not files.",
  why: "It shows which code the graph can see; a language missing here is code no question about the graph will reach.",
  action: ({ indexed }) =>
    indexed
      ? noAction
      : { kind: "act", where: "command", target: "logos index", text: "Index the project." },
};

export interface GraphState {
  /** Whether the line counts were computed (a full index computes them). */
  readonly linesCounted: boolean;
}

export const graph: CopyEntry<GraphState> = {
  what: "The size of the code graph — files, lines of code, symbols and the links between them — and the share of references it resolved.",
  why: "Answers about callers and impact are only as complete as the references the graph resolved.",
  action: ({ linesCounted }) =>
    linesCounted
      ? noAction
      : {
          kind: "act",
          where: "command",
          target: "logos index",
          text: "Run a full index to count the lines of code.",
        },
};

export interface ActivityState {
  /** Whether any call was recorded in the window. */
  readonly recorded: boolean;
}

export const activity: CopyEntry<ActivityState> = {
  what: "Calls to Logos over the window, their latency, and the file reads they are estimated to have saved.",
  why: "It shows whether Logos is in use here, and roughly what it saves.",
  action: ({ recorded }) =>
    recorded
      ? noAction
      : {
          kind: "act",
          where: "command",
          target: "logos stats",
          text: "Use Logos from your agent or the command line, and check with the command what has been recorded.",
        },
};

export interface ProjectOverviewState {
  /** Whether the wiki holds the project overview page. */
  readonly written: boolean;
}

export const projectOverview: CopyEntry<ProjectOverviewState> = {
  what: "The opening of the wiki's project overview, written by an agent about this codebase.",
  why: "It orients a reader new to the code before the figures below.",
  action: ({ written }) =>
    written
      ? noAction
      : {
          kind: "act",
          where: "command",
          target: "logos wiki write overview/project-overview",
          text: "Have an agent write the page; the logos-wiki skill does this with the command.",
        },
};

/** The named absences each widget states in its figure row. */
export const DASHBOARD_ABSENCE = {
  noSignal: "No quality signal recorded yet.",
  noCoverage: "No coverage ingested yet.",
  noLanguages: "No languages indexed yet.",
  noLines: "Lines of code not yet counted.",
  noTelemetry: "No telemetry yet.",
  noOverview: "No project overview generated yet.",
} as const;
