/*
 * The hidden-widget register (S-612, CR-203, FR-UI-41) — the ONE place a web
 * widget is hidden from the interface.
 *
 * Each entry names the widget, the view it is hidden from, why, and the surfaces
 * that still serve its data. Hiding is a web-UI decision only: the HTTP
 * endpoints, CLI commands and MCP tools behind a hidden widget answer exactly as
 * before, and the component itself stays in the source tree. A view asks
 * {@link isWidgetHidden} before rendering the widget, so removing an entry here
 * is the whole of bringing it back — no view edit, no rework.
 *
 * A whole VIEW can be hidden the same way (CR-208): an entry that carries a
 * `view` drops that view's sidebar entry (`navItemsFor`) and sends its route to
 * the view's `landsOn` (`App.tsx`), so removing that one entry brings back both
 * the route and the sidebar entry — again with no edit anywhere else.
 *
 * `docs/howto/usage.md` lists the same entries for the reader, with the same
 * "still served by" surfaces.
 *
 * The array is typed `readonly` but deliberately not frozen: each view's tests
 * remove an entry (`src/test/hiddenWidgets.ts`) and assert the widget renders
 * again through the real lookup, so the return path is exercised rather than
 * assumed.
 */

/** Every widget the register knows how to hide. Declared apart from the
 *  register's entries so removing an entry leaves every call site compiling —
 *  the widget simply renders again. */
export type HideableWidget =
  | "coverage-by-relation-arm"
  | "cross-service-impact"
  | "non-gated-tier"
  | "architecture-cycles"
  | "architecture-view"
  | "declared-contracts";

/** A whole view the register hides (CR-208): its route, and where that route
 *  lands while the view is hidden. */
export interface HiddenView {
  /** The view's route, as `nav.ts` registers it. */
  path: string;
  /** Where a visit to `path` (or a sub-path of it) is redirected instead. */
  landsOn: string;
}

export interface HiddenWidget {
  id: HideableWidget;
  /** The widget as a reader knows it. */
  widget: string;
  /** Where it used to render. */
  hiddenFrom: string;
  /** Why it is hidden. */
  reason: string;
  /** The surfaces that still serve its data, unchanged. */
  stillServedBy: readonly string[];
  /** Set when the entry hides a whole view: its sidebar entry and its route. */
  view?: HiddenView;
}

export const HIDDEN_WIDGETS: readonly HiddenWidget[] = [
  {
    id: "coverage-by-relation-arm",
    widget: "Coverage by relation arm",
    hiddenFrom: "Workspace Dashboard; the Workspace tab's Cross-service coverage panel",
    reason: "Not useful (owner, CR-203 item 1): a table only, with no explanatory text.",
    stillServedBy: [
      "GET /api/v1/workspace/status (coverage)",
      "logos workspace status",
      "MCP workspace_status",
    ],
  },
  {
    id: "cross-service-impact",
    widget: "Cross-service impact",
    hiddenFrom: "The Workspace tab (Service map and Cross-service coverage remain)",
    reason: "Does more harm than good (owner, CR-203 item 11).",
    stillServedBy: [
      "GET /api/v1/workspace/impact",
      "logos xservice impact",
      "MCP xservice_impact",
    ],
  },
  {
    id: "non-gated-tier",
    widget: "Non-gated tier callout",
    hiddenFrom: "Health",
    reason:
      "No actionable insight (CR-203 item 21): a static pointer to a view the sidebar already lists, with no figure.",
    stillServedBy: ["Files & Risk in the sidebar", "logos hotspots"],
  },
  {
    id: "architecture-cycles",
    widget: "Cycles band and cycle list",
    // The whole view is hidden too (`architecture-view`, CR-208); this entry
    // stays so that bringing the view back does not bring the band back with it.
    hiddenFrom: "Architecture (the Dependency matrix keeps its cycle cells outlined ↺)",
    reason:
      "No insight right now (owner, CR-203 item 22): the cycle links seed the Graph view generically, not at the cycle.",
    stillServedBy: ["GET /api/v1/architecture", "logos dsm", "MCP dsm"],
  },
  {
    id: "architecture-view",
    widget: "Dependency matrix (the Architecture view)",
    hiddenFrom: "The sidebar; /architecture and the retired /dsm land on Health",
    reason:
      "Misleads more than it informs (owner, CR-208 item 1): with no layers declared its order is alphabetical, so its \"against order\" marks are arbitrary, and it is unreadable at a thousand rows.",
    stillServedBy: ["GET /api/v1/architecture", "logos dsm", "MCP dsm"],
    view: { path: "/architecture", landsOn: "/health" },
  },
  {
    id: "declared-contracts",
    widget: "Declared contracts",
    hiddenFrom: "Workspace → Service map (the map's declared layer and its legend stay)",
    reason:
      "Unusable as built (owner, CR-208 item 2): up to four tables and a join line, with nothing to tell the tables apart.",
    stillServedBy: [
      "GET /api/v1/workspace/status (coverage.declared_contracts, coverage.bound_external)",
      "logos workspace status",
      "logos xservice route-providers (declared_contracts, bound_external)",
      "MCP workspace_status",
      "MCP xservice_route_providers",
    ],
  },
];

/** Whether the register hides `id` from the web UI. */
export function isWidgetHidden(id: HideableWidget): boolean {
  return HIDDEN_WIDGETS.some((w) => w.id === id);
}

/** Does `pathname` resolve to the hidden view `view` — its route, or a sub-path
 *  of it? The same whole-segment rule `navItemMatches` spells for the sidebar. */
function inView(view: HiddenView, pathname: string): boolean {
  return pathname === view.path || pathname.startsWith(`${view.path}/`);
}

/** Whether the register hides the view whose route is `path` (its sidebar entry
 *  is then dropped). */
export function isViewHidden(path: string): boolean {
  return HIDDEN_WIDGETS.some((w) => w.view !== undefined && w.view.path === path);
}

/** Where a visit to `pathname` lands because the register hides the view it
 *  resolves to, or `null` when that view is not hidden. */
export function hiddenViewLanding(pathname: string): string | null {
  const entry = HIDDEN_WIDGETS.find((w) => w.view !== undefined && inView(w.view, pathname));
  return entry?.view?.landsOn ?? null;
}
