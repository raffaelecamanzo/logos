//! Multi-step memory-store fitness tests (S-175, [FR-UI-20], [NFR-CC-04]).
//!
//! These drive a **real on-disk** `chat.db` in a throwaway directory so the
//! persistence assertions are genuine: a store written, dropped, and re-opened
//! proves memory survives a `serve --ui` restart / process bounce (the S-175
//! acceptance criterion). They prove the three acceptance bullets:
//!
//! 1. a multi-step orchestrated turn records its plan + per-subagent observations
//!    in the scratchpad, and the Synthesizer's grounding (the rendered scratchpad)
//!    reflects every intermediate finding — not just the last step;
//! 2. per-thread working/conversation memory survives a simulated restart and a
//!    follow-up turn in the same thread sees prior context;
//! 3. Clear-history wipes a thread's memory together with its messages.
//!
//! The no-embedding/no-vector static check lives in `tests/no_embeddings.rs`.
//!
//! [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use agent_core::{MockCompletionModel, MockTurn};
use chat_agent::orchestrator::{
    BudgetBound, BudgetTree, EventSink, Orchestrator, OrchestratorEvent, PlanStep, StepContext,
    StepError, StepExecutor, StepObservation, StepRole, TurnOutcome,
};
use chat_agent::{
    prior_turns, thread_window, ChatRole, ChatStore, MemoryStore, ScratchpadEntry, ScratchpadSink,
};
use tempfile::TempDir;

/// A scripted stand-in for the S-174 subagent roster: each step charges
/// `calls_per_step` tool calls through the [`StepContext`] (exercising the budget
/// tree) and returns a role-tagged summary so the scratchpad carries a distinct,
/// grounded observation per subagent.
struct ScriptedExecutor {
    calls_per_step: usize,
    synth_answer: String,
}

impl ScriptedExecutor {
    fn new(calls_per_step: usize) -> Self {
        Self {
            calls_per_step,
            synth_answer: "the synthesized grounded answer".to_string(),
        }
    }

    fn with_answer(calls_per_step: usize, answer: &str) -> Self {
        Self {
            calls_per_step,
            synth_answer: answer.to_string(),
        }
    }
}

