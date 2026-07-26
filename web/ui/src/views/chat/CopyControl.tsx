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
 */

import { useCallback, useEffect, useRef, useState } from "react";

import styles from "./Chat.module.css";

export function CopyControl({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  const copy = useCallback(() => {
    void navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setCopied(true);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {
        /* clipboard blocked — non-fatal */
      });
  }, [text]);

  return (
    <button type="button" className={styles.codeCopy} onClick={copy} aria-label="Copy code">
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
