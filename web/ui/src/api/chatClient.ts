/*
 * The Chat tab's data-access layer (S-190, CR-049, FR-UI-18, FR-UI-19, NFR-SE-06).
 *
 * The first MUTATING SPA surface: a chat turn and the per-conversation delete both
 * ride the intent-guarded `POST` seam (`apiMutate`, `src/intent.ts`) so they carry
 * the same-origin + per-session intent token the server's guard requires — the
 * streaming turn consumes SSE over that `POST` via `fetch` (not a `GET`
 * EventSource, which cannot set the custom intent header). The config-state read is
 * a plain `/api/v1` GET. The SSE turn contract is UNCHANGED ([chat-agent]); this is
 * a re-homed client.
 *
 * S-211 ([CR-053], [FR-UI-26], [ADR-47]) retired the global clear-history helper
 * along with the route itself (S-209 removed it server-side — a POST to it is now
 * `405`): per-conversation delete is the sole deletion path. The retired route
 * literal must not reappear in this module — `logos-chat/tests/uat_ui_08.rs` locks that.
 *
 * S-485 ([FR-WS-34], [ADR-71]) serves the workspace chat over the same functions:
 * each turn/thread helper takes the {@link ChatRoutes} of the chat it talks to, and
 * the two workspace reads beside `fetchChatConfig` feed the Workspace Chat's
 * readiness verdict and consent disclosure without starting a member engine.
 *
 * Lives in its own module (not the shared `client.ts`) so the parallel Config
 * migration (S-191) and this one do not collide on the data layer — the only shared
 * SPA wiring both touch is `nav.ts` + the view registry.
 */

import { apiFetch } from "./client.ts";
import { apiMutate } from "../intent.ts";
import { withMemberScope } from "../workspace/scope.ts";
import type {
  ChatConfigReadModel,
  MemberChatReadRoots,
  PersistedChatMessage,
  ThreadSummary,
  WorkspaceChatConfigReadModel,
} from "../views/chat/chatModel.ts";
import { DEFAULT_PRESET, type ChatPresetId } from "../views/chat/presets.ts";

/** The intent-guarded chat-turn route (mirrors `logos_chat::CHAT_POST_ROUTE`). */
export const CHAT_ROUTE = "/chat";
/** The conversation-history route tree (mirrors `logos_chat::CHAT_THREADS_ROUTE`): GET for
 *  the list and one thread's transcript, and the mutating verbs beneath it —
 *  `POST …/{id}/delete` ({@link chatThreadDeleteRoute}), `POST …/{id}/rename`
 *  ({@link chatThreadRenameRoute}) and `POST …/{id}/truncate`
 *  ({@link chatThreadTruncateRoute}). */
export const CHAT_THREADS_ROUTE = "/api/v1/chat/threads";
/** The workspace chat's turn route (mirrors `logos_chat::WORKSPACE_CHAT_POST_ROUTE`, S-482):
 *  the `/chat` contract unchanged, answering for the workspace. A server route, not a
 *  client one — no view mounts here; the Workspace Chat VIEW is the `chat`
 *  extension's, registered in `views/chat/extension.tsx`. */
export const WORKSPACE_CHAT_ROUTE = "/workspace/chat";
/** The workspace chat's conversation-history tree (mirrors
 *  `logos_chat::WORKSPACE_CHAT_THREADS_ROUTE`, S-482), over `<workspace root>/.logos/chat.db`. */
export const WORKSPACE_CHAT_THREADS_ROUTE = "/api/v1/workspace/chat/threads";

/**
 * Which chat service a surface talks to (S-485, FR-WS-34, ADR-71): the member
 * chat's routes or the workspace chat's. Both carry one contract — the turn, the
 * list, one transcript and the per-thread delete — so the components and hooks
 * that drive a chat take this as a parameter rather than knowing which they serve.
 *
 * Always passed, never defaulted: a defaulted member route is the silent wrong
 * answer for the workspace chat, which would then read and delete a member's
 * conversations under the workspace's heading.
 */
export interface ChatRoutes {
  /** The intent-guarded turn `POST`. */
  turn: string;
  /** The conversation-history tree (GET list / `{id}`; `POST {id}/delete`, `{id}/rename`,
   *  `{id}/truncate`). */
  threads: string;
  /** Whether requests carry the active member's `?repo=`. The member chat answers
   *  for one member; the workspace chat reads no `?repo=` at all. */
  memberScoped: boolean;
}

