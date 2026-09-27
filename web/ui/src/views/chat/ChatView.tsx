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
 * Whether chat is usable is not decided here. `chatReadiness` (`chatModel.ts`,
 * S-452) reads it off the read-model's effective-chat origins — the resolution the
 * turn path dials — and this file renders the verdict.
 */

import { useCallback, useState } from "react";
import type { MouseEvent } from "react";
import {
  ActionBarPrimitive,
  AssistantRuntimeProvider,
  ComposerPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  useMessage,
} from "@assistant-ui/react";

import { fetchChatConfig } from "../../api/chatClient.ts";
import { AsyncResource, useApiResource } from "../../api/hooks.tsx";
import { Button, Callout, Card } from "../../components/index.ts";
import { useWorkspace } from "../../workspace/WorkspaceContext.tsx";
import { MarkdownAnswer } from "./MarkdownAnswer.tsx";
import { ThreadList } from "./ThreadList.tsx";
import { useChatRuntime } from "./chatRuntime.tsx";
import {
  chatReadiness,
  chatScope,
  configureFirstCopy,
  endpointHost,
  hasConsent,
  modelLabel,
  rememberConsent,
  roleLabel,
  turnEndedEmpty,
  workspaceConfigRepairHref,
  WORKSPACE_CONFIG_FILE,
  WORKSPACE_SECRETS_FILE,
  type ChatConfigReadModel,
  type ChatPolicy,
  type ChatReady,
  type ConfigureFirst as ConfigureFirstState,
  type TurnState,
} from "./chatModel.ts";
import styles from "./Chat.module.css";

