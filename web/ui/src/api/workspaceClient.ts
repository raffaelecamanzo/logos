/*
 * The workspace data-access layer (S-250, CR-061, FR-UI-29) — typed reads over the
 * S-249 `/api/v1/workspace/*` cross-service fan-out.
 *
 * These endpoints exist ONLY in workspace mode. A single-root serve answers them
 * `404` with an honest "not a workspace" body ([FR-WS-06], ADR-52) — which is
 * precisely how the SPA discovers its own mode: {@link probeWorkspace} treats that
 * `404` as "single-root", not as a failure, and every other status still throws so
 * a genuine fault is never mistaken for a plain repo ([NFR-RA-05]).
 *
 * Unlike the per-view reads these are **app-level**: they must NOT carry the shell's
 * member scope (`?repo=` on a fan-out narrows it, and the service map is deliberately
 * a view of every member). `apiUrl` exempts the `workspace/*` prefix for exactly
 * that reason; the one place a member is passed here, it is passed explicitly.
 */

import { ApiError } from "../intent.ts";
import { apiFetch } from "./client.ts";
import type { WorkspaceStatus, XserviceImpact, XserviceRouteProviders } from "./types.ts";

/** `GET /api/v1/workspace/status` — the member roster + the cross-service coverage
 *  summary (the selector's list, the service map's nodes, the dashboard's figures). */
export function fetchWorkspaceStatus(): Promise<WorkspaceStatus> {
  return apiFetch<WorkspaceStatus>("workspace/status");
}

/** `GET /api/v1/workspace/route-providers` — every resolved cross-service binding:
 *  the service map's edges. App-level (unscoped) by design. */
export function fetchWorkspaceBindings(): Promise<XserviceRouteProviders> {
  return apiFetch<XserviceRouteProviders>("workspace/route-providers");
}

/** `GET /api/v1/workspace/impact?symbol=<s>` — the cross-service impact of a symbol:
 *  the seed member(s)' own impact plus each far-side impact stitched across a
 *  binding. `member` optionally scopes the seed side to one member. */
export function fetchWorkspaceImpact(symbol: string, member?: string): Promise<XserviceImpact> {
  return apiFetch<XserviceImpact>("workspace/impact", { symbol, repo: member });
}

/** What the boot-time probe found: a workspace (with its status) or a plain repo. */
export type WorkspaceProbe =
  | { mode: "workspace"; status: WorkspaceStatus }
  | { mode: "single" };

/**
 * Discover whether this serve is a workspace, from the fan-out's own honest `404`
 * ([FR-WS-06]). A `404` is the *answer* "this is not a workspace" — not an error —
 * so it resolves to `{ mode: "single" }` and the shell renders no selector. Any
 * other failure (a 500, a transport fault) is rethrown: a broken read must never
 * masquerade as a single-root repo, which would silently hide the workspace UI.
 */
export async function probeWorkspace(): Promise<WorkspaceProbe> {
  try {
    return { mode: "workspace", status: await fetchWorkspaceStatus() };
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) return { mode: "single" };
    throw err;
  }
}
