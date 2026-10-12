//! A preset changes only the planner and Synthesizer preambles ([S-625],
//! [FR-UI-49], [ADR-41]).
//!
//! For every non-Default preset, on the member roster and on the workspace roster:
//!
//! - every role's registered tools (name, description, schema, in order) equal
//!   Default's, and so does the set of roles the roster carries;
//! - every subagent preamble but the Synthesizer's equals Default's;
//! - the planner and Synthesizer preambles differ from Default's, and each is
//!   Default's followed by the preset's clause — the base is kept, never replaced.
//!
//! And a thread's first turn under every preset is still titled ([S-623]): the
//! title trailer follows the preset's Synthesizer preamble, so the two compose.
//!
//! The budget tree is not an input of the roster at all; its bounds under each
//! preset are pinned where the turn is assembled, in `web`'s launch-path tests.
//!
//! [S-625]: ../../docs/planning/journal.md#s-625-a-chat-turn-runs-under-a-prompt-only-preset-chosen-in-the-composer
//! [S-623]: ../../docs/planning/journal.md#s-623-the-synthesizer-titles-a-new-conversation-and-a-conversation-can-be-renamed
//! [FR-UI-49]: ../../docs/specs/requirements/FR-UI-49.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md

use std::path::Path;
use std::sync::Arc;

use agent_core::rig::completion::ToolDefinition;
use agent_core::{MockCompletionModel, MockTurn, Sandbox, XserviceBacking};
use chat_agent::orchestrator::{
    BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, Planner, Preset, StepRole,
    SubagentRoster, TurnOutcome, WorkspaceRoster, TITLE_TRAILER_CLAUSE,
};
use logos_core::federation::{Backing, ContractBridge, EngineRegistry, Federation, Member, RegistryMode};
use logos_core::Engine;

/// Every role a plan step can name.
const ROLES: [StepRole; 5] = [
    StepRole::GraphNavigator,
    StepRole::GovernanceAnalyst,
    StepRole::SourceReader,
    StepRole::Synthesizer,
    StepRole::WorkspaceAnalyst,
];

/// The presets that are not Default.
fn steered() -> impl Iterator<Item = Preset> {
    Preset::ALL.into_iter().filter(|p| *p != Preset::Default)
}

/// One registered tool, comparably: name, description and schema.
type Tool = (String, String, String);

/// A comparable rendering of a role's registered tools, in registration order.
fn tool_list(defs: Vec<ToolDefinition>) -> Vec<Tool> {
    defs.into_iter()
        .map(|d| (d.name, d.description, d.parameters.to_string()))
        .collect()
}

fn member_fixture(dir: &Path) -> (Arc<Engine>, Arc<Sandbox>) {
    std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
    std::fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").expect("write fixture");
    let engine = Arc::new(Engine::start(dir).expect("engine start"));
    let sandbox = Arc::new(Sandbox::new(dir, std::iter::empty()).expect("sandbox"));
    (engine, sandbox)
}

/// A two-member workspace backing over a lazy registry: no member engine starts
/// to list tools or render preambles.
fn workspace_backing(root: &Path) -> XserviceBacking {
    let federation = Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members: ["api", "web"]
            .iter()
            .map(|name| Member { name: name.to_string(), root: root.join(name) })
            .collect(),
        default: Some("api".to_string()),
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
    };
    let backing = Arc::new(Backing::Federated(Box::new(EngineRegistry::<Engine>::new(
        federation,
        RegistryMode::Lazy,
    ))));
    XserviceBacking::federated(backing, Arc::new(ContractBridge::new()))
        .expect("a federated backing")
}

/// What a roster under one preset offers and says, role by role.
#[derive(Debug, PartialEq)]
struct Surface {
    planner: String,
    preambles: Vec<(StepRole, String)>,
    tools: Vec<(StepRole, Vec<Tool>)>,
}

impl Surface {
    fn preamble(&self, role: StepRole) -> &str {
        &self.preambles.iter().find(|(r, _)| *r == role).expect("every role").1
    }
}

