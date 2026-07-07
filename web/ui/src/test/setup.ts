// Vitest setup (S-186): wire the jest-dom matchers (`toBeInTheDocument`, …) into
// Vitest's `expect`, and provide the jsdom shims the design-system components and
// the router touch (matchMedia for the theme, which jsdom lacks).
import "@testing-library/jest-dom/vitest";

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
// Chat consent gate, S-190, persists its first-use acknowledgement there). Mirrors
// the matchMedia shim above — installed only when the runtime lacks it.
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

// jsdom shims the assistant-ui chat surface (S-200) touches: its thread viewport
// observes resize and scrolls the newest turn into view, and the message-level
// copy control writes to the clipboard. jsdom provides none of these; install
// inert shims so the component tree mounts and the copy/scroll paths no-op safely.
if (!("ResizeObserver" in globalThis)) {
  (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
if (!Element.prototype.scrollTo) {
  // assistant-ui's thread viewport auto-scrolls via `el.scrollTo`; jsdom omits it.
  Element.prototype.scrollTo = () => {};
}
if (!navigator.clipboard) {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: () => Promise.resolve() },
  });
}
