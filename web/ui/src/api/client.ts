/*
 * The typed `/api/v1` fetch client (S-186, CR-049, FR-UI-21) — the SHARED
 * data-access layer's transport half, reused by every migrated view
 * (S-187–S-189).
 *
 * It is a thin, typed layer over the foundation's same-origin {@link apiGet}
 * seam (`src/intent.ts`): `apiGet` already enforces same-origin GET + honest
 * `ApiError` on a non-2xx (NFR-SE-01, NFR-RA-05). This module adds (a) the
 * `/api/v1` base path so callers name only the endpoint, and (b) a query-string
 * builder that drops empty/undefined params so a request stays byte-identical to
 * the legacy contract when a filter is unset. No store is mutated on read
 * (ADR-28): every call here is a GET.
 *
 * Reads carry no intent token (the surface is GET-only on `/api/v1`); a future
 * mutating view uses `apiMutate` from `src/intent.ts` directly.
 */

import { apiGet, ApiError } from "../intent.ts";
import type {
  ArchitectureModel,
  CoverageModel,
  FilesModel,
  GapsModel,
  GraphElements,
  GraphGranularity,
  HealthModel,
  ImpactResult,
  NodeInfo,
  OverviewModel,
  QueryResponse,
  WikiHit,
  WikiNav,
  WikiPageView,
  WikiStatus,
} from "./types.ts";

export { ApiError };

/** The same-origin base every endpoint hangs off (FR-UI-21). */
const API_BASE = "/api/v1";

/** A query-param value the builder accepts; `undefined`/`null`/`""` are omitted. */
export type ParamValue = string | number | boolean | null | undefined;

/** Any params bag the builder accepts (the typed endpoint interfaces satisfy it). */
export type Params = Record<string, ParamValue>;

/**
 * Build an `/api/v1/<endpoint>` URL with a query string, **omitting** any param
 * whose value is `undefined`, `null`, or `""` — so an unset filter leaves the
 * request byte-identical to the no-filter contract (the canvas's
 * all-on ⇒ no-param invariant, S-122). A `false` boolean is also omitted (a
 * toggle that is off carries no param, matching the `?intent=` contract); pass a
 * truthy value to include a flag. Values are URL-encoded.
 */
export function apiUrl(endpoint: string, params?: Params): string {
  const path = `${API_BASE}/${endpoint.replace(/^\/+/, "")}`;
  if (!params) return path;
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "" || value === false) continue;
    search.set(key, String(value));
  }
  const qs = search.toString();
  return qs ? `${path}?${qs}` : path;
}

/** A typed GET against an `/api/v1` endpoint with optional query params. */
export function apiFetch<T>(endpoint: string, params?: Params): Promise<T> {
  return apiGet<T>(apiUrl(endpoint, params));
}

// ── Endpoint-typed helpers (the read-model surface the Graph view consumes) ──
// One function per `/api/v1` endpoint this story touches, each returning its
// typed read-model. S-187–S-189 add their own beside these.

/** Parameters for the graph-elements endpoint (mirrors the canvas's `/api/v1/graph` contract). */
export interface GraphParams {
  /** Seed symbol to scope the snapshot to; omit for the whole graph. */
  seed?: string;
  /** Visible-element budget. */
  cap?: number;
  /** Comma-joined layer wire tokens, or omit when every layer is on. */
  layers?: string;
  /** Comma-joined edge-type wire tokens, or omit when every type is on. */
  edge_types?: string;
  /** Semantic tier; omit at the default `symbol` tier. */
  granularity?: GraphGranularity;
  /** Truthy to activate the documentation-intent overlay; omit when off. */
  intent?: boolean;
}

/** `GET /api/v1/graph` — the read-only nodes+edges snapshot (FR-UI-08). */
export function fetchGraph(params: GraphParams = {}): Promise<GraphElements> {
  // `symbol` granularity carries no param (byte-identical to the pre-rollup fetch).
  const granularity = params.granularity === "symbol" ? undefined : params.granularity;
  return apiFetch<GraphElements>("graph", { ...params, granularity } as Params);
}

/** Parameters for the structured + relational whole-graph query (FR-UI-14). */
export interface QueryParams {
  /** Field-filter search term. */
  q?: string;
  /** Kind filter (snake_case wire token). */
  kind?: string;
  /** Layer filter (`code`/`doc`/`artifact`); the server reports an unknown layer honestly. */
  layer?: string;
  /** File-path substring filter. */
  file?: string;
  /** Relational verb (`callers-of`/`callees-of`/`impact-of`); takes precedence over `q`. */
  verb?: string;
  /** Relation target symbol. */
  target?: string;
}