async fn member_surface(engine: &Arc<Engine>, sandbox: &Arc<Sandbox>, preset: Preset) -> Surface {
    let roster = SubagentRoster::new(Arc::clone(engine), Arc::clone(sandbox), MockCompletionModel::new([]))
        .with_preset(preset);
    let mut tools = Vec::new();
    for role in ROLES {
        tools.push((role, tool_list(roster.registered_tools(role).await)));
    }
    Surface {
        planner: roster.planner_preamble(),
        preambles: ROLES.iter().map(|r| (*r, roster.preamble(*r))).collect(),
        tools,
    }
}

async fn workspace_surface(xservice: &XserviceBacking, preset: Preset) -> Surface {
    let roster = WorkspaceRoster::new(xservice.clone(), MockCompletionModel::new([])).with_preset(preset);
    let mut tools = Vec::new();
    for role in ROLES {
        tools.push((role, tool_list(roster.registered_tools(role).await)));
    }
    Surface {
        planner: roster.planner_preamble(),
        preambles: ROLES.iter().map(|r| (*r, roster.preamble(*r))).collect(),
        tools,
    }
}

/// The equality half and the difference half of the acceptance criterion, for
/// one roster's `default` and `steered` surfaces under `preset`.
fn assert_only_the_two_preambles_moved(default: &Surface, steered: &Surface, preset: Preset, roster: &str) {
    assert_eq!(steered.tools, default.tools, "{roster} / {preset:?}: the tool lists are Default's");
    // The roster: the same roles carried (a role with no preamble is one the roster
    // does not carry), each tool-bearing one under Default's own preamble.
    for role in ROLES {
        assert_eq!(
            steered.preamble(role).is_empty(),
            default.preamble(role).is_empty(),
            "{roster} / {preset:?}: {role:?} is carried exactly when Default carries it"
        );
        if role != StepRole::Synthesizer {
            assert_eq!(
                steered.preamble(role),
                default.preamble(role),
                "{roster} / {preset:?}: {role:?}'s preamble is Default's"
            );
        }
    }
    assert_ne!(steered.planner, default.planner, "{roster} / {preset:?}: the planner preamble differs");
    assert_eq!(
        steered.planner,
        format!("{}\n\n{}", default.planner, preset.planner_clause().expect("a clause")),
        "{roster} / {preset:?}: the planner keeps Default's preamble and adds the clause"
    );
    let (default_synth, steered_synth) =
        (default.preamble(StepRole::Synthesizer), steered.preamble(StepRole::Synthesizer));
    assert_ne!(steered_synth, default_synth, "{roster} / {preset:?}: the Synthesizer preamble differs");
    assert_eq!(
        steered_synth,
        format!("{default_synth}\n\n{}", preset.synthesizer_clause().expect("a clause")),
        "{roster} / {preset:?}: the Synthesizer keeps Default's preamble and adds the clause"
    );
}

#[tokio::test]
async fn on_the_member_roster_a_preset_moves_only_the_planner_and_synthesizer_preambles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = member_fixture(dir.path());
    let default = member_surface(&engine, &sandbox, Preset::Default).await;
    // Default is the roster as it was before presets: byte-identical preambles.
    assert_eq!(default.planner, chat_agent::orchestrator::DEFAULT_PLANNER_PREAMBLE);
    assert_eq!(default.preamble(StepRole::Synthesizer), chat_agent::SYNTHESIZER_PREAMBLE);
    for preset in steered() {
        let steered = member_surface(&engine, &sandbox, preset).await;
        assert_only_the_two_preambles_moved(&default, &steered, preset, "member");
    }
}

#[tokio::test]
async fn on_the_workspace_roster_a_preset_moves_only_the_planner_and_synthesizer_preambles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let xservice = workspace_backing(dir.path());
    let federation = xservice.registry().federation().clone();
    let default = workspace_surface(&xservice, Preset::Default).await;
    assert_eq!(default.planner, chat_agent::workspace_planner_preamble(&federation));
    assert_eq!(
        default.preamble(StepRole::Synthesizer),
        chat_agent::workspace_synthesizer_preamble(&federation)
    );
    for preset in steered() {
        let steered = workspace_surface(&xservice, preset).await;
        assert_only_the_two_preambles_moved(&default, &steered, preset, "workspace");
    }
    assert_eq!(xservice.registry().resident_count(), 0, "no member engine was started");
}

