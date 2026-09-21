/*
 * The root SPA component (S-185 shell, re-skinned onto the design system in
 * S-193). Composes the shared AppShell layout primitive with the Sidebar + Header,
 * wrapped in the ToastProvider so any view can surface notifications.
 *
 * Every tab is a React view mounted in the AppShell content slot keyed off the
 * client pathname (`usePathname` → `viewForPath`), registered in `views/index.ts`.
 * The Dashboard is at `/` (S-194). The retired `/overview` route is silently
 * redirected to `/` (replaceState — no extra history entry, bookmarks survive).
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
 * refusal takes the content slot instead. A view mounted there would read unscoped
 * and paint the DEFAULT member's figures under the requested member's name — a
 * `200`-shaped page that is wrong about its own subject, which is precisely what
 * the server refuses with a `404` and what this shell must not undo.
 */

import { useEffect } from "react";

import { AppShell, LoadingState, ToastProvider } from "./components/index.ts";
import { isAppLevelPath } from "./nav.ts";
import { usePathname, redirect } from "./router.tsx";
import { Header } from "./shell/Header.tsx";
import { Sidebar } from "./shell/Sidebar.tsx";
import { viewForPath } from "./views/index.ts";
import { UnknownMember } from "./workspace/UnknownMember.tsx";
import { useWorkspace, WorkspaceProvider } from "./workspace/WorkspaceContext.tsx";

function Shell() {
  const rawPathname = usePathname();
  const { cacheKey, mode, unknownMember } = useWorkspace();

  // Silently migrate the retired /overview bookmark to / without adding a
  // back-stack entry.
  useEffect(() => {
    if (rawPathname === "/overview") redirect("/");
  }, [rawPathname]);

  // Canonical path: resolve the redirect synchronously so the Dashboard view
  // renders on the first frame (no blank-content flash before the effect fires).
  const pathname = rawPathname === "/overview" ? "/" : rawPathname;
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
        /* INSTEAD of the view, never beside it: there is no member to answer for,
           and the transport is unscoped, so nothing on screen can be one member's
           figures wearing another member's name (NFR-RA-05). */
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
