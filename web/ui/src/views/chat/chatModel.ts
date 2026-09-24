/*
 * Chat view model (S-190, CR-049, FR-UI-18, FR-UI-19, FR-UI-23) — the PURE half of
 * the migrated Chat tab: the SSE wire types, the SSE-block parser, the per-turn
 * event reducer (plan / subagent-activity / answer-token / final-answer / honest
 * halt / honest error), the readiness verdict over the effective chat resolution
 * (S-452 — which root was inspected, which half is absent, where the present half
 * came from), and the consent + endpoint-disclosure helpers.
 *
 * It holds NO React and NO network: the orchestrator's SSE contract is unchanged
 * ([chat-agent]) — this is a re-homed client. Keeping the reducer pure makes every
 * branch (incl. `halted`/`error`) deterministically testable without a DOM or a
 * fetch, and lets the component (`ChatView.tsx`) stay a thin renderer over it.
 *
 * The masked chat key is NEVER referenced here — the consent banner discloses only
 * the effective provider, model, and endpoint host (NFR-SE-07).
 */

import { WORKSPACE_NAV_ITEMS } from "../../nav.ts";
import { urlWithMember } from "../../workspace/scope.ts";
import type { WorkspaceMode } from "../../workspace/WorkspaceContext.tsx";

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

// ── Config read-model (the effective-chat slice, S-448/S-452) ──────────────────
// A focused mirror of the ONE part of `ConfigReadModel`
// (`logos-core/src/config/writeback.rs`) the Chat tab reads: `effective_chat`, the
// resolution of the `[chat]` policy and its credential across the member root and,
// in a workspace, the workspace root — each half with the root it came from
// ([FR-WS-30], [ADR-67]). The full read-model and its typed fetch belong to the
// Config view.
//
// The member's LITERAL document (`config.parsed.chat`) and its own key (`chat_key`)
// are deliberately NOT mirrored here: an inheriting member declares neither, so a
// verdict read off them reports "not configured" over a configuration the turn path
// would dial — the CR-145 defect. Leaving them out of the type makes that
// misreading impossible to write rather than merely wrong.
//
// The credential is mirrored as presence alone. The wire's last-4 is never needed
// by this surface, so it is not carried into it (NFR-SE-07).

/** The `[chat]` policy slice (mirrors `ChatConfig`). `model` is omitted on the
 *  wire when unset. */
export interface ChatPolicy {
  provider: "anthropic" | "openai";
  model?: string | null;
  base_url: string;
  max_tool_calls: number;
  max_subagent_tool_calls: number;
  max_replans: number;
}

/** Where one half of the effective resolution came from (mirrors `ChatOrigin`).
 *  `workspace` is reachable only under a federated server. */
export type ChatOrigin = "member" | "workspace" | "unset";

/** The effective chat resolution (mirrors `EffectiveChat`, `GET /api/v1/config`). */
export interface EffectiveChatSlice {
  /** The effective `[chat]` table — the member's, the workspace's, or the defaults. */
  policy: ChatPolicy;
  policy_origin: ChatOrigin;
  /** Presence only — never rendered (NFR-SE-07). */
  credential: { present: boolean };
  credential_origin: ChatOrigin;
  /** The member's own key exists but is NOT used, because the policy is inherited
   *  from the workspace: a member key never reaches a workspace endpoint (HF-1,
   *  ADR-67 §2). Stated by the seam, never inferred here from `chat_key`. */
  member_key_withheld: boolean;
}

/** The chat-relevant slice of `ConfigReadModel` (`GET /api/v1/config`). */
export interface ChatConfigReadModel {
  effective_chat: EffectiveChatSlice;
}

// ── Thread read API (S-209 producer contract; S-210 consumer) ─────────────────
// The wire shapes the merged S-209 read endpoints serialize. Pure mirrors — no
// React, no fetch — so the rail's ordering/hydration logic stays testable off a
// plain fixture. No secret ever rides these payloads ([NFR-SE-07]).

/** One conversation in the rail list (mirrors `web::ThreadSummary`,
 *  `GET /api/v1/chat/threads`): the rowid, the S-208 auto-title, and the
 *  most-recent-first `updated_at` sort key (Unix seconds). */
export interface ThreadSummary {
  id: number;
  title: string;
  updated_at: number;
}

/** A persisted tool trace on a restored message (mirrors `chat_agent::ToolTrace`). */
export interface PersistedToolTrace {
  tool_name: string;
  arguments: string;
  result: string;
  is_error: boolean;
}

/** One stored message in a restored transcript (mirrors `chat_agent::ChatMessage`,
 *  `GET /api/v1/chat/threads/{id}`). `role` is the snake_case wire token; the rail
 *  renders `user`/`assistant` and skips the internal `system`/`tool` rows. */
