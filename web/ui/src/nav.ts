/*
 * The navigable view registry (S-185, FR-UI-22). Defines the SPA sidebar's
 * ordering, labels, navigation groups (CR-042), and — since S-425 — the SCOPE
 * each view answers for (FR-UI-35, ADR-66).
 *
 * Every tab is a React view in the SPA (server-rendered stack decommissioned in
 * S-192; migration completed in Sprint 33). The Dashboard lives at `/` (S-194).
 */

import type { ExtensionNavEntry } from "./api/extensionsClient.ts";
import { mountedNavAnchor, mountedNavEntries } from "./extensions/registry.ts";
import { routeOwns } from "./routeOwns.ts";
import { isViewHidden } from "./views/hiddenWidgets.ts";

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
   * built-in view — means offered in both modes; an extension's entry declares it in
   * its manifest (`replaced_in_workspace_by`).
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
  // An extension entry its registration anchors to Graph (`ExtensionUi.navAfter`)
  // sits here, in the order the server lists the extensions.
  // S-612 (FR-UI-41): "Architecture", not "Architecture / Cycles" — the Cycles
  // band and list are hidden through the hidden-widget register, and a label
  // naming them would point at nothing on the page. The route is unchanged.
  // CR-208 hides the whole view through the same register (`architecture-view`):
  // the entry stays registered here, and `navItemsFor` leaves it out while the
  // register names its route, so removing that register entry brings it back.
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
 * other here. (An extension's entries, which come from a manifest that carries no
 * workspace-only flag, are the one exception: `appNavItems` / `memberNavItems` offer
 * an `app`-scoped extension entry in workspace mode only and a `member`-scoped one in
 * both — the declared scope drives both axes for them.)
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
  // it would prefix-match in {@link navItemMatches} — the rule that keeps a tab
  // lit on the client sub-routes it owns — and light up BOTH the Workspace tab
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
 * The navigation entries the mounted extensions add (S-642, FR-UI-51), by the role
 * they play beside the built-in lists: a `member`-scoped entry joins
 * {@link NAV_ITEMS} (offered in both modes, replaced in workspace mode when it says
 * so), an `app`-scoped one joins {@link WORKSPACE_NAV_ITEMS} (workspace mode only).
 *
 * Read at call time from the registry, which holds an entry only when the server
 * reports its extension active (`extensions/registry.ts`). Empty in a build that
 * composes none, which is what keeps every list below equal to the built-in one.
 *
 * Two repairs keep a malformed manifest from breaking the shell: an entry whose id
 * or route a built-in (or an earlier extension) already owns is dropped — the
 * built-in wins, as it does for a route or a tool on the server — and a
 * `replacedInWorkspaceBy` naming no offered app entry is cleared, so the sidebar
 * never drops a view that nothing replaces (see {@link workspaceReplacementPath}).
 */
/** A wire navigation entry as a `NavItem`, or `null` when its group or scope is not
 *  one the shell knows (the server types both closed, so this is a stale-bundle
 *  guard: a sidebar section keyed on an unknown scope would render nothing). */
function navItemOf(entry: ExtensionNavEntry): NavItem | null {
  const group = NAV_GROUPS.find((g) => g === entry.group);
  const scope = NAV_SCOPES.find((s) => s === entry.scope);
  if (group === undefined || scope === undefined) return null;
  return {
    id: entry.id,
    label: entry.label,
    path: entry.path,
    group,
    scope,
    ...(entry.replaced_in_workspace_by !== undefined && {
      replacedInWorkspaceBy: entry.replaced_in_workspace_by,
    }),
  };
}

function extensionNavItems(): { member: readonly NavItem[]; app: readonly NavItem[] } {
  const taken = new Set(
    [...NAV_ITEMS, ...WORKSPACE_NAV_ITEMS].flatMap((i) => [`id:${i.id}`, `path:${i.path}`]),
  );
  const accepted = mountedNavEntries().flatMap((entry) => navItemOf(entry) ?? []).filter((item) => {
    const keys = [`id:${item.id}`, `path:${item.path}`];
    if (keys.some((k) => taken.has(k))) return false;
    keys.forEach((k) => taken.add(k));
    return true;
  });
  const appIds = new Set(
    [...WORKSPACE_NAV_ITEMS, ...accepted.filter((i) => i.scope === "app")].map((i) => i.id),
  );
  const checked = accepted.map((item) => {
    if (item.replacedInWorkspaceBy === undefined || appIds.has(item.replacedInWorkspaceBy)) {
      return item;
    }
    const { replacedInWorkspaceBy: _unresolved, ...rest } = item;
    return rest;
  });
  return {
    member: checked.filter((i) => i.scope === "member"),
    app: checked.filter((i) => i.scope === "app"),
  };
}

