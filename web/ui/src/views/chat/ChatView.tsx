/*
 * ChatView (S-200, S-300, S-308, CR-051, CR-089, CR-092, FR-UI-18, FR-UI-19,
 * FR-UI-20, FR-UI-24, FR-UI-31, FR-UI-33, ADR-45) — the
 * Chat tab rebuilt on **assistant-ui** (`@assistant-ui/react`) over a custom
 * runtime adapter on the UNCHANGED intent-guarded SSE stream (`chatRuntime.tsx`).
 *
 * assistant-ui provides the thread, the composer, message rendering, and the
 * copy / stop / regenerate affordances; this file supplies the Logos-specific
 * surface around it — the configure-first state, the first-use consent gate
 * (NFR-SE-07), the conversation-history rail with its confirm-gated per-conversation
 * delete (S-210/S-211, FR-UI-26, FR-UI-20 — there is no global clear-all), and the
 * CUSTOM assistant-turn components that re-implement the bespoke surface over
 * assistant-ui's primitives:
 *   - the "Activity" disclosure — the planner's plan and every subagent step,
 *   - the honest budget-halt notice and the honest provider error ([FR-UI-24],
 *     rendered verbatim from the SSE `error` frame), never a fabricated answer
 *     ([NFR-CC-04]),
 *   - the answer as streamed markdown with code blocks (`MarkdownAnswer.tsx`).
 *
 * S-300 ([FR-UI-31]) flattened the TURN to the base assistant-ui grammar — the
 * assistant turn a flat full-width left-aligned block (no card fill, no red top
 * rule, no shadow), the user turn a right-aligned bubble. S-308 ([FR-UI-33],
 * CR-092) then re-rooted the VIEW in the application's own two-pane card grammar,
 * adopted from the Wiki tab: the transcript and composer live inside ONE shared
 * `Card` beside the history rail, filling its track, and S-300's centred reading
 * measure is retired. The card wraps the transcript, never a turn. S-301
 * (same FR) then folded the separate plan list and the subagent-activity pills
 * into ONE native "Activity" disclosure whose steps carry their full observed
 * result as rendered markdown, replacing the native hover tooltip. S-309 (same
 * CR) then built the third §4.13 lead-callout state — the `StatusBand`, this
 * view's leading verdict element (frontend-design §9): once configured and
 * consented, it names the provider, endpoint host and model plus the turn's
 * budget-tree bounds, which used to live only in the empty-thread hint and
 * vanish after the first message. All of the above are presentation changes
 * only; the SSE contract, the runtime adapter, and the orchestrator are
 * untouched.
 *
 * Everything renders through the S-193 design tokens (`Chat.module.css`); no
 * inline `<style>`/`<script>`, no CSS-in-JS, so the byte-identical self-only CSP
 * holds ([NFR-SE-06]). The masked chat key never reaches this surface (NFR-SE-07):
 * the configured body receives only the EFFECTIVE `[chat]` policy and the two
 * origins, and the runtime adapter only ever sends the user message.
 *
 * S-619 (CR-204 L2/L3/L6/C8, [FR-UI-45]) then made the card fill the view's height
 * with the transcript as its ONLY scroller, turned the composer into one pill pinned
 * to the card's bottom edge (a leading "+" slot, a trailing round Send / Stop), took
 * the rail out of its card behind a collapse toggle remembered per scope, and folded
 * the post-consent status band into a one-line chip that expands to it.
 *
 * S-620 (CR-204 L4/C1/C2/C10, [FR-UI-45], [FR-UI-46]) then gave the turns their last
 * layer: the user bubble a 24px radius, the action bar two 24px icon buttons (Copy
 * shows a check for 1.5 s), a floating ↓ control that appears when the reader has
 * scrolled away and resumes follow-the-stream, and four keyboard shortcuts — Enter,
 * Shift+Enter (the composer), Esc (stop) and ⌘K / Ctrl+K (new chat) — named in a
 * muted hint under the composer.
 *
 * S-621 (CR-204 C1, [FR-UI-46]) then replaced the empty thread's one-line hint with a
 * welcome — a greeting and 2–4 suggestion chips filled from the read-models
 * (`Welcome.tsx`, `suggestionChips.ts`); a chip sends through the same `append` path
 * the composer uses.
 *
 * Whether chat is usable is not decided here. `chatReadiness` (`chatModel.ts`,
 * S-452) reads it off the read-model's effective-chat origins — the resolution the
 * turn path dials — and this file renders the verdict.
 */

import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent, ReactNode } from "react";
import {
  ActionBarPrimitive,
  AssistantRuntimeProvider,
  ComposerPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  useAui,
  useAuiState,
  useMessage,
  useThreadViewport,
} from "@assistant-ui/react";

import {
  fetchChatConfig,
  MEMBER_CHAT_ROUTES,
  WORKSPACE_CHAT_ROUTES,
} from "../../api/chatClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import { Button, Callout, Widget, WidgetStack } from "../../components/index.ts";
import {
  IconArrowDown,
  IconArrowUp,
  IconChevronDown,
  IconPanelLeft,
  IconPencil,
  IconPlus,
  IconRefresh,
  IconStop,
} from "../../components/icons.tsx";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { CopyControl } from "./CopyControl.tsx";
import { MarkdownAnswer } from "./MarkdownAnswer.tsx";
import { ThreadList } from "./ThreadList.tsx";
import { Welcome } from "./Welcome.tsx";
import { useChatRuntime } from "./chatRuntime.tsx";
import { CHAT_PRESETS, DEFAULT_PRESET, presetOf, type ChatPresetId } from "./presets.ts";
import { useRailDisclosure } from "./useThreadRail.ts";
import {
  chatReadiness,
  chatScope,
  configureFirstCopy,
  consentEntries,
  endpointHost,
  hasConsent,
  memberChatStorageScope,
  memberReadRootsDisclosure,
  modelLabel,
  rememberConsent,
  roleLabel,
  STALL_NOTICE,
  turnEndedEmpty,
  workspaceConfigRepairHref,
  WORKSPACE_CONFIG_FILE,
  WORKSPACE_SECRETS_FILE,
  type ChatConfigReadModel,
  type ChatReady,
  type ChatStorageScope,
  type ConfigureFirst as ConfigureFirstState,
  type ReadRootsDisclosure,
  type TurnState,
} from "./chatModel.ts";
import styles from "./Chat.module.css";

