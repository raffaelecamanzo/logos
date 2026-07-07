/*
 * The Mermaid render seam for the migrated Wiki reader (S-189, S-196, FR-WK-15,
 * NFR-SE-01).
 *
 * It renders the wiki body's `.mermaid` blocks client-side by reusing the
 * **already-vendored, egress-audited** Mermaid bundle (`/assets/vendor/mermaid.min.js`,
 * kept in `web/ui/public/assets/vendor/` — Vite copies it into `dist/` and the binary's
 * SPA fallback handler serves it same-origin; S-192 decommissioned the prior
 * `web/src/assets.rs` route). The bundle is loaded on demand — only on a page that
 * actually carries a diagram.
 *
 *   - **No CDN, no egress.** The script is same-origin; under the self-only CSP
 *     (`script-src 'self'`) a dynamically-inserted same-origin `<script src>` is
 *     permitted (it is not an inline script), and the bundle names no fetch origin
 *     (NFR-SE-01, confirmed by the `spa_bundle` fitness test). Nothing is added to the
 *     SPA bundle itself.
 *   - **CSP-safe theming via themeVariables + external CSS fallback (S-196, CR-051).**
 *     `mermaid.initialize({ theme: "base", themeVariables })` pre-populates Mermaid's
 *     per-render `<style id="mermaid-XXXX">` block with design-token hex values. In
 *     development (no CSP), Mermaid's own style wins (higher specificity). In
 *     production, the self-only CSP blocks that injected `<style>`; the external CSS
 *     Module fallback rules in `WikiView.module.css` (served as a hashed `<link>`,
 *     ADR-44) supply the theme colors via `var(--surface-1)` / `var(--text-1)` etc.
 *   - **Theme-aware (ADR-44).** `currentTheme()` reads the `data-theme` attribute on
 *     `:root` (or OS preference as fallback) so diagrams follow the app's dark/light
 *     toggle. Re-initialization is triggered only when the effective theme changes.
 *   - **Safety preserved.** `securityLevel: "strict"` + `htmlLabels: false` mirror
 *     the legacy `mermaid-init.js`: diagram text is escaped, labels are SVG `<text>`
 *     (no `<foreignObject>` the CSP would strip), and the layout font is aligned with
 *     the painted `--font-sans` so boxes size to their labels.
 *   - **Progressive enhancement.** A load or parse failure leaves the escaped diagram
 *     source visible (never a blank) — the same honesty contract as the legacy.
 *
 * This module is the single seam the Wiki tests mock, so jsdom never loads a 3 MB
 * UMD bundle.
 */

/** The same-origin URL the legacy asset table serves the vendored Mermaid bundle at. */
export const VENDORED_MERMAID_URL = "/assets/vendor/mermaid.min.js";

/** Kept in lockstep with the `.mermaid text` font-size the design system paints. */
const LABEL_FONT_REM = 0.85;

/** The slice of the Mermaid UMD API this seam drives. */
interface MermaidApi {
  initialize: (config: Record<string, unknown>) => void;
  run: (opts: { nodes?: ArrayLike<Element>; querySelector?: string }) => Promise<unknown> | void;
}

// ── Theme detection ───────────────────────────────────────────────────────────

export type MermaidTheme = "dark" | "light";

/**
 * Read the effective theme from the DOM: the `data-theme` attribute on `:root`,
 * or the OS preference as the dark-first fallback (ADR-44).
 *
 * Mirrors `effectiveTheme()` in theme/theme.ts without importing the React module,
 * so this seam stays usable in plain-TS contexts (tests, workers).
 */