export interface PersistedChatMessage {
  id: number;
  role: "user" | "assistant" | "system" | "tool";
  content: string;
  created_at: number;
  tool_traces: PersistedToolTrace[];
}

/** The native Anthropic endpoint host disclosed for the `anthropic` provider —
 *  mirrors the server view's `host_of(DEFAULT_ANTHROPIC_BASE_URL)` (web/src/views/chat.rs). */
export const ANTHROPIC_HOST = "api.anthropic.com";

// ── Readiness (S-452, FR-UI-18, NFR-CC-04) ─────────────────────────────────────

/** Which root the tab's reads were answered from — the shell's scope, not the
 *  read-model's (the read-model carries no root name). In workspace mode the
 *  member is always selected before any view mounts; `null` survives only for a
 *  manifest with no members, and is then named as what it is rather than guessed. */
export type ChatScope = { mode: "single" } | { mode: "workspace"; member: string | null };

/** The root the configure-first state names. */
export interface RootInspected {
  kind: "repository" | "member" | "default-member";
  /** `this repository`, the member's name, or `the workspace's default member`. */
  label: string;
}

/** Which half of the configuration is missing. */
export type AbsentHalf = "model" | "key" | "both";

/** The half that IS declared, and the root that declares it. */
export interface PresentHalf {
  half: "model" | "key";
  origin: "member" | "workspace";
}

/** Chat is usable: the configured surface receives the EFFECTIVE policy, and the
 *  origins so the consent banner can say where an inherited endpoint came from. */
export interface ChatReady {
  ready: true;
  policy: ChatPolicy;
  policyOrigin: "member" | "workspace";
  credentialOrigin: "member" | "workspace";
}

/** Chat is not yet usable, stated checkably: the root inspected, the absent half,
 *  the origin of any present half, and where the absent half is written. */
export interface ConfigureFirst {
  ready: false;
  root: RootInspected;
  absent: AbsentHalf;
  present: PresentHalf | null;
  /** The member Config tab — the control that writes either half at this root. */
  configHref: string;
  /** Workspace mode only: the workspace-root file each absent half would be
   *  declared in — named, so the reader knows which file the linked editor writes. */
  workspaceFiles: string[];
  /** Workspace mode only: the app-level workspace Config view, whose chat group
   *  writes both of those files (S-451) — carrying the member so returning here
   *  reopens this member's chat. `null` in single-root mode, which has no
   *  workspace tier and no such view. */
  workspaceConfigHref: string | null;
  /** The member declares its own key, withheld from the inherited workspace
   *  endpoint (HF-1) — so the absent key is the WORKSPACE's, and owning the
   *  policy is what lets the member use its own. */
  memberKeyWithheld: boolean;
}

export type ChatReadiness = ChatReady | ConfigureFirst;

/** The workspace-root file that declares the policy half. */
export const WORKSPACE_CONFIG_FILE = "<workspace-root>/.logos/config.toml";
/** The workspace-root file that holds the credential half. */
export const WORKSPACE_SECRETS_FILE = "<workspace-root>/.logos/secrets.toml";

/** The workspace Config view's route, read off its nav registration (S-430) so the
 *  configure-first link cannot drift from the route the shell mounts it at. */
export const WORKSPACE_CONFIG_HREF: string = registeredPath("workspace-config");

function registeredPath(id: string): string {
  const item = WORKSPACE_NAV_ITEMS.find((i) => i.id === id);
  if (!item) throw new Error(`nav.ts registers no "${id}" view`);
  return item.path;
}

function rootInspected(scope: ChatScope): RootInspected {
  if (scope.mode === "single") return { kind: "repository", label: "this repository" };
  if (scope.member === null) {
    return { kind: "default-member", label: "the workspace's default member" };
  }
  return { kind: "member", label: scope.member };
}

/**
 * Is chat usable, and if not, what exactly is missing? A PURE function of the
 * read-model's effective-chat slice and the shell's scope — the view renders the
 * answer and decides nothing.
 *
 * Ready iff BOTH origins are declared. That is the predicate the turn path applies
 * (`turn_provider`, `web/src/chat/mod.rs`) to the same resolution ([ADR-67] §6), so
 * the tab and the turn cannot disagree. No second model-or-key check is made here:
 * the seam already treats a blank model and a blank key as undeclared.
 */
