/*
 * ChatView (S-200, CR-051, FR-UI-18, FR-UI-19, FR-UI-20, FR-UI-24, ADR-45) — the
 * Chat tab rebuilt on **assistant-ui** (`@assistant-ui/react`) over a custom
 * runtime adapter on the UNCHANGED intent-guarded SSE stream (`chatRuntime.tsx`).
 *
 * assistant-ui provides the thread, the composer, message rendering, and the
 * copy / stop / regenerate affordances; this file supplies the Logos-specific
 * surface around it — the configure-first state, the first-use consent gate
 * (NFR-SE-07), Clear-history (FR-UI-20), and the CUSTOM assistant-turn components
 * that re-implement the bespoke surface over assistant-ui's primitives:
 *   - the planner's plan list,
 *   - the subagent-activity chips,
 *   - the honest budget-halt notice and the honest provider error ([FR-UI-24],
 *     rendered verbatim from the SSE `error` frame), never a fabricated answer
 *     ([NFR-CC-04]),
 *   - the answer as streamed markdown with code blocks (`MarkdownAnswer.tsx`).
 *
 * Everything renders through the S-193 design tokens (`Chat.module.css`); no
 * inline `<style>`/`<script>`, no CSS-in-JS, so the byte-identical self-only CSP
 * holds ([NFR-SE-06]). The masked chat key never reaches this surface (NFR-SE-07):
 * the configured body receives only the `[chat]` policy, and the runtime adapter
 * only ever sends the user message.
 */

import { useCallback, useState } from "react";
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
import { Button, Callout } from "../../components/index.ts";
import { MarkdownAnswer } from "./MarkdownAnswer.tsx";
import { useChatRuntime } from "./chatRuntime.tsx";
import {
  endpointHost,
  hasConsent,
  isConfigured,
  rememberConsent,
  roleLabel,
  type ChatConfigReadModel,
  type ChatPolicy,
  type TurnState,
} from "./chatModel.ts";
import styles from "./Chat.module.css";

export function ChatView() {
  const config = useApiResource<ChatConfigReadModel>(() => fetchChatConfig(), []);
  return (
    <div className={styles.view}>
      <AsyncResource resource={config} loadingLabel="Loading chat…">
        {(model) =>
          isConfigured(model) ? (
            // Only the policy slice crosses into the configured body — the masked
            // key (`model.chat_key`) is deliberately NOT passed (NFR-SE-07).
            <ChatConfigured chat={model.config.parsed.chat} />
          ) : (
            <ConfigureFirst />
          )
        }
      </AsyncResource>
    </div>
  );
}

/** The honest configure-first state ([FR-UI-18]): a muted callout into the Config
 *  tab — NOT an error, and no composer. */
function ConfigureFirst() {
  return (
    <Callout label="CONFIGURE" tone="muted">
      <p>
        The agentic chat needs an LLM provider before it can answer. Set the provider,
        model, and API key in the <a href="/config">Config</a> tab, then return here to
        start chatting. Until then no outbound call is possible.
      </p>
    </Callout>
  );
}

/** The configured chat surface: the consent banner, the assistant-ui thread, and
 *  the Clear-history control. */
function ChatConfigured({ chat }: { chat: ChatPolicy }) {
  const [consented, setConsented] = useState<boolean>(() => hasConsent());
  const { runtime, clearHistory, clearing, clearMessage } = useChatRuntime(consented);

  const acceptConsent = useCallback(() => {
    rememberConsent();
    setConsented(true);
  }, []);

  const onClear = useCallback(() => {
    if (!window.confirm("Clear all chat history and its memory? This cannot be undone.")) return;
    void clearHistory();
  }, [clearHistory]);

  return (
    <div className={styles.chat}>
      {!consented && <ConsentBanner chat={chat} onAccept={acceptConsent} />}

      <AssistantRuntimeProvider runtime={runtime}>
        <ThreadPrimitive.Root className={styles.threadRoot}>
          <ThreadPrimitive.Viewport className={styles.log}>
            <ThreadPrimitive.Empty>
              <EmptyHint chat={chat} />
            </ThreadPrimitive.Empty>
            <ThreadPrimitive.Messages components={{ UserMessage, AssistantMessage }} />
          </ThreadPrimitive.Viewport>
          <Composer consented={consented} />
        </ThreadPrimitive.Root>
      </AssistantRuntimeProvider>

      <div className={styles.historyActions}>
        <Button variant="ghost" onClick={onClear} disabled={clearing}>
          Clear history
        </Button>
        <span className={styles.clearResult} role="status" aria-live="polite">
          {clearMessage}
        </span>
      </div>
    </div>
  );
}

/** The first-use consent disclosure (NFR-SE-07): names the endpoint and what is
 *  sent before any outbound call; the composer is disabled until it is accepted. */
function ConsentBanner({ chat, onAccept }: { chat: ChatPolicy; onAccept: () => void }) {
  return (
    <Callout label="BEFORE YOU START" tone="warm" className={styles.consent}>
      <p>
        Asking a question sends your message together with{" "}
        <strong>source and graph excerpts</strong> from this project to{" "}
        <strong>{endpointHost(chat)}</strong> (the configured <code>{chat.provider}</code>{" "}
        endpoint). Nothing is sent until you ask.
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

/** The empty-thread hint: what to ask and the turn's budget bounds. */
function EmptyHint({ chat }: { chat: ChatPolicy }) {
  return (
    <p className={styles.empty}>
      No messages yet. Ask a question to start a turn — the planner&apos;s steps and each
      subagent&apos;s activity appear as the answer streams. The turn is bounded by the budget
      tree ({chat.max_tool_calls} tool calls, {chat.max_subagent_tool_calls} per subagent,{" "}
      {chat.max_replans} replans).
    </p>
  );
}

/** A user turn: the message text, right-aligned. */
function UserMessage() {
  return (
    <MessagePrimitive.Root className={styles.user}>
      <MessagePrimitive.Parts />
    </MessagePrimitive.Root>
  );
}

/** An assistant turn: the plan, the subagent-activity chips, the streamed markdown
 *  answer, an honest halt or error, and the copy/regenerate action bar. The folded
 *  turn rides on `metadata.custom.turn`; data is rendered as React-escaped text or
 *  through `react-markdown` (which never injects raw HTML). */
function AssistantMessage() {
  const turn = useMessage((m) => m.metadata.custom.turn as TurnState | undefined);
  if (!turn) return null;
  return (
    <MessagePrimitive.Root className={styles.assistant}>
      <PlanList plan={turn.plan} />
      <ActivityChips turn={turn} />
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

/** The planner's plan (a replan supersedes the prior plan). */
function PlanList({ plan }: { plan: TurnState["plan"] }) {
  if (!plan) return null;
  return (
    <ol className={styles.plan}>
      <li className={styles.planCaption}>{plan.round > 0 ? "Revised plan" : "Plan"}</li>
      {plan.steps.map((s, i) => (
        <li key={i} className={styles.planStep}>
          {roleLabel(s.role)}: {s.instruction}
        </li>
      ))}
    </ol>
  );
}

/** The subagent-activity chips, in start order (running → done). */
function ActivityChips({ turn }: { turn: TurnState }) {
  if (turn.chips.length === 0) return null;
  return (
    <div className={styles.chips}>
      {turn.chips.map((c) => (
        <span
          key={c.index}
          className={`${styles.chip} ${c.done ? styles.chipDone : styles.chipRunning}`}
          title={c.summary}
        >
          {c.done ? "✓" : "▸"} {roleLabel(c.role)}
          {c.instruction ? `: ${c.instruction}` : ""}
        </span>
      ))}
    </div>
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
