//! `chat.db` conversation-store fitness tests (S-168, [FR-UI-18], [ADR-40]).
//!
//! These drive a **real on-disk** `chat.db` in a throwaway directory so the
//! persistence assertions are genuine — a store opened, written, dropped, and
//! re-opened proves data survives a `serve --ui` restart / process bounce, the
//! S-168 acceptance criterion. The Clear-history and migration assertions ride
//! the same store.
//!
//! [FR-UI-18]: ../../docs/specs/requirements/FR-UI-18.md
//! [ADR-40]: ../../docs/specs/architecture/decisions/ADR-40.md

use chat_agent::db::{normalize_thread_title, title_from_first_message, THREAD_TITLE_MAX};
use chat_agent::{db, ChatRole, ChatStore, MemoryStore, ScratchpadEntry, ToolTrace};
use tempfile::TempDir;

fn trace(name: &str, is_error: bool) -> ToolTrace {
    ToolTrace {
        tool_name: name.to_string(),
        arguments: format!("{{\"q\":\"{name}\"}}"),
        result: if is_error {
            "tool failed".to_string()
        } else {
            format!("{{\"hits\":[\"{name}\"]}}")
        },
        is_error,
    }
}

/// Threads and messages — including tool-call/tool-result traces — persist
/// across a simulated `serve --ui` restart (the store is dropped and re-opened
/// from the same file). This is the core S-168 acceptance criterion.
#[test]
fn threads_and_messages_survive_a_restart() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let (thread_id, user_msg, assistant_msg) = {
        let mut store = ChatStore::open(root).unwrap();
        let thread_id = store.create_thread("Where is the binder?").unwrap();
        let user_msg = store
            .append_message(thread_id, ChatRole::User, "where is the binder?", &[])
            .unwrap();
        // An assistant message carrying two grounded tool traces — one ok, one
        // an honest tool failure (NFR-CC-04).
        let assistant_msg = store
            .append_message(
                thread_id,
                ChatRole::Assistant,
                "It lives in binder.rs.",
                &[trace("search", false), trace("read", true)],
            )
            .unwrap();
        (thread_id, user_msg, assistant_msg)
        // `store` is dropped here — the connection (and WAL) close.
    };

    // Re-open from the same path: a brand-new handle over the persisted file.
    let store = ChatStore::open(root).unwrap();

    let thread = store.thread(thread_id).unwrap().expect("thread persisted");
    assert_eq!(thread.title, "Where is the binder?");

    let messages = store.messages(thread_id).unwrap();
    assert_eq!(messages.len(), 2, "both messages survived the restart");

    assert_eq!(messages[0].id, user_msg);
    assert_eq!(messages[0].role, ChatRole::User);
    assert_eq!(messages[0].content, "where is the binder?");
    assert!(messages[0].tool_traces.is_empty());

    assert_eq!(messages[1].id, assistant_msg);
    assert_eq!(messages[1].role, ChatRole::Assistant);
    // Tool traces round-trip verbatim, in issue order, with the error flag.
    assert_eq!(messages[1].tool_traces, vec![trace("search", false), trace("read", true)]);
    assert!(!messages[1].tool_traces[0].is_error);
    assert!(messages[1].tool_traces[1].is_error);
}

/// `list_threads` returns most-recently-updated first; appending to an older
/// thread floats it back to the top (the conversation-list ordering).
#[test]
fn list_threads_is_most_recently_updated_first() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();

    let first = store.create_thread("first").unwrap();
    let second = store.create_thread("second").unwrap();

    // Newly-created `second` leads; tie-broken by id desc deterministically.
    let ids: Vec<i64> = store.list_threads().unwrap().iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![second, first]);

    // Touch `first`: its updated_at advances, so it must lead now. `unixepoch()`
    // is one-second resolution, so force the bump to a later second.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    store
        .append_message(first, ChatRole::User, "ping", &[])
        .unwrap();
    let ids: Vec<i64> = store.list_threads().unwrap().iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![first, second], "the touched thread floats to the top");
}

