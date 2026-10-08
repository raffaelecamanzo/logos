/*
 * The Workspace Health catalogue (S-613, CR-203 §3.2 D item 5, FR-UI-39). This
 * story converts Workspace rules; the view's other widgets join this catalogue
 * when they are converted (S-617).
 *
 * Every figure the sentences carry is the server's (NFR-MA-02); `HEALTH_TEXT`
 * formats the numbers it is handed and computes none.
 */

import { noAction, plural, type CopyEntry } from "./types.ts";

// ── Workspace rules (item 5) ─────────────────────────────────────────────────

export interface WorkspaceRulesState {
  /** Whether the manifest declares any workspace rule at all. */
  readonly declared: boolean;
  /** Rule findings. */
  readonly findings: number;
  /** Rule references naming a member this workspace does not have. */
  readonly unknownMembers: number;
}

export const workspaceRules: CopyEntry<WorkspaceRulesState> = {
  what: "The workspace rules declared in logos.workspace.toml, checked against every cross-service binding this workspace resolved, with each finding listed.",
  why: "A finding is a call your declared architecture forbids; the check is advisory, so it moves no exit code and no member's quality signal.",
  action: ({ declared, findings, unknownMembers }) => {
    if (!declared) {
      return {
        kind: "act",
        where: "configuration",
        target: "logos.workspace.toml [[governance.boundaries]]",
        text: "Declare [[governance.boundaries]] in logos.workspace.toml to have cross-service calls checked.",
      };
    }
    if (unknownMembers > 0) {
      return {
        kind: "act",
        where: "configuration",
        target: "logos.workspace.toml",
        text: "Correct each rule that names a member this workspace does not have; until then it can never match.",
      };
    }
    if (findings > 0) {
      return {
        kind: "act",
        where: "source code",
        text: "Remove each listed call that breaks a rule, or change the rule in logos.workspace.toml if the call is intended.",
      };
    }
    return noAction;
  },
};

// ── Figure-row and absence sentences ─────────────────────────────────────────

export const HEALTH_TEXT = {
  /** The rules figure (CR-203 item 5): "r rules checked over b bindings · v findings". */
  rulesChecked: (rules: number, bindings: number, findings: number) =>
    `${rules} ${plural(rules, "rule", "rules")} checked over ${bindings} ${plural(bindings, "binding", "bindings")} · ${findings} ${plural(findings, "finding", "findings")}`,
  /** No rules declared: nothing was checked, which is not a pass (ADR-56). */
  noRules: "No workspace rules are declared, so nothing was checked — this is not a pass.",
  /** A clean report over zero bindings says nothing. */
  nothingToCheck: "Nothing was bound to check, so a clean result here says nothing about this workspace.",
  /** Rule references naming a member the workspace does not have. */
  unknownMembers: (n: number) =>
    `${n} rule ${plural(n, "reference names", "references name")} a member this workspace does not have, so ${plural(n, "that rule was", "those rules were")} silently narrowed and can never match:`,
  /** The answer missed members that could not be opened. */
  incomplete: (n: number) =>
    `This answer is incomplete: ${n} ${plural(n, "member", "members")} could not be opened, so a rule quantified over their bindings was quantified over fewer than all of them:`,
} as const;
