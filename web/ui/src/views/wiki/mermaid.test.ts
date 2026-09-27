import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { currentTheme, renderMermaidIn, THEME_VARS } from "./mermaid.ts";

// ── Helpers ───────────────────────────────────────────────────────────────────

function setThemeAttr(t: "light" | "dark" | null) {
  if (t) document.documentElement.setAttribute("data-theme", t);
  else document.documentElement.removeAttribute("data-theme");
}

afterEach(() => {
  setThemeAttr(null);
  // Remove any <script> a test injected.
  document.querySelectorAll('script[src*="mermaid"]').forEach((s) => s.remove());
});

// ── currentTheme() ────────────────────────────────────────────────────────────

describe("currentTheme (S-196, ADR-44)", () => {
  it("returns 'dark' when data-theme='dark'", () => {
    setThemeAttr("dark");
    expect(currentTheme()).toBe("dark");
  });

  it("returns 'light' when data-theme='light'", () => {
    setThemeAttr("light");
    expect(currentTheme()).toBe("light");
  });

  it("returns 'dark' as the default when no data-theme is set (setup shim: matchMedia always false)", () => {
    // The test setup's matchMedia shim always returns matches:false, so
    // prefers-color-scheme: light → false → dark default (ADR-44).
    setThemeAttr(null);
    expect(currentTheme()).toBe("dark");
  });

  it("returns 'light' when no data-theme but matchMedia reports light OS preference", () => {
    setThemeAttr(null);
    const saved = window.matchMedia;
    window.matchMedia = (q: string) =>
      ({ matches: q === "(prefers-color-scheme: light)", media: q } as MediaQueryList);
    expect(currentTheme()).toBe("light");
    window.matchMedia = saved;
  });
});

// ── THEME_VARS ────────────────────────────────────────────────────────────────

describe("THEME_VARS design-token alignment (S-196)", () => {
  it("dark: primaryTextColor matches --neutral-100 (#e8ebf0)", () => {
    expect(THEME_VARS.dark.primaryTextColor).toBe("#e8ebf0");
  });

  it("dark: primaryColor (node fill) matches --surface-2 (#1f242c)", () => {
    expect(THEME_VARS.dark.primaryColor).toBe("#1f242c");
  });

  it("dark: background matches --surface-0 (#0f1216)", () => {
    expect(THEME_VARS.dark.background).toBe("#0f1216");
  });

  it("light: primaryTextColor matches --so-merlin (#3d3935)", () => {
    expect(THEME_VARS.light.primaryTextColor).toBe("#3d3935");
  });

  it("light: primaryColor (node fill) is white (#ffffff — surface-1)", () => {
    expect(THEME_VARS.light.primaryColor).toBe("#ffffff");
  });

  it("light: background matches --so-merlin-50 (#f4f4f2)", () => {
    expect(THEME_VARS.light.background).toBe("#f4f4f2");
  });
});

// ── renderMermaidIn — progressive enhancement ─────────────────────────────────

describe("renderMermaidIn progressive enhancement (S-189, FR-WK-15)", () => {
  it("is a no-op (loads nothing) when the container has no .mermaid blocks", async () => {
    const container = document.createElement("div");
    container.innerHTML = "<p>prose, no diagram</p>";
    await expect(renderMermaidIn(container)).resolves.toBeUndefined();
    expect(document.querySelector('script[src*="mermaid"]')).toBeNull();
  });
});

// ── renderMermaidIn — theme-aware initialization ──────────────────────────────