export function ChatView() {
  const config = useApiResource<ChatConfigReadModel>(() => fetchChatConfig(), []);
  const { mode, member } = useWorkspace();
  const scope = chatScope(mode, member);
  const storageScope = memberChatStorageScope(scope);
  const repairHref = workspaceConfigRepairHref(scope);
  return (
    <div className={styles.view}>
      <AsyncResource
        resource={config}
        loadingLabel="Loading chat…"
        errorNote={repairHref && <ConfigReadRepairNote href={repairHref} />}
      >
        {(model) => {
          // The verdict is the pure function's (S-452); this view only renders it.
          const verdict = chatReadiness(model, scope);
          // Only the effective policy crosses into the configured body — the
          // credential is presence-only and never reaches it (NFR-SE-07).
          // Keyed by the storage scope, so a scope change re-seeds the remembered
          // thread and consent even where the shell's own member key does not
          // remount this view (S-485).
          return verdict.ready ? (
            <ChatConfigured
              key={storageScope}
              ready={verdict}
              surface="member"
              storageScope={storageScope}
              disclosure={memberReadRootsDisclosure(verdict)}
            />
          ) : (
            <ConfigureFirst state={verdict} />
          );
        }}
      </AsyncResource>
    </div>
  );
}

/**
 * Workspace mode only (HF-2, Sprint 77 review option 4i): appended to the
 * shared error panel (`AsyncResource`, `describeReadError`) when the config read
 * this tab and the member Config tab both make fails. The fault could be this
 * member's own `config.toml`/`secrets.toml`, or the inherited workspace-tier
 * file at the workspace root — the 500 body cannot reliably say which
 * (`workspaceConfigRepairHref`), so this names both rather than guess wrong.
 * Renders no file content (NFR-SE-07): only the two workspace-root paths by
 * name, exactly as the configure-first state already does.
 */
function ConfigReadRepairNote({ href }: { href: string }) {
  return (
    <p>
      This can be an invalid file at this member&apos;s own root, or at the workspace root (
      <code>{WORKSPACE_CONFIG_FILE}</code> or <code>{WORKSPACE_SECRETS_FILE}</code>) — every member
      inheriting from a broken workspace-root file fails the same read. The{" "}
      <a href={href}>workspace Config</a> view says whether either workspace-root file is at fault
      and repairs an invalid <code>{WORKSPACE_CONFIG_FILE}</code> in place; an unreadable{" "}
      <code>{WORKSPACE_SECRETS_FILE}</code> is repaired by hand.
    </p>
  );
}

/**
 * The honest configure-first state ([FR-UI-18], [NFR-CC-04]): a muted advisory —
 * NOT an error, and no composer — that names the root it inspected, the absent
 * half, where any present half came from, and — under an inherited policy — why a
 * member key is not used with the workspace endpoint (HF-1), and links the
 * member's Config tab.
 *
 * In workspace mode the workspace-root files that would declare the absent half
 * for every member are named, and the app-level workspace Config view whose chat
 * group writes them (S-451) is linked beside them.
 */
function ConfigureFirst({ state }: { state: ConfigureFirstState }) {
  const copy = configureFirstCopy(state);
  return (
    <Callout label="CONFIGURE" tone="muted">
      <p>{copy.summary}</p>
      {copy.present && <p>{copy.present}</p>}
      {copy.memberKeyNote && <p>{copy.memberKeyNote}</p>}
      <p>
        {copy.action} in the <a href={state.configHref}>Config</a> tab{copy.actionScope}, then
        return here to start chatting. Until then no outbound call is possible.
      </p>
      {copy.workspaceLead && (
        <p>
          {copy.workspaceLead}{" "}
          {state.workspaceFiles.map((file, i) => (
            <span key={file}>
              {i > 0 && " and "}
              <code>{file}</code>
            </span>
          ))}
          .
          {state.workspaceConfigHref && (
            <>
              {" "}
              The <a href={state.workspaceConfigHref}>workspace Config</a> view edits{" "}
              {state.workspaceFiles.length === 1 ? "that file" : "both files"}.
            </>
          )}
        </p>
      )}
    </Callout>
  );
}

/** Which chat a configured surface serves (S-485, [FR-WS-34], [ADR-71]): the
 *  member chat, or the workspace chat. It picks the routes the surface talks to and
 *  the words its disclosures use; nothing else about the surface differs. */
export type ChatSurfaceKind = "member" | "workspace";

export interface ChatConfiguredProps {
  ready: ChatReady;
  surface: ChatSurfaceKind;
  /** The scope the remembered thread and the consent are keyed by. */
  storageScope: ChatStorageScope;
  /** The extra read roots the consent banner and status band name. */
  disclosure: ReadRootsDisclosure;
  /** What an empty history rail says, when the chat has more to say than the
   *  member chat's default. */
  emptyRailNote?: ReactNode;
}

/** The configured chat surface: the lead callout (the consent banner, then the
 *  S-309 status band), the conversation-history rail (S-210/S-211), and the
 *  assistant-ui thread inside the view's `Card` (S-308,
 *  [FR-UI-33]). There is no global Clear-history — deletion is per conversation,
 *  in the rail (S-211, [FR-UI-26], [ADR-47]).
 *
 *  Shared by the member chat and the Workspace Chat (S-485, frontend-design
 *  §4.22): the component set is reused whole, with the chat it serves as a
 *  parameter. */
