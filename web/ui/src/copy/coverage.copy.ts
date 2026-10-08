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

import { gloss, noAction, type CopyEntry, type CopyText, type WhereKind } from "./types.ts";

// ── Resolved cross-service edges (item 4) ────────────────────────────────────

/** What to do about one not-resolved reason: CR-203's remedy table. */
export interface Remedy {
  /** The remedy, as a clause that follows the reason ("commit the base URL…"). */
  readonly remedy: string;
  /** The kind of place the remedy happens in. */
  readonly where: WhereKind;
  /** The place in words when the table names two kinds ("source code or configuration"). */
  readonly whereText?: string;
}

/** One remedy per reason token the server sends (CR-203 §3.2, "Remedies for
 *  not-bound reasons"). Keyed by the closed union, so a reason added to
 *  `UnboundReason` without a remedy here is a `tsc -b` error. */
export const NOT_RESOLVED_REMEDY: Record<UnboundReason, Remedy> = {
  "base-url-runtime": {
    remedy: "commit the base URL, or a default, in the member's application configuration",
    where: "configuration",
  },
  "path-not-composed": {
    remedy: "build the path from literals or from constants of the same member",
    where: "source code",
  },
  "config-key-missing": {
    remedy: "define the named key in a committed configuration source",
    where: "configuration",
  },
  "config-placeholder-value": {
    remedy: "replace the placeholder with the real committed value",
    where: "configuration",
  },
  "topic-not-literal": {
    remedy: "name the topic by a literal or by a committed configuration value",
    where: "source code",
    whereText: "source code or configuration",
  },
  ambiguous: {
    remedy: "two services serve the same endpoint, so make one path or method distinct, or remove the duplicate",
    where: "source code",
  },
  "no-provider-in-workspace": {
    remedy:
      "add the called service as a member in logos.workspace.toml, or vendor its spec to name it; otherwise there is nothing to do",
    where: "configuration",
    whereText: "configuration or documentation",
  },
};

/** The remedy for a reason this build does not know (the wire union is OPEN),
 *  and for sites the answer counts but does not itemise. */
export const UNLISTED_REMEDY: Remedy = {
  remedy: "list these call sites with logos workspace status and read each one's reason",
  where: "command",
};

/** The remedy for one reason token, known or not. */
export function remedyFor(reason: string): Remedy {
  return Object.hasOwn(NOT_RESOLVED_REMEDY, reason)
    ? NOT_RESOLVED_REMEDY[reason as UnboundReason]
    : UNLISTED_REMEDY;
}

/** One line of the not-resolved list: a reason, its words and its count. */
export interface NotResolvedReason {
  readonly reason: string;
  readonly label: string;
  readonly count: number;
}

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

export const resolvedEdges: CopyEntry<ResolvedEdgesState> = {
  what: "How many of the outbound calls captured in this workspace's code reach a service in the workspace, and how many cross-service edges they resolve to.",
  why: "The service map and impact analysis see only resolved calls, so every unresolved call is a coupling they miss.",
  action: ({ unresolved, reasons, outside }) => {
    if (unresolved === 0 || reasons.length === 0) return noAction;
    const items = reasons.map((r) => {
      const m = remedyFor(r.reason);
      return `${r.count} × ${r.label}: ${m.remedy} (${m.whereText ?? m.where})`;
    });
    const apart =
      outside === 0
        ? ""
        : ` A further ${outside} call ${outside === 1 ? "site calls" : "sites call"} a service outside this workspace and ${outside === 1 ? "is" : "are"} not counted above: ${NOT_RESOLVED_REMEDY["no-provider-in-workspace"].remedy}.`;
    return {
      kind: "act",
      where: remedyFor(reasons[0].reason).where,
      text: `Fix why ${unresolved} call ${unresolved === 1 ? "site" : "sites"} did not resolve, largest reason first — ${items.join("; ")}.${apart}`,
    };
  },
};

// ── Spec conformance (item 10) ───────────────────────────────────────────────

export interface SpecConformanceState {
  /** References that matched no provider or several — ambiguous plus unmatched. */
  readonly notMatched: number;
}

