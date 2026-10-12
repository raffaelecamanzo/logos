/*
 * Syntax highlighting for chat code blocks (S-622, [FR-UI-47], [ADR-44]).
 *
 * The `highlight.js` CORE with an explicit language list — never the full build,
 * which would bundle ~190 grammars. The list is the languages Logos indexes
 * (`logos languages`) plus the common config and shell ones an answer quotes.
 * A language the indexer reads that highlight.js does not ship (Terraform/HCL)
 * renders plain, like any unknown tag.
 *
 * CSP-clean by construction ([NFR-SE-06]): highlight.js runs as plain JS — no
 * wasm, no worker, no external origin — and emits `<span class="hljs-…">` only,
 * never a `style` attribute. That HTML is not injected either: {@link highlightTree}
 * parses it into a node tree the renderer turns into React elements, so the answer
 * stays React-escaped with no `dangerouslySetInnerHTML`, as `MarkdownAnswer.tsx`
 * promises. The colours come from the `--syntax-*` tokens in `tokens.css`, applied
 * by `CodeBlock.module.css`.
 */

import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import graphql from "highlight.js/lib/languages/graphql";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import makefile from "highlight.js/lib/languages/makefile";
import markdown from "highlight.js/lib/languages/markdown";
import php from "highlight.js/lib/languages/php";
import protobuf from "highlight.js/lib/languages/protobuf";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import scala from "highlight.js/lib/languages/scala";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

/** The bundled grammars, by highlight.js name. Each also answers to its own
 *  aliases (`rs`, `py`, `ts`/`tsx`, `sh`/`zsh`, `yml`, `toml`, `proto`, …). */
const LANGUAGES = {
  // The languages Logos indexes.
  rust,
  python,
  typescript,
  javascript,
  go,
  java,
  c,
  cpp,
  kotlin,
  csharp,
  ruby,
  php,
  scala,
  markdown,
  yaml,
  json,
  ini, // also `toml`
  dockerfile,
  makefile,
  bash, // also `sh`, `zsh`
  protobuf,
  graphql,
  sql,
  // Common config, shell and patch formats an answer quotes.
  shell, // a console session (`$ cargo test`)
  diff,
  xml, // also `html`, `svg`
  css,
} as const;

for (const [name, grammar] of Object.entries(LANGUAGES)) {
  hljs.registerLanguage(name, grammar);
}

/** The bundled language names — exported so a test can pin the list. */
export const BUNDLED_LANGUAGES: readonly string[] = Object.keys(LANGUAGES);

/** One node of a highlighted block: verbatim text, or a token span. */
export type HighlightNode = string | { className: string; children: HighlightNode[] };

/**
 * Highlight `code` as `language`, or `null` when the block should render plain: no
 * language, one this bundle does not carry, or output this parser does not
 * recognise. Never throws — a plain block is the honest fallback.
 */
export function highlightTree(code: string, language: string | undefined): HighlightNode[] | null {
  if (!language || !hljs.getLanguage(language)) return null;
  try {
    return parseHighlightHtml(hljs.highlight(code, { language, ignoreIllegals: true }).value);
  } catch {
    return null;
  }
}

const ENTITIES: Record<string, string> = {
  "&amp;": "&",
  "&lt;": "<",
  "&gt;": ">",
  "&quot;": '"',
  "&#x27;": "'",
  "&#39;": "'",
};

/** A class list highlight.js may emit: `hljs-keyword`, `hljs-title function_`,
 *  `language-xml`. Anything else is not its output. */
const CLASS_LIST = /^[A-Za-z0-9_-]+( [A-Za-z0-9_-]+)*$/;

/**
 * Parse highlight.js's HTML into a node tree. Its output is a closed grammar: text
 * with `& < > " '` escaped as entities, and `<span class="…">`/`</span>` pairs —
 * nothing else. Anything outside that grammar (another tag, an unknown entity, an
 * unbalanced span) returns `null`, so the block renders plain rather than guessed.
 */
export function parseHighlightHtml(html: string): HighlightNode[] | null {
  const root: HighlightNode[] = [];
  const stack: HighlightNode[][] = [root];
  const token = /<span class="([^"]*)">|<\/span>|([^<&]+)|(&[#A-Za-z0-9]+;)/gy;
  let at = 0;
  while (at < html.length) {
    token.lastIndex = at;
    const m = token.exec(html);
    if (!m) return null;
    at = token.lastIndex;
    const top = stack[stack.length - 1];
    if (m[1] !== undefined) {
      if (!CLASS_LIST.test(m[1])) return null;
      const span: HighlightNode = { className: m[1], children: [] };
      top.push(span);
      stack.push(span.children);
    } else if (m[0] === "</span>") {
      if (stack.length === 1) return null;
      stack.pop();
    } else {
      const text = m[2] ?? ENTITIES[m[3]];
      if (text === undefined) return null;
      const last = top[top.length - 1];
      if (typeof last === "string") top[top.length - 1] = last + text;
      else top.push(text);
    }
  }
  return stack.length === 1 ? root : null;
}
