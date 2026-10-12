/*
 * What the shell does with a route an extension owns (S-642, FR-UI-51): wait for the
 * server's answer, then mount the view if the extension is active, or land on the
 * Dashboard — the landing a hidden view gets — if it is not. Kept beside the registry
 * rather than in `views/index.ts` so the view registry stays one thing: path → component.
 */

import { viewForPath } from "../views/index.ts";
import { getExtensions, ownsRegisteredPath } from "./registry.ts";

/**
 * Where a visit to `pathname` lands because it names a route a REGISTERED extension
 * owns but that extension is not mounted for this serve (S-642, FR-UI-51): the
 * Dashboard, the same landing a hidden view gets (`hiddenViewLanding`). `null` for
 * every other path — a built-in route, a mounted extension's, or one nobody owns —
 * and until the boot read has settled, when {@link extensionDecisionPending} holds
 * the shell instead: the answer depends on what the server reports.
 */
export function inactiveExtensionLanding(pathname: string): string | null {
  if (extensionDecisionPending(pathname)) return null;
  return ownsRegisteredPath(pathname) && viewForPath(pathname) === null ? "/" : null;
}

/**
 * Is `pathname` a route a registered extension owns while the boot read of
 * `GET /api/v1/extensions` is still in flight? The shell mounts nothing for such a
 * path yet — mounting the extension's view early would show a surface the server
 * may be about to disown, and landing on the Dashboard early would bounce a valid
 * deep link. Only a registered extension's routes wait, so a build that registers
 * none (the public one) never waits at all — and neither does a route a built-in view
 * serves, even one an extension also registered over: nothing is mounted while the
 * read is in flight, so `viewForPath` can only answer with a built-in, and a built-in
 * route always wins.
 */
export function extensionDecisionPending(pathname: string): boolean {
  return (
    getExtensions().status === "loading" &&
    ownsRegisteredPath(pathname) &&
    viewForPath(pathname) === null
  );
}
