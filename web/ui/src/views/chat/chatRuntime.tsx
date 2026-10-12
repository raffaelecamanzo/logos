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
 * `answer_delta`/`final_answer`/`halted`/`error`) is consumed verbatim, and the
 * additive `thread_title` (S-623, FR-UI-48) goes to the rail, not the turn.
 *
 * Why an external store (not a local runtime): we keep our own message array so
 * each assistant turn carries its orchestrator side-channel (plan, subagent
 * chips, honest halt, honest error) as `metadata.custom.turn`, rendered by the
 * custom components in `ChatView.tsx`. The answer text is mirrored into the
 * message's text content so assistant-ui's Copy affordance copies it.
 *
 * Regenerate and Edit (S-624, FR-UI-46, FR-UI-20): both RE-RUN a question — the
 * same one, or the reader's edited text — after cutting the conversation back to
 * before it IN THE STORE (`truncateChatThread`): the question, every later message
 * and the per-thread memory written after the cut go in one transaction, so a
 * restored conversation holds one answer per turn, never a superseded one. (Until
 * S-624 Regenerate was a presentation-level replace [ADR-45] recorded: the store kept
 * the old answer, so a reload showed both.) There is no branching. The re-run is an
 * ordinary send through `runTurn`, so it carries the conversation's preset (S-625).
 *
 * The masked chat key never reaches this layer (NFR-SE-07): the adapter only ever
 * sends the user message over the SSE `POST` — the key material is structurally
 * absent from every code path here.
 *
 * How the hook is composed: `useChatRuntime` accumulated three separable concerns
 * across S-200/S-210/S-211, so each now owns its own state and the top-level hook is
 * the composition of them —
 *
 * - `useActiveThread` — the OPEN conversation's id and its `localStorage` mirror;
 * - `useConversationPreset` — the open conversation's prompt-only preset (S-625),
 *   remembered per conversation and scope;
 * - `useTurnStream` — the message array and one streamed turn's lifecycle;
 * - `useThreadRail` (`./useThreadRail.ts`) — the conversation rail: the list, its
 *   read sequencing, first-send adoption, per-conversation delete and rename, and the
 *   Synthesizer's title for a new conversation;
 *
 * leaving `useChatRuntime` with only what genuinely spans them: turn dispatch (send,
 * regenerate, edit, select-to-restore), the mount bootstrap, and the assistant-ui adapter.
 */

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import {
  useExternalStoreRuntime,
  type AppendMessage,
  type ExternalStoreAdapter,
  type ThreadMessageLike,
} from "@assistant-ui/react";

import {
  fetchThreadMessages,
  streamChatTurn,
  truncateChatThread,
  type ChatRoutes,
} from "../../api/chatClient.ts";
import { ApiError } from "../../api/client.ts";
import { createLivenessWatchdog } from "../../api/sse.ts";
import {
  applyFrame,
  chatStorageKey,
  initialTurn,
  readSseStream,
  STALL_THRESHOLD_SECONDS,
  threadTitleOf,
  turnEndedEmpty,
  type ChatStorageScope,
  type PersistedChatMessage,
  type ThreadSummary,
  type TurnState,
} from "./chatModel.ts";
import {
  DEFAULT_PRESET,
  loadStoredPresets,
  readStoredPresets,
  storePresets,
  withPreset,
  type ChatPresetId,
} from "./presets.ts";
import { useThreadRail } from "./useThreadRail.ts";

/** A conversation entry: a user turn, or an assistant turn folded from its SSE
 *  frames. `id` keys React rows and routes streamed updates; an assistant turn
 *  remembers its `parentId` (the user message it answers) so regenerate can
 *  re-run exactly that message. `stalled` is set while the turn's stream has been
 *  silent past the threshold (S-626) — a transport observation, not a frame, so it
 *  lives beside the folded turn rather than in the reducer's state. */
