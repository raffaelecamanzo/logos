/*
 * The Wiki-tab generation runtime (S-178, CR-047, FR-WK-18, FR-UI-19, NFR-SE-07;
 * re-attach behavior CR-056, S-222, S-223).
 *
 * `useWikiGeneration` drives the background, single-run generation pass from the
 * EXISTING intent-guarded SSE client (`wikiGenClient.ts`) and the EXISTING pure
 * per-run reducer (`wikiGenModel.ts`) — the surface holds no generation logic
 * ([ADR-01]); the wiki-agent owns the run ([ADR-42]). When `active` (a configured
 * provider AND accepted consent), it POSTs the trigger ONCE on the Wiki-tab open,
 * consumes the per-page `WikiProgress` SSE stream, and exposes a `refreshKey` that
 * bumps on each page write so the view re-fetches the affected read-models — the
 * "existing pages render immediately, refreshes stream in" behavior ([FR-WK-18]).
 *
 * The run's lifetime is owned by the SERVER's application state, not this hook's
 * mount ([CR-056], [S-222]): unmounting (leaving the Wiki tab) aborts only this
 * client's `fetch`/subscription — an in-flight run keeps going server-side and does
 * NOT release the single-run lock. Remounting (reopening the tab) fires the SAME
 * trigger again with a fresh `startedRef`; the server answers a re-open mid-run by
 * **re-attaching** this new POST to the SAME run's cumulative progress instead of
 * starting a second one or reporting `busy` ([S-223], [FR-UI-19]) — the reattached
 * stream replays the run's history first, so `applyWikiFrame` folds it into the same
 * cumulative `written`/`total` state a subscriber present from the start would have,
 * never resetting to "page 1 of N". This hook needs no reattach-specific logic: it
 * consumes whatever frames the trigger route sends, exactly as it always has. A
 * genuinely idle wiki (no run in flight, nothing queued) sends no frames at all, so
 * `state.phase` stays `"idle"` and the separate work-list/status read
 * (`WikiLanding`'s freshness banner) is what shows "up to date" — this hook never
 * fabricates a run indication for a trigger that did nothing ([NFR-CC-04]). The
 * masked key never reaches this layer (NFR-SE-07): the trigger sends only the
 * intent header, no body.
 */

import { useEffect, useRef, useState } from "react";

import { readSseStream } from "../../api/sse.ts";
import { streamWikiGeneration } from "../../api/wikiGenClient.ts";
import { applyWikiFrame, initialWikiGenState, type WikiGenState } from "./wikiGenModel.ts";

/** The observable result of {@link useWikiGeneration}: the folded run state plus a
 *  `refreshKey` that increments on each page write (the view threads it into its
 *  read-model fetches so a refreshed page reloads as it completes). */
export interface WikiGeneration {
  state: WikiGenState;
  refreshKey: number;
}

/**
 * Trigger + stream one background wiki-generation run when `active` becomes true (a
 * configured provider AND accepted consent). Fires the intent-guarded SSE `POST`
 * once per activation; folds each `WikiProgress` frame into the run state and bumps
 * `refreshKey` on every `page-written` so the view reloads the refreshed page/menu.
 * Aborts this client's `fetch` on unmount; a remount re-fires the trigger, which the
 * server answers by **re-attaching** to a still-in-flight run rather than starting a
 * second one ([S-223]).
 */
export function useWikiGeneration(active: boolean): WikiGeneration {
  const [state, setState] = useState<WikiGenState>(initialWikiGenState());
  const [refreshKey, setRefreshKey] = useState(0);
  const abortRef = useRef<AbortController | null>(null);
  // Fire the trigger once per activation — a re-render must not re-POST. The server
  // single-run lock is the backstop if a double-fire slips through; a genuine remount
  // (a fresh hook instance, e.g. reopening the Wiki tab) resets this ref and DOES
  // re-fire, which is exactly how re-attach kicks in ([S-223]).
  const startedRef = useRef(false);

  // Unmounting (leaving the Wiki tab) aborts only THIS client's `fetch`/subscription
  // — the run itself is owned by server application state, not this view's lifetime
  // ([CR-056], [S-222]), so an in-flight run keeps going and the single-run lock is
  // NOT released. Without this abort the dropped `fetch` would otherwise linger.
  useEffect(() => () => abortRef.current?.abort(), []);

  useEffect(() => {
    if (!active || startedRef.current) return;
    startedRef.current = true;

    const controller = new AbortController();
    abortRef.current = controller;

    void (async () => {
      try {
        const resp = await streamWikiGeneration(controller.signal);
        if (!resp.ok || !resp.body) {
          setState((s) => ({
            ...s,
            phase: "error",
            message: `Wiki generation could not start (status ${resp.status}).`,
          }));
          return;
        }
        await readSseStream(resp.body, (frame) => {
          setState((prev) => applyWikiFrame(prev, frame));
          // A completed page re-reads fresh: bump the key so the view reloads the
          // menu/landing/page read-models ([FR-WK-18] per-page refresh).
          if (frame.name === "page-written") setRefreshKey((k) => k + 1);
        });
      } catch (e) {
        // An aborted run (unmount / navigation) is not a fault to surface.
        if (controller.signal.aborted) return;
        const message = e instanceof Error ? e.message : String(e);
        setState((s) => ({ ...s, phase: "error", message: `Wiki generation failed: ${message}` }));
      }
    })();
  }, [active]);

  return { state, refreshKey };
}
