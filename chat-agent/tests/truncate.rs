//! `ChatStore::truncate_after` fitness tests (S-624, [FR-UI-46], [FR-UI-20],
//! [ADR-47] as extended by CR-204).
//!
//! An edited or regenerated turn replaces everything after it: the later messages
//! AND the per-thread memory written after the cut, in one transaction. These drive
//! a real on-disk `chat.db` through the production writers — the conversation through
//! [`ChatStore::append_message`], the memory through [`MemoryStore::record`] and
//! [`MemoryStore::set_working_memory`] at the point in the turn the chat surface
//! writes them — and count the rows that survive straight from SQLite.
//!
//! [FR-UI-46]: ../../docs/specs/requirements/FR-UI-46.md
//! [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
//! [ADR-47]: ../../docs/specs/architecture/decisions/ADR-47.md

use chat_agent::{db, ChatRole, ChatStore, MemoryStore, ScratchpadEntry};
use rusqlite::Connection;
use tempfile::TempDir;

/// One turn as the chat surface runs it: the question is appended, the turn's
/// scratchpad is recorded while it runs, then the answer is appended.
fn turn(store: &mut ChatStore, memory: &MemoryStore, thread: i64, question: &str, answer: &str) {
    store.append_message(thread, ChatRole::User, question, &[]).unwrap();
    let n = memory.next_turn(thread).unwrap();
    memory
        .record(thread, n, &ScratchpadEntry::Note { note: format!("looked into {question}") })
        .unwrap();
    memory
        .record(thread, n, &ScratchpadEntry::FinalAnswer { answer: answer.to_string() })
        .unwrap();
    store.append_message(thread, ChatRole::Assistant, answer, &[]).unwrap();
}

/// A thread of four answered turns — `U0 A0 U1 A1 U2 A2 U3 A3` at ordinals 0..=7 —
/// with two scratchpad entries per turn, and a working-memory summary written after
/// the third turn (at ordinal 5).
fn four_turns(root: &std::path::Path) -> i64 {
    let mut store = ChatStore::open(root).unwrap();
    let memory = MemoryStore::open(root).unwrap();
    let thread = store.create_thread("four turns").unwrap();
    for i in 0..4 {
        turn(&mut store, &memory, thread, &format!("question {i}"), &format!("answer {i}"));
        if i == 2 {
            memory.set_working_memory(thread, "the first three turns").unwrap();
        }
    }
    thread
}

fn conn(root: &std::path::Path) -> Connection {
    Connection::open(db::db_path(root)).unwrap()
}

fn count(root: &std::path::Path, sql: &str, thread: i64) -> i64 {
    conn(root).query_row(sql, [thread], |row| row.get(0)).unwrap()
}

fn scratchpad_rows(root: &std::path::Path, thread: i64) -> i64 {
    count(root, "SELECT COUNT(*) FROM chat_scratchpad WHERE thread_id = ?1", thread)
}

fn working_rows(root: &std::path::Path, thread: i64) -> i64 {
    count(root, "SELECT COUNT(*) FROM chat_working_memory WHERE thread_id = ?1", thread)
}

/// The memory rows of `thread` written after `cut` — the count the AC says is zero.
fn memory_written_after(root: &std::path::Path, thread: i64, cut: i64) -> i64 {
    conn(root)
        .query_row(
            "SELECT (SELECT COUNT(*) FROM chat_scratchpad
                      WHERE thread_id = ?1 AND written_at_ordinal > ?2)
                  + (SELECT COUNT(*) FROM chat_working_memory
                      WHERE thread_id = ?1 AND written_at_ordinal > ?2)",
            [thread, cut],
            |row| row.get(0),
        )
        .unwrap()
}

fn transcript(store: &ChatStore, thread: i64) -> Vec<(ChatRole, String)> {
    store
        .messages(thread)
        .unwrap()
        .into_iter()
        .map(|m| (m.role, m.content))
        .collect()
}