export type ChatMessage =
  /** `unsent`: the server refused the turn before storing its question (a non-ok
   *  start), so the store holds no row for it — a later re-run's cut does not count
   *  it among the stored questions ({@link storedUserTurn}). */
  | { kind: "user"; id: number; text: string; unsent?: boolean }
  | { kind: "assistant"; id: number; parentId: number; turn: TurnState; stalled?: boolean };

/** The orchestrator side-channel carried on an assistant message's
 *  `metadata.custom`; the custom components in `ChatView.tsx` read it back. */
export interface TurnCustom {
  turn: TurnState;
  /** The turn's stream has been silent past {@link STALL_THRESHOLD_SECONDS}. */
  stalled: boolean;
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
    metadata: { custom: { turn: message.turn, stalled: message.stalled === true } satisfies TurnCustom },
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

/** The localStorage key BASE remembering the open conversation so the selection is
 *  restored across a `serve --ui` restart / reload (S-210 AC-1) — qualified by the
 *  chat's storage scope (S-485, {@link chatStorageKey}), because a thread id names a
 *  row in ONE chat's store and the same number in another is a different
 *  conversation. */
export const ACTIVE_THREAD_KEY = "logos.chat.activeThread";

/** The last-open conversation id remembered under `scope` by a prior session, or
 *  `null` when none / storage is blocked. Read once to seed the runtime so the
 *  persistence effect never wipes it before the restore runs (fail SAFE to a fresh
 *  composer). */
function readStoredThreadId(scope: ChatStorageScope): number | null {
  try {
    const raw = window.localStorage.getItem(chatStorageKey(ACTIVE_THREAD_KEY, scope));
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
  /** Rename ONE conversation (S-623, FR-UI-48; the rail refuses an empty name
   *  first — this issues the guarded write). */
  renameThread: (id: number, title: string) => Promise<void>;
  /** An honest note when the thread list could not be read or a delete or rename failed,
   *  else `null`. */
  threadsError: string | null;
  /** The open conversation's prompt-only preset (S-625, FR-UI-49) — what every turn
   *  sent in it runs under. */
  preset: ChatPresetId;
  /** Choose the open conversation's preset (Default clears it). */
  choosePreset: (preset: ChatPresetId) => void;
}

/** The open conversation plus the ref async callbacks read it through (S-210 AC-1). */
interface ActiveThread {
  activeThreadId: number | null;
  /** The open conversation, read inside async callbacks without re-binding them. */
  activeThreadIdRef: MutableRefObject<number | null>;
  setActiveThreadId: (id: number | null) => void;
}

/**
 * Own the OPEN conversation's id and its `localStorage` mirror (S-210 AC-1).
 *
 * Seeded from the prior session's selection so the persistence effect writes it back
 * (never wipes it) on mount; `useChatRuntime`'s restore effect then hydrates its
 * history.
 */
function useActiveThread(scope: ChatStorageScope): ActiveThread {
  const [activeThreadId, setActiveThreadId] = useState<number | null>(() =>
    readStoredThreadId(scope),
  );
  const activeThreadIdRef = useRef(activeThreadId);
  activeThreadIdRef.current = activeThreadId;

  // Persist the open conversation so the selection is restored across a
  // `serve --ui` restart / reload (S-210 AC-1). Best-effort — storage-blocked is
  // non-fatal (the conversation itself is durable server-side).
  useEffect(() => {
    const key = chatStorageKey(ACTIVE_THREAD_KEY, scope);
    try {
      if (activeThreadId == null) window.localStorage.removeItem(key);
      else window.localStorage.setItem(key, String(activeThreadId));
    } catch {
      /* non-fatal: the list still restores the conversation on the next load */
    }
  }, [activeThreadId, scope]);

  return { activeThreadId, activeThreadIdRef, setActiveThreadId };
}

/** The open conversation's preset, the ref a turn reads it through at send time, and
 *  the three moves that change it. */
interface ConversationPreset {
  preset: ChatPresetId;
  /** The preset a turn sends, read by `runTurn` without re-binding it. */
  presetRef: MutableRefObject<ChatPresetId>;
  choosePreset: (preset: ChatPresetId) => void;
  /** Bind an unsent conversation's choice to the thread its first send created. */
  adoptDraft: (threadId: number) => void;
  /** Return an unsent conversation to Default ("+ New chat"). */
  resetDraft: () => void;
  /** Forget a deleted conversation's preset: the store reuses the id of a deleted
   *  newest conversation, and a new thread is not always adopted (a failed rail
   *  refresh, or the reader moving on mid-send). */
  forget: (threadId: number) => void;
}

/**
 * Own the open conversation's prompt-only preset (S-625, [FR-UI-49]).
 *
 * The choice belongs to a CONVERSATION, so it is remembered per thread id under the
 * chat's storage scope ({@link readStoredPresets}) and read back whenever that
 * conversation is open — after a reload too — while every other conversation keeps its
 * own. An unsent "+ New chat" has no id yet: its choice is held as a draft, which the
 * first send's adoption binds to the thread the server created ({@link adoptDraft}).
 *
 * The preset a turn sends is read through a ref at send time, so every send path that
 * goes through `runTurn` — the composer, Retry / Regenerate, an edit, a suggestion chip — runs
 * under the open conversation's preset without passing it along.
 */
function useConversationPreset(
  scope: ChatStorageScope,
  { activeThreadId, activeThreadIdRef }: ActiveThread,
): ConversationPreset {
  const [draft, setDraft] = useState<ChatPresetId>(DEFAULT_PRESET);
  const [byThread, setByThread] = useState(() => readStoredPresets(scope));
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const byThreadRef = useRef(byThread);
  byThreadRef.current = byThread;
  const preset =
    activeThreadId == null ? draft : (byThread.get(activeThreadId) ?? DEFAULT_PRESET);
  const presetRef = useRef(preset);
  presetRef.current = preset;

  // A pick changes its own entry over what is stored NOW, not over this tab's mount-
  // time snapshot — another tab of the same chat may have steered a conversation since.
  // Only when storage cannot be read does the session's own map stand in.
  const remember = useCallback(
    (threadId: number, chosen: ChatPresetId) => {
      const current = loadStoredPresets(scope) ?? byThreadRef.current;
      const next = withPreset(current, threadId, chosen);
      byThreadRef.current = next;
      setByThread(next);
      storePresets(scope, next);
    },
    [scope],
  );

  const setDraftNow = useCallback((chosen: ChatPresetId) => {
    draftRef.current = chosen;
    setDraft(chosen);
  }, []);

  const choosePreset = useCallback(
    (chosen: ChatPresetId) => {
      // A send in the same tick as the pick (before the re-render) sends the pick.
      presetRef.current = chosen;
      const threadId = activeThreadIdRef.current;
      if (threadId == null) setDraftNow(chosen);
      else remember(threadId, chosen);
    },
    [activeThreadIdRef, remember, setDraftNow],
  );

  // The new thread takes the draft OVER whatever its id held: the store reuses the id
  // of a deleted newest conversation, whose preset must not carry over (a Default
  // draft clears the entry).
  const adoptDraft = useCallback(
    (threadId: number) => {
      remember(threadId, draftRef.current);
      setDraftNow(DEFAULT_PRESET);
    },
    [remember, setDraftNow],
  );

  const resetDraft = useCallback(() => setDraftNow(DEFAULT_PRESET), [setDraftNow]);

  const forget = useCallback(
    (threadId: number) => remember(threadId, DEFAULT_PRESET),
    [remember],
  );

  return { preset, presetRef, choosePreset, adoptDraft, resetDraft, forget };
}

/** The conversation surface's own state: the message array and one turn's SSE
 *  lifecycle. The refs are part of the contract — `useChatRuntime` composes turn
 *  dispatch (send / regenerate / select) over them, and each guards a
 *  supersede-check that a plain state read could not (a stale closure would see the
 *  wrong turn). */
interface TurnStream {
  messages: ChatMessage[];
  isRunning: boolean;
  setMessages: Dispatch<SetStateAction<ChatMessage[]>>;
  setIsRunning: (running: boolean) => void;
  /** Latest message snapshot, read by callbacks without re-binding them. */
  messagesRef: MutableRefObject<ChatMessage[]>;
  /** The monotonic local id allocator shared by sent and restored turns. */
  idRef: MutableRefObject<number>;
  /** The in-flight turn's controller — the identity a trailing reconcile checks. */
  abortRef: MutableRefObject<AbortController | null>;
  /** The "conversation session" generation a superseded turn compares against. */
  sessionRef: MutableRefObject<number>;
  /** Mark the question an assistant turn answers as not held by the store. */
  markUnsent: (turnId: number) => void;
  runTurn: (turnId: number, question: string) => Promise<AbortController>;
  newChat: () => void;
  onCancel: () => Promise<void>;
}

/**
 * Own the conversation surface: the message array, the streamed turn's lifecycle,
 * and the two resets that end one (Stop, "+ New chat").
 *
 * The open conversation arrives from [`useActiveThread`]: a turn sends that id as
 * its `thread` field, and "+ New chat" resets BOTH halves — the transcript and the
 * open-conversation id — in one action.
 */
function useTurnStream(
  routes: ChatRoutes,
  { activeThreadIdRef, setActiveThreadId }: ActiveThread,
  presetRef: MutableRefObject<ChatPresetId>,
  onThreadTitle: (threadId: number, title: string) => void,
): TurnStream {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [isRunning, setIsRunning] = useState(false);

  const idRef = useRef(0);
  const abortRef = useRef<AbortController | null>(null);
  // Latest snapshot read by callbacks without re-binding them every render.
  const messagesRef = useRef(messages);
  messagesRef.current = messages;
  // A monotonic "conversation session" generation, bumped whenever the open
  // conversation changes out from under an in-flight turn ("+ New chat" or a
  // select). A turn's trailing reconcile checks its captured session is still
  // current before adopting — so a superseded turn never rebinds the fresh one.
  const sessionRef = useRef(0);

  // A turn's lifetime is tied to the mounted view: leaving the tab / unmounting
  // aborts the in-flight turn, so the server cancels it ([FR-UI-19]). Without this
  // the streamed `fetch` would outlive the view.
  useEffect(() => () => abortRef.current?.abort(), []);

  // Route a streamed update to one assistant turn (functional set — no stale
  // closure as frames arrive).
  const updateTurn = useCallback((turnId: number, fn: (t: TurnState) => TurnState) => {
    setMessages((prev) =>
      prev.map((m) => (m.kind === "assistant" && m.id === turnId ? { ...m, turn: fn(m.turn) } : m)),
    );
  }, []);

  // Mark the question an assistant turn answers as never stored (S-624).
  const markUnsent = useCallback((turnId: number) => {
    setMessages((prev) => {
      const turn = prev.find((m) => m.kind === "assistant" && m.id === turnId);
      if (turn?.kind !== "assistant") return prev;
      return prev.map((m) => (m.kind === "user" && m.id === turn.parentId ? { ...m, unsent: true } : m));
    });
  }, []);

  // Raise or clear the stalled-stream notice on one assistant turn (S-626).
  const setStalled = useCallback((turnId: number, stalled: boolean) => {
    setMessages((prev) =>
      prev.map((m) => (m.kind === "assistant" && m.id === turnId ? { ...m, stalled } : m)),
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
  //
  // Liveness (S-626, [FR-UI-50]): a watchdog armed before the request counts ANY
  // received byte — the response head, an event, or the server's `: keepalive`
  // comment, which parses to no frame and so reaches only the reader's `onBytes` —
  // and raises the stalled notice after STALL_THRESHOLD_SECONDS of silence. It is
  // disarmed when the turn ends however it ends — including the instant it is
  // aborted (Stop, Retry, "+ New chat", a select, unmount), not only once the
  // aborted read unwinds — and a raised notice goes with it.
  const runTurn = useCallback(
    async (turnId: number, question: string): Promise<AbortController> => {
      setIsRunning(true);
      const controller = new AbortController();
      abortRef.current = controller;
      let stalled = false;
      const watchdog = createLivenessWatchdog(STALL_THRESHOLD_SECONDS * 1000, (now) => {
        stalled = now;
        setStalled(turnId, now);
      });
      const disarm = () => {
        watchdog.stop();
        if (stalled) {
          stalled = false;
          setStalled(turnId, false);
        }
      };
      controller.signal.addEventListener("abort", disarm, { once: true });
      try {
        const resp = await streamChatTurn(
          routes,
          question,
          activeThreadIdRef.current,
          presetRef.current,
          controller.signal,
        );
        watchdog.touch();
        // A refused turn (the server answers 4xx before its setup runs) stored no
        // question: mark it, so a later cut counts only the questions the store holds.
        if (!resp.ok) markUnsent(turnId);
        if (!resp.ok || !resp.body) {
          updateTurn(turnId, (t) => ({
            ...t,
            error: `The chat turn could not start (status ${resp.status}).`,
          }));
          return controller;
        }
        await readSseStream(
          resp.body,
          (frame) => {
            // The conversation's title (S-623) belongs to the rail, not this turn —
            // and only while the turn is still live: an aborted turn's server run
            // was cancelled before it could store the title.
            const named = threadTitleOf(frame);
            if (named) {
              if (!controller.signal.aborted) onThreadTitle(named.threadId, named.title);
              return;
            }
            updateTurn(turnId, (t) => applyFrame(t, frame));
          },
          watchdog.touch,
        );
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
      } finally {
        disarm();
      }
      return controller;
    },
    [routes, updateTurn, markUnsent, setStalled, activeThreadIdRef, presetRef, onThreadTitle],
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
  }, [setActiveThreadId]);

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

  return {
    messages,
    isRunning,
    setMessages,
    setIsRunning,
    messagesRef,
    idRef,
    abortRef,
    sessionRef,
    markUnsent,
    runTurn,
    newChat,
    onCancel,
  };
}


/** The index of the user message a regenerate re-runs: the named `parentId` while it
 *  is still present, else the LAST user message; `-1` when the conversation holds no
 *  user message at all (nothing to re-run). */
export function findRerunIndex(messages: readonly ChatMessage[], parentId: string | null): number {
  if (parentId != null) {
    const named = messages.findIndex((m) => m.kind === "user" && String(m.id) === parentId);
    if (named !== -1) return named;
  }
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].kind === "user") return i;
  }
  return -1;
}

/** The question a re-run of the user message at `index` cuts the store back to: the
 *  number of STORED questions before it — every earlier user message except one the
 *  server refused before storing ({@link ChatMessage} `unsent`). The truncate route's
 *  `user_turn` (S-624). */
export function storedUserTurn(messages: readonly ChatMessage[], index: number): number {
  let stored = 0;
  for (const m of messages.slice(0, index)) if (m.kind === "user" && !m.unsent) stored++;
  return stored;
}

/** Cut conversation `threadId` back to before its `userTurn`-th stored question
 *  (S-624). `null` when the cut landed; otherwise the honest note shown on the turn
 *  the reader tried to re-run, which is not re-run — re-running over an uncut store
 *  would persist the superseded answer beside the new one ([NFR-CC-04]). */
async function cutBefore(routes: ChatRoutes, threadId: number, userTurn: number): Promise<string | null> {
  let resp: Response;
  try {
    resp = await truncateChatThread(routes, threadId, userTurn);
  } catch (e) {
    return `The turn was not re-run: the conversation could not be cut back (${e instanceof Error ? e.message : String(e)}).`;
  }
  if (resp.ok) return null;
  return resp.status === 404
    ? "The turn was not re-run: this conversation no longer exists."
    : `The turn was not re-run: the conversation could not be cut back (status ${resp.status}).`;
}

/**
 * Build the assistant-ui runtime over the SSE client. `consented` gates the first
 * outbound call (NFR-SE-07): until the user accepts the consent disclosure the
 * composer is disabled (`isDisabled`) and `onNew` is a hard no-op (defense in
 * depth).
 *
 * This is the COMPOSITION layer: the open conversation ([`useActiveThread`]), the
 * message surface ([`useTurnStream`]), and the conversation rail
 * ([`useThreadRail`](./useThreadRail.ts)) each own their own state, and the turn
 * dispatch below is what spans them — a send/regenerate streams into the surface and
 * then reconciles the rail, a select hydrates the surface from the rail's row.
 *
 * `routes` names the chat it drives (the member's or the workspace's, S-485) and
 * `scope` the storage scope its open conversation is remembered under — both
 * required, so neither chat can fall back onto the other's store or state.
 */
export function useChatRuntime(
  consented: boolean,
  routes: ChatRoutes,
  scope: ChatStorageScope,
): ChatRuntime {
  const active = useActiveThread(scope);
  const { activeThreadId, activeThreadIdRef, setActiveThreadId } = active;
  // The stream hands a `thread_title` to the rail, which is built after it (the rail
  // resets through the stream's `newChat`): a ref closes the loop with a stable
  // callback.
  const applyTitleRef = useRef<(threadId: number, title: string) => void>(() => {});
  const onThreadTitle = useCallback(
    (threadId: number, title: string) => applyTitleRef.current(threadId, title),
    [],
  );
  const presets = useConversationPreset(scope, active);
  const { adoptDraft, resetDraft, forget: forgetPreset } = presets;
  const stream = useTurnStream(routes, active, presets.presetRef, onThreadTitle);
  const { messagesRef, idRef, abortRef, sessionRef } = stream;
  const { setMessages, setIsRunning, markUnsent, runTurn } = stream;
  // "+ New chat" starts a fresh conversation at Default: the old draft was the last
  // unsent conversation's.
  const resetStream = stream.newChat;
  const newChat = useCallback(() => {
    resetStream();
    resetDraft();
  }, [resetStream, resetDraft]);
  // A first send's new thread inherits the preset it was sent under.
  const adoptThread = useCallback(
    (id: number) => {
      adoptDraft(id);
      setActiveThreadId(id);
    },
    [adoptDraft, setActiveThreadId],
  );
  const rail = useThreadRail({
    routes,
    activeThreadIdRef,
    adoptThread,
    resetSurface: newChat,
    onDeleted: forgetPreset,
  });
  const { knownThreadIds, loadThreads, reconcileThreads, setThreadsError } = rail;
  applyTitleRef.current = rail.applyTitle;

  const consentRef = useRef(consented);
  consentRef.current = consented;

  // The controller of a re-run's cut while it is in flight (S-624), so Stop can
  // tell a cut from a streaming turn.
  const cutRef = useRef<AbortController | null>(null);

  // Stop (FR-UI-19) and Esc (S-620). A streaming turn stops at once. A cut is only
  // abandoned: the re-run it leads to is skipped, but the composer stays held until
  // the cut settles, and `rerun` then releases it — a question sent while the cut is
  // still in flight could otherwise be stored first and then cut with it.
  const { onCancel: stopTurn } = stream;
  const onCancel = useCallback(async () => {
    const cutting = cutRef.current;
    if (cutting !== null && abortRef.current === cutting) {
      cutting.abort();
      return;
    }
    await stopTurn();
  }, [abortRef, stopTurn]);

  // The trailing half of a dispatched turn, shared by send and regenerate: reconcile
  // the rail only while this turn is STILL the current one in the SAME conversation —
  // otherwise the user superseded it (regenerate / "+ New chat" / select) and the
  // fresh conversation must be left untouched — then settle the composer. Both
  // dispatch paths go through this one implementation, so the supersede protocol
  // cannot drift between them.
  const settleTurn = useCallback(
    async (session: number, controller: AbortController, reconcile: () => Promise<void>) => {
      if (sessionRef.current === session && abortRef.current === controller) await reconcile();
      if (abortRef.current === controller) setIsRunning(false);
    },
    [sessionRef, abortRef, setIsRunning],
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
        ? knownThreadIds()
        : null;
      const session = ++sessionRef.current;
      setMessages((prev) => [
        ...prev,
        { kind: "user", id: userId, text: question },
        { kind: "assistant", id: turnId, parentId: userId, turn: initialTurn() },
      ]);
      void (async () => {
        const controller = await runTurn(turnId, question);
        await settleTurn(session, controller, () => reconcileThreads(createdNewThread, knownIds));
      })();
    },
    [
      idRef,
      activeThreadIdRef,
      sessionRef,
      knownThreadIds,
      setMessages,
      runTurn,
      reconcileThreads,
      settleTurn,
    ],
  );