export const specConformance: CopyEntry<SpecConformanceState> = {
  what: "How many cross-service references — endpoints declared in API documents and calls captured in code — match exactly one provider in this workspace, of those that could match here.",
  why: "An endpoint that matches no controller is an API document that has drifted from the code; the figure is mostly declared endpoints, so it is not a measure of coupling.",
  action: ({ notMatched }) =>
    notMatched === 0
      ? noAction
      : {
          kind: "act",
          where: "command",
          target: "logos workspace status",
          text: "List the references that did not match, then align each API document with its controller's path and method, or remove the stale operation.",
        },
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

export const coverageByIntake: CopyEntry<{ finding: IntakeFinding }> = {
  what: [
    "The same references split by ",
    gloss("intake"),
    ": endpoints declared in API documents, and calls captured in code.",
  ],
  why: "A healthy count of declared endpoints can hide outbound calls that resolve nowhere; the split shows which of the two carries the figure.",
  action: ({ finding }) =>
    finding === "captured-unresolved"
      ? {
          kind: "act",
          where: "command",
          target: "logos workspace status",
          text: "List the captured call sites that did not resolve, and fix each by the reason given under Resolved cross-service edges.",
        }
      : noAction,
};

// ── Declared contracts and named externals (item 10, the coverage tab) ───────

export const declaredRelations: CopyEntry = {
  what: "Contracts members declare by vendoring another service's API document, and the named external services those documents describe.",
  why: "A declared contract is a dependency someone wrote down rather than one observed in code, so it shows which services expect each other even where no call was captured.",
  action: () => noAction,
};

// ── Build dependencies (item 10, the coverage tab) ───────────────────────────

export const buildDependencies: CopyEntry<{ unread: number }> = {
  what: "What each member builds against, read from its Maven or Gradle manifests.",
  why: "A build dependency is never a runtime call, so it is counted apart from every figure above; it shows which services share a library or a parent build.",
  action: ({ unread }) =>
    unread === 0
      ? noAction
      : {
          kind: "act",
          where: "command",
          target: "logos index",
          text: "Run logos index in each member whose build facts were not read, so they are read on the next answer.",
        },
};

// ── Figure-row and absence sentences ─────────────────────────────────────────

const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

export const COVERAGE_TEXT = {
  /** The headline figure: resolved sites of captured sites. */
  outboundResolved: (resolved: number, measured: number) =>
    `${resolved} of ${measured} outbound call ${plural(measured, "site", "sites")} resolved`,
  /** Leads the server's composed edge line, glossing the word that line uses
   *  ("egress") before the reader meets it there (FR-UI-39 first use). */
  edgeLineLead: ["Edges and ", gloss("egress"), " rate, as the server reports them:"] as CopyText,
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
    `${matched} matched · ${ambiguous} ambiguous · ${unmatched} unmatched · ${outside} call a service outside this workspace (reported apart, and left out of the ratio: a call to a service outside this workspace is not a broken link).`,
  /** An absent ratio over references that all leave the workspace. */
  specNotMeasured: (outside: number) =>
    `Not measured: all ${outside} cross-boundary references call a service outside this workspace, so the ratio has no denominator.`,
  /** The intake findings, one per state of the captured-call half. */
  capturedUnresolved: (measured: number) =>
    `No captured call site in this workspace resolves: every one of the ${measured} captured calls that could match here is ambiguous or unmatched. The matched count is entirely declared endpoints.`,
  capturedOutside: (outside: number) =>
    `Every captured call in this workspace (${outside}) calls a service outside it — reported apart, and not a broken link. Nothing here failed to resolve.`,
  capturedAbsent:
    "No calls were captured in this workspace — honest absence, not a resolution failure. The matched count says nothing about outbound call sites either way.",
  capturedResolves: (resolved: number, measured: number) =>
    `${resolved} of ${measured} captured call ${plural(measured, "site", "sites")} resolve.`,
  /** The declared relation's figure, in plain words from its headline's counts
   *  (the server's composed line, which uses wire tokens, is the evidence). */
  declaredPairs: (pairs: number, externals: number) =>
    `${pairs} declared contract ${plural(pairs, "pair", "pairs")} · ${externals} named ${plural(externals, "external", "externals")}`,
  /** The external join's figure, likewise. */
  externalsMatched: (matched: number, rows: number) =>
    `${matched} of ${rows} outbound REST ${plural(rows, "call", "calls")} to a service outside this workspace matched a named external`,
  /** The build relation is absent: no member holds a build manifest. */
  buildAbsent: "No member holds a Maven or Gradle build manifest, so no build dependency was read.",
  /** A bound external call stays in the outside-the-workspace count. */
  externalStaysApart:
    "A call matched to a named external still counts under the calls to a service outside this workspace; it is reported beside them, not added to the matched count.",
} as const;