describe("renderMermaidIn initializes Mermaid with theme-matched themeVariables (S-196)", () => {
  // Each test resets the module so loadPromise and initializedTheme start fresh.
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (window as any).mermaid;
    setThemeAttr(null);
  });

  it("passes theme:'base' + dark themeVariables when data-theme is dark", async () => {
    const mockInit = vi.fn();
    const mockRun = vi.fn().mockResolvedValue(undefined);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: mockInit, run: mockRun };
    setThemeAttr("dark");

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';
    await fresh(container);

    expect(mockInit).toHaveBeenCalledWith(
      expect.objectContaining({
        theme: "base",
        themeVariables: expect.objectContaining({ primaryTextColor: "#e8ebf0" }),
      }),
    );
  });

  it("passes theme:'base' + light themeVariables when data-theme is light", async () => {
    const mockInit = vi.fn();
    const mockRun = vi.fn().mockResolvedValue(undefined);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: mockInit, run: mockRun };
    setThemeAttr("light");

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';
    await fresh(container);

    expect(mockInit).toHaveBeenCalledWith(
      expect.objectContaining({
        theme: "base",
        themeVariables: expect.objectContaining({ primaryTextColor: "#3d3935" }),
      }),
    );
  });

  it("re-initializes (calls initialize again) when the theme changes between renders", async () => {
    const mockInit = vi.fn();
    const mockRun = vi.fn().mockResolvedValue(undefined);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: mockInit, run: mockRun };

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';

    setThemeAttr("dark");
    await fresh(container);
    expect(mockInit).toHaveBeenCalledTimes(1);

    // Simulate re-render after theme switch: reset processed state.
    container.querySelectorAll(".mermaid").forEach((el) => el.removeAttribute("data-processed"));
    setThemeAttr("light");
    await fresh(container);
    expect(mockInit).toHaveBeenCalledTimes(2);
    expect(mockInit).toHaveBeenLastCalledWith(
      expect.objectContaining({
        themeVariables: expect.objectContaining({ primaryTextColor: "#3d3935" }),
      }),
    );
  });

  it("does NOT re-initialize when the same theme is used for a second render", async () => {
    const mockInit = vi.fn();
    const mockRun = vi.fn().mockResolvedValue(undefined);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: mockInit, run: mockRun };
    setThemeAttr("dark");

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML =
      '<div class="mermaid">graph TD\nA --> B</div><div class="mermaid">graph TD\nC --> D</div>';

    await fresh(container);
    // Navigate to another page with a diagram (still dark theme).
    const container2 = document.createElement("div");
    container2.innerHTML = '<div class="mermaid">graph TD\nX --> Y</div>';
    await fresh(container2);

    // Only one initialize call — the theme did not change.
    expect(mockInit).toHaveBeenCalledTimes(1);
  });
});

// ── Constructable stylesheet adopt/unadopt lifecycle (HF-3) ───────────────────
//
// jsdom (this project's test environment) implements the `CSSStyleSheet`
// constructor but not `#replaceSync` or `document.adoptedStyleSheets` — confirmed
// against the installed jsdom 25.0.1. The polyfills below patch in exactly the
// two missing pieces onto the REAL jsdom classes, so these tests exercise
// `renderMermaidIn`'s actual adopt/unadopt code path (including its
// `typeof CSSStyleSheet === "undefined"` feature-detect finding a real
// constructor) rather than a fully mocked stand-in.

