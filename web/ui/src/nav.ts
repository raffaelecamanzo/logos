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

/**
 * The workspace-only tabs (S-250, CR-061, FR-UI-29). Appended to the sidebar **only
 * in workspace mode** — a single-root serve has no cross-service axis, so offering
 * a service map there would be a fabricated surface, and the sidebar must stay
 * byte-for-byte what it has always been.
 *
 * One tab, three panels (service map / cross-service coverage / cross-service
 * impact): they share one member roster and one binding set, so splitting them
 * across three sidebar items would mean three probes of the same read-models.
 */
export const WORKSPACE_NAV_ITEMS: readonly NavItem[] = [
  { id: "workspace", label: "Workspace", path: "/workspace", group: "A" },
];

/** The sidebar groups in render order. */
export const NAV_GROUPS: readonly NavGroup[] = ["A", "B", "C"];

/**
 * The navigable views for the current serve: the unchanged {@link NAV_ITEMS} in
 * single-root mode, plus {@link WORKSPACE_NAV_ITEMS} in workspace mode.
 */
export function navItemsFor(isWorkspace: boolean): readonly NavItem[] {
  return isWorkspace ? [...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS] : NAV_ITEMS;
}

/**
 * Is this route **app-level** — a view of the whole workspace rather than of one
 * member? Such a view reads the unscoped `workspace/*` fan-out, so its data does not
 * change when the member does, and the shell must NOT remount it on a member switch
 * (`App.tsx`). Today that is exactly the Workspace tab.
 */
export function isAppLevelPath(pathname: string): boolean {
  return WORKSPACE_NAV_ITEMS.some(
    (item) => pathname === item.path || pathname.startsWith(`${item.path}/`),
  );
}
