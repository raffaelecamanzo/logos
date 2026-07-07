/*
 * The Statistics view's data-access layer (S-235, CR-058, FR-UI-27, FR-OB-04) —
 * a thin, typed GET over the same-origin `/api/v1/statistics` endpoint (S-234).
 *
 * Like every read surface it goes through the shared `apiFetch` seam: same-origin,
 * GET-only, no intent token, honest `ApiError` on a non-2xx (NFR-SE-01, NFR-RA-05).
 * Viewing the tab mutates no store (ADR-28) — this module only ever GETs. The
 * window is a lenient query param the server clamps/defaults; an unset or empty
 * value leaves the request byte-identical to the default-window contract.
 */

import { apiFetch } from "./client.ts";
import type { StatsInfo } from "./types.ts";

/** The three windows the Statistics tab offers, in days; the first is the default. */
export const STATISTICS_WINDOWS = [7, 30, 90] as const;

/** A window the selector offers (7 / 30 / 90 days). */
export type StatisticsWindow = (typeof STATISTICS_WINDOWS)[number];

/** The default reporting window (FR-OB-04): the trailing 7 days. */
export const DEFAULT_STATISTICS_WINDOW: StatisticsWindow = 7;

/**
 * `GET /api/v1/statistics?window=<days>` — the enriched telemetry read-model
 * (FR-UI-27): usage/latency/value plus the daily-activity series and the
 * dev-vs-`main` origin split. A pure read; an empty store degrades to a zeroed
 * model carrying a `warnings` note, never an error (NFR-CC-04).
 */
export function fetchStatistics(window: StatisticsWindow): Promise<StatsInfo> {
  return apiFetch<StatsInfo>("statistics", { window });
}
