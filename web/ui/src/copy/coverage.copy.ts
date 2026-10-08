/*
 * The cross-service coverage catalogue (S-613, CR-203 §3.2 D items 4 and 10,
 * FR-UI-39). ONE catalogue for the boards `CoveragePanel` renders on both the
 * Workspace Dashboard and the Workspace tab's Cross-service coverage tab, and for
 * the tab's two relation widgets — so a board two views share cannot say one
 * thing twice.
 *
 * Every figure the sentences carry is the server's (NFR-MA-02): a statement
 * below formats the numbers it is handed and computes none. The entries follow
 * the S-611 shape; `COVERAGE_TEXT` holds the figure-row and absence sentences,
 * so a wording change edits this file alone and the view tests name a key, not
 * the prose.
 */

import type { UnboundReason } from "../api/types.ts";

import { gloss, plural, type CopyEntry, type CopyText, type WhereKind } from "./types.ts";

// ── Resolved cross-service edges (item 4) ────────────────────────────────────

/** A remedy a Binding evidence row's action takes (S-614): the clause and the
 *  kind of place it happens in. */
export interface Remedy {
  /** The remedy, as a clause that follows the reason ("define the named key…"). */
  readonly remedy: string;
  /** The kind of place the remedy happens in. */
  readonly where: WhereKind;
}

/** The remedies for the two not-bound reasons a repository can fix in its
 *  configuration, which the Binding evidence rows' own actions take
 *  (`evidenceRowAction`). Only these two: the Resolved cross-service edges
 *  widget states its reasons as evidence, with no remedy (CR-206). */
export const NOT_RESOLVED_REMEDY: Readonly<Record<Extract<UnboundReason, "config-key-missing" | "config-placeholder-value">, Remedy>> = {
  "config-key-missing": {
    remedy: "define the named key in a committed configuration source",
    where: "configuration",
  },
  "config-placeholder-value": {
    remedy: "replace the placeholder with the real committed value",
    where: "configuration",
  },
};

/** One row of the not-resolved evidence table: a reason, its words and its count. */
export interface NotResolvedReason {
  readonly reason: string;
  readonly label: string;
  readonly count: number;
}

/** What the Resolved cross-service edges widget renders from (S-613): its figure
 *  and the not-resolved evidence table. */
export interface ResolvedEdgesState {
  /** The captured outbound call sites the rate is over; `0` is nothing measured. */
  readonly measured: number;
  /** Of those, the ones that did not resolve. */
  readonly unresolved: number;
  /** Why, largest first. The counts sum to `unresolved`. */
  readonly reasons: readonly NotResolvedReason[];
  /** Captured call sites calling a service outside this workspace — outside the
   *  rate, so never in the sum above (ADR-53). */
  readonly outside: number;
}

export const resolvedEdges: CopyEntry = {
  what: "How many of the outbound calls captured in this workspace's code reach a service in the workspace, and how many cross-service edges they resolve to.",
  why: "The service map and impact analysis see only resolved calls, so every unresolved call is a coupling they miss.",
};

// ── Spec conformance (item 10) ───────────────────────────────────────────────

export const specConformance: CopyEntry = {
  what: "How many cross-service references — endpoints declared in API documents and calls captured in code — match exactly one provider in this workspace, of those that could match here.",
  why: "An endpoint that matches no controller is an API document that has drifted from the code; the figure is mostly declared endpoints, so it is not a measure of coupling.",
};

// ── Coverage by intake (item 10) ─────────────────────────────────────────────

/** What the captured-call half of the split says — four different statements
 *  from the same zero (NFR-CC-04). */
export type IntakeFinding =
  /** Captured call sites exist inside the rate and none of them resolves. */
  | "captured-unresolved"
  /** Every captured call site calls a service outside this workspace. */
  | "captured-outside"
  /** No call site was captured at all. */
  | "captured-absent"
  /** Some captured call sites resolve. */
  | "captured-resolves";

export const coverageByIntake: CopyEntry = {
  what: [
    "The same references split by ",
    gloss("intake"),
    ": endpoints declared in API documents, and calls captured in code.",
  ],
  why: "A healthy count of declared endpoints can hide outbound calls that resolve nowhere; the split shows which of the two carries the figure.",
};

// ── Coverage by relation arm (item 1: hidden, S-612) ─────────────────────────

/**
 * The per-arm board is hidden through the hidden-widget register (S-612); it
 * carries its entry so removing the register entry brings it back explained
 * (S-617). Resolved cross-service edges aggregates its not-bound reasons across
 * every arm (item 1).
 */
export const coverageByArm: CopyEntry = {
  what: [
    "The same references split by ",
    gloss("arm", "relation arm"),
    " — HTTP, gRPC and broker — each with how many resolved, are ambiguous or did not resolve, and why.",
  ],
  why: "It shows which kind of cross-service call the service map sees least of.",
};

// ── Declared contracts and named externals (item 10, the coverage tab) ───────

export const declaredRelations: CopyEntry = {
  what: "Contracts members declare by vendoring another service's API document, and the named external services those documents describe.",
  why: "A declared contract is a dependency someone wrote down rather than one observed in code, so it shows which services expect each other even where no call was captured.",
};

// ── Build dependencies (item 10, the coverage tab) ───────────────────────────

export const buildDependencies: CopyEntry = {
  what: "What each member builds against, read from its Maven or Gradle manifests.",
  why: "A build dependency is never a runtime call, so it is counted apart from every figure above; it shows which services share a library or a parent build.",
};

