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
 * A ` ```mermaid ` fence is the one code block that renders as something other than
 * source: it branches into `MermaidBlock.tsx`, which draws it with the vendored
 * same-origin bundle (S-302, [FR-UI-32]). This renderer is shared by the finalized
 * answer body AND the S-301 Activity step results, so diagrams appear in both.
 *
 * Every value is a design token (`Chat.module.css`); nothing here injects a style
 * tag, so the byte-identical self-only CSP holds ([NFR-SE-06]).
 */

import type { ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import styles from "./Chat.module.css";
import { CopyControl } from "./CopyControl.tsx";
import { MermaidBlock } from "./MermaidBlock.tsx";

/** A fenced code block: a language label, a copy control, and the verbatim code.
 *  The copy control is best-effort — a blocked clipboard simply no-ops. */
function CodeBlock({ language, code }: { language: string | undefined; code: string }) {
  return (
    <div className={styles.codeBlock}>
      <div className={styles.codeHeader}>
        <span className={styles.codeLang}>{language ?? "code"}</span>
        <CopyControl text={code} />
      </div>
      <pre className={styles.pre}>
        <code>{code}</code>
      </pre>
    </div>
  );
}

/** react-markdown `code` renderer: inline code stays a bare `<code>`; a fenced
 *  block (has a `language-*` class, or spans multiple lines) becomes a
 *  {@link CodeBlock} — except a `language-mermaid` fence, which becomes a
 *  {@link MermaidBlock} diagram viewer (S-302, [FR-UI-32]). The default `pre`
 *  wrapper is collapsed (below) so the block is not nested inside a `<pre>`.
 *
 *  The mermaid branch keys off the fence's declared LANGUAGE, so a bare (unlabelled)
 *  fence that merely happens to contain diagram-ish text stays an ordinary code
 *  block; every other language keeps today's block and today's Copy control
 *  untouched. */
function CodeRenderer({ className, children }: { className?: string; children?: ReactNode }) {
  const raw = String(children ?? "");
  const isBlock = /language-/.test(className ?? "") || raw.includes("\n");
  if (!isBlock) return <code className={styles.inlineCode}>{children}</code>;
  const language = /language-(\w+)/.exec(className ?? "")?.[1];
  const code = raw.replace(/\n$/, "");
  if (language?.toLowerCase() === "mermaid") return <MermaidBlock code={code} />;
  return <CodeBlock language={language} code={code} />;
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
