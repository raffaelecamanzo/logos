//! `chat.db` — the agentic-chat conversation store ([FR-UI-18], [ADR-40],
//! [ADR-13] reapplied a fifth time).
//!
//! Deliberately a **fifth SQLite file** under `.logos/`, beside `logos.db`,
//! `telemetry.db`, `history.db`, and `wiki.db`. The reasons are the
//! [ADR-13]/[ADR-22]/[ADR-24] reasons one more time, sharpened by [ADR-40]:
//!
//! - A full `index` rebuilds `logos.db` wholesale; a conversation is **user
//!   data and not re-derivable** by Logos, so a separate file is simply never
//!   touched by the rebuild ([ADR-40] reversibility).
//! - The store is **never `ATTACH`-ed** to `logos.db` and the gated metric path
//!   holds no connection to it, so "the quality gate cannot see chat history"
//!   is a physical property, not a coding convention.
//! - It is **`ui`-gated**: this crate reaches the `logos` binary only through
//!   the `ui`-only [`web`] adapter, so removing `--features ui` yields exactly
//!   today's offline binary — no HTTP client, no secret, and no `chat.db`
//!   ([ADR-40] "Reversibility", [NFR-SE-01]).
//! - Its migration track is **independent**: `chat.db` starts at
//!   `user_version = 1` and advances on its own, regardless of `logos.db`'s
//!   version (the shared store discipline).
//!
//! Schema (migration 1 — the conversation substrate the chat surface, the
//! multi-step memory ([S-175]), and the UI ([S-171]) ride):
//! - `chat_threads` — one row per conversation. `updated_at` is bumped on every
//!   appended message so `list_threads` can surface most-recent-first.
//! - `chat_messages` — one row per turn message, ordered within a thread by a
//!   monotonic `ordinal` (not `created_at`, which can collide at one-second
//!   resolution). `role` is the speaker (`user`/`assistant`/`system`/`tool`).
//!   `ON DELETE CASCADE` on `thread_id` is what makes Clear-history a single
//!   `DELETE FROM chat_threads`.
//! - `chat_tool_traces` — zero or more rows per message, each one a recorded
//!   **tool-call/tool-result** pair (the [FR-UI-18] grounded-tool trace): the
//!   `tool_name` + `arguments` the agent issued and the `result` it observed,
//!   with `is_error` carrying the honest tool-failure flag ([NFR-CC-04] — a
//!   failed tool is recorded as failed, never papered over). Cascades off its
//!   message so Clear-history wipes traces too.
//!
//! Migrations follow the same forward-only `user_version` discipline as
//! [`logos-core`'s `graph_store`], `history::db`, and `wiki::db` — one
//! transaction per migration, all-or-nothing ([NFR-RA-07]).
//!
//! [FR-UI-18]: ../../docs/specs/requirements/FR-UI-18.md
//! [NFR-SE-01]: ../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-07]: ../../docs/specs/requirements/NFR-RA-07.md
//! [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
//! [ADR-22]: ../../docs/specs/architecture/decisions/ADR-22.md
//! [ADR-24]: ../../docs/specs/architecture/decisions/ADR-24.md
//! [ADR-40]: ../../docs/specs/architecture/decisions/ADR-40.md
//! [S-171]: ../../docs/planning/journal.md#s-171-chat-tab-view-conversation-ui-streaming-and-consent-banner
//! [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// Forward-only migration ledger for `chat.db` — dense, 1-based, strictly
/// increasing (mirrors `wiki::db::MIGRATIONS`). Migration 1 is **never** edited;
/// later chat stories (e.g. the multi-step memory of [S-175]) append migration
/// 2+ on this same track.
///
/// [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
const MIGRATIONS: &[(i64, &str)] = &[(
    1,
    "CREATE TABLE schema_versions (
         version    INTEGER PRIMARY KEY,
         applied_at INTEGER NOT NULL
     ) STRICT;

     -- One row per conversation. `updated_at` is bumped whenever a message is
     -- appended, so the conversation list (FR-UI-18) is most-recent-first
     -- without a join. Both timestamps are unix seconds (provenance/ordering).
     CREATE TABLE chat_threads (
         id         INTEGER PRIMARY KEY,
         title      TEXT    NOT NULL,
         created_at INTEGER NOT NULL,
         updated_at INTEGER NOT NULL
     ) STRICT;

     -- One row per message. Ordered within a thread by the monotonic `ordinal`
     -- (NOT `created_at` — one-second resolution can collide inside a turn).
     -- `role` is the speaker label. ON DELETE CASCADE on `thread_id` makes
     -- Clear-history a single `DELETE FROM chat_threads`.
     CREATE TABLE chat_messages (
         id         INTEGER PRIMARY KEY,
         thread_id  INTEGER NOT NULL REFERENCES chat_threads(id) ON DELETE CASCADE,
         ordinal    INTEGER NOT NULL,            -- monotonic order within a thread
         role       TEXT    NOT NULL,            -- 'user'|'assistant'|'system'|'tool'
         content    TEXT    NOT NULL,
         created_at INTEGER NOT NULL             -- unix seconds (provenance)
     ) STRICT;
     CREATE INDEX chat_messages_thread ON chat_messages (thread_id, ordinal);

     -- Zero or more tool-call/tool-result traces per message, in issue order
     -- (`ordinal`). Each row is one grounded tool invocation: the `tool_name` +
     -- `arguments` the agent issued and the `result` it observed; `is_error`
     -- (0/1) carries the honest tool-failure flag (NFR-CC-04). Cascades off its
     -- message so Clear-history wipes traces with their messages.
     CREATE TABLE chat_tool_traces (
         id         INTEGER PRIMARY KEY,
         message_id INTEGER NOT NULL REFERENCES chat_messages(id) ON DELETE CASCADE,
         ordinal    INTEGER NOT NULL,            -- issue order within a message
         tool_name  TEXT    NOT NULL,
         arguments  TEXT    NOT NULL,            -- serialized tool-call input
         result     TEXT    NOT NULL,            -- serialized tool-result/observation
         is_error   INTEGER NOT NULL,            -- 0|1 honest tool-failure flag
         created_at INTEGER NOT NULL             -- unix seconds (provenance)
     ) STRICT;
     CREATE INDEX chat_tool_traces_message ON chat_tool_traces (message_id, ordinal);",
), (
    // Migration 2 — the multi-step agent memory ([S-175], [FR-UI-20]) on this
    // same `chat.db` track (no new DB file, [ADR-41]). Two tiers, both reached
    // through [`crate::memory::MemoryStore`]:
    //
    // - `chat_scratchpad` — the **per-turn scratchpad**: the planner's plan,
    //   each specialized subagent's observation, and intermediate findings the
    //   planner and the Synthesizer read across steps within ONE orchestrated
    //   turn. Scoped to `(thread_id, turn)` — `turn` is a per-thread monotonic
    //   ordinal (one orchestrated run = one turn); entries are ordered within a
    //   turn by the monotonic `ordinal`. `payload` is the verbatim JSON of the
    //   typed `ScratchpadEntry` so an honest observation — a tool failure, an
    //   empty result, an honest budget halt — is recorded as-is, never papered
    //   over (NFR-CC-04). Plain relational rows: **no embeddings, no vector
    //   index, no RAG** (FR-UI-20 v1 constraint).
    // - `chat_working_memory` — the **per-thread working/conversation memory**:
    //   a running summary carried across turns so a follow-up turn sees prior
    //   context after a `serve --ui` restart. One row per thread (PK =
    //   `thread_id`); also plain text, no semantic store.
    //
    // Both `ON DELETE CASCADE` off `chat_threads`, so Clear-history (the single
    // `DELETE FROM chat_threads` of [`ChatStore::clear_history`]) wipes a
    // thread's memory together with its messages — no orphaned memory survives
    // (FR-UI-20). The migration follows the same forward-only `user_version`
    // discipline as migration 1; migration 1 is never edited.
    2,
    "-- Per-turn scratchpad: plan + per-subagent observations + intermediate
     -- findings, scoped to (thread_id, turn). See the migration-2 doc comment.
     CREATE TABLE chat_scratchpad (
         id         INTEGER PRIMARY KEY,
         thread_id  INTEGER NOT NULL REFERENCES chat_threads(id) ON DELETE CASCADE,
         turn       INTEGER NOT NULL,            -- per-thread turn ordinal (one run)
         ordinal    INTEGER NOT NULL,            -- entry order within a turn
         kind       TEXT    NOT NULL,            -- 'plan'|'observation'|'note'|'final_answer'
         payload    TEXT    NOT NULL,            -- verbatim JSON of the ScratchpadEntry
         created_at INTEGER NOT NULL             -- unix seconds (provenance)
     ) STRICT;
     CREATE INDEX chat_scratchpad_turn ON chat_scratchpad (thread_id, turn, ordinal);

     -- Per-thread working/conversation memory: one running-summary row per
     -- thread. Plain text — no embeddings/RAG (FR-UI-20 v1).
     CREATE TABLE chat_working_memory (
         thread_id  INTEGER PRIMARY KEY REFERENCES chat_threads(id) ON DELETE CASCADE,
         summary    TEXT    NOT NULL,
         updated_at INTEGER NOT NULL             -- unix seconds (last write)
     ) STRICT;",
)];

