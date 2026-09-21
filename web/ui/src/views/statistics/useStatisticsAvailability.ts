/*
 * useStatisticsAvailability (S-235, CR-058, FR-UI-27, NFR-CC-04) — the sidebar's
 * honest "is there anything to show?" probe for the Statistics nav item.
 *
 * The shell already fetches `/api/v1/health` from the Header on mount; this mirrors
 * that pattern for the Statistics tab: a same-origin GET at the default window. It
 * reports `awaiting = true` ONLY when the read settled and the store is genuinely
 * empty — a loading or failed probe never mutes the item (an unknown state is not an
 * empty one, NFR-CC-04). The nav muting is the same `isStatsEmpty` predicate the
 * view's empty state uses, so the two never disagree.
 *
 * Member scope (S-250): the sidebar sits OUTSIDE the member-keyed view subtree, so it
 * is not remounted when the workspace member changes. The member is therefore an
 * explicit dependency here — without it this probe would keep reporting the member it
 * happened to read at shell mount, and could mute Statistics for a member that has
 * telemetry (or leave it lit for one that has none) while the tab's own empty state,
 * which IS keyed, says the opposite. That is precisely the disagreement the docstring
 * above promises cannot happen.
 *
 * Sitting outside that subtree has a second consequence (S-426, NFR-RA-05). When the
 * URL names a member this workspace does not have, the shell mounts no view — but
 * this probe is not a view, so it would still fire, UNSCOPED, and mute or light the
 * Statistics item from the DEFAULT member's telemetry while the address bar names
 * another member. So it does not fire at all in that state, and the resource stays
 * `loading` — which the contract above already defines as "does not mute": an
 * unknown state is not an empty one.
 *
 * It holds for the same reason until the workspace probe SETTLES. Before that the
 * member is not yet known, so a read now would go out unscoped against the default
 * member and have to be re-issued the moment the mode flips — the two-pass page load
 * the shell's own probe gate exists to avoid. This is the policy the Header already
 * applies to its readout; this probe is the other reader outside the view subtree.
 */

import { fetchStatistics, DEFAULT_STATISTICS_WINDOW } from "../../api/statisticsClient.ts";
import { useApiResource } from "../../api/hooks.tsx";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { isStatsEmpty } from "./statsModel.ts";

/** Whether the Statistics tab is awaiting data (empty store) — drives the muted
 *  nav item. `false` while the probe is loading or after it failed. */
export function useStatisticsAwaiting(): boolean {
  const { cacheKey, mode, unknownMember } = useWorkspace();
  const stats = useApiResource(
    () =>
      mode !== "loading" && unknownMember === null
        ? fetchStatistics(DEFAULT_STATISTICS_WINDOW)
        : // Never settles, so the resource holds at `loading` — the state this hook
          // already treats as "no answer, do not mute". A rejection would be a lie
          // (nothing failed) and a resolved empty would mute the item on no evidence.
          new Promise<never>(() => {}),
    [cacheKey, mode, unknownMember],
  );
  return stats.status === "ready" && stats.data !== undefined && isStatsEmpty(stats.data);
}