describe("renderMermaidIn adopts a constructable stylesheet per diagram (HF-3)", () => {
  beforeEach(() => {
    vi.resetModules();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (CSSStyleSheet.prototype as any).replaceSync = function (this: { cssText?: string }, css: string) {
      this.cssText = css;
    };
    Object.defineProperty(document, "adoptedStyleSheets", {
      value: [],
      writable: true,
      configurable: true,
    });
  });

  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (CSSStyleSheet.prototype as any).replaceSync;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (document as any).adoptedStyleSheets;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (window as any).mermaid;
    setThemeAttr(null);
  });

  /**
   * Installs a `.run` mock that hands each `nodes` entry its own scoped `<style>`,
   * mirroring the vendored bundle's real behaviour (a `<style>` as the drawn
   * `<svg>`'s first child, its rules prefixed by that diagram's own `#mermaid-N`
   * id). Returns a `setStyle` setter rather than taking a fixed style up front,
   * because `renderMermaidIn` memoizes the loaded bundle reference for the life of
   * the module (`loadMermaid`'s `loadPromise`) — a test that re-renders more than
   * once per module instance must vary this SAME mock's output across calls, not
   * install a second `window.mermaid` object that the already-resolved
   * `loadPromise` would never pick up.
   */
  function mockMermaidDraw(initial: (index: number) => string) {
    let styleFor = initial;
    const run = vi.fn(async (opts: { nodes?: ArrayLike<Element> }) => {
      const nodes = opts.nodes ? Array.from(opts.nodes) : [];
      nodes.forEach((node, i) => {
        node.innerHTML = `<svg><style>${styleFor(i)}</style><rect /></svg>`;
        node.setAttribute("data-processed", "true");
      });
    });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: vi.fn(), run };
    return { run, setStyle: (fn: (index: number) => string) => (styleFor = fn) };
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  function adopted(): any[] {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return (document as any).adoptedStyleSheets;
  }

  it("adopts the rendered diagram's <style> text as a constructable stylesheet", async () => {
    mockMermaidDraw(() => "#mermaid-1 .node rect{fill:#123456}");
    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';

    await fresh(container);

    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].cssText).toBe("#mermaid-1 .node rect{fill:#123456}");
  });

  it("keeps each diagram's sheet independent when several render in one container", async () => {
    mockMermaidDraw((i) => `#mermaid-${i} .node rect{fill:#${i}${i}${i}${i}${i}${i}}`);
    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML =
      '<div class="mermaid">graph TD\nA-->B</div><div class="mermaid">graph TD\nC-->D</div>';

    await fresh(container);

    expect(adopted()).toHaveLength(2);
    const cssTexts = adopted().map((s) => s.cssText);
    expect(cssTexts).toContain("#mermaid-0 .node rect{fill:#000000}");
    expect(cssTexts).toContain("#mermaid-1 .node rect{fill:#111111}");
  });

  it("replaces (never stacks) the sheet when the same element re-renders", async () => {
    const mock = mockMermaidDraw(() => "#mermaid-1 .node rect{fill:#111111}");
    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';

    await fresh(container);
    expect(adopted()).toHaveLength(1);

    // Simulate a theme-toggle re-render: same element, freshly generated CSS.
    container.querySelectorAll(".mermaid").forEach((el) => el.removeAttribute("data-processed"));
    mock.setStyle(() => "#mermaid-1 .node rect{fill:#222222}");
    await fresh(container);

    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].cssText).toBe("#mermaid-1 .node rect{fill:#222222}");
  });

  it("unadoptMermaidStyleFor removes exactly that element's sheet, leaving others", async () => {
    const mock = mockMermaidDraw(() => "#mermaid-1 .node rect{fill:#123456}");
    const { renderMermaidIn: fresh, unadoptMermaidStyleFor } = await import("./mermaid.ts");
    const containerA = document.createElement("div");
    containerA.innerHTML = '<div class="mermaid">graph TD\nA-->B</div>';
    const containerB = document.createElement("div");
    containerB.innerHTML = '<div class="mermaid">graph TD\nC-->D</div>';

    await fresh(containerA);
    mock.setStyle(() => "#mermaid-2 .node rect{fill:#654321}");
    await fresh(containerB);
    expect(adopted()).toHaveLength(2);

    unadoptMermaidStyleFor(containerA.querySelector(".mermaid")!);

    expect(adopted()).toHaveLength(1);
    expect(adopted()[0].cssText).toBe("#mermaid-2 .node rect{fill:#654321}");
  });

  it("unadoptMermaidStyleFor on an element with no adopted sheet is a no-op", async () => {
    const { unadoptMermaidStyleFor } = await import("./mermaid.ts");
    expect(() => unadoptMermaidStyleFor(document.createElement("div"))).not.toThrow();
    expect(adopted()).toHaveLength(0);
  });

  it("does not adopt a stylesheet when Mermaid draws no <style> (nothing to copy)", async () => {
    const run = vi.fn(async (opts: { nodes?: ArrayLike<Element> }) => {
      const nodes = opts.nodes ? Array.from(opts.nodes) : [];
      nodes.forEach((node) => {
        node.innerHTML = "<svg><rect /></svg>";
        node.setAttribute("data-processed", "true");
      });
    });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: vi.fn(), run };

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';

    await fresh(container);

    expect(adopted()).toHaveLength(0);
  });
});