/// The conversational role of a [`ChatMessage`].
///
/// Persisted as the lowercase string in the `role` column. An unknown stored
/// value is an error on read, never silently coerced ([NFR-CC-04]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    /// A message from the human.
    User,
    /// A message from the assistant/agent.
    Assistant,
    /// A system/priming message.
    System,
    /// A tool-role message (a tool result folded back into the transcript).
    Tool,
}

impl ChatRole {
    /// The stored string form (the value written to the `role` column).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::System => "system",
            ChatRole::Tool => "tool",
        }
    }

}

impl std::str::FromStr for ChatRole {
    type Err = anyhow::Error;

    /// Parse a stored `role` string back into a [`ChatRole`]. Any value the
    /// store never writes is surfaced as an error, not coerced to a default — a
    /// corrupt row is reported honestly ([NFR-CC-04]).
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(ChatRole::User),
            "assistant" => Ok(ChatRole::Assistant),
            "system" => Ok(ChatRole::System),
            "tool" => Ok(ChatRole::Tool),
            other => anyhow::bail!("unknown chat message role {other:?}"),
        }
    }
}

/// One recorded tool-call/tool-result pair attached to a message ([FR-UI-18]).
///
/// `arguments` and `result` are stored verbatim as the caller serialized them
/// (typically JSON); this store imposes no shape on them. `is_error` is the
/// honest tool-failure flag ([NFR-CC-04]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolTrace {
    /// The tool the agent invoked.
    pub tool_name: String,
    /// The serialized tool-call input (verbatim, typically JSON).
    pub arguments: String,
    /// The serialized tool-result/observation (verbatim).
    pub result: String,
    /// Whether the tool reported an error — recorded honestly ([NFR-CC-04]).
    pub is_error: bool,
}