  const onNew = useCallback(
    async (message: AppendMessage) => {
      if (!consentRef.current) return; // consent gate (NFR-SE-07)
      const text = appendText(message);
      if (text) startTurn(text);
    },
    [startTurn],
  );

  // Re-run the question at `userIdx` — Regenerate asks it again as it stands, an
  // edit asks `text` in its place (S-624, [FR-UI-46]). Everything after the question
  // is dropped from the surface, and — in a conversation the store already holds —
  // from the store first: the question, the later messages and the memory written
  // after the cut, so a reload shows this answer alone ([FR-UI-20]). The question is
  // then sent down `runTurn`, the path every send takes, so it runs under the
  // conversation's preset (S-625).
  //
  // Whatever happens to the cut, the surface keeps counting only the questions the
  // store holds — `storedUserTurn` names every later cut from it. A cut that fails
  // left the store whole, so the surface goes back to what it was, with the note on
  // the turn the reader tried to re-run, and nothing is re-run. A cut that lands
  // after the reader stopped the re-run removed this question from the store, so it
  // is marked `unsent`. Both only while the surface is still this re-run's: a move to
  // another conversation, or another re-run, owns it then.
  const rerun = useCallback(
    async (userIdx: number, text: string) => {
      abortRef.current?.abort(); // supersede any in-flight turn
      const cur = messagesRef.current;
      const prior = cur[userIdx] as Extract<ChatMessage, { kind: "user" }>;
      const userTurn = storedUserTurn(cur, userIdx);
      // The same id keeps the row; the re-run is a fresh attempt, so not `unsent`.
      const question: ChatMessage = { kind: "user", id: prior.id, text };
      const turnId = ++idRef.current;
      setMessages([
        ...cur.slice(0, userIdx),
        question,
        { kind: "assistant", id: turnId, parentId: question.id, turn: initialTurn() },
      ]);
      // A re-run stays in the SAME conversation (never a new thread); capture the
      // session so a concurrent "+ New chat"/select suppresses the trailing refresh.
      // The one exception is a Retry of a first send that never settled (a stalled
      // turn, S-626): no conversation was adopted yet, so there is nothing to cut, the
      // re-run carries no thread and the server creates one — adopted exactly as a
      // first send's is, or the next send would fork yet another thread.
      const threadId = activeThreadIdRef.current;
      const createdNewThread = threadId == null;
      const knownIds = createdNewThread ? knownThreadIds() : null;
      const session = sessionRef.current;
      if (threadId != null) {
        // The cut is part of the turn: it holds the composer as the turn will, and
        // Stop, "+ New chat", a select or another re-run supersede it like one.
        // Stop holds the composer until the cut settles (see `onCancel` below): the
        // server resolves `user_turn` only when the cut runs, so a question sent
        // while it is still in flight could be stored first and then cut with it.
        const cutting = new AbortController();
        abortRef.current = cutting;
        cutRef.current = cutting;
        setIsRunning(true);
        const failure = await cutBefore(routes, threadId, userTurn);
        if (cutRef.current === cutting) cutRef.current = null;
        const ours = abortRef.current === cutting && sessionRef.current === session;
        if (failure) {
          if (ours) {
            const answered = cur[userIdx + 1];
            if (answered?.kind === "assistant") {
              setMessages(
                cur.map((m) => (m === answered ? { ...answered, turn: { ...answered.turn, error: failure } } : m)),
              );
            } else {
              setMessages(cur);
              setThreadsError(failure);
            }
            setIsRunning(false);
          }
          return;
        }
        if (cutting.signal.aborted) {
          if (ours) {
            markUnsent(turnId);
            setMessages((prev) =>
              prev.map((m) =>
                m.kind === "assistant" && m.id === turnId
                  ? { ...m, turn: { ...m.turn, streaming: false, finalized: true } }
                  : m,
              ),
            );
            setIsRunning(false);
          }
          return;
        }
      }
      const controller = await runTurn(turnId, text);
      await settleTurn(session, controller, () => reconcileThreads(createdNewThread, knownIds));
    },
    [
      routes,
      messagesRef,
      idRef,
      sessionRef,
      abortRef,
      activeThreadIdRef,
      knownThreadIds,
      setMessages,
      setIsRunning,
      setThreadsError,
      markUnsent,
      runTurn,
      reconcileThreads,
      settleTurn,
    ],
  );

