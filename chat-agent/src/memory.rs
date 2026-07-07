//! The multi-step agent memory store ([S-175], [FR-UI-20], [ADR-41]).
//!
//! Two tiers of memory over the **same** `.logos/chat.db` ([S-168]) — migration 2
//! ([`crate::db`]), **not** a new file ([ADR-41] reversibility):
//!
//! - a **per-turn scratchpad** — the planner's plan, each specialized subagent's
//!   observation, and intermediate findings the planner and the [Synthesizer]
//!   read across steps within ONE orchestrated turn (S-173 holds this in-memory
//!   as a `Vec<(PlanStep, StepObservation)>`; this persists it);
//! - a **per-thread working/conversation memory** — a running summary carried
//!   across turns so a follow-up turn in the same thread sees prior context after
//!   a `serve --ui` restart.
//!
//! It is emphatically **not** a semantic store: **no embeddings, no vector index,
//! no RAG** in v1 ([FR-UI-20]) — a static fitness check (`tests/no_embeddings.rs`)
//! asserts no such dependency was added. Every entry is recorded **honestly**: a
//! tool failure, an empty result, or an honest budget halt is stored verbatim,
//! never papered over with a fabricated finding ([NFR-CC-04]).
//!
//! The orchestrator core ([S-173]) already emits every plan, observation, halt,
//! and final answer through its [`EventSink`]; [`ScratchpadSink`] is a thin
//! `EventSink` adapter that **persists** those events as it streams, so the
//! scratchpad is captured with **no change to the loop** — the SSE seam (S-170)
//! composes its own sink alongside this one.
//!
//! [Synthesizer]: crate::orchestrator::StepRole::Synthesizer
//! [S-168]: ../../docs/planning/journal.md#s-168-chat-persistence-store-threads-messages-and-clear-history
//! [S-173]: ../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
//! [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{ensure_db_dir, open_migrated};
use crate::orchestrator::{
    EventSink, OrchestratorEvent, PlanStep, StepRole, SynthesizerGrounding,
};

/// One typed entry in the per-turn scratchpad, stored as verbatim JSON in the
/// `chat_scratchpad.payload` column (the `kind` column mirrors the serde tag for
/// queryability). The variants map one-to-one onto the orchestrator's
/// [`OrchestratorEvent`]s a turn produces, so [`ScratchpadSink`] records them
/// as they stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScratchpadEntry {
    /// The planner's step plan for one planning round (0 = initial, ≥1 = replan).
    Plan {
        /// The planning round this plan was produced in.
        round: u32,
        /// The ordered steps the planner laid out.
        steps: Vec<PlanStep>,
    },
    /// A specialized subagent's grounded observation for one executed step.
    Observation {
        /// The step's index within its plan.
        step_index: usize,
        /// The subagent that produced the observation.
        role: StepRole,
        /// A short summary of what the step observed (honest — [NFR-CC-04]).
        summary: String,
    },
    /// An intermediate finding or an honest control note (e.g. a budget halt).
    Note {
        /// The free-text note.
        note: String,
    },
    /// The synthesized final answer the turn produced.
    FinalAnswer {
        /// The answer returned to the user.
        answer: String,
    },
}

impl ScratchpadEntry {
    /// The stored `kind` discriminant — mirrors the serde tag so the column and
    /// the payload never disagree.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            ScratchpadEntry::Plan { .. } => "plan",
            ScratchpadEntry::Observation { .. } => "observation",
            ScratchpadEntry::Note { .. } => "note",
            ScratchpadEntry::FinalAnswer { .. } => "final_answer",
        }
    }
}

/// The multi-step memory store over `.logos/chat.db` ([FR-UI-20], [ADR-41]).
///
/// Owns its own migrated [`Connection`] (WAL admits a second handle beside the
/// [`ChatStore`](crate::ChatStore)) behind a [`Mutex`] so its write methods take
/// `&self` — that is what lets a borrowed [`ScratchpadSink`] persist from the
/// orchestrator's `&self` [`EventSink::emit`]. Memory persists across a
/// `serve --ui` restart because it lives in the on-disk file, not this handle.
#[derive(Debug)]
pub struct MemoryStore {
    conn: Mutex<Connection>,
}

