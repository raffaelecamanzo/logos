/*
 * A minimal client-side router (S-185, FR-UI-22). The SPA shell is served at `/`
 * and, server-side, an unmatched HTML navigation falls back to the shell (ADR-43)
 * so a refresh on a client route resolves. This hook tracks the current pathname
 * so the shell can render the React view for it.
 *
 * Deliberately dependency-free: the client route table is small (one route per
 * tab), so a full router (react-router) would be premature weight on the bundle
 * the S-184 fitness budget tracks. It can be adopted if the route table grows.
 */

import { useEffect, useState } from "react";

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
  window.history.pushState(state, "", path);
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
  window.history.replaceState({}, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}
