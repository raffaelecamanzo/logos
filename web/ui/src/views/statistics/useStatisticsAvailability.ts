/*
 * useStatisticsAvailability (S-235, CR-058, FR-UI-27, NFR-CC-04) — the sidebar's
 * honest "is there anything to show?" probe for the Statistics nav item.
 *
 * The shell already fetches `/api/v1/health` from the Header on mount; this mirrors
 * that pattern for the Statistics tab: a single same-origin GET at the default
 * window, read once when the shell mounts (the sidebar is not remounted on
 * client-side navigation). It reports `awaiting = true` ONLY when the read settled
 * and the store is genuinely empty — a loading or failed probe never mutes the item
 * (an unknown state is not an empty one, NFR-CC-04). The nav muting is the same
 * `isStatsEmpty` predicate the view's empty state uses, so the two never disagree.
 */

import { fetchStatistics, DEFAULT_STATISTICS_WINDOW } from "../../api/statisticsClient.ts";
import { useApiResource } from "../../api/hooks.tsx";
import { isStatsEmpty } from "./statsModel.ts";

/** Whether the Statistics tab is awaiting data (empty store) — drives the muted
 *  nav item. `false` while the probe is loading or after it failed. */
export function useStatisticsAwaiting(): boolean {
  const stats = useApiResource(() => fetchStatistics(DEFAULT_STATISTICS_WINDOW), []);
  return stats.status === "ready" && stats.data !== undefined && isStatsEmpty(stats.data);
}
