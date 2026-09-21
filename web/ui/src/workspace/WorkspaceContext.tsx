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
 *
 * The URL (S-426, FR-UI-35, NFR-RA-05). The selected member is read from `?repo=`
 * on the URL the page opened on, and written back on every switch, so a workspace
 * URL names the member it shows. Two properties are load-bearing:
 *
 *   - It is resolved **before the first view mounts**, not after. The shell already
 *     holds every view back until the probe settles (`App.tsx`), and the member the
 *     URL names is decided inside that same probe handler — so a deep-linked member
 *     is the FIRST member any read is scoped to. There is no default-member pass
 *     followed by a correction, which would be two full read-model passes per page
 *     load, the first against a member the user did not ask for.
 *   - An unknown member is a REFUSAL, not a fallback. A `?repo=` naming a member the
 *     manifest does not list resolves to no member at all: the scope stays `null`,
 *     {@link WorkspaceContextValue.unknownMember} names what was asked for, and the
 *     shell renders that instead of any view. Answering from the default member
 *     would put one member's figures on screen under another member's name — the
 *     same substitution `member.rs` refuses server-side with a `404`, and the client
 *     must not re-merge what the server split. A member that IS in the roster but
 *     whose engine fails to start is a different claim: it scopes normally and the
 *     view surfaces the read's own `500`.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import type { WorkspaceRoster } from "../api/types.ts";
import { probeWorkspace } from "../api/workspaceClient.ts";
import { currentUrl, replaceUrl } from "../router.tsx";
import { memberFromSearch, normaliseMember, setScopedMember, urlWithMember } from "./scope.ts";

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
  /** The member the URL named that this workspace does not have, or `null`.
   *
   *  Non-`null` means the shell renders the unknown-member refusal and **no view at
   *  all** ([NFR-RA-05]): there is no member to answer for, and the default member's
   *  figures under the requested name is the exact lie this field exists to prevent.
   *  {@link WorkspaceContextValue.member} is `null` alongside it, and so is the
   *  transport scope. */
  unknownMember: string | null;
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
  unknownMember: null,
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
  const [unknownMember, setUnknownMember] = useState<string | null>(null);
  const [error, setError] = useState<Error | null>(null);
  /** The manifest roster, for the resolutions that happen after the probe (a
   *  back/forward that names a different member). Held in a ref, not read out of
   *  `members`, so the popstate listener does not have to be torn down and rebuilt
   *  every time the roster state settles. */
  const roster = useRef<WorkspaceRoster | null>(null);

  /**
   * Resolve `requested` (a normalised member name, or `null` for unscoped) against
   * the roster and commit it — the ONE place the member, the transport scope and the
   * unknown-member refusal move, so they can never disagree. Both callers go through
   * it: the boot/`popstate` URL read, and {@link selectMember}.
   *
   * Unscoped opens on the manifest's DEFAULT member: the one an unscoped request
   * would have answered from anyway. Falling back to the first roster entry would
   * silently present a different member than the CLI and the unscoped API do.
   */
  const resolveMember = useCallback((requested: string | null) => {
    const known = roster.current;
    // No roster means no workspace (single-root, or the probe has not answered), and
    // an unvalidated member must never be scoped: it is precisely the roster that
    // separates "a member this workspace has" from the refusal below.
    if (known === null) return;
    if (requested !== null && !known.members.includes(requested)) {
      // Absent from the manifest roster — decided here, against the roster the shell
      // already holds, so it costs no request and starts no engine. The scope is left
      // null and no member is selected; `App.tsx` renders the refusal in place of any
      // view (NFR-RA-05).
      setScopedMember(null);
      setMember(null);
      setUnknownMember(requested);
      return;
    }
    const opening = requested ?? known.default ?? known.members[0] ?? null;
    // Scope the transport BEFORE the mode flip re-renders the views, so the views'
    // first fetch already carries the selected member.
    setScopedMember(opening);
    setMember(opening);
    setUnknownMember(null);
  }, []);

  useEffect(() => {
    let alive = true;
    probeWorkspace()
      .then((probe) => {
        if (!alive) return;
        if (probe.mode === "single") {
          // A plain repo: leave the scope null so no request ever carries `?repo=`.
          // The URL is not consulted AT ALL here — a hand-typed `?repo=` stays inert,
          // exactly as any unrecognised query param always has (ADR-52).
          setScopedMember(null);
          setMode("single");
          return;
        }
        roster.current = probe.roster;
        setWorkspace(probe.roster.workspace);
        setMembers(probe.roster.members);
        // The URL's member is resolved HERE, in the same handler that flips the mode —
        // so the first member any view is scoped to is the one the URL named. No
        // default-member pass is fired and then corrected.
        resolveMember(memberFromSearch(window.location.search));
        setMode("workspace");
      })
      .catch((err: unknown) => {
        if (!alive) return;
        // A genuine fault (a 500, a transport failure) is NOT a plain repo. There is
        // no roster to render, so the shell falls back to the unscoped single-root
        // layout — but it records the fault, and the header states it
        // (WorkspaceFault) rather than passing the degradation off as "this is not a
        // workspace" (NFR-RA-05, NFR-CC-04).
        setError(err instanceof Error ? err : new Error(String(err)));
        setMode("single");
      });
    return () => {
      alive = false;
    };
  }, [resolveMember]);

  // Back/forward: the entry names a member, so restore the one it names — including
  // back out of an unknown member into a known one, and vice versa. Workspace mode
  // only: single-root never reads `?repo=` from anywhere.
  useEffect(() => {
    if (mode !== "workspace") return;
    const onPop = () => resolveMember(memberFromSearch(window.location.search));
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [mode, resolveMember]);

  const selectMember = useCallback(
    (name: string) => {
      // Through `resolveMember`, not beside it. Committing the three pieces of scope
      // state here as well would be a second spelling of the same invariant fifty
      // lines from the first, and the next piece of scope state would be added to
      // one of them — so a selection is resolved against the roster exactly as a
      // URL's member is, and only the history write is this callback's own.
      const requested = normaliseMember(name);
      // "Select nothing" is not an operation this offers: the selector's options are
      // roster names, and being unscoped is what an ABSENT `?repo=` means, not what
      // a blank selection does.
      if (requested === null) return;
      resolveMember(requested);
      // Write the selection through history so the URL names what is on screen and
      // can be bookmarked or shared. `replaceUrl`, not `navigate`: this is the same
      // view with a different member, so it must not cost a back-stack entry. The
      // URL names what was ASKED for, so a name the roster refuses survives a
      // refresh as the same refusal rather than silently becoming the default.
      replaceUrl(urlWithMember(currentUrl(), requested));
    },
    [resolveMember],
  );

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      mode,
      workspace,
      members,
      member,
      selectMember,
      unknownMember,
      error,
      // Single-root's key never changes, so nothing ever remounts and the UI behaves
      // exactly as before; in workspace mode it is the selected member, namespaced.
      cacheKey: member ? `member:${member}` : UNSCOPED_KEY,
    }),
    [mode, workspace, members, member, selectMember, unknownMember, error],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}
