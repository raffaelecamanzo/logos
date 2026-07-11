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
 */

import { useEffect } from "react";

import { AppShell, ToastProvider } from "./components/index.ts";
import { usePathname, redirect } from "./router.tsx";
import { Header } from "./shell/Header.tsx";
import { Sidebar } from "./shell/Sidebar.tsx";
import { viewForPath } from "./views/index.ts";
import { useWorkspace, WorkspaceProvider } from "./workspace/WorkspaceContext.tsx";

function Shell() {
  const rawPathname = usePathname();
  const { cacheKey } = useWorkspace();

  // Silently migrate the retired /overview bookmark to / without adding a
  // back-stack entry.
  useEffect(() => {
    if (rawPathname === "/overview") redirect("/");
  }, [rawPathname]);

  // Canonical path: resolve the redirect synchronously so the Dashboard view
  // renders on the first frame (no blank-content flash before the effect fires).
  const pathname = rawPathname === "/overview" ? "/" : rawPathname;
  const View = viewForPath(pathname);

  return (
    <AppShell sidebar={<Sidebar pathname={pathname} />} header={<Header />}>
      {View && <View key={cacheKey} />}
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
