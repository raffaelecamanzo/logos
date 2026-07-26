/*
 * The chat Mermaid diagram viewer (S-302, [FR-UI-32], [FR-WK-15]).
 *
 * A ` ```mermaid ` fence in an assistant answer renders as a VISUAL diagram instead
 * of source text, by reusing the wiki's already-vendored, egress-audited Mermaid
 * bundle through the SAME render seam (`../wiki/mermaid.ts`). Nothing new is added:
 * no runtime dependency, no second bundle, no external origin — the seam loads
 * `/assets/vendor/mermaid.min.js` same-origin, on demand, only for an answer that
 * actually carries a diagram ([NFR-SE-01]).
 *
 * Because `MarkdownAnswer` is the one renderer behind BOTH the finalized answer body
 * and the S-301 Activity step results, branching there means diagrams reach both
 * surfaces with no second integration point.
 *
 * Three things this component owns, and why each is shaped the way it is:
 *
 *   - **Zoom is a CSS-CLASS LADDER, not an inline transform.** The served policy is
 *     `default-src 'self'` with no `style-src` override ([NFR-SE-06]), so a
 *     `style="transform: scale(…)"` attribute would be blocked exactly as Mermaid's
 *     injected `<style>` is. The scale steps therefore live in `Chat.module.css` as
 *     one class per level and the component only picks a class — the CSP stays
 *     byte-identical and no stylesheet is injected.
 *   - **The diagram target is managed IMPERATIVELY.** Mermaid replaces the
 *     element's content with an `<svg>`, so the `.mermaid` div is rendered with no
 *     React children and its text is written by the effect. React therefore never
 *     fights Mermaid over the same node, and the text is set via `textContent` —
 *     escaped by construction, no `innerHTML` and no `dangerouslySetInnerHTML`
 *     anywhere on this path.
 *   - **Failure keeps the SOURCE visible, never a blank.** The seam swallows a
 *     missing bundle and a parse error alike, so this component asks the DOM what
 *     actually happened and restores the escaped source when the render did not
 *     land ([FR-UI-32] progressive enhancement, [NFR-CC-04] honesty).
 *
 * Theming reuses the wiki's two-layer strategy verbatim ([ADR-44]): the seam's
 * `themeVariables` bake design-token colours into the SVG in development, and the
 * token-driven `.mermaid*` fallback rules in `Chat.module.css` — served as a hashed
 * external `<link>`, token-only with no raw hex — supply the colours in production
 * where the CSP strips Mermaid's injected `<style>`.
 */

import { useContext, useEffect, useRef, useState } from "react";

import { ThemeContext } from "../../theme/theme.ts";
import { renderMermaidIn } from "../wiki/mermaid.ts";
import styles from "./Chat.module.css";
import { CopyControl } from "./CopyControl.tsx";

/** The zoom ladder, in percent. Hand-rolled (no zoom/pan library — [FR-UI-32]);
 *  one CSS class per step, because an inline transform is CSP-blocked. */
const ZOOM_STEPS = [50, 67, 80, 100, 125, 150, 200, 250, 300] as const;

/** Index of the 100% step — where a diagram opens and where **Reset** returns. */
const RESET_STEP = ZOOM_STEPS.indexOf(100);

/** Percent → the `Chat.module.css` class carrying that `transform: scale(...)`. */
const ZOOM_CLASS: Record<number, string> = {
  50: styles.zoom50,
  67: styles.zoom67,
  80: styles.zoom80,
  100: styles.zoom100,
  125: styles.zoom125,
  150: styles.zoom150,
  200: styles.zoom200,
  250: styles.zoom250,
  300: styles.zoom300,
};

/**
 * Did Mermaid actually paint a diagram into `target`?
 *
 * `renderMermaidIn` resolves the same way whether the bundle loaded, whether it
 * parsed, and whether it did nothing at all, so the DOM is the only honest witness.
 * An unparseable diagram is the subtle case: Mermaid does not leave the element
 * empty, it paints its own "syntax error" bomb graphic — which is worse than the
 * source, since the source at least tells the user what the answer meant to draw.
 * Both of Mermaid's error markers are checked (the `aria-roledescription` on the
 * error SVG and its `error-icon` / `error-text` children) so either shape counts as
 * a failure.
 */