/**
 * `extras` placed into `base`: each directly after the entry its extension anchors it
 * to (`ExtensionUi.navAfter`, read through {@link mountedNavAnchor}) — after the
 * anchor and any entry already placed after it, so two entries anchored to one id keep
 * their registration order — or appended when it names no anchor, or one this list
 * does not offer. The anchor is matched within the one list, so an entry can never
 * jump from its sidebar section into another.
 */
function placed(base: readonly NavItem[], extras: readonly NavItem[]): readonly NavItem[] {
  if (extras.length === 0) return base;
  const out = [...base];
  // anchor id → the id of the last entry placed after it.
  const runs = new Map<string, string>();
  for (const item of extras) {
    const anchor = mountedNavAnchor(item.id);
    const at = anchor === undefined ? -1 : out.findIndex((i) => i.id === (runs.get(anchor) ?? anchor));
    if (anchor === undefined || at === -1) {
      out.push(item);
      continue;
    }
    out.splice(at + 1, 0, item);
    runs.set(anchor, item.id);
  }
  return out;
}

/** The member-scoped list in force: {@link NAV_ITEMS} plus the mounted extensions',
 *  each where its extension places it. */
function memberNavItems(): readonly NavItem[] {
  return placed(NAV_ITEMS, extensionNavItems().member);
}

/** The workspace-only list in force: {@link WORKSPACE_NAV_ITEMS} plus the mounted
 *  extensions' app-scoped entries, each where its extension places it. */
function appNavItems(): readonly NavItem[] {
  return placed(WORKSPACE_NAV_ITEMS, extensionNavItems().app);
}

/**
 * The navigable views for the current serve: {@link NAV_ITEMS} in single-root
 * mode; in workspace mode, every one of them a workspace view does not replace
 * ({@link NavItem.replacedInWorkspaceBy}), plus {@link WORKSPACE_NAV_ITEMS}. In
 * both, a view the hidden-widget register hides (CR-208, FR-UI-41) is left out —
 * read off the register at call time, so removing its entry offers it again. The
 * entries of the mounted extensions are part of both lists (S-642).
 */
export function navItemsFor(isWorkspace: boolean): readonly NavItem[] {
  const members = memberNavItems();
  const offered = isWorkspace
    ? [...members.filter((item) => item.replacedInWorkspaceBy === undefined), ...appNavItems()]
    : members;
  return offered.filter((item) => !isViewHidden(item.path));
}

/**
 * Workspace mode only: the route `pathname` redirects to because the view it
 * resolves to is replaced there (S-485 — an entry's route and its sub-routes land
 * on the app entry it names in {@link NavItem.replacedInWorkspaceBy}), or `null`
 * when it is offered as-is. The caller carries the query
 * and fragment across; this answers for the path alone.
 *
 * Read off the same declaration {@link navItemsFor} filters on, so the sidebar
 * cannot drop a view whose route still mounts it, nor redirect away from one it
 * still lists.
 */
export function workspaceReplacementPath(pathname: string): string | null {
  const replaced = memberNavItems().find(
    (item) => item.replacedInWorkspaceBy !== undefined && navItemMatches(item, pathname),
  );
  if (!replaced) return null;
  const replacement = appNavItems().find((item) => item.id === replaced.replacedInWorkspaceBy);
  if (!replacement) {
    throw new Error(
      `nav.ts: "${replaced.id}" is replaced in workspace mode by "${replaced.replacedInWorkspaceBy}", which is not registered`,
    );
  }
  return replacement.path;
}

/**
 * Does `pathname` resolve to `item` — exactly, or as one of the client sub-routes
 * it owns (`/<path>/…`)?
 *
 * What keeps the root Dashboard (`path: "/"`) from prefix-matching `/health`, and
 * makes a match a whole path segment, is spelled once in `routeOwns`.
 *
 * This is the ONE spelling of "does this route belong to this tab" in the SPA — the
 * sidebar's active-item highlight and {@link scopeForPath} both call it, rather than
 * each carrying its own copy of the rule — and it is `routeOwns` (the rule over a bare
 * route), which the hidden-widget register and the extension registry call too.
 */
export function navItemMatches(item: NavItem, pathname: string): boolean {
  return routeOwns(item.path, pathname);
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
  for (const item of [...memberNavItems(), ...appNavItems()]) {
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
