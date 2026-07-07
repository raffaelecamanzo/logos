/*
 * Intent-token bootstrap and the same-origin fetch seam (S-185, CR-049,
 * FR-UI-21, NFR-SE-06, ADR-31).
 *
 * The web-surface injects the per-session intent (CSRF) token into the served
 * shell as a `<meta name="logos-intent">` tag. The SPA reads it ONCE at startup
 * (below) and echoes it in the `x-logos-intent` header on every mutating request
 * — the second factor the server's same-origin/intent guard requires. Reads of
 * the `/api/v1` read-model carry no token (the surface is GET-only there).
 *
 * The masked chat key is never delivered here, and this module never logs the
 * token (NFR-SE-07).
 */

/** The request header the server's intent guard checks (mirrors web::INTENT_HEADER). */
const INTENT_HEADER = "x-logos-intent";

/** Read the per-session intent token from the served shell's `<meta>` tag. */
function readIntentToken(): string | null {
  const meta = document.querySelector('meta[name="logos-intent"]');
  const token = meta?.getAttribute("content")?.trim();
  return token && token.length > 0 ? token : null;
}

/**
 * The per-session intent token, read once at module load from the served shell.
 * `null` when absent (e.g. the Node-free placeholder shell, which carries the tag
 * but no bundle to run) — mutating helpers fail loudly rather than send a
 * token-less request the server would (correctly) reject.
 */
export const intentToken: string | null = readIntentToken();

/** A fetch that failed because the response status was not ok. */
export class ApiError extends Error {
  constructor(
    public readonly path: string,
    public readonly status: number,
  ) {
    super(`${path} responded ${status}`);
    this.name = "ApiError";
  }
}

/**
 * A same-origin GET against the `/api/v1` JSON read-model (FR-UI-21). Reads need
 * no intent token. Throws {@link ApiError} on a non-2xx so callers render an
 * honest error state rather than a fabricated figure (NFR-RA-05, NFR-CC-04).
 */
export async function apiGet<T = unknown>(path: string): Promise<T> {
  const res = await fetch(path, { headers: { Accept: "application/json" } });
  if (!res.ok) throw new ApiError(path, res.status);
  return (await res.json()) as T;
}

/**
 * A same-origin **mutating** request (config write/apply/secret) that echoes the
 * intent token in `x-logos-intent` so the server's same-origin/intent guard
 * admits it (NFR-SE-06). Defaults to `POST`. Throws if no token was delivered to
 * the shell — a mutating request without one cannot succeed, so failing here is
 * clearer than a silent 403. The per-tab migrations (Group DA) use this for their
 * mutating actions; the shell itself performs only reads.
 */
export async function apiMutate(path: string, init: RequestInit = {}): Promise<Response> {
  if (!intentToken) {
    throw new Error("no logos-intent token in the shell; cannot issue a mutating request");
  }
  const headers = new Headers(init.headers);
  headers.set(INTENT_HEADER, intentToken);
  return fetch(path, { ...init, method: init.method ?? "POST", headers });
}