impl StepExecutor for ScriptedExecutor {
    fn execute(
        &self,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> impl std::future::Future<Output = Result<StepObservation, StepError>> + Send {
        let calls = self.calls_per_step;
        let role = step.role;
        let synth_answer = self.synth_answer.clone();
        async move {
            // The tool-less Synthesizer composes the terminal answer ([CR-086]): it
            // charges no budget and streams its prose as the answer.
            if role == StepRole::Synthesizer {
                ctx.emit_answer_delta(&synth_answer);
                return Ok(StepObservation::new(synth_answer));
            }
            for _ in 0..calls {
                ctx.charge_tool_call()?;
            }
            Ok(StepObservation::new(format!(
                "{role:?} observed result after {calls} tool call(s)"
            )))
        }
    }
}

fn plan_json(role: &str, instruction: &str) -> String {
    format!(r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#)
}

/// A `final` decision — a control marker only, carrying no answer prose ([CR-086]).
fn final_json(grounded: bool) -> String {
    format!(r#"{{"action":"final","grounded":{grounded}}}"#)
}

/// A multi-step turn records its plan and each subagent's observation in the
/// scratchpad, and the Synthesizer's grounding (the rendered scratchpad) reflects
/// every intermediate finding — the first S-175 acceptance criterion. Persisting
/// happens with **no change to the orchestrator loop**: the [`ScratchpadSink`] is
/// just the run's event sink.
#[tokio::test]
async fn a_multi_step_turn_records_plan_and_observations_for_the_synthesizer() {
    let dir = TempDir::new().unwrap();
    let mut chat = ChatStore::open(dir.path()).unwrap();
    let memory = MemoryStore::open(dir.path()).unwrap();

    // A real conversation row to hang the turn's memory off (FK parent).
    let thread = chat.create_thread("Who calls Engine and is it clean?").unwrap();
    let turn = memory.next_turn(thread).unwrap();
    assert_eq!(turn, 0, "the first turn of a fresh thread is turn 0");

    // A compound request: a graph step, a replanned governance step, then synthesis.
    let planner = MockCompletionModel::new([
        MockTurn::text(plan_json("graph_navigator", "find callers of Engine")),
        MockTurn::text(plan_json("governance_analyst", "scan the Engine module")),
        MockTurn::text(final_json(true)),
    ]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor::with_answer(1, "Engine has 3 callers and a clean scan."),
        BudgetTree::new(24, 8, 3),
    );

    // The ScratchpadSink IS the run's event sink — persistence rides streaming.
    let sink = ScratchpadSink::new(&memory, thread, turn);
    let outcome = orchestrator
        .run("Who calls Engine and is the module clean?", &sink)
        .await
        .expect("the orchestrated turn completes");
    assert_eq!(
        outcome,
        TurnOutcome::Answered("Engine has 3 callers and a clean scan.".to_string())
    );
    assert!(sink.first_error().is_none(), "no scratchpad write failed");

    // The scratchpad recorded both plans, both subagent observations, and the
    // final answer, in order.
    let entries = memory.scratchpad(thread, turn).unwrap();
    let observations: Vec<(&StepRole, &str)> = entries
        .iter()
        .filter_map(|e| match e {
            ScratchpadEntry::Observation { role, summary, .. } => Some((role, summary.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(observations.len(), 2, "one observation per executed step: {entries:?}");
    assert_eq!(observations[0].0, &StepRole::GraphNavigator);
    assert_eq!(observations[1].0, &StepRole::GovernanceAnalyst);
    assert!(
        entries
            .iter()
            .filter(|e| matches!(e, ScratchpadEntry::Plan { .. }))
            .count()
            == 2,
        "the initial plan and the replan were both recorded: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, ScratchpadEntry::FinalAnswer { .. })),
        "the final answer was recorded: {entries:?}"
    );

    // The Synthesizer's grounding reflects BOTH intermediate findings, not just
    // the last step — the property FR-UI-20 names explicitly.
    let synth_context = memory.render_scratchpad(thread, turn).unwrap();
    assert!(
        synth_context.contains("GraphNavigator observed result"),
        "the graph step's finding feeds synthesis: {synth_context}"
    );
    assert!(
        synth_context.contains("GovernanceAnalyst observed result"),
        "the governance step's finding feeds synthesis: {synth_context}"
    );
}

/// A budget halt is recorded honestly in the scratchpad as a note — never a
/// fabricated finding ([NFR-CC-04]).
#[tokio::test]
async fn an_honest_budget_halt_is_recorded_in_the_scratchpad() {
    let dir = TempDir::new().unwrap();
    let mut chat = ChatStore::open(dir.path()).unwrap();
    let memory = MemoryStore::open(dir.path()).unwrap();
    let thread = chat.create_thread("a long question").unwrap();
    let turn = memory.next_turn(thread).unwrap();

    // Global ceiling = 1, the step charges 2: the second call trips the bound.
    let planner =
        MockCompletionModel::new([MockTurn::text(plan_json("graph_navigator", "deep dive"))]);
    let orchestrator = Orchestrator::new(
        planner,
        ScriptedExecutor::new(2),
        BudgetTree::new(1, 8, 3),
    );

    let sink = ScratchpadSink::new(&memory, thread, turn);
    let outcome = orchestrator.run("a long question", &sink).await.unwrap();
    assert_eq!(outcome, TurnOutcome::Halted(BudgetBound::GlobalToolCalls { limit: 1 }));
    assert!(sink.first_error().is_none());

    let entries = memory.scratchpad(thread, turn).unwrap();
    // No fabricated observation for the halted step; an honest halt note instead.
    assert!(
        !entries.iter().any(|e| matches!(e, ScratchpadEntry::Observation { .. })),
        "the halted step produced no fabricated observation: {entries:?}"
    );
    assert!(
        !entries.iter().any(|e| matches!(e, ScratchpadEntry::FinalAnswer { .. })),
        "no answer is fabricated on a halt: {entries:?}"
    );
    assert!(
        entries.iter().any(|e| matches!(
            e,
            ScratchpadEntry::Note { note } if note.contains("budget halt")
        )),
        "the honest halt is recorded as a note: {entries:?}"
    );
}

/// Per-thread working/conversation memory persists across a simulated
/// `serve --ui` restart, and a follow-up turn in the same thread sees prior
/// context (its prior messages and the running summary) — the second S-175
/// acceptance criterion.
#[test]
fn working_memory_survives_a_restart_and_a_follow_up_sees_prior_context() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let thread = {
        let mut chat = ChatStore::open(root).unwrap();
        let memory = MemoryStore::open(root).unwrap();
        let thread = chat.create_thread("about the binder").unwrap();
        // Turn 1: a user/assistant exchange plus a running summary.
        chat.append_message(thread, ChatRole::User, "where is the binder?", &[])
            .unwrap();
        chat.append_message(thread, ChatRole::Assistant, "It lives in binder.rs.", &[])
            .unwrap();
        memory
            .set_working_memory(thread, "User asked where the binder is; answer: binder.rs.")
            .unwrap();
        thread
        // Both stores drop here — connections (and WAL) close: the restart.
    };

    // Re-open from the same files: brand-new handles over the persisted data.
    let chat = ChatStore::open(root).unwrap();
    let memory = MemoryStore::open(root).unwrap();

    // The follow-up turn sees the running summary …
    assert_eq!(
        memory.working_memory(thread).unwrap().as_deref(),
        Some("User asked where the binder is; answer: binder.rs."),
        "working memory survived the restart"
    );
    // … and the prior transcript (the other half of "prior context").
    let prior = chat.messages(thread).unwrap();
    assert_eq!(prior.len(), 2, "the prior turn's messages are still there");
    assert_eq!(prior[0].content, "where is the binder?");

    // A follow-up turn updates the running summary; the upsert replaces in place.
    let memory = MemoryStore::open(root).unwrap();
    memory
        .set_working_memory(
            thread,
            "User asked where the binder is (binder.rs), then asked who calls it.",
        )
        .unwrap();
    assert_eq!(
        memory.working_memory(thread).unwrap().as_deref(),
        Some("User asked where the binder is (binder.rs), then asked who calls it."),
        "the follow-up turn replaced the running summary, not appended a second row"
    );
}

/// Clear-history (a single `DELETE FROM chat_threads`) wipes the thread's memory
/// — scratchpad and working memory — together with its messages, via the live FK
/// cascade. No orphaned memory survives (the third S-175 acceptance criterion).
#[test]
fn clear_history_wipes_memory_with_the_conversation() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    let mut chat = ChatStore::open(root).unwrap();
    let memory = MemoryStore::open(root).unwrap();

    let thread = chat.create_thread("doomed").unwrap();
    chat.append_message(thread, ChatRole::User, "hi", &[]).unwrap();
    memory
        .record_plan(
            thread,
            0,
            0,
            &[PlanStep::new(StepRole::GraphNavigator, "find X")],
        )
        .unwrap();
    memory
        .record_observation(thread, 0, 0, StepRole::GraphNavigator, "found X")
        .unwrap();
    memory.set_working_memory(thread, "a summary").unwrap();
    assert!(!memory.is_empty().unwrap(), "memory is populated before the wipe");

    // The store-level Clear-history (S-168) — unchanged by S-175.
    let removed = chat.clear_history().unwrap();
    assert_eq!(removed, 1);

    // The cascade reached the new memory tables (separate connection sees the
    // committed delete).
    assert!(memory.scratchpad(thread, 0).unwrap().is_empty(), "scratchpad cascaded");
    assert!(memory.working_memory(thread).unwrap().is_none(), "working memory cascaded");
    assert!(memory.is_empty().unwrap(), "memory is empty after Clear-history");

    // The wipe persists across a restart too — no resurrection from the WAL.
    drop(memory);
    let memory = MemoryStore::open(root).unwrap();
    assert!(memory.is_empty().unwrap());
}

/// `next_turn` advances per thread: turn 0, then turn 1 once turn 0 has entries,
/// and turns are isolated per thread.
#[test]
fn next_turn_advances_per_thread() {
    let dir = TempDir::new().unwrap();
    let mut chat = ChatStore::open(dir.path()).unwrap();
    let memory = MemoryStore::open(dir.path()).unwrap();

    let t1 = chat.create_thread("one").unwrap();
    let t2 = chat.create_thread("two").unwrap();

    assert_eq!(memory.next_turn(t1).unwrap(), 0);
    memory.record_note(t1, 0, "turn 0 happened").unwrap();
    assert_eq!(memory.next_turn(t1).unwrap(), 1, "a recorded turn advances the counter");
    // A different thread is independent.
    assert_eq!(memory.next_turn(t2).unwrap(), 0, "turns are per-thread");

    // Entries within a turn come back in insertion order.
    memory.record_note(t1, 1, "first").unwrap();
    memory.record_note(t1, 1, "second").unwrap();
    let notes: Vec<String> = memory
        .scratchpad(t1, 1)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            ScratchpadEntry::Note { note } => Some(note),
            _ => None,
        })
        .collect();
    assert_eq!(notes, vec!["first", "second"], "entries preserve insertion order");
}

/// Recording memory for a thread that does not exist is rejected by the FK rather
/// than silently orphaning it.
#[test]
fn recording_for_a_missing_thread_is_rejected() {
    let dir = TempDir::new().unwrap();
    let memory = MemoryStore::open(dir.path()).unwrap();

    assert!(
        memory.record_note(999, 0, "ghost").is_err(),
        "a scratchpad entry for a missing thread is refused by the FK"
    );
    assert!(
        memory.set_working_memory(999, "ghost").is_err(),
        "working memory for a missing thread is refused by the FK"
    );
}

/// `ScratchpadEntry` round-trips through its stored JSON payload, and the `kind`
/// discriminant matches the serde tag (the column and payload never disagree).
#[test]
fn scratchpad_entry_round_trips_and_kinds_match_the_tag() {
    let entries = [
        ScratchpadEntry::Plan {
            round: 1,
            steps: vec![PlanStep::new(StepRole::SourceReader, "read lib.rs")],
        },
        ScratchpadEntry::Observation {
            step_index: 2,
            role: StepRole::GovernanceAnalyst,
            summary: "clean".to_string(),
        },
        ScratchpadEntry::Note { note: "halt".to_string() },
        ScratchpadEntry::FinalAnswer { answer: "done".to_string() },
    ];
    for entry in entries {
        let json = serde_json::to_string(&entry).unwrap();
        let back: ScratchpadEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
        // The serde tag (the `kind` column value) matches `kind()`.
        let tag = serde_json::from_str::<serde_json::Value>(&json).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(tag, entry.kind());
    }
}

/// `ScratchpadSink` surfaces a persistence failure **honestly** ([NFR-CC-04]): a
/// write for a non-existent thread (FK-rejected) is captured into `first_error`
/// rather than swallowed, and only the FIRST error is retained.
#[test]
fn scratchpad_sink_captures_the_first_persistence_error() {
    let dir = TempDir::new().unwrap();
    // No thread is created — any scratchpad write FK-fails (foreign_keys = ON).
    let memory = MemoryStore::open(dir.path()).unwrap();
    let sink = ScratchpadSink::new(&memory, 999, 0);

    sink.emit(OrchestratorEvent::Plan {
        round: 0,
        steps: vec![PlanStep::new(StepRole::GraphNavigator, "x")],
    });
    let first = sink.first_error();
    assert!(first.is_some(), "an FK-failing write is captured, not swallowed");

    // A second failing write must not overwrite the first captured error.
    sink.emit(OrchestratorEvent::FinalAnswer {
        answer: "y".to_string(),
    });
    assert_eq!(sink.first_error(), first, "only the first error is retained");

    // Nothing was actually persisted for the ghost thread.
    assert!(memory.scratchpad(999, 0).unwrap().is_empty());
}

/// `render_scratchpad` of a turn with no recorded entries returns the explicit
/// placeholder rather than an empty string, so a Synthesizer prompt is never
/// silently blank.
#[test]
fn render_scratchpad_of_an_empty_turn_returns_a_placeholder() {
    let dir = TempDir::new().unwrap();
    let mut chat = ChatStore::open(dir.path()).unwrap();
    let memory = MemoryStore::open(dir.path()).unwrap();
    let thread = chat.create_thread("empty").unwrap();
    assert_eq!(
        memory.render_scratchpad(thread, 0).unwrap(),
        "(no observations recorded this turn)"
    );
}

// ---- prior turns come from the THREAD STORE ([S-483], [FR-UI-20] AC-2) ----------

/// Append `question`/`answer` pairs to `thread` the way a served turn does.
fn converse(store: &mut ChatStore, thread: i64, pairs: &[(&str, &str)]) {
    for (question, answer) in pairs {
        store.append_message(thread, ChatRole::User, question, &[]).unwrap();
        store.append_message(thread, ChatRole::Assistant, answer, &[]).unwrap();
    }
}

/// [S-483] AC-1: the window is read from the stored messages — NOT from
/// `set_working_memory`, which has no production caller. A thread whose working
/// memory holds a distinctive summary still shows the stored turns, and the summary
/// never leaks into the window.
#[test]
fn the_window_is_built_from_stored_messages_not_working_memory() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let memory = MemoryStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    converse(&mut store, thread, &[("who calls Engine?", "three callers"), ("is it clean?", "yes")]);
    memory.set_working_memory(thread, "WORKING-MEMORY-SENTINEL").unwrap();

    let window = thread_window(&store, thread, "and for Planner?", 6, 16_000).unwrap();
    let text = window.render();
    assert!(text.find("who calls Engine?") < text.find("is it clean?"), "{text}");
    assert!(text.contains("Assistant: three callers") && text.contains("Assistant: yes"), "{text}");
    assert!(!text.contains("WORKING-MEMORY-SENTINEL"), "{text}");
    assert_eq!(window.omitted(), 0);
}

/// [S-483] AC-1: a thread with no stored turns yields the empty window — the
/// first turn renders exactly as before.
#[test]
fn a_thread_with_no_prior_turns_has_an_empty_window() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    assert!(thread_window(&store, thread, "first question", 6, 16_000).unwrap().is_empty());
}

/// [S-483] AC-3: a deleted conversation contributes nothing — its messages cascade
/// away with the thread, so the window over it is empty and renders to "".
#[test]
fn a_deleted_conversation_contributes_nothing() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("doomed").unwrap();
    converse(&mut store, thread, &[("an old question", "an old answer")]);
    assert!(!thread_window(&store, thread, "next", 6, 16_000).unwrap().is_empty());