// ── Figure-row and absence sentences ─────────────────────────────────────────

export const COVERAGE_TEXT = {
  /** The headline figure: resolved sites of captured sites. */
  outboundResolved: (resolved: number, measured: number) =>
    `${resolved} of ${measured} outbound call ${plural(measured, "site", "sites")} resolved`,
  /** Leads the server's composed edge line, glossing the word that line uses
   *  ("egress") before the reader meets it there (FR-UI-39 first use). */
  edgeLineLead: ["Edges and ", gloss("egress"), " rate, as the server reports them:"] as CopyText,
  /** Leads the server's composed named-external line, which ends "outside
   *  egress_resolution": the word is glossed before the reader meets it there
   *  (FR-UI-39 first use), as `edgeLineLead` does for the edge line. */
  externalLineLead: ["Calls matched to named externals, outside the ", gloss("egress"), " rate, as the server reports them:"] as CopyText,
  /** No outbound call site was captured, so the rate has no denominator. */
  outboundNotMeasured:
    "Not measured: no outbound call site was captured in this workspace, so the rate has no denominator.",
  /** Outbound calls WERE captured, and every one calls a service outside the
   *  workspace — outside the rate (ADR-53), so it has no denominator either. */
  outboundAllOutside: (outside: number) =>
    `Not measured: every captured outbound call (${outside}) calls a service outside this workspace, so the rate has no denominator. Add those services as members in logos.workspace.toml, or vendor their specs, if they belong in this picture.`,
  /** No cross-boundary reference at all — qualified when members were not read. */
  nothingFound: (coversAllMembers: boolean, read: number, total: number) =>
    coversAllMembers
      ? "Nothing measured: no cross-boundary references found in this workspace, so no coverage is reported (never a fabricated 100%)."
      : `Nothing measured: no cross-boundary references found among the ${read} of ${total} workspace members that could be read — this is NOT a statement about the whole workspace.`,
  /** The coverage shortfall (FR-WS-16): computed over fewer than all members. */
  shortfall: (read: number, total: number) =>
    `Partial: computed over ${read} of ${total} workspace members — the rest did not contribute (their store could not be opened, or their contract surface could not be read), so every figure here is a minimum: the true count may be higher.`,
  /** The members that could not be opened, named after this prefix. */
  degradedPrefix: (n: number) => `${n} ${plural(n, "member", "members")} could not be opened:`,
  /** The spec-conformance figure. */
  specMatched: (matched: number, measured: number) =>
    `${matched} of ${measured} ${plural(measured, "reference", "references")} matched exactly one provider`,
  /** The four buckets behind the spec-conformance figure. */
  specBreakdown: (matched: number, ambiguous: number, unmatched: number, outside: number) =>
    `${matched} matched · ${ambiguous} ambiguous · ${unmatched} unmatched · ${outside} ${plural(outside, "calls", "call")} a service outside this workspace (reported apart, and left out of the ratio: a call to a service outside this workspace is not a broken link).`,
  /** An absent ratio over references that all leave the workspace. */
  specNotMeasured: (outside: number) =>
    `Not measured: ${outside === 1 ? "the 1 cross-boundary reference calls" : `all ${outside} cross-boundary references call`} a service outside this workspace, so the ratio has no denominator.`,
  /** The intake findings, one per state of the captured-call half. */
  capturedUnresolved: (measured: number) =>
    `No captured call site in this workspace resolves: ${measured === 1 ? "the 1 captured call" : `every one of the ${measured} captured calls`} that could match here is ambiguous or unmatched. The matched count is entirely declared endpoints.`,
  capturedOutside: (outside: number) =>
    `Every captured call in this workspace (${outside}) calls a service outside it — reported apart, and not a broken link. Nothing here failed to resolve.`,
  capturedAbsent:
    "No calls were captured in this workspace — honest absence, not a resolution failure. The matched count says nothing about outbound call sites either way.",
  capturedResolves: (resolved: number, measured: number) =>
    `${resolved} of ${measured} captured call ${plural(measured, "site", "sites")} ${plural(resolved, "resolves", "resolve")}.`,
  /** The declared relation's figure, in plain words from its headline's counts
   *  (the server's composed line, which uses wire tokens, is the evidence). */
  declaredPairs: (pairs: number, externals: number) =>
    `${pairs} declared contract ${plural(pairs, "pair", "pairs")} · ${externals} named ${plural(externals, "external", "externals")}`,
  /** The external join's figure, likewise. */
  externalsMatched: (matched: number, rows: number) =>
    `${matched} of ${rows} outbound REST ${plural(rows, "call", "calls")} to a service outside this workspace matched a named external`,
  /** The caption of Resolved cross-service edges' not-resolved evidence table:
   *  the reasons, largest first, summing to `unresolved`. */
  notResolvedCaption: (unresolved: number) =>
    `Why ${unresolved} outbound call ${plural(unresolved, "site", "sites")} did not resolve, largest reason first`,
  /** The label of unresolved call sites the answer counts but does not itemise. */
  notItemised: "Not itemised in this answer",
  /** The build relation is absent: no member holds a build manifest. */
  buildAbsent: "No member holds a Maven or Gradle build manifest, so no build dependency was read.",
  /** A bound external call stays in the outside-the-workspace count. */
  externalStaysApart:
    "A call matched to a named external still counts under the calls to a service outside this workspace; it is reported beside them, not added to the matched count.",
} as const;