/// Clear-history wipes every conversation — threads, messages, and tool traces
/// — returning the store to an empty state (S-168 acceptance criterion).
#[test]
fn clear_history_empties_the_store() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();

    let t1 = store.create_thread("one").unwrap();
    store
        .append_message(t1, ChatRole::Assistant, "hi", &[trace("search", false)])
        .unwrap();
    let t2 = store.create_thread("two").unwrap();
    store.append_message(t2, ChatRole::User, "yo", &[]).unwrap();

    assert!(!store.is_empty().unwrap());

    let removed = store.clear_history().unwrap();
    assert_eq!(removed, 2, "both threads removed");

    assert!(store.is_empty().unwrap(), "store is empty after Clear-history");
    assert!(store.list_threads().unwrap().is_empty());
    assert!(store.thread(t1).unwrap().is_none());
    assert!(store.messages(t1).unwrap().is_empty(), "messages cascaded");

    // The wipe persists across a restart too — no resurrection from the WAL.
    drop(store);
    let store = ChatStore::open(dir.path()).unwrap();
    assert!(store.is_empty().unwrap());
}

/// The cascade is physical: deleting a thread removes its messages and their
/// tool traces, so no orphan child rows survive a Clear-history.
#[test]
fn clearing_cascades_to_messages_and_traces() {
    let dir = TempDir::new().unwrap();
    let path = db::db_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    let t = {
        let mut store = ChatStore::open_at(&path).unwrap();
        let t = store.create_thread("t").unwrap();
        store
            .append_message(t, ChatRole::Assistant, "a", &[trace("scan", false)])
            .unwrap();
        store.clear_history().unwrap();
        t
    };

    // Inspect the raw tables with a separate connection: every child table is
    // empty after the wipe.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    for table in ["chat_threads", "chat_messages", "chat_tool_traces"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table} must be empty after Clear-history");
    }

    // Prove the emptiness was driven by a LIVE foreign-key cascade off the
    // parent delete — not by some unrelated child-table delete. With
    // `foreign_keys = ON`, inserting a message for a now-absent thread must be
    // refused by the constraint; if the FK were not enforced this insert would
    // succeed and the test would (correctly) fail.
    let orphan = conn.execute(
        "INSERT INTO chat_messages (thread_id, ordinal, role, content, created_at)
         VALUES (?1, 0, 'user', 'orphan', 0)",
        [t],
    );
    assert!(
        orphan.is_err(),
        "the FK must reject a message whose parent thread is gone — the cascade is live"
    );
}

