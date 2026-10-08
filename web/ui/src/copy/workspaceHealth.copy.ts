/*
 * The Workspace Health catalogue (S-613, CR-203 §3.2 D item 5, FR-UI-39).
 * S-613 converted Workspace rules; S-617 the view's other four widgets —
 * Members answering, Members, Warm state and Broker topics.
 *
 * Every figure the sentences carry is the server's (NFR-MA-02); `HEALTH_TEXT`
 * formats the numbers it is handed and computes none. The no-rules absence names
 * where rules are declared (CR-206).
 */

import { plural, type CopyEntry } from "./types.ts";

// ── Workspace rules (item 5) ─────────────────────────────────────────────────

export const workspaceRules: CopyEntry = {
  what: "The workspace rules declared in logos.workspace.toml, checked against every cross-service binding this workspace resolved, with each finding listed.",
  why: "A finding is a call your declared architecture forbids; the check is advisory, so it moves no exit code and no member's quality signal.",
};

// ── Members answering (S-617) ────────────────────────────────────────────────

export const membersAnswering: CopyEntry = {
  what: "How many of the workspace's members answered this read, and which could not be opened.",
  why: "A member that does not answer is missing from every other figure on this page, so each of them counts less than the whole workspace.",
};

// ── Members (S-617) ──────────────────────────────────────────────────────────

export const memberFreshness: CopyEntry = {
  what: "Each member's index age, whether it is indexed, whether its store opened, and its own share of resolved references.",
  why: "A member with an old index answers from an old picture of its code, and one that did not open answers nothing.",
};

// ── Warm state (S-617) ───────────────────────────────────────────────────────

export const warmState: CopyEntry = {
  what: "How many members are indexed (warm), will index when first asked (deferred), or failed to index (degraded).",
  why: "A deferred member answers its first question slowly, while it indexes; a degraded one answers nothing.",
};

// ── Broker topics (S-617) ────────────────────────────────────────────────────

export const brokerTopics: CopyEntry = {
  what: "The message-broker topics each member publishes or subscribes to, with how many places in its code do each.",
  why: "A topic couples the services that use it even before any other service subscribes, so it is part of how the workspace fits together.",
};

// ── Figure-row and absence sentences ─────────────────────────────────────────

export const HEALTH_TEXT = {
  /** The rules figure (CR-203 item 5): "r rules checked over b bindings · v findings". */
  rulesChecked: (rules: number, bindings: number, findings: number) =>
    `${rules} ${plural(rules, "rule", "rules")} checked over ${bindings} ${plural(bindings, "binding", "bindings")} · ${findings} ${plural(findings, "finding", "findings")}`,
  /** No rules declared: nothing was checked, which is not a pass (ADR-56). */
  noRules:
    "No workspace rules are declared, so nothing was checked — this is not a pass; declare [[governance.boundaries]] in logos.workspace.toml to have cross-service calls checked.",
  /** A clean report over zero bindings says nothing. */
  nothingToCheck: "Nothing was bound to check, so a clean result here says nothing about this workspace.",
  /** Rule references naming a member the workspace does not have. */
  unknownMembers: (n: number) =>
    `${n} rule ${plural(n, "reference names", "references name")} a member this workspace does not have, so ${plural(n, "that rule was", "those rules were")} silently narrowed and can never match:`,
  /** Members answering (S-617): "2 of 3 members answered". */
  answered: (opened: number, members: number) => `${opened} of ${members} ${plural(members, "member", "members")} answered`,
  /** Warm state (S-617): "2 of 3 members are warm". */
  warm: (warm: number, members: number) => `${warm} of ${members} ${plural(members, "member is", "members are")} warm`,
  /** Members (S-617): "1 of 3 members degraded" or none. */
  degraded: (degraded: number, members: number) =>
    `${degraded} of ${members} ${plural(members, "member", "members")} degraded`,
  /** Members (S-617): every figure is that member's own (BR-56). */
  perMember: "Figures are per member; they are not averaged.",
  /** Broker topics (S-617): the topics and the members that declare them. */
  topics: (topics: number, members: number) =>
    `${topics} ${plural(topics, "topic", "topics")} across ${members} ${plural(members, "member", "members")}`,
  /** Broker topics (S-617): an honest absence, not a resolution failure. */
  noTopics:
    "No member publishes or subscribes to a broker topic. A topic appears here as soon as one member uses it, before any other service does.",
  /** The answer missed members that could not be opened. */
  incomplete: (n: number) =>
    `This answer is incomplete: ${n} ${plural(n, "member", "members")} could not be opened, so a rule quantified over their bindings was quantified over fewer than all of them:`,
} as const;
