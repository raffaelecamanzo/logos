/*
 * The root SPA component (S-185 shell, re-skinned onto the design system in
 * S-193). Composes the shared AppShell layout primitive with the Sidebar + Header,
 * wrapped in the ToastProvider so any view can surface notifications.
 *
 * Every tab is a React view mounted in the AppShell content slot keyed off the
 * client pathname (`usePathname` → `viewForPath`), registered in `views/index.ts`.
 * The Dashboard is at `/` (S-194). The retired `/overview` and `/dsm` routes are
 * silently redirected to `/` and `/architecture` (replaceState — no extra history
 * entry, bookmarks survive).
 *
 * S-250 (CR-061, FR-UI-29) wraps the shell in the WorkspaceProvider and keys the
 * mounted view on the workspace cache key — the member. Switching members remounts
 * the view, so every `useApiResource` in it re-runs against the newly-scoped
 * transport: "member is part of the cache key", enforced once in the shell rather
 * than re-implemented per view. In single-root mode the key is a constant, so
 * nothing ever remounts and the UI behaves exactly as it did before.
 *
 * S-426 (FR-UI-35, NFR-RA-05) adds one more gate beside the probe gate: while the
 * URL names a member this workspace does not have, NO view is mounted at all. The
 * refusal takes the content slot instead.
 *
 * For a MEMBER-scoped view the reason is direct: mounted there it would read
 * unscoped and paint the DEFAULT member's figures under the requested member's name
 * — a `200`-shaped page that is wrong about its own subject, which is what the
 * server refuses with a `404` and what this shell must not undo.
 *
 * The gate is deliberately NOT narrowed to those views, and the reason above is not
 * the reason why. An app-level view reads the `workspace/*` fan-out, which
 * `api/client.ts` never scopes to a member, so it would render correct figures — it
 * cannot commit that substitution. It is refused because the refusal is about the
 * URL, not about what the view happens to read: `?repo=ghost` is a claim this
 * workspace cannot honour, and a page that answered it in full while the address bar
 * named a member that does not exist would invite exactly the reading the refusal
 * exists to prevent. One invalid URL, one page state.
 *
 * This reads no scope and adds no second notion of one (ADR-66): it does not consult
 * the path at all. `viewKey` below remains the only consumer of the declared field.
 */

import { useEffect } from "react";

import { AppShell, LoadingState, ToastProvider } from "./components/index.ts";
import { isAppLevelPath, workspaceReplacementPath } from "./nav.ts";
import { usePathname, redirect } from "./router.tsx";
import { Header } from "./shell/Header.tsx";
import { Sidebar } from "./shell/Sidebar.tsx";
import { viewForPath } from "./views/index.ts";
import { UnknownMember } from "./workspace/UnknownMember.tsx";
import { useWorkspace, WorkspaceProvider } from "./workspace/WorkspaceContext.tsx";

/** Retired routes and where their bookmarks land (CR-038, CR-051). Only the PATH
 *  is retired; the query and fragment ride across (see the effect below). */
const RETIRED_ROUTES: Readonly<Record<string, string>> = {
  "/overview": "/",
  "/dsm": "/architecture",
};

function retiredRouteTarget(pathname: string): string | null {
  return Object.hasOwn(RETIRED_ROUTES, pathname) ? RETIRED_ROUTES[pathname] : null;
}

function Shell() {
  const rawPathname = usePathname();
  const { cacheKey, mode, unknownMember } = useWorkspace();

  // Silently migrate a retired bookmark (/overview → /, /dsm → /architecture)
  // without adding a back-stack entry. Only the PATH is retired, so the query and
  // fragment are carried across verbatim: this effect fires on mount, before the
  // workspace probe has answered, so `redirect` has no member scope to re-apply yet
  // (S-426). A bare `redirect("/")` therefore discarded the whole query — and with
  // it a deep-linked `?repo=`, which then resolved to the manifest default and
  // painted ITS figures for a URL that named another member, or bypassed the
  // unknown-member refusal outright (NFR-RA-05).
  const retiredTo = retiredRouteTarget(rawPathname);
  useEffect(() => {
    if (retiredTo !== null) {
      redirect(`${retiredTo}${window.location.search}${window.location.hash}`);
    }
  }, [retiredTo]);

  // S-485 (FR-UI-35, ADR-71): in a workspace the member Chat is not offered — the
  // chat there is the Workspace Chat — so `/chat` (and `/chat?repo=x`) lands on it.
  // Which routes are replaced, and by what, is READ off `nav.ts`, the same
  // declaration the sidebar drops the entry by. Workspace mode only, and only once
  // the probe has said so: single-root and `--standalone` serve `/chat` as always.
  // The query and fragment ride across verbatim, as for the retired routes above — an
  // unknown `?repo=` must reach the refusal rather than be dropped on the way.
  const replacement = mode === "workspace" ? workspaceReplacementPath(rawPathname) : null;
  useEffect(() => {
    if (replacement !== null) {
      redirect(`${replacement}${window.location.search}${window.location.hash}`);
    }
  }, [replacement]);

  // Canonical path: resolve the redirects synchronously so the destination view
  // renders on the first frame (no blank-content flash before the effect fires).
  const pathname = retiredTo ?? replacement ?? rawPathname;
  const View = viewForPath(pathname);

  // An APP-scoped view reads the unscoped `workspace/*` fan-out, identical for every
  // member. Re-keying it on the member would tear the ECharts canvas down and re-run
  // the whole fan-out every time the user clicks a service in the map (which selects
  // that member) — losing the open tab and the typed impact query to no purpose.
  //
  // Which views those are is READ, not decided here: `isAppLevelPath` is a lookup
  // over the `scope` field the navigation entry declares (S-425, FR-UI-35, ADR-66 §3),
  // the same field the sidebar sections itself by. This shell holds no list of its
  // own, so a view registered tomorrow cannot be keyed one way here and rendered
  // under the other section there.
  const viewKey = isAppLevelPath(pathname) ? "app" : cacheKey;

  return (
    <AppShell sidebar={<Sidebar pathname={pathname} />} header={<Header />}>
      {/* Nothing is mounted until the probe settles. A view that mounted first would
          fire its reads UNSCOPED (the scope is not set yet) and then re-fire them all
          on the mode flip — two full read-model passes per page load, the first of
          them against a member the user did not choose. The wait is one loopback
          round-trip against an engine-free endpoint. */}
      {mode === "loading" ? (
        <LoadingState label="Starting…" />
      ) : unknownMember !== null ? (
        /* INSTEAD of the view, never beside it — for EVERY path, app-level ones
           included; see the header for why that is the URL's claim and not the
           view's reads (NFR-RA-05). */
        <UnknownMember />
      ) : (
        View && <View key={viewKey} />
      )}
    </AppShell>
  );
}

export function App() {
  return (
    <ToastProvider>
      <WorkspaceProvider>
        <Shell />
      </WorkspaceProvider>
    </ToastProvider>
  );
}