  // Regenerate (FR-UI-20): re-run the named assistant turn's question — `parentId`
  // is the user message to re-run; absent a match, the last user message.
  const onReload = useCallback(
    async (parentId: string | null) => {
      if (!consentRef.current) return;
      const cur = messagesRef.current;
      const userIdx = findRerunIndex(cur, parentId);
      if (userIdx === -1) return;
      await rerun(userIdx, (cur[userIdx] as Extract<ChatMessage, { kind: "user" }>).text);
    },
    [messagesRef, rerun],
  );

  // Edit and resend (S-624, [FR-UI-46]): assistant-ui's edit composer hands back the
  // edited message, `sourceId` naming the user message it replaces. An unchanged
  // edit never arrives (the composer just closes); an empty one re-runs nothing.
  const onEdit = useCallback(
    async (message: AppendMessage) => {
      if (!consentRef.current) return;
      const text = appendText(message);
      if (!text) return;
      const userIdx = messagesRef.current.findIndex(
        (m) => m.kind === "user" && String(m.id) === message.sourceId,
      );
      if (userIdx !== -1) await rerun(userIdx, text);
    },
    [messagesRef, rerun],
  );

  // Select-to-restore (S-210 AC-1): hydrate a conversation's full transcript into
  // the surface and mark it active. A deleted/unknown id is an honest `404` — fall
  // back to a fresh composer rather than a broken view; a transport fault leaves
  // the current view intact with an honest note.
  // Whether a turn is streaming, read by `selectThread` without re-binding it.
  const runningRef = useRef(stream.isRunning);
  runningRef.current = stream.isRunning;

