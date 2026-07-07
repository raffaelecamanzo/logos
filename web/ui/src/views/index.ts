/*
 * The view registry (S-186, CR-049, ADR-43) — the SHARED page-integration
 * pattern every per-tab migration (S-187–S-191) plugged into.
 *
 * The pattern:
 *   client route (`nav.ts` `path`)
 *     → this registry maps the path to a React view component
 *       → `App.tsx` mounts it in the `AppShell` content slot, keyed off
 *         `usePathname()`
 *         → the view renders exclusively through the S-193 design-system
 *           components over the `src/api` data-access layer.
 *
 * Every tab is registered here. The SPA is the sole renderer (server-rendered
 * stack decommissioned in S-192). The Dashboard is now at `/` (S-194); the
 * retired `/overview` route issues a client-side redirect in App.tsx.
 */

import type { ComponentType } from "react";

import { ArchitectureView } from "./architecture/ArchitectureView.tsx";
import { ChatView } from "./chat/ChatView.tsx";
import { ConfigView } from "./config/ConfigView.tsx";
import { DashboardView } from "./dashboard/DashboardView.tsx";
import { GapsView } from "./gaps/GapsView.tsx";
import { GraphView } from "./graph/GraphView.tsx";
import { HealthView } from "./health/HealthView.tsx";
import { StatisticsView } from "./statistics/StatisticsView.tsx";
import { CoverageView } from "./analytics/CoverageView.tsx";
import { FilesView } from "./analytics/FilesView.tsx";
import { QuadrantView } from "./analytics/QuadrantView.tsx";
import { WikiView } from "./wiki/WikiView.tsx";

/** A view is a plain, prop-less component mounted in the shell content slot. */
export type ViewComponent = ComponentType;

/** Path → React view. Mirrors the route paths in `nav.ts`. */
export const VIEW_REGISTRY: Readonly<Record<string, ViewComponent>> = {
  // S-194: Dashboard lives at `/`; `/overview` issues a client-side redirect.
  "/": DashboardView,
  "/health": HealthView,
  "/graph": GraphView,
  // S-190 — the Chat tab: a React SSE client over the intent-guarded `POST /chat`.
  "/chat": ChatView,
  // S-188 — the Files & Risk, Coverage, and Quadrant read-only display tabs.
  "/files": FilesView,
  "/coverage": CoverageView,
  "/quadrant": QuadrantView,
  // S-189 — the Architecture/Cycles, Gaps, and Wiki tabs.
  "/architecture": ArchitectureView,
  "/gaps": GapsView,
  // The Wiki tab owns its own client sub-routes — `/wiki`, `/wiki/search`, and the
  // `/wiki/page/*` reader — via the longest-prefix match in `viewForPath`.
  "/wiki": WikiView,
  // S-235 — the read-only Statistics view over GET /api/v1/statistics (CR-058),
  // sitting directly above Config in the last sidebar group.
  "/statistics": StatisticsView,
  // S-191 — the Config policy editor (the last interactive tab, the SPA's only
  // mutating surface) over the unchanged intent-guarded config POSTs (ADR-31).
  "/config": ConfigView,
};

/**
 * The React view for a client pathname, or `null` when the path owns no view.
 *
 * An exact route wins; otherwise the **longest registered route that owns this
 * sub-path** does (`/wiki/page/x` → `/wiki`), so a tab can host its own client
 * sub-routes (the Wiki reader's index/search/page) inside one mounted view.
 */
export function viewForPath(pathname: string): ViewComponent | null {
  const exact = VIEW_REGISTRY[pathname];
  if (exact) return exact;
  let best: ViewComponent | null = null;
  let bestLen = -1;
  for (const [route, view] of Object.entries(VIEW_REGISTRY)) {
    if (pathname.startsWith(`${route}/`) && route.length > bestLen) {
      best = view;
      bestLen = route.length;
    }
  }
  return best;
}
