/*
 * The Wiki tab's generation-trigger data-access layer (S-178, CR-047, FR-WK-18,
 * FR-UI-19, NFR-SE-06).
 *
 * Opening the Wiki tab triggers a background, single-run wiki-agent generation pass
 * and streams its per-page progress. Starting a run MUTATES (consent-gated egress +
 * `wiki write`), so — exactly like the chat turn (`chatClient.ts`) — it rides the
 * intent-guarded `POST` seam (`apiMutate`, `src/intent.ts`) so it carries the
 * same-origin + per-session intent token the server's guard requires; the streamed
 * progress is consumed as SSE over that `POST` via `fetch` (not a `GET`
 * `EventSource`, which cannot set the custom intent header). The config-state read
 * (for the configure-first gate + the consent disclosure) is a plain `/api/v1` GET.
 *
 * Lives in its own module (not the shared `client.ts`) so the Wiki read-model client
 * and this mutating trigger stay separable — the only shared SPA wiring is the view.
 */

import { apiFetch } from "./client.ts";
import { apiMutate } from "../intent.ts";
import { withMemberScope } from "../workspace/scope.ts";
import type { ConfigReadModel } from "./types.ts";

/** The intent-guarded wiki-generation trigger route (mirrors `web::WIKI_GENERATE_ROUTE`). */
export const WIKI_GENERATE_ROUTE = "/wiki/generate";

/**
 * `GET /api/v1/config` → the full config read-model. The Wiki tab reads the
 * `[chat]`/`[wiki]` policy (provider/model/endpoint) plus the MASKED key's presence
 * to decide its configure-first state and to disclose the endpoint in the consent
 * banner. A pure read — no token, no store mutation ([ADR-28]); the masked key is
 * never rendered (NFR-SE-07).
 */
export function fetchWikiConfig(): Promise<ConfigReadModel> {
  return apiFetch<ConfigReadModel>("config");
}

/**
 * Trigger a wiki-generation run — `POST /wiki/generate` with
 * `Accept: text/event-stream`, carrying the intent header (NFR-SE-06), streaming the
 * per-page `WikiProgress` SSE events back. The server's single-run lock guarantees
 * exactly one background run; a concurrent open streams a single `busy` frame. The
 * `signal` ties the run's lifetime to the caller (unmount / a superseding open →
 * abort → the server cancels the in-flight run and releases the lock, [FR-UI-19]).
 * The trigger carries no body — the runner checks the work-list server-side.
 */
export function streamWikiGeneration(signal?: AbortSignal): Promise<Response> {
  return apiMutate(withMemberScope(WIKI_GENERATE_ROUTE), {
    headers: { Accept: "text/event-stream" },
    signal,
  });
}