impl MemoryStore {
    /// Open (creating + migrating if absent) the multi-step memory tier of
    /// `.logos/chat.db` under `root`.
    ///
    /// # Errors
    /// Returns an error if the directory/file cannot be created/opened or a
    /// migration fails.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_at(&ensure_db_dir(root)?)
    }

    /// Open (creating + migrating if absent) the memory tier of `chat.db` at an
    /// explicit `path`, through the shared store open contract (WAL,
    /// `foreign_keys = ON`, [`crate::db`] migrations).
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or a migration fails.
    pub fn open_at(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: Mutex::new(open_migrated(path)?),
        })
    }

    /// Lock the connection, recovering the guard on a poisoned mutex rather than
    /// panicking (a poisoned lock means a prior writer panicked mid-statement;
    /// the SQLite transaction was rolled back, so the connection is still sound).
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- per-turn scratchpad -------------------------------------------------

    /// The next per-thread turn ordinal — one orchestrated run is one turn.
    ///
    /// `0` for a thread with no scratchpad yet, else `max(turn) + 1`. The surface
    /// calls this once per user message to scope a fresh [`ScratchpadSink`].
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure.
    pub fn next_turn(&self, thread_id: i64) -> Result<i64> {
        self.lock()
            .query_row(
                "SELECT COALESCE(MAX(turn) + 1, 0) FROM chat_scratchpad WHERE thread_id = ?1",
                [thread_id],
                |row| row.get(0),
            )
            .with_context(|| format!("computing the next turn for thread {thread_id}"))
    }

    /// Record one scratchpad entry at the next `ordinal` within `(thread, turn)`.
    ///
    /// The ordinal is assigned in-SQL as the turn's current max + 1, so entry
    /// order is stable and the write is a single statement under the lock.
    ///
    /// # Errors
    /// Returns an error if the entry cannot be serialized or the insert fails
    /// (e.g. the thread does not exist — the FK rejects an orphan).
    pub fn record(&self, thread_id: i64, turn: i64, entry: &ScratchpadEntry) -> Result<()> {
        let payload = serde_json::to_string(entry).context("serializing a scratchpad entry")?;
        self.lock()
            .execute(
                "INSERT INTO chat_scratchpad (thread_id, turn, ordinal, kind, payload, created_at)
                 VALUES (
                     ?1, ?2,
                     (SELECT COALESCE(MAX(ordinal) + 1, 0)
                        FROM chat_scratchpad WHERE thread_id = ?1 AND turn = ?2),
                     ?3, ?4, unixepoch()
                 )",
                rusqlite::params![thread_id, turn, entry.kind(), payload],
            )
            .with_context(|| {
                format!(
                    "recording a {} scratchpad entry for thread {thread_id} turn {turn}",
                    entry.kind()
                )
            })?;
        Ok(())
    }

    /// Record the planner's step plan for a round.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn record_plan(
        &self,
        thread_id: i64,
        turn: i64,
        round: u32,
        steps: &[PlanStep],
    ) -> Result<()> {
        self.record(
            thread_id,
            turn,
            &ScratchpadEntry::Plan {
                round,
                steps: steps.to_vec(),
            },
        )
    }

    /// Record a subagent's grounded observation for an executed step.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn record_observation(
        &self,
        thread_id: i64,
        turn: i64,
        step_index: usize,
        role: StepRole,
        summary: &str,
    ) -> Result<()> {
        self.record(
            thread_id,
            turn,
            &ScratchpadEntry::Observation {
                step_index,
                role,
                summary: summary.to_string(),
            },
        )
    }

    /// Record an intermediate finding / honest control note.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn record_note(&self, thread_id: i64, turn: i64, note: &str) -> Result<()> {
        self.record(thread_id, turn, &ScratchpadEntry::Note { note: note.to_string() })
    }

    /// Record the turn's synthesized final answer.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn record_final_answer(&self, thread_id: i64, turn: i64, answer: &str) -> Result<()> {
        self.record(
            thread_id,
            turn,
            &ScratchpadEntry::FinalAnswer {
                answer: answer.to_string(),
            },
        )
    }

    /// Every scratchpad entry of `(thread, turn)` in stored `ordinal` order — the
    /// deterministic per-turn record the [Synthesizer] reads to ground the final
    /// answer in the plan and every subagent observation, not just the last step.
    ///
    /// [Synthesizer]: crate::orchestrator::StepRole::Synthesizer
    ///
    /// # Errors
    /// Returns an error on a store failure or a corrupt payload.
    pub fn scratchpad(&self, thread_id: i64, turn: i64) -> Result<Vec<ScratchpadEntry>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare_cached(
                "SELECT payload FROM chat_scratchpad
                 WHERE thread_id = ?1 AND turn = ?2 ORDER BY ordinal",
            )
            .context("preparing the scratchpad load")?;
        let rows = stmt
            .query_map(rusqlite::params![thread_id, turn], |row| {
                row.get::<_, String>(0)
            })
            .with_context(|| format!("loading scratchpad for thread {thread_id} turn {turn}"))?;

        let mut entries = Vec::new();
        for row in rows {
            let payload = row.context("reading a scratchpad row")?;
            entries.push(
                serde_json::from_str(&payload).context("deserializing a scratchpad entry")?,
            );
        }
        Ok(entries)
    }

    /// Render `(thread, turn)`'s scratchpad as the grounding context block a
    /// tool-less Synthesizer ([S-174]) is prompted with — the plan and every
    /// subagent observation, so the final answer reflects the intermediate
    /// findings ([FR-UI-20]). The final-answer entry (the turn's output) is
    /// omitted; an empty scratchpad renders an explicit "(no observations …)".
    ///
    /// # Errors
    /// Returns an error on a store failure or a corrupt payload.
    pub fn render_scratchpad(&self, thread_id: i64, turn: i64) -> Result<String> {
        use std::fmt::Write as _;

        let entries = self.scratchpad(thread_id, turn)?;
        if entries.is_empty() {
            return Ok("(no observations recorded this turn)".to_string());
        }
        let mut out = String::new();
        for entry in &entries {
            match entry {
                ScratchpadEntry::Plan { round, steps } => {
                    let _ = writeln!(out, "Plan (round {round}):");
                    for (i, step) in steps.iter().enumerate() {
                        let _ = writeln!(out, "  {}. [{:?}] {}", i + 1, step.role, step.instruction);
                    }
                }
                ScratchpadEntry::Observation { step_index, role, summary } => {
                    let _ = writeln!(out, "Observation [{role:?}] step #{step_index}: {summary}");
                }
                ScratchpadEntry::Note { note } => {
                    let _ = writeln!(out, "Note: {note}");
                }
                // The final answer is the synthesis output, not its input.
                ScratchpadEntry::FinalAnswer { .. } => {}
            }
        }
        Ok(out.trim_end().to_string())
    }

    // ---- per-thread working / conversation memory ----------------------------

    /// Upsert a thread's running working/conversation-memory summary — the
    /// context a follow-up turn reads after a `serve --ui` restart ([FR-UI-20]).
    ///
    /// One row per thread (PK = `thread_id`); a second call replaces the summary.
    ///
    /// # Errors
    /// Returns an error if the upsert fails (e.g. the thread does not exist — the
    /// FK rejects an orphan).
    pub fn set_working_memory(&self, thread_id: i64, summary: &str) -> Result<()> {
        self.lock()
            .execute(
                "INSERT INTO chat_working_memory (thread_id, summary, updated_at)
                 VALUES (?1, ?2, unixepoch())
                 ON CONFLICT(thread_id)
                 DO UPDATE SET summary = excluded.summary, updated_at = excluded.updated_at",
                rusqlite::params![thread_id, summary],
            )
            .with_context(|| format!("setting working memory for thread {thread_id}"))?;
        Ok(())
    }

    /// A thread's working/conversation-memory summary, or `None` if none was ever
    /// recorded.
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure.
    pub fn working_memory(&self, thread_id: i64) -> Result<Option<String>> {
        self.lock()
            .query_row(
                "SELECT summary FROM chat_working_memory WHERE thread_id = ?1",
                [thread_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .with_context(|| format!("loading working memory for thread {thread_id}"))
    }

    /// Whether the store holds no memory at all — no scratchpad entries and no
    /// working memory (the post-Clear-history / fresh-store assertion).
    ///
    /// # Errors
    /// Returns an error on an unexpected store failure.
    pub fn is_empty(&self) -> Result<bool> {
        let conn = self.lock();
        let scratchpad: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_scratchpad", [], |row| row.get(0))
            .context("counting scratchpad entries")?;
        let working: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_working_memory", [], |row| row.get(0))
            .context("counting working-memory rows")?;
        Ok(scratchpad == 0 && working == 0)
    }
}

/// An [`EventSink`] that **persists** an orchestrated turn's scratchpad to a
/// [`MemoryStore`] as the events stream ([S-175], [FR-UI-20]).
///
/// The orchestrator core ([S-173]) already emits the plan, each step's
/// observation, an honest budget halt, and the final answer; this adapter records
/// each as a [`ScratchpadEntry`] without the loop knowing memory exists. The SSE
/// seam (S-170) drives its own streaming sink alongside this one.
///
/// `emit` cannot return an error, so a persistence failure is captured into
/// [`first_error`](ScratchpadSink::first_error) (the **first** one) for the caller
/// to surface **honestly** after the run rather than swallow ([NFR-CC-04]).
pub struct ScratchpadSink<'m> {
    store: &'m MemoryStore,
    thread_id: i64,
    turn: i64,
    error: Mutex<Option<String>>,
}

