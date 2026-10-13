/*
 * The SPA's one rule for "does this route own this pathname" (S-642) — the route
 * itself, or a whole-segment sub-route under it. A leaf module with no imports, so
 * `nav.ts`, `views/hiddenWidgets.ts` and `extensions/registry.ts` can all call it
 * without importing one another.
 *
 * The trailing slash is what makes a match a whole path segment (`/healthz` is not
 * under `/health`), and it is also what keeps the root (`/`) from owning every route:
 * its sub-route test would be `startsWith("//")`, which no route satisfies. The
 * explicit `route !== "/"` states that intent — drop the trailing slash and it is the
 * only thing left standing between `/` and every route in the SPA.
 */
export function routeOwns(route: string, pathname: string): boolean {
  return pathname === route || (route !== "/" && pathname.startsWith(`${route}/`));
}
