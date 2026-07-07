/*
 * The Chat tab's data-access layer (S-190, CR-049, FR-UI-18, FR-UI-19, NFR-SE-06).
 *
 * The first MUTATING SPA surface: a chat turn and Clear-history both ride the
 * intent-guarded `POST` seam (`apiMutate`, `src/intent.ts`) so they carry the
 * same-origin + per-session intent token the server's guard requires — the
 * streaming turn consumes SSE over that `POST` via `fetch` (not a `GET`
 * EventSource, which cannot set the custom intent header). The config-state read is
 * a plain `/api/v1` GET. The SSE turn contract is UNCHANGED ([chat-agent]); this is
 * a re-homed client.
 *
 * Lives in its own module (not the shared `client.ts`) so the parallel Config
 * migration (S-191) and this one do not collide on the data layer — the only shared
 * SPA wiring both touch is `nav.ts` + the view registry.
 */

import { apiFetch } from "./client.ts";
import { apiMutate } from "../intent.ts";
import type { ChatConfigReadModel } from "../views/chat/chatModel.ts";

/** The intent-guarded chat-turn route (mirrors `web::CHAT_POST_ROUTE`). */
export const CHAT_ROUTE = "/chat";
/** The intent-guarded Clear-history route (mirrors `web::CHAT_CLEAR_ROUTE`). */
export const CHAT_CLEAR_ROUTE = "/chat/clear";

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
 */
export function streamChatTurn(question: string, signal?: AbortSignal): Promise<Response> {
  return apiMutate(CHAT_ROUTE, {
    headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" },
    body: `q=${encodeURIComponent(question)}`,
    signal,
  });
}

/**
 * `POST /chat/clear` → wipe the conversation AND its per-thread memory ([FR-UI-20]).
 * Intent-guarded like the turn.
 */
export function clearChatHistory(): Promise<Response> {
  return apiMutate(CHAT_CLEAR_ROUTE, {});
}
