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
 *   - **CSP-safe theming via themeVariables + adopted stylesheet + CSS fallback
 *     (S-196, CR-051, HF-3).** `mermaid.initialize({ theme: "base", themeVariables })`
 *     pre-populates Mermaid's per-render `<style id="mermaid-XXXX">` block with
 *     design-token hex values. That `<style>` is an element Mermaid inserts into the
 *     rendered SVG, so the self-only CSP blocks it outright (not merely
 *     out-specificitied) — confirmed live on 1.4.24 (236 CSP violations on one page,
 *     every non-flowchart shape and the flowchart cylinder painted solid black).
 *     `adoptMermaidStyleFor` (below) copies that blocked `<style>`'s text into a
 *     constructable `CSSStyleSheet` and adds it to `document.adoptedStyleSheets` —
 *     a CSSOM-constructed sheet is not an inline style and the CSP does not touch
 *     it. Mermaid's inline `style="…"` attributes (a self-message curve's
 *     `fill: none`, label font sizes) are dropped by the same CSP;
 *     `replayMermaidInlineStyles` re-applies them through `el.style`, a CSSOM write
 *     the CSP also does not touch. The external CSS Module fallback rules in `WikiView.module.css` (served as
 *     a hashed `<link>`, ADR-44) remain as a second layer: they cover the frame
 *     before adoption lands and the jsdom test environment, which does not support
 *     constructable stylesheets. Only the flowchart shapes those fallback rules
 *     enumerate are covered by that second layer — everything else (classDiagram,
 *     sequence, ER) depends on the adopted sheet.
 *   - **Theme-aware (ADR-44).** `currentTheme()` reads the `data-theme` attribute on
 *     `:root` (or OS preference as fallback) so diagrams follow the app's dark/light
 *     toggle. Re-initialization is triggered only when the effective theme changes.
 *   - **Safety preserved.** `securityLevel: "strict"` + `htmlLabels: false` mirror
 *     the legacy `mermaid-init.js`: diagram text is escaped, labels are SVG `<text>`
 *     (no `<foreignObject>` the CSP would strip), and the layout font is aligned with
 *     the painted `--font-sans` so boxes size to their labels.
 *   - **Progressive enhancement.** A load or parse failure leaves the escaped diagram
 *     source visible (never a blank) — the same honesty contract as the legacy.
 *   - **Sequence-diagram ';' repair (HF-2).** Mermaid's `sequenceDiagram` grammar
 *     treats a bare `;` in message/note text as a statement separator and rejects
 *     the whole diagram. `repairSequenceDiagramSource` (below) rewrites it to
 *     Mermaid's own `#59;` escape before render — only on this seam's DOM copy,
 *     never on the caller's own source (Source toggle, copy) — so a model-written
 *     `;` no longer takes down the entire diagram.
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
 * fills its per-render `<style>` block with these colors. The CSP blocks that
 * block as an injected element; `adoptMermaidStyleFor` re-applies it as a
 * constructable stylesheet (HF-3), with no `unsafe-inline`. Values are the raw hex
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
 * production the self-only CSP blocks that block as an element, so
 * `renderMermaidIn` adopts its text as a constructable stylesheet after the render
 * (HF-3); `WikiView.module.css` supplies a design-token fallback for the frame
 * before that. The module header describes all three layers.
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
 * Before handing anything to Mermaid, each node's text is passed through
 * `repairSequenceDiagramSource` (HF-2): a bare `;` in a sequenceDiagram message or
 * note is Mermaid's own statement separator, so the model text it appears in
 * rejects the WHOLE diagram. The repair only ever touches this DOM copy — the
 * caller's own source (React state, the Source toggle, copy) is untouched.
 *
 * The theme is read from the DOM at call time (`currentTheme()`) so diagrams always
 * reflect the active light/dark choice (ADR-44). When re-calling after a theme
 * toggle, the caller (WikiView.tsx) should first restore `.mermaid[data-processed]`
 * elements to their original source so Mermaid re-renders them cleanly.
 *
 * Also adopts each rendered `.mermaid` element's constructable stylesheet (HF-3) —
 * replacing any sheet already adopted for that same element, so a re-render never
 * stacks sheets. The caller owns the other half of that lifecycle: when a
 * `.mermaid` element unmounts for good, call `unadoptMermaidStyleFor` on it, or its
 * sheet stays adopted forever.
 */
