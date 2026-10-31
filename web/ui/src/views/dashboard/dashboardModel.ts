/*
 * Pure Dashboard model (S-187, FR-UI-09) — the presentation logic ported from the
 * server-rendered Dashboard (web/src/views/overview.rs) into framework-free,
 * unit-testable functions: the BR-34 advisory quality bands, the freshness
 * statement and the basis-point → percent reprojection. No DOM, no React — every
 * figure is a projection of a read-model field, inventing nothing (NFR-RA-05).
 *
 * Absence wording here follows the one taxonomy rather than restating it:
 * `models::quality::absence` in `logos-core/src/models/quality.rs` (S-434) —
 * the closed sentinel vocabulary and the rules (R0-R5) every absence-
 * reporting site keeps. Enumerated from source by
 * `logos-core/tests/absence_taxonomy_audit.rs`.
 */

import type {
  InternalResolution,
  OverviewSecurity,
  SecurityFindingTally,
  SecurityGateBand,
  SecuritySeverityCounts,
  SecuritySummary,
  StatusInfo,
} from "../../api/types.ts";
import type { ScoreBarTone } from "../../components/index.ts";
import { plural } from "../../copy/types.ts";
import { DASHBOARD_TEXT } from "../../copy/workspaceDashboard.copy.ts";

// ── BR-34 advisory quality bands (frontend-design §4.1) ──────────────────────

/** A BR-34 quality band: its human label and the score-bar tint tone. */
export interface Band {
  label: string;
  tone: ScoreBarTone;
}

/**
 * The BR-34 band a raw 0–10000 quality signal falls into. They tint **only** the
 * quality score bar — coverage/test bars stay green and raw (BR-28).
 *   `< 5000` Poor (red) · `5000–6999` Average (orange) · `7000–8499` Good (lime) ·
 *   `>= 8500` Excellent (green).
 */
export function bandOf(signal: number): Band {
  if (signal < 5_000) return { label: "Poor", tone: "poor" };
  if (signal < 7_000) return { label: "Average", tone: "average" };
  if (signal < 8_500) return { label: "Good", tone: "good" };
  return { label: "Excellent", tone: "excellent" };
}

// ── Integer grouping (display-only) ──────────────────────────────────────────

/** Group an integer figure's digits for display (e.g. `54321` → `54,321`). The
 *  read-model counts are plain integers; this is a presentation projection only,
 *  matching the Statistics tab's `num` formatter so the two tabs read alike. */
export function fmtInt(n: number): string {
  return n.toLocaleString("en-US");
}

// ── Basis-point reprojection (web/src/views/mod.rs::pct_bp) ───────────────────

/** Reproject basis points (0–10000) to a one-decimal percent string, clamped. */
export function pctBp(bp: number): string {
  const clamped = Math.min(10_000, Math.max(0, bp));
  return `${(clamped / 100).toFixed(1)}%`;
}

// ── Freshness statement (frontend-design §4.1 verdict element) ───────────────

/** Parse an optional unix-seconds string field into a number of seconds.
 *  Exported because `healthModel`'s snapshot/index comparison reads the same two
 *  `StatusInfo` fields and must read them the same way (S-436). */
export function parseSecs(field: string | null): number | null {
  if (field === null) return null;
  // Only an all-digits string is a valid unix-seconds field (parseInt is too lax).
  if (!/^\d+$/.test(field)) return null;
  return Number(field);
}

/**
 * A century. Past this the recorded timestamp is wrong rather than merely old:
 * no index or sync of a project predates the tool by decades. Mirrors
 * `IMPLAUSIBLE_AGE_SECS` in `logos-core/src/governance/readout.rs`.
 */
const IMPLAUSIBLE_AGE_SECS = 100 * 365 * 24 * 60 * 60;

/**
 * The two degradations, as the exact phrases every surface renders for them.
 *
 * Named constants rather than literals because a second surface now reuses this
 * vocabulary instead of inventing a parallel one for the same condition (S-436,
 * `healthModel::snapshotStaleness`): one product says one thing about one
 * condition, and a future rewording changes both surfaces or neither.
 */
export const UNKNOWN_AGE_AHEAD_OF_NOW = "at an unknown age (recorded ahead of now — check the clock)";
export const UNKNOWN_AGE_IMPLAUSIBLY_OLD =
  "at an unknown age (the recorded time is implausibly old — check the store)";

/**
 * Humanise the age between `now` and `then` (both unix seconds) into a coarse
 * relative phrase — `just now`, `5m ago`, `6d ago`.
 *
 * An age that cannot be established is **named, not rendered** (FR-EH-04,
 * NFR-RA-05). The two cases are the ones the CLI readout already names, in its
 * words (S-314, `37909c9c`), so one product says one thing about one condition:
 * - **Ahead of now** — `then` is stamped after `now`. `status.last_sync_at` is a
 *   file mtime, and a future mtime is routine on a copied tree, an NFS mount or
 *   a restored backup; clamping the interval to zero and saying "just now"
 *   would invent the most reassuring reading of a fact the SPA cannot
 *   establish.
 * - **Implausibly old** — an age past {@link IMPLAUSIBLE_AGE_SECS} arrives from
 *   a corrupted or hand-edited field, and "36500d ago" is a fabricated
 *   precision, not a fact.
 */