/** The member chat — `?repo=`-scoped in a workspace, unscoped in a single root. */
export const MEMBER_CHAT_ROUTES: ChatRoutes = {
  turn: CHAT_ROUTE,
  threads: CHAT_THREADS_ROUTE,
  memberScoped: true,
};

/** The workspace chat (S-482) — never member-scoped. */
export const WORKSPACE_CHAT_ROUTES: ChatRoutes = {
  turn: WORKSPACE_CHAT_ROUTE,
  threads: WORKSPACE_CHAT_THREADS_ROUTE,
  memberScoped: false,
};

const API_PREFIX = "/api/v1/";

/** `routes.threads` as the `/api/v1`-relative endpoint {@link apiFetch} takes. */
function threadsEndpoint(routes: ChatRoutes): string {
  return routes.threads.startsWith(API_PREFIX)
    ? routes.threads.slice(API_PREFIX.length)
    : routes.threads;
}

/** A mutating route, carrying the member scope only for a member-scoped chat. */
function mutationRoute(routes: ChatRoutes, path: string): string {
  return routes.memberScoped ? withMemberScope(path) : path;
}

/**
 * `GET /api/v1/config` → the chat-relevant slice of the config read-model: the
 * EFFECTIVE `[chat]` policy (provider/model/endpoint/budget) and the credential's
 * presence, each with the root it resolved from (S-448's `effective_chat`). A pure
 * read — no token, no store mutation ([ADR-28]).
 */
export function fetchChatConfig(): Promise<ChatConfigReadModel> {
  return apiFetch<ChatConfigReadModel>("config");
}

/**
 * `GET /api/v1/workspace/config` → the chat-relevant slice of the workspace root's
 * config tier (S-450): the effective chat resolved at the workspace root with NO
 * tier above it — exactly the resolution the workspace chat's turn dials (S-482) —
 * plus the two parse faults that leave it `null`. Engine-free, no `?repo=`.
 */
export function fetchWorkspaceChatConfig(): Promise<WorkspaceChatConfigReadModel> {
  return apiFetch<WorkspaceChatConfigReadModel>("workspace/config");
}

/**
 * `GET /api/v1/workspace/config/read-roots` → every member's effective `[chat]
 * read_roots` and the root that declared them (S-485), the roots a repo-addressed
 * source call may read through. Built from config reads alone, so the consent
 * banner warms no member engine (NFR-PE-10) — which a fan-out over
 * `GET /api/v1/config?repo=<m>` would.
 */
export function fetchWorkspaceChatReadRoots(): Promise<MemberChatReadRoots[]> {
  return apiFetch<MemberChatReadRoots[]>("workspace/config/read-roots");
}

/**
 * Start a chat turn — `POST` the turn route with `Accept: text/event-stream`,
 * carrying the intent header (NFR-SE-06), streaming the orchestrator's SSE events
 * back. The `signal` ties the turn's lifetime to the caller (unmount / a superseding
 * turn → abort → the server cancels the in-flight turn, [FR-UI-19]). The body is the
 * form-encoded user message, byte-identical to the no-JS POST.
 *
 * `threadId` (S-210, [FR-UI-26], [ADR-47]) appends the turn to an existing
 * conversation; `null` (a fresh "+ New chat") omits the `thread` field entirely, so
 * the server creates the thread on this first send — the byte-identical single-thread
 * body when no conversation is active. The thread the server (auto-)selected is not
 * returned on the SSE stream; the caller re-reads {@link fetchThreads} to learn a
 * newly-created id.
 *
 * `preset` (S-625, [FR-UI-49]) is the conversation's prompt-only preset. A steered
 * one rides a `preset` field the server validates against the same four ids (an
 * unknown value is a `400` that runs no turn); Default omits the field, which the
 * server reads as Default — so an unsteered turn keeps the byte-identical body.
 */
export function streamChatTurn(
  routes: ChatRoutes,
  question: string,
  threadId: number | null,
  preset: ChatPresetId,
  signal?: AbortSignal,
): Promise<Response> {
  let body = `q=${encodeURIComponent(question)}`;
  if (threadId != null) body += `&thread=${encodeURIComponent(threadId)}`;
  if (preset !== DEFAULT_PRESET) body += `&preset=${encodeURIComponent(preset)}`;
  return apiMutate(mutationRoute(routes, routes.turn), {
    headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" },
    body,
    signal,
  });
}