export function chatReadiness(model: ChatConfigReadModel, scope: ChatScope): ChatReadiness {
  const { policy, policy_origin, credential_origin, member_key_withheld } = model.effective_chat;
  if (policy_origin !== "unset" && credential_origin !== "unset") {
    return { ready: true, policy, policyOrigin: policy_origin, credentialOrigin: credential_origin };
  }
  const absent: AbsentHalf =
    policy_origin === "unset" ? (credential_origin === "unset" ? "both" : "model") : "key";
  const present: PresentHalf | null =
    absent === "both"
      ? null
      : absent === "model"
        ? { half: "key", origin: credential_origin as PresentHalf["origin"] }
        : { half: "model", origin: policy_origin as PresentHalf["origin"] };
  const member = scope.mode === "workspace" ? scope.member : null;
  const workspaceFiles =
    scope.mode === "single"
      ? []
      : [
          ...(absent === "key" ? [] : [WORKSPACE_CONFIG_FILE]),
          ...(absent === "model" ? [] : [WORKSPACE_SECRETS_FILE]),
        ];
  return {
    ready: false,
    root: rootInspected(scope),
    absent,
    present,
    configHref: urlWithMember("/config", member),
    workspaceFiles,
    workspaceConfigHref: scope.mode === "single" ? null : urlWithMember(WORKSPACE_CONFIG_HREF, member),
    memberKeyWithheld: member_key_withheld,
  };
}

/** Map the shell's workspace mode + selected member onto the scope the verdict
 *  names. Views mount only after the probe settles, so `loading` is never seen in
 *  the shell; outside it (a bare render) nothing is scoped, which is single-root. */
export function chatScope(mode: WorkspaceMode, member: string | null): ChatScope {
  return mode === "workspace" ? { mode: "workspace", member } : { mode: "single" };
}

/** The configure-first state's sentences, composed from the verdict alone so the
 *  view only lays them out. Worded to match the turn path's refusal
 *  (`configure_first_message`, `web/src/chat/mod.rs`): same halves, same origins. */
export interface ConfigureFirstCopy {
  /** The root inspected and the absent half, e.g. "Chat is not configured yet for
   *  billing-service — no API key is declared by billing-service or by the
   *  workspace root." */
  summary: string;
  /** Where the present half came from, or `null` when both are absent. */
  present: string | null;
  /** Under an inherited policy with no workspace key: why a member key does not
   *  help (a declared one is withheld; a new one would be) and how to use one — or
   *  `null` when the member's policy or no policy is in effect. */
  memberKeyNote: string | null;
  /** What to do, e.g. "Add an API key" — the view links the Config tab after it. */
  action: string;
  /** Trails the Config-tab link: " for billing-service", or "" in single-root. */
  actionScope: string;
  /** Leads the named workspace files, or `null` when there are none to name. */
  workspaceLead: string | null;
}

const HALF_LABEL: Record<PresentHalf["half"], string> = { model: "provider model", key: "API key" };

export function configureFirstCopy(state: ConfigureFirst): ConfigureFirstCopy {
  const { root, absent, present } = state;
  // How the member root is referred to after the root has been named once.
  const memberRef =
    root.kind === "member"
      ? root.label
      : root.kind === "default-member"
        ? "that member"
        : "this repository";
  const withheld = state.memberKeyWithheld;
  // The key is absent under a policy inherited from the workspace: that endpoint
  // takes the workspace root's key alone (HF-1, ADR-67 §2), so only the workspace
  // root can supply it — a member key, declared or added, is not used.
  const inheritedPolicy = absent === "key" && present?.origin === "workspace";
  const whereLooked = inheritedPolicy
    ? " by the workspace root"
    : root.kind === "repository"
      ? ""
      : ` by ${memberRef} or by the workspace root`;
  const absentPhrase =
    absent === "both"
      ? "neither a provider model nor an API key is declared"
      : absent === "model"
        ? "no provider model is declared"
        : "no API key is declared";
  const presentLine =
    present === null
      ? null
      : present.origin === "workspace"
        ? `The ${HALF_LABEL[present.half]} is inherited from the workspace root.`
        : `The ${HALF_LABEL[present.half]} is declared by ${memberRef}.`;
  const action =
    absent === "both"
      ? "Choose a provider model and add an API key"
      : absent === "model" || withheld
        ? "Choose a provider model"
        : inheritedPolicy
          ? "Choose a provider model and add an API key"
          : "Add an API key";
  return {
    summary: `Chat is not configured yet for ${root.label} — ${absentPhrase}${whereLooked}.`,
    present: presentLine,
    memberKeyNote: withheld
      ? `The API key ${memberRef} declares is not used with the inherited workspace endpoint — setting a [chat] model on ${memberRef} makes it use its own key.`
      : inheritedPolicy
        ? `An API key added to ${memberRef} is not used with the inherited workspace endpoint — it is used once ${memberRef} declares its own [chat] model.`
        : null,
    action,
    actionScope: root.kind === "member" ? ` for ${root.label}` : "",
    workspaceLead:
      state.workspaceFiles.length === 0
        ? null
        : inheritedPolicy
          ? "Or declare an API key once for every member of the workspace, in"
          : `Or declare ${absent === "both" ? "them" : "it"} once for every member of the workspace, in`,
  };
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