export function ChatConfigured({
  ready,
  surface,
  storageScope,
  disclosure,
  emptyRailNote,
}: ChatConfiguredProps) {
  const routes = surface === "workspace" ? WORKSPACE_CHAT_ROUTES : MEMBER_CHAT_ROUTES;
  // Consent covers the disclosed read roots too (HF-1): a new or changed set
  // shows the banner again, naming them before anything is sent. It is this
  // chat's own consent (S-485): another scope's acceptance does not carry over.
  const entries = consentEntries(disclosure);
  const [consented, setConsented] = useState<boolean>(() => hasConsent(storageScope, entries));
  // The rail's collapse toggle (S-619, [FR-UI-45]): shown at every width. Beside a
  // desktop conversation the reader's choice is remembered per scope (open until
  // collapsed); above a narrow one the rail starts collapsed and a pick closes it,
  // as before (S-210 AC-3).
  const rail = useRailDisclosure(storageScope);
  const { closeIfStacked } = rail;
  const {
    runtime,
    threads,
    activeThreadId,
    selectThread,
    newChat,
    deleteThread,
    renameThread,
    threadsError,
    preset,
    choosePreset,
  } = useChatRuntime(consented, routes, storageScope);

  // The entries are re-derived each render; their joined form is the stable dep.
  const entriesKey = JSON.stringify(entries);
  const acceptConsent = useCallback(() => {
    rememberConsent(storageScope, JSON.parse(entriesKey) as string[]);
    setConsented(true);
  }, [storageScope, entriesKey]);

  // Deleting keeps the rail OPEN (unlike select / new chat): the user is managing
  // the list and usually deletes more than one row.
  const onDelete = useCallback((id: number) => void deleteThread(id), [deleteThread]);

  // Selecting or starting a conversation closes the narrow-viewport rail so the
  // restored thread is in view (a no-op at ≥1024px, where the rail sits beside it).
  const onSelect = useCallback(
    (id: number) => {
      void selectThread(id);
      closeIfStacked();
    },
    [selectThread, closeIfStacked],
  );
  const onNewChat = useCallback(() => {
    newChat();
    closeIfStacked();
  }, [newChat, closeIfStacked]);

  // One stack (S-617, FR-UI-40): the lead callout, then the two panes.
  return (
    <WidgetStack className={styles.chat}>
      {/* The lead callout (frontend-design §4.13): one slot, full width above the
          two panes, whose state moves from the first-use consent gate to the
          persistent status band (S-309, [FR-UI-33]) once consented. Here rather
          than inside `.main`, it leads the view at every width — below ~1023px the
          panes stack and the rail toggle and list come before `.main`. */}
      {consented ? (
        <StatusChip ready={ready} disclosure={disclosure} />
      ) : (
        <ConsentBanner
          ready={ready}
          surface={surface}
          disclosure={disclosure}
          onAccept={acceptConsent}
        />
      )}

      <div className={rail.open ? styles.layout : `${styles.layout} ${styles.layoutCollapsed}`}>
        {/* The rail column: the collapse toggle over the rail itself. The toggle stays
            when the rail is hidden, so the column narrows to it and the conversation
            takes the freed width. */}
        <div className={styles.railColumn}>
          <button
            type="button"
            className={styles.railToggle}
            aria-expanded={rail.open}
            aria-controls="chat-rail"
            aria-label={rail.open ? "Hide conversations" : "Show conversations"}
            title={rail.open ? "Hide conversations" : "Show conversations"}
            onClick={rail.toggle}
          >
            <IconPanelLeft />
          </button>

          <aside id="chat-rail" className={styles.railPane} hidden={!rail.open}>
            <ThreadList
              threads={threads}
              activeThreadId={activeThreadId}
              onSelect={onSelect}
              onNewChat={onNewChat}
              onDelete={onDelete}
              onRename={renameThread}
              error={threadsError}
              emptyNote={emptyRailNote}
            />
          </aside>
        </div>

        <div className={styles.main}>
          {/* The view's ONE card (S-308, [FR-UI-33]): it wraps the whole transcript
              and the composer, never a turn — the turns inside stay flat, so the
              signal-red edge appears once per view, not once per answer. Since
              S-617 it is the `chatConversation` tool panel. Since S-619 it fills the
              view's height: the transcript is its `flex: 1` scroller — the page's
              only one — which assistant-ui drives to follow the stream, and the pill
              composer is pinned under it. */}
          <Widget panel="chatConversation" title="Conversation" className={styles.conversation}>
            <AssistantRuntimeProvider runtime={runtime}>
              <ThreadPrimitive.Root className={styles.threadRoot} {...{ [THREAD_ROOT_ATTR]: "" }}>
                <ChatShortcuts onNewChat={onNewChat} />
                <ThreadPrimitive.Viewport className={styles.log}>
                  <ThreadPrimitive.Empty>
                    <Welcome consented={consented} surface={surface} restoring={activeThreadId !== null} />
                  </ThreadPrimitive.Empty>
                  <ThreadPrimitive.Messages
                    components={{ UserMessage, UserEditComposer, AssistantMessage }}
                  />
                  <ScrollToBottom />
                </ThreadPrimitive.Viewport>
                <Composer consented={consented} preset={preset} onChoosePreset={choosePreset} />
              </ThreadPrimitive.Root>
            </AssistantRuntimeProvider>
          </Widget>
        </div>
      </div>
    </WidgetStack>
  );
}

/** The first-use consent disclosure (NFR-SE-07): names the EFFECTIVE endpoint and
 *  what is sent before any outbound call — and, when the policy or the key is
 *  inherited, says so, since the endpoint is then one this member's own
 *  `config.toml` never names. The composer is disabled until it is accepted.
 *
 *  The Workspace Chat's disclosure (S-485) names the workspace root as the
 *  endpoint's declarer and the members as the excerpts' source. */
function ConsentBanner({
  ready,
  surface,
  disclosure,
  onAccept,
}: {
  ready: ChatReady;
  surface: ChatSurfaceKind;
  disclosure: ReadRootsDisclosure;
  onAccept: () => void;
}) {
  const chat = ready.policy;
  return (
    <Callout label="BEFORE YOU START" tone="warm" className={styles.consent}>
      <p>
        Asking a question sends your message together with{" "}
        <strong>source and graph excerpts</strong> from{" "}
        {surface === "workspace" ? "this workspace's members" : "this project"} to{" "}
        <strong>{endpointHost(chat)}</strong>{" "}
        {surface === "workspace" ? (
          <>
            (the <code>{chat.provider}</code> endpoint declared at the workspace root)
          </>
        ) : ready.policyOrigin === "workspace" ? (
          <>
            (the <code>{chat.provider}</code> endpoint inherited from the workspace root)
          </>
        ) : (
          <>
            (the configured <code>{chat.provider}</code> endpoint)
          </>
        )}
        .{ready.credentialOrigin === "workspace" && " The API key is inherited from the workspace root."}{" "}
        Nothing is sent until you ask.
      </p>
      <ReadRootsNote disclosure={disclosure} />
      <p className={styles.providerLine}>
        {chat.provider} · {endpointHost(chat)} · {chat.model}
      </p>
      {/* Neutral, not the red primary: a control this size never fills with the
          signal red (CR-204 L7, FR-UI-45). */}
      <Button variant="secondary" onClick={onAccept}>
        Start chatting
      </Button>
    </Callout>
  );
}

