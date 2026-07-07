/*
 * The root SPA component (S-185 shell, re-skinned onto the design system in
 * S-193). Composes the shared AppShell layout primitive with the Sidebar + Header,
 * wrapped in the ToastProvider so any view can surface notifications.
 *
 * Every tab is a React view mounted in the AppShell content slot keyed off the
 * client pathname (`usePathname` → `viewForPath`), registered in `views/index.ts`.
 * The Dashboard is at `/` (S-194). The retired `/overview` route is silently
 * redirected to `/` (replaceState — no extra history entry, bookmarks survive).
 */

import { useEffect } from "react";

import { AppShell, ToastProvider } from "./components/index.ts";
import { usePathname, redirect } from "./router.tsx";
import { Header } from "./shell/Header.tsx";
import { Sidebar } from "./shell/Sidebar.tsx";
import { viewForPath } from "./views/index.ts";

export function App() {
  const rawPathname = usePathname();

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
    <ToastProvider>
      <AppShell sidebar={<Sidebar pathname={pathname} />} header={<Header />}>
        {View && <View />}
      </AppShell>
    </ToastProvider>
  );
}