/// **S-208 core acceptance:** `delete_thread(id)` is one `DELETE FROM
/// chat_threads` whose live cascade wipes that thread's messages, tool traces,
/// scratchpad, and working memory — leaving **zero orphan rows** across every
/// child table — while a sibling conversation is untouched (the delete is scoped
/// to the id, [FR-UI-26]/[FR-UI-20]).
#[test]
fn delete_thread_cascades_leaving_no_orphans() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let path = db::db_path(root);

    // Two conversations, each with messages+traces and per-thread memory
    // (scratchpad + working memory, written through the memory tier's own
    // connection to the same chat.db — migration 2's cascade tables).
    let (doomed, survivor) = {
        let mut store = ChatStore::open(root).unwrap();
        let memory = MemoryStore::open(root).unwrap();

        let seed = |store: &mut ChatStore, title: &str| -> i64 {
            let id = store.create_thread(title).unwrap();
            store
                .append_message(id, ChatRole::User, "q", &[trace("search", false)])
                .unwrap();
            store
                .append_message(id, ChatRole::Assistant, "a", &[trace("read", true)])
                .unwrap();
            let turn = memory.next_turn(id).unwrap();
            memory
                .record(id, turn, &ScratchpadEntry::Note { note: "thinking".into() })
                .unwrap();
            memory.set_working_memory(id, "running summary").unwrap();
            id
        };

        let doomed = seed(&mut store, "doomed");
        let survivor = seed(&mut store, "survivor");

        // Prove `seed` actually populated the DOOMED thread's memory tiers before
        // the delete, so the post-delete zero-counts below assert a real cascade
        // rather than a thread that never had child rows in the first place.
        {
            let pre = rusqlite::Connection::open(&path).unwrap();
            let n = |sql: &str| -> i64 { pre.query_row(sql, [doomed], |r| r.get(0)).unwrap() };
            assert_eq!(n("SELECT COUNT(*) FROM chat_messages WHERE thread_id = ?1"), 2);
            assert_eq!(n("SELECT COUNT(*) FROM chat_scratchpad WHERE thread_id = ?1"), 1);
            assert_eq!(n("SELECT COUNT(*) FROM chat_working_memory WHERE thread_id = ?1"), 1);
        }

        let removed = store.delete_thread(doomed).unwrap();
        assert!(removed, "delete_thread reports the thread was removed");
        (doomed, survivor)
    };

    // Inspect the raw tables with a separate connection: not one row keyed to the
    // deleted thread survives in ANY child table (messages, traces, scratchpad,
    // working memory) — the no-orphans acceptance criterion.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    let count_for = |conn: &rusqlite::Connection, sql: &str, id: i64| -> i64 {
        conn.query_row(sql, [id], |r| r.get(0)).unwrap()
    };

    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_threads WHERE id = ?1", doomed),
        0,
        "the thread row is gone"
    );
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_messages WHERE thread_id = ?1", doomed),
        0,
        "chat_messages: no orphan rows for the deleted thread"
    );
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_scratchpad WHERE thread_id = ?1", doomed),
        0,
        "chat_scratchpad: no orphan rows for the deleted thread"
    );
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_working_memory WHERE thread_id = ?1", doomed),
        0,
        "chat_working_memory: no orphan rows for the deleted thread"
    );
    // Tool traces cascade transitively through their (now-deleted) messages.
    let orphan_traces: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_tool_traces t
             LEFT JOIN chat_messages m ON m.id = t.message_id
             WHERE m.id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphan_traces, 0, "chat_tool_traces: no traces orphaned by the delete");

    // The sibling conversation is fully intact — delete is scoped to the id.
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_threads WHERE id = ?1", survivor),
        1
    );
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_messages WHERE thread_id = ?1", survivor),
        2,
        "the surviving thread keeps its messages"
    );
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_scratchpad WHERE thread_id = ?1", survivor),
        1
    );
    assert_eq!(
        count_for(&conn, "SELECT COUNT(*) FROM chat_working_memory WHERE thread_id = ?1", survivor),
        1
    );
}

/// `delete_thread` reports whether a row was removed and is idempotent: deleting
/// a present thread returns `true`; deleting an absent (or already-deleted) id is
/// a harmless `false`, never an error — the shape the per-thread delete API maps
/// to 200-vs-404.
#[test]
fn delete_thread_reports_removal_and_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();

    let id = store.create_thread("gone soon").unwrap();
    assert!(store.delete_thread(id).unwrap(), "first delete removes the thread");
    assert!(
        !store.delete_thread(id).unwrap(),
        "a second delete of the same id is a no-op, not an error"
    );
    assert!(
        !store.delete_thread(4242).unwrap(),
        "deleting an id that never existed reports false"
    );
    assert!(store.thread(id).unwrap().is_none());
}

/// **S-208 acceptance:** a new conversation is auto-titled from its first user
/// message (truncated), and that title is **stable** — later turns appended to
/// the thread never rewrite it ([FR-UI-26], [ADR-47]).
#[test]
fn auto_title_from_first_message_is_stable_across_turns() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();

    let first = "  Where does the binder live in this codebase?  ";
    let id = store.create_thread_from_message(first).unwrap();

    let created_title = store.thread(id).unwrap().unwrap().title;
    assert_eq!(
        created_title,
        title_from_first_message(first),
        "the conversation is born with the canonical truncated title"
    );

    // Later turns (the user's follow-ups and the assistant's replies) must not
    // touch the title — it is fixed at creation.
    store
        .append_message(id, ChatRole::User, first, &[])
        .unwrap();
    store
        .append_message(id, ChatRole::Assistant, "It lives in binder.rs.", &[])
        .unwrap();
    store
        .append_message(id, ChatRole::User, "and the parser?", &[])
        .unwrap();

    assert_eq!(
        store.thread(id).unwrap().unwrap().title,
        created_title,
        "the auto-title is stable across later turns"
    );
}

