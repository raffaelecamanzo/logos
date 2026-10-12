/*
 * The chat history rail (S-210/S-211, CR-053, FR-UI-26, FR-UI-20, ADR-47, ADR-45) —
 * the left sub-rail listing durable conversations most-recent-first over the merged
 * S-209 `GET /api/v1/chat/threads`, with select-to-restore, a "+ New chat" reset,
 * a per-conversation delete behind a confirmation step, and an inline rename
 * (S-623, FR-UI-48).
 *
 * Presentation only: it owns no transport and no thread state — the multi-thread
 * runtime (`chatRuntime.tsx`) supplies the list, the active id, and the actions, so
 * the rail stays a thin, testable renderer (the S-193 design grammar, every value a
 * token; no inline style, so the self-only CSP stays byte-identical — NFR-SE-06).
 * No secret ever rides a thread summary (NFR-SE-07).
 *
 * The state it DOES own is which row is awaiting confirmation and which row is being
 * renamed, with the name being typed: ephemeral presentation, scoped to the rendered
 * list. A pending confirm must not survive a conversation switch or outlive the rail.
 */

import { useCallback, useEffect, useState, type KeyboardEvent, type ReactNode } from "react";

import { Button } from "../../components/index.ts";
import { IconPencil, IconPlus, IconTrash } from "../../components/icons.tsx";
import { normalizeThreadTitle, THREAD_TITLE_MAX, type ThreadSummary } from "./chatModel.ts";
import styles from "./Chat.module.css";
import railStyles from "./ThreadList.module.css";

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
  /** Rename a conversation (S-623, FR-UI-48), called with the new name already
   *  normalized and never empty. Absent, the rail offers no rename. */
  onRename?: (id: number, title: string) => void;
  /** An honest note when the list could not be read (or a delete or rename failed), else `null`. */
  error?: string | null;
  /** What an empty rail says — the member chat's default sentence unless the chat
   *  has more to say about where else conversations live (the Workspace Chat names
   *  the member conversations it does not list, S-485). */
  emptyNote?: ReactNode;
}

/** The conversation-history rail: a "+ New chat" action over a most-recent-first
 *  list of titled conversations, the open one marked `aria-current`, each with an
 *  inline rename and a confirm-gated delete.
 *
 *  S-619 ([FR-UI-45], CR-204 L6) took it out of its card: it sits on the page surface,
 *  each row LINE (title, pencil, trash) is the unit the filled pill and the active row's red
 *  leading marker ride on, and the delete affordance is a trash glyph the stylesheet
 *  shows on row hover, row focus and the active row only. */
