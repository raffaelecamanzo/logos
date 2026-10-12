/*
 * The per-code-block Copy control (S-200, S-302, FR-UI-18, FR-UI-32).
 *
 * Extracted from `MarkdownAnswer.tsx`'s `CodeBlock` so the plain fenced block and
 * the Mermaid viewer (`MermaidBlock.tsx`) share ONE copy affordance — same label,
 * same transient "Copied" acknowledgement, same best-effort semantics — rather than
 * carrying two drifting copies of the clipboard dance. The Mermaid viewer passes the
 * RAW fence source in both its Diagram and Source modes, so what lands on the
 * clipboard is always the diagram source, never the rendered SVG.
 *
 * Best-effort by design: a blocked or absent clipboard no-ops instead of surfacing
 * an error the user cannot act on.
 *
 * S-620 (CR-204 C10, [FR-UI-45]) gives the same control a second face: the per-turn
 * action bar's Copy is a 24px ICON button (`variant="icon"`) that swaps its copy glyph
 * for a check for the same 1.5 s the code blocks say "Copied" for. One clipboard dance,
 * one timer, two looks — the action bar no longer needs the library's own Copy and its
 * separate timer.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { IconCheck, IconCopy } from "../../components/icons.tsx";
import styles from "./Chat.module.css";

/** How long the acknowledgement (the "Copied" label, the check glyph) stays up. */
const COPIED_MS = 1500;

export interface CopyControlProps {
  text: string;
  /** `code` (default): the text button on a code or diagram block. `icon`: the 24px
   *  icon button of the per-turn action bar. */
  variant?: "code" | "icon";
  /** The accessible name; the icon face also shows it as its tooltip. */
  label?: string;
}

export function CopyControl({ text, variant = "code", label = "Copy code" }: CopyControlProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The clipboard write is async, so a turn replaced by Regenerate can unmount this
  // control between the click and the resolution — after the cleanup has already run.
  // Without this latch the continuation would install a fresh timer nothing will ever
  // clear, and fire state onto an unmounted component.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const copy = useCallback(() => {
    void navigator.clipboard
      ?.writeText(text)
      .then(() => {
        if (!alive.current) return;
        setCopied(true);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), COPIED_MS);
      })
      .catch(() => {
        /* clipboard blocked — non-fatal */
      });
  }, [text]);

  if (variant === "icon") {
    return (
      <>
        <button
          type="button"
          className={styles.actionIcon}
          onClick={copy}
          disabled={!text}
          aria-label={label}
          title={label}
          data-copied={copied ? "true" : undefined}
        >
          {copied ? <IconCheck /> : <IconCopy />}
        </button>
        {/* The glyph swap is silent to a screen reader, and a button's children are
            presentational — so the acknowledgement is said once, beside it. */}
        <span className="sr-only" role="status">
          {copied ? "Copied" : ""}
        </span>
      </>
    );
  }

  return (
    <button type="button" className={styles.codeCopy} onClick={copy} aria-label={label}>
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