export function humanizeAge(now: number, then: number): string {
  const secs = now - then;
  if (secs < 0) return UNKNOWN_AGE_AHEAD_OF_NOW;
  if (secs > IMPLAUSIBLE_AGE_SECS) return UNKNOWN_AGE_IMPLAUSIBLY_OLD;
  if (secs < 60) return "just now";
  if (secs < 3_600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3_600)}h ago`;
  return `${Math.floor(secs / 86_400)}d ago`;
}

/**
 * A member's own reference resolution, **never bare**: the lower bound with its
 * numerator, its denominator and the population it was taken over, or why there
 * is none (NFR-CC-04, FR-RS-49).
 *
 * One spelling, two call sites — the two app-level workspace views' member
 * tables (S-428). The S-428 review found the two tables printing the same field
 * as a bare `"40.0%"`: two members with identical coverage over wildly different
 * reference populations rendered identically, which is the denominator-disclosure
 * duty [CR-111] made general. Since S-635 the figure is `internal_resolution`,
 * never the deprecated whole-ledger ratio: both tables carry the
 * `referenceResolution` gloss, which states that lower bound.
 *
 * [CR-111]: ../../../../docs/requests/CR-111-bound-ratio-carries-its-denominator.md
 */
export function memberResolution(figure: InternalResolution): string {
  return figure.lower_bound === null
    ? DASHBOARD_TEXT.resolutionAbsent(figure)
    : DASHBOARD_TEXT.ownResolution(`${(figure.lower_bound * 100).toFixed(1)}%`, figure);
}

// ── The Graph widget's reference resolution (S-636, FR-RS-49) ────────────────

/** What the Graph widget's headline states, decided from `status` alone. */
export type ResolutionHeadline =
  /** The share of internal candidates resolved — a lower bound, one-decimal percent. */
  | { kind: "share"; share: string }
  /** The graph records no code reference: there is no population to take a share of. */
  | { kind: "no_references" }
  /** Every code reference is provably external: none could resolve here. */
  | { kind: "all_external"; provablyExternal: number }
  /** No share can be read: the server sent no `internal_resolution` (one older
   *  than S-635), or a null share whose counts name no absence this build has a
   *  sentence for. Stated as such, never filled from the deprecated whole-ledger
   *  ratio. */
  | { kind: "unreported" };

/**
 * The headline the Graph widget leads with: `internal_resolution.lower_bound`
 * (resolved code references ÷ internal candidates, FR-RS-49). A zero denominator
 * arrives as `lower_bound: null` with its `absence`, and reads absent here —
 * never 100% or 0% ([NFR-CC-04]). `resolution_coverage` is deprecated: its
 * denominator holds doc references and external code, so it is never read.
 *
 * The absence carries a `cause` tag from the server's closed lexicon
 * (`absence::SENTINELS`), whose spellings the absence census pins to the sites
 * that own them. This view does not re-spell them: it reads the population each
 * stands for — no code reference at all, or code references that are all
 * provably external — from the counts the server sent beside it. A null share
 * whose counts fit neither reads unreported.
 *
 * [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
 */
export function resolutionHeadline(status: StatusInfo): ResolutionHeadline {
  const r = status.internal_resolution;
  if (r === undefined) return { kind: "unreported" };
  if (r.lower_bound !== null) return { kind: "share", share: pctBp(r.lower_bound * 10_000) };
  if (r.absence === null) return { kind: "unreported" };
  if (r.code_references === 0) return { kind: "no_references" };
  if (r.internal_candidates === 0) return { kind: "all_external", provablyExternal: r.provably_external };
  return { kind: "unreported" };
}

/** The population the headline was taken over — *x internal candidates of y code
 *  references, z provably external* — beside how many of the candidates resolved.
 *  `null` when the headline is absent: there is then no population to state. */
export function internalPopulationStatement(status: StatusInfo): string | null {
  const r = status.internal_resolution;
  if (r === undefined || r.lower_bound === null) return null;
  const candidates = `${fmtInt(r.internal_candidates)} internal ${plural(r.internal_candidates, "candidate", "candidates")}`;
  return (
    `${fmtInt(r.resolved)} of ${candidates} resolved — ${candidates} of ` +
    `${fmtInt(r.code_references)} code ${plural(r.code_references, "reference", "references")}, ` +
    `${fmtInt(r.provably_external)} provably external`
  );
}

/** The doc references' own figure, stated apart from the code headline they
 *  would otherwise prop up; `null` when the server did not report it. */
export function docResolutionStatement(status: StatusInfo): string | null {
  const d = status.doc_resolution;
  if (d === undefined) return null;
  if (d.ratio === null) return "No documentation references recorded.";
  return `Documentation references: ${pctBp(d.ratio * 10_000)} resolved (${fmtInt(d.resolved)} of ${fmtInt(d.references)})`;
}

/**
 * The informative freshness line: a relative age from `last_full_index_at` /
 * `last_sync_at` plus the plain-language caveat — never the raw internal freshness
 * citation (`status.freshness`, the ADR-11 contract prose). Mirrors
 * `overview.rs::freshness_statement`.
 */
export function freshnessStatement(status: StatusInfo, nowUnix: number): string {
  const caveat = "reflects the last index, not unsaved edits";
  const indexedAt = parseSecs(status.last_full_index_at);
  const syncedAt = parseSecs(status.last_sync_at);
  let phrase: string;
  if (indexedAt !== null) {
    phrase = `Indexed ${humanizeAge(nowUnix, indexedAt)}`;
  } else if (syncedAt !== null) {
    phrase = `Last synced ${humanizeAge(nowUnix, syncedAt)}`;
  } else {
    phrase = "Index present";
  }
  return `${phrase} — ${caveat}`;
}

// ── The Security widget (S-789, FR-UI-09) ────────────────────────────────────

/** What the Security widget states, decided from `overview.security` alone. */
export type SecurityWidgetState =
  /** The read failed: stated with its cause, never shown as zero findings. */
  | { kind: "fault"; error: string }
  /** Nothing ever scanned and no scanner is consented to: names the consent command. */
  | { kind: "no_consent" }
  /** Nothing ever scanned: names the scan command. */
  | { kind: "never_scanned" }
  /** A report is stored: its counts, current or not. */
  | { kind: "scanned"; summary: SecuritySummary; counted: SecuritySeverityCounts };

/** The state the widget renders. A fault outranks everything, and a project with
 *  no stored report has no count at all — `tally` is `null` then (BR-66), which
 *  is read as the absence it is, never as zero. */
export function securityState(security: OverviewSecurity): SecurityWidgetState {
  const { summary, error } = security;
  if (error !== null) return { kind: "fault", error };
  if (summary === null) return { kind: "fault", error: "the overview carried no security state" };
  if (summary.tally === null || summary.scan === "never_run") {
    return summary.consent.consented.length === 0 ? { kind: "no_consent" } : { kind: "never_scanned" };
  }
  return { kind: "scanned", summary, counted: summary.tally.counted };
}

/** *N of M scanners* whose last scan failed. */
export function scannersFailedStatement(summary: SecuritySummary): string {
  return `${summary.scanners_failed} of ${summary.scanners_total} ${plural(summary.scanners_total, "scanner", "scanners")}`;
}

/** The newest stored scan as a relative age, `unknown` phrases included. */
export function newestScanStatement(summary: SecuritySummary, nowUnix: number): string {
  return summary.newest_scan_at === null ? "none stored" : humanizeAge(nowUnix, summary.newest_scan_at);
}

/** The `[security]` gate verdict in words; `null` when no table is configured, so
 *  the widget states that absence instead. */
export function gateStatement(gate: SecurityGateBand | null): string | null {
  if (gate === null) return null;
  if (gate.passed) return `passes ${gate.checked_rules} ${plural(gate.checked_rules, "rule", "rules")}`;
  const violations = gate.violations.length;
  const missing = gate.missing_scans.length;
  const reasons = [
    ...(violations > 0 ? [`${violations} ${plural(violations, "finding", "findings")} fail a rule`] : []),
    ...(missing > 0 ? [`${missing} required ${plural(missing, "scan", "scans")} missing`] : []),
  ];
  return reasons.length > 0 ? `fails: ${reasons.join(", ")}` : "fails";
}

/** How many more findings are on a dev-only dependency — their lockfile marks
 *  the package dev — which the counts leave out (FR-SC-30, S-810); `null` when
 *  none are. */
export function devOnlyStatement(tally: SecurityFindingTally): string | null {
  const n = tally.excluded.dev;
  return n > 0 ? `${fmtInt(n)} more in dev-only dependencies` : null;
}

/** How many more findings need review — their scanner marks them low-confidence
 *  — which the counts leave out (FR-SC-31, S-817); `null` when none do. */
export function needsReviewStatement(tally: SecurityFindingTally): string | null {
  const n = tally.excluded.needs_review;
  return n > 0 ? `${fmtInt(n)} more ${plural(n, "needs", "need")} review` : null;
}

/** How many more findings lie in test code, which the counts leave out
 *  (FR-SC-28, S-795); `null` when none do. */
export function testCodeStatement(tally: SecurityFindingTally): string | null {
  return tally.excluded.test > 0 ? `${fmtInt(tally.excluded.test)} more in test code` : null;
}