/**
 * The persistent status band (S-309, [FR-UI-33], CR-092's third §4.13
 * lead-callout state): once configured and consented, names the provider, endpoint
 * host and model, plus the turn's budget-tree bounds, giving Chat the leading verdict
 * element every other view opens with (frontend-design §9, e.g. Health's `GATE ·
 * PASS`, Wiki's `WIKI · STALE`). It reads the same effective `[chat]` policy the
 * configured body already receives, so the masked key never reaches it
 * ([NFR-SE-07]); an undeclared model is named honestly rather than defaulted
 * ([NFR-CC-04], {@link modelLabel}). The bounds used to live only in the
 * empty-thread hint and vanish after the first message — they
 * persist here instead, moved rather than duplicated.
 *
 * S-619 (CR-204 C8, [FR-UI-45]) folds it into a ONE-LINE CHIP: the chip names the
 * provider, host, model and the bounds in short (`budget 24/8/3`), so the bounds stay
 * visible after the first message as [FR-UI-33] requires, and a click expands it to
 * the band's sentences — the bounds spelled out and any extra read roots. The chip
 * says when read roots are disclosed inside, so collapsing never hides that more
 * than the project is read. The consent banner before it is unchanged.
 */
function StatusChip({ ready, disclosure }: { ready: ChatReady; disclosure: ReadRootsDisclosure }) {
  const [open, setOpen] = useState(false);
  const bandId = useId();
  const chat = ready.policy;
  const hasReadRoots = disclosure.groups.length > 0 || disclosure.unreadable.length > 0;
  return (
    <section className={styles.status} aria-label="Chat status">
      <button
        type="button"
        className={styles.statusChip}
        aria-expanded={open}
        aria-controls={bandId}
        onClick={() => setOpen((was) => !was)}
      >
        <span className={styles.statusLabel}>CHAT</span>
        <span className={styles.statusLine}>
          {chat.provider} · {endpointHost(chat)} · {modelLabel(chat)} · budget {chat.max_tool_calls}/
          {chat.max_subagent_tool_calls}/{chat.max_replans}
          {hasReadRoots && " · extra read roots"}
        </span>
        <IconChevronDown className={styles.statusChevron} width={16} height={16} />
      </button>
      <StatusBand id={bandId} hidden={!open} ready={ready} disclosure={disclosure} />
    </section>
  );
}

/** The expanded chip: the band's sentences under the chip's one line. */
function StatusBand({
  id,
  hidden,
  ready,
  disclosure,
}: {
  id: string;
  hidden: boolean;
  ready: ChatReady;
  disclosure: ReadRootsDisclosure;
}) {
  const chat = ready.policy;
  return (
    <div id={id} className={styles.statusBand} hidden={hidden}>
      <p className={styles.budgetLine}>
        Budget tree: {chat.max_tool_calls} tool calls, {chat.max_subagent_tool_calls} per
        subagent, {chat.max_replans} replans.
      </p>
      <ReadRootsNote disclosure={disclosure} />
    </div>
  );
}

/** The extra read roots (`[chat] read_roots`, sprint-79 HF-1), named wherever the
 *  view says what is sent: files under them can be read through symlinks and
 *  quoted to the endpoint, so the consent banner and the status band both name
 *  them. Renders nothing when none are declared — the default. A root relative to
 *  another root than the chat's own says which (an inherited table's entries are
 *  relative to the workspace root that declared it, which this member's own
 *  `config.toml` never names); the Workspace Chat's groups also name the members
 *  that read through them, and any member whose roots could not be listed
 *  (S-485). */
function ReadRootsNote({ disclosure }: { disclosure: ReadRootsDisclosure }) {
  const { groups, unreadable, through } = disclosure;
  if (groups.length === 0 && unreadable.length === 0) return null;
  return (
    <>
      {groups.length > 0 && (
        <p className={styles.budgetLine}>
          Extra read roots:{" "}
          {groups.map((group, g) => (
            <span key={`${group.relativeTo ?? ""}-${g}`}>
              {g > 0 && "; "}
              {group.roots.map((root, i) => (
                <span key={root}>
                  {i > 0 && ", "}
                  <code>{root}</code>
                </span>
              ))}
              {readRootQualifier(group)}
            </span>
          ))}{" "}
          — files under them, reached through {through} symlinks, can be sent too.
        </p>
      )}
      {unreadable.length > 0 && (
        <p className={styles.budgetLine}>
          The chat configuration of {unreadable.join(", ")} could not be read, so{" "}
          {unreadable.length === 1 ? "its" : "their"} read roots are not listed here; a source
          call on {unreadable.length === 1 ? "that member" : "those members"} fails until it is
          repaired.
        </p>
      )}
    </>
  );
}

/** A group's parenthetical: the root its entries resolve against and who reads
 *  through them, whichever it has — or nothing (the chat's own roots). */
function readRootQualifier(group: ReadRootsDisclosure["groups"][number]): string {
  const parts = [
    ...(group.relativeTo !== null ? [`relative to ${group.relativeTo}`] : []),
    ...(group.readBy.length > 0 ? [`read by ${group.readBy.join(", ")}`] : []),
  ];
  return parts.length === 0 ? "" : ` (${parts.join("; ")})`;
}

/** A user turn: the message text in a bubble hugging the RIGHT edge of the
 *  transcript (S-300, [FR-UI-31]). The root spans the transcript's width so both
 *  roles share one alignment line inside the card (S-308, [FR-UI-33]); the bubble
 *  is the inner element, so it right-aligns within that line rather than against
 *  the viewport.
 *
 *  Under the bubble, its action bar (S-624, [FR-UI-46]): one 24px Edit icon, the
 *  answer bar's grammar, hidden while a turn runs. Edit swaps the turn for
 *  {@link UserEditComposer}. */
function UserMessage() {
  return (
    <MessagePrimitive.Root className={styles.user}>
      <div className={styles.userBubble}>
        <MessagePrimitive.Parts />
      </div>
      <ActionBarPrimitive.Root
        hideWhenRunning
        autohide="never"
        className={`${styles.actions} ${styles.userActions}`}
      >
        <ActionBarPrimitive.Edit className={styles.actionIcon} aria-label="Edit message" title="Edit message">
          <IconPencil />
        </ActionBarPrimitive.Edit>
      </ActionBarPrimitive.Root>
    </MessagePrimitive.Root>
  );
}