/// AC: editing the second of four user turns cuts before it — the store keeps the
/// first two messages, and after a reopen it returns exactly those, with no memory
/// row written after the cut (counted) while the first turn's memory stays.
#[test]
fn editing_the_second_of_four_turns_keeps_the_first_two_messages_and_their_memory_only() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let thread = four_turns(root);
    assert_eq!(scratchpad_rows(root, thread), 8, "two entries per turn");
    assert_eq!(working_rows(root, thread), 1);

    {
        let mut store = ChatStore::open(root).unwrap();
        let second = store.user_turn_ordinal(thread, 1).unwrap();
        assert_eq!(second, Some(2), "the second question is the third message");
        assert!(store.truncate_after(thread, second.unwrap() - 1).unwrap());
    }

    // Reopen: a new handle over the persisted file.
    let store = ChatStore::open(root).unwrap();
    assert_eq!(
        transcript(&store, thread),
        [
            (ChatRole::User, "question 0".to_string()),
            (ChatRole::Assistant, "answer 0".to_string()),
        ],
    );
    assert_eq!(memory_written_after(root, thread, 1), 0, "no memory written after the cut");
    assert_eq!(scratchpad_rows(root, thread), 2, "the first turn's scratchpad stays");
    assert_eq!(working_rows(root, thread), 0, "a summary written after the cut goes");
    let memory = MemoryStore::open(root).unwrap();
    assert_eq!(memory.scratchpad(thread, 0).unwrap().len(), 2);

    // The re-run turn appends after the kept messages, and its memory is its own.
    let mut store = store;
    turn(&mut store, &memory, thread, "question 1, edited", "answer 1′");
    assert_eq!(
        transcript(&store, thread).into_iter().map(|(_, c)| c).collect::<Vec<_>>(),
        ["question 0", "answer 0", "question 1, edited", "answer 1′"],
    );
    assert_eq!(scratchpad_rows(root, thread), 4);
}

/// A row written exactly at the cut stays; one written after it goes. Cutting just
/// after a question (ordinal 2) keeps that question's scratchpad — it was written
/// while the question was the newest message — and drops the answer and everything
/// later, including the summary written at ordinal 5.
#[test]
fn a_row_written_at_the_cut_stays_and_one_written_after_it_goes() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let thread = four_turns(root);

    assert!(ChatStore::open(root).unwrap().truncate_after(thread, 2).unwrap());

    let store = ChatStore::open(root).unwrap();
    assert_eq!(store.messages(thread).unwrap().len(), 3, "U0 A0 U1");
    assert_eq!(scratchpad_rows(root, thread), 4, "turns 0 and 1 (written at 0 and 2)");
    assert_eq!(memory_written_after(root, thread, 2), 0);
    assert_eq!(working_rows(root, thread), 0);
}

/// Cutting after the last message removes nothing — a no-op truncate keeps every
/// message and every memory row, the summary included.
#[test]
fn a_cut_after_the_last_message_removes_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let thread = four_turns(root);

    assert!(ChatStore::open(root).unwrap().truncate_after(thread, 7).unwrap());

    assert_eq!(ChatStore::open(root).unwrap().messages(thread).unwrap().len(), 8);
    assert_eq!(scratchpad_rows(root, thread), 8);
    assert_eq!(working_rows(root, thread), 1);
}

/// Editing the FIRST question empties the conversation but keeps it, title and all;
/// another conversation is untouched.
#[test]
fn a_negative_cut_empties_the_conversation_and_spares_the_others() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let thread = four_turns(root);
    let other = four_turns(root);

    let mut store = ChatStore::open(root).unwrap();
    assert_eq!(store.user_turn_ordinal(thread, 0).unwrap(), Some(0));
    assert!(store.truncate_after(thread, -1).unwrap());

    assert!(store.messages(thread).unwrap().is_empty());
    assert_eq!(store.thread(thread).unwrap().expect("the thread stays").title, "four turns");
    assert_eq!(scratchpad_rows(root, thread) + working_rows(root, thread), 0);
    assert_eq!(store.messages(other).unwrap().len(), 8, "another thread keeps its messages");
    assert_eq!(scratchpad_rows(root, other), 8, "and its memory");
    assert_eq!(working_rows(root, other), 1);
}

