/*
 * The conversation-history RAIL's state machine — the multi-thread half of the Chat
 * tab (S-209, S-210, S-211, FR-UI-26, FR-UI-20, ADR-47).
 *
 * Extracted from `chatRuntime.tsx` so the runtime adapter composes it rather than
 * carrying it: this hook owns exactly the rail's concerns — the conversation list,
 * the generation sequencing that keeps concurrent list reads honest, adopting the
 * thread a first send creates, and the per-conversation delete — while the surface
 * (messages, the streamed turn, the assistant-ui adapter) stays in `chatRuntime.tsx`.
 * Nothing here touches the message array; the two cross-cutting actions it needs —
 * adopting a thread and resetting the surface — arrive as callbacks.
 *
 * The masked chat key never reaches this layer (NFR-SE-07): these are the read-only
 * list/transcript reads and the intent-guarded delete, none of which carry key
 * material on any code path.
 */

import { useCallback, useRef, useState, type MutableRefObject } from "react";

import { deleteChatThread, fetchThreads } from "../../api/chatClient.ts";
import type { ThreadSummary } from "./chatModel.ts";

/** What the rail needs from the conversation surface it lives beside. */
export interface ThreadRailDeps {
  /** The open conversation, read inside async callbacks without re-binding them. */
  activeThreadIdRef: MutableRefObject<number | null>;
  /** Adopt the conversation a first send just created (S-210 first-send persistence). */
  adoptThread: (id: number) => void;
  /** Reset the surface to a fresh composer — used when the OPEN conversation is deleted. */
  resetSurface: () => void;
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
}

/**
 * Own the conversation rail: the list, its concurrency sequencing, the first-send
 * adoption, and the per-conversation delete (S-209/S-210/S-211).
 */
export function useThreadRail({
  activeThreadIdRef,
  adoptThread,
  resetSurface,
}: ThreadRailDeps): ThreadRail {
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  const [threadsError, setThreadsError] = useState<string | null>(null);

  // Latest rail list, read at send time to diff the just-created thread.
  const threadsRef = useRef(threads);
  threadsRef.current = threads;
  // A monotonic RAIL-LIST generation, the `sessionRef` idea applied to `threads`.
  // Three call sites read the list concurrently (mount/restore, a turn's trailing
  // reconcile, and a delete's refresh) and HTTP responses can resolve out of the
  // order they were issued. Without sequencing, a reconcile whose `fetchThreads`
  // started BEFORE a delete landed could resolve after it and overwrite the list
  // with its stale pre-delete snapshot — resurrecting a conversation that is gone
  // server-side (the rail would lie until the user clicked the ghost row and hit
  // its 404). Each reader takes a ticket via `nextThreadsGen` and applies its
  // result only while that ticket is still the newest: last STARTED read wins.
  const threadsGenRef = useRef(0);

  // Take the next rail-list ticket. Also called by a mutation that already knows
  // the truth (the delete), so any read still in flight is superseded outright.
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
      const list = await fetchThreads();
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
  }, [nextThreadsGen]);

  // After a turn settles, refresh the rail (updated_at re-orders the list; the first
  // send auto-titles the new thread). For a genuine new-conversation send, adopt the
  // just-created thread — identified precisely as the id ABSENT from the pre-send
  // set (`knownIds`), never "top of list": the server creates the thread in setup
  // even if the turn then faults, so a new id always exists, and diffing never
  // adopts an unrelated existing conversation. The SSE stream carries no thread id
  // (S-209), so this list re-read is how the SPA learns it (S-210 first-send
  // persistence).
  const reconcileThreads = useCallback(
    async (createdNewThread: boolean, knownIds: ReadonlySet<number> | null) => {
      const gen = nextThreadsGen();
      let list: ThreadSummary[];
      try {
        list = await fetchThreads();
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
    [nextThreadsGen, activeThreadIdRef, adoptThread],
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
        resp = await deleteChatThread(id);
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
      // Drop the row on the delete's own authority, then re-read for the
      // authoritative order — so the rail is right even if that refresh faults.
      // Take a ticket FIRST: any list read already in flight was issued before we
      // knew this conversation was gone, so its result must not paint the row back.
      nextThreadsGen();
      setThreads((prev) => prev.filter((t) => t.id !== id));
      setThreadsError(null);
      await loadThreads();
    },
    [activeThreadIdRef, resetSurface, loadThreads, nextThreadsGen],
  );

  return {
    threads,
    threadsError,
    setThreadsError,
    knownThreadIds,
    loadThreads,
    reconcileThreads,
    deleteThread,
  };
}