/** `GET /api/v1/query` — the read-only structured + relational query (FR-UI-14). */
export function runQuery(params: QueryParams): Promise<QueryResponse> {
  return apiFetch<QueryResponse>("query", params as Params);
}

/** `GET /api/v1/impact?seed=<symbol>` — the Decisions-panel impact read-model (FR-NV-10). */
export function fetchImpact(seed: string): Promise<ImpactResult> {
  return apiFetch<ImpactResult>("impact", { seed });
}

/** `GET /api/v1/node?symbol=<sym>` — the single-symbol detail read-model (FR-NV-04). */
export function fetchNode(symbol: string): Promise<NodeInfo> {
  return apiFetch<NodeInfo>("node", { symbol });
}

/** `GET /api/v1/overview` — the Dashboard roll-up bundle (FR-UI-09). */
export function fetchOverview(): Promise<OverviewModel> {
  return apiFetch<OverviewModel>("overview");
}

/** `GET /api/v1/health` — the Health gate/metrics/evolution bundle (FR-UI-04). */
export function fetchHealth(): Promise<HealthModel> {
  return apiFetch<HealthModel>("health");
}

// ── Files & Risk / Coverage (S-188, FR-UI-11) ──
// Read-only twins of the server-rendered analytics views, each over the
// already-registered `/api/v1` handler (web/src/api_v1.rs) — no new Rust handler:
// the foundation suite (S-183) shipped these endpoints behind the legacy UI.

/** `GET /api/v1/files[?untested][?production_scope]` — the Files & Risk bundle
 *  (FR-UI-11). The `untested` toggle scopes the board to files lacking fresh
 *  positive coverage, exactly as the server-rendered view's filter does; the
 *  `productionScope` toggle drops whole test files from the candidate set
 *  before ranking (CR-076), matching the CLI `--production-scope` flag / MCP
 *  `production_scope` argument. */
export function fetchFiles(untested = false, productionScope = false): Promise<FilesModel> {
  return apiFetch<FilesModel>("files", { untested, production_scope: productionScope });
}

/** `GET /api/v1/coverage` — the Coverage bundle (FR-UI-11). */
export function fetchCoverage(): Promise<CoverageModel> {
  return apiFetch<CoverageModel>("coverage");
}

// ── Display-tab read-models (S-189: Architecture / Gaps / Wiki) ────────────────

/** `GET /api/v1/architecture` — the Architecture / Cycles (DSM) bundle (FR-UI-21). */
export function fetchArchitecture(): Promise<ArchitectureModel> {
  return apiFetch<ArchitectureModel>("architecture");
}

/** `GET /api/v1/gaps` — the Rule-findings bundle (status + rules, CR-079). */
export function fetchGaps(): Promise<GapsModel> {
  return apiFetch<GapsModel>("gaps");
}

/** `GET /api/v1/wiki` — the dual-axis `wiki status` freshness read-model (FR-UI-06). */
export function fetchWikiStatus(): Promise<WikiStatus> {
  return apiFetch<WikiStatus>("wiki");
}

/** `GET /api/v1/wiki/nav` — the four-tier wiki menu IA (FR-UI-06, S-189). */
export function fetchWikiNav(): Promise<WikiNav> {
  return apiFetch<WikiNav>("wiki/nav");
}

/** `GET /api/v1/wiki/search?q=<term>` — the FTS hits over the wiki (FR-WK-05). An
 *  empty term is an honest empty list at the surface, so the caller need not guard. */
export function searchWiki(q: string): Promise<WikiHit[]> {
  return apiFetch<WikiHit[]>("wiki/search", { q });
}

/** `GET /api/v1/wiki/page/*slug` — the agent wiki-page presentation bundle with the
 *  server-rendered safe HTML body (FR-UI-06, S-189). The path-like slug rides the
 *  `*slug` wildcard; each segment is URL-encoded but the `/` separators are kept. */
export function fetchWikiPage(slug: string): Promise<WikiPageView> {
  const encoded = slug.split("/").map(encodeURIComponent).join("/");
  return apiFetch<WikiPageView>(`wiki/page/${encoded}`);
}
