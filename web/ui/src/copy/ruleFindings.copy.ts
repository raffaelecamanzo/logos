/*
 * Rule findings copy catalogue (S-617, CR-203, FR-UI-39). One entry, shared by
 * the two views that render the architecture-rules report: the Rule findings
 * view and the member Dashboard's Rule findings widget — one figure, so one set
 * of words (the `coverage.copy.ts` precedent for a board two views render).
 *
 * A check over zero rules is not a pass (CR-141, S-438): with nothing checked
 * the action is to declare rules, never "nothing to do".
 */

import { noAction, plural, type CopyEntry } from "./types.ts";

export interface RuleFindingsState {
  /** Findings in the report (always-on structural fold-ins included). */
  readonly findings: number;
  /** Rules the contract declares and the check evaluated. */
  readonly checked: number;
}

export const ruleFindings: CopyEntry<RuleFindingsState> = {
  what: "Where the code breaks the architecture rules declared in .logos/rules.toml, with how many rules were checked.",
  why: "A finding is code your declared architecture forbids, such as a layer reached past or a forbidden import; the quality gate fails on it.",
  action: ({ findings, checked }) => {
    // Findings first: the always-on structural checks fire with no contract too.
    if (findings > 0) {
      return {
        kind: "act",
        where: "source code",
        text: "Fix each listed finding, or change the rule in .logos/rules.toml if the code is right.",
      };
    }
    if (checked === 0) {
      return {
        kind: "act",
        where: "configuration",
        target: ".logos/rules.toml",
        text: "Declare rules for your layers and forbidden imports, then run logos check to evaluate them.",
      };
    }
    return noAction;
  },
};

/** The figure-row and absence sentences. */
export const RULE_FINDINGS_TEXT = {
  /** "2 findings across 4 checked rules". */
  findings: (findings: number, checked: number) =>
    `${findings} ${plural(findings, "finding", "findings")} across ${checked} checked ${plural(checked, "rule", "rules")}`,
  /** A clean check over at least one rule. */
  clean: (checked: number) => `No findings — ${checked} ${plural(checked, "rule", "rules")} checked`,
  /** Nothing declared, or a contract that declares nothing: not a pass. */
  noRules: "No architecture rules yet, so nothing was checked — this is not a pass.",
} as const;
