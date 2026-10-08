/*
 * The Workspace Dashboard catalogue (S-613, CR-203 §3.2 D items 2 and 3,
 * FR-UI-39): Cross-service reachability and Members. The coverage boards the
 * dashboard also renders speak from `coverage.copy.ts`, the catalogue the
 * Workspace tab shares, so the two views cannot word one board two ways.
 *
 * Every figure the sentences carry is the server's (NFR-MA-02); `DASHBOARD_TEXT`
 * formats the numbers it is handed and computes none.
 */

import { noRowAction, plural, type CopyEntry, type RowAction } from "./types.ts";

// ── Cross-service reachability (item 2) ──────────────────────────────────────

export const reachability: CopyEntry = {
  what: "Callables that nothing in their own service calls, but that another service in this workspace does.",
  why: "Their own repository's dead-code report lists them, yet deleting one breaks the service that calls it.",
};

// ── Members (item 3) ─────────────────────────────────────────────────────────

export const members: CopyEntry = {
  what: "Each service's own figures, one row per service: how much of its code links up, and how many of its callables nothing calls.",
  why: "A service that could not be opened adds nothing to any figure on this page, and a callable nothing in the workspace calls is a safe candidate for deletion.",
};

/** One member row's own action (the Members table's last column). */
export function memberRowAction(row: { degraded: boolean; unusedAcross: number | null }): RowAction {
  if (row.degraded) {
    return { kind: "act", where: "command", target: "logos index", text: "Run logos index in this member." };
  }
  if (row.unusedAcross !== null && row.unusedAcross > 0) {
    return {
      kind: "act",
      where: "source code",
      text: `Review ${row.unusedAcross} ${plural(row.unusedAcross, "callable", "callables")} for deletion.`,
    };
  }
  return noRowAction;
}

// ── Figure-row and absence sentences ─────────────────────────────────────────

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
  /** BR-56, as one plain sentence (CR-203 item 3). */
  notAveraged: "Figures are per service; they are not averaged.",
} as const;