    assert!(store.delete_thread(thread).unwrap());
    let window = thread_window(&store, thread, "next", 6, 16_000).unwrap();
    assert!(window.is_empty());
    assert_eq!(window.render(), "");
}

/// [S-483] AC-3: a regenerated turn replaces its predecessor. Regenerate re-runs the
/// user message, so the store holds `U1, A1, U1, A1′`; only the latest answer is in
/// the window, and the user message is not duplicated.
#[test]
fn a_regenerated_turn_replaces_its_predecessor_in_the_window() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    converse(
        &mut store,
        thread,
        &[("what is X?", "first answer"), ("what is X?", "regenerated answer")],
    );

    let text = thread_window(&store, thread, "and Y?", 6, 16_000).unwrap().render();
    assert!(text.contains("regenerated answer"), "{text}");
    assert!(!text.contains("first answer"), "the predecessor is gone: {text}");
    assert_eq!(text.matches("what is X?").count(), 1, "no duplicated user turn: {text}");
}

/// [S-483]: the regenerate match is exact (modulo surrounding whitespace) — a
/// question that merely shares a prefix with the previous one is a new turn, never a
/// regenerate, in the stored sequence AND for the live question.
#[test]
fn a_near_miss_question_is_not_mistaken_for_a_regenerate() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    converse(
        &mut store,
        thread,
        &[("what is X?", "answer one"), ("what is X? in detail", "answer two")],
    );

    let text = thread_window(&store, thread, "what is X? in detail please", 6, 16_000)
        .unwrap()
        .render();
    assert!(text.contains("answer one") && text.contains("answer two"), "{text}");
    assert_eq!(prior_turns(&store.messages(thread).unwrap(), "what is X?").len(), 2);
    // Whitespace around an otherwise identical message does not defeat the match.
    assert_eq!(prior_turns(&store.messages(thread).unwrap(), "  what is X? in detail\n").len(), 1);
}

