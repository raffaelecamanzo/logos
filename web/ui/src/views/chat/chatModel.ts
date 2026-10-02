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
  /** `[chat] read_roots` (sprint-79 HF-1): extra directories the Source-Reader may
   *  read through in-tree symlinks, as declared — relative to the root that
   *  declared the table, or absolute. Omitted by the server when empty. */
  read_roots?: string[];
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

/**
 * The chat-relevant slice of the workspace root's config tier
 * (`GET /api/v1/workspace/config`, S-450) — what the Workspace Chat reads its
 * verdict from (S-485). `effective_chat` is resolved at the workspace root with no
 * tier above it, exactly as the workspace chat's turn resolves it (S-482), so its
 * origins are relative to that root: `member` means *declared at the workspace
 * root*, and `workspace` never appears. It is `null` when either tier file does not
 * parse, and the fault that explains it is then named by file and position only.
 */
export interface WorkspaceChatConfigReadModel {
  effective_chat: EffectiveChatSlice | null;
  config: { error: string | null };
  chat_key_error: string | null;
}

/** One member's effective chat read roots (mirrors `MemberChatReadRoots`,
 *  `GET /api/v1/workspace/config/read-roots`, S-485): the roots a repo-addressed
 *  source call on that member reads through. `policy_origin` and `declared_by` are
 *  `null` when the member's chat config cannot be read. */
export interface MemberChatReadRoots {
  name: string;
  policy_origin: ChatOrigin | null;
  /** The root relative entries resolve against: the member's own, or the
   *  workspace root for an inherited policy. */
  declared_by: "member" | "workspace" | null;
  read_roots: string[];
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
    // Delegates to `workspaceConfigRepairHref` (HF-2) rather than repeating its
    // `scope.mode === "single" ? null : urlWithMember(...)` — same target view,
    // same member-carrying semantics, one implementation (review-fix, HF-2).
    workspaceConfigHref: workspaceConfigRepairHref(scope),
    memberKeyWithheld: member_key_withheld,
  };
}

/** Map the shell's workspace mode + selected member onto the scope the verdict
 *  names. Views mount only after the probe settles, so `loading` is never seen in
 *  the shell; outside it (a bare render) nothing is scoped, which is single-root. */
export function chatScope(mode: WorkspaceMode, member: string | null): ChatScope {
  return mode === "workspace" ? { mode: "workspace", member } : { mode: "single" };
}

/**
 * Workspace mode only: the workspace Config view's href for the member Chat and
 * Config tabs' failed `GET /api/v1/config` read (HF-2, Sprint 77 review option
 * 4i) — `null` in single-root mode, which has no workspace tier and no such
 * view.
 *
 * That read fails the same fail-loud `500` (S-450) whether the fault is THIS
 * member's own `config.toml`/`secrets.toml` or the workspace-tier file it may
 * inherit from at the workspace root — every member inheriting from a broken
 * workspace-root file fails identically. The `500` body (`web/src/api_v1.rs`'s
 * `fail`) carries only the façade's flattened error chain, which names an
 * absolute filesystem path this surface has no baseline to compare against (no
 * root path is ever sent to the client) — it cannot reliably say which root
 * faulted, so the caller's wording names both possibilities rather than guess
 * wrong (NFR-SE-07 forbids rendering the body itself regardless).
 */
