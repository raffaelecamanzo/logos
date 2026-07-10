/*
 * Pure Dashboard model (S-187, FR-UI-09) — the presentation logic ported from the
 * server-rendered Dashboard (web/src/views/overview.rs) into framework-free,
 * unit-testable functions: the BR-34 advisory quality bands, the freshness
 * statement, the basis-point → percent reprojection, and the Project-Overview prose
 * snippet. No DOM, no React — every figure is a projection of a read-model field,
 * inventing nothing (NFR-RA-05).
 */

import type { StatusInfo } from "../../api/types.ts";
import type { ScoreBarTone } from "../../components/index.ts";

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

// ── Basis-point reprojection (web/src/views/mod.rs::pct_bp) ───────────────────

/** Reproject basis points (0–10000) to a one-decimal percent string, clamped. */
export function pctBp(bp: number): string {
  const clamped = Math.min(10_000, Math.max(0, bp));
  return `${(clamped / 100).toFixed(1)}%`;
}

// ── Freshness statement (frontend-design §4.1 verdict element) ───────────────

/** Parse an optional unix-seconds string field into a number of seconds. */
function parseSecs(field: string | null): number | null {
  if (field === null) return null;
  // Only an all-digits string is a valid unix-seconds field (parseInt is too lax).
  if (!/^\d+$/.test(field)) return null;
  return Number(field);
}

/**
 * Humanise the age between `now` and `then` (both unix seconds) into a coarse
 * relative phrase. A future timestamp (clock skew) saturates to "just now".
 */
export function humanizeAge(now: number, then: number): string {
  const secs = Math.max(0, now - then);
  if (secs < 60) return "just now";
  if (secs < 3_600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86_400) return `${Math.floor(secs / 3_600)}h ago`;
  return `${Math.floor(secs / 86_400)}d ago`;
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

// ── Project-Overview prose snippet (overview.rs::snippet_of) ──────────────────

/** A non-empty run of all `=` (H1) or all `-` (H2) — a setext-heading underline. */
function isSetextUnderline(line: string): boolean {
  return line.length > 0 && (/^=+$/.test(line) || /^-+$/.test(line));
}

/** Is `line` a thematic break (`---`/`***`/`___`, ≥3, allowing spaces)? */
function isHr(line: string): boolean {
  const compact = line.replace(/\s+/g, "");
  return compact.length >= 3 && (/^-+$/.test(compact) || /^\*+$/.test(compact) || /^_+$/.test(compact));
}

/** Drop a single leading block marker (bullet / blockquote / ordered number). */
function stripLeadingMarker(line: string): string {
  const t = line.replace(/^\s+/, "");
  const bullet = t.match(/^([-*+]|>)\s+(.*)$/);
  if (bullet) return bullet[2].replace(/^\s+/, "");
  const ordered = t.match(/^\d+[.)]\s+(.*)$/);
  if (ordered) return ordered[1].replace(/^\s+/, "");
  return t;
}

/**
 * Reduce one line of inline Markdown to plain text: drop a leading list/quote
 * marker, unwrap `[label](url)` links to their `label`, and strip `*`/`_`/`` ` ``
 * emphasis runs. Coarse but safe — the result is plain text the renderer escapes.
 */
function stripInlineMarkdown(line: string): string {
  const withoutMarker = stripLeadingMarker(line);
  // Unwrap links: `[label](url)` → `label`, then `[label]` → `label`.
  const unlinked = withoutMarker
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]/g, "$1");
  // Strip emphasis/code runs.
  return unlinked.replace(/[*_`]/g, "").trim();
}

/** Truncate `s` to at most `max` chars at a word boundary, appending an ellipsis. */
function truncateWords(s: string, max: number): string {
  if ([...s].length <= max) return s;
  const cut = [...s].slice(0, max).join("");
  const lastSpace = cut.search(/\s\S*$/);
  const trimmed = lastSpace >= 0 ? cut.slice(0, lastSpace).replace(/\s+$/, "") : cut.replace(/\s+$/, "");
  return `${trimmed}…`;
}

/**
 * A short plain-text snippet from a page's Markdown `body` for the Project-Overview
 * widget — the first prose paragraph with structural Markdown (headings, fences,
 * list/quote markers, inline emphasis, link syntax) reduced to plain text and
 * truncated at a word boundary (~480 chars). A presentation-only projection that
 * invents nothing; an empty result is returned verbatim so the caller falls back
 * honestly. Mirrors `overview.rs::snippet_of`.
 */
export function snippetOf(body: string): string {
  const MAX = 480;
  const lines = body.split("\n");
  let para = "";
  let fence: string | null = null;
  let idx = 0;
  while (idx < lines.length) {
    const line = lines[idx].trim();
    idx += 1;
    if (fence !== null) {
      if (line.startsWith(fence)) fence = null;
      continue;
    }
    if (line.startsWith("```")) {
      fence = "```";
      continue;
    }
    if (line.startsWith("~~~")) {
      fence = "~~~";
      continue;
    }
    if (line === "") {
      if (para !== "") break;
      continue;
    }
    if (line.startsWith("#") || isHr(line)) continue;
    // A setext-underlined first line is a title — skip it and its underline.
    if (para === "" && idx < lines.length && isSetextUnderline(lines[idx].trim())) {
      idx += 1;
      continue;
    }
    const cleaned = stripInlineMarkdown(line);
    if (cleaned === "") continue;
    para = para === "" ? cleaned : `${para} ${cleaned}`;
  }
  return truncateWords(para, MAX);
}