/**
 * A user turn opened for editing (S-624, [FR-UI-46]): its text in place, in a box
 * drawn like the composer pill, with Cancel and Resend. Resend (or Enter) hands the
 * edited text to the runtime's `onEdit`, which cuts the conversation back to before
 * this question — in the store, memory included — and re-runs it; an unchanged text
 * just closes the box. There is no branching.
 *
 * **Esc** cancels the edit and goes no further. It is handled HERE, the way the
 * rail's rename field handles it (S-623): the keystroke is claimed and stopped, so
 * the chat's document-level Esc ({@link ChatShortcuts}, which stops a running turn)
 * never sees it. The input's own document-level Esc (`cancelOnEscape`) is switched
 * off so there is one handler, not two. A key that confirms or cancels an IME
 * composition belongs to the IME.
 */
function UserEditComposer() {
  const aui = useAui();
  const onKeyDown = (e: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key !== "Escape" || e.nativeEvent.isComposing || e.keyCode === 229) return;
    e.preventDefault();
    e.stopPropagation();
    aui.composer().cancel();
  };
  return (
    <MessagePrimitive.Root className={styles.user}>
      <ComposerPrimitive.Root className={styles.userEdit}>
        <ComposerPrimitive.Input
          className={styles.input}
          aria-label="Edit your message"
          minRows={1}
          maxRows={8}
          submitMode="enter"
          cancelOnEscape={false}
          autoFocus
          onKeyDown={onKeyDown}
        />
        <div className={`${styles.actions} ${styles.userActions}`}>
          <ComposerPrimitive.Cancel className={styles.action}>Cancel</ComposerPrimitive.Cancel>
          <ComposerPrimitive.Send className={styles.action}>Resend</ComposerPrimitive.Send>
        </div>
      </ComposerPrimitive.Root>
    </MessagePrimitive.Root>
  );
}

/** An assistant turn: the Activity disclosure, the streamed markdown answer, an
 *  honest halt or error, and the copy/regenerate action bar. A full-width block
 *  flush with the LEFT edge of the same transcript the user turn sits in — not a
 *  card of its own; the view's one card wraps the transcript (S-300, [FR-UI-31];
 *  S-308, [FR-UI-33]). The folded turn rides on `metadata.custom.turn`; data is
 *  rendered as React-escaped text or through `react-markdown` (which never injects
 *  raw HTML). */
function AssistantMessage() {
  const turn = useMessage((m) => m.metadata.custom.turn as TurnState | undefined);
  const stalled = useMessage((m) => m.metadata.custom.stalled === true);
  if (!turn) return null;
  return (
    <MessagePrimitive.Root className={styles.assistant}>
      <ActivityDisclosure turn={turn} />
      <div className={styles.answer}>
        {/* The pulse claims progress, so it shows only while the turn is genuinely
            in flight and not silent: never on a stalled stream (the notice below
            replaces it) nor on a turn Stopped before any answer arrived. */}
        {!turn.answer && !turn.halt && !turn.error && !turn.finalized && !stalled && (
          <p className={styles.working} role="status">
            <span className={styles.workingPulse} aria-hidden="true" />
            Working…
          </p>
        )}
        {turn.answer && (
          <div className={turn.streaming ? `${styles.final} ${styles.streaming}` : styles.final}>
            <MarkdownAnswer text={turn.answer} />
          </div>
        )}
        {turn.halt && <p className={styles.halt}>{turn.halt}</p>}
        {turn.error && <p className={styles.error}>{turn.error}</p>}
        {stalled && <StalledNotice />}
      </div>
      <MessageActions answer={turn.answer} />
    </MessagePrimitive.Root>
  );
}

/**
 * The stalled-stream notice (S-626, [FR-UI-50]): an open turn's stream has carried
 * no byte — not even the server's 15 s keepalive comment — for 45 s. It replaces
 * the "Working…" pulse, which would otherwise claim progress nobody can see, and
 * names only what was observed: silence ([NFR-CC-04]). The runtime clears it when a
 * byte arrives or the turn ends.
 *
 * **Retry** re-runs this turn's question in place — the same replace-not-append
 * reload Regenerate uses ([FR-UI-20]), which first aborts the silent stream.
 * **Stop** ends the turn, as the composer's Stop does. Both go through the runtime
 * rather than the action-bar primitives, which assistant-ui disables while a turn is
 * running — and a stalled turn is still running.
 *
 * Styled from existing classes only: the halt note's warm rule (a possible loss,
 * not yet an error), the answer column's spacing, and the action-bar buttons.
 */
function StalledNotice() {
  const aui = useAui();
  return (
    <div role="alert" className={`${styles.halt} ${styles.answer}`}>
      <span>{STALL_NOTICE}</span>
      <div className={styles.actions}>
        <button type="button" className={styles.action} onClick={() => aui.message().reload()}>
          Retry
        </button>
        <button type="button" className={styles.action} onClick={() => aui.thread().cancelRun()}>
          Stop
        </button>
      </div>
    </div>
  );
}

/**
 * The turn's single "Activity" disclosure (S-301, [FR-UI-31]): the planner's plan
 * AND every subagent step — role, instruction, and the FULL observed result as
 * rendered markdown — inside one native `<details>` fold. It replaces the separate
 * plan list plus the pill-with-`title=`-hover-tooltip pair, so the grounding is
 * read in place rather than on mouse-over.
 *
 * The fold is CONTROLLED rather than left to the native toggle, so the open state
 * has one derivation instead of two competing sources of truth: it is open while
 * the turn is in flight, auto-collapses once the answer is `finalized`, and once
 * the user has toggled it their choice wins for the rest of the turn's life
 * (`userOpen`).
 *
 * It collapses onto an ANSWER, so a turn that produced none stays open — the
 * activity is then the only honest record of how far the turn got ([NFR-CC-04]).
 * That covers three states the bare `finalized` bit cannot tell apart: a halt and
 * an error never set `finalized` at all, but **Stop** does — `onCancel`
 * (`chatRuntime.tsx`) marks the turn finalized on abort whether or not anything
 * arrived, and collapsing the trail at the exact moment the user stopped to look
 * at it is the wrong answer. `turnEndedEmpty` is the reducer's existing name for
 * "closed without producing anything", the same predicate the surface already uses
 * to own up to an empty turn.
 *
 * A turn with neither a plan nor a step renders NOTHING: the plan/activity
 * side-channel is ephemeral SSE and is never persisted, so a restored answer-only
 * turn must not grow an empty fold. The gate counts plan STEPS rather than testing
 * for a plan object, because a malformed `plan` frame is reduced to a present
 * plan with zero steps (`applyFrame` guards a non-array `steps` into `[]`) — which
 * would otherwise open onto a caption and an empty list.
 *
 * This is presentation only — it reads the same `TurnState` the reducer already
 * folded; the SSE contract, the orchestrator, and the budget tree are untouched.
 */
