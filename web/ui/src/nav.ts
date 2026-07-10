/*
 * The navigable view registry (S-185, FR-UI-22). Defines the SPA sidebar's
 * ordering, labels, and navigation groups (CR-042).
 *
 * Every tab is a React view in the SPA (server-rendered stack decommissioned in
 * S-192; migration completed in Sprint 33). The Dashboard lives at `/` (S-194).
 */

/** The three sidebar groups (CR-042): primary read surfaces, risk & coverage,
 *  and the isolated policy editor. */
export type NavGroup = "A" | "B" | "C";

export interface NavItem {
  /** Stable id (matches the server `View` variant, lowercased). */
  id: string;
  /** Sidebar / breadcrumb label. */
  label: string;
  /** The in-SPA client route this tab resolves to. */
  path: string;
  group: NavGroup;
}

/** Every navigable view, in sidebar order. */
export const NAV_ITEMS: readonly NavItem[] = [
  // Group A — primary read surfaces.
  // S-194: Dashboard is now at `/` (was `/overview` prior to Sprint 34).
  { id: "overview", label: "Dashboard", path: "/", group: "A" },
  { id: "health", label: "Health", path: "/health", group: "A" },
  { id: "graph", label: "Graph", path: "/graph", group: "A" },
  // Chat — migrated (S-190) to a React SSE client over the unchanged
  // intent-guarded `POST /chat` stream.
  { id: "chat", label: "Chat", path: "/chat", group: "A" },
  { id: "wiki", label: "Wiki", path: "/wiki", group: "A" },
  { id: "architecture", label: "Architecture / Cycles", path: "/architecture", group: "A" },
  // Group B — risk & coverage surfaces.
  { id: "files", label: "Files & Risk", path: "/files", group: "B" },
  // CR-079: the "Gaps" tab is now "Rule findings" (test-gaps roll-up removed).
  { id: "gaps", label: "Rule findings", path: "/gaps", group: "B" },
  { id: "coverage", label: "Coverage", path: "/coverage", group: "B" },
  // Group C — the isolated policy editor (the only mutating surface), with the
  // read-only Statistics view directly above it (S-235, CR-058, FR-UI-27).
  { id: "statistics", label: "Statistics", path: "/statistics", group: "C" },
  { id: "config", label: "Config", path: "/config", group: "C" },
];

/** The sidebar groups in render order. */
export const NAV_GROUPS: readonly NavGroup[] = ["A", "B", "C"];
