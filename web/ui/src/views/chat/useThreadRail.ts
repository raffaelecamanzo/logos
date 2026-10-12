/*
 * The conversation-history RAIL's state machine — the multi-thread half of the Chat
 * tab (S-209, S-210, S-211, FR-UI-26, FR-UI-20, ADR-47).
 *
 * Extracted from `chatRuntime.tsx` so the runtime adapter composes it rather than
 * carrying it: this hook owns exactly the rail's concerns — the conversation list,
 * the generation sequencing that keeps concurrent list reads honest, adopting the
 * thread a first send creates, the per-conversation delete and rename, and the
 * title a first turn's Synthesizer names (S-623, FR-UI-48) — while the surface
 * (messages, the streamed turn, the assistant-ui adapter) stays in `chatRuntime.tsx`.
 * Nothing here touches the message array; the two cross-cutting actions it needs —
 * adopting a thread and resetting the surface — arrive as callbacks.
 *
 * The masked chat key never reaches this layer (NFR-SE-07): these are the read-only
 * list/transcript reads and the intent-guarded delete and rename, none of which carry key
 * material on any code path.
 */

import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";

import {
  deleteChatThread,
  fetchThreads,
  renameChatThread,
  type ChatRoutes,
} from "../../api/chatClient.ts";
import {
  chatStorageKey,
  normalizeThreadTitle,
  type ChatStorageScope,
  type ThreadSummary,
} from "./chatModel.ts";

/** What the rail needs from the conversation surface it lives beside. */
export interface ThreadRailDeps {
  /** The chat whose conversations this rail lists and deletes (S-485) — the
   *  member's, or the workspace's own store. */
  routes: ChatRoutes;
  /** The open conversation, read inside async callbacks without re-binding them. */
  activeThreadIdRef: MutableRefObject<number | null>;
  /** Adopt the conversation a first send just created (S-210 first-send persistence). */
  adoptThread: (id: number) => void;
  /** Reset the surface to a fresh composer — used when the OPEN conversation is deleted. */
  resetSurface: () => void;
  /** A conversation is gone (deleted, or already gone): drop what the surface keeps
   *  per conversation — its preset (S-625) — since the store may hand its id to the
   *  next conversation. */
  onDeleted?: (id: number) => void;
}

/** The rail's state and actions, consumed by `useChatRuntime` and `ThreadList`. */
export interface ThreadRail {
  /** The conversation list, most-recent-first (S-209 `GET /chat/threads`). */
  threads: ThreadSummary[];
  /** An honest note when the list could not be read or an action failed, else `null`. */
  threadsError: string | null;
  /** Report (or clear) an honest rail note from an action the surface owns. */
  setThreadsError: (message: string | null) => void;
  /** The ids currently on the rail — the pre-send snapshot a first send diffs against. */
  knownThreadIds: () => ReadonlySet<number>;
  /** Read the conversation list, most-recent-first. */
  loadThreads: () => Promise<void>;
  /** Refresh the rail after a turn settles, adopting a just-created conversation. */
  reconcileThreads: (
    createdNewThread: boolean,
    knownIds: ReadonlySet<number> | null,
  ) => Promise<void>;
  /** Delete ONE conversation and its per-thread memory (the caller confirms first). */
  deleteThread: (id: number) => Promise<void>;
  /** Rename ONE conversation (S-623; the caller has already refused an empty name). */
  renameThread: (id: number, title: string) => Promise<void>;
  /** Show the title a first turn's Synthesizer named (the `thread_title` frame). */
  applyTitle: (id: number, title: string) => void;
}

/**
 * Own the conversation rail: the list, its concurrency sequencing, the first-send
 * adoption, the per-conversation delete and rename, and a streamed title
 * (S-209/S-210/S-211/S-623).
 */
