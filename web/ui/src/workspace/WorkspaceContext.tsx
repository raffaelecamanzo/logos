/*
 * The workspace context (S-250, CR-061, FR-UI-29, FR-WS-06) — the shell-wide
 * "which member am I looking at?" state, and the mode discovery behind it.
 *
 * At boot the provider probes the S-249 fan-out once (`probeWorkspace`). The
 * surface's own honest `404` ("not a workspace") is the discovery signal, so no
 * new endpoint, no new shell meta tag, and — decisively — **no change to the
 * single-root served bytes**: a plain repo serves the identical bundle and shell,
 * the probe 404s, and the SPA renders exactly the pre-workspace UI (no selector,
 * no workspace tabs, no `?repo=` on any request).
 *
 * The cache key. Switching members must re-fetch *every* view, and the views are
 * many and pre-existing. Rather than thread a member into each view's dependency
 * array — which would mean editing every view and would silently rot the moment a
 * new one is added — the provider exposes {@link WorkspaceContextValue.cacheKey}
 * and the shell keys the mounted view subtree on it (`App.tsx`). A switch therefore
 * remounts the view, every `useApiResource` re-runs, and the transport scope
 * (`workspace/scope.ts`) is already set to the new member when it does. One
 * invariant, one place, and a view added tomorrow inherits it for free.
 */

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

import { probeWorkspace } from "../api/workspaceClient.ts";
import type { StatusInfo, WorkspaceStatus } from "../api/types.ts";
import { setScopedMember } from "./scope.ts";

/** One member as the shell knows it — its name, and whether it has an index yet. */
export interface WorkspaceMember {
  /** Repo-qualified member name (its workspace-relative path). */
  name: string;
  /** The member's index freshness, or `null` when its engine degraded / is unread. */
  status: StatusInfo | null;
  /** `false` when the member has no index yet — rendered as an honest "awaiting
   *  index" state rather than as zeroes (NFR-CC-04). */
  indexed: boolean;
  /** The per-member degradation, when the fan-out reported one. */
  error: string | null;
}

/** Which serve this SPA is talking to. `loading` is the pre-probe frame: the UI is
 *  rendered exactly as single-root until the probe answers, so a plain repo never
 *  flashes workspace chrome. */
export type WorkspaceMode = "loading" | "single" | "workspace";

export interface WorkspaceContextValue {
  mode: WorkspaceMode;
  /** The workspace name from the manifest, or `null` in single-root mode. */
  workspace: string | null;
  /** Every member, in manifest order. Empty in single-root mode. */
  members: WorkspaceMember[];
  /** The selected member, or `null` in single-root mode (nothing to select). */
  member: string | null;
  /** Select a member: re-scopes the transport and re-keys every view. */
  selectMember: (name: string) => void;
  /** The full workspace status (roster + cross-service coverage), or `null`. */
  status: WorkspaceStatus | null;
  /** The probe failed (a genuine fault, never a plain repo) — surfaced honestly. */
  error: Error | null;
  /** Changes whenever the scope changes — the shell keys the view subtree on it. */
  cacheKey: string;
}

const SINGLE_ROOT: WorkspaceContextValue = {
  mode: "loading",
  workspace: null,
  members: [],
  member: null,
  selectMember: () => {},
  status: null,
  error: null,
  cacheKey: "single",
};

const WorkspaceContext = createContext<WorkspaceContextValue>(SINGLE_ROOT);

/** The shell-wide workspace state. Single-root callers get `mode: "single"`, an
 *  empty roster, and a `null` member — so a view can render honestly either way. */
export function useWorkspace(): WorkspaceContextValue {
  return useContext(WorkspaceContext);
}

/** Project the fan-out's per-member results onto the roster the shell renders. */
function rosterOf(status: WorkspaceStatus): WorkspaceMember[] {
  return status.members.map((m) => ({
    name: m.member,
    status: m.result ?? null,
    indexed: m.result?.indexed ?? false,
    error: m.error ?? null,
  }));
}

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<WorkspaceMode>("loading");
  const [status, setStatus] = useState<WorkspaceStatus | null>(null);
  const [member, setMember] = useState<string | null>(null);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let alive = true;
    probeWorkspace()
      .then((probe) => {
        if (!alive) return;
        if (probe.mode === "single") {
          // A plain repo: leave the scope null so no request ever carries `?repo=`.
          setScopedMember(null);
          setMode("single");
          return;
        }
        const first = probe.status.members[0]?.member ?? null;
        // Scope the transport BEFORE the mode flip re-renders the views, so the
        // remounted views' first fetch already carries the selected member.
        setScopedMember(first);
        setStatus(probe.status);
        setMember(first);
        setMode("workspace");
      })
      .catch((err: unknown) => {
        if (!alive) return;
        // A genuine fault (a 500, a transport failure) is NOT a plain repo. There is
        // no roster to render, so the shell falls back to the unscoped single-root
        // layout — but it records the fault, and the header states it (MemberSelector)
        // rather than passing the degradation off as "this is not a workspace"
        // (NFR-RA-05, NFR-CC-04).
        setError(err instanceof Error ? err : new Error(String(err)));
        setMode("single");
      });
    return () => {
      alive = false;
    };
  }, []);

  const selectMember = useCallback((name: string) => {
    setScopedMember(name);
    setMember(name);
  }, []);

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      mode,
      workspace: status?.workspace ?? null,
      members: status ? rosterOf(status) : [],
      member,
      selectMember,
      status,
      error,
      // Single-root's key never changes, so nothing ever remounts and the UI
      // behaves byte-for-byte as before; in workspace mode it IS the member.
      cacheKey: member ?? "single",
    }),
    [mode, status, member, selectMember, error],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}
