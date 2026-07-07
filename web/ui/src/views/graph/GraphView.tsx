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

import { AsyncResource, fetchGraph, useApiResource } from "../../api/index.ts";
import type { GraphElements } from "../../api/types.ts";
import { EmptyState } from "../../components/index.ts";
import { GraphExplorer } from "./GraphExplorer.tsx";
import { DEFAULT_CAP } from "./graphModel.ts";

export function GraphView() {
  const graph = useApiResource<GraphElements>(() => fetchGraph({ cap: DEFAULT_CAP }), []);
  return (
    <AsyncResource
      resource={graph}
      loadingLabel="Loading the graph…"
      isEmpty={(elements) => elements.nodes.length === 0}
      empty={<EmptyState message="No graph elements yet — run" command="logos index" />}
    >
      {(elements) => <GraphExplorer initial={elements} />}
    </AsyncResource>
  );
}
