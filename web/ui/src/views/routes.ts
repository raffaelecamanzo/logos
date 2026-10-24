/*
 * The routes the SPA ships a view for (S-788) — the component-free half of the view
 * registry. `nav.ts` asks {@link hasViewFor} before offering a sidebar entry, so an
 * entry whose route no view owns is never rendered; `views/index.ts` maps the same
 * routes to their components and cannot compile without exactly this list
 * (`satisfies Record<BuiltInViewRoute, …>`), so the two cannot drift.
 *
 * A module of its own, rather than `VIEW_REGISTRY` read from `nav.ts`, because the
 * components import CSS modules: the Playwright specs import `nav.ts` into plain
 * Node, which cannot parse a stylesheet, and `nav.ts` must stay loadable there.
 */

import { mountedViews } from "../extensions/registry.ts";
import { routeOwns } from "../routeOwns.ts";

/** Every route a built-in view is registered for. A route is added here and in
 *  `VIEW_REGISTRY` together — either alone fails `tsc -b`. */
export const BUILT_IN_VIEW_ROUTES = [
  "/",
  "/health",
  "/graph",
  "/files",
  "/coverage",
  "/architecture",
  "/gaps",
  "/security",
  "/statistics",
  "/config",
  "/workspace",
  "/workspace-dashboard",
  "/workspace-health",
  "/workspace-statistics",
  "/workspace-config",
  "/workspace-security",
  "/workspace-coverage",
] as const;

export type BuiltInViewRoute = (typeof BUILT_IN_VIEW_ROUTES)[number];

/**
 * Does a registered view own `pathname` — a built-in route, or a route of a mounted
 * extension (S-642) — exactly or as one of its sub-routes? The same answer
 * `viewForPath` gives (`views/index.ts`), without the component.
 */
export function hasViewFor(pathname: string): boolean {
  return (
    BUILT_IN_VIEW_ROUTES.some((route) => routeOwns(route, pathname)) ||
    mountedViews().some(([route]) => routeOwns(route, pathname))
  );
}