/// The title a first turn's events carry, if any.
fn title_of(sink: &CapturingSink) -> Option<(i64, String)> {
    sink.events().into_iter().find_map(|e| match e {
        OrchestratorEvent::ThreadTitle { thread_id, title } => Some((thread_id, title)),
        _ => None,
    })
}

/// One conversational first turn of thread 7 over `executor`, its planner under
/// `planner_preamble`: the Synthesizer answers with a `TITLE:` trailer.
async fn first_turn<E>(planner_preamble: String, executor: impl FnOnce(MockCompletionModel) -> E) -> (MockCompletionModel, CapturingSink, TurnOutcome)
where
    E: chat_agent::orchestrator::StepExecutor,
{
    // One model backs the planner and the Synthesizer, consumed in order.
    let model = MockCompletionModel::new([
        MockTurn::text(r#"{"action":"final","grounded":false}"#),
        MockTurn::text("I can map your code.\n\nTITLE: What Logos can do"),
    ]);
    let planner = Planner::with_preamble(model.clone(), planner_preamble);
    let orchestrator = Orchestrator::with_planner(planner, executor(model.clone()), BudgetTree::new(24, 8, 3))
        .titling(7);
    let sink = CapturingSink::new();
    let outcome = orchestrator.run("hello, what can you do?", &sink).await.expect("answers");
    (model, sink, outcome)
}

/// [S-623]'s planner note, pinned: a first turn under EVERY preset, on both
/// rosters, still yields a title — the trailer clause follows the preset's
/// Synthesizer preamble, and the planner ran under the preset's own.
#[tokio::test]
async fn a_first_turn_under_every_preset_is_still_titled_on_both_rosters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (engine, sandbox) = member_fixture(&dir.path().join("member"));
    let xservice = workspace_backing(&dir.path().join("ws"));
    for preset in Preset::ALL {
        let member = SubagentRoster::new(Arc::clone(&engine), Arc::clone(&sandbox), MockCompletionModel::new([]))
            .with_preset(preset);
        let workspace = WorkspaceRoster::new(xservice.clone(), MockCompletionModel::new([])).with_preset(preset);
        let runs = [
            (
                "member",
                member.planner_preamble(),
                member.preamble(StepRole::Synthesizer),
                first_turn(member.planner_preamble(), |m| {
                    SubagentRoster::new(Arc::clone(&engine), Arc::clone(&sandbox), m).with_preset(preset)
                })
                .await,
            ),
            (
                "workspace",
                workspace.planner_preamble(),
                workspace.preamble(StepRole::Synthesizer),
                first_turn(workspace.planner_preamble(), |m| {
                    WorkspaceRoster::new(xservice.clone(), m).with_preset(preset)
                })
                .await,
            ),
        ];
        for (roster, planner_preamble, synthesizer_preamble, (model, sink, outcome)) in runs {
            assert!(matches!(outcome, TurnOutcome::Answered(_)), "{roster} / {preset:?}: {outcome:?}");
            assert_eq!(
                title_of(&sink),
                Some((7, "What Logos can do".to_string())),
                "{roster} / {preset:?}: the first turn is titled"
            );
            let prompts = model.system_prompts();
            assert_eq!(prompts.len(), 2, "{roster} / {preset:?}: one planner and one Synthesizer request");
            assert_eq!(prompts[0].as_deref(), Some(planner_preamble.as_str()), "{roster} / {preset:?}");
            assert_eq!(
                prompts[1],
                Some(format!("{synthesizer_preamble}\n\n{TITLE_TRAILER_CLAUSE}")),
                "{roster} / {preset:?}: the trailer clause follows the preset's Synthesizer preamble"
            );
        }
    }
}
