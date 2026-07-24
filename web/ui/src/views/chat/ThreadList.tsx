/*
 * The chat history rail (S-210/S-211, CR-053, FR-UI-26, FR-UI-20, ADR-47, ADR-45) —
 * the left sub-rail listing durable conversations most-recent-first over the merged
 * S-209 `GET /api/v1/chat/threads`, with select-to-restore, a "+ New chat" reset,
 * and a per-conversation delete behind a confirmation step.
 *
 * Presentation only: it owns no transport and no thread state — the multi-thread
 * runtime (`chatRuntime.tsx`) supplies the list, the active id, and the actions, so
 * the rail stays a thin, testable renderer (the S-193 design grammar, every value a
 * token; no inline style, so the self-only CSP stays byte-identical — NFR-SE-06).
 * No secret ever rides a thread summary (NFR-SE-07).
 *
 * The one piece of state it DOES own is which row is awaiting confirmation: that is
 * ephemeral presentation, scoped to the rendered list, and it must not survive a
 * conversation switch or outlive the rail.
 */

import { useState } from "react";

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
  /** Delete a conversation and its per-thread memory. Called ONLY after the user
   *  has confirmed on the row (S-211 AC-1). */
  onDelete: (id: number) => void;
  /** An honest note when the list could not be read (or a delete failed), else `null`. */
  error?: string | null;
}

/** The conversation-history rail: a "+ New chat" action over a most-recent-first
 *  list of auto-titled conversations, the open one marked `aria-current`, each with
 *  a confirm-gated delete. */
export function ThreadList({
  threads,
  activeThreadId,
  onSelect,
  onNewChat,
  onDelete,
  error,
}: ThreadListProps) {
  // The row awaiting confirmation, or `null` when none is. Exactly one row can be
  // pending at a time, so opening a second confirm closes the first — the user is
  // never looking at two armed destructive actions.
  const [confirmingId, setConfirmingId] = useState<number | null>(null);

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
            const confirming = t.id === confirmingId;
            return (
              <li key={t.id} className={styles.threadRow}>
                <div className={styles.threadLine}>
                  <button
                    type="button"
                    className={active ? `${styles.threadItem} ${styles.threadActive}` : styles.threadItem}
                    aria-current={active ? "true" : undefined}
                    onClick={() => onSelect(t.id)}
                  >
                    {t.title}
                  </button>
                  <button
                    type="button"
                    className={styles.threadDelete}
                    // The accessible name names the CONVERSATION: in a list of rows a
                    // bare "Delete" is the classic mis-target hazard.
                    aria-label={`Delete conversation “${t.title}”`}
                    aria-expanded={confirming}
                    onClick={() => setConfirmingId(confirming ? null : t.id)}
                  >
                    ✕
                  </button>
                </div>

                {confirming && (
                  <div className={styles.threadConfirm} role="group" aria-label={`Confirm deleting “${t.title}”`}>
                    <p className={styles.threadConfirmText}>
                      Delete this conversation and its memory? This cannot be undone.
                    </p>
                    <div className={styles.threadConfirmActions}>
                      <button
                        type="button"
                        className={styles.threadConfirmDelete}
                        // Disarm BEFORE dispatching: the row is single-shot, so a
                        // double-click cannot issue two deletes.
                        onClick={() => {
                          setConfirmingId(null);
                          onDelete(t.id);
                        }}
                      >
                        Delete
                      </button>
                      <button
                        type="button"
                        className={styles.threadConfirmCancel}
                        onClick={() => setConfirmingId(null)}
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </nav>
  );
}
