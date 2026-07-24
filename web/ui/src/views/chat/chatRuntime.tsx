/*
 * The custom assistant-ui runtime adapter for the Chat tab (S-200, CR-051,
 * FR-UI-18, FR-UI-19, FR-UI-20, FR-UI-24, ADR-45).
 *
 * assistant-ui (`@assistant-ui/react`) owns the *surface* — thread, composer,
 * message list, markdown/code rendering, copy/stop/regenerate affordances — but
 * the *transport* is unchanged: this `useExternalStoreRuntime` adapter drives
 * assistant-ui from the EXISTING intent-guarded SSE client (`chatClient.ts`) and
 * the EXISTING pure per-turn reducer (`chatModel.ts`). The orchestrator, budget
 * tree, and per-thread memory backend are untouched ([ADR-45] is presentation
 * only); the SSE turn contract (`plan`/`step_started`/`step_observed`/
 * `answer_delta`/`final_answer`/`halted`/`error`) is consumed verbatim.
 *
 * Why an external store (not a local runtime): we keep our own message array so
 * each assistant turn carries its orchestrator side-channel (plan, subagent
 * chips, honest halt, honest error) as `metadata.custom.turn`, rendered by the
 * custom components in `ChatView.tsx`. The answer text is mirrored into the
 * message's text content so assistant-ui's Copy affordance copies it.
 *
 * Regenerate (FR-UI-20): the backend's only write verbs are append-turn and
 * per-thread delete (S-209/S-211) — there is no "replace the last turn" primitive,
 * so the SPA cannot re-address an individual persisted message. Regenerate is
 * therefore a presentation-level REPLACE over the conversation model this surface
 * owns: the prior assistant turn is dropped and the user message is re-run, never
 * appending a duplicate assistant turn (no orphaned memory in the conversation
 * model). See [ADR-45] for the full rationale.
 *
 * The masked chat key never reaches this layer (NFR-SE-07): the adapter only ever
 * sends the user message over the SSE `POST` — the key material is structurally
 * absent from every code path here.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  useExternalStoreRuntime,
  type AppendMessage,
  type ExternalStoreAdapter,
  type ThreadMessageLike,
} from "@assistant-ui/react";

import {
  deleteChatThread,
  fetchThreadMessages,
  fetchThreads,
  streamChatTurn,
} from "../../api/chatClient.ts";
import { ApiError } from "../../api/client.ts";
import {
  applyFrame,
  initialTurn,
  readSseStream,
  turnEndedEmpty,
  type PersistedChatMessage,
  type ThreadSummary,
  type TurnState,
} from "./chatModel.ts";

/** A conversation entry: a user turn, or an assistant turn folded from its SSE
 *  frames. `id` keys React rows and routes streamed updates; an assistant turn
 *  remembers its `parentId` (the user message it answers) so regenerate can
 *  re-run exactly that message. */
export type ChatMessage =
  | { kind: "user"; id: number; text: string }
  | { kind: "assistant"; id: number; parentId: number; turn: TurnState };

/** The orchestrator side-channel carried on an assistant message's
 *  `metadata.custom`; the custom components in `ChatView.tsx` read it back. */
export interface TurnCustom {
  turn: TurnState;
}

/** Map a folded turn to an assistant-ui message status: an honest error is
 *  `incomplete`, a finalized or budget-halted turn is `complete`, and anything
 *  still in flight is `running`. */
function statusOf(turn: TurnState): ThreadMessageLike["status"] {
  if (turn.error) return { type: "incomplete", reason: "error" };
  if (turn.finalized || turn.halt) return { type: "complete", reason: "stop" };
  return { type: "running" };
}

/**
 * Convert one of our messages into the assistant-ui wire shape. The assistant
 * answer is mirrored into a text content part (so Copy copies it) while the full
 * folded turn rides on `metadata.custom.turn` for the custom render path.
 */
