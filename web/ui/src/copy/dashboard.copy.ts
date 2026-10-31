/*
 * The member Dashboard's copy catalogue (S-617, CR-203, FR-UI-39). One entry per
 * widget; the Rule findings widget shares `ruleFindings.copy.ts` with the Rule
 * findings view, since both render the one rules report.
 *
 * Every figure is the read-model's own (NFR-CC-04); these sentences say what it
 * shows and why it matters, and each absence names the command that fills it
 * (CR-206).
 */

import { COVERAGE_VIEW_ABSENCE } from "./coverageView.copy.ts";
import { gloss, plural, type CopyEntry } from "./types.ts";

export const qualityIndex: CopyEntry = {
  what: ["The quality signal from the last scan, out of 10,000, and whether it holds its ", gloss("baseline"), "."],
  why: "The quality gate compares this figure, so a drop below the floor fails a push or a CI run.",
};

export const codeCoverage: CopyEntry = {
  what: "The share of lines your tests cover, from the last coverage report ingested.",
  why: "Uncovered code is code a change can break without a test noticing.",
};

export const securityWidget: CopyEntry = {
  what: "The security findings that count — fresh, not suppressed and in production code — by severity, the verdict of the [security] table, when the newest scan ran, and how many scanners failed their last scan.",
  why: "A scanner that failed or never ran has looked at nothing, so a count of zero beside it is not a clean result.",
};

export const languages: CopyEntry = {
  what: "The languages indexed here, sized by how many symbols each holds — symbols, not files.",
  why: "It shows which code the graph can see; a language missing here is code no question about the graph will reach.",
};

export const graph: CopyEntry = {
  what: "The size of the code graph — files, lines of code, symbols and the links between them — and the share of internal candidate references it resolved, as a lower bound on the true share.",
  why: "Answers about callers and impact are only as complete as the references the graph resolved.",
};

export const activity: CopyEntry = {
  what: "Calls to Logos over the window, their latency, and the file reads they are estimated to have saved.",
  why: "It shows whether Logos is in use here, and roughly what it saves.",
};

/** The Graph widget's one-sentence lower-bound explanation (FR-RS-49): why the
 *  headline is not the whole story, and where the remaining gap is. */
export const RESOLUTION_LOWER_BOUND =
  "The share is a lower bound on what resolves: method calls on values of unknown type stay unresolved, so the true share is higher.";

/** The named absences each widget states in its figure row. */
export const DASHBOARD_ABSENCE = {
  noSignal: "No quality signal recorded yet; run logos scan to record one.",
  /** The Coverage view's own sentence, so the ingest command is worded once. */
  noCoverage: COVERAGE_VIEW_ABSENCE.notIngested,
  /** Nothing ever scanned, and scanning is already allowed. */
  noSecurityScan: "No security scan has run yet, so nothing was looked at; run logos security scan.",
  /** Nothing ever scanned, and no scanner is consented to yet. */
  noSecurityConsent:
    "No security scan has run yet and no scanner is consented to; run logos security scan --consent to allow and run the first one.",
  /** A read that failed is stated with its cause, never shown as zero findings. */
  securityFault: (cause: string) => `The security state could not be read, so no count is shown: ${cause}`,
  /** The gate band's absence: no `[security]` table, so nothing is evaluated. */
  noSecurityTable: "No [security] table in .logos/rules.toml, so no security verdict is reached.",
  /** The graph records no code reference, so there is no share to take. */
  noCodeReferences: "No code references recorded, so there is no share of them to state.",
  /** Every code reference is provably external: none could resolve here. */
  allReferencesExternal: (provablyExternal: number) =>
    `All ${provablyExternal.toLocaleString("en-US")} code ${plural(provablyExternal, "reference is", "references are")} provably external, so none could resolve here and there is no share to state.`,
  /** No share can be read: a server older than the one that classifies references,
   *  or an absence this build has no sentence for. */
  resolutionUnreported: "This server reported no share of references resolved, so none is shown.",
  noLanguages: "No languages indexed yet; run logos index.",
  noLines: "Lines of code not yet counted; a full logos index counts them.",
  noTelemetry:
    "No telemetry yet; use Logos from your agent or the command line, and logos stats shows what has been recorded.",
} as const;
