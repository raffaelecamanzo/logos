/*
 * The member Dashboard's copy catalogue (S-617, CR-203, FR-UI-39). One entry per
 * widget; the Rule findings widget shares `ruleFindings.copy.ts` with the Rule
 * findings view, since both render the one rules report.
 *
 * Every figure is the read-model's own (NFR-CC-04); these sentences say what it
 * shows and why it matters, and each absence names the command that fills it
 * (CR-206). Project Overview is the one entry with no explanation (CR-208).
 */

import { COVERAGE_VIEW_ABSENCE } from "./coverageView.copy.ts";
import { gloss, type CopyEntry, type NoExplanationEntry } from "./types.ts";

export const qualityIndex: CopyEntry = {
  what: ["The quality signal from the last scan, out of 10,000, and whether it holds its ", gloss("baseline"), "."],
  why: "The quality gate compares this figure, so a drop below the floor fails a push or a CI run.",
};

export const codeCoverage: CopyEntry = {
  what: "The share of lines your tests cover, from the last coverage report ingested.",
  why: "Uncovered code is code a change can break without a test noticing.",
};

export const languages: CopyEntry = {
  what: "The languages indexed here, sized by how many symbols each holds — symbols, not files.",
  why: "It shows which code the graph can see; a language missing here is code no question about the graph will reach.",
};

export const graph: CopyEntry = {
  what: "The size of the code graph — files, lines of code, symbols and the links between them — and the share of references it resolved.",
  why: "Answers about callers and impact are only as complete as the references the graph resolved.",
};

export const activity: CopyEntry = {
  what: "Calls to Logos over the window, their latency, and the file reads they are estimated to have saved.",
  why: "It shows whether Logos is in use here, and roughly what it saves.",
};

/** The one entry with no explanation (CR-208): the wiki snippet says what it is,
 *  so the widget renders its title and its snippet — or its absence — alone. */
export const projectOverview: NoExplanationEntry = { noExplanation: "project-overview" };

/** The named absences each widget states in its figure row. */
export const DASHBOARD_ABSENCE = {
  noSignal: "No quality signal recorded yet; run logos scan to record one.",
  /** The Coverage view's own sentence, so the ingest command is worded once. */
  noCoverage: COVERAGE_VIEW_ABSENCE.notIngested,
  noLanguages: "No languages indexed yet; run logos index.",
  noLines: "Lines of code not yet counted; a full logos index counts them.",
  noTelemetry:
    "No telemetry yet; use Logos from your agent or the command line, and logos stats shows what has been recorded.",
  noOverview:
    "No project overview generated yet; have an agent write it — the logos-wiki skill does this with logos wiki write overview/project-overview.",
} as const;
