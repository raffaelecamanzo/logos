/*
 * Rule findings copy catalogue (S-617, CR-203, FR-UI-39). One entry, shared by
 * the two views that render the architecture-rules report: the Rule findings
 * view and the member Dashboard's Rule findings widget — one figure, so one set
 * of words (the `coverage.copy.ts` precedent for a board two views render).
 *
 * A check over zero rules is not a pass (CR-141, S-438): with nothing checked
 * the absence says so, and names where rules are declared and the command that
 * evaluates them (CR-206).
 */

import type { RulesReport, Violation } from "../api/types.ts";

import { plural, type CopyEntry } from "./types.ts";

export interface RuleFindingsState {
  /** Findings in the report (always-on structural fold-ins included), blessed
   *  ratchet debt excluded: blessed debt is a warning `check` passes (FR-GV-23). */
  readonly findings: number;
  /** Rules the contract declares and the check evaluated. */
  readonly checked: number;
}

/** Whether `v` is blessed ratchet debt (FR-GV-23): an offender held in the
 *  blessed set at or below its blessed value — a warning, rendered apart. */
export function isBlessedDebt(v: Violation): boolean {
  return v.ratchet?.status === "blessed";
}

/**
 * The widget's state from the report, for both views, so "a check over zero
 * rules is not a pass" is decided in one place. A report with no contract
 * counts as zero rules checked; findings are kept as they are, since the
 * always-on structural checks fire without a contract (S-354).
 */
export function ruleFindingsState(report: RulesReport): RuleFindingsState {
  const findings = report.violations.filter((v) => !isBlessedDebt(v)).length;
  const blessed = report.violations.length - findings;
  return { findings, checked: findings === 0 && blessed === 0 && !report.rules_present ? 0 : report.checked_rules };
}

export const ruleFindings: CopyEntry = {
  what: "Where the code breaks the architecture rules declared in .logos/rules.toml, with how many rules were checked.",
  why: "A finding is code your declared architecture forbids, such as a layer reached past or a forbidden import; the quality gate fails on it.",
};

/** The blessed-debt widget (S-508, FR-GV-23): the ratchet's tolerated offenders,
 *  shown apart from the findings that fail the check. */
export const blessedDebt: CopyEntry = {
  what: "Offenders of a ratchet rule that the checked-in blessed set tolerates, and how many blessed offenders have been fixed since the set was blessed.",
  why: "Blessed debt is a warning, not a failure: the check fails only on new or worsened offenders, and re-blessing after fixes tightens the set.",
};

/** The figure-row and absence sentences. */
export const RULE_FINDINGS_TEXT = {
  /** "2 blessed · 1 fixed since bless" (S-508, FR-GV-23). */
  blessed: (blessed: number, fixed: number) => `${blessed} blessed · ${fixed} fixed since bless`,
  /** "2 findings across 4 checked rules". */
  findings: (findings: number, checked: number) =>
    `${findings} ${plural(findings, "finding", "findings")} across ${checked} checked ${plural(checked, "rule", "rules")}`,
  /** A clean check over at least one rule. */
  clean: (checked: number) => `No findings — ${checked} ${plural(checked, "rule", "rules")} checked`,
  /** The view's verdict line over zero checked rules. */
  noneChecked: "no rules checked",
  /** Nothing declared, or a contract that declares nothing: not a pass. */
  noRules:
    "No architecture rules yet, so nothing was checked — this is not a pass; declare rules in .logos/rules.toml, then run logos check.",
} as const;