export function ChatView() {
  const config = useApiResource<ChatConfigReadModel>(() => fetchChatConfig(), []);
  const { mode, member } = useWorkspace();
  const scope = chatScope(mode, member);
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
          return verdict.ready ? <ChatConfigured ready={verdict} /> : <ConfigureFirst state={verdict} />;
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

/** The configured chat surface: the conversation-history rail (S-210/S-211), the
 *  consent banner, and the assistant-ui thread inside the view's `Card` (S-308,
 *  [FR-UI-33]). There is no global Clear-history — deletion is per conversation,
 *  in the rail (S-211, [FR-UI-26], [ADR-47]). */
function ChatConfigured({ ready }: { ready: ChatReady }) {
  const chat = ready.policy;
  const [consented, setConsented] = useState<boolean>(() => hasConsent());
  // The rail collapses behind a toggle below ~1023px (S-210 AC-3); `railOpen`
  // drives that toggle. At ≥1024px the rail is always shown (CSS), so this state
  // is inert there — it only gates the narrow-viewport disclosure.
  const [railOpen, setRailOpen] = useState(false);
  const { runtime, threads, activeThreadId, selectThread, newChat, deleteThread, threadsError } =
    useChatRuntime(consented);

  const acceptConsent = useCallback(() => {
    rememberConsent();
    setConsented(true);
  }, []);

  // Deleting keeps the rail OPEN (unlike select / new chat): the user is managing
  // the list and usually deletes more than one row.
  const onDelete = useCallback((id: number) => void deleteThread(id), [deleteThread]);

  // Selecting or starting a conversation closes the narrow-viewport rail so the
  // restored thread is in view (a no-op at ≥1024px, where the rail stays open).
  const onSelect = useCallback(
    (id: number) => {
      void selectThread(id);
      setRailOpen(false);
    },
    [selectThread],
  );
  const onNewChat = useCallback(() => {
    newChat();
    setRailOpen(false);
  }, [newChat]);

  return (
    <div className={styles.chat}>
      {!consented && <ConsentBanner ready={ready} onAccept={acceptConsent} />}

      <div className={styles.layout}>
        <button
          type="button"
          className={styles.railToggle}
          aria-expanded={railOpen}
          aria-controls="chat-rail"
          onClick={() => setRailOpen((open) => !open)}
        >
          {railOpen ? "Hide conversations" : "Conversations"}
        </button>

        <aside
          id="chat-rail"
          className={railOpen ? `${styles.railPane} ${styles.railPaneOpen}` : styles.railPane}
        >
          <ThreadList
            threads={threads}
            activeThreadId={activeThreadId}
            onSelect={onSelect}
            onNewChat={onNewChat}
            onDelete={onDelete}
            error={threadsError}
          />
        </aside>

        <div className={styles.main}>
          {/* The persistent status band (S-309, [FR-UI-33]): this view's leading
              verdict element, once configured and consented — the first-use
              consent gate above is preserved verbatim ahead of it. `.main` is
              this component's stable composition point (S-308). */}
          {consented && <StatusBand chat={chat} />}
          {/* The view's ONE card (S-308, [FR-UI-33]): it wraps the whole transcript
              and the composer, never a turn — the turns inside stay flat, so the
              signal-red edge appears once per view, not once per answer. The
              viewport inside stays a bounded scroll box; assistant-ui drives its
              `scrollTop` to follow the stream. */}
          <Card>
            <AssistantRuntimeProvider runtime={runtime}>
              <ThreadPrimitive.Root className={styles.threadRoot}>
                <ThreadPrimitive.Viewport className={styles.log}>
                  <ThreadPrimitive.Empty>
                    <EmptyHint />
                  </ThreadPrimitive.Empty>
                  <ThreadPrimitive.Messages components={{ UserMessage, AssistantMessage }} />
                </ThreadPrimitive.Viewport>
                <Composer consented={consented} />
              </ThreadPrimitive.Root>
            </AssistantRuntimeProvider>
          </Card>
        </div>
      </div>
    </div>
  );
}

/** The first-use consent disclosure (NFR-SE-07): names the EFFECTIVE endpoint and
 *  what is sent before any outbound call — and, when the policy or the key is
 *  inherited, says so, since the endpoint is then one this member's own
 *  `config.toml` never names. The composer is disabled until it is accepted. */
function ConsentBanner({ ready, onAccept }: { ready: ChatReady; onAccept: () => void }) {
  const chat = ready.policy;
  return (
    <Callout label="BEFORE YOU START" tone="warm" className={styles.consent}>
      <p>
        Asking a question sends your message together with{" "}
        <strong>source and graph excerpts</strong> from this project to{" "}
        <strong>{endpointHost(chat)}</strong>{" "}
        {ready.policyOrigin === "workspace" ? (
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
      <p className={styles.providerLine}>
        {chat.provider} · {endpointHost(chat)} · {chat.model}
      </p>
      <Button variant="primary" onClick={onAccept}>
        Start chatting
      </Button>
    </Callout>
  );
}

/**
 * The persistent status band (S-309, [FR-UI-33], CR-092's third §4.13
 * lead-callout state — specified but never built until now): once configured
 * and consented, names the provider, endpoint host and model, plus the turn's
 * budget-tree bounds, giving Chat the leading verdict element every other view
 * opens with (frontend-design §9, e.g. Health's `GATE · PASS`, Wiki's `WIKI ·
 * STALE`). It reads the same effective `[chat]` policy the configured body
 * already receives, so the masked key never reaches it ([NFR-SE-07]); an
 * undeclared model is named honestly rather than defaulted ([NFR-CC-04],
 * {@link modelLabel}). The bounds used to live only in the empty-thread hint
 * and vanish after the first message ({@link EmptyHint}) — they now persist
 * here instead, moved rather than duplicated.
 */
function StatusBand({ chat }: { chat: ChatPolicy }) {
  return (
    <Callout label="CHAT" tone="muted" className={styles.status}>
      <p className={styles.providerLine}>
        {chat.provider} · {endpointHost(chat)} · {modelLabel(chat)}
      </p>
      <p className={styles.budgetLine}>
        Budget tree: {chat.max_tool_calls} tool calls, {chat.max_subagent_tool_calls} per
        subagent, {chat.max_replans} replans.
      </p>
    </Callout>
  );
}

/** The empty-thread hint: what to ask. The turn's budget bounds used to live here
 *  and vanished after the first message — they now persist in the {@link
 *  StatusBand} above instead (S-309). */
function EmptyHint() {
  return (
    <p className={styles.empty}>
      No messages yet. Ask a question to start a turn — the planner&apos;s steps and each
      subagent&apos;s activity appear as the answer streams.
    </p>
  );
}

/** A user turn: the message text in a bubble hugging the RIGHT edge of the
 *  transcript (S-300, [FR-UI-31]). The root spans the transcript's width so both
 *  roles share one alignment line inside the card (S-308, [FR-UI-33]); the bubble
 *  is the inner element, so it right-aligns within that line rather than against
 *  the viewport. */
function UserMessage() {
  return (
    <MessagePrimitive.Root className={styles.user}>
      <div className={styles.userBubble}>
        <MessagePrimitive.Parts />
      </div>
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
  if (!turn) return null;
  return (
    <MessagePrimitive.Root className={styles.assistant}>
      <ActivityDisclosure turn={turn} />
      <div className={styles.answer}>
        {!turn.answer && !turn.halt && !turn.error && (
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
      </div>
      <MessageActions />
    </MessagePrimitive.Root>
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

/** The per-turn action bar: copy the answer, and regenerate the turn (a replace,
 *  not an append — [FR-UI-20]). Hidden while the turn is running. */
function MessageActions() {
  return (
    <ActionBarPrimitive.Root hideWhenRunning autohide="never" className={styles.actions}>
      <ActionBarPrimitive.Copy className={styles.action} aria-label="Copy answer">
        Copy
      </ActionBarPrimitive.Copy>
      <ActionBarPrimitive.Reload className={styles.action} aria-label="Regenerate answer">
        Regenerate
      </ActionBarPrimitive.Reload>
    </ActionBarPrimitive.Root>
  );
}

/** The composer: the message input, a Send control (gated on consent via the
 *  runtime's `isDisabled`), and a Stop control while a turn is in flight
 *  ([FR-UI-19]). */
function Composer({ consented }: { consented: boolean }) {
  return (
    <ComposerPrimitive.Root className={styles.composer}>
      <ComposerPrimitive.Input
        className={styles.input}
        rows={3}
        aria-label="Your message"
        disabled={!consented}
        placeholder="Ask about this project — e.g. what's the riskiest untested code and who calls it?"
      />
      <div className={styles.composerActions}>
        <ThreadPrimitive.If running={false}>
          <ComposerPrimitive.Send className={styles.send}>Send</ComposerPrimitive.Send>
        </ThreadPrimitive.If>
        <ThreadPrimitive.If running>
          <ComposerPrimitive.Cancel className={styles.stop}>Stop</ComposerPrimitive.Cancel>
        </ThreadPrimitive.If>
      </div>
    </ComposerPrimitive.Root>
  );
}