/// The title-derivation contract [`title_from_first_message`] must honor — the
/// cleaned first message [FR-UI-48] falls back to: trimmed, whitespace collapsed,
/// sentence case, single-line, capped at [`THREAD_TITLE_MAX`] characters at a word
/// boundary (a char cut, never a byte cut that could panic on / corrupt a
/// multi-byte message, when one word outruns the cap), with a non-empty fallback
/// for a blank message (`chat_threads.title` is `NOT NULL`).
///
/// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
#[test]
fn title_from_first_message_honors_the_cleaning_contract() {
    // A short, padded message is trimmed to its text, in sentence case.
    assert_eq!(title_from_first_message("  hello world  "), "Hello world");
    // Whitespace runs collapse; only the FIRST letter is cased, so a code name
    // keeps its spelling.
    assert_eq!(
        title_from_first_message("where   is\tEngine::run   defined?"),
        "Where is Engine::run defined?"
    );
    // Leading blank lines are skipped: the title is the first line with text.
    assert_eq!(
        title_from_first_message("\n  \n how does the binder work?\nmore"),
        "How does the binder work?"
    );
    // An already-capitalised or non-letter start is left as it is.
    assert_eq!(title_from_first_message("`binder.rs` callers"), "`binder.rs` callers");

    // A single-line title: a multi-line paste is titled by its first line only.
    let multiline = title_from_first_message("first line\nsecond line\nthird");
    assert!(!multiline.contains('\n'), "title is single-line, got {multiline:?}");

    // A blank / whitespace-only message falls back to a non-empty title.
    assert!(
        !title_from_first_message("").trim().is_empty(),
        "an empty message must still yield a non-empty title"
    );
    assert!(
        !title_from_first_message("   \t  ").trim().is_empty(),
        "a whitespace-only message must still yield a non-empty title"
    );

    // A single word longer than the cap has no boundary to cut at: it is cut to
    // exactly THREAD_TITLE_MAX *characters* — pinning the content, not just the
    // length, guards against an off-by-one or wrong-prefix bug in the char take.
    let long = "x".repeat(THREAD_TITLE_MAX * 3);
    assert_eq!(
        title_from_first_message(&long),
        format!("X{}", "x".repeat(THREAD_TITLE_MAX - 1)),
        "an unbroken long title is its first THREAD_TITLE_MAX characters"
    );

    // Exact boundary: a message of exactly THREAD_TITLE_MAX chars is kept whole;
    // one char longer loses exactly the final char.
    let exact = format!("A{}", "a".repeat(THREAD_TITLE_MAX - 1));
    assert_eq!(title_from_first_message(&exact), exact, "exactly MAX chars is kept whole");
    let over_by_one = "a".repeat(THREAD_TITLE_MAX + 1);
    assert_eq!(
        title_from_first_message(&over_by_one),
        format!("A{}", "a".repeat(THREAD_TITLE_MAX - 1)),
        "MAX+1 chars truncates to exactly MAX"
    );

    // Words over the cap are cut at the last word boundary inside it — never
    // mid-word — and a cap that lands exactly on a space keeps the whole last word.
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo";
    let cut = title_from_first_message(words);
    assert_eq!(cut, "Alpha bravo charlie delta echo foxtrot golf hotel india");
    assert!(cut.chars().count() <= THREAD_TITLE_MAX);
    let on_space = format!("{} tail", "b".repeat(THREAD_TITLE_MAX));
    assert_eq!(
        title_from_first_message(&on_space),
        format!("B{}", "b".repeat(THREAD_TITLE_MAX - 1)),
        "a cap that falls on a word boundary keeps the word before it whole"
    );
    // MAX-2 chars, a space, then a two-letter word: MAX+1 chars, the last word
    // straddling the cap by one char — it is dropped whole, never cut to "c".
    let near = format!("{} cd", "a".repeat(THREAD_TITLE_MAX - 2));
    assert_eq!(
        title_from_first_message(&near),
        format!("A{}", "a".repeat(THREAD_TITLE_MAX - 3)),
        "a word straddling the cap by one char is dropped whole"
    );

    // A multi-byte message over the cap must truncate on a char boundary — no
    // panic, still capped by character count (never by raw byte offset).
    let multibyte = "日本語のテキストです。".repeat(20);
    let title = title_from_first_message(&multibyte);
    assert!(
        title.chars().count() <= THREAD_TITLE_MAX,
        "multi-byte titles are capped by character count, not byte length"
    );
}

