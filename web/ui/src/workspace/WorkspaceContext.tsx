/*
 * The workspace context (S-250, CR-061, FR-UI-29, FR-WS-06) — the shell-wide
 * "which member am I looking at?" state, and the mode discovery behind it.
 *
 * At boot the provider probes the workspace ROSTER once (`probeWorkspace` →
 * `/api/v1/workspace/roster`). Two properties of that endpoint are load-bearing:
 *
 *   - It is **engine-free**: it projects the manifest and starts no member. The
 *     shell probes on every page load, so probing `workspace/status` instead (which
 *     fans out over every member) would eagerly construct and watch all N member
 *     engines on first paint — undoing the warm-only-the-default policy (NFR-PE-10)
 *     that the federated serve exists to keep.
 *   - Its `404` ("not a workspace") IS the single-root signal. Consuming the
 *     surface's own honest refusal means no new shell meta tag and no capability
 *     flag — and therefore **no change to the single-root served bytes**: a plain
 *     repo serves the identical bundle and shell, the probe 404s, and the SPA
 *     renders exactly the pre-workspace UI (no selector, no workspace tabs, no
 *     `?repo=` on any request).
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

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { probeWorkspace } from "../api/workspaceClient.ts";
import { setScopedMember } from "./scope.ts";

/** Which serve this SPA is talking to. `loading` is the pre-probe frame: the UI is
 *  rendered as it always was until the probe answers, so a plain repo never flashes
 *  workspace chrome — and, just as importantly, nothing ASSERTS a mode it does not
 *  yet know (NFR-CC-04). */
export type WorkspaceMode = "loading" | "single" | "workspace";

export interface WorkspaceContextValue {
  mode: WorkspaceMode;
  /** The workspace name from the manifest, or `null` in single-root mode. */
  workspace: string | null;
  /** Every member's name, in manifest order. Empty in single-root mode. */
  members: string[];
  /** The selected member, or `null` in single-root mode (nothing to select). */
  member: string | null;
  /** Select a member: re-scopes the transport and re-keys every view. */
  selectMember: (name: string) => void;
  /** The probe failed (a genuine fault, never a plain repo) — surfaced honestly. */
  error: Error | null;
  /** Changes whenever the scope changes — the shell keys the view subtree on it. */
  cacheKey: string;
}

/** The cache key for "no member scope" — namespaced apart from every member key so a
 *  member literally named `single` cannot collide with it (member names are
 *  workspace-relative paths, so `single` is a perfectly legal one). A collision would
 *  mean the view never remounts on the mode flip and keeps the unscoped default
 *  member's data under that member's name — exactly the cross-member contamination
 *  the key exists to prevent. */
const UNSCOPED_KEY = "single";

/** The pre-probe / no-provider default: mode is `loading`, nothing is scoped. */
const PRE_PROBE: WorkspaceContextValue = {
  mode: "loading",
  workspace: null,
  members: [],
  member: null,
  selectMember: () => {},
  error: null,
  cacheKey: UNSCOPED_KEY,
};

const WorkspaceContext = createContext<WorkspaceContextValue>(PRE_PROBE);

/** The shell-wide workspace state. Single-root callers get `mode: "single"`, an
 *  empty roster, and a `null` member — so a view can render honestly either way. */
export function useWorkspace(): WorkspaceContextValue {
  return useContext(WorkspaceContext);
}

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<WorkspaceMode>("loading");
  const [workspace, setWorkspace] = useState<string | null>(null);
  const [members, setMembers] = useState<string[]>([]);
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
        const { roster } = probe;
        // Open on the manifest's DEFAULT member — the one an unscoped request would
        // have answered from anyway. Falling back to the first roster entry would
        // silently present a different member than the CLI and the unscoped API do.
        const opening = roster.default ?? roster.members[0] ?? null;
        // Scope the transport BEFORE the mode flip re-renders the views, so the
        // views' first fetch already carries the selected member.
        setScopedMember(opening);
        setWorkspace(roster.workspace);
        setMembers(roster.members);
        setMember(opening);
        setMode("workspace");
      })
      .catch((err: unknown) => {
        if (!alive) return;
        // A genuine fault (a 500, a transport failure) is NOT a plain repo. There is
        // no roster to render, so the shell falls back to the unscoped single-root
        // layout — but it records the fault, and the header states it
        // (MemberSelector) rather than passing the degradation off as "this is not a
        // workspace" (NFR-RA-05, NFR-CC-04).
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
      workspace,
      members,
      member,
      selectMember,
      error,
      // Single-root's key never changes, so nothing ever remounts and the UI behaves
      // exactly as before; in workspace mode it is the selected member, namespaced.
      cacheKey: member ? `member:${member}` : UNSCOPED_KEY,
    }),
    [mode, workspace, members, member, selectMember, error],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}