function ActivityDisclosure({ turn }: { turn: TurnState }) {
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const open = userOpen ?? (!turn.finalized || turnEndedEmpty(turn));
  // Intercept the summary's activation so the native `open` flip can never race
  // the derived state. Enter/Space on a focused summary dispatch a click too, so
  // the disclosure stays keyboard-operable.
  const toggle = (event: MouseEvent<HTMLElement>) => {
    event.preventDefault();
    setUserOpen(!open);
  };
  // A click is not the only way `open` can flip: a find-in-page hit inside
  // collapsed content auto-expands a <details> with no click on the summary at all.
  // React does not re-assert an unchanged prop, so that drift would stick — and the
  // user's next click would be spent silently correcting it instead of closing the
  // fold. Adopt any externally-driven flip. React's OWN attribute writes also fire
  // `toggle`, which is why this is guarded: by then the attribute already equals
  // `open`, so the derived value is never mistaken for a user choice.
  const syncNativeToggle = (event: { currentTarget: HTMLDetailsElement }) => {
    if (event.currentTarget.open !== open) setUserOpen(event.currentTarget.open);
  };

  // Nothing renderable in ANY round, not merely in the latest plan: a replan to
  // zero steps must not hide an earlier round's plan (sprint-78 review).
  const rounds = activityRounds(turn);
  if (rounds.length === 0) return null;
  return (
    <details className={styles.activity} open={open} onToggle={syncNativeToggle}>
      <summary className={styles.activitySummary} onClick={toggle}>
        <span className={styles.activityLabel}>Activity</span>
        <span className={styles.activityMeta}>{activityMeta(turn)}</span>
      </summary>
      <div className={styles.activityBody}>
        {rounds.length > 1 ? (
          <ActivityByRound turn={turn} rounds={rounds} />
        ) : (
          <>
            <PlanList plan={turn.plans.find((p) => p.round === rounds[0]) ?? turn.plan} />
            <ActivitySteps chips={turn.chips} />
          </>
        )}
      </div>
    </details>
  );
}

/** Every round this turn has touched — every `plan` frame's round union every
 *  chip's round, ascending and deduplicated. Length 1 for a single-round turn
 *  (the common case, and the CR-089 baseline this fold must render identically
 *  to), so the grouped branch below is reached only on an actual replan. The
 *  flat branch renders the plan of `rounds[0]`, not the latest plan: after a
 *  replan to zero steps the one renderable round is the earlier one.
 *
 *  A round whose plan carries zero steps (a malformed `plan` frame guards to
 *  `steps: []`, same as `PlanList`'s own guard) and that started no chip
 *  contributes nothing renderable — excluded here rather than in `ActivityByRound`,
 *  so it never produces an empty group with only a stray `.activityRound` divider
 *  (review-fix, S-303). */
function activityRounds(turn: TurnState): number[] {
  const rounds = new Set<number>();
  for (const p of turn.plans) if (planStepCount(p) > 0) rounds.add(p.round);
  for (const c of turn.chips) rounds.add(c.round);
  return [...rounds].sort((a, b) => a - b);
}

/** A replanned turn's fold: each round's plan grouped with that round's OWN
 *  steps, so the plan-to-steps association stays unambiguous even though the
 *  orchestrator restarts `index` at 0 every round (S-303, CR-090). Only reached
 *  for a multi-round turn — a single round renders the flat, ungrouped layout
 *  above, unchanged from the CR-089 baseline. */
function ActivityByRound({ turn, rounds }: { turn: TurnState; rounds: number[] }) {
  return (
    <div className={styles.activityRounds}>
      {rounds.map((round) => (
        <div key={round} className={styles.activityRound}>
          <PlanList plan={turn.plans.find((p) => p.round === round) ?? null} />
          <ActivitySteps chips={turn.chips.filter((c) => c.round === round)} />
        </div>
      ))}
    </div>
  );
}

/** How many steps a plan actually carries — `0` for no plan AND for a malformed
 *  plan the reducer guarded into zero steps, which the fold treats alike. */
function planStepCount(plan: TurnState["plan"]): number {
  return plan?.steps.length ?? 0;
}

/** The collapsed fold's one-line census, so the user knows what is inside before
 *  opening it: how far the observed steps have got, or — before the first step
 *  starts — how many the planner intends. */
function activityMeta(turn: TurnState): string {
  const total = turn.chips.length;
  if (total === 0) {
    const planned = planStepCount(turn.plan);
    return planned === 1 ? "1 planned step" : `${planned} planned steps`;
  }
  const done = turn.chips.filter((c) => c.done).length;
  if (done < total) return `${done} of ${total} steps`;
  return total === 1 ? "1 step" : `${total} steps`;
}

/** The planner's plan, inside the fold (a replan supersedes the prior plan). A plan
 *  with no steps renders nothing rather than a caption over an empty list. */
function PlanList({ plan }: { plan: TurnState["plan"] }) {
  if (!plan || planStepCount(plan) === 0) return null;
  return (
    <div className={styles.plan}>
      <p className={styles.activityCaption}>{plan.round > 0 ? "Revised plan" : "Plan"}</p>
      <ol className={styles.planSteps}>
        {plan.steps.map((s, i) => (
          <li key={i} className={styles.planStep}>
            <span className={styles.activityRole}>{roleLabel(s.role)}</span> {s.instruction}
          </li>
        ))}
      </ol>
    </div>
  );
}

/** The subagent steps, in start order (running → done), each with its full observed
 *  result rendered as markdown through the same renderer the answer uses. A step
 *  that finished with nothing observed says so rather than leaving a blank
 *  ([NFR-CC-04]); the status glyph is `aria-hidden` and paired with a text
 *  equivalent, so the state never rides on colour alone. */
function ActivitySteps({ chips }: { chips: TurnState["chips"] }) {
  if (chips.length === 0) return null;
  return (
    <ol className={styles.activitySteps}>
      {chips.map((c) => (
        // Keyed by the (round, index) PAIR, never `index` alone: the orchestrator
        // restarts `index` at 0 every replan round, so on a replanned turn two
        // chips can share an index — a bare `c.index` key collides and React
        // warns / misrenders (S-303, CR-090).
        <li key={`${c.round}-${c.index}`} className={styles.activityStep}>
          <p className={styles.activityStepHead}>
            <span
              className={c.done ? styles.activityDone : styles.activityRunning}
              aria-hidden="true"
            >
              {c.done ? "✓" : "▸"}
            </span>
            <span className="sr-only">{c.done ? "done:" : "running:"}</span>
            <span className={styles.activityRole}>{roleLabel(c.role)}</span>
            {c.instruction ? (
              <span className={styles.activityInstruction}>{c.instruction}</span>
            ) : null}
          </p>
          {c.summary ? (
            <div className={styles.activityResult}>
              <MarkdownAnswer text={c.summary} />
            </div>
          ) : c.done ? (
            <p className={styles.activityEmpty}>This step reported no result.</p>
          ) : null}
        </li>
      ))}
    </ol>
  );
}