export async function renderMermaidIn(container: HTMLElement): Promise<void> {
  const nodes = container.querySelectorAll(".mermaid");
  if (nodes.length === 0) return;
  for (const node of nodes) {
    const original = node.textContent;
    if (original === null) continue;
    const repaired = repairSequenceDiagramSource(original);
    if (repaired !== original) node.textContent = repaired;
  }
  const mermaid = await loadMermaid();
  if (!mermaid) return;
  try {
    const theme = currentTheme();
    initialize(mermaid, theme);
    await mermaid.run({ nodes });
    // A target can be detached while this call was awaiting `mermaid.run` (or,
    // on the session's first diagram, the vendored-bundle fetch inside
    // `loadMermaid()`) — an unmount, a page navigation, or a theme re-render
    // racing ahead of this one. Its cleanup already ran and found nothing to
    // unadopt, so adopting for it now would leak the sheet for the page's
    // lifetime (review-fix, HF-3).
    for (const node of nodes) {
      if (!node.isConnected) continue;
      adoptMermaidStyleFor(node);
      replayMermaidInlineStyles(node);
    }
  } catch {
    // Leave the diagram source visible rather than breaking the page.
  }
}

// ── CSP-safe styling via constructable stylesheets ────────────────────────────

/**
 * The constructable stylesheet currently adopted for each rendered diagram,
 * keyed by its `.mermaid` mount element (stable across a theme-toggle
 * re-render — only its contents are replaced). A `WeakMap` lets an unmounted
 * target's entry drop once nothing else references the element.
 */
const adoptedSheets = new WeakMap<Element, CSSStyleSheet>();

/**
 * Copy `target`'s freshly-rendered `<style>` (the first child of the `<svg>`
 * Mermaid just drew — CSP-blocked as an injected element) into a constructable
 * `CSSStyleSheet` and add it to `document.adoptedStyleSheets`. CSSOM-constructed
 * sheets are not inline styles, so `default-src 'self'` does not block them
 * (proven against the served CSP — see the module header).
 *
 * Mermaid scopes every rule in that `<style>` to the diagram's own `#mermaid-…`
 * id, so adopting many diagrams' sheets side by side cannot let one diagram
 * restyle another.
 *
 * Replaces (never stacks) `target`'s previous sheet, so a theme toggle's
 * re-render does not accumulate stale sheets for the same target.
 */
function adoptMermaidStyleFor(target: Element): void {
  const css = target.querySelector("svg > style")?.textContent;
  if (!css) return;
  if (typeof CSSStyleSheet === "undefined" || !("adoptedStyleSheets" in document)) return;
  let sheet: CSSStyleSheet;
  try {
    sheet = new CSSStyleSheet();
    sheet.replaceSync(css);
  } catch {
    // Older engines / jsdom: leave the CSS-Module fallback as the only styling.
    return;
  }
  unadoptMermaidStyleFor(target);
  adoptedSheets.set(target, sheet);
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
}

/**
 * Re-apply every inline `style="…"` attribute inside `target`'s rendered `<svg>`
 * through the CSSOM (`el.style.cssText`), returning how many were replayed.
 *
 * Mermaid writes part of its styling as inline style attributes — e.g.
 * `fill: none` on a sequence diagram's self-message curve, and each label's
 * font size. The self-only CSP drops those attributes exactly as it drops the
 * injected `<style>`, so a self-message rendered as a filled blob and labels
 * lost their sizes. Setting `el.style` is a CSSOM write, which the CSP does not
 * restrict (proven on the served CSP, Sprint 79 test execution: 106 attributes
 * replayed on one sequence diagram, every self-message blob gone).
 */
export function replayMermaidInlineStyles(target: Element): number {
  let replayed = 0;
  for (const el of target.querySelectorAll<SVGElement | HTMLElement>("svg[style], svg [style]")) {
    const declared = el.getAttribute("style");
    if (!declared) continue;
    el.style.cssText = declared;
    replayed += 1;
  }
  return replayed;
}

// ── Sequence-diagram ';' repair (HF-2) ────────────────────────────────────────

/**
 * Is `source`'s first non-empty, non-comment, non-frontmatter/directive line
 * exactly `sequenceDiagram`? Gates `repairSequenceDiagramSource` — every other
 * diagram type passes through untouched.
 */
