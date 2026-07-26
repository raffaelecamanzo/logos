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
 * Theming reuses the wiki's two-layer strategy ([ADR-44]): the seam's
 * `themeVariables` bake design-token colours into the SVG in development, and the
 * token-driven `.mermaid*` fallback rules in `Chat.module.css` — served as a hashed
 * external `<link>`, token-only with no raw hex — supply the colours in production
 * where the CSP strips Mermaid's injected `<style>`. That fallback must stay in step
 * with `WikiView.module.css`: it carries the label `text-anchor` fix too, which is
 * layout rather than colour but is stripped by the very same mechanism.
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

/**
 * How long the fence text must hold still before a render is attempted.
 *
 * An answer streams token by token, and per CommonMark an *unterminated*
 * ` ```mermaid ` fence is already a `language-mermaid` code block — so this
 * component is handed a syntactically incomplete diagram on every delta. Rendering
 * each one would parse-fail, paint Mermaid's error graphic, and flash "could not be
 * rendered" over a diagram that is merely unfinished, while queueing one full
 * `mermaid.run()` per token. Letting the text settle first shows the growing source
 * (which is honest) and renders exactly once, when the fence is whole.
 */
const RENDER_SETTLE_MS = 120;

/**
 * Did Mermaid actually paint a diagram into `target`?
 *
 * `renderMermaidIn` resolves the same way whether the bundle loaded, whether it
 * parsed, and whether it did nothing at all, so the DOM is the only honest witness.
 * An unparseable diagram is the subtle case: Mermaid does not leave the element
 * empty, it paints its own "syntax error" bomb graphic — which is worse than the
 * source, since the source at least tells the user what the answer meant to draw.
 *
 * The error probe matches Mermaid's OWN graphic **exactly**: `path.error-icon` and
 * `text.error-text` are the only two error classes the vendored bundle emits, and
 * only on that graphic. A substring test over class attributes would also match
 * AUTHOR-chosen names, because Mermaid stamps them into the SVG — a
 * `classDef errorNode` lands on a node's class list, and every edge carries
 * `LS-<from> LE-<to>` built from the raw node ids. So `A --> error_handler` alone
 * would have condemned a perfectly good diagram to render as its own source.
 */
function renderedCleanly(target: HTMLElement): boolean {
  const svg = target.querySelector("svg");
  if (!svg) return false;
  if (svg.getAttribute("aria-roledescription") === "error") return false;
  return svg.querySelector("path.error-icon, text.error-text") === null;
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
  // Serializes render attempts for THIS target — see the effect for why overlapping
  // attempts cannot simply race.
  const inFlight = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    if (source) return;
    const host = hostRef.current;
    const target = targetRef.current;
    if (!host || !target) return;
    let cancelled = false;

    // Seed the target with the ESCAPED source. It is both the pre-render fallback
    // the user reads while the bundle loads and the clean slate Mermaid needs in
    // order to re-run — `data-processed` is its own "already rendered" latch, which
    // a theme toggle or a streamed edit must clear.
    const seed = () => {
      target.textContent = code;
      target.removeAttribute("data-processed");
    };
    seed();
    setFailed(false);

    // An empty fence has no diagram to draw AND no source to fall back on, so the
    // only thing a render attempt could produce is a failure note over a blank.
    const timer = code.trim()
      ? setTimeout(() => {
          // Chain onto any attempt still in flight rather than racing it.
          // `mermaid.run` SKIPS a node that already carries `data-processed`, so an
          // overlapping second attempt returns having drawn nothing, sees no SVG,
          // and reports failure — while the first attempt's render lands underneath
          // it. That left a correctly rendered diagram wearing a permanent "could
          // not be rendered" note (and, since the latch had been stripped, the
          // source-text chrome painted over the SVG). React's StrictMode
          // double-invokes this effect on mount, so it was not a rare interleaving.
          inFlight.current = inFlight.current
            .then(async () => {
              if (cancelled) return;
              seed();
              await renderMermaidIn(host);
              if (cancelled || renderedCleanly(target)) return;
              seed();
              setFailed(true);
            })
            // The seam swallows load and parse failures, but its bundle-load step
            // sits outside that guard — keep a rejection from surfacing as an
            // unhandled one, and keep the chain usable for the next attempt.
            .catch(() => {});
        }, RENDER_SETTLE_MS)
      : undefined;

    return () => {
      cancelled = true;
      if (timer !== undefined) clearTimeout(timer);
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
          <div
            className={`${styles.mermaidScale} ${styles[`zoom${zoom}` as keyof typeof styles]}`}
          >
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