export function workspaceConfigRepairHref(scope: ChatScope): string | null {
  return scope.mode === "single" ? null : urlWithMember(WORKSPACE_CONFIG_HREF, scope.member);
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

/** The model named in the persistent status band (S-309): the declared model, or
 *  an honest "not configured" note rather than a fabricated default
 *  ([NFR-CC-04]). `resolve_chat` (`logos-core/src/config/chat.rs`) only resolves a
 *  policy origin once its model is a non-empty string, so a `ChatReady` policy
 *  carries one in practice — the type stays optional (`model?: string | null`) so
 *  a change to that invariant renders honestly instead of silently printing
 *  `undefined`. The consent banner (`ConsentBanner`) predates this helper and
 *  still names `chat.model` directly, verbatim — out of scope here, since the
 *  AC requires that gate preserved exactly as it was. */
export function modelLabel(chat: ChatPolicy): string {
  return chat.model && chat.model.trim() !== "" ? chat.model : "no model configured";
}

/** The extra read roots the effective policy declares (sprint-79 HF-1), named by
 *  the consent banner and the status band because their content can be sent to
 *  the endpoint too. Empty — and so rendering nothing — unless the project opted
 *  in; the server omits the key then, so an absent key is the common case. */
export function readRoots(chat: ChatPolicy): string[] {
  return (chat.read_roots ?? []).filter((root) => root.trim() !== "");
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

/** One subagent-activity chip's lifecycle (running → done) in a turn. `round` is
 *  stamped from the turn's latest `plan` frame at the moment `step_started`
 *  arrives — `step_started`/`step_observed` carry no round on the wire, because
 *  the orchestrator restarts `index` at 0 every replan round (S-303, CR-090).
 *  Completion is matched on the `(round, index)` pair, never `index` alone, so a
 *  later round's observation cannot mark an earlier round's step done. */
export interface ActivityChip {
  round: number;
  index: number;
  role: StepRole;
  instruction: string;
  done: boolean;
  summary?: string;
}

/** One round's plan (mirrors the `plan` frame that produced it). */
export type RoundPlan = { round: number; steps: PlanStep[] };

/** The accumulated render state of one assistant turn, folded from its SSE frames. */
export interface TurnState {
  /** The latest plan (a replan supersedes the prior plan), or `null` before one. */
  plan: RoundPlan | null;
  /** Every round's plan seen this turn, in arrival order — the S-303 grouping
   *  source, so the expanded fold can show each round's plan with its own steps.
   *  A single-round turn carries exactly one entry, identical to `[plan]`. */
  plans: RoundPlan[];
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
  return {
    plan: null,
    plans: [],
    chips: [],
    answer: "",
    streaming: false,
    halt: null,
    error: null,
    finalized: false,
  };
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
    const round: RoundPlan = { round: Number(data.round) || 0, steps };
    return { ...state, plan: round, plans: [...state.plans, round] };
  }
  if (frame.name === "step_started") {
    // Stamped from the round already held in client state (this turn's latest
    // `plan` frame) — `step_started` carries no round on the wire.
    const chip: ActivityChip = {
      round: state.plan?.round ?? 0,
      index: Number(data.index),
      role: data.role as StepRole,
      instruction: typeof data.instruction === "string" ? data.instruction : "",
      done: false,
    };
    return { ...state, chips: [...state.chips, chip] };
  }
  if (frame.name === "step_observed") {
    // Matched on the (round, index) pair — the same round the wire's `index`
    // restarted from — so a later round's observation cannot mark an earlier
    // round's colliding index done (S-303, CR-090).
    const round = state.plan?.round ?? 0;
    const index = Number(data.index);
    const summary = typeof data.summary === "string" ? data.summary : undefined;
    return {
      ...state,
      chips: state.chips.map((c) =>
        c.round === round && c.index === index ? { ...c, done: true, summary } : c,
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

// ── Scope-keyed client state (S-485, FR-UI-26, NFR-SE-07) ─────────────────────

/**
 * The scope a chat's client state is remembered under (S-485): the member chat of
 * a single-root serve, the member chat of one workspace member, or the workspace
 * chat. Every chat has its own conversation store — a member's `chat.db`, or
 * `<workspace root>/.logos/chat.db` — and thread ids are per-store rowids, so thread
 * 3 of one scope is an unrelated conversation in another. Keying the remembered
 * thread (and the consent) by scope is what stops one chat from reopening, or
 * inheriting the consent of, another's.
 */
export type ChatStorageScope = "single" | `member:${string}` | "workspace";

/** The workspace chat's storage scope. */
export const WORKSPACE_CHAT_SCOPE: ChatStorageScope = "workspace";

/** The member chat's storage scope for the shell's {@link ChatScope} — spelled as
 *  the shell's own cache key (`WorkspaceContext`), so a member's chat state is
 *  keyed exactly as that member's views are. */
export function memberChatStorageScope(scope: ChatScope): ChatStorageScope {
  return scope.mode === "workspace" && scope.member !== null ? `member:${scope.member}` : "single";
}

/**
 * `base` qualified by `scope` — the one spelling of a scope-keyed storage key.
 *
 * The unqualified keys every chat shared before S-485 are deliberately not read:
 * their scope is unknown (a workspace serve's member chat wrote them member-blind),
 * so trusting one would reopen, or skip the consent of, another chat's state. The
 * cost is one fresh consent prompt per scope after the upgrade — fail safe, never
 * open.
 */
export function chatStorageKey(base: string, scope: ChatStorageScope): string {
  return `${base}:${scope}`;
}

// ── Consent gate (NFR-SE-07; mirrors chat.js localStorage gate) ───────────────

/** The localStorage key BASE remembering the first-use consent acknowledgement —
 *  qualified by scope ({@link chatStorageKey}). */
export const CONSENT_KEY = "logos.chat.consent";

/** The localStorage key BASE remembering WHICH extra read roots (sprint-79 HF-1)
 *  the consent was given for — their content can reach the endpoint too, so a
 *  consent given before they were declared, or for a different set, does not
 *  cover them. Qualified by scope like {@link CONSENT_KEY}. */
export const READ_ROOTS_CONSENT_KEY = "logos.chat.consent.readRoots";

/** The disclosed read-root set as one comparable value: sorted, deduplicated. */
function readRootsScope(roots: string[]): string {
  return JSON.stringify([...new Set(roots)].sort());
}

/** Has the user acknowledged this scope's first-use consent — and, when the
 *  disclosure names extra read roots, acknowledged exactly this set of them? With
 *  none declared this is the plain first-use gate it always was. Storage-blocked ⇒
 *  re-ask each load (fail SAFE, not open). */
export function hasConsent(scope: ChatStorageScope, roots: string[] = []): boolean {
  try {
    if (window.localStorage.getItem(chatStorageKey(CONSENT_KEY, scope)) !== "1") return false;
    return (
      roots.length === 0 ||
      window.localStorage.getItem(chatStorageKey(READ_ROOTS_CONSENT_KEY, scope)) ===
        readRootsScope(roots)
    );
  } catch {
    return false;
  }
}

/** Remember this scope's consent acknowledgement, and the read-root set it
 *  disclosed (best-effort; non-fatal if storage is blocked). */
export function rememberConsent(scope: ChatStorageScope, roots: string[] = []): void {
  try {
    window.localStorage.setItem(chatStorageKey(CONSENT_KEY, scope), "1");
    if (roots.length > 0) {
      window.localStorage.setItem(
        chatStorageKey(READ_ROOTS_CONSENT_KEY, scope),
        readRootsScope(roots),
      );
    }
  } catch {
    /* non-fatal: consent holds for this page even if it cannot persist */
  }
}

// ── The read-roots disclosure (sprint-79 HF-1, S-485, NFR-SE-07) ──────────────

/** One run of extra read roots that resolve against the same root. */
export interface ReadRootGroup {
  roots: string[];
  /** The root these entries resolve against, in words (`the workspace root`, a
   *  member's name), or `null` when they need no qualifier: the chat's own root,
   *  or absolute entries, which resolve as written. */
  relativeTo: string | null;
  /** The members whose source calls read through these roots, when the group
   *  does not already say so — the workspace chat's disclosure only. */
  readBy: string[];
}

/** Everything the consent banner and status band say can be read and sent
 *  besides the project itself: the extra read roots, grouped by the root they
 *  resolve against, and the members whose roots could not be listed. */
export interface ReadRootsDisclosure {
  groups: ReadRootGroup[];
  /** Members whose chat config could not be read — their roots are unknown. */
  unreadable: string[];
  /** Whose symlinks the roots are reached through: `this project's` or
   *  `each member's`. */
  through: string;
}

/** Is `root` absolute — resolved as written, never against the root that
 *  declared it? A POSIX path, a drive-letter path, or a UNC share. */
function isAbsoluteRoot(root: string): boolean {
  return root.startsWith("/") || root.startsWith("\\\\") || /^[A-Za-z]:[\\/]/.test(root);
}

/** `roots` as disclosure groups: the relative entries qualified by `relativeTo`,
 *  the absolute ones unqualified — an absolute entry is not relative to anything,
 *  and saying so would misstate where its files come from (NFR-SE-07). With no
 *  qualifier at all the order is kept as declared, in one group. `readers` names
 *  who reads through them wherever the qualifier does not already say so. */
function anchoredGroups(
  roots: string[],
  relativeTo: string | null,
  readers: { relative: string[]; absolute: string[] },
): ReadRootGroup[] {
  if (relativeTo === null) {
    return roots.length === 0 ? [] : [{ roots, relativeTo: null, readBy: readers.relative }];
  }
  const relative = roots.filter((root) => !isAbsoluteRoot(root));
  const absolute = roots.filter(isAbsoluteRoot);
  return [
    ...(relative.length > 0 ? [{ roots: relative, relativeTo, readBy: readers.relative }] : []),
    ...(absolute.length > 0 ? [{ roots: absolute, relativeTo: null, readBy: readers.absolute }] : []),
  ];
}

/** The member chat's disclosure: the effective policy's own read roots, relative
 *  to the workspace root when the policy is inherited (sprint-79 HF-1). */
export function memberReadRootsDisclosure(ready: ChatReady): ReadRootsDisclosure {
  return {
    groups: anchoredGroups(
      readRoots(ready.policy),
      ready.policyOrigin === "workspace" ? "the workspace root" : null,
      { relative: [], absolute: [] },
    ),
    unreadable: [],
    through: "this project's",
  };
}

/**
 * The workspace chat's disclosure (S-485, FR-WS-34): every member's EFFECTIVE read
 * roots — the set a repo-addressed source call reads through — from the engine-free
 * read-roots read-model. Roots declared by the workspace tier are named once,
 * relative to the workspace root, with the members that inherit them; a member
 * that owns its policy gets its own group, relative to that member.
 */
export function workspaceReadRootsDisclosure(members: MemberChatReadRoots[]): ReadRootsDisclosure {
  const inherited = { roots: [] as string[], readBy: [] as string[] };
  const owned: ReadRootGroup[] = [];
  const unreadable: string[] = [];
  for (const member of members) {
    if (member.declared_by === null) {
      unreadable.push(member.name);
      continue;
    }
    const roots = member.read_roots.filter((root) => root.trim() !== "");
    if (roots.length === 0) continue;
    if (member.declared_by === "workspace") {
      // Every inheriting member carries the workspace tier's one table, so its
      // roots are the same entries each time — named once, with who reads them.
      for (const root of roots) if (!inherited.roots.includes(root)) inherited.roots.push(root);
      inherited.readBy.push(member.name);
    } else {
      // Relative to the member says whose roots they are; an absolute entry
      // names its member as its reader instead.
      owned.push(...anchoredGroups(roots, member.name, { relative: [], absolute: [member.name] }));
    }
  }
  return {
    groups: [
      ...anchoredGroups(inherited.roots, "the workspace root", {
        relative: inherited.readBy,
        absolute: inherited.readBy,
      }),
      ...owned,
    ],
    unreadable,
    through: "each member's",
  };
}

/** The disclosure as the comparable set a consent is remembered for: each root
 *  qualified by the root it resolves against, and each member whose roots are
 *  unknown — so a root moving to another declaring root, or a member's roots
 *  becoming known, asks again. The member chat's own unqualified roots stay the
 *  bare entries they always were. */
export function consentEntries(disclosure: ReadRootsDisclosure): string[] {
  return [
    ...disclosure.groups.flatMap((group) =>
      group.roots.map((root) => (group.relativeTo === null ? root : `${group.relativeTo}: ${root}`)),
    ),
    ...disclosure.unreadable.map((name) => `unreadable: ${name}`),
  ];
}

// ── The Workspace Chat's readiness (S-485, FR-WS-34, NFR-CC-04) ───────────────

/** The workspace chat cannot be configured from what the tier holds, because a
 *  tier file does not parse: the faults, by file and position only. */
export interface WorkspaceTierUnreadable {
  ready: false;
  unreadable: true;
  faults: string[];
}

/** The workspace chat's configure-first state: the absent half at the workspace
 *  root, the present half if any, and the files that would declare it. */
export interface WorkspaceConfigureFirst {
  ready: false;
  unreadable: false;
  absent: AbsentHalf;
  /** The half the workspace root DOES declare, or `null`. */
  present: PresentHalf["half"] | null;
  /** The workspace-root files the absent half is declared in. */
  workspaceFiles: string[];
}

export type WorkspaceChatReadiness = ChatReady | WorkspaceTierUnreadable | WorkspaceConfigureFirst;

/**
 * Is the workspace chat usable? A PURE function of the workspace tier's effective
 * chat slice — the resolution the workspace turn dials (`resolve_chat` at the
 * workspace root with no tier above it, S-482) — so the view and the turn cannot
 * disagree. No member's `[chat]` is consulted: none configures this chat.
 *
 * Ready iff both halves are declared there. Origins arrive relative to the
 * workspace root (`member` = declared at it), which is what `ChatReady` carries.
 */
export function workspaceChatReadiness(model: WorkspaceChatConfigReadModel): WorkspaceChatReadiness {
  const effective = model.effective_chat;
  if (effective === null) {
    return {
      ready: false,
      unreadable: true,
      faults: [model.config.error, model.chat_key_error].filter(
        (fault): fault is string => typeof fault === "string" && fault !== "",
      ),
    };
  }
  const { policy, policy_origin, credential_origin } = effective;
  if (policy_origin !== "unset" && credential_origin !== "unset") {
    return { ready: true, policy, policyOrigin: policy_origin, credentialOrigin: credential_origin };
  }
  const absent: AbsentHalf =
    policy_origin === "unset" ? (credential_origin === "unset" ? "both" : "model") : "key";
  return {
    ready: false,
    unreadable: false,
    absent,
    present: absent === "both" ? null : absent === "model" ? "key" : "model",
    workspaceFiles: [
      ...(absent === "key" ? [] : [WORKSPACE_CONFIG_FILE]),
      ...(absent === "model" ? [] : [WORKSPACE_SECRETS_FILE]),
    ],
  };
}

/** The Workspace Chat's configure-first sentences, from the verdict and the
 *  workspace's name — worded as the workspace turn's own refusal is
 *  (`configure_first_message`, `web/src/chat/workspace.rs`): the workspace root, the
 *  missing half, the present half, and that a member's `[chat]` does not configure
 *  this chat. The view links the action to Workspace Config. */
export interface WorkspaceConfigureFirstCopy {
  summary: string;
  present: string | null;
  memberNote: string;
  /** What to do — the view links Workspace Config after it. */
  action: string;
}

export function workspaceConfigureFirstCopy(
  state: WorkspaceConfigureFirst,
  workspace: string,
): WorkspaceConfigureFirstCopy {
  const [absentPhrase, action] =
    state.absent === "both"
      ? ["neither a provider model nor an API key is declared", "Choose a provider model and add an API key"]
      : state.absent === "model"
        ? ["no provider model is declared", "Choose a provider model"]
        : ["no API key is declared", "Add an API key"];
  return {
    summary: `The workspace chat is not configured yet for the workspace root of ${workspace} — ${absentPhrase} there.`,
    present: state.present === null ? null : `Its ${HALF_LABEL[state.present]} is declared.`,
    memberNote: "A member's own [chat] does not configure the workspace chat.",
    action,
  };
}
