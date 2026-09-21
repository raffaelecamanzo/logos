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
import type { StatisticsWindow } from "./statisticsClient.ts";
import type {
  WorkspaceGovernanceAnswer,
  WorkspaceReachabilityAnswer,
  WorkspaceRoster,
  WorkspaceStatistics,
  WorkspaceStatus,
  XserviceImpact,
  XserviceRouteProviders,
} from "./types.ts";

/** `GET /api/v1/workspace/roster` — the manifest-only roster (name, default member,
 *  member names). The shell's boot probe: it starts NO member engine, so opening the
 *  dashboard cannot eagerly warm all N members (NFR-PE-10). */
export function fetchWorkspaceRoster(): Promise<WorkspaceRoster> {
  return apiFetch<WorkspaceRoster>("workspace/roster");
}

/** `GET /api/v1/workspace/status` — per-member index freshness + the cross-service
 *  coverage summary. This one DOES fan out over every member, so it is fetched only
 *  by the app-level views that exist to show exactly that — the Workspace tab and
 *  S-428's Workspace Dashboard and Workspace Health — and never by the shell. */
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

/**
 * `GET /api/v1/workspace/reachability` (S-427, FR-WS-28, FR-WS-12) — the app-wide
 * cross-service reachability union view, bounded and saying so.
 *
 * **No `all` param is ever sent.** The server's promotions-only default carries
 * the usually-tiny promotion set and suppresses the per-repo dead set to `null`;
 * lifting it pulls an estimated ~500 KB of claims for a large workspace, which no
 * view here renders (CR-084, NFR-PE-10). Every applied bound comes back in
 * `reachability.scope`, so the reply states its own bounds rather than relying on
 * the caller to remember them.
 *
 * `member` scopes the tallies and claims to one member. It is passed EXPLICITLY,
 * never picked up from the shell's ambient scope — `apiUrl` exempts the
 * `workspace/*` prefix for that reason, and these views are app-level.
 */
export function fetchWorkspaceReachability(member?: string): Promise<WorkspaceReachabilityAnswer> {
  return apiFetch<WorkspaceReachabilityAnswer>("workspace/reachability", { repo: member });
}

/** `GET /api/v1/workspace/check` (S-427, FR-WS-28, FR-WS-13) — the workspace
 *  governance report, advisory and never a gate input (ADR-56).
 *
 *  Its `governance` is `null` over a workspace that declares no rules: the honest
 *  empty, and the caller must render it as "nothing was checked" rather than as a
 *  passing report (NFR-CC-04). */
export function fetchWorkspaceGovernance(): Promise<WorkspaceGovernanceAnswer> {
  return apiFetch<WorkspaceGovernanceAnswer>("workspace/check");
}

/**
 * `GET /api/v1/workspace/statistics?window=<days>` (S-429, FR-UI-37) — usage
 * summed across every member over one trailing window, carrying the member
 * denominator it summed over and naming every member it could not read.
 *
 * It fans out, but **not through `Engine::stats`**: each member's `telemetry.db`
 * is opened read-only on its own, so a view load constructs no member engine and
 * the resident-engine count is what a `workspace status` already pays
 * ([NFR-PE-10]). That is the whole reason this read exists as its own endpoint
 * rather than as N calls to `/api/v1/statistics?repo=`.
 *
 * App-level, so it carries **no** `?repo=` — `apiUrl` exempts the `workspace/*`
 * prefix, and narrowing this to the shell's selected member would answer a
 * different question from the one the view asks. The `window` is the same lenient
 * query param `/api/v1/statistics` takes; the server clamps and defaults it.
 *
 * Always `200` in workspace mode: an unreadable member is named *in the payload*,
 * never raised as an error ([NFR-RA-05]). A single-root serve answers `404`, like
 * every other `workspace/*` route.
 */
export function fetchWorkspaceStatistics(window: StatisticsWindow): Promise<WorkspaceStatistics> {
  return apiFetch<WorkspaceStatistics>("workspace/statistics", { window });
}

/** What the boot-time probe found: a workspace (with its roster) or a plain repo. */
export type WorkspaceProbe =
  | { mode: "workspace"; roster: WorkspaceRoster }
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
    return { mode: "workspace", roster: await fetchWorkspaceRoster() };
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) return { mode: "single" };
    throw err;
  }
}
