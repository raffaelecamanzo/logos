/*
 * The chat history rail (S-210, CR-053, FR-UI-26, ADR-47, ADR-45) — the left
 * sub-rail listing durable conversations most-recent-first over the merged S-209
 * `GET /api/v1/chat/threads`, with select-to-restore and a "+ New chat" reset.
 *
 * Presentation only: it owns no transport and no thread state — the multi-thread
 * runtime (`chatRuntime.tsx`) supplies the list, the active id, and the two
 * actions, so the rail stays a thin, testable renderer (the S-193 design grammar,
 * every value a token; no inline style, so the self-only CSP stays byte-identical
 * — NFR-SE-06). No secret ever rides a thread summary (NFR-SE-07).
 */

import { Button } from "../../components/index.ts";
import type { ThreadSummary } from "./chatModel.ts";
import styles from "./Chat.module.css";

export interface ThreadListProps {
  /** The conversation list, most-recent-first (the server's sort). */
  threads: ThreadSummary[];
  /** The open conversation's id, or `null` for an unsent "+ New chat". */
  activeThreadId: number | null;
  /** Restore a conversation into the surface. */
  onSelect: (id: number) => void;
  /** Reset the composer to a fresh conversation. */
  onNewChat: () => void;
  /** An honest note when the list could not be read, else `null`. */
  error?: string | null;
}

/** The conversation-history rail: a "+ New chat" action over a most-recent-first
 *  list of auto-titled conversations, the open one marked `aria-current`. */
export function ThreadList({
  threads,
  activeThreadId,
  onSelect,
  onNewChat,
  error,
}: ThreadListProps) {
  return (
    <nav className={styles.rail} aria-label="Conversations">
      <Button variant="secondary" size="sm" className={styles.newChat} onClick={onNewChat}>
        + New chat
      </Button>

      {error ? (
        <p className={styles.railError} role="status">
          {error}
        </p>
      ) : threads.length === 0 ? (
        <p className={styles.railEmpty}>No conversations yet — your chats will appear here.</p>
      ) : (
        <ul className={styles.threadList}>
          {threads.map((t) => {
            const active = t.id === activeThreadId;
            return (
              <li key={t.id}>
                <button
                  type="button"
                  className={active ? `${styles.threadItem} ${styles.threadActive}` : styles.threadItem}
                  aria-current={active ? "true" : undefined}
                  onClick={() => onSelect(t.id)}
                >
                  {t.title}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </nav>
  );
}
