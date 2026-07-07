/*
 * Chat view model (S-190, CR-049, FR-UI-18, FR-UI-19, FR-UI-23) — the PURE half of
 * the migrated Chat tab: the SSE wire types, the SSE-block parser, the per-turn
 * event reducer (plan / subagent-activity / answer-token / final-answer / honest
 * halt / honest error), and the consent + endpoint-disclosure helpers.
 *
 * It holds NO React and NO network: the orchestrator's SSE contract is unchanged
 * ([chat-agent]) — this is a re-homed client. Keeping the reducer pure makes every
 * branch (incl. `halted`/`error`) deterministically testable without a DOM or a
 * fetch, and lets the component (`ChatView.tsx`) stay a thin renderer over it.
 *
 * The masked chat key is NEVER referenced here — the consent banner discloses only
 * the provider, the configured model, and the endpoint host (NFR-SE-07).
 */

// ── SSE wire types (mirror chat-agent's `OrchestratorEvent`, serde-tagged on
//    `event`, `rename_all = "snake_case"`; `chat-agent/src/orchestrator/event.rs`,
//    `plan.rs`, `budget.rs`). Internal to the bundled frontend, not a public API. ──

/** A subagent role wire token (mirrors `StepRole`, snake_case). */
export type StepRole = "graph_navigator" | "governance_analyst" | "source_reader" | "synthesizer";

/** One planned step (mirrors `PlanStep`). */
export interface PlanStep {
  role: StepRole;
  instruction: string;
}

/** A budget-tree bound (mirrors `BudgetBound`, serde-tagged on `bound`). */
export type BudgetBound =
  | { bound: "global_tool_calls"; limit: number }
  | { bound: "subagent_tool_calls"; limit: number }
  | { bound: "replans"; limit: number };

/** A streamed orchestrator transition (mirrors `OrchestratorEvent`). */
export type OrchestratorEvent =
  | { event: "plan"; round: number; steps: PlanStep[] }
  | { event: "step_started"; index: number; role: StepRole; instruction: string }
  | { event: "step_observed"; index: number; role: StepRole; summary: string }
  | { event: "halted"; round: number; bound: BudgetBound }
  | { event: "answer_delta"; delta: string }
  | { event: "final_answer"; answer: string };

// ── Config read-model (the slice the consent banner needs) ────────────────────
// A focused mirror of the chat-relevant fields of `ConfigReadModel`
// (`logos-core/src/config/writeback.rs`) the `GET /api/v1/config` endpoint
// serializes. The full read-model + its typed fetch belong to the Config view
// (S-191); chat reads only what its consent banner discloses, so it mirrors only
// that slice here to stay independent of the parallel Config migration.
//
// `chat_key` is the MASKED secret (presence + last-4 only — masked by construction
// server-side); the chat surface reads `present` for the configured-state gate and
// NEVER renders the key material (NFR-SE-07).

/** The `[chat]` policy slice (mirrors `ChatConfig`). */
export interface ChatPolicy {
  provider: "anthropic" | "openai";
  model: string | null;
  base_url: string;
  max_tool_calls: number;
  max_subagent_tool_calls: number;
  max_replans: number;
}

/** The chat-relevant slice of `ConfigReadModel` (`GET /api/v1/config`). */
export interface ChatConfigReadModel {
  config: { parsed: { chat: ChatPolicy } };
  /** The MASKED chat key — presence + last-4 only; never rendered (NFR-SE-07). */
  chat_key: { present: boolean; last4?: string | null };
}

/** The native Anthropic endpoint host disclosed for the `anthropic` provider —
 *  mirrors the server view's `host_of(DEFAULT_ANTHROPIC_BASE_URL)` (web/src/views/chat.rs). */
export const ANTHROPIC_HOST = "api.anthropic.com";

/** Is chat usable? A configured model AND a present key (mirrors the server view's
 *  `configured` predicate). A model with no key is still configure-first. */
export function isConfigured(model: ChatConfigReadModel): boolean {
  const chat = model.config.parsed.chat;
  return chat.model != null && chat.model.trim() !== "" && model.chat_key.present;
}

/** Extract the host authority from a URL — the run between `://` and the next `/`,
 *  falling back to the trimmed input when it carries no scheme (so a misconfigured
 *  `base_url` is shown honestly rather than blanked). Mirrors `views::chat::host_of`. */