function renderedCleanly(target: HTMLElement): boolean {
  const svg = target.querySelector("svg");
  if (!svg) return false;
  if (svg.getAttribute("aria-roledescription") === "error") return false;
  return svg.querySelector('[class*="error"]') === null;
}

export function MermaidBlock({ code }: { code: string }) {
  const [source, setSource] = useState(false);
  const [step, setStep] = useState(RESET_STEP);
  const [failed, setFailed] = useState(false);
  // The container the seam scans for `.mermaid`, and the diagram target inside it.
  const hostRef = useRef<HTMLDivElement>(null);
  const targetRef = useRef<HTMLDivElement>(null);
  // Read the theme through the context OPTIONALLY rather than via `useTheme()`,
  // which throws outside a provider: the chat surface mounts under `ThemeProvider`
  // in the app (main.tsx), and a provider-less mount (a unit test rendering
  // `MarkdownAnswer` on its own) still renders its diagram once. As a dependency it
  // is what re-runs the render after a light/dark toggle so the new themeVariables
  // apply (ADR-44) — the same trigger `WikiPageBody` uses.
  const theme = useContext(ThemeContext)?.theme ?? null;

  useEffect(() => {
    if (source) return;
    const host = hostRef.current;
    const target = targetRef.current;
    if (!host || !target) return;
    let cancelled = false;

    // Seed the target with the ESCAPED source before every attempt. It is both the
    // pre-render fallback the user reads while the bundle loads and the clean slate
    // Mermaid needs in order to re-run — `data-processed` is its own "already
    // rendered" latch, which a theme toggle or a streamed edit must clear.
    target.textContent = code;
    target.removeAttribute("data-processed");
    setFailed(false);

    void renderMermaidIn(host).then(() => {
      if (cancelled) return;
      if (renderedCleanly(target)) return;
      target.textContent = code;
      target.removeAttribute("data-processed");
      setFailed(true);
    });

    return () => {
      cancelled = true;
    };
  }, [code, source, theme]);

  const zoom = ZOOM_STEPS[step];
  return (
    <div className={styles.mermaidBlock}>
      <div className={styles.codeHeader}>
        <span className={styles.codeLang}>mermaid</span>
        <div className={styles.mermaidControls}>
          {!source && (
            <>
              <button
                type="button"
                className={styles.mermaidControl}
                onClick={() => setStep((s) => Math.max(0, s - 1))}
                disabled={step === 0}
                aria-label="Zoom out"
              >
                −
              </button>
              <span className={styles.mermaidZoom}>{zoom}%</span>
              <button
                type="button"
                className={styles.mermaidControl}
                onClick={() => setStep((s) => Math.min(ZOOM_STEPS.length - 1, s + 1))}
                disabled={step === ZOOM_STEPS.length - 1}
                aria-label="Zoom in"
              >
                +
              </button>
              <button
                type="button"
                className={styles.mermaidControl}
                onClick={() => setStep(RESET_STEP)}
                disabled={step === RESET_STEP}
                aria-label="Reset zoom"
              >
                Reset
              </button>
            </>
          )}
          <button
            type="button"
            className={styles.mermaidControl}
            onClick={() => setSource((s) => !s)}
            aria-label={source ? "Show diagram" : "Show source"}
          >
            {source ? "Diagram" : "Source"}
          </button>
          {/* The RAW fence source in both modes — the diagram is never copyable
              as an SVG, which is not what "copy the code" means to the reader. */}
          <CopyControl text={code} />
        </div>
      </div>
      {source ? (
        <pre className={styles.pre}>
          <code>{code}</code>
        </pre>
      ) : (
        <div className={styles.mermaidViewport} ref={hostRef}>
          {/* `mermaid` is a deliberately GLOBAL class name (not a hashed module
              class): it is the selector the shared seam scans for. */}
          <div className={`${styles.mermaidScale} ${ZOOM_CLASS[zoom]}`}>
            <div className="mermaid" ref={targetRef} />
          </div>
          {failed && (
            <p className={styles.mermaidFallback}>
              This diagram could not be rendered — showing its source.
            </p>
          )}
        </div>
      )}
    </div>
  );
}
