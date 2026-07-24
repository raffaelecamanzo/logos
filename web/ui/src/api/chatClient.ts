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
 * literal must not reappear in this module — `web/tests/uat_ui_08.rs` locks that.
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
  PersistedChatMessage,
  ThreadSummary,
} from "../views/chat/chatModel.ts";

/** The intent-guarded chat-turn route (mirrors `web::CHAT_POST_ROUTE`). */
export const CHAT_ROUTE = "/chat";
/** The conversation-history route tree (mirrors `web::CHAT_THREADS_ROUTE`): GET for
 *  the list and one thread's transcript, and the one mutating verb beneath it —
 *  `POST …/{id}/delete` ({@link chatThreadDeleteRoute}). */
export const CHAT_THREADS_ROUTE = "/api/v1/chat/threads";

/**
 * `GET /api/v1/config` → the chat-relevant slice of the config read-model: the
 * `[chat]` policy (provider/model/endpoint/budget) plus the MASKED key's presence.
 * A pure read — no token, no store mutation ([ADR-28]).
 */
export function fetchChatConfig(): Promise<ChatConfigReadModel> {
  return apiFetch<ChatConfigReadModel>("config");
}

/**
 * Start a chat turn — `POST /chat` with `Accept: text/event-stream`, carrying the
 * intent header (NFR-SE-06), streaming the orchestrator's SSE events back. The
 * `signal` ties the turn's lifetime to the caller (unmount / a superseding turn →
 * abort → the server cancels the in-flight turn, [FR-UI-19]). The body is the
 * form-encoded user message, byte-identical to the no-JS POST.
 *
 * `threadId` (S-210, [FR-UI-26], [ADR-47]) appends the turn to an existing
 * conversation; `null` (a fresh "+ New chat") omits the `thread` field entirely, so
 * the server creates the thread on this first send — the byte-identical single-thread
 * body when no conversation is active. The thread the server (auto-)selected is not
 * returned on the SSE stream; the caller re-reads {@link fetchThreads} to learn a
 * newly-created id.
 */
export function streamChatTurn(
  question: string,
  threadId: number | null,
  signal?: AbortSignal,
): Promise<Response> {
  const body =
    threadId == null
      ? `q=${encodeURIComponent(question)}`
      : `q=${encodeURIComponent(question)}&thread=${encodeURIComponent(threadId)}`;
  return apiMutate(withMemberScope(CHAT_ROUTE), {
    headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" },
    body,
    signal,
  });
}

/**
 * `GET /api/v1/chat/threads` → the conversation list, most-recent-first (S-209
 * producer contract, [FR-UI-26], [ADR-47]). A pure same-origin read carrying no
 * secret ([NFR-SE-07]); member-scoped like every `/api/v1` read.
 */
export function fetchThreads(): Promise<ThreadSummary[]> {
  return apiFetch<ThreadSummary[]>("chat/threads");
}

/**
 * `GET /api/v1/chat/threads/{id}` → one thread's ordered transcript (S-209), the
 * messages the rail hydrates on select-to-restore. An unknown id is an honest
 * `404` ({@link apiFetch} throws `ApiError`), never a misleading empty `200`.
 */
export function fetchThreadMessages(id: number): Promise<PersistedChatMessage[]> {
  return apiFetch<PersistedChatMessage[]>(`chat/threads/${id}`);
}

/** The per-thread delete path for `id` (mirrors the server's `…/{id}/delete`, the
 *  only `POST` admitted under {@link CHAT_THREADS_ROUTE}). */
export function chatThreadDeleteRoute(id: number): string {
  return `${CHAT_THREADS_ROUTE}/${id}/delete`;
}

/**
 * `POST /api/v1/chat/threads/{id}/delete` → delete ONE conversation and its
 * per-thread memory by cascade (S-209 producer contract, [FR-UI-26], [FR-UI-20],
 * [ADR-47]). The per-conversation replacement for the retired global clear.
 *
 * Intent-guarded like the turn ([ADR-31]) — a forged or intent-less delete never
 * reaches the handler ([NFR-SE-06]). The caller confirms first; this helper only
 * carries the request. The response is returned raw so the caller can distinguish
 * the server's three honest outcomes: `204` deleted, `404` already gone (an
 * idempotent no-op), anything else a fault to surface.
 */
export function deleteChatThread(id: number): Promise<Response> {
  return apiMutate(withMemberScope(chatThreadDeleteRoute(id)), {});
}