impl<'m> ScratchpadSink<'m> {
    /// A sink that records `(thread_id, turn)`'s scratchpad to `store`.
    pub fn new(store: &'m MemoryStore, thread_id: i64, turn: i64) -> Self {
        Self {
            store,
            thread_id,
            turn,
            error: Mutex::new(None),
        }
    }

    /// The first persistence error this sink hit, if any — the caller surfaces it
    /// after the run so a failed write to memory is reported, never silently
    /// dropped ([NFR-CC-04]).
    #[must_use = "persistence failures are surfaced only through first_error; \
                  ignoring it silently swallows a failed memory write (NFR-CC-04)"]
    pub fn first_error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Record the first error seen, leaving an already-captured one in place.
    fn capture(&self, err: anyhow::Error) {
        let mut slot = self.error.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(err.to_string());
        }
    }
}

impl EventSink for ScratchpadSink<'_> {
    fn emit(&self, event: OrchestratorEvent) {
        let result = match event {
            OrchestratorEvent::Plan { round, steps } => {
                self.store.record_plan(self.thread_id, self.turn, round, &steps)
            }
            OrchestratorEvent::StepObserved { index, role, summary } => self
                .store
                .record_observation(self.thread_id, self.turn, index, role, &summary),
            OrchestratorEvent::Halted { bound, .. } => self.store.record_note(
                self.thread_id,
                self.turn,
                &format!("honest budget halt: {bound}"),
            ),
            OrchestratorEvent::FinalAnswer { answer } => {
                self.store.record_final_answer(self.thread_id, self.turn, &answer)
            }
            // The instruction is already captured in the round's `Plan` entry.
            OrchestratorEvent::StepStarted { .. } => Ok(()),
            // A streamed answer chunk is a transient live preview; the authoritative
            // full text is persisted once, from `FinalAnswer` ([FR-UI-19]).
            OrchestratorEvent::AnswerDelta { .. } => Ok(()),
        };
        if let Err(err) = result {
            self.capture(err);
        }
    }
}

