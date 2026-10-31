/*
 * The Workspace Dashboard catalogue (S-613, CR-203 §3.2 D items 2 and 3,
 * FR-UI-39): Cross-service reachability and Members. The coverage boards the
 * dashboard also renders speak from `coverage.copy.ts`, the catalogue the
 * Workspace tab shares, so the two views cannot word one board two ways.
 *
 * Every figure the sentences carry is the server's (NFR-MA-02); `DASHBOARD_TEXT`
 * formats the numbers it is handed and computes none.
 */

import { plural, type CopyEntry } from "./types.ts";

/** The counts a reference-resolution sentence states (S-637) — the shape of the
 *  read-model's `InternalResolution`, stated here so the catalogue formats what
 *  it is handed and imports no read-model. */
interface ResolutionCounts {
  resolved: number;
  internal_candidates: number;
  provably_external: number;
  code_references: number;
}

// ── Security (S-790) ─────────────────────────────────────────────────────────

export const workspaceSecurityWidget: CopyEntry = {
  what: "Each member's own security findings that count — fresh, not suppressed and in production code — by severity, and whether its scan is fresh, stale or has never run, linked to that member's Security page.",
  why: "A member nothing has scanned has looked at nothing, so it reads never run rather than a clean zero; a member that did not open is a row with its reason, not a row of zeros.",
};

// ── Cross-service reachability (item 2) ──────────────────────────────────────

export const reachability: CopyEntry = {
  what: "Callables that nothing in their own service calls, but that another service in this workspace does.",
  why: "Their own repository's dead-code report lists them, yet deleting one breaks the service that calls it.",
};

// ── Reference resolution across the workspace (S-637) ────────────────────────

export const workspaceResolution: CopyEntry = {
  what: "How much of the code that could link up across every service does: all services' references pooled, where a call into another service's own code counts as one that could link.",
  why: "It can only undercount: a method call on a value of unknown type stays unlinked even when its target is in the workspace, and a service that did not open adds nothing to it.",
};

// ── Members (item 3) ─────────────────────────────────────────────────────────

export const members: CopyEntry = {
  what: "Each service's own figures, one row per service: how much of its code links up, and how many of its callables nothing calls.",
  why: "A service that could not be opened adds nothing to any figure on this page, and a callable nothing in the workspace calls is a safe candidate for deletion.",
};

// ── Figure-row and absence sentences ─────────────────────────────────────────

/** Why a reference-resolution figure states no ratio, read from its own counts
 *  (S-637): no code reference at all, or every one provably external. */
function resolutionAbsent(codeReferences: number, provablyExternal: number): string {
  return codeReferences === 0
    ? "no code reference recorded"
    : `all ${provablyExternal} code ${plural(provablyExternal, "reference", "references")} provably external`;
}

