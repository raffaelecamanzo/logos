//! The Synthesizer titles the conversation its turn opened ([S-623], [FR-UI-48]).
//!
//! These drive the public orchestrator over the real member roster with the
//! offline mock `CompletionModel` behind the planner and the Synthesizer, so the
//! preamble the Synthesizer actually ran under and every provider request are
//! observable. They pin the acceptance criteria the orchestrator owns:
//!
//! - a first turn ending in a `TITLE:` trailer emits that title as
//!   `ThreadTitle`, and neither the streamed deltas nor the final answer carry the
//!   trailer — even when it is split across two deltas;
//! - asking for the title adds no provider request (counted against the same turn
//!   run without it);
//! - no trailer, a malformed one, or one over six words emits no title;
//! - a later turn is not asked and never retitles.
//!
//! [S-623]: ../../docs/planning/journal.md#s-623-the-synthesizer-titles-a-new-conversation-and-a-conversation-can-be-renamed
//! [FR-UI-48]: ../../docs/specs/requirements/FR-UI-48.md

use std::sync::Arc;

use agent_core::{MockCompletionModel, MockTurn, Sandbox};
use chat_agent::orchestrator::{
    BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, RoleModels, SubagentRoster,
    TurnOutcome, SYNTHESIZER_PREAMBLE, TITLE_TRAILER_CLAUSE,
};
use logos_core::Engine;

/// The conversation id the titled turns open.
const THREAD: i64 = 7;

fn fixture(dir: &std::path::Path) -> (Arc<Engine>, Arc<Sandbox>) {
    std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
    std::fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").expect("write fixture");
    let engine = Arc::new(Engine::start(dir).expect("engine start"));
    let sandbox = Arc::new(Sandbox::new(dir, std::iter::empty()).expect("sandbox"));
    (engine, sandbox)
}