/** The per-turn action bar (S-620, CR-204 C10, [FR-UI-45]): two 24px ICON buttons —
 *  Copy the answer (a check for 1.5 s once copied, {@link CopyControl}) and Regenerate
 *  the turn (a replace, not an append — [FR-UI-20]: the store is cut back to before the
 *  question and it is re-run, S-624). Each carries its accessible name and a tooltip,
 *  and takes the `--tint-hover` fill on hover and keyboard focus. Hidden while the
 *  turn is running. */
function MessageActions({ answer }: { answer: string }) {
  return (
    <ActionBarPrimitive.Root hideWhenRunning autohide="never" className={styles.actions}>
      <CopyControl variant="icon" text={answer} label="Copy answer" />
      <ActionBarPrimitive.Reload
        className={styles.actionIcon}
        aria-label="Regenerate answer"
        title="Regenerate answer"
      >
        <IconRefresh />
      </ActionBarPrimitive.Reload>
    </ActionBarPrimitive.Root>
  );
}

/** The marker the shortcut scope is read from: the conversation (transcript and
 *  composer) carries it, so a text field INSIDE it is the chat's own. */
const THREAD_ROOT_ATTR = "data-chat-thread";

/** Whether a key event came from somewhere text is being typed. */
function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName);
}

/** The platform's command key: ⌘ on macOS, Ctrl elsewhere ([FR-UI-46]). */
function isCommandKey(event: KeyboardEvent): boolean {
  const mac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  return mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}

/**
 * The chat's two document-level shortcuts (S-620, [FR-UI-46]): **Esc** stops a running
 * turn and **⌘K / Ctrl+K** starts a new chat. (Enter and Shift+Enter are the composer
 * input's own.)
 *
 * Esc goes through the pill's Stop — `aui.composer().cancel()`, the very call the
 * round Stop button makes, enabled by the same `canCancel` — so a stop has ONE path
 * to the runtime's `onCancel`, whoever asks for it. ⌘K calls the `onNewChat` the
 * rail's "+ New chat" uses.
 *
 * Scope: the keys are heard on the document, so they work from the transcript, the
 * rail's buttons and the page body — but a keystroke from a text field OUTSIDE the
 * conversation is not the chat's: another view's field, the shell's, the rail's
 * rename box. It is left alone, and the browser's own binding for it stands. A
 * keystroke some other handler already claimed (`defaultPrevented`), an IME
 * composition and a held key are ignored.
 */
function ChatShortcuts({ onNewChat }: { onNewChat: () => void }) {
  const aui = useAui();
  // The pill shows Stop only while a turn runs (`ThreadPrimitive.If running`); the
  // composer's `canCancel` alone is true whenever the runtime CAN cancel, running or not.
  const canStop = useAuiState((s) => s.thread.isRunning && s.composer.canCancel);
  // The listener is bound once per mount; it reads the latest of these at call time.
  const latest = useRef({ aui, canStop, onNewChat });
  latest.current = { aui, canStop, onNewChat };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.repeat) return;
      const target = event.target;
      if (isTextField(target) && !(target as HTMLElement).closest(`[${THREAD_ROOT_ATTR}]`)) return;
      const { aui: runtime, canStop: running, onNewChat: newChat } = latest.current;
      if (event.key === "Escape" && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey) {
        if (!running) return;
        event.preventDefault();
        runtime.composer().cancel();
      } else if (event.key.toLowerCase() === "k" && !event.shiftKey && !event.altKey && isCommandKey(event)) {
        // Claimed even where the browser binds it (Ctrl+K focuses the search box in some).
        event.preventDefault();
        newChat();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);
  return null;
}

/**
 * The floating ↓ control (S-620, [FR-UI-46]): once the reader has scrolled away from
 * the bottom of the transcript, a round button floats over its bottom edge, just above
 * the composer (a zero-height sticky anchor at the end of the transcript, so it stays
 * on the visible bottom while the turns scroll beneath it); activating it
 * scrolls to the bottom and the thread follows the stream again. At the bottom it is NOT
 * RENDERED — absent from the accessibility tree, not merely invisible — so there is no
 * dead control to tab to.
 *
 * "At the bottom" is the viewport's own `isAtBottom`, the same flag assistant-ui's
 * follow-the-stream reads: when it is false the transcript is not being followed, and
 * `ScrollToBottom` sets the scroll that makes it true again. One source of truth for
 * both halves. Activating it also hands focus to the composer, because the control
 * unmounts at the bottom and would otherwise drop a keyboard user's focus on `<body>`.
 */
function ScrollToBottom() {
  // The control unmounts the moment the transcript is back at the bottom. Activated
  // from the keyboard it would take focus with it (to `<body>`), so focus goes to the
  // composer — where the reader is heading anyway.
  const returnFocusToComposer = (event: MouseEvent<HTMLElement>) => {
    event.currentTarget
      .closest(`[${THREAD_ROOT_ATTR}]`)
      ?.querySelector<HTMLTextAreaElement>("textarea:not(:disabled)")
      ?.focus({ preventScroll: true });
  };
  const atBottom = useThreadViewport((s) => s.isAtBottom);
  return (
    <div className={styles.scrollAnchor}>
      {!atBottom && (
        <ThreadPrimitive.ScrollToBottom
          className={styles.scrollToBottom}
          aria-label="Scroll to bottom"
          title="Scroll to bottom"
          onClick={returnFocusToComposer}
        >
          <IconArrowDown />
        </ThreadPrimitive.ScrollToBottom>
      )}
    </div>
  );
}