/// S-208's per-thread delete + auto-title rode the existing multi-thread tables
/// with **no new migration** ([ADR-47] "no migration"), as did S-623's rename. The
/// head of the ledger is migration 3, S-624's memory attribution for truncation
/// (`tests/truncate.rs`) — the only migration since the multi-step memory of [S-175].
#[test]
fn latest_version_is_unchanged_no_migration() {
    assert_eq!(db::latest_version(), 3, "only S-624 has migrated since S-175");
}

/// Appending to a non-existent thread is rejected (no orphaned message) rather
/// than silently succeeding.
#[test]
fn appending_to_a_missing_thread_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let err = store
        .append_message(999, ChatRole::User, "ghost", &[])
        .unwrap_err();
    assert!(err.to_string().contains("no chat thread"), "got: {err}");
}

/// A fully-migrated `chat.db` reports the latest schema version on its own
/// independent track (the shared forward-only migration discipline).
#[test]
fn store_reports_latest_schema_version() {
    let dir = TempDir::new().unwrap();
    let path = db::db_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    drop(ChatStore::open_at(&path).unwrap());

    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::latest_version());
    assert!(db::latest_version() >= 1);
}

/// `ChatRole` round-trips through its stored string form, and an unknown stored
/// value is surfaced as an error rather than coerced (NFR-CC-04).
#[test]
fn chat_role_round_trips_and_rejects_garbage() {
    for role in [ChatRole::User, ChatRole::Assistant, ChatRole::System, ChatRole::Tool] {
        assert_eq!(role.as_str().parse::<ChatRole>().unwrap(), role);
    }
    assert!("wizard".parse::<ChatRole>().is_err());
}

/// A fresh store is empty, and re-opening an already-migrated `chat.db` (the
/// common `serve --ui` process-bounce path) is idempotent: the migration
/// ledger's `version <= current` guard skips applied migrations, so the second
/// open neither re-runs migration 1 (which would error on `CREATE TABLE … already
/// exists`) nor records a duplicate ledger row.
#[test]
fn reopening_a_migrated_store_is_idempotent_and_starts_empty() {
    let dir = TempDir::new().unwrap();

    // First open creates + migrates; a brand-new store holds no conversations.
    {
        let store = ChatStore::open(dir.path()).unwrap();
        assert!(store.is_empty().unwrap(), "a fresh store is empty before any write");
    }
    // Second open over the already-migrated file must succeed (no re-run).
    {
        let store = ChatStore::open(dir.path()).unwrap();
        assert!(store.is_empty().unwrap());
    }

    let conn = rusqlite::Connection::open(db::db_path(dir.path())).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::latest_version());
    // Each migration is recorded exactly once — a re-run would have inserted a
    // second `schema_versions` row (dense 1..=latest ⇒ count == latest_version).
    let recorded: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_versions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        recorded,
        db::latest_version(),
        "each migration recorded once — reopen did not re-apply"
    );
}