export const DASHBOARD_TEXT = {
  /** The reachability lead (CR-203 item 2): "at least" when coverage is partial. */
  keepThem: (n: number, partial: boolean) =>
    n === 0
      ? `No callable is unused in its own service but called from another${partial ? " among the members read" : ""}.`
      : `${partial ? "At least " : ""}${n} ${plural(n, "callable", "callables")} unused in ${plural(n, "its", "their")} own service but called from another — keep ${plural(n, "it", "them")}.`,
  /** What the reachability figure rests on. */
  keepThemBasis: (edges: number, read: number, total: number, partial: boolean) =>
    `Found from ${edges} cross-service call ${plural(edges, "edge", "edges")}; ${read} of ${total} members read${partial ? " — every figure here is a minimum" : ""}.`,
  /** An empty answer is only as good as the calls it was found from. */
  keepThemNone:
    "This rests on the resolved calls above: read their coverage before taking it as an all-clear.",
  /** The asserted promotions' lead (S-709): kept in use only by a declared
   *  `[[links]]` call — a claim, stated apart from the measured promotions. */
  keptByLinks: (n: number, links: number) =>
    `${n} ${plural(n, "more callable is", "more callables are")} unused in ${plural(n, "its", "their")} own service and kept in use only by a declared link (${links} asserted call ${plural(links, "link", "links")}) — asserted, not measured.`,
  /** The legend row for the asserted promotions. */
  assertedLegend: "Asserted — declared in the workspace manifest's [[links]], never observed in code",
  /** Members the reachability answer could not read. */
  skipped: (members: readonly string[]) =>
    `${members.length} ${plural(members.length, "member", "members")} could not be read, so ${plural(members.length, "it contributes", "they contribute")} no callables to this answer:`,
  /** The bounds the answer was taken under. */
  scopedTo: "Scoped to",
  unscoped: "Unscoped — every member is projected.",
  deadSuppressed:
    "The full list of callables nothing in the workspace calls was not requested, so it is absent from this answer rather than empty.",
  deadCount: (n: number) =>
    `${n} ${plural(n, "callable is", "callables are")} unused across every service of the workspace.`,
  /** The Members figure. */
  membersRead: (opened: number, total: number) => `${opened} of ${total} members read`,
  /** The Security figure: how many members have a fresh scan out of all of them
   *  (S-790) — a count of members, never a count of findings across them. */
  securityFresh: (fresh: number, total: number) =>
    `${fresh} of ${total} ${plural(total, "member has", "members have")} a fresh scan`,
  /** Members the Security read could not read: they are rows, not omissions. */
  securityUnread: (n: number) =>
    `${n} ${plural(n, "member", "members")} could not be read, so ${plural(n, "it has", "they have")} no count:`,
  /** A member never scanned has no count, and the command that resolves it is
   *  named (FR-UI-39, NFR-CC-04): shown when any row reads never run. */
  securityNeverRunNote:
    "A member nothing has scanned reads never run, which is not a count of zero; run logos security scan in that member, or open its Security page.",
  /** The scan state in words, so the row is never colour alone. */
  securityScan: (scan: "fresh" | "stale" | "never_run") =>
    scan === "never_run" ? "never run" : scan,
  /** BR-56 for the Security widget: per member, never rolled up. */
  securityPerMember: "Counts are per member; they are not summed or averaged across members.",
  /** Why a reference-resolution figure states no ratio (S-637). */
  resolutionAbsent: (r: ResolutionCounts) => resolutionAbsent(r.code_references, r.provably_external),
  /** A member's own figure in the Members column, never bare: its numerator,
   *  its denominator and the population it was taken over ride with it
   *  (CR-111, FR-RS-49). */
  ownResolution: (percent: string, r: ResolutionCounts) =>
    `${percent} (${r.resolved.toLocaleString("en-US")} of ${r.internal_candidates.toLocaleString("en-US")} internal candidates; ${r.code_references.toLocaleString("en-US")} code references, ${r.provably_external.toLocaleString("en-US")} provably external)`,
  /** The workspace figure's qualifier (S-637). */
  workspaceResolutionUnit: "at least, of the internal candidates resolved",
  /** The workspace figure stated absent (NFR-CC-04), never 0% or 100%. */
  workspaceResolutionAbsent: (r: ResolutionCounts) =>
    `No figure: ${resolutionAbsent(r.code_references, r.provably_external)}.`,
  /** The population line the figure is never shown without (FR-RS-49). */
  workspacePopulation: (r: ResolutionCounts) =>
    `${r.internal_candidates.toLocaleString("en-US")} internal candidates of ${r.code_references.toLocaleString("en-US")} code references (${r.provably_external.toLocaleString("en-US")} provably external).`,
  /** What the workspace counted that no member could (S-637). */
  workspaceCrossMember: (byName: number, byType: number, resolved: number) =>
    `${byName} ${plural(byName, "reference counts", "references count")} as internal because another service declares ${plural(byName, "its name", "their names")}, and ${byType} because another service declares ${plural(byType, "its type", "their types")}; ${resolved} ${plural(resolved, "call is", "calls are")} resolved by a cross-service call into another service.`,
  /** The member denominator, and BR-56's rule for this figure. */
  workspacePooled: (read: number, total: number) =>
    `Pooled over ${read} of ${total} members — their counts summed, never their figures averaged.`,
  /** Members the pooled figure could not read: named, never dropped silently. */
  workspaceUnread: (n: number) =>
    `${n} ${plural(n, "member is", "members are")} not summed, because ${plural(n, "its", "their")} references were not read:`,
  /** BR-56, as one plain sentence (CR-203 item 3). */
  notAveraged: "Figures are per service; they are not averaged.",
} as const;