/// The truncate is ONE transaction: when the last of its deletes fails, the earlier
/// ones roll back and the thread is exactly as it was.
#[test]
fn a_failed_truncate_leaves_the_thread_exactly_as_it_was() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let thread = four_turns(root);
    // Make the working-memory delete — the last statement — fail.
    conn(root)
        .execute_batch(
            "CREATE TRIGGER refuse BEFORE DELETE ON chat_working_memory
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();

    let err = ChatStore::open(root).unwrap().truncate_after(thread, 1).unwrap_err();
    assert!(format!("{err:#}").contains("refused"), "got: {err:#}");

    assert_eq!(ChatStore::open(root).unwrap().messages(thread).unwrap().len(), 8);
    assert_eq!(scratchpad_rows(root, thread), 8);
    assert_eq!(working_rows(root, thread), 1);
}

/// An unknown thread is `false` (the route's `404`) and touches nothing; a user turn
/// past the stored ones has no ordinal.
#[test]
fn an_unknown_thread_or_user_turn_is_reported_not_guessed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let thread = four_turns(root);

    let mut store = ChatStore::open(root).unwrap();
    assert!(!store.truncate_after(thread + 100, -1).unwrap());
    assert_eq!(store.user_turn_ordinal(thread + 100, 0).unwrap(), None);
    assert_eq!(store.user_turn_ordinal(thread, 3).unwrap(), Some(6));
    assert_eq!(store.user_turn_ordinal(thread, 4).unwrap(), None, "only four questions");
    assert_eq!(store.messages(thread).unwrap().len(), 8);
}

/// A user turn counts USER messages only: a turn that stored its question and no
/// answer (stopped, halted, faulted) still counts, so the ordinals are not
/// position × 2.
#[test]
fn user_turns_count_questions_even_when_an_answer_is_missing() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("gaps").unwrap();
    store.append_message(thread, ChatRole::User, "unanswered", &[]).unwrap();
    store.append_message(thread, ChatRole::User, "answered", &[]).unwrap();
    store.append_message(thread, ChatRole::Assistant, "yes", &[]).unwrap();
    store.append_message(thread, ChatRole::User, "third", &[]).unwrap();

    assert_eq!(store.user_turn_ordinal(thread, 1).unwrap(), Some(1));
    assert_eq!(store.user_turn_ordinal(thread, 2).unwrap(), Some(3));
}

/// Migration 3 is the head of the ledger, applied on the store's own track.
#[test]
fn the_attribution_migration_is_the_head_of_the_ledger() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    drop(ChatStore::open(root).unwrap());
    let version: i64 = conn(root).query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 3);
    assert_eq!(db::latest_version(), 3);
}

/// A memory row's position is read from ITS OWN thread: in a store where another,
/// busier conversation came first, cutting this one before its second question keeps
/// this one's first-turn memory (and touches none of the other's).
#[test]
fn attribution_reads_only_the_rows_own_thread() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let busy = four_turns(root);
    let quiet = {
        let mut store = ChatStore::open(root).unwrap();
        let memory = MemoryStore::open(root).unwrap();
        let quiet = store.create_thread("quiet").unwrap();
        turn(&mut store, &memory, quiet, "question 0", "answer 0");
        turn(&mut store, &memory, quiet, "question 1", "answer 1");
        quiet
    };

    let mut store = ChatStore::open(root).unwrap();
    let second = store.user_turn_ordinal(quiet, 1).unwrap().unwrap();
    assert!(store.truncate_after(quiet, second - 1).unwrap());

    assert_eq!(scratchpad_rows(root, quiet), 2, "the first turn's scratchpad stays");
    assert_eq!(memory_written_after(root, quiet, second - 1), 0);
    assert_eq!(scratchpad_rows(root, busy), 8, "the other conversation keeps its memory");
}