/// A persisted message plus its tool traces, as loaded for a read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    /// The message rowid.
    pub id: i64,
    /// The speaker.
    pub role: ChatRole,
    /// The message text.
    pub content: String,
    /// Unix-seconds recording time (provenance).
    pub created_at: i64,
    /// The tool-call/tool-result traces issued by this message, in issue order.
    pub tool_traces: Vec<ToolTrace>,
}

/// A conversation's metadata row (no messages — load those with
/// [`ChatStore::messages`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatThread {
    /// The thread rowid (stable for the life of the store).
    pub id: i64,
    /// The conversation title.
    pub title: String,
    /// Unix-seconds creation time.
    pub created_at: i64,
    /// Unix-seconds time of the most recent appended message (or creation).
    pub updated_at: i64,
}

/// The `.logos/chat.db` conversation store ([FR-UI-18], [ADR-40]).
///
/// Owns one migrated [`Connection`]. Read methods take `&self`; write methods
/// take `&mut self` because they open a transaction. Conversations persist
/// across `serve --ui` restarts and a process bounce because the data lives in
/// the on-disk file, not in this handle.
#[derive(Debug)]
pub struct ChatStore {
    conn: Connection,
}

impl ChatStore {
    /// Open (creating if absent) and migrate `.logos/chat.db` under `root`.
    ///
    /// The parent `.logos/` directory is created if missing so the store works
    /// out of the box (it is `Engine::init`'s directory, but the chat surface
    /// must not crash if reached before a full `init`).
    ///
    /// # Errors
    /// Returns an error if the directory or file cannot be created/opened or a
    /// migration fails.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_at(&ensure_db_dir(root)?)
    }

    /// Open (creating if absent) and migrate `chat.db` at an explicit `path`.
    ///
    /// Applies the same pragma contract as the other stores: WAL journalling, a
    /// busy timeout, and `foreign_keys = ON` so the thread → message → trace
    /// cascade is live (Clear-history relies on it).
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or a migration fails.
    pub fn open_at(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: open_migrated(path)?,
        })
    }

    /// Create a new conversation, returning its stable id.
    ///
    /// `created_at` and `updated_at` start equal (no messages yet).
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn create_thread(&mut self, title: &str) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO chat_threads (title, created_at, updated_at)
                 VALUES (?1, unixepoch(), unixepoch())",
                rusqlite::params![title],
            )
            .with_context(|| format!("creating chat thread {title:?}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Append a message (and its tool traces) to a thread in **one**
    /// transaction, bumping the thread's `updated_at`. Returns the new message
    /// id. The `ordinal` is assigned as the thread's current max + 1, so message
    /// order is stable regardless of clock resolution.
    ///
    /// # Errors
    /// Returns an error if the thread does not exist or the transaction cannot
    /// commit.
    pub fn append_message(
        &mut self,
        thread_id: i64,
        role: ChatRole,
        content: &str,
        tool_traces: &[ToolTrace],
    ) -> Result<i64> {
        let tx = self
            .conn
            .transaction()
            .context("opening the chat append transaction")?;

        // Reject an append to a missing thread rather than silently orphaning a
        // message (the FK has no parent to cascade from on a wipe).
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM chat_threads WHERE id = ?1",
                [thread_id],
                |_| Ok(true),
            )
            .optional()
            .with_context(|| format!("checking chat thread {thread_id}"))?
            .unwrap_or(false);
        anyhow::ensure!(exists, "no chat thread with id {thread_id}");

        let next_ordinal: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM chat_messages WHERE thread_id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .with_context(|| format!("computing next message ordinal for thread {thread_id}"))?;

        tx.execute(
            "INSERT INTO chat_messages (thread_id, ordinal, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())",
            rusqlite::params![thread_id, next_ordinal, role.as_str(), content],
        )
        .with_context(|| format!("inserting message into thread {thread_id}"))?;
        let message_id = tx.last_insert_rowid();

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO chat_tool_traces
                         (message_id, ordinal, tool_name, arguments, result, is_error, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())",
                )
                .context("preparing the tool-trace insert")?;
            for (ordinal, trace) in tool_traces.iter().enumerate() {
                stmt.execute(rusqlite::params![
                    message_id,
                    ordinal as i64,
                    trace.tool_name,
                    trace.arguments,
                    trace.result,
                    i64::from(trace.is_error),
                ])
                .with_context(|| {
                    format!("inserting tool trace {} of message {message_id}", trace.tool_name)
                })?;
            }
        }

        tx.execute(
            "UPDATE chat_threads SET updated_at = unixepoch() WHERE id = ?1",
            [thread_id],
        )
        .with_context(|| format!("bumping updated_at of thread {thread_id}"))?;

        tx.commit().context("committing the chat append")?;
        Ok(message_id)
    }

    /// Load one thread's metadata by id, or `None` if no such thread exists.
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure.
    pub fn thread(&self, thread_id: i64) -> Result<Option<ChatThread>> {
        self.conn
            .query_row(
                "SELECT id, title, created_at, updated_at FROM chat_threads WHERE id = ?1",
                [thread_id],
                map_thread_row,
            )
            .optional()
            .with_context(|| format!("loading chat thread {thread_id}"))
    }

    /// Every thread, most-recently-updated first then by id — the conversation
    /// list the UI renders ([FR-UI-18]).
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure.
    pub fn list_threads(&self) -> Result<Vec<ChatThread>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT id, title, created_at, updated_at FROM chat_threads
                 ORDER BY updated_at DESC, id DESC",
            )
            .context("preparing the thread list")?;
        let rows = stmt
            .query_map([], map_thread_row)
            .context("listing chat threads")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting chat threads")
    }

    /// Every message of a thread in stored `ordinal` order, each with its tool
    /// traces (in their own issue order) — the deterministic transcript the UI
    /// and the multi-step memory ([S-175]) read.
    ///
    /// [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure or a corrupt `role`.
    pub fn messages(&self, thread_id: i64) -> Result<Vec<ChatMessage>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT id, role, content, created_at FROM chat_messages
                 WHERE thread_id = ?1 ORDER BY ordinal",
            )
            .context("preparing the message load")?;
        let rows = stmt
            .query_map([thread_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .with_context(|| format!("loading messages of thread {thread_id}"))?;

        let mut messages = Vec::new();
        for row in rows {
            let (id, role, content, created_at) = row.context("reading a message row")?;
            messages.push(ChatMessage {
                id,
                role: role.parse()?,
                content,
                created_at,
                tool_traces: self.tool_traces(id)?,
            });
        }
        Ok(messages)
    }

    /// The tool traces of one message, in stored `ordinal` order.
    fn tool_traces(&self, message_id: i64) -> Result<Vec<ToolTrace>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT tool_name, arguments, result, is_error FROM chat_tool_traces
                 WHERE message_id = ?1 ORDER BY ordinal",
            )
            .context("preparing the tool-trace load")?;
        let rows = stmt
            .query_map([message_id], |row| {
                Ok(ToolTrace {
                    tool_name: row.get(0)?,
                    arguments: row.get(1)?,
                    result: row.get(2)?,
                    is_error: row.get::<_, i64>(3)? != 0,
                })
            })
            .with_context(|| format!("loading tool traces of message {message_id}"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collecting tool traces")
    }

    /// **Clear history** ([FR-UI-18]): wipe every conversation and return the
    /// store to an empty state. One `DELETE FROM chat_threads` cascades through
    /// `chat_messages` → `chat_tool_traces` (the FK contract), so no child rows
    /// survive. Returns the number of threads removed.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn clear_history(&mut self) -> Result<usize> {
        let removed = self
            .conn
            .execute("DELETE FROM chat_threads", [])
            .context("clearing chat history")?;
        Ok(removed)
    }

    /// Whether the store holds no conversations — the post-Clear-history /
    /// fresh-store assertion ([FR-UI-18]).
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure.
    pub fn is_empty(&self) -> Result<bool> {
        let threads: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chat_threads", [], |row| row.get(0))
            .context("counting chat threads")?;
        Ok(threads == 0)
    }
}

