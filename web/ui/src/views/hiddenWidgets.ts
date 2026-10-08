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
  | "architecture-cycles";

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
    hiddenFrom: "Architecture (the Dependency matrix stays, its cycle cells still outlined ↺)",
    reason:
      "No insight right now (owner, CR-203 item 22): the cycle links seed the Graph view generically, not at the cycle.",
    stillServedBy: ["GET /api/v1/architecture", "logos dsm", "MCP dsm"],
  },
];

/** Whether the register hides `id` from the web UI. */
export function isWidgetHidden(id: HideableWidget): boolean {
  return HIDDEN_WIDGETS.some((w) => w.id === id);
}