export function hostOf(url: string): string {
  const afterScheme = url.includes("://") ? url.slice(url.indexOf("://") + 3) : url;
  const host = afterScheme.split("/")[0].trim();
  return host === "" ? url.trim() : host;
}

/** The endpoint host named in the consent banner (NFR-SE-07): the native Anthropic
 *  host for the `anthropic` provider, else the OpenAI-compatible `base_url`'s host. */
export function endpointHost(chat: ChatPolicy): string {
  return chat.provider === "anthropic" ? ANTHROPIC_HOST : hostOf(chat.base_url);
}

// ── Display labels (ported verbatim from the legacy chat.js client) ───────────

const ROLE_LABELS: Record<string, string> = {
  graph_navigator: "Graph-Navigator",
  governance_analyst: "Governance-Analyst",
  source_reader: "Source-Reader",
  synthesizer: "Synthesizer",
};

/** Map a wire role to its display label; an unknown role is shown verbatim. */
export function roleLabel(role: string): string {
  return ROLE_LABELS[role] ?? role ?? "subagent";
}

/** An honest, named halt note for a budget-tree bound ([NFR-CC-04]). */
export function boundNote(bound: BudgetBound | undefined): string {
  if (!bound) return "the turn halted at a budget bound";
  if (bound.bound === "global_tool_calls") {
    return `halted: the global per-turn tool-call ceiling was reached (${bound.limit} calls)`;
  }
  if (bound.bound === "subagent_tool_calls") {
    return `halted: a subagent reached its per-subagent tool-call cap (${bound.limit} calls)`;
  }
  if (bound.bound === "replans") {
    return `halted: the planner reached the max-replans bound (${bound.limit} replans)`;
  }
  return "the turn halted at a budget bound";
}

// ── SSE block parsing (mirrors chat.js `parseBlock`) ──────────────────────────

/** One parsed SSE frame: the `event:` name and the joined `data:` payload. */
export interface SseFrame {
  name: string;
  data: string;
}

/**
 * Parse one SSE block (a run of lines up to a blank line) into a {@link SseFrame},
 * or `null` when the block carries no `data:` line (a bare keep-alive comment). The
 * default event name is `"message"` (the SSE spec default), matching the wire.
 */
export function parseSseBlock(block: string): SseFrame | null {
  let name = "message";
  const dataParts: string[] = [];
  for (const line of block.split("\n")) {
    if (line === "" || line.charAt(0) === ":") continue; // blank or keep-alive comment
    if (line.startsWith("event:")) {
      name = line.slice(6).trim();
    } else if (line.startsWith("data:")) {
      dataParts.push(line.slice(5).replace(/^ /, ""));
    }
  }
  return dataParts.length > 0 ? { name, data: dataParts.join("\n") } : null;
}

/**
 * Read an SSE response body to completion, invoking `onFrame` for each parsed
 * frame. Mirrors chat.js's reader loop: decode incrementally, split on the `\n\n`
 * block separator, flush the tail. Operates on the raw `ReadableStream` so it is
 * driven identically by the browser and by a test's hand-built stream.
 */
export async function readSseStream(
  body: ReadableStream<Uint8Array> | null,
  onFrame: (frame: SseFrame) => void,
): Promise<void> {
  if (!body) return;
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const chunk = await reader.read();
    if (chunk.done) break;
    buffer += decoder.decode(chunk.value, { stream: true });
    let sep: number;
    while ((sep = buffer.indexOf("\n\n")) !== -1) {
      const block = buffer.slice(0, sep);
      buffer = buffer.slice(sep + 2);
      const frame = parseSseBlock(block);
      if (frame) onFrame(frame);
    }
  }
  buffer += decoder.decode(); // flush any partial multi-byte char the streaming decoder held
  if (buffer.trim()) {
    const frame = parseSseBlock(buffer);
    if (frame) onFrame(frame);
  }
}

// ── Per-turn state machine ────────────────────────────────────────────────────

/** One subagent-activity chip's lifecycle (running → done) in a turn. */
export interface ActivityChip {
  index: number;
  role: StepRole;
  instruction: string;
  done: boolean;
  summary?: string;
}

