/*
 * Verified code-reference links in chat answers (S-622, [FR-UI-47], [NFR-CC-04]).
 *
 * After a turn finalizes, the answer's backticked inline spans are sent in ONE
 * batched `GET /api/v1/code-refs` to the graph. Only a span the graph resolves —
 * an indexed file, or a name exactly one node carries — becomes a link to
 * `/graph?seed=…`. An ambiguous or unresolved span stays plain code: a link is
 * never inferred from what a span looks like ([NFR-CC-04]).
 *
 * The pieces, in the order a finalized answer meets them:
 *   - {@link remarkCollectInlineCode} reads the spans off the SAME markdown tree
 *     `react-markdown` renders, so fenced code, and code already inside a link,
 *     are excluded exactly as the renderer sees them — no second parser to drift;
 *   - {@link useIsFinalizedAnswer} decides whether this renderer instance is a
 *     finalized answer body (and not an Activity-fold step result) from the
 *     assistant-ui message it renders inside;
 *   - {@link useCodeRefLinks} issues the one request and maps each resolved span
 *     to its href. Requests are memoised per (member, span list), so a remount, a
 *     re-render or a thread reopened later never asks twice for the same turn.
 *
 * Member-addressed: the request goes through the shared `/api/v1` URL builder,
 * which carries the shell's member as `?repo=` in workspace mode. That is the
 * member every member read is addressed to, in the member chat and in the
 * Workspace Chat alike ([FR-WS-34]).
 */

import { useMessage, type MessageState } from "@assistant-ui/react";
import { useEffect, useState } from "react";

import { fetchCodeRefs } from "../../api/client.ts";
import type { CodeRefResolution } from "../../api/types.ts";
import { scopedMember } from "../../workspace/scope.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { graphSeedHref } from "../graph/graphModel.ts";
import type { TurnState } from "./chatModel.ts";

/** The most spans one finalized answer sends — the server's `CODE_REF_CAP`
 *  (`web/src/api_v1.rs`), which enforces it whatever a client sends. */
export const CODE_REF_CAP = 24;

/** The server's per-span length bound (`CODE_REF_MAX_LEN`). */
const CODE_REF_MAX_LEN = 200;

/**
 * Is a backticked span worth a lookup? A coarse pre-filter so the cap is spent on
 * plausible references, not `cargo test` or `--release`: one token, bounded, with
 * at least one letter or underscore. The SERVER classifies the shape and the graph
 * decides the link; this only keeps obvious non-references off the wire.
 */
export function isCodeRefCandidate(span: string): boolean {
  return span.length <= CODE_REF_MAX_LEN && /^[^\s-][^\s]*$/.test(span) && /[A-Za-z_]/.test(span);
}

/** The spans one answer sends: candidates only, first occurrence kept, capped. */
export function codeRefCandidates(spans: readonly string[]): string[] {
  const out: string[] = [];
  for (const span of spans) {
    if (out.length === CODE_REF_CAP) break;
    if (isCodeRefCandidate(span) && !out.includes(span)) out.push(span);
  }
  return out;
}

/** The slice of an mdast node the collector reads. */
interface MdNode {
  type: string;
  value?: string;
  children?: MdNode[];
}

/**
 * A remark plugin that records every inline-code span's text into `sink`, in
 * document order, each time the tree is parsed. It never changes the tree. Spans
 * inside a link are skipped: they are already links, and a second anchor inside
 * one would be invalid HTML.
 */
export function remarkCollectInlineCode(options: { sink: string[] }) {
  return (tree: MdNode) => {
    options.sink.length = 0;
    const walk = (node: MdNode) => {
      if (node.type === "inlineCode" && node.value !== undefined) options.sink.push(node.value);
      if (node.type === "link" || node.type === "linkReference") return;
      node.children?.forEach(walk);
    };
    walk(tree);
  };
}