function isSequenceDiagram(source: string): boolean {
  const lines = source.split("\n");
  let i = 0;
  if (lines[i]?.trim() === "---") {
    // YAML frontmatter block — skip to its closing '---'.
    i++;
    while (i < lines.length && lines[i].trim() !== "---") i++;
    i++;
  }
  while (i < lines.length) {
    const trimmed = lines[i].trim();
    if (trimmed === "" || trimmed.startsWith("%%")) {
      // Blank line, a `%% comment`, or a `%%{init: ...}%%` directive.
      i++;
      continue;
    }
    return trimmed === "sequenceDiagram";
  }
  return false;
}

/** Matches a `Note left of|right of|over <participants>:` line. Group 1 is the
 *  `Note ... :` head (left untouched); group 2 is the note text. */
const NOTE_LINE_RE = /^(\s*Note\s+(?:left of|right of|over)\s+[^:]*:)([\s\S]*)$/;

/** Any Mermaid sequence-message arrow token (with an optional `+`/`-` activation
 *  marker handled by the caller — this only needs to detect the arrow itself). */
const ARROW_TOKEN_RE = /(<<->>|-->>|->>|-->|->|--x|-x|--\)|-\))/;

/** Replace every bare `;` with Mermaid's own literal-semicolon escape (`#59;`),
 *  leaving an already-escaped `#59;` alone (idempotent). */
function escapeSemicolons(text: string): string {
  return text.replace(/#59;|;/g, (m) => (m === ";" ? "#59;" : m));
}

/**
 * Repair one line of a sequenceDiagram source: a message line (`A->>B: text`,
 * any arrow form, with or without an activation marker) or a
 * `Note left of|right of|over …: text` line has every `;` in its text — the
 * part after the FIRST `:` — replaced with `#59;`. Every other line (a
 * participant/actor declaration, a keyword line, a comment, a line with no
 * colon) is returned unchanged.
 *
 * A `%% comment` is excluded explicitly, not merely by missing both patterns:
 * a comment can itself contain an arrow-token substring before a colon (e.g.
 * `%% A->>B: note this`), which would otherwise be misclassified as a message
 * line and mutated — breaking the "comments pass through byte-identical"
 * guarantee (review-fix, HF-2).
 */
function repairSequenceDiagramLine(line: string): string {
  if (line.trim().startsWith("%%")) return line;

  const noteMatch = line.match(NOTE_LINE_RE);
  if (noteMatch) return noteMatch[1] + escapeSemicolons(noteMatch[2]);

  const colonIdx = line.indexOf(":");
  if (colonIdx === -1) return line;
  const head = line.slice(0, colonIdx);
  if (!ARROW_TOKEN_RE.test(head)) return line;
  return line.slice(0, colonIdx + 1) + escapeSemicolons(line.slice(colonIdx + 1));
}

/**
 * Repair a Mermaid diagram's source so a bare `;` in sequenceDiagram message or
 * note text does not reject the whole diagram (HF-2).
 *
 * Mermaid's `sequenceDiagram` grammar treats `;` as a statement separator, so a
 * model-written `Note over SPA: ...only;<br/>separate GET ...` line splits into
 * an invalid second statement and Mermaid rejects the entire diagram. Mermaid's
 * own entity escape, `#59;`, renders as a literal semicolon — so this rewrites
 * every bare `;` in message/note TEXT (the part after the first `:`) to that
 * escape before the source ever reaches `mermaid.parse`/`mermaid.run`.
 *
 * A pure, DOM-free function so it can be unit-tested directly. Non-sequence
 * diagrams (gated by `isSequenceDiagram`) pass through byte-identical, as do
 * participant/actor declarations, keyword lines (loop/alt/opt/par/critical/
 * break/rect/end/else/and/autonumber/activate/deactivate), comments, and any
 * `;` already escaped as `#59;`.
 */
export function repairSequenceDiagramSource(source: string): string {
  if (!isSequenceDiagram(source)) return source;
  return source
    .split("\n")
    .map(repairSequenceDiagramLine)
    .join("\n");
}

/**
 * Remove `target`'s adopted stylesheet, if one was adopted, from
 * `document.adoptedStyleSheets`.
 *
 * Callers MUST call this when a diagram's mount element unmounts for good
 * (not merely re-rendering in place) — otherwise a long-lived conversation or
 * wiki session keeps growing `document.adoptedStyleSheets` by one sheet per
 * diagram ever shown, none of them ever released.
 */
export function unadoptMermaidStyleFor(target: Element): void {
  const sheet = adoptedSheets.get(target);
  if (!sheet) return;
  adoptedSheets.delete(target);
  document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== sheet);
}