/// [S-483] AC-3: a regenerate IN PROGRESS — the stored thread ends `U1, A1` and the
/// question being run is `U1` again — leaves the predecessor out of the window, since
/// the turn about to be generated replaces it.
#[test]
fn a_regenerate_in_progress_leaves_its_predecessor_out() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    converse(&mut store, thread, &[("earlier", "kept"), ("what is X?", "to be replaced")]);

    let window = thread_window(&store, thread, "what is X?", 6, 16_000).unwrap();
    let text = window.render();
    assert!(text.contains("kept"), "{text}");
    assert!(!text.contains("to be replaced") && !text.contains("what is X?"), "{text}");

    // The same question asked as a genuinely new follow-up keeps the prior turn.
    let followup = thread_window(&store, thread, "something else", 6, 16_000).unwrap().render();
    assert!(followup.contains("to be replaced"), "{followup}");
}

/// [S-483]: a halted turn persists no assistant row — the next window shows its
/// question without inventing an answer, and a retried question replaces it.
#[test]
fn an_unanswered_turn_has_no_answer_and_a_retry_replaces_it() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    store.append_message(thread, ChatRole::User, "halted question", &[]).unwrap();
    let messages = store.messages(thread).unwrap();
    let turns = prior_turns(&messages, "a different question");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].assistant, None);

    store.append_message(thread, ChatRole::User, "halted question", &[]).unwrap();
    store.append_message(thread, ChatRole::Assistant, "now answered", &[]).unwrap();
    let turns = prior_turns(&store.messages(thread).unwrap(), "next");
    assert_eq!(turns.len(), 1, "the retry replaced the unanswered turn");
    assert_eq!(turns[0].assistant.as_deref(), Some("now answered"));
}

