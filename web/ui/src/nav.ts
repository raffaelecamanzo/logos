/*
 * The navigable view registry (S-185, FR-UI-22). Defines the SPA sidebar's
 * ordering, labels, navigation groups (CR-042), and — since S-425 — the SCOPE
 * each view answers for (FR-UI-35, ADR-66).
 *
 * Every tab is a React view in the SPA (server-rendered stack decommissioned in
 * S-192; migration completed in Sprint 33). The Dashboard lives at `/` (S-194).
 */

/** The three sidebar groups (CR-042): primary read surfaces, risk & coverage,
 *  and the isolated policy editor. */
export type NavGroup = "A" | "B" | "C";

/**
 * The scope a view answers for (S-425, FR-UI-35, ADR-66).
 *
 * - `member` — the view answers for ONE workspace member, through the optional
 *   `?repo=` the request-scoped engine extractor resolves. Its data changes when
 *   the member does, so the shell re-keys it on a member switch.
 * - `app` — the view answers for the WHOLE workspace over the unscoped
 *   `/api/v1/workspace/*` fan-out. Its data does not change when the member does,
 *   so the shell must NOT remount it on a switch.
 *
 * It is DECLARED, never derived: not from the route spelling, not from which
 * endpoints the view happens to call, and not from a second list kept beside this
 * one. {@link NavItem.scope} is required at the type level, so a view registered
 * without one fails `tsc -b`; `nav.test.ts` fails on it too, for a registration
 * that reaches runtime with the field stripped (ADR-66 §2 — the wrong default is
 * silent and there is no safe fallback, so absence must be loud).
 */
export type NavScope = "app" | "member";

export interface NavItem {
  /** Stable id (matches the server `View` variant, lowercased). */
  id: string;
  /** Sidebar / breadcrumb label. */
  label: string;
  /** The in-SPA client route this tab resolves to. */
  path: string;
  group: NavGroup;
  /** REQUIRED — the scope this view answers for. See {@link NavScope}. */
  scope: NavScope;
}

/** Every navigable view, in sidebar order. */
export const NAV_ITEMS: readonly NavItem[] = [
  // Group A — primary read surfaces.
  // S-194: Dashboard is now at `/` (was `/overview` prior to Sprint 34).
  { id: "overview", label: "Dashboard", path: "/", group: "A", scope: "member" },
  { id: "health", label: "Health", path: "/health", group: "A", scope: "member" },
  { id: "graph", label: "Graph", path: "/graph", group: "A", scope: "member" },
  // Chat — migrated (S-190) to a React SSE client over the unchanged
  // intent-guarded `POST /chat` stream.
  { id: "chat", label: "Chat", path: "/chat", group: "A", scope: "member" },
  { id: "wiki", label: "Wiki", path: "/wiki", group: "A", scope: "member" },
  {
    id: "architecture",
    label: "Architecture / Cycles",
    path: "/architecture",
    group: "A",
    scope: "member",
  },
  // Group B — risk & coverage surfaces.
  { id: "files", label: "Files & Risk", path: "/files", group: "B", scope: "member" },
  // CR-079: the "Gaps" tab is now "Rule findings" (test-gaps roll-up removed).
  { id: "gaps", label: "Rule findings", path: "/gaps", group: "B", scope: "member" },
  { id: "coverage", label: "Coverage", path: "/coverage", group: "B", scope: "member" },
  // Group C — the isolated policy editor (the only mutating surface), with the
  // read-only Statistics view directly above it (S-235, CR-058, FR-UI-27).
  { id: "statistics", label: "Statistics", path: "/statistics", group: "C", scope: "member" },
  { id: "config", label: "Config", path: "/config", group: "C", scope: "member" },
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
 *
 * Workspace-mode-only and app-scoped are two different axes that happen to agree
 * for every entry here today: this list decides whether the tab is OFFERED,
 * {@link NavItem.scope} decides what it ANSWERS FOR. Nothing derives one from the
 * other.
 */
export const WORKSPACE_NAV_ITEMS: readonly NavItem[] = [
  { id: "workspace", label: "Workspace", path: "/workspace", group: "A", scope: "app" },
];

/** The sidebar groups in render order. */
export const NAV_GROUPS: readonly NavGroup[] = ["A", "B", "C"];

/** The sidebar sections in render order: the whole workspace first, then the one
 *  member (S-425, FR-UI-35). */
export const NAV_SCOPES: readonly NavScope[] = ["app", "member"];

/** The words the sidebar puts on each scope. The label is the ONLY thing that
 *  distinguishes two views registered under the same name in different scopes
 *  (ADR-66), so it is rendered as text and never as colour or position alone
 *  (NFR-CC-04). */
export const NAV_SCOPE_LABELS: Readonly<Record<NavScope, string>> = {
  app: "Workspace",
  member: "Service",
};

/**
 * The navigable views for the current serve: the unchanged {@link NAV_ITEMS} in
 * single-root mode, plus {@link WORKSPACE_NAV_ITEMS} in workspace mode.
 */
export function navItemsFor(isWorkspace: boolean): readonly NavItem[] {
  return isWorkspace ? [...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS] : NAV_ITEMS;
}

/**
 * Does `pathname` resolve to `item` — exactly, or as one of the client sub-routes
 * it owns (the Wiki reader's `/wiki/page/*`)?
 *
 * What keeps the root Dashboard (`path: "/"`) from prefix-matching `/health` is the
 * TRAILING SLASH: the arm asks for `${path}/`, so a match is a whole path segment
 * and the root's test is `startsWith("//")`, which no route satisfies. The explicit
 * `item.path !== "/"` beside it is redundant against that spelling and is kept as a
 * statement of the intent — drop the trailing slash and it is the only thing left
 * standing between `/` and every route in the SPA.
 *
 * This is the ONE spelling of "does this route belong to this tab" in the SPA — the
 * sidebar's active-item highlight and {@link scopeForPath} both call it, rather than
 * each carrying its own copy of the rule.
 */
export function navItemMatches(item: NavItem, pathname: string): boolean {
  return pathname === item.path || (item.path !== "/" && pathname.startsWith(`${item.path}/`));
}

/**
 * The declared scope of the view `pathname` resolves to.
 *
 * Longest registered route wins, mirroring `viewForPath` in `views/index.ts` — the
 * registry that decides which component this same path mounts. A path that
 * resolves to no registered view answers `member`, which is what an unrecognised
 * route has always been keyed as; it is a fallback for an UNREGISTERED path, never
 * a default for a registered entry that omitted the field (ADR-66 §2).
 */
export function scopeForPath(pathname: string): NavScope {
  let best: NavItem | null = null;
  for (const item of [...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS]) {
    if (navItemMatches(item, pathname) && item.path.length > (best?.path.length ?? -1)) {
      best = item;
    }
  }
  return best?.scope ?? "member";
}

/**
 * Is this route **app-level** — a view of the whole workspace rather than of one
 * member? Such a view reads the unscoped `workspace/*` fan-out, so its data does not
 * change when the member does, and the shell must NOT remount it on a member switch
 * (`App.tsx`).
 *
 * A lookup over the declared {@link NavItem.scope} field and nothing else (S-425,
 * ADR-66 §3). It was a path-prefix test over a hard-coded list until S-425; there
 * is no such list anywhere in the tree now, and `nav.test.ts` asserts that by
 * reading the source.
 */
export function isAppLevelPath(pathname: string): boolean {
  return scopeForPath(pathname) === "app";
}
