/*
 * Client-side search-term jump-to-match (S-271, FR-WK-28, FR-WK-05). Wraps the
 * first occurrence of a search query term already present in the mounted, safe
 * server-rendered wiki body in a `<mark>` so the reader can scroll to it — a
 * presentation-only DOM mutation over already-rendered content. No FTS/read-model
 * change: the backend and `WikiHit` stay page-granular (no per-match offset,
 * section anchor, or snippet).
 */

/** The class stamped on the inserted `<mark>` — also its idempotency guard. */
const HIGHLIGHT_CLASS = "wiki-search-hit";
export const HIGHLIGHT_MARK_SELECTOR = `mark.${HIGHLIGHT_CLASS}`;

/**
 * Find the first case-insensitive occurrence of `term` in `container`'s rendered
 * text and wrap it in a `<mark>`, returning the inserted element so the caller can
 * scroll it into view. Returns `null` — no error, no mutation — when `term` is
 * empty/whitespace, absent from the body, or a mark from a prior call is already
 * present (idempotent against repeated effect runs on the same content).
 */
export function highlightFirstMatch(container: HTMLElement, term: string): HTMLElement | null {
  const needle = term.trim();
  if (!needle) return null;
  if (container.querySelector(HIGHLIGHT_MARK_SELECTOR)) return null;

  const needleLower = needle.toLowerCase();
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode() as Text | null;
  while (node !== null) {
    const text = node.textContent ?? "";
    const idx = text.toLowerCase().indexOf(needleLower);
    if (idx !== -1) {
      // splitText(idx) leaves `node` holding the text before the match and returns
      // a new node starting at the match; splitting that again after the needle's
      // length isolates the match itself as its own text node to wrap.
      const matchNode = node.splitText(idx);
      matchNode.splitText(needle.length);
      const mark = document.createElement("mark");
      mark.className = HIGHLIGHT_CLASS;
      matchNode.replaceWith(mark);
      mark.appendChild(matchNode);
      return mark;
    }
    node = walker.nextNode() as Text | null;
  }
  return null;
}

/**
 * Remove a mark left by a prior `highlightFirstMatch` call, restoring its text in
 * place. `dangerouslySetInnerHTML` only touches the DOM when its HTML string
 * changes, so re-visiting the same page (e.g. re-clicking its own already-active
 * menu link) with no search term leaves a stale mark from an earlier
 * search-originated visit unless it is explicitly cleared. A no-op when no mark
 * is present.
 */
export function clearHighlight(container: HTMLElement): void {
  const mark = container.querySelector(HIGHLIGHT_MARK_SELECTOR);
  if (!mark) return;
  const text = mark.firstChild;
  if (text) {
    mark.replaceWith(text);
  } else {
    mark.remove();
  }
}