export function useThreadRail({
  routes,
  activeThreadIdRef,
  adoptThread,
  resetSurface,
  onDeleted,
}: ThreadRailDeps): ThreadRail {
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [threadsError, setThreadsError] = useState<string | null>(null);

  // Latest rail list, read at send time to diff the just-created thread.
  const threadsRef = useRef(threads);
  threadsRef.current = threads;
  // A monotonic RAIL-LIST generation, the `sessionRef` idea applied to `threads`.
  // Several call sites read the list concurrently (mount/restore, a turn's trailing
  // reconcile, a delete's or a rename's refresh) and HTTP responses can resolve out of the
  // order they were issued. Without sequencing, a reconcile whose `fetchThreads`
  // started BEFORE a delete landed could resolve after it and overwrite the list
  // with its stale pre-delete snapshot — resurrecting a conversation that is gone
  // server-side (the rail would lie until the user clicked the ghost row and hit
  // its 404). Each reader takes a ticket via `nextThreadsGen` and applies its
  // result only while that ticket is still the newest: last STARTED read wins.
  const threadsGenRef = useRef(0);

  // Take the next rail-list ticket. Also called by a change that already knows the
  // truth (a delete, a rename, a streamed title), so any read still in flight is
  // superseded outright.
  const nextThreadsGen = useCallback(() => (threadsGenRef.current += 1), []);

  const knownThreadIds = useCallback(
    (): ReadonlySet<number> => new Set(threadsRef.current.map((t) => t.id)),
    [],
  );

  // Read the conversation list (most-recent-first, S-209). An honest error note on
  // failure rather than a silently empty rail ([NFR-CC-04]).
  const loadThreads = useCallback(async () => {
    const gen = nextThreadsGen();
    try {
      const list = await fetchThreads(routes);
      // A newer read (or a delete) started while this one was in flight — it knows
      // more than we do, so drop this result rather than overwrite it.
      if (threadsGenRef.current !== gen) return;
      setThreads(list);
      setThreadsError(null);
    } catch (e) {
      if (threadsGenRef.current !== gen) return;
      setThreadsError(
        `Could not load your conversations: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }, [routes, nextThreadsGen]);

  // After a turn settles, refresh the rail (updated_at re-orders the list; the first
  // send titles the new thread). For a genuine new-conversation send, adopt the
  // just-created thread — identified precisely as the id ABSENT from the pre-send
  // set (`knownIds`), never "top of list": the server creates the thread in setup
  // even if the turn then faults, so a new id always exists, and diffing never
  // adopts an unrelated existing conversation. The SSE stream names the new thread
  // only in a `thread_title` (S-623), which a fallback-titled turn never sends, so
  // this list re-read is how the SPA learns it (S-210 first-send persistence).
  const reconcileThreads = useCallback(
    async (createdNewThread: boolean, knownIds: ReadonlySet<number> | null) => {
      const gen = nextThreadsGen();
      let list: ThreadSummary[];
      try {
        list = await fetchThreads(routes);
      } catch {
        // The turn itself succeeded and is durable server-side; a rail-refresh
        // failure is non-fatal. But for a brand-new conversation we could not learn
        // its id — say so honestly so the user reopens it from the list, rather than
        // the next send silently forking a second thread against a still-null id.
        if (createdNewThread && threadsGenRef.current === gen) {
          setThreadsError(
            "Your conversation was saved, but the history list could not refresh — reopen it from the rail.",
          );
        }
        return;
      }
      // Only RENDER this list while it is still the newest read: a delete (or a
      // later read) that landed while this was in flight knows more, and painting
      // our older snapshot over it would resurrect a deleted conversation.
      if (threadsGenRef.current === gen) {
        setThreads(list);
        setThreadsError(null);
      }
      // ADOPTION is not subject to that guard. It answers "which id did MY send
      // create?" — a question a concurrent delete of some other conversation cannot
      // change, and the id is absent from `knownIds` in a stale and a fresh list
      // alike. Skipping it would leave `activeThreadId` null and let the next send
      // fork a duplicate thread — the exact S-210 regression this diffing prevents.
      if (createdNewThread && knownIds && activeThreadIdRef.current == null) {
        const created = list.find((t) => !knownIds.has(t.id));
        if (created) adoptThread(created.id);
      }
    },
    [routes, nextThreadsGen, activeThreadIdRef, adoptThread],
  );

  // Per-conversation delete (S-211, [FR-UI-26], [FR-UI-20], [ADR-47]) — the granular
  // replacement for the retired global Clear-history. The CONFIRMATION is the rail's
  // (`ThreadList`); by the time this runs the user has already confirmed, so it goes
  // straight to the intent-guarded `POST …/{id}/delete` whose FK cascade wipes that
  // thread's messages AND its per-thread memory server-side.
  //
  // Honest outcomes, mirroring the S-209 handler: `204` deleted and `404` already
  // gone are BOTH success for the user's intent ("this conversation should not
  // exist") — the row leaves the rail either way. Anything else keeps the row and
  // says so ([NFR-CC-04]), never a silent disappearance of a conversation that is
  // still on disk.
  const deleteThread = useCallback(
    async (id: number) => {
      let resp: Response;
      try {
        resp = await deleteChatThread(routes, id);
      } catch (e) {
        setThreadsError(
          `Could not delete that conversation: ${e instanceof Error ? e.message : String(e)}`,
        );
        return;
      }
      if (!resp.ok && resp.status !== 404) {
        setThreadsError(`Could not delete that conversation (status ${resp.status}).`);
        return;
      }
      // Deleting the OPEN conversation leaves the surface pointing at a thread that
      // no longer exists — reset to a fresh composer (which also aborts any turn
      // still streaming into it and drops the stored selection). Deleting any other
      // conversation leaves the current transcript untouched.
      if (activeThreadIdRef.current === id) resetSurface();
      onDeleted?.(id);
      // Drop the row on the delete's own authority, then re-read for the
      // authoritative order — so the rail is right even if that refresh faults.
      // Take a ticket FIRST: any list read already in flight was issued before we
      // knew this conversation was gone, so its result must not paint the row back.
      nextThreadsGen();
      setThreads((prev) => prev.filter((t) => t.id !== id));
      setThreadsError(null);
      await loadThreads();
    },
    [routes, activeThreadIdRef, resetSurface, onDeleted, loadThreads, nextThreadsGen],
  );

  // Per-conversation rename (S-623, [FR-UI-48], [ADR-47]) — the intent-guarded
  // `POST …/{id}/rename`. The rail has already refused an empty name, and the store
  // refuses one again (`400`). On success the row takes the new title at once and
  // the list is re-read; a rejected or failed rename keeps the row's title and says
  // so ([NFR-CC-04]) rather than showing a name the store does not hold.
  const renameThread = useCallback(
    async (id: number, title: string) => {
      let resp: Response;
      try {
        resp = await renameChatThread(routes, id, title);
      } catch (e) {
        setThreadsError(
          `Could not rename that conversation: ${e instanceof Error ? e.message : String(e)}`,
        );
        return;
      }
      if (resp.status === 404) {
        // Gone server-side: re-read so the row leaves the rail, THEN say why — a
        // successful re-read clears the note, so the note must come after it.
        await loadThreads();
        setThreadsError("That conversation no longer exists.");
        return;
      }
      if (!resp.ok) {
        setThreadsError(`Could not rename that conversation (status ${resp.status}).`);
        return;
      }
      // Rename on the write's own authority, then re-read. The ticket supersedes a
      // list read already in flight, which would otherwise paint the old title back.
      const stored = normalizeThreadTitle(title) ?? title;
      nextThreadsGen();
      setThreads((prev) => prev.map((t) => (t.id === id ? { ...t, title: stored } : t)));
      setThreadsError(null);
      await loadThreads();
    },
    [routes, loadThreads, nextThreadsGen],
  );

  // The Synthesizer's title for the conversation a first send just created
  // (S-623): it arrives mid-stream, before the turn's trailing re-read, and the row
  // is not on the rail yet — the list was read before the send. Show it at once:
  // retitle the row if it is there, else add it at the top, where the re-read will
  // put it. Adoption is untouched: it diffs the PRE-send ids, so the row added here
  // is still recognised as the one the send created.
  const applyTitle = useCallback(
    (id: number, title: string) => {
      nextThreadsGen();
      setThreads((prev) =>
        prev.some((t) => t.id === id)
          ? prev.map((t) => (t.id === id ? { ...t, title } : t))
          : [{ id, title, updated_at: Math.floor(Date.now() / 1000) }, ...prev],
      );
    },
    [nextThreadsGen],
  );

  return {
    threads,
    threadsError,
    setThreadsError,
    knownThreadIds,
    loadThreads,
    reconcileThreads,
    deleteThread,
    renameThread,
    applyTitle,
  };
}

// ── The rail's collapse toggle (S-619, FR-UI-45, CR-204 L6) ─────────────────────

/** The localStorage key BASE remembering whether the reader left the rail open BESIDE
 *  the conversation (desktop widths) — qualified by the chat's storage scope
 *  ({@link chatStorageKey}), like the open conversation's key, so the member chat and
 *  the Workspace Chat each keep their own choice. `"1"` open, `"0"` collapsed;
 *  anything else is "no choice made". */
export const RAIL_OPEN_KEY = "logos.chat.railOpen";

/** The width below which the panes stack and the rail sits ABOVE the conversation —
 *  the stylesheet's `@media (max-width: 1023px)` rung, and the shell's own. */
const NARROW_QUERY = "(max-width: 1023px)";

/** Whether the viewport is narrow enough that the rail stacks above the conversation.
 *  `false` where `matchMedia` is unavailable: the desktop default. */
function isNarrowViewport(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(NARROW_QUERY).matches;
}

/** The reader's remembered choice under `scope`, or `null` when none was made or
 *  storage is blocked. */
function readStoredRailOpen(scope: ChatStorageScope): boolean | null {
  try {
    const raw = window.localStorage.getItem(chatStorageKey(RAIL_OPEN_KEY, scope));
    return raw === "1" ? true : raw === "0" ? false : null;
  } catch {
    return null;
  }
}

/** Whether the viewport is narrow, kept current as the window crosses the breakpoint. */
function useNarrowViewport(): boolean {
  const [narrow, setNarrow] = useState(isNarrowViewport);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(NARROW_QUERY);
    const sync = () => setNarrow(query.matches);
    sync();
    query.addEventListener?.("change", sync);
    return () => query.removeEventListener?.("change", sync);
  }, []);
  return narrow;
}

/** The rail's disclosure, consumed by `ChatConfigured`. */
export interface RailDisclosure {
  /** Whether the rail is shown. */
  open: boolean;
  /** The reader's toggle: flips the rail — and, beside a desktop conversation,
   *  remembers the choice for this scope. */
  toggle: () => void;
  /** Close the rail after a pick, where it sits above the conversation (≤1023px) so
   *  the restored thread is in view. A no-op at desktop widths, where the rail sits
   *  beside. */
  closeIfStacked: () => void;
}

/**
 * Own the rail's collapse toggle (S-619, CR-204 L6): shown at every width.
 *
 * The two widths keep two states, because the rail means two different things there.
 * BESIDE a desktop conversation it is a layout preference: open unless the reader
 * collapsed it, remembered per scope (best-effort, as the open conversation is).
 * ABOVE a narrow conversation it is a disclosure that pushes the conversation down,
 * so it behaves exactly as before S-619 (S-210 AC-3, [FR-UI-33]): it always starts
 * collapsed, a pick closes it, and nothing about it is remembered. Crossing the
 * breakpoint shows the state of the width you land on, so a narrow pick never hides
 * the rail beside a desktop conversation, and a desktop choice never opens it above a
 * narrow one.
 */
export function useRailDisclosure(scope: ChatStorageScope): RailDisclosure {
  const narrow = useNarrowViewport();
  const [besideOpen, setBesideOpen] = useState<boolean>(() => readStoredRailOpen(scope) ?? true);
  const [aboveOpen, setAboveOpen] = useState(false);

  const toggle = useCallback(() => {
    if (narrow) {
      setAboveOpen((was) => !was);
      return;
    }
    const next = !besideOpen;
    setBesideOpen(next);
    try {
      window.localStorage.setItem(chatStorageKey(RAIL_OPEN_KEY, scope), next ? "1" : "0");
    } catch {
      /* non-fatal: the rail still toggles, it is only not remembered */
    }
  }, [narrow, besideOpen, scope]);

  const closeIfStacked = useCallback(() => setAboveOpen(false), []);

  return { open: narrow ? aboveOpen : besideOpen, toggle, closeIfStacked };
}
