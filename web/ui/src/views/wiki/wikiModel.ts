/*
 * Pure Wiki presentation helpers (S-189, FR-UI-06) — the sub-route parsing, the
 * "On this page" TOC extraction, and the freshness-banner phrasing — kept DOM-free
 * so they are unit-testable. The TOC reads the `toc-<slug>` heading ids the
 * server-side renderer already injects (`web/src/markdown.rs`), so the rail and the
 * body anchors line up with no client re-anchoring.
 */

import type { WikiStatus } from "../../api/types.ts";

/** The reader path for an agent page slug — the slug is path-like (`a/b`). */
export function pageHref(slug: string): string {
  return `/wiki/page/${slug}`;
}

/** Which Wiki sub-view a client pathname resolves to. */
export type WikiRoute =
  | { kind: "landing" }
  | { kind: "search" }
  | { kind: "page"; slug: string };

/** Resolve the Wiki sub-route from a pathname (the Wiki tab owns `/wiki/*`). An
 *  empty page slug falls back to the landing rather than an empty reader. */
export function wikiRoute(pathname: string): WikiRoute {
  if (pathname === "/wiki/search") return { kind: "search" };
  const PAGE = "/wiki/page/";
  if (pathname.startsWith(PAGE)) {
    const slug = decodeURIComponent(pathname.slice(PAGE.length)).replace(/^\/+|\/+$/g, "");
    return slug ? { kind: "page", slug } : { kind: "landing" };
  }
  return { kind: "landing" };
}

/** One "On this page" rail entry — its heading level, anchor id, and text label. */
export interface TocEntry {
  level: 2 | 3;
  id: string;
  label: string;
}

/** Strip HTML tags from a heading's inner markup, leaving its text (entities are
 *  left as-is — they were escaped at render and the browser renders them). */
function stripTags(html: string): string {
  return html.replace(/<[^>]*>/g, "").trim();
}

/**
 * Extract the `<h2 id>` / `<h3 id>` headings from the server-rendered wiki HTML into
 * ordered TOC entries (the renderer shifts body headings into the h2/h3 range and
 * gives each a `toc-<slug>` id, so this is a read, never a re-anchor). A heading
 * with no id is skipped (it cannot be linked).
 */
export function extractToc(html: string): TocEntry[] {
  const entries: TocEntry[] = [];
  const re = /<h([23])\s+id="([^"]+)">([\s\S]*?)<\/h\1>/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html)) !== null) {
    const level = Number(m[1]) as 2 | 3;
    const label = stripTags(m[3]);
    if (label) entries.push({ level, id: m[2], label });
  }
  return entries;
}

/** The verdict-first freshness summary the landing leads with (mirrors the legacy
 *  `freshness_banner`): FRESH only when nothing is regeneration-pending and no
 *  anchor is stale/missing; otherwise an honest breakdown. */
export interface FreshnessSummary {
  state: "FRESH" | "STALE";
  tone: "pass" | "signal";
  /** The detail line (counts + revision), already composed. */
  detail: string;
}

export function freshnessSummary(status: WikiStatus): FreshnessSummary {
  const anchorsFresh = status.stale_count === 0 && status.missing_anchor_count === 0;
  const allFresh = status.revision_stale_count === 0 && anchorsFresh;
  const plural = status.page_count === 1 ? "" : "s";
  const parts: string[] = [];
  if (allFresh) {
    parts.push(`${status.page_count} page${plural} · all fresh`);
  } else {
    parts.push(
      `${status.page_count} page${plural} · ${status.fresh_count} fresh · ` +
        `${status.stale_count} stale · ${status.missing_anchor_count} missing-anchor`,
    );
    if (status.revision_stale_count > 0) {
      parts.push(`${status.revision_stale_count} agent page(s) stale — regeneration pending`);
    }
  }
  parts.push(`graph @revision ${status.current_revision}`);
  return {
    state: allFresh ? "FRESH" : "STALE",
    tone: allFresh ? "pass" : "signal",
    detail: parts.join(" · "),
  };
}