export function convertMessage(message: ChatMessage): ThreadMessageLike {
  if (message.kind === "user") {
    return {
      role: "user",
      id: String(message.id),
      content: [{ type: "text", text: message.text }],
    };
  }
  return {
    role: "assistant",
    id: String(message.id),
    content: [{ type: "text", text: message.turn.answer }],
    status: statusOf(message.turn),
    metadata: { custom: { turn: message.turn } satisfies TurnCustom },
  };
}

/** Extract the plain text from an appended composer message (text parts only). */
function appendText(message: AppendMessage): string {
  return message.content
    .map((part) => (part.type === "text" ? part.text : ""))
    .join("")
    .trim();
}

/**
 * Fold a restored server transcript (`GET /api/v1/chat/threads/{id}`) into the
 * conversation model this surface owns (S-210, [FR-UI-26], [ADR-47]).
 *
 * The store persists only the DURABLE messages — the user text and the final
 * assistant answer — while the plan / subagent-activity side-channel is ephemeral
 * SSE (never written to `chat_messages`). So a restored assistant turn renders
 * answer-only and `finalized` (its Copy/Regenerate actions still work); the
 * internal `system`/`tool` rows are not part of the surface and are skipped. Local
 * ids are re-allocated via `nextId` so restored turns share one monotonic id space
 * with subsequently-sent turns — never colliding with server rowids.
 */
export function foldPersistedMessages(
  persisted: PersistedChatMessage[],
  nextId: () => number,
): ChatMessage[] {
  const out: ChatMessage[] = [];
  let lastUserId = 0;
  for (const m of persisted) {
    if (m.role === "user") {
      const id = nextId();
      lastUserId = id;
      out.push({ kind: "user", id, text: m.content });
    } else if (m.role === "assistant") {
      out.push({
        kind: "assistant",
        id: nextId(),
        parentId: lastUserId,
        turn: { ...initialTurn(), answer: m.content, finalized: true },
      });
    }
    // `system` / `tool` rows are internal — not part of the rendered surface.
  }
  return out;
}

/** The localStorage key remembering the open conversation so the selection is
 *  restored across a `serve --ui` restart / reload (S-210 AC-1). */
export const ACTIVE_THREAD_KEY = "logos.chat.activeThread";

/** The last-open conversation id remembered from a prior session, or `null` when
 *  none / storage is blocked. Read once to seed the runtime so the persistence
 *  effect never wipes it before the restore runs (fail SAFE to a fresh composer). */
function readStoredThreadId(): number | null {
  try {
    const raw = window.localStorage.getItem(ACTIVE_THREAD_KEY);
    const id = raw != null && raw !== "" ? Number(raw) : Number.NaN;
    return Number.isFinite(id) ? id : null;
  } catch {
    return null;
  }
}

/** The shape returned to `ChatView`: the assistant-ui runtime plus the multi-thread
 *  rail state and actions (S-210/S-211 — not an assistant-ui concern). */
export interface ChatRuntime {
  runtime: ReturnType<typeof useExternalStoreRuntime>;
  /** The conversation list, most-recent-first (S-209 `GET /chat/threads`). */
  threads: ThreadSummary[];
  /** The open conversation's id, or `null` for an unsent "+ New chat". */
  activeThreadId: number | null;
  /** Restore a conversation's full history into the surface (select-to-restore). */
  selectThread: (id: number) => Promise<void>;
  /** Reset the composer to a fresh, not-yet-persisted conversation (no empty rows). */
  newChat: () => void;
  /** Delete ONE conversation and its per-thread memory (S-211; the caller confirms
   *  first — this issues the guarded write). */
  deleteThread: (id: number) => Promise<void>;
  /** An honest note when the thread list could not be read or a delete failed,
   *  else `null`. */
  threadsError: string | null;
}

