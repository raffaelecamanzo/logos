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
 * Regenerate (FR-UI-20): the backend exposes only append-turn and clear-all — no
 * "replace the last turn" primitive, and the SSE stream returns no thread id, so
 * the SPA cannot re-address a server thread. Regenerate is therefore a
 * presentation-level REPLACE over the conversation model this surface owns: the
 * prior assistant turn is dropped and the user message is re-run, never appending
 * a duplicate assistant turn (no orphaned memory in the conversation model). See
 * [ADR-45] for the full rationale.
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
  clearChatHistory,
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

/** The shape returned to `ChatView`: the assistant-ui runtime, the multi-thread
 *  rail state (S-210), and the Clear-history control (neither an assistant-ui
 *  concern). */
export interface ChatRuntime {
  runtime: ReturnType<typeof useExternalStoreRuntime>;
  clearHistory: () => Promise<void>;
  clearing: boolean;
  clearMessage: string;
  /** The conversation list, most-recent-first (S-209 `GET /chat/threads`). */
  threads: ThreadSummary[];
  /** The open conversation's id, or `null` for an unsent "+ New chat". */
  activeThreadId: number | null;
  /** Restore a conversation's full history into the surface (select-to-restore). */
  selectThread: (id: number) => Promise<void>;
  /** Reset the composer to a fresh, not-yet-persisted conversation (no empty rows). */
  newChat: () => void;
  /** An honest note when the thread list could not be read, else `null`. */
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
  const [clearing, setClearing] = useState(false);
  const [clearMessage, setClearMessage] = useState("");
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
    try {
      setThreads(await fetchThreads());
      setThreadsError(null);
    } catch (e) {
      setThreadsError(
        `Could not load your conversations: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }, []);

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
  const runTurn = useCallback(
    async (turnId: number, question: string) => {
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
          return;
        }
        await readSseStream(resp.body, (frame) => updateTurn(turnId, (t) => applyFrame(t, frame)));
        updateTurn(turnId, (t) =>
          turnEndedEmpty(t)
            ? { ...t, error: "The turn ended without an answer — the connection may have closed early." }
            : t,
        );
      } catch (e) {
        // An aborted turn (stop / unmount / regenerate) is not a fault to surface.
        if (controller.signal.aborted) return;
        const message = e instanceof Error ? e.message : String(e);
        updateTurn(turnId, (t) => ({ ...t, error: `The chat turn failed: ${message}` }));
      } finally {
        setIsRunning(false);
      }
    },
    [updateTurn],
  );

  // After a turn settles, refresh the rail (updated_at re-orders the list, the
  // first send auto-titles the new thread) and, if this was a fresh "+ New chat"
  // (no active id), adopt the just-persisted conversation. The server creates the
  // thread on a `thread`-less first send but returns no id on the SSE stream
  // (S-209), so the newly-written thread — the most-recent `updated_at` — is the
  // top of the refreshed most-recent-first list (S-210 first-send persistence).
  const adoptAfterTurn = useCallback(async () => {
    let list: ThreadSummary[];
    try {
      list = await fetchThreads();
    } catch {
      return; // the turn itself succeeded; leave the rail as-is rather than error
    }
    setThreads(list);
    setThreadsError(null);
    if (activeThreadIdRef.current == null && list.length > 0) {
      setActiveThreadId(list[0].id);
    }
  }, []);

  // Append a fresh user turn + its assistant placeholder, then stream the answer;
  // reconcile the rail once the turn settles.
  const startTurn = useCallback(
    (question: string) => {
      const userId = ++idRef.current;
      const turnId = ++idRef.current;
      setMessages((prev) => [
        ...prev,
        { kind: "user", id: userId, text: question },
        { kind: "assistant", id: turnId, parentId: userId, turn: initialTurn() },
      ]);
      void runTurn(turnId, question).then(adoptAfterTurn);
    },
    [runTurn, adoptAfterTurn],
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
      await runTurn(turnId, userMsg.text);
      await adoptAfterTurn();
    },
    [runTurn, adoptAfterTurn],
  );

  // "+ New chat" (S-210 AC-2): reset the composer to a fresh, not-yet-persisted
  // conversation. No thread is created until the first send (the server only
  // creates a thread on a `thread`-less turn), so the rail grows no empty row.
  const newChat = useCallback(() => {
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
      abortRef.current?.abort();
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

  const clearHistory = useCallback(async () => {
    setClearing(true);
    setClearMessage("Clearing…");
    try {
      const resp = await clearChatHistory();
      if (!resp.ok) {
        setClearMessage(`Could not clear history (status ${resp.status}).`);
        return;
      }
      abortRef.current?.abort();
      setMessages([]);
      setClearMessage("History cleared.");
    } catch (e) {
      setClearMessage(`Could not clear history: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setClearing(false);
    }
  }, []);

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
    clearHistory,
    clearing,
    clearMessage,
    threads,
    activeThreadId,
    selectThread,
    newChat,
    threadsError,
  };
}