/** Each resolved span's Graph-view href. Only `file` and `symbol` outcomes carry a
 *  seed and only they are mapped; everything else stays plain. */
export function linksFrom(resolution: CodeRefResolution): Map<string, string> {
  const links = new Map<string, string>();
  for (const ref of resolution.refs) {
    if (ref.seed === undefined) continue;
    if (ref.outcome === "file") links.set(ref.text, graphSeedHref(ref.seed, "file"));
    else if (ref.outcome === "symbol") links.set(ref.text, graphSeedHref(ref.seed));
  }
  return links;
}

/** Memoised resolutions, keyed by member and span list. Bounded, oldest first. */
const resolutions = new Map<string, Promise<Map<string, string>>>();
const RESOLUTION_MEMO_LIMIT = 200;

/**
 * Resolve `candidates` once per (member, span list). A failed request is kept as
 * failed — the spans stay plain — rather than retried, so a turn never issues a
 * second request.
 */
export function resolveCodeRefs(candidates: readonly string[]): Promise<Map<string, string>> {
  const key = JSON.stringify([scopedMember(), candidates]);
  const memo = resolutions.get(key);
  if (memo) return memo;
  const pending = fetchCodeRefs(candidates).then(linksFrom);
  resolutions.set(key, pending);
  if (resolutions.size > RESOLUTION_MEMO_LIMIT) {
    const oldest = resolutions.keys().next().value;
    if (oldest !== undefined) resolutions.delete(oldest);
  }
  return pending;
}

/** Forget every memoised resolution — for tests, which reuse span lists. */
export function clearCodeRefResolutions(): void {
  resolutions.clear();
}

const NO_LINKS: ReadonlyMap<string, string> = new Map();

/**
 * The span → href map for one rendered answer. `spans` is read after the render
 * that filled it (the effect runs after commit), and only while `enabled`, so a
 * streaming answer issues nothing and a finalized one issues at most one request.
 *
 * The links hold only for the member they were verified against ([NFR-CC-04]).
 * The Workspace Chat is app-scoped, so a shell member switch does not remount it:
 * the switch re-resolves the turn against the newly selected member — one request
 * per (turn, member), memoised, so switching back asks nothing — and until that
 * answer lands the spans render plain rather than keep another member's links.
 */
export function useCodeRefLinks(text: string, spans: readonly string[], enabled: boolean): ReadonlyMap<string, string> {
  const [resolved, setResolved] = useState<{ member: string | null; links: ReadonlyMap<string, string> }>({
    member: null,
    links: NO_LINKS,
  });
  // Subscribed for the re-render a member switch causes; the provider updates the
  // transport scope `scopedMember()` reads before it updates this context.
  useWorkspace();
  const member = scopedMember();
  useEffect(() => {
    if (!enabled) return;
    const candidates = codeRefCandidates(spans);
    if (candidates.length === 0) return;
    let live = true;
    resolveCodeRefs(candidates).then(
      (links) => {
        if (live) setResolved({ member, links });
      },
      () => {
        // An unreachable resolver leaves every span plain — never a guessed link.
      },
    );
    return () => {
      live = false;
    };
    // `spans` is a stable sink refilled on each parse; `text` is what changes it.
    // `member` keys the request (the memo key carries it too).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, text, member]);
  return enabled && resolved.member === member ? resolved.links : NO_LINKS;
}

/**
 * Is the renderer showing `text` the finalized answer body of the assistant turn
 * it sits in? Read from the assistant-ui message it renders inside — the turn the
 * chat runtime folded from the stream — so the chat view passes nothing new.
 * Outside a message (a bare render) it is `false`; an Activity-fold step result
 * is not the turn's answer, so it is `false` there too.
 */
export function useIsFinalizedAnswer(text: string): boolean {
  const turn = useMessage({
    optional: true,
    selector: (m: MessageState) => (m.metadata.custom as { turn?: TurnState } | undefined)?.turn,
  });
  return turn?.finalized === true && turn.answer === text;
}