export function ThreadList({
  threads,
  activeThreadId,
  onSelect,
  onNewChat,
  onDelete,
  onRename,
  error,
  emptyNote = "No conversations yet — your chats will appear here.",
}: ThreadListProps) {
  // The row awaiting confirmation, or `null` when none is. Exactly one row can be
  // pending at a time, so opening a second confirm closes the first — the user is
  // never looking at two armed destructive actions.
  const [confirmingId, setConfirmingId] = useState<number | null>(null);

  // Switching conversation ("+ New chat" or selecting another) disarms any pending
  // confirm. An armed destructive prompt is intent bound to a MOMENT, not just to a
  // row: it must not lie in wait behind a context switch for a user who returns to
  // the rail later (e.g. via the narrow-viewport toggle) and clicks it through
  // without re-establishing that intent. The rail is not unmounted by either action,
  // so nothing else would clear it.
  useEffect(() => setConfirmingId(null), [activeThreadId]);

  // The row being renamed and the name typed so far (S-623, [FR-UI-48]). One row
  // at a time, and never beside an armed delete: each disarms the other.
  const [renaming, setRenaming] = useState<{ id: number; draft: string; refused: boolean } | null>(
    null,
  );
  // Focus the field when it opens, with the old name selected so typing replaces
  // it. A stable callback, so it runs once per opened field, not on every keystroke.
  const focusField = useCallback((el: HTMLInputElement | null) => {
    el?.focus();
    el?.select();
  }, []);
  const startRename = (t: ThreadSummary) => {
    setConfirmingId(null);
    setRenaming({ id: t.id, draft: t.title, refused: false });
  };
  // Enter saves, Esc cancels, an empty name is refused — the field stays open and
  // says so, and nothing is sent. An unchanged name closes the field with no write.
  const onRenameKey = (e: KeyboardEvent<HTMLInputElement>, t: ThreadSummary) => {
    if (!renaming) return;
    // A key that confirms or cancels an IME composition (Japanese, Chinese,
    // Korean input) belongs to the IME, not to the rename.
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "Escape") {
      // Esc here cancels the rename only — it must not also reach a view-wide
      // handler (e.g. one that stops a running turn).
      e.preventDefault();
      e.stopPropagation();
      setRenaming(null);
      return;
    }
    if (e.key !== "Enter") return;
    e.preventDefault();
    const title = normalizeThreadTitle(renaming.draft);
    if (title == null) {
      setRenaming({ ...renaming, refused: true });
      return;
    }
    setRenaming(null);
    if (title !== t.title) onRename?.(t.id, title);
  };

  return (
    <nav className={styles.rail} aria-label="Conversations">
      {/* The "+ New chat" action, full-size and pinned above the scrolling list
          (S-308, FR-UI-33): the `md` Button, kept out of the list's scroll region
          by the rail's layout. The leading `IconPlus` IS the "+" — it is decorative
          (`aria-hidden` via the shared `Svg` wrapper), so the accessible name is
          "New chat" rather than a spoken "plus New chat". */}
      <Button variant="secondary" className={styles.newChat} onClick={onNewChat}>
        <IconPlus />
        New chat
      </Button>

      {/* The note sits BESIDE the list, not instead of it. Two different things can
          go wrong: the list could not be READ (there is nothing to show, so the note
          stands alone), or an ACTION on an existing list failed (a delete) — and in
          that second case replacing the rail with the note would hide every
          conversation the user still has, a worse lie than the failure itself. */}
      {error && (
        <p className={styles.railError} role="status">
          {error}
        </p>
      )}

      {threads.length === 0 ? (
        // Silent when a read failed: "no conversations yet" would be a claim we
        // cannot make — we do not know what is there ([NFR-CC-04]).
        !error && (
          <p className={styles.railEmpty}>{emptyNote}</p>
        )
      ) : (
        <ul className={styles.threadList}>
          {threads.map((t) => {
            const active = t.id === activeThreadId;
            const confirming = t.id === confirmingId;
            const editing = renaming?.id === t.id ? renaming : null;
            return (
              <li key={t.id} className={styles.threadRow}>
                <div className={active ? `${styles.threadLine} ${styles.threadLineActive}` : styles.threadLine}>
                  {editing ? (
                    <input
                      className={railStyles.renameInput}
                      aria-label={`Rename conversation “${t.title}”`}
                      aria-invalid={editing.refused ? "true" : undefined}
                      aria-describedby={editing.refused ? `rename-refused-${t.id}` : undefined}
                      value={editing.draft}
                      maxLength={THREAD_TITLE_MAX}
                      ref={focusField}
                      onChange={(e) => setRenaming({ ...editing, draft: e.target.value, refused: false })}
                      onKeyDown={(e) => onRenameKey(e, t)}
                      // Leaving the field cancels: only Enter commits a rename.
                      onBlur={() => setRenaming(null)}
                    />
                  ) : (
                    <button
                      type="button"
                      className={active ? `${styles.threadItem} ${styles.threadActive}` : styles.threadItem}
                      aria-current={active ? "true" : undefined}
                      onClick={() => onSelect(t.id)}
                      onDoubleClick={onRename ? () => startRename(t) : undefined}
                    >
                      {t.title}
                    </button>
                  )}
                  {onRename && !editing && (
                    // The rail's icon-action look (`threadDelete`): hidden on an idle
                    // row, shown on hover, focus and the active row.
                    <button
                      type="button"
                      className={styles.threadDelete}
                      aria-label={`Rename conversation “${t.title}”`}
                      onClick={() => startRename(t)}
                    >
                      <IconPencil width={16} height={16} />
                    </button>
                  )}
                  {!editing && (
                  <button
                    type="button"
                    className={styles.threadDelete}
                    // The accessible name names the CONVERSATION: in a list of rows a
                    // bare "Delete" is the classic mis-target hazard.
                    aria-label={`Delete conversation “${t.title}”`}
                    aria-expanded={confirming}
                    onClick={() => setConfirmingId(confirming ? null : t.id)}
                  >
                    <IconTrash width={16} height={16} />
                  </button>
                  )}
                </div>

                {editing?.refused && (
                  <p id={`rename-refused-${t.id}`} className={railStyles.renameRefused} role="status">
                    A conversation needs a name.
                  </p>
                )}

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
