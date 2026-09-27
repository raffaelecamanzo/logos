import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { currentTheme, renderMermaidIn, replayMermaidInlineStyles, repairSequenceDiagramSource, THEME_VARS } from "./mermaid.ts";

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
//
// Every container is appended to `document.body` before rendering (and cleared
// in `afterEach`): `renderMermaidIn` gates adoption on `node.isConnected`
// (review-fix, HF-3), so an unattached `document.createElement` container would
// silently skip adoption for a reason that has nothing to do with what each test
// claims to check.

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
    document.body.innerHTML = "";
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
    document.body.appendChild(container);

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
    document.body.appendChild(container);

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
    document.body.appendChild(container);

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
    document.body.appendChild(containerA);
    document.body.appendChild(containerB);

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
    document.body.appendChild(container);

    await fresh(container);

    expect(adopted()).toHaveLength(0);
  });

  it("does not adopt a stylesheet for a target detached before the async render resolved (review-fix, HF-3)", async () => {
    // `mermaid.run` is a real async boundary in production (the vendored bundle's
    // own layout/draw work, or — on the session's first diagram — the bundle
    // fetch inside `loadMermaid()`). Model that with a deferred promise this test
    // controls, resolved from OUTSIDE the mock so this test never has to guess
    // how many microtask ticks pass before `run` is actually invoked: whenever it
    // is, `runPromise` may already be resolved and its `.then()` still fires.
    let resolveRun!: () => void;
    const runPromise = new Promise<void>((resolve) => {
      resolveRun = resolve;
    });
    const run = vi.fn((opts: { nodes?: ArrayLike<Element> }) =>
      runPromise.then(() => {
        const nodes = opts.nodes ? Array.from(opts.nodes) : [];
        nodes.forEach((node) => {
          node.innerHTML = `<svg><style>#mermaid-1 .node rect{fill:#123456}</style></svg>`;
          node.setAttribute("data-processed", "true");
        });
      }),
    );
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: vi.fn(), run };

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    container.innerHTML = '<div class="mermaid">graph TD\nA --> B</div>';
    document.body.appendChild(container);

    const pending = fresh(container);
    // Simulate the owning component unmounting (or the Wiki page/theme
    // re-rendering) while `mermaid.run` is still in flight.
    container.remove();
    resolveRun();
    await pending;

    expect(adopted()).toHaveLength(0);
  });
});

// ── repairSequenceDiagramSource — ';' repair (HF-2) ───────────────────────────
//
// Pure and DOM-free by design, so these test the repair directly rather than
// through `renderMermaidIn`'s DOM plumbing (covered separately below).

describe("repairSequenceDiagramSource escapes bare ';' in sequenceDiagram message/note text (HF-2)", () => {
  it("escapes the ';' in the user's failing Note line (root-cause reproduction)", () => {
    const source = [
      "sequenceDiagram",
      "    participant SPA",
      "    participant API",
      "    SPA->>API: GET /page",
      "    Note over SPA: Client-side navigation only;<br/>separate GET /api/v1/graph fetch<br/>(not part of this sequence)",
    ].join("\n");
    const repaired = repairSequenceDiagramSource(source);
    expect(repaired).toContain(
      "Note over SPA: Client-side navigation only#59;<br/>separate GET /api/v1/graph fetch<br/>(not part of this sequence)",
    );
    expect(repaired).not.toMatch(/only;/);
  });

  it("escapes a ';' inside a plain message line", () => {
    expect(repairSequenceDiagramSource("sequenceDiagram\nA->>B: x; y")).toBe(
      "sequenceDiagram\nA->>B: x#59; y",
    );
  });

  it("escapes a ';' inside a Note over two participants", () => {
    expect(repairSequenceDiagramSource("sequenceDiagram\nNote over U,S: a note; with semicolon")).toBe(
      "sequenceDiagram\nNote over U,S: a note#59; with semicolon",
    );
  });

  it("escapes a ';' after an activation-marker arrow ('+')", () => {
    expect(repairSequenceDiagramSource("sequenceDiagram\nA->>+B: a;b")).toBe(
      "sequenceDiagram\nA->>+B: a#59;b",
    );
  });

  it("leaves an already-escaped '#59;' alone (idempotent)", () => {
    const source = "sequenceDiagram\nNote over U,S: a note#59; with semicolon";
    expect(repairSequenceDiagramSource(source)).toBe(source);
  });

  it("does not touch a participant alias containing ';' (no colon on the line at all)", () => {
    const source = "sequenceDiagram\nparticipant A as Foo;Bar\nA->>A: hi";
    expect(repairSequenceDiagramSource(source)).toBe(source);
  });

  it("does not touch a loop label", () => {
    const source = "sequenceDiagram\nloop Every minute;check\nA->>B: hi\nend";
    expect(repairSequenceDiagramSource(source)).toBe(source);
  });

  it("leaves a non-sequence diagram byte-identical, including its ';'", () => {
    const source = "flowchart TD\n  A[Start;here] --> B[End]";
    expect(repairSequenceDiagramSource(source)).toBe(source);
  });

  it("skips a leading %%{init}%% directive and blank lines to find the diagram type", () => {
    const source = [
      "%%{init: {'theme':'base'}}%%",
      "",
      "sequenceDiagram",
      "A->>B: x;y",
    ].join("\n");
    expect(repairSequenceDiagramSource(source)).toBe(
      ["%%{init: {'theme':'base'}}%%", "", "sequenceDiagram", "A->>B: x#59;y"].join("\n"),
    );
  });

  it("skips a YAML frontmatter block ('---' ... '---') to find the diagram type", () => {
    const source = ["---", "title: incident flow", "---", "sequenceDiagram", "A->>B: x;y"].join(
      "\n",
    );
    expect(repairSequenceDiagramSource(source)).toBe(
      ["---", "title: incident flow", "---", "sequenceDiagram", "A->>B: x#59;y"].join("\n"),
    );
  });

  it("does not touch a colon line whose head contains '-' but no real arrow token (e.g. a title/metadata line)", () => {
    const source = "sequenceDiagram\ntitle: pre-release notes;more\nA->>B: hi";
    expect(repairSequenceDiagramSource(source)).toBe(source);
  });

  it("does not touch a '%% comment' line even when it contains an arrow-token substring before a colon", () => {
    const source = "sequenceDiagram\n%% A->>B: this should not be touched; right?\nA->>B: hi";
    expect(repairSequenceDiagramSource(source)).toBe(source);
  });
});

