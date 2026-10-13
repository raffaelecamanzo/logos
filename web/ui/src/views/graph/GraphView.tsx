/*
 * GraphView (S-186, FR-UI-08, FR-UI-21) — the Graph & Decisions tab, the first
 * tab migrated to React over `/api/v1` and the pattern-setter for S-187–S-189.
 *
 * It demonstrates the page-integration pattern end-to-end: registered in
 * `views/index.ts` for the `/graph` client route, mounted by `App.tsx` in the
 * AppShell content slot, rendering exclusively through the S-193 design system. It
 * loads the first graph snapshot through the shared `useApiResource` hook and maps
 * its loading/empty/error states with `AsyncResource` (an honest "run logos index"
 * empty state, never a blank canvas), then hands the snapshot to the stateful
 * `GraphExplorer` for the interactive session. Every read is GET-only — loading the
 * view mutates no store (ADR-28).
 */

import { useEffect, useState } from "react";

import { AsyncResource, fetchGraph, useApiResource } from "../../api/index.ts";
import type { GraphElements } from "../../api/types.ts";
import { EmptyState } from "../../components/index.ts";
import { currentUrl, replaceUrl } from "../../router.tsx";
import { GraphExplorer } from "./GraphExplorer.tsx";
import { DEFAULT_CAP, graphSeedFromSearch, withoutGraphSeed } from "./graphModel.ts";

export function GraphView() {
  // A `/graph?seed=` deep link (S-622, FR-UI-47 — a code reference, or an
  // Architecture cycle participant) opens the view focused on that node. It is
  // CONSUMED at mount: read once, then removed from the URL (the member's `repo`
  // stays), so a member switch — which remounts this view and keeps the query —
  // or a reload after Reset opens the whole graph rather than re-seeding a node of
  // another member's graph. With no seed the opening fetch is byte-identical to
  // the unseeded one.
  const [link] = useState(() => graphSeedFromSearch(window.location.search));
  useEffect(() => {
    if (link.seed) replaceUrl(withoutGraphSeed(currentUrl()));
  }, [link]);
  const graph = useApiResource<GraphElements>(
    () => fetchGraph({ cap: DEFAULT_CAP, seed: link.seed ?? undefined, granularity: link.granularity }),
    [],
  );
  return (
    <AsyncResource
      resource={graph}
      loadingLabel="Loading the graph…"
      isEmpty={(elements) => elements.nodes.length === 0}
      empty={
        link.seed ? (
          <EmptyState message={`Nothing in this graph answers to “${link.seed}”.`} />
        ) : (
          <EmptyState message="No graph elements yet — run" command="logos index" />
        )
      }
    >
      {(elements) => (
        <GraphExplorer initial={elements} initialFocus={link.seed} initialGranularity={link.granularity} />
      )}
    </AsyncResource>
  );
}