/**
 * `GET …/chat/threads` → the conversation list, most-recent-first (S-209 producer
 * contract, [FR-UI-26], [ADR-47]). A pure same-origin read carrying no secret
 * ([NFR-SE-07]); the member chat's is member-scoped like every `/api/v1` read, the
 * workspace chat's carries no `?repo=` (`apiFetch` scopes no `workspace/*` read).
 */
export function fetchThreads(routes: ChatRoutes): Promise<ThreadSummary[]> {
  return apiFetch<ThreadSummary[]>(threadsEndpoint(routes));
}

/**
 * `GET …/chat/threads/{id}` → one thread's ordered transcript (S-209), the
 * messages the rail hydrates on select-to-restore. An unknown id is an honest
 * `404` ({@link apiFetch} throws `ApiError`), never a misleading empty `200`.
 */
export function fetchThreadMessages(routes: ChatRoutes, id: number): Promise<PersistedChatMessage[]> {
  return apiFetch<PersistedChatMessage[]>(`${threadsEndpoint(routes)}/${id}`);
}

/** The per-thread delete path for `id` (mirrors the server's `…/{id}/delete`, one of
 *  the `POST`s admitted under a threads tree — `web::THREAD_WRITE_VERBS`). */
export function chatThreadDeleteRoute(routes: ChatRoutes, id: number): string {
  return `${routes.threads}/${id}/delete`;
}

/**
 * `POST …/chat/threads/{id}/delete` → delete ONE conversation and its
 * per-thread memory by cascade (S-209 producer contract, [FR-UI-26], [FR-UI-20],
 * [ADR-47]). The per-conversation replacement for the retired global clear.
 *
 * Intent-guarded like the turn ([ADR-31]) — a forged or intent-less delete never
 * reaches the handler ([NFR-SE-06]). The caller confirms first; this helper only
 * carries the request. The response is returned raw so the caller can distinguish
 * the server's three honest outcomes: `204` deleted, `404` already gone (an
 * idempotent no-op), anything else a fault to surface.
 */
export function deleteChatThread(routes: ChatRoutes, id: number): Promise<Response> {
  return apiMutate(mutationRoute(routes, chatThreadDeleteRoute(routes, id)), {});
}

/** The per-thread rename path for `id` (mirrors the server's `…/{id}/rename`, S-623). */
export function chatThreadRenameRoute(routes: ChatRoutes, id: number): string {
  return `${routes.threads}/${id}/rename`;
}

/**
 * `POST …/chat/threads/{id}/rename` → rename ONE conversation (S-623 producer
 * contract, [FR-UI-48], [ADR-47] as extended by CR-204). The new title rides a
 * form-encoded `title` field, as the turn's question rides `q`.
 *
 * Intent-guarded like the delete ([ADR-31]) — a forged or intent-less rename never
 * reaches the handler ([NFR-SE-06]). The response is returned raw so the caller can
 * tell the server's honest outcomes apart: `204` renamed, `400` the title is blank
 * or over-long (unchanged), `404` the conversation is gone, anything else a fault.
 */
export function renameChatThread(routes: ChatRoutes, id: number, title: string): Promise<Response> {
  return apiMutate(mutationRoute(routes, chatThreadRenameRoute(routes, id)), {
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: `title=${encodeURIComponent(title)}`,
  });
}

/** The per-thread truncate path for `id` (mirrors the server's `…/{id}/truncate`, S-624). */
export function chatThreadTruncateRoute(routes: ChatRoutes, id: number): string {
  return `${routes.threads}/${id}/truncate`;
}

/**
 * `POST …/chat/threads/{id}/truncate` → cut ONE conversation back to before a
 * question (S-624 producer contract, [FR-UI-46], [FR-UI-20], [ADR-47] as extended by
 * CR-204): the question, every later message and the per-thread memory written after
 * the cut are deleted together. `userTurn` names the question — the 0-based index
 * among the questions the store holds — and rides a form-encoded `user_turn` field.
 * Edit-and-resend and Regenerate call it before re-running the turn.
 *
 * Intent-guarded like the rename ([ADR-31]) — a forged or intent-less truncate never
 * reaches the handler ([NFR-SE-06]). The response is returned raw so the caller can
 * tell the server's honest outcomes apart: `204` cut (or nothing lay past the stored
 * questions), `404` the conversation is gone, anything else a fault.
 */
export function truncateChatThread(routes: ChatRoutes, id: number, userTurn: number): Promise<Response> {
  return apiMutate(mutationRoute(routes, chatThreadTruncateRoute(routes, id)), {
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: `user_turn=${encodeURIComponent(userTurn)}`,
  });
}
