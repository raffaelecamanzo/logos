/*
 * Every rendered table header holds its sort button free of anything focusable
 * (CR-208 AC-4, FR-UI-39). A `Term` written into a column's `header` lands INSIDE
 * the sort `<button>` — two tab stops, and a click on the gloss sorts — and it can
 * reach `header` by any route: a literal, a const, a column-builder argument. So
 * this reads the RENDERED DOM rather than the source: the test setup
 * (`setup.ts`) watches every node any test mounts and fails the test that mounted
 * a header sort button holding a focusable element.
 *
 * A MutationObserver rather than a query at the end of the test, because a
 * test's own `afterEach(cleanup)` runs before the setup file's hook and would
 * leave nothing to query.
 */

/** A focusable element (or a gloss) inside a table header's sort button. */
const NESTED = ["dfn", "[tabindex]", "a[href]", "button", "input", "select", "textarea"]
  .map((inner) => `th button ${inner}`)
  .join(", ");

let found: string[] = [];
let observer: MutationObserver | null = null;
/** Elements already reported: a node and its parent can both arrive as added
 *  (the test container, then the table mounted into it), so one subtree is often
 *  scanned twice. */
let reported = new WeakSet<Element>();

function scan(node: Node): void {
  if (!(node instanceof Element)) return;
  const hits = node.matches(NESTED) ? [node] : [];
  hits.push(...node.querySelectorAll(NESTED));
  for (const hit of hits) {
    if (reported.has(hit)) continue;
    reported.add(hit);
    const header = (hit.closest("th")?.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 60);
    found.push(`<${hit.tagName.toLowerCase()}> inside the sort button of header "${header}"`);
  }
}

/** Starts watching the document for nested sort controls (once per test file). */
export function watchSortButtons(): void {
  if (observer !== null) return;
  observer = new MutationObserver((records) => {
    for (const record of records) record.addedNodes.forEach(scan);
  });
  observer.observe(document.body, { childList: true, subtree: true });
}

/** The nested sort controls mounted since the last call, then forgets them. */
export function takeNestedSortControls(): string[] {
  observer?.takeRecords().forEach((record) => record.addedNodes.forEach(scan));
  const taken = found;
  found = [];
  reported = new WeakSet();
  return taken;
}
