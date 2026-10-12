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
 * A GFM table (S-311, [FR-UI-31], [FR-UI-32]) renders inside a scroll wrapper that
 * shares the fenced code block's chrome (`.codeBlock, .mermaidBlock, .tableWrapper`
 * in `Chat.module.css`) so a table wider than the column scrolls on its own axis
 * without widening the transcript; the table itself takes the §5 house table voice
 * from `.markdown table/th/td`, not a class here — same renderer, same both surfaces.
 *
 * A fenced block in a language the bundle carries is syntax-highlighted (S-622,
 * [FR-UI-47]) by the `highlight.js` core in `highlight.ts`: class-only token spans,
 * parsed into React elements rather than injected, coloured by the `--syntax-*`
 * tokens through `CodeBlock.module.css`. An unknown or absent language renders the
 * verbatim code, unhighlighted. The language label and Copy control are unchanged.
 *
 * After a turn finalizes, its backticked inline spans are resolved against the
 * graph in one request, and only the spans the graph verifies become links to the
 * Graph view (S-622, [FR-UI-47], [NFR-CC-04]; `codeRefs.ts`). The Activity-fold
 * step results are highlighted the same way but never resolved.
 *
 * Every value is a design token (`Chat.module.css`, `CodeBlock.module.css`);
 * nothing here injects a style tag or a `style` attribute, so the byte-identical
 * self-only CSP holds ([NFR-SE-06]).
 */

import { createContext, useContext, useMemo, useRef, type MouseEvent, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { navigate } from "../../router.tsx";
import { scopedMember, urlWithMember } from "../../workspace/scope.ts";
import styles from "./Chat.module.css";
import codeStyles from "./CodeBlock.module.css";
import { remarkCollectInlineCode, useCodeRefLinks, useIsFinalizedAnswer } from "./codeRefs.ts";
import { CopyControl } from "./CopyControl.tsx";
import { highlightTree, type HighlightNode } from "./highlight.ts";
import { MermaidBlock } from "./MermaidBlock.tsx";

/** Render a highlight tree as React elements: text stays text, each token is a
 *  `<span>` carrying highlight.js's class list and nothing else. */
function renderHighlight(nodes: HighlightNode[]): ReactNode[] {
  return nodes.map((node, i) =>
    typeof node === "string" ? (
      node
    ) : (
      <span key={i} className={node.className}>
        {renderHighlight(node.children)}
      </span>
    ),
  );
}

/** A fenced code block: a language label, a copy control, and the code —
 *  highlighted when the bundle carries its language, verbatim otherwise. The copy
 *  control is best-effort — a blocked clipboard simply no-ops — and always copies
 *  the verbatim source. */
function CodeBlock({ language, code }: { language: string | undefined; code: string }) {
  const tree = useMemo(() => highlightTree(code, language), [code, language]);
  return (
    <div className={styles.codeBlock}>
      <div className={styles.codeHeader}>
        <span className={styles.codeLang}>{language ?? "code"}</span>
        <CopyControl text={code} />
      </div>
      <pre className={styles.pre}>
        <code className={codeStyles.code}>{tree ? renderHighlight(tree) : code}</code>
      </pre>
    </div>
  );
}

/** The verified span → Graph-view href map of the answer being rendered (empty
 *  until the one resolution request answers, and always empty outside a
 *  finalized answer). */
const CodeRefLinks = createContext<ReadonlyMap<string, string>>(new Map());
/** True inside a markdown link, where a span is never linked a second time. */
const InsideLink = createContext(false);

/** An inline code span: a link into the Graph view when the graph verified it,
 *  plain code otherwise. A plain click navigates in place; a modified click (new
 *  tab or window) is left to the browser, whose href carries the member. */
function InlineCode({ children }: { children?: ReactNode }) {
  const links = useContext(CodeRefLinks);
  const insideLink = useContext(InsideLink);
  const code = <code className={styles.inlineCode}>{children}</code>;
  const href = insideLink ? undefined : links.get(String(children ?? ""));
  if (!href) return code;
  const onClick = (e: MouseEvent<HTMLAnchorElement>) => {
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    navigate(href);
  };
  return (
    <a href={urlWithMember(href, scopedMember())} title="Open in the Graph view" onClick={onClick}>
      {code}
    </a>
  );
}

/** A GFM table: wrapped in a scroll box (`min-width: 0`, `overflow-x: auto` in
 *  Chat.module.css) so a table wider than the conversation column scrolls inside
 *  its own wrapper while the column and the page do not (S-311, [FR-UI-31]). The
 *  house voice (header, row rules, hover) is declared entirely in the stylesheet
 *  against `.markdown table/th/td` — no class or content inspection here, so no
 *  column is re-typed from its content. */
function TableRenderer({ children }: { children?: ReactNode }) {
  return (
    <div className={styles.tableWrapper}>
      <table>{children}</table>
    </div>
  );
}

/** react-markdown `code` renderer: inline code is an {@link InlineCode} span (a
 *  bare `<code>`, or a Graph-view link when the graph verified it); a fenced
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
  if (!isBlock) return <InlineCode>{children}</InlineCode>;
  // The whole tag, not its leading word: `c#` and `c++` are highlight.js aliases
  // of their own grammars, which `\w+` would read as `c` (S-622).
  const language = /language-(\S+)/.exec(className ?? "")?.[1];
  const code = raw.replace(/\n$/, "");
  if (language?.toLowerCase() === "mermaid") return <MermaidBlock code={code} />;
  return <CodeBlock language={language} code={code} />;
}

/** A markdown link. Opens in a new tab with `noopener` (an answer may cite an
 *  external URL; the click is a top-level navigation, not a CSP-governed fetch). */
function LinkRenderer({ href, children }: { href?: string; children?: ReactNode }) {
  return (
    <a href={href} target="_blank" rel="noreferrer noopener">
      <InsideLink.Provider value={true}>{children}</InsideLink.Provider>
    </a>
  );
}

const COMPONENTS = {
  pre: ({ children }: { children?: ReactNode }) => <>{children}</>,
  code: CodeRenderer,
  table: TableRenderer,
  a: LinkRenderer,
};

/** Render an assistant answer as GFM markdown. When it is a finalized answer
 *  body, its graph-verified code references link to the Graph view. */
export function MarkdownAnswer({ text }: { text: string }) {
  const finalized = useIsFinalizedAnswer(text);
  // Refilled by the collector on every parse of `text`; read after commit.
  const spans = useRef<string[]>([]).current;
  const remarkPlugins = useMemo(
    () => [remarkGfm, [remarkCollectInlineCode, { sink: spans }] as [typeof remarkCollectInlineCode, { sink: string[] }]],
    [spans],
  );
  const links = useCodeRefLinks(text, spans, finalized);
  return (
    <div className={styles.markdown}>
      <CodeRefLinks.Provider value={links}>
        <ReactMarkdown remarkPlugins={remarkPlugins} components={COMPONENTS}>
          {text}
        </ReactMarkdown>
      </CodeRefLinks.Provider>
    </div>
  );
}