/// The resolved `.logos/chat.db` path for a worktree `root`.
#[must_use]
pub fn db_path(root: &Path) -> PathBuf {
    root.join(".logos").join("chat.db")
}

/// Resolve the `.logos/chat.db` path under `root`, creating the parent `.logos/`
/// directory if absent so the store works out of the box. The single home of the
/// "ensure the chat store directory exists" logic, shared by [`ChatStore::open`]
/// and [`crate::memory::MemoryStore::open`] (both must work standalone before a
/// full `init`).
///
/// # Errors
/// Returns an error if the directory cannot be created.
pub(crate) fn ensure_db_dir(root: &Path) -> Result<PathBuf> {
    let path = db_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating the chat store directory {}", parent.display()))?;
    }
    Ok(path)
}

/// Map a `(id, title, created_at, updated_at)` row to a [`ChatThread`] — the
/// shared projection behind [`ChatStore::thread`] and [`ChatStore::list_threads`]
/// so the column order lives in exactly one place (mirrors `wiki::db`'s
/// `load_anchors` decomposition).
fn map_thread_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatThread> {
    Ok(ChatThread {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

/// Open `chat.db` at `path`, apply the shared store pragma contract (WAL,
/// `NORMAL` sync, **`foreign_keys = ON`** so the thread → message/scratchpad/…
/// cascades are live, a busy timeout), and migrate it through [`MIGRATIONS`].
///
/// The one place the `chat.db` open contract lives, shared by [`ChatStore`] and
/// [`crate::memory::MemoryStore`] so a second connection to the same file (WAL
/// admits many) is migrated identically and the FK cascade Clear-history relies
/// on is enabled on every handle.
///
/// # Errors
/// Returns an error if the file cannot be opened or a migration fails.
pub(crate) fn open_migrated(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("opening the chat store at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 250;",
    )
    .context("applying the chat pragma contract")?;
    apply_migrations(&mut conn)?;
    Ok(conn)
}

/// Apply every embedded migration newer than the store's `user_version` — one
/// transaction per migration, all or nothing (the shared store discipline).
///
/// `user_version` (a database-header value, not a table) is the authoritative
/// gate, so it can be read before migration 1 creates `schema_versions`, and a
/// rolled-back migration reverts the version pointer with the schema.
///
/// `pub(crate)` so [`crate::memory::MemoryStore`] can migrate the **same**
/// `chat.db` track through the one shared [`MIGRATIONS`] ledger when it opens its
/// own connection (the multi-step memory rides migration 2+, not a new file).
pub(crate) fn apply_migrations(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading chat user_version")?;

    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        let tx = conn
            .transaction()
            .with_context(|| format!("opening transaction for chat migration {version}"))?;
        tx.execute_batch(sql)
            .with_context(|| format!("applying chat migration {version}"))?;
        tx.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (?1, unixepoch())",
            [version],
        )
        .with_context(|| format!("recording chat migration {version}"))?;
        tx.pragma_update(None, "user_version", version)
            .with_context(|| format!("advancing chat user_version to {version}"))?;
        tx.commit()
            .with_context(|| format!("committing chat migration {version}"))?;
    }
    Ok(())
}

/// The newest schema version the embedded ledger knows about — the version a
/// fully migrated `chat.db` reports on its own, independent track.
#[must_use]
pub fn latest_version() -> i64 {
    MIGRATIONS.last().map(|&(version, _)| version).unwrap_or(0)
}
