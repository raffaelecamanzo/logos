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
  /**
   * The id of the {@link WORKSPACE_NAV_ITEMS} entry that answers this view's
   * question in workspace mode, where this view is therefore NOT offered (S-485,
   * FR-UI-35, ADR-71): the sidebar drops it, and its route redirects to the
   * replacement's ({@link workspaceReplacementPath}). Absent — the case for every
   * view but the member Chat — means offered in both modes.
   *
   * Declared on the replaced entry, never derived from a shared label: two views
   * may share a name across scopes and BOTH be offered (Dashboard, Health…).
   */
  replacedInWorkspaceBy?: string;
}

/** Every navigable view, in sidebar order. */
export const NAV_ITEMS: readonly NavItem[] = [
  // Group A — primary read surfaces.
  // S-194: Dashboard is now at `/` (was `/overview` prior to Sprint 34).
  { id: "overview", label: "Dashboard", path: "/", group: "A", scope: "member" },
  { id: "health", label: "Health", path: "/health", group: "A", scope: "member" },
  { id: "graph", label: "Graph", path: "/graph", group: "A", scope: "member" },
  // Chat — migrated (S-190) to a React SSE client over the unchanged
  // intent-guarded `POST /chat` stream. The MEMBER chat: offered in single-root and
  // `--standalone` serves only. In a workspace the chat is the Workspace Chat
  // (S-485, ADR-71), and a member's conversations live in its `--standalone` serve.
  {
    id: "chat",
    label: "Chat",
    path: "/chat",
    group: "A",
    scope: "member",
    replacedInWorkspaceBy: "workspace-chat",
  },
  { id: "wiki", label: "Wiki", path: "/wiki", group: "A", scope: "member" },
  // S-612 (FR-UI-41): "Architecture", not "Architecture / Cycles" — the Cycles
  // band and list are hidden through the hidden-widget register, and a label
  // naming them would point at nothing on the page. The route is unchanged.
  {
    id: "architecture",
    label: "Architecture",
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
 * The workspace-only tabs (S-250, CR-061, FR-UI-29; extended by S-428, FR-UI-36).
 * Appended to the sidebar **only in workspace mode** — a single-root serve has no
 * cross-service axis, so offering a service map there would be a fabricated
 * surface, and the sidebar must stay byte-for-byte what it has always been.
 *
 * The **Workspace** tab is one tab over its panels (service map /
 * cross-service coverage; cross-service impact is hidden through the
 * hidden-widget register, S-612): they share one member roster and one binding
 * set, so splitting them across sidebar items would mean one probe of the same
 * read-models per item. The two S-428 views beside it are
 * separate tabs for the opposite reason — they answer two different questions
 * ("how coupled is this?", "is this current?") over two different read-model
 * pairs, and neither re-reads the other's.
 *
 * Workspace-mode-only and app-scoped are two different axes that happen to agree
 * for every entry here today: this list decides whether the tab is OFFERED,
 * {@link NavItem.scope} decides what it ANSWERS FOR. Nothing derives one from the
 * other.
 */
export const WORKSPACE_NAV_ITEMS: readonly NavItem[] = [
  // S-428 (CR-137, FR-UI-36): the two app-level views over read-models that
  // already existed and had never been reachable from the interface — the
  // cross-service picture, and the operational roll-up.
  //
  // Named "Dashboard" and "Health", exactly like their member-scoped twins.
  // ADR-66 accepted that deliberately: `/` stays the per-member Dashboard so no
  // bookmark changes meaning, and the two levels are told apart by the SECTION
  // label the sidebar renders, not by a second name.
  //
  // Their routes are siblings of `/workspace`, not children of it. A path under
  // it would prefix-match in {@link navItemMatches} — the rule that keeps the
  // Wiki tab lit while its reader is open — and light up BOTH the Workspace tab
  // and the new one at once.
  {
    id: "workspace-dashboard",
    label: "Dashboard",
    path: "/workspace-dashboard",
    group: "A",
    scope: "app",
  },
  { id: "workspace-health", label: "Health", path: "/workspace-health", group: "A", scope: "app" },
  { id: "workspace", label: "Workspace", path: "/workspace", group: "A", scope: "app" },
  // S-485 (CR-155, FR-WS-34, ADR-71): the Workspace Chat — the workspace roster on
  // its own route and store (S-482). App-scoped: it answers for the whole
  // workspace and reads no `?repo=`, so a member switch must not remount it. It
  // REPLACES the member Chat in workspace mode rather than sitting beside it
  // (`replacedInWorkspaceBy` on that entry). Group A after the cross-service
  // surfaces, as frontend-design §3 draws it; a sibling of `/workspace`, not a
  // child, for the reason the S-428 entries above give.
  { id: "workspace-chat", label: "Chat", path: "/workspace-chat", group: "A", scope: "app" },
  // S-429 (CR-137, FR-UI-37): usage summed across every member, over an aggregate
  // that constructs no member engine (NFR-PE-10).
  //
  // Group C, not A, so it sits apart from the three cross-service surfaces exactly
  // as its member-scoped twin sits apart from the read surfaces above it — the
  // CR-042 grouping is a property of WHAT a tab answers, and this one answers the
  // same question one scope up. Group B falls out empty in this section and renders
  // nothing, which {@link NavItem} already relies on.
  //
  // Appended LAST, and this position is load-bearing rather than incidental:
  // `Sidebar.test.tsx` asserts the Workspace section renders in this list's order,
  // and within the section a group-C entry renders after every group-A one.
  {
    id: "workspace-statistics",
    label: "Statistics",
    path: "/workspace-statistics",
    group: "C",
    scope: "app",
  },
  // S-430 (CR-137, FR-UI-38): the app-level Config editor over the workspace's own
  // files — `logos.workspace.toml` today. Named "Config" like its member-scoped
  // twin (ADR-66 tells the two apart by section label), group C beside it, and
  // after Statistics exactly as the member-scoped Config sits after its
  // Statistics. A sibling route of `/workspace`, not a child, for the reason the
  // S-428 entries above give.
  {
    id: "workspace-config",
    label: "Config",
    path: "/workspace-config",
    group: "C",
    scope: "app",
  },
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
 * single-root mode; in workspace mode, every one of them a workspace view does not
 * replace ({@link NavItem.replacedInWorkspaceBy}), plus {@link WORKSPACE_NAV_ITEMS}.
 */
export function navItemsFor(isWorkspace: boolean): readonly NavItem[] {
  return isWorkspace
    ? [...NAV_ITEMS.filter((item) => item.replacedInWorkspaceBy === undefined), ...WORKSPACE_NAV_ITEMS]
    : NAV_ITEMS;
}

/**
 * Workspace mode only: the route `pathname` redirects to because the view it
 * resolves to is replaced there (S-485 — `/chat` and its sub-routes land on the
 * Workspace Chat), or `null` when it is offered as-is. The caller carries the query
 * and fragment across; this answers for the path alone.
 *
 * Read off the same declaration {@link navItemsFor} filters on, so the sidebar
 * cannot drop a view whose route still mounts it, nor redirect away from one it
 * still lists.
 */
export function workspaceReplacementPath(pathname: string): string | null {
  const replaced = NAV_ITEMS.find(
    (item) => item.replacedInWorkspaceBy !== undefined && navItemMatches(item, pathname),
  );
  if (!replaced) return null;
  const replacement = WORKSPACE_NAV_ITEMS.find((item) => item.id === replaced.replacedInWorkspaceBy);
  if (!replacement) {
    throw new Error(
      `nav.ts: "${replaced.id}" is replaced in workspace mode by "${replaced.replacedInWorkspaceBy}", which is not registered`,
    );
  }
  return replacement.path;
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
