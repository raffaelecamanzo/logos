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

import { ApiError, apiMutate } from "../intent.ts";
import { apiFetch, apiUrl } from "./client.ts";
import { ConfigMutateError, FORM_HEADERS, detailOf, formBody, writeSecret } from "./configClient.ts";
import type { StatisticsWindow } from "./statisticsClient.ts";
import type {
  ManifestSaveOutcome,
  SecretWriteOutcome,
  WorkspaceGovernanceAnswer,
  WorkspaceManifestDocument,
  WorkspaceReachabilityAnswer,
  WorkspaceRoster,
  WorkspaceStatistics,
  WorkspaceStatus,
  WorkspaceTierDocument,
  WorkspaceTierSaveOutcome,
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

/**
 * `GET /api/v1/workspace/manifest` (S-430, FR-UI-38) — `logos.workspace.toml` as
 * the workspace Config editor loads it: the literal document, the fingerprint a
 * save must post back, and the parse verdict. A manifest broken on disk still
 * loads (`parsed: null`, `error` set) — the editor is its repair path.
 *
 * App-level: no `?repo=`, like every `workspace/*` read.
 */
export function fetchWorkspaceManifest(): Promise<WorkspaceManifestDocument> {
  return apiFetch<WorkspaceManifestDocument>("workspace/manifest");
}

/** The `/api/v1` family's JSON error body (`{ "error": "…" }`) as its message, or
 *  {@link detailOf}'s verbatim text when the body is not that shape. */
async function workspaceErrorDetail(res: Response): Promise<string> {
  const text = await detailOf(res);
  try {
    const parsed = JSON.parse(text) as { error?: unknown };
    if (typeof parsed.error === "string") return parsed.error;
  } catch {
    // Not JSON — the verbatim text is the honest detail.
  }
  return text;
}

/**
 * `POST /api/v1/workspace/manifest/save` (S-430, FR-UI-38) — save the whole
 * candidate manifest against the `fingerprint` the editor loaded, through the
 * intent-guarded {@link apiMutate} seam (ADR-31, NFR-SE-06).
 *
 * Resolves with the {@link ManifestSaveOutcome} for `written`, `unchanged` AND
 * `conflict`: a `409` is not a fault but the server declining to clobber an edit
 * made on disk since the load, and its body carries what is on disk now so the
 * view can put the choice to the user. Every other non-2xx throws a
 * {@link ConfigMutateError} carrying the server's message — `422` for a candidate
 * the parser rejects (the file untouched), `400` for a missing fingerprint, `500`
 * for an I/O fault.
 */
export async function saveWorkspaceManifest(
  content: string,
  fingerprint: string,
): Promise<ManifestSaveOutcome> {
  const res = await apiMutate(apiUrl("workspace/manifest/save"), {
    headers: FORM_HEADERS,
    body: formBody({ content, fingerprint }),
    credentials: "same-origin",
  });
  if (res.ok || res.status === 409) return (await res.json()) as ManifestSaveOutcome;
  throw new ConfigMutateError(res.status, await workspaceErrorDetail(res));
}

/**
 * `GET /api/v1/workspace/config` (S-450, S-451 T2, FR-WS-30) — the workspace
 * root's own config tier, `<workspace-root>/.logos/`, as the workspace Config
 * editor's chat group loads it: the literal `config.toml` with the fingerprint a
 * save must post back, the **masked** credential (NFR-SE-07), and an
 * effective-chat slice whose origins are relative to that root.
 *
 * Like the manifest read, a tier file broken on disk still loads — `parsed:
 * null` with the fault in `error`, or `chat_key: null` with it in
 * `chat_key_error`, by file and position or key only, never a fragment of the
 * file — because the editor is its repair path.
 *
 * A `2xx` that is not that document is refused here rather than handed on: the
 * editor's Save replaces the whole file with its raw pane, so an editor seeded
 * from a payload with no document in it would offer to overwrite the tier with
 * nothing, and one missing its parse verdict or key state would throw while
 * rendering and take the whole page down with it, the manifest group included
 * (NFR-RA-05). A `null` half is accepted only with the fault that explains it.
 *
 * App-level: no `?repo=`, like every `workspace/*` read.
 */
export async function fetchWorkspaceConfig(): Promise<WorkspaceTierDocument> {
  const model = await apiFetch<WorkspaceTierDocument>("workspace/config");
  // Every field the editor reads to seed itself: the raw pane and its load
  // fingerprint, the typed [chat] fields (or the fault that stands for them), and
  // the masked key badge (or the fault that stands for it).
  const config = model?.config;
  const parseState =
    config?.parsed === null
      ? typeof config.error === "string"
      : typeof config?.parsed?.chat?.provider === "string";
  const keyState =
    model?.chat_key === null
      ? typeof model.chat_key_error === "string"
      : typeof model?.chat_key?.present === "boolean";
  if (typeof config?.content !== "string" || typeof config.fingerprint !== "string" || !parseState || !keyState) {
    throw new Error("GET /api/v1/workspace/config answered without a config document or key state.");
  }
  return model;
}

/**
 * `POST /api/v1/workspace/config/save` (S-450, S-451 T2, FR-WS-30) — save the
 * candidate `content` as `<workspace-root>/.logos/config.toml` against the
 * `fingerprint` the editor loaded, through the intent-guarded {@link apiMutate}
 * seam (ADR-31, NFR-SE-06). The workspace-root twin of
 * {@link saveWorkspaceManifest}: `file=config` is the only document this root
 * carries (a `rules.toml` here would be read by nothing), and the server reaches
 * no member — it writes only under `<workspace-root>/.logos/` (the file, and the
 * managed `.gitignore` on a first save) and runs no pipeline.
 *
 * Resolves with the {@link WorkspaceTierSaveOutcome} for `written`, `unchanged`
 * AND `conflict`: a `409` is the server declining to clobber an edit made on disk
 * since the load, and its body carries what is on disk now. Every other non-2xx
 * throws a {@link ConfigMutateError} carrying the server's message — `422` for a
 * refused candidate (file left byte-identical), `400` for a missing fingerprint,
 * `500` for an I/O fault.
 */
export async function saveWorkspaceConfig(
  content: string,
  fingerprint: string,
): Promise<WorkspaceTierSaveOutcome> {
  const res = await apiMutate(apiUrl("workspace/config/save"), {
    headers: FORM_HEADERS,
    body: formBody({ file: "config", content, fingerprint }),
    credentials: "same-origin",
  });
  if (res.ok || res.status === 409) return (await res.json()) as WorkspaceTierSaveOutcome;
  throw new ConfigMutateError(res.status, await workspaceErrorDetail(res));
}

/**
 * `POST /api/v1/workspace/config/secret` (S-450, FR-CF-06, NFR-SE-07) — write (or,
 * with a blank key, clear) the credential every member that declares none
 * inherits, into the owner-only `<workspace-root>/.logos/secrets.toml`.
 *
 * Write-only on the same terms as `saveSecret`, through the one {@link writeSecret}
 * both call: it resolves with the **masked**
 * {@link SecretWriteOutcome} (presence + last-4) and never returns the response
 * body — a non-JSON `2xx` resolves to `null`, and a non-`2xx` throws with a fixed,
 * body-free detail, so no reply from this route can carry key material onto a
 * SPA surface.
 */
export function saveWorkspaceSecret(apiKey: string): Promise<SecretWriteOutcome | null> {
  return writeSecret(apiUrl("workspace/config/secret"), apiKey);
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