/// A conversational `final` — the planner's one request, answered directly by the
/// Synthesizer with no grounding step ([FR-UI-30]).
fn final_conversational() -> MockTurn {
    MockTurn::text(r#"{"action":"final","grounded":false}"#)
}

/// What one turn produced, and what it cost.
struct Ran {
    sink: CapturingSink,
    outcome: TurnOutcome,
    planner: MockCompletionModel,
    synthesizer: MockCompletionModel,
}

impl Ran {
    fn requests(&self) -> usize {
        self.planner.request_count() + self.synthesizer.request_count()
    }

    fn deltas(&self) -> Vec<String> {
        self.sink
            .events()
            .into_iter()
            .filter_map(|e| match e {
                OrchestratorEvent::AnswerDelta { delta } => Some(delta),
                _ => None,
            })
            .collect()
    }

    fn titles(&self) -> Vec<(i64, String)> {
        self.sink
            .events()
            .into_iter()
            .filter_map(|e| match e {
                OrchestratorEvent::ThreadTitle { thread_id, title } => Some((thread_id, title)),
                _ => None,
            })
            .collect()
    }

    fn final_answer(&self) -> String {
        self.sink
            .events()
            .into_iter()
            .find_map(|e| match e {
                OrchestratorEvent::FinalAnswer { answer } => Some(answer),
                _ => None,
            })
            .expect("a final answer is emitted")
    }

    fn synthesizer_preamble(&self) -> String {
        self.synthesizer.system_prompts()[0]
            .clone()
            .expect("the synthesizer ran with a preamble")
    }
}

/// Run one conversational turn whose Synthesizer streams `chunks`, opening
/// conversation [`THREAD`] when `opens` is set.
async fn run(chunks: &[&str], opens: bool) -> Ran {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());
    let planner = MockCompletionModel::new([final_conversational()]);
    let synthesizer = MockCompletionModel::new([MockTurn::text_chunks(chunks.iter().copied())]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator = Orchestrator::new(planner.clone(), roster, BudgetTree::new(24, 8, 3));
    let orchestrator = if opens {
        orchestrator.titling(THREAD)
    } else {
        orchestrator
    };
    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("hello, what can you do?", &sink)
        .await
        .expect("answers");
    Ran {
        sink,
        outcome,
        planner,
        synthesizer,
    }
}

#[tokio::test]
async fn a_first_turn_trailer_titles_the_thread_and_never_reaches_the_answer() {
    let ran = run(
        &["I can map your code.", "\n\nTITLE: What Logos can do"],
        true,
    )
    .await;

    assert_eq!(ran.titles(), [(THREAD, "What Logos can do".to_string())]);
    assert_eq!(ran.final_answer(), "I can map your code.");
    assert_eq!(
        ran.outcome,
        TurnOutcome::Answered("I can map your code.".to_string())
    );
    assert_eq!(
        ran.deltas().concat(),
        "I can map your code.",
        "the deltas are the answer alone"
    );

    let events = ran.sink.events();
    let title_at = events
        .iter()
        .position(|e| matches!(e, OrchestratorEvent::ThreadTitle { .. }))
        .expect("a title is emitted");
    let final_at = events
        .iter()
        .position(|e| matches!(e, OrchestratorEvent::FinalAnswer { .. }))
        .expect("a final answer is emitted");
    assert!(
        final_at < title_at,
        "the title follows the final answer (FR-UI-19): {events:?}"
    );

    let preamble = ran.synthesizer_preamble();
    assert!(
        preamble.starts_with(SYNTHESIZER_PREAMBLE),
        "the role's preamble is kept: {preamble}"
    );
    assert!(
        preamble.ends_with(TITLE_TRAILER_CLAUSE),
        "the trailer clause follows it: {preamble}"
    );
}

#[tokio::test]
async fn asking_for_the_title_adds_no_provider_request() {
    let chunks = ["I can map your code.", "\nTITLE: What Logos can do"];
    let titled = run(&chunks, true).await;
    let untitled = run(&chunks, false).await;
    assert_eq!(titled.planner.request_count(), 1);
    assert_eq!(titled.synthesizer.request_count(), 1);
    assert_eq!(
        titled.requests(),
        untitled.requests(),
        "the title rides the answer's own request"
    );
}

#[tokio::test]
async fn a_trailer_split_across_two_deltas_never_reaches_the_answer() {
    let ran = run(&["Here is the map.\n\nTI", "TLE: Code map"], true).await;
    for delta in ran.deltas() {
        assert!(
            !delta.contains("TI"),
            "no part of the trailer is streamed: {delta:?}"
        );
    }
    assert_eq!(ran.deltas().concat(), "Here is the map.");
    assert_eq!(ran.final_answer(), "Here is the map.");
    assert_eq!(ran.titles(), [(THREAD, "Code map".to_string())]);
}

#[tokio::test]
async fn no_trailer_a_malformed_one_or_an_over_long_one_emits_no_title() {
    let ran = run(&["Just an answer."], true).await;
    assert!(ran.titles().is_empty(), "no trailer, no title");
    assert_eq!(
        ran.final_answer(),
        "Just an answer.",
        "the answer is untouched"
    );

    let ran = run(&["An answer.\nTITLE:"], true).await;
    assert!(ran.titles().is_empty(), "an empty trailer names nothing");
    assert_eq!(ran.final_answer(), "An answer.", "but it is still stripped");

    let ran = run(
        &["An answer.\nTITLE: one two three four five six seven"],
        true,
    )
    .await;
    assert!(ran.titles().is_empty(), "seven words is over the limit");
    assert_eq!(ran.final_answer(), "An answer.");
}

#[tokio::test]
async fn a_later_turn_is_not_asked_and_never_retitles() {
    // The model volunteers a trailer anyway: on a turn that did not open the
    // conversation it is the model's text, not a title.
    let ran = run(&["Follow-up answer.\nTITLE: Something else"], false).await;
    assert!(ran.titles().is_empty(), "a later turn never retitles");
    assert!(
        !ran.synthesizer_preamble().contains(TITLE_TRAILER_CLAUSE),
        "a later turn's Synthesizer is not asked for a title"
    );
}

#[tokio::test]
async fn a_first_turn_bounded_by_its_budget_is_still_titled_and_stripped() {
    // max_replans = 0: the planner's second plan is refused, and the turn finalizes
    // best-effort over its one usable observation — the bounded-halt path, which
    // composes its answer through the same Synthesizer pass.
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = fixture(dir.path());
    let plan = r#"{"action":"plan","steps":[{"role":"graph_navigator","instruction":"look"}]}"#;
    let planner = MockCompletionModel::new([MockTurn::text(plan), MockTurn::text(plan)]);
    let synthesizer = MockCompletionModel::new([MockTurn::text_chunks([
        "Partial.\n\nTI",
        "TLE: Bounded map",
    ])]);
    let roster = SubagentRoster::with_models(
        engine,
        sandbox,
        RoleModels {
            graph_navigator: MockCompletionModel::new([MockTurn::text("alpha is defined.")]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: synthesizer.clone(),
        },
    );
    let orchestrator =
        Orchestrator::new(planner.clone(), roster, BudgetTree::new(24, 8, 0)).titling(THREAD);
    let sink = CapturingSink::new();
    let outcome = orchestrator
        .run("map alpha", &sink)
        .await
        .expect("answers best-effort");
    let ran = Ran {
        sink,
        outcome,
        planner,
        synthesizer,
    };
    assert_eq!(ran.outcome, TurnOutcome::Answered(ran.final_answer()));
    assert_eq!(ran.titles(), [(THREAD, "Bounded map".to_string())]);
    let answer = ran.final_answer();
    assert!(
        answer.starts_with("[bounded"),
        "the bounded marker leads: {answer}"
    );
    assert!(
        answer.ends_with("\nPartial."),
        "the trailer is stripped: {answer}"
    );
    for delta in ran.deltas() {
        assert!(
            !delta.contains("TI"),
            "no part of the trailer is streamed: {delta:?}"
        );
    }
    assert!(ran.synthesizer_preamble().ends_with(TITLE_TRAILER_CLAUSE));
}
