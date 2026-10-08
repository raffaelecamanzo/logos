/*
 * Test seam for the hidden-widget register (S-612, FR-UI-41): remove one entry
 * from the REAL register so a view's test can assert the widget renders again.
 *
 * It splices the live array rather than mocking `isWidgetHidden`, so the return
 * path a test exercises is the production lookup — a mock would only prove the
 * mock. Always pair it with the returned restore (e.g. in `afterEach`), or the
 * entry stays out for every later test in the file.
 */

import { HIDDEN_WIDGETS, type HiddenWidget, type HideableWidget } from "../views/hiddenWidgets.ts";

/** Remove `id`'s register entry; the returned function puts it back where it was. */
export function removeHiddenWidgetEntry(id: HideableWidget): () => void {
  const register = HIDDEN_WIDGETS as HiddenWidget[];
  const at = register.findIndex((w) => w.id === id);
  if (at < 0) throw new Error(`"${id}" is not in the hidden-widget register`);
  const [entry] = register.splice(at, 1);
  return () => {
    register.splice(at, 0, entry);
  };
}
