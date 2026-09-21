/*
 * A minimal client-side router (S-185, FR-UI-22). The SPA shell is served at `/`
 * and, server-side, an unmatched HTML navigation falls back to the shell (ADR-43)
 * so a refresh on a client route resolves. This hook tracks the current pathname
 * so the shell can render the React view for it.
 *
 * Deliberately dependency-free: the client route table is small (one route per
 * tab), so a full router (react-router) would be premature weight on the bundle
 * the S-184 fitness budget tracks. It can be adopted if the route table grows.
 *
 * S-426 (FR-UI-35, NFR-RA-05): the router is where `?repo=` meets `window.history`.
 * {@link navigate} and {@link redirect} stamp the ACTIVE member onto the entry they
 * write, so a workspace URL names the member it shows and a tab change does not
 * silently re-open the default one. ({@link replaceUrl} does not: it writes the URL
 * its caller hands it, already stamped.) That is the whole of the member policy
 * here — which member is active, and what an unknown one means, belong to
 * `WorkspaceContext`. In single-root mode `scopedMember()` is `null` and
 * `urlWithMember` returns the path verbatim, so every URL this module writes is
 * byte-for-byte the pre-workspace one ([ADR-52]).
 *
 * The stamp is the ACTIVE member, which is not always the one the URL names, and the
 * gap is deliberate rather than overlooked. `scopedMember()` is null in two states
 * the sidebar is clickable in — before the boot probe answers, and while an unknown
 * member is being refused — so a tab click in either drops the `?repo=` instead of
 * carrying it. Reading the URL's member instead would be worse: single-root must
 * treat a hand-typed `?repo=` as inert, and the router cannot tell the modes apart,
 * so it would start propagating that param onto every navigation ([ADR-52], AC4).
 * The resulting page is honest — the URL names no member, and nothing on screen is
 * labelled with one — so the member is lost, never misreported. A URL migration that
 * must keep the member therefore passes the query through itself; `App.tsx`'s
 * `/overview` redirect does exactly that.
 */

import { useEffect, useState } from "react";

import { scopedMember, urlWithMember } from "./workspace/scope.ts";

/** The current client-side pathname, kept in sync with browser history. */
export function usePathname(): string {
  const [pathname, setPathname] = useState<string>(() => window.location.pathname);
  useEffect(() => {
    const onPop = () => setPathname(window.location.pathname);
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  return pathname;
}

/**
 * Navigate to an in-SPA route without a full reload (every tab is a React view).
 * The optional `state` rides in `history.state` for the destination view to read
 * via `useNavigationState` — an ephemeral client-side payload (e.g. the wiki
 * search term crossing into the reader, FR-WK-28) that needs no URL query param
 * or read-model change.
 */
export function navigate(path: string, state: unknown = {}): void {
  window.history.pushState(state, "", urlWithMember(path, scopedMember()));
  window.dispatchEvent(new PopStateEvent("popstate"));
}

/**
 * The current history-entry's state, kept in sync like `usePathname`. Reads
 * `window.history.state` (set by `navigate`'s `pushState`, or by the browser on
 * back/forward) rather than the dispatched event's own `.state` — the synthetic
 * `PopStateEvent` above carries none, but `window.history.state` is authoritative
 * either way.
 */
export function useNavigationState<T = unknown>(): T | null {
  const [state, setState] = useState<T | null>(() => (window.history.state as T) ?? null);
  useEffect(() => {
    const onPop = () => setState((window.history.state as T) ?? null);
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  return state;
}

/**
 * Replace the current history entry and update the SPA pathname — used for
 * transparent URL migrations so no extra back-stack entry is created (S-194:
 * `/overview` → `/`).
 */
export function redirect(path: string): void {
  window.history.replaceState({}, "", urlWithMember(path, scopedMember()));
  window.dispatchEvent(new PopStateEvent("popstate"));
}

/** The current history entry's full in-SPA URL — path, query and fragment. */
export function currentUrl(): string {
  const { pathname, search, hash } = window.location;
  return `${pathname}${search}${hash}`;
}

/**
 * Rewrite the current history entry's URL in place (S-426).
 *
 * It shares `replaceState` with {@link redirect}, so a member switch on the view you
 * are already looking at adds **no** back-stack entry — ten switches must not cost
 * ten presses of Back. Two further properties are its own:
 *
 *   - `history.state` is carried across, not replaced with `{}` — the current entry's
 *     ephemeral payload (the wiki search term, FR-WK-28) belongs to the entry, not to
 *     the member shown in it.
 *   - **no** `popstate` is dispatched. The caller is the one that just changed the
 *     state; re-notifying the SPA would have it re-derive from the URL the answer it
 *     has already committed to.
 */
export function replaceUrl(url: string): void {
  window.history.replaceState(window.history.state, "", url);
}