/// Message order follows the monotonic `ordinal` across more than one increment,
/// and a message's tool traces come back in issue order regardless of name —
/// guarding the `ORDER BY ordinal` reads against a shuffle or an ordinal-reset
/// regression that a two-element check could miss.
#[test]
fn message_and_trace_order_is_insertion_order() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let t = store.create_thread("ordering").unwrap();

    for c in ["a", "b", "c", "d", "e"] {
        store.append_message(t, ChatRole::User, c, &[]).unwrap();
    }
    // Tool traces issued in deliberately non-alphabetical order.
    let traces = [trace("c", false), trace("a", true), trace("b", false)];
    store
        .append_message(t, ChatRole::Assistant, "answer", &traces)
        .unwrap();

    let messages = store.messages(t).unwrap();
    let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        contents,
        vec!["a", "b", "c", "d", "e", "answer"],
        "messages return in insertion (ordinal) order across many increments"
    );

    let names: Vec<&str> = messages
        .last()
        .unwrap()
        .tool_traces
        .iter()
        .map(|t| t.tool_name.as_str())
        .collect();
    assert_eq!(names, vec!["c", "a", "b"], "traces preserve issue order, not name order");
}

/// **S-623 / [FR-UI-48]:** a conversation can be renamed, the rename persists
/// across a store reopen (a `serve --ui` restart), it leaves the rail order alone,
/// and an empty or over-long name is refused with the title unchanged.
///
/// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
#[test]
fn rename_thread_persists_across_a_reopen_and_refuses_an_empty_name() {
    let dir = TempDir::new().unwrap();
    let (older, newer) = {
        let mut store = ChatStore::open(dir.path()).unwrap();
        let older = store.create_thread_from_message("where is the binder?").unwrap();
        let newer = store.create_thread_from_message("what is risky here?").unwrap();
        assert!(store.rename_thread(older, "  Binder\n location  ").unwrap(), "a hit renames");
        (older, newer)
    };

    let mut store = ChatStore::open(dir.path()).unwrap();
    assert_eq!(
        store.thread(older).unwrap().unwrap().title,
        "Binder location",
        "the rename is durable and normalized"
    );
    let order: Vec<i64> = store.list_threads().unwrap().iter().map(|t| t.id).collect();
    assert_eq!(order, [newer, older], "renaming is not activity: the rail order is unchanged");

    for refused in ["", "   \t\n ", &"x".repeat(THREAD_TITLE_MAX + 1)] {
        assert!(store.rename_thread(older, refused).is_err(), "{refused:?} is refused");
    }
    assert_eq!(store.thread(older).unwrap().unwrap().title, "Binder location", "title unchanged");

    assert!(!store.rename_thread(9_999, "Ghost").unwrap(), "an unknown id renames nothing");
}

/// The one shaping rule a rename and a Synthesizer title share.
#[test]
fn normalize_thread_title_collapses_whitespace_and_bounds_length() {
    assert_eq!(normalize_thread_title(" a \n b\t c ").as_deref(), Some("a b c"));
    assert_eq!(normalize_thread_title(""), None);
    assert_eq!(normalize_thread_title(" \n "), None);
    let exact = "y".repeat(THREAD_TITLE_MAX);
    assert_eq!(normalize_thread_title(&exact).as_deref(), Some(exact.as_str()));
    assert_eq!(normalize_thread_title(&"y".repeat(THREAD_TITLE_MAX + 1)), None);
}

/// **S-623 / [FR-UI-48]:** the first turn's title replaces only the title the
/// thread was created with — a rename made meanwhile is kept.
///
/// [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md
#[test]
fn replace_title_only_replaces_the_expected_title() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let id = store.create_thread_from_message("where is the binder?").unwrap();
    let fallback = title_from_first_message("where is the binder?");

    assert!(store.replace_title(id, &fallback, "  Binder   map ").unwrap());
    assert_eq!(store.thread(id).unwrap().unwrap().title, "Binder map", "normalized");

    store.rename_thread(id, "My own name").unwrap();
    assert!(
        !store.replace_title(id, "Binder map", "Generated").unwrap(),
        "a title that is no longer the expected one is not replaced"
    );
    assert_eq!(store.thread(id).unwrap().unwrap().title, "My own name");
    assert!(store.replace_title(id, "My own name", "").is_err(), "an empty title is refused");
    assert!(!store.replace_title(9_999, "x", "Ghost").unwrap(), "an unknown id replaces nothing");
}