/** The accumulated render state of one assistant turn, folded from its SSE frames. */
export interface TurnState {
  /** The latest plan (a replan supersedes the prior plan), or `null` before one. */
  plan: { round: number; steps: PlanStep[] } | null;
  /** The subagent-activity chips, in start order. */
  chips: ActivityChip[];
  /** The answer text — streamed token-by-token, reconciled by `final_answer`. */
  answer: string;
  /** True while `answer_delta`s arrive, cleared by `final_answer`. */
  streaming: boolean;
  /** The honest halt note when the turn hit a budget bound, else `null`. */
  halt: string | null;
  /** The honest error message when the turn faulted, else `null`. */
  error: string | null;
  /** True once a `final_answer` arrived (the authoritative answer landed). */
  finalized: boolean;
}

/** A fresh, empty turn. */
export function initialTurn(): TurnState {
  return { plan: null, chips: [], answer: "", streaming: false, halt: null, error: null, finalized: false };
}

/**
 * Fold one SSE frame into a turn's state, returning a NEW state (immutable — the
 * component drives React re-renders off the reference). Honest by construction:
 * `error` carries the plain-text error body verbatim, `halted` becomes a named
 * bound note, and a malformed (non-`error`) frame is dropped rather than guessed
 * at — never a fabricated answer ([NFR-CC-04]).
 */
export function applyFrame(state: TurnState, frame: SseFrame): TurnState {
  // The error event carries a PLAIN-TEXT message, not JSON (NFR-CC-04).
  if (frame.name === "error") {
    return { ...state, error: frame.data };
  }
  let data: Record<string, unknown>;
  try {
    data = JSON.parse(frame.data) as Record<string, unknown>;
  } catch {
    return state; // a malformed frame is dropped rather than guessed at
  }
  if (frame.name === "plan") {
    // Guard `steps` at the boundary: a malformed frame whose `steps` is not an
    // array must not crash the render's `.map` (consistent with the type guards on
    // the other event payloads below) — drop to an empty plan instead.
    const steps = Array.isArray(data.steps) ? (data.steps as PlanStep[]) : [];
    return { ...state, plan: { round: Number(data.round) || 0, steps } };
  }
  if (frame.name === "step_started") {
    const chip: ActivityChip = {
      index: Number(data.index),
      role: data.role as StepRole,
      instruction: typeof data.instruction === "string" ? data.instruction : "",
      done: false,
    };
    return { ...state, chips: [...state.chips, chip] };
  }
  if (frame.name === "step_observed") {
    const summary = typeof data.summary === "string" ? data.summary : undefined;
    return {
      ...state,
      chips: state.chips.map((c) =>
        c.index === Number(data.index) ? { ...c, done: true, summary } : c,
      ),
    };
  }
  if (frame.name === "halted") {
    return { ...state, halt: boundNote(data.bound as BudgetBound | undefined) };
  }
  if (frame.name === "answer_delta") {
    if (typeof data.delta !== "string") return state;
    return { ...state, answer: state.answer + data.delta, streaming: true };
  }
  if (frame.name === "final_answer") {
    const answer = typeof data.answer === "string" ? data.answer : "";
    // The final answer is the record of truth: reconcile to it when present,
    // otherwise keep what streamed. Clear the streaming caret either way.
    return { ...state, answer: answer || state.answer, streaming: false, finalized: true };
  }
  return state;
}

/**
 * A cleanly-closed turn that produced no answer, halt, or error is itself an honest
 * state — the connection may have closed early ([NFR-CC-04]); the component surfaces
 * it rather than leaving a silently empty turn.
 */
export function turnEndedEmpty(state: TurnState): boolean {
  return state.answer === "" && state.halt === null && state.error === null;
}

// ── Consent gate (NFR-SE-07; mirrors chat.js localStorage gate) ───────────────

/** The localStorage key remembering the first-use consent acknowledgement. */
export const CONSENT_KEY = "logos.chat.consent";

/** Has the user acknowledged the first-use consent? Storage-blocked ⇒ re-ask each
 *  load (fail SAFE, not open). */
export function hasConsent(): boolean {
  try {
    return window.localStorage.getItem(CONSENT_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remember the consent acknowledgement (best-effort; non-fatal if storage is blocked). */
export function rememberConsent(): void {
  try {
    window.localStorage.setItem(CONSENT_KEY, "1");
  } catch {
    /* non-fatal: consent holds for this page even if it cannot persist */
  }
}
