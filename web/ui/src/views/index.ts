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

import { mountedViews } from "../extensions/registry.ts";

import { ArchitectureView } from "./architecture/ArchitectureView.tsx";
import { ConfigView } from "./config/ConfigView.tsx";
import { DashboardView } from "./dashboard/DashboardView.tsx";
import { GapsView } from "./gaps/GapsView.tsx";
import { GraphView } from "./graph/GraphView.tsx";
import { HealthView } from "./health/HealthView.tsx";
import { StatisticsView } from "./statistics/StatisticsView.tsx";
import { WorkspaceStatisticsView } from "./statistics/WorkspaceStatisticsView.tsx";
import { CoverageView } from "./analytics/CoverageView.tsx";
import { FilesView } from "./analytics/FilesView.tsx";
import { WorkspaceConfigView } from "./workspace/WorkspaceConfigView.tsx";
import { WorkspaceDashboardView } from "./workspace/WorkspaceDashboardView.tsx";
import { WorkspaceHealthView } from "./workspace/WorkspaceHealthView.tsx";
import { WorkspaceView } from "./workspace/WorkspaceView.tsx";

/** A view is a plain, prop-less component mounted in the shell content slot. */
export type ViewComponent = ComponentType;

/** Path → React view. Mirrors the route paths in `nav.ts`. */
export const VIEW_REGISTRY: Readonly<Record<string, ViewComponent>> = {
  // S-194: Dashboard lives at `/`; `/overview` issues a client-side redirect.
  "/": DashboardView,
  "/health": HealthView,
  "/graph": GraphView,
  // S-188 — the Files & Risk and Coverage read-only display tabs.
  "/files": FilesView,
  "/coverage": CoverageView,
  // S-189 — the Architecture/Cycles and Rule-findings tabs. The Wiki tab is the
  // `wiki` extension's view since S-644 (`views/wiki/extension.tsx`); it still owns
  // its client sub-routes (`/wiki/search`, `/wiki/page/*`) through the
  // longest-prefix match in `viewForPath`.
  "/architecture": ArchitectureView,
  "/gaps": GapsView,
  // S-235 — the read-only Statistics view over GET /api/v1/statistics (CR-058),
  // sitting directly above Config in the last sidebar group.
  "/statistics": StatisticsView,
  // S-191 — the Config policy editor (the last interactive tab, the SPA's only
  // mutating surface) over the unchanged intent-guarded config POSTs (ADR-31).
  "/config": ConfigView,
  // S-250 — the app-level Workspace tab (service map / cross-service coverage; its
  // cross-service impact panel is hidden through the hidden-widget register, S-612)
  // over the S-249 `/api/v1/workspace/*` fan-out. Registered
  // unconditionally so a hand-typed `/workspace` resolves; its NAV ITEM, though, is
  // rendered only in workspace mode (nav.ts), and in a single-root serve the view
  // states honestly that this is not a workspace (FR-UI-29).
  "/workspace": WorkspaceView,
  // S-428 — the two app-level workspace views (FR-UI-36). Registered
  // unconditionally for the same reason `/workspace` is: a hand-typed URL must
  // resolve to a view that states honestly that this is not a workspace, rather
  // than to a blank shell. Their NAV items are workspace-mode-only (nav.ts).
  "/workspace-dashboard": WorkspaceDashboardView,
  "/workspace-health": WorkspaceHealthView,
  // S-429 — the app-level Statistics view (FR-UI-37) over the engine-free
  // `workspace/statistics` aggregate. Registered unconditionally for the same
  // reason as the two above; its nav item is workspace-mode-only (nav.ts). It
  // lives beside its member-scoped twin in `views/statistics/` because it shares
  // that view's pure model layer, not the `views/workspace/` layout grammar.
  "/workspace-statistics": WorkspaceStatisticsView,
  // S-430 — the app-level Config editor over `logos.workspace.toml` (FR-UI-38).
  // Registered unconditionally like the three above, so a hand-typed URL in a
  // single-root serve states that this is not a workspace; its nav item is
  // workspace-mode-only (nav.ts), and its endpoints answer `404` there.
  "/workspace-config": WorkspaceConfigView,
  // `/chat` and `/workspace-chat` are the `chat` extension's views (S-643), mounted
  // from its registration (`views/chat/extension.tsx`) when the server reports it
  // active, through `mountedViews()` below.
};

/**
 * The React view for a client pathname, or `null` when the path owns no view.
 *
 * An exact route wins; otherwise the **longest registered route that owns this
 * sub-path** does (`/wiki/page/x` → `/wiki`), so a tab can host its own client
 * sub-routes (the Wiki reader's index/search/page) inside one mounted view.
 *
 * The views of the mounted extensions (S-642, FR-UI-51) are registered beside the
 * built-in ones and resolve under the same rules; a built-in route always wins a
 * clash. A view of an extension the server does not report active is not in the
 * registry, so its route resolves to `null` here — see `inactiveExtensionLanding`
 * (`extensions/landing.ts`).
 */
export function viewForPath(pathname: string): ViewComponent | null {
  // Built-in routes first: both lookups below keep the FIRST of equal routes, so a
  // mounted extension never shadows a built-in one.
  const routes: readonly (readonly [string, ViewComponent])[] = [
    ...Object.entries(VIEW_REGISTRY),
    ...mountedViews(),
  ];
  const exact = routes.find(([route]) => route === pathname);
  if (exact) return exact[1];
  let best: ViewComponent | null = null;
  let bestLen = -1;
  for (const [route, view] of routes) {
    if (pathname.startsWith(`${route}/`) && route.length > bestLen) {
      best = view;
      bestLen = route.length;
    }
  }
  return best;
}