describe("renderMermaidIn repairs sequence-diagram ';' on the DOM copy only, before mermaid.run (HF-2)", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (window as any).mermaid;
  });

  it("passes the repaired text to mermaid.run but leaves the container's original prop-level source untouched", async () => {
    const mockRun = vi.fn().mockResolvedValue(undefined);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).mermaid = { initialize: vi.fn(), run: mockRun };

    const { renderMermaidIn: fresh } = await import("./mermaid.ts");
    const container = document.createElement("div");
    const original = "sequenceDiagram\nNote over A: hi;there";
    container.innerHTML = `<div class="mermaid">${original}</div>`;
    const target = container.querySelector(".mermaid")!;

    await fresh(container);

    expect(target.textContent).toBe("sequenceDiagram\nNote over A: hi#59;there");
    expect(mockRun).toHaveBeenCalledWith(expect.objectContaining({ nodes: expect.anything() }));
  });
});

// ── replayMermaidInlineStyles — CSP-dropped style attributes (Sprint 79) ─────

describe("replayMermaidInlineStyles re-applies inline style attributes through the CSSOM", () => {
  /** jsdom parses a style attribute into `el.style` on its own (it has no CSP), so
   *  each element gets a recording `style` object: the test then proves the
   *  function WRITES `cssText` — the CSSOM path the browser CSP leaves open —
   *  rather than relying on the attribute having been honoured. */
  function recordingStyle(el: Element): { cssText: string } {
    const rec = { cssText: "" };
    Object.defineProperty(el, "style", { value: rec, configurable: true });
    return rec;
  }

  it("writes each svg element's style attribute into el.style.cssText and counts them", () => {
    const target = document.createElement("div");
    target.innerHTML =
      '<svg style="max-width: 100%"><path class="messageLine0" style="fill: none;"></path>' +
      '<text class="actor" style="font-size: 13.6px"></text><rect></rect></svg>';
    const svg = target.querySelector("svg")!;
    const path = target.querySelector("path")!;
    const text = target.querySelector("text")!;
    const recs = [recordingStyle(svg), recordingStyle(path), recordingStyle(text)];

    expect(replayMermaidInlineStyles(target)).toBe(3);
    expect(recs.map((r) => r.cssText)).toEqual(["max-width: 100%", "fill: none;", "font-size: 13.6px"]);
  });

  it("touches nothing outside the rendered svg and replays nothing when there is no svg", () => {
    const target = document.createElement("div");
    target.innerHTML = '<p style="color: red"></p>';
    const rec = recordingStyle(target.querySelector("p")!);
    expect(replayMermaidInlineStyles(target)).toBe(0);
    expect(rec.cssText).toBe("");
  });
});
