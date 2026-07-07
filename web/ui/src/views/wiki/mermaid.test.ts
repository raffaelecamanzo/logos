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