/// A [`SynthesizerGrounding`] that renders `(thread, turn)`'s **persisted**
/// scratchpad from a [`MemoryStore`] — the production wiring of S-175's memory
/// into the S-174 Synthesizer's prompt ([S-175] AC1, the S-170 composition).
///
/// Built per turn (it captures the turn ordinal) and read at synthesis time, by
/// which point the [`ScratchpadSink`] has already streamed the plan and the prior
/// steps' observations into the store — so the Synthesizer is grounded on the
/// authoritative recorded findings, not just the planner-built instruction. Shares
/// the store behind an [`Arc`] with the turn's [`ScratchpadSink`]; the store's
/// internal [`Mutex`] serializes the sink's writes against this read.
///
/// A render failure degrades **honestly** to an explicit "(scratchpad
/// unavailable: …)" note rather than a fabricated grounding ([NFR-CC-04]) — the
/// Synthesizer then says so rather than inventing facts.
///
/// [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug)]
pub struct MemoryGrounding {
    store: Arc<MemoryStore>,
    thread_id: i64,
    turn: i64,
}

impl MemoryGrounding {
    /// Ground the Synthesizer on `(thread_id, turn)`'s scratchpad in `store`.
    pub fn new(store: Arc<MemoryStore>, thread_id: i64, turn: i64) -> Self {
        Self {
            store,
            thread_id,
            turn,
        }
    }
}

impl SynthesizerGrounding for MemoryGrounding {
    fn grounding(&self) -> String {
        self.store
            .render_scratchpad(self.thread_id, self.turn)
            .unwrap_or_else(|e| format!("(scratchpad unavailable: {e})"))
    }
}