/// [S-483]: the thread store's `system`/`tool` rows are not conversation and never
/// reach the window.
#[test]
fn system_and_tool_rows_are_not_part_of_the_window() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    store.append_message(thread, ChatRole::System, "SYSTEM-ROW", &[]).unwrap();
    converse(&mut store, thread, &[("q", "a")]);
    store.append_message(thread, ChatRole::Tool, "TOOL-ROW", &[]).unwrap();

    let text = thread_window(&store, thread, "next", 6, 16_000).unwrap().render();
    assert!(!text.contains("SYSTEM-ROW") && !text.contains("TOOL-ROW"), "{text}");
    assert!(text.contains("User: q") && text.contains("Assistant: a"), "{text}");
}

/// [S-483] end to end over a real `chat.db`: turn 3's planner prompt carries turns
/// 1–2 from the thread store, oldest first, ahead of the question.
#[tokio::test]
async fn turn_three_of_a_stored_thread_prompts_the_planner_with_turns_one_and_two() {
    let dir = TempDir::new().unwrap();
    let mut store = ChatStore::open(dir.path()).unwrap();
    let thread = store.create_thread("t").unwrap();
    converse(
        &mut store,
        thread,
        &[("where is the binder?", "binder.rs"), ("who calls it?", "the indexer")],
    );
    let window = thread_window(&store, thread, "and for mailbox-manager?", 6, 16_000).unwrap();

    let planner = MockCompletionModel::new([MockTurn::text(final_json(false))]);
    let orchestrator = Orchestrator::new(
        planner.clone(),
        ScriptedExecutor::with_answer(0, "an answer"),
        BudgetTree::new(24, 8, 3),
    )
    .with_history(window);
    orchestrator.run("and for mailbox-manager?", &chat_agent::CapturingSink::new()).await.unwrap();

    let prompt = planner.user_prompts().remove(0).unwrap();
    let at = |needle: &str| prompt.find(needle).unwrap_or_else(|| panic!("{needle} in {prompt}"));
    assert!(at("where is the binder?") < at("binder.rs"));
    assert!(at("binder.rs") < at("who calls it?"));
    assert!(at("the indexer") < at("User question:\nand for mailbox-manager?"));
}
