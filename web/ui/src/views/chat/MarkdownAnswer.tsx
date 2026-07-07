/*
 * Markdown rendering for assistant answers (S-200, CR-051, FR-UI-18, ADR-45).
 *
 * `react-markdown` builds a React element tree from the answer text — it never
 * uses `dangerouslySetInnerHTML`, so rendered answers stay React-escaped and
 * CSP-clean (no inline `<style>`/`<script>`, no `eval`). GFM (tables, strikethrough,
 * task lists, autolinks) comes from `remark-gfm`. Fenced code blocks render with a
 * language label and a per-block copy control; the message-level copy control is
 * assistant-ui's `ActionBarPrimitive.Copy` (see `ChatView.tsx`).
 *
 * Every value is a design token (`Chat.module.css`); nothing here injects a style
 * tag, so the byte-identical self-only CSP holds ([NFR-SE-06]).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import styles from "./Chat.module.css";

/** A fenced code block: a language label, a copy control, and the verbatim code.
 *  The copy control is best-effort — a blocked clipboard simply no-ops. */
function CodeBlock({ language, code }: { language: string | undefined; code: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  const copy = useCallback(() => {
    void navigator.clipboard
      ?.writeText(code)
      .then(() => {
        setCopied(true);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {
        /* clipboard blocked — non-fatal */
      });
  }, [code]);

  return (
    <div className={styles.codeBlock}>
      <div className={styles.codeHeader}>
        <span className={styles.codeLang}>{language ?? "code"}</span>
        <button type="button" className={styles.codeCopy} onClick={copy} aria-label="Copy code">
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className={styles.pre}>
        <code>{code}</code>
      </pre>
    </div>
  );
}

/** react-markdown `code` renderer: inline code stays a bare `<code>`; a fenced
 *  block (has a `language-*` class, or spans multiple lines) becomes a
 *  {@link CodeBlock}. The default `pre` wrapper is collapsed (below) so the block
 *  is not nested inside a `<pre>`. */
function CodeRenderer({ className, children }: { className?: string; children?: ReactNode }) {
  const raw = String(children ?? "");
  const isBlock = /language-/.test(className ?? "") || raw.includes("\n");
  if (!isBlock) return <code className={styles.inlineCode}>{children}</code>;
  const language = /language-(\w+)/.exec(className ?? "")?.[1];
  return <CodeBlock language={language} code={raw.replace(/\n$/, "")} />;
}

/** Render an assistant answer as GFM markdown. Links open in a new tab with
 *  `noopener` (an answer may cite an external URL; the click is a top-level
 *  navigation, not a CSP-governed fetch). */
export function MarkdownAnswer({ text }: { text: string }) {
  return (
    <div className={styles.markdown}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          pre: ({ children }) => <>{children}</>,
          code: CodeRenderer,
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer noopener">
              {children}
            </a>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