  const selectThread = useCallback(
    async (id: number) => {
      // Picking the conversation that is already open while its turn streams is not
      // a move: it must not abort that turn. A double-click on the open row (the
      // rail's rename gesture, S-623) delivers two clicks first.
      if (id === activeThreadIdRef.current && runningRef.current) return;
      // Moving to another conversation supersedes any in-flight turn's reconcile.
      sessionRef.current += 1;
      abortRef.current?.abort();
      setIsRunning(false);
      try {
        const persisted = await fetchThreadMessages(routes, id);
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
    [
      routes,
      idRef,
      sessionRef,
      abortRef,
      activeThreadIdRef,
      setMessages,
      setIsRunning,
      setActiveThreadId,
      setThreadsError,
      newChat,
      loadThreads,
    ],
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

  const adapter: ExternalStoreAdapter<ChatMessage> = {
    messages: stream.messages,
    isRunning: stream.isRunning,
    isDisabled: !consented,
    convertMessage,
    onNew,
    onEdit,
    onReload,
    onCancel,
    unstable_capabilities: { copy: true },
  };

  return {
    runtime: useExternalStoreRuntime(adapter),
    threads: rail.threads,
    activeThreadId,
    selectThread,
    newChat,
    deleteThread: rail.deleteThread,
    renameThread: rail.renameThread,
    threadsError: rail.threadsError,
    preset: presets.preset,
    choosePreset: presets.choosePreset,
  };
}