export function currentTheme(): MermaidTheme {
  const attr = document.documentElement.getAttribute("data-theme");
  if (attr === "light") return "light";
  if (attr === "dark") return "dark";
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

/**
 * Design-token-derived themeVariables for each theme.
 *
 * Passed to `mermaid.initialize({ theme: "base", themeVariables })` so Mermaid
 * bakes colors into the SVG as inline presentation attributes (CSP-safe — no
 * `<style>` injection, no `unsafe-inline` required). Values are the raw hex
 * equivalents of the semantic tokens in styles/tokens.css; raw hex is intentional
 * here because Mermaid's themeVariables API accepts only hex strings, not CSS
 * custom properties. This is the SINGLE place raw hex appears outside the token
 * file for Mermaid.
 */
export const THEME_VARS: Record<MermaidTheme, Record<string, string>> = {
  dark: {
    /* Surfaces from --neutral-9xx, text from --neutral-1xx/3xx */
    background: "#0f1216",        // --surface-0 (--neutral-950)
    mainBkg: "#171b21",           // --surface-1 (--neutral-900)
    primaryColor: "#1f242c",      // --surface-2 (--neutral-850) — node fill
    primaryTextColor: "#e8ebf0",  // --text-1    (--neutral-100)
    primaryBorderColor: "#2b313a",// --border-subtle (--neutral-700)
    secondaryColor: "#171b21",    // --surface-1
    tertiaryColor: "#1f242c",     // --surface-2
    textColor: "#e8ebf0",         // --text-1
    lineColor: "#aab2bf",         // --text-2 (--neutral-300)
    edgeLabelBackground: "#171b21",// --surface-1
    clusterBkg: "#1f242c",        // --surface-2
    titleColor: "#e8ebf0",        // --text-1
    nodeBorder: "#2b313a",        // --border-subtle
  },
  light: {
    /* Brand values from §1.2 (--so-merlin-50, --so-merlin, --so-muted, --light-border) */
    background: "#f4f4f2",        // --surface-0 (--so-merlin-50)
    mainBkg: "#ffffff",           // --surface-1
    primaryColor: "#ffffff",      // --surface-1 — node fill
    primaryTextColor: "#3d3935",  // --text-1 (--so-merlin)
    primaryBorderColor: "#e4e2dd",// --border-subtle (--light-border)
    secondaryColor: "#f4f4f2",    // --surface-0
    tertiaryColor: "#f4f4f2",     // --surface-0
    textColor: "#3d3935",         // --text-1
    lineColor: "#716b5d",         // --text-2 (--so-muted)
    edgeLabelBackground: "#ffffff",// --surface-1
    clusterBkg: "#f4f4f2",        // --surface-0
    titleColor: "#3d3935",        // --text-1
    nodeBorder: "#e4e2dd",        // --border-subtle
  },
};

// ── Bundle loading ────────────────────────────────────────────────────────────

function globalMermaid(): MermaidApi | undefined {
  return (window as unknown as { mermaid?: MermaidApi }).mermaid;
}

let loadPromise: Promise<MermaidApi | null> | null = null;

/** Load the vendored bundle once (memoized). Resolves `null` on a load failure so
 *  the caller leaves the diagram source visible rather than throwing. */
function loadMermaid(): Promise<MermaidApi | null> {
  if (loadPromise) return loadPromise;
  loadPromise = new Promise((resolve) => {
    const existing = globalMermaid();
    if (existing) {
      resolve(existing);
      return;
    }
    const script = document.createElement("script");
    script.src = VENDORED_MERMAID_URL;
    script.defer = true;
    script.addEventListener("load", () => resolve(globalMermaid() ?? null));
    script.addEventListener("error", () => resolve(null));
    document.head.appendChild(script);
  });
  return loadPromise;
}

// ── Initialization ────────────────────────────────────────────────────────────

/**
 * The last theme passed to `mermaid.initialize()`. `null` means Mermaid has not
 * been initialized yet this session. Re-initialization fires only when the theme
 * changes (one `initialize()` call per theme per bundle load), so the overhead is
 * negligible.
 */
let initializedTheme: MermaidTheme | null = null;

/**
 * Initialize Mermaid once per effective theme with the CSP-safe config and the
 * design-token-matched themeVariables. Mermaid still injects a per-render
 * `<style id="mermaid-XXXX">` block (unavoidable with any theme setting). In
 * production the self-only CSP blocks that block; `WikiView.module.css` provides
 * the color fallback via design tokens. See that file's comment block for the
 * full two-layer strategy (themeVariables for dev, external CSS for production).
 */
function initialize(mermaid: MermaidApi, theme: MermaidTheme): void {
  if (initializedTheme === theme) return;
  initializedTheme = theme;
  const config: Record<string, unknown> = {
    startOnLoad: false,
    securityLevel: "strict",
    htmlLabels: false,
    flowchart: { htmlLabels: false, useMaxWidth: true },
    theme: "base",
    themeVariables: THEME_VARS[theme],
  };
  try {
    const root = window.getComputedStyle(document.documentElement);
    const fontFamily = (root.getPropertyValue("--font-sans") || "").trim();
    const rootPx = parseFloat(root.fontSize);
    if (fontFamily) config.fontFamily = fontFamily;
    if (rootPx > 0) config.fontSize = LABEL_FONT_REM * rootPx;
  } catch {
    // getComputedStyle should never throw; fall through to Mermaid's own defaults.
  }
  mermaid.initialize(config);
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Render every `.mermaid` block within `container` into an inline SVG. A no-op when
 * the container has no diagram (so a diagram-free page never loads the bundle). A
 * load/parse failure is swallowed — the escaped diagram source stays visible
 * (FR-WK-15 progressive enhancement).
 *
 * The theme is read from the DOM at call time (`currentTheme()`) so diagrams always
 * reflect the active light/dark choice (ADR-44). When re-calling after a theme
 * toggle, the caller (WikiView.tsx) should first restore `.mermaid[data-processed]`
 * elements to their original source so Mermaid re-renders them cleanly.
 */
export async function renderMermaidIn(container: HTMLElement): Promise<void> {
  const nodes = container.querySelectorAll(".mermaid");
  if (nodes.length === 0) return;
  const mermaid = await loadMermaid();
  if (!mermaid) return;
  try {
    const theme = currentTheme();
    initialize(mermaid, theme);
    await mermaid.run({ nodes });
  } catch {
    // Leave the diagram source visible rather than breaking the page.
  }
}