/** The composer (S-619, CR-204 L3, [FR-UI-45]): ONE pill pinned to the card's bottom
 *  edge, whose focus ring is drawn on the pill itself. Inside it, a leading "+" menu,
 *  the message input — one line, growing to a capped height and then scrolling — and
 *  a trailing round button: Send, gated on consent via the runtime's `isDisabled` and
 *  neutral until there is something to send, then Stop while a turn is in flight
 *  ([FR-UI-19]).
 *
 *  S-625 ([FR-UI-49]): the "+" opens the preset menu ({@link PresetMenu}), and while
 *  the conversation runs under a preset other than Default a clearable chip names it
 *  beside the input ({@link PresetChip}). Like the input, both wait for consent.
 *
 *  S-620 ([FR-UI-46]): Enter sends and Shift+Enter inserts a newline (the input's
 *  `submitMode="enter"`, stated rather than left to the library's default), and a
 *  muted hint UNDER the pill names both — the input's `aria-describedby`. Esc and ⌘K
 *  are {@link ChatShortcuts}'s. */
function Composer({
  consented,
  preset,
  onChoosePreset,
}: {
  consented: boolean;
  preset: ChatPresetId;
  onChoosePreset: (preset: ChatPresetId) => void;
}) {
  const hintId = useId();
  return (
    <>
      <ComposerPrimitive.Root className={styles.composer}>
        <PresetMenu disabled={!consented} preset={preset} onChoose={onChoosePreset} />
        {preset !== DEFAULT_PRESET && (
          <PresetChip preset={preset} onClear={() => onChoosePreset(DEFAULT_PRESET)} />
        )}
        <ComposerPrimitive.Input
          className={styles.input}
          minRows={1}
          maxRows={8}
          aria-label="Your message"
          aria-describedby={hintId}
          submitMode="enter"
          disabled={!consented}
          placeholder="Ask about this project — e.g. what's the riskiest untested code and who calls it?"
        />
        <ThreadPrimitive.If running={false}>
          <ComposerPrimitive.Send className={styles.send} aria-label="Send" title="Send">
            <IconArrowUp />
          </ComposerPrimitive.Send>
        </ThreadPrimitive.If>
        <ThreadPrimitive.If running>
          <ComposerPrimitive.Cancel className={styles.stop} aria-label="Stop" title="Stop">
            <IconStop />
          </ComposerPrimitive.Cancel>
        </ThreadPrimitive.If>
      </ComposerPrimitive.Root>
      <p id={hintId} className={styles.composerHint}>
        <kbd>Enter</kbd> to send · <kbd>Shift</kbd>+<kbd>Enter</kbd> for a new line
      </p>
    </>
  );
}

/** The composer's message input, from any control inside the pill — where focus goes
 *  when a pill control that held it closes or unmounts. */
function composerInputOf(el: HTMLElement | null): HTMLTextAreaElement | null {
  return el?.closest("form")?.querySelector<HTMLTextAreaElement>("textarea:not(:disabled)") ?? null;
}

/**
 * The "+" preset menu (S-625, [FR-UI-49], CR-204 C7): a menu button over the four
 * built-in presets, each a `menuitemradio` with its one-line description, the
 * conversation's own checked. Picking one sets the open conversation's preset; the
 * next turn — however it is sent — runs under it.
 *
 * Keyboard: the menu opens on the checked item; ↑ / ↓ (wrapping), Home and End move;
 * Enter or Space picks; Esc closes and returns focus to the "+". Esc stops at the menu
 * (`stopPropagation`, and `preventDefault` for any listener before it), so it never
 * also reaches {@link ChatShortcuts}' document-level Esc-stops-the-turn. Tab, or a
 * pointer down outside, closes it.
 */
function PresetMenu({
  disabled,
  preset,
  onChoose,
}: {
  disabled: boolean;
  preset: ChatPresetId;
  onChoose: (preset: ChatPresetId) => void;
}) {
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const anchorRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  // Open on the conversation's own preset, so the reader starts where they are.
  useEffect(() => {
    if (open) menuRef.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
  }, [open]);

  // A pointer down anywhere outside the menu and its button closes it.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!anchorRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  // A consent withdrawn while the menu is open (another scope's banner) closes it.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  const closeToButton = () => {
    setOpen(false);
    buttonRef.current?.focus();
  };

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [],
    );
    const at = items.indexOf(document.activeElement as HTMLElement);
    const focusAt = (i: number) => items[(i + items.length) % items.length]?.focus();
    switch (event.key) {
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        closeToButton();
        break;
      case "ArrowDown":
        event.preventDefault();
        focusAt(at + 1);
        break;
      case "ArrowUp":
        event.preventDefault();
        focusAt(at - 1);
        break;
      case "Home":
        event.preventDefault();
        focusAt(0);
        break;
      case "End":
        event.preventDefault();
        focusAt(items.length - 1);
        break;
      case "Tab":
        setOpen(false);
        break;
    }
  };

  return (
    <div ref={anchorRef} className={styles.presetAnchor}>
      <button
        ref={buttonRef}
        type="button"
        className={styles.composerMenu}
        aria-label="Composer options"
        title="Choose a preset"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        disabled={disabled}
        onClick={() => setOpen((was) => !was)}
      >
        <IconPlus />
      </button>
      {open && (
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-label="Preset"
          className={styles.presetMenu}
          onKeyDown={onMenuKeyDown}
        >
          {CHAT_PRESETS.map((p) => (
            <button
              key={p.id}
              type="button"
              role="menuitemradio"
              aria-checked={p.id === preset}
              aria-labelledby={`${menuId}-${p.id}-label`}
              aria-describedby={`${menuId}-${p.id}-description`}
              tabIndex={-1}
              className={styles.presetItem}
              onClick={() => {
                onChoose(p.id);
                closeToButton();
              }}
            >
              <span id={`${menuId}-${p.id}-label`} className={styles.presetLabel}>
                {p.label}
              </span>
              <span id={`${menuId}-${p.id}-description`} className={styles.presetDescription}>
                {p.description}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The non-Default preset's chip (S-625, [FR-UI-49]): names the preset the open
 *  conversation runs under, and clears it back to Default. Clearing unmounts the chip,
 *  so focus moves on to the message input rather than dropping to `<body>`. */
function PresetChip({ preset, onClear }: { preset: ChatPresetId; onClear: () => void }) {
  const { label } = presetOf(preset);
  return (
    <span className={styles.presetChip}>
      <span className={styles.presetChipLabel}>{label}</span>
      <button
        type="button"
        className={styles.presetChipClear}
        aria-label={`Clear the ${label} preset`}
        title="Back to Default"
        onClick={(event) => {
          composerInputOf(event.currentTarget)?.focus();
          onClear();
        }}
      >
        <IconPlus />
      </button>
    </span>
  );
}
