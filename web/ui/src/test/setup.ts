// Vitest setup (S-186): wire the jest-dom matchers (`toBeInTheDocument`, …) into
// Vitest's `expect`, and provide the jsdom shims the design-system components and
// the router touch (matchMedia for the theme, which jsdom lacks).
import "@testing-library/jest-dom/vitest";
import { afterEach, expect } from "vitest";

import { takeNestedSortControls, watchSortButtons } from "./nestedSortControls.ts";

// CR-208 AC-4: no table header's sort button may hold a gloss or any other
// focusable element, in ANY test that renders a table (see nestedSortControls.ts).
watchSortButtons();
afterEach(() => {
  expect(takeNestedSortControls(), "a table header's sort button holds a focusable element (CR-208 AC-4)").toEqual([]);
});

// The served shell injects the per-session intent (CSRF) token as a
// `<meta name="logos-intent">` tag, which `src/intent.ts` reads ONCE at module
// load to authorise mutating requests (NFR-SE-06, S-191 Config writes). jsdom
// serves no such shell, so provide the tag before any test module imports
// `intent.ts` — otherwise `apiMutate` would (correctly) refuse to send a
// token-less request and the Config editor's write tests could not run.
if (!document.querySelector('meta[name="logos-intent"]')) {
  const meta = document.createElement("meta");
  meta.setAttribute("name", "logos-intent");
  meta.setAttribute("content", "test-intent-token");
  document.head.appendChild(meta);
}

if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as unknown as MediaQueryList;
}

// An in-memory localStorage shim for the jsdom environments that omit Storage (the
// theme persists its choice there, `theme/theme.ts`). Mirrors the matchMedia shim
// above — installed only when the runtime lacks it.
if (!window.localStorage) {
  const store = new Map<string, string>();
  window.localStorage = {
    getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
    setItem: (key: string, value: string) => void store.set(key, String(value)),
    removeItem: (key: string) => void store.delete(key),
    clear: () => store.clear(),
    key: (index: number) => Array.from(store.keys())[index] ?? null,
    get length() {
      return store.size;
    },
  } as Storage;
}
