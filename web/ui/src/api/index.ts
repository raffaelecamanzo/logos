/*
 * The shared `/api/v1` data-access layer (S-186, CR-049, FR-UI-21) — the single
 * import surface every migrated view (S-187–S-189) consumes for typed reads and
 * the loading/empty/error state machine. Pairs with the page-integration pattern
 * in `src/views/index.ts`.
 */

export * from "./types.ts";
export {
  ApiError,
  apiFetch,
  apiUrl,
  fetchArchitecture,
  fetchCoverage,
  fetchFiles,
  fetchGaps,
  fetchGraph,
  fetchHealth,
  fetchImpact,
  fetchNode,
  fetchOverview,
  fetchQuadrant,
  fetchWikiNav,
  fetchWikiPage,
  fetchWikiStatus,
  runQuery,
  searchWiki,
} from "./client.ts";
export type { GraphParams, QueryParams } from "./client.ts";
export {
  ConfigMutateError,
  applyConfig,
  fetchConfig,
  saveConfig,
  saveSecret,
} from "./configClient.ts";
export type { PolicyFile } from "./configClient.ts";
export { AsyncResource, useApiResource } from "./hooks.tsx";
export type { ApiResource, AsyncResourceProps, ResourceStatus } from "./hooks.tsx";