/**
 * Build the assistant-ui runtime over the SSE client. `consented` gates the first
 * outbound call (NFR-SE-07): until the user accepts the consent disclosure the
 * composer is disabled (`isDisabled`) and `onNew` is a hard no-op (defense in
 * depth).
 */
export function useChatRuntime(consented: boolean): ChatRuntime {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [threads, setThreads] = useState<ThreadSummary[]>([]);
  // Seeded from the prior session's selection so the persistence effect writes it
  // back (never wipes it) on mount; the restore effect then hydrates its history.
  const [activeThreadId, setActiveThreadId] = useState<number | null>(readStoredThreadId);
  const [threadsError, setThreadsError] = useState<string | null>(null);

  const idRef = useRef(0);
  const abortRef = useRef<AbortController | null>(null);
  // Latest snapshots read by callbacks without re-binding them every render.
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  const consentRef = useRef(consented);
  consentRef.current = consented;
  // The open conversation, read inside async callbacks (a turn sends it as the
  // `thread` field; adoption after a first send checks it) without re-binding them.
  const activeThreadIdRef = useRef(activeThreadId);
  activeThreadIdRef.current = activeThreadId;
  // Latest rail list, read at send time to diff the just-created thread.
  const threadsRef = useRef(threads);
  threadsRef.current = threads;
  // A monotonic "conversation session" generation, bumped whenever the open
  // conversation changes out from under an in-flight turn ("+ New chat" or a
  // select). A turn's trailing reconcile checks its captured session is still
  // current before adopting — so a superseded turn never rebinds the fresh one.
  const sessionRef = useRef(0);
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

  // A turn's lifetime is tied to the mounted view: leaving the tab / unmounting
  // aborts the in-flight turn, so the server cancels it ([FR-UI-19]). Without this
  // the streamed `fetch` would outlive the view.
  useEffect(() => () => abortRef.current?.abort(), []);

  // Persist the open conversation so the selection is restored across a
  // `serve --ui` restart / reload (S-210 AC-1). Best-effort — storage-blocked is
  // non-fatal (the conversation itself is durable server-side).
  useEffect(() => {
    try {
      if (activeThreadId == null) window.localStorage.removeItem(ACTIVE_THREAD_KEY);
      else window.localStorage.setItem(ACTIVE_THREAD_KEY, String(activeThreadId));
    } catch {
      /* non-fatal: the list still restores the conversation on the next load */
    }
  }, [activeThreadId]);

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

  // Route a streamed update to one assistant turn (functional set — no stale
  // closure as frames arrive).
  const updateTurn = useCallback((turnId: number, fn: (t: TurnState) => TurnState) => {
    setMessages((prev) =>
      prev.map((m) => (m.kind === "assistant" && m.id === turnId ? { ...m, turn: fn(m.turn) } : m)),
    );
  }, []);

  // Stream one turn into the assistant message `turnId`. Honest by construction:
  // a non-ok start, a fault, or a cleanly-closed-but-empty turn each surface a
  // message rather than a fabricated answer ([NFR-CC-04]); the `error` SSE frame
  // (the honest provider cause chain from [S-199]/[FR-UI-24]) is rendered
  // verbatim by the reducer.
  // Returns the turn's controller so the caller can tell whether this turn is still
  // the current one before clearing `isRunning` / reconciling (a superseded turn —
  // regenerate / new-chat / select — must not flip the composer state of the turn
  // that replaced it). Clearing `isRunning` and the trailing reconcile are the
  // caller's job, so the composer stays disabled until the conversation is fully
  // settled (a first send's new thread is adopted before Send re-enables).
  const runTurn = useCallback(
    async (turnId: number, question: string): Promise<AbortController> => {
      setIsRunning(true);
      const controller = new AbortController();
      abortRef.current = controller;
      try {
        const resp = await streamChatTurn(question, activeThreadIdRef.current, controller.signal);
        if (!resp.ok || !resp.body) {
          updateTurn(turnId, (t) => ({
            ...t,
            error: `The chat turn could not start (status ${resp.status}).`,
          }));
          return controller;
        }
        await readSseStream(resp.body, (frame) => updateTurn(turnId, (t) => applyFrame(t, frame)));
        updateTurn(turnId, (t) =>
          turnEndedEmpty(t)
            ? { ...t, error: "The turn ended without an answer — the connection may have closed early." }
            : t,
        );
      } catch (e) {
        // An aborted turn (stop / unmount / regenerate) is not a fault to surface.
        if (controller.signal.aborted) return controller;
        const message = e instanceof Error ? e.message : String(e);
        updateTurn(turnId, (t) => ({ ...t, error: `The chat turn failed: ${message}` }));
      }
      return controller;
    },
    [updateTurn],
  );

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
        if (created) setActiveThreadId(created.id);
      }
    },
    [nextThreadsGen],
  );

  // Append a fresh user turn + its assistant placeholder, stream the answer, then
  // reconcile the rail — keeping the turn "settling" (composer disabled) until a
  // first send's new thread is adopted, so a second send can never fork a duplicate
  // thread against a still-null active id.
  const startTurn = useCallback(
    (question: string) => {
      const userId = ++idRef.current;
      const turnId = ++idRef.current;
      // A send with no active conversation creates a new server thread; capture the
      // pre-send id set (to identify it) and this conversation's session (to skip the
      // reconcile if the user moves on — "+ New chat" / select — while it streams).
      const createdNewThread = activeThreadIdRef.current == null;
      const knownIds: ReadonlySet<number> | null = createdNewThread
        ? new Set(threadsRef.current.map((t) => t.id))
        : null;
      const session = ++sessionRef.current;
      setMessages((prev) => [
        ...prev,
        { kind: "user", id: userId, text: question },
        { kind: "assistant", id: turnId, parentId: userId, turn: initialTurn() },
      ]);
      void (async () => {
        const controller = await runTurn(turnId, question);
        // Still this conversation and still the current turn? Reconcile (and adopt).
        // Otherwise the user superseded it — leave the fresh session untouched.
        if (sessionRef.current === session && abortRef.current === controller) {
          await reconcileThreads(createdNewThread, knownIds);
        }
        if (abortRef.current === controller) setIsRunning(false);
      })();
    },
    [runTurn, reconcileThreads],
  );

  const onNew = useCallback(
    async (message: AppendMessage) => {
      if (!consentRef.current) return; // consent gate (NFR-SE-07)
      const text = appendText(message);
      if (text) startTurn(text);
    },
    [startTurn],
  );

  // Regenerate (FR-UI-20): replace the last (or the named) assistant turn rather
  // than appending a duplicate. `parentId` is the user message to re-run; absent
  // a match, fall back to the last user message.
  const onReload = useCallback(
    async (parentId: string | null) => {
      if (!consentRef.current) return;
      abortRef.current?.abort(); // supersede any in-flight turn
      const cur = messagesRef.current;
      let userIdx = -1;
      if (parentId != null) {
        userIdx = cur.findIndex((m) => m.kind === "user" && String(m.id) === parentId);
      }
      if (userIdx === -1) {
        for (let i = cur.length - 1; i >= 0; i--) {
          if (cur[i].kind === "user") {
            userIdx = i;
            break;
          }
        }
      }
      if (userIdx === -1) return;
      const userMsg = cur[userIdx] as Extract<ChatMessage, { kind: "user" }>;
      const turnId = ++idRef.current;
      // Drop everything after the user message (its prior assistant turn) and
      // re-run — a replace, never an append (no orphaned conversation memory).
      setMessages([
        ...cur.slice(0, userIdx + 1),
        { kind: "assistant", id: turnId, parentId: userMsg.id, turn: initialTurn() },
      ]);
      // Regenerate stays in the SAME conversation (never a new thread); capture the
      // session so a concurrent "+ New chat"/select suppresses the trailing refresh.
      const session = sessionRef.current;
      const controller = await runTurn(turnId, userMsg.text);
      if (sessionRef.current === session && abortRef.current === controller) {
        await reconcileThreads(false, null);
      }
      if (abortRef.current === controller) setIsRunning(false);
    },
    [runTurn, reconcileThreads],
  );

  // "+ New chat" (S-210 AC-2): reset the composer to a fresh, not-yet-persisted
  // conversation. No thread is created until the first send (the server only
  // creates a thread on a `thread`-less turn), so the rail grows no empty row.
  // Bumping the session supersedes any in-flight turn's trailing reconcile, so a
  // streaming turn can never rebind this fresh conversation back to its thread.
  const newChat = useCallback(() => {
    sessionRef.current += 1;
    abortRef.current?.abort();
    setMessages([]);
    setActiveThreadId(null);
    setIsRunning(false);
  }, []);

  // Select-to-restore (S-210 AC-1): hydrate a conversation's full transcript into
  // the surface and mark it active. A deleted/unknown id is an honest `404` — fall
  // back to a fresh composer rather than a broken view; a transport fault leaves
  // the current view intact with an honest note.
  const selectThread = useCallback(
    async (id: number) => {
      // Moving to another conversation supersedes any in-flight turn's reconcile.
      sessionRef.current += 1;
      abortRef.current?.abort();
      setIsRunning(false);
      try {
        const persisted = await fetchThreadMessages(id);
        setMessages(foldPersistedMessages(persisted, () => ++idRef.current));
        setActiveThreadId(id);
        setThreadsError(null);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) {
          newChat();
          void loadThreads(); // the list is stale — drop the vanished row
          return;
        }
        setThreadsError(
          `Could not open that conversation: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    },
    [newChat, loadThreads],
  );

  // On mount: load the rail, then restore the last-open conversation across a
  // `serve --ui` restart (S-210 AC-1) from the seeded selection. A stored id that
  // no longer exists resolves to a fresh composer via `selectThread`'s 404 path.
  useEffect(() => {
    const seeded = activeThreadIdRef.current; // the prior session's selection, or null
    void (async () => {
      await loadThreads();
      if (seeded != null) await selectThread(seeded);
    })();
    // Mount-only: the callbacks are stable and re-running would refetch on every
    // render. Restore is a one-shot bootstrap off the seeded id.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Stop (FR-UI-19): abort the in-flight turn (the server cancels it, the
  // existing client-disconnect cancellation now user-triggered) and mark the turn
  // ended so its Copy/Regenerate actions appear over what streamed.
  const onCancel = useCallback(async () => {
    abortRef.current?.abort();
    setMessages((prev) =>
      prev.map((m, i) =>
        i === prev.length - 1 && m.kind === "assistant"
          ? { ...m, turn: { ...m.turn, streaming: false, finalized: true } }
          : m,
      ),
    );
    setIsRunning(false);
  }, []);

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
      if (activeThreadIdRef.current === id) newChat();
      // Drop the row on the delete's own authority, then re-read for the
      // authoritative order — so the rail is right even if that refresh faults.
      // Take a ticket FIRST: any list read already in flight was issued before we
      // knew this conversation was gone, so its result must not paint the row back.
      nextThreadsGen();
      setThreads((prev) => prev.filter((t) => t.id !== id));
      setThreadsError(null);
      await loadThreads();
    },
    [newChat, loadThreads, nextThreadsGen],
  );

  const adapter: ExternalStoreAdapter<ChatMessage> = {
    messages,
    isRunning,
    isDisabled: !consented,
    convertMessage,
    onNew,
    onReload,
    onCancel,
    unstable_capabilities: { copy: true },
  };

  return {
    runtime: useExternalStoreRuntime(adapter),
    threads,
    activeThreadId,
    selectThread,
    newChat,
    deleteThread,
    threadsError,
  };
}
