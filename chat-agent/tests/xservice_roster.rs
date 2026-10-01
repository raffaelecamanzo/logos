//! The cross-service tool surface, on the workspace roster ([S-481], [S-431],
//! [FR-WS-34], [FR-WS-29], [ADR-71], [ADR-52], [BR-53], [BR-59], [NFR-PE-10],
//! [FR-CF-06]).
//!
//! Driven through the **public** roster/orchestrator API with the offline mock
//! `CompletionModel` over a real two-member federation — each member a real,
//! indexed store — so every `xservice_*` answer below is the thick-core
//! `federation::query` read-model the MCP tools return, not a stub.
//!
//! [S-431] put the `xservice_*` tools on the member roster's Graph-Navigator under
//! a federated backing. [S-481] retired that branch: the member roster is
//! single-backing under any backing, and the tools live on the **workspace
//! roster's Workspace-Analyst**. Every test here that asserted federated
//! behaviour was re-targeted to the workspace roster rather than deleted; the
//! single-backing pin stayed on the member roster.
//!
//! What is pinned, in the order the acceptance criteria state it:
//!
//! 1. the **registered** tool lists: the member roster's eight graph tools
//!    byte-for-byte under a single backing **and** under a federated one; the
//!    workspace roster's Workspace-Analyst carrying the five workspace tools then
//!    the four `xservice_*` tools, and its other roles repo-addressed; its
//!    planner and Synthesizer preambles naming every member with its declared
//!    kind, and the Synthesizer's carrying the [BR-59] clause ([ADR-71]);
//! 2. a cross-service turn dispatches an `xservice_*` tool and its observation
//!    names every hit with its member — a symbol in two members is two results;
//! 3. **the residue reaches the answer** ([BR-53]): an empty cross-service answer
//!    over the fixture's known, non-zero residue is carried into the observation
//!    as `UNRESOLVED` with its count and reason, even though the mock model's own
//!    summary says "none"; the zero-residue twin stands unqualified;
//! 4. the tools inherit the registry's laziness: building the workspace roster
//!    opens no member, and a `repo`-scoped call opens exactly that one
//!    ([NFR-PE-10], counted on the registry the way
//!    `logos-core/tests/workspace_connection_budget.rs` counts it);
//! 5. the turn stays inside the shipped budget tree, and a bound — the
//!    Workspace-Analyst's own per-subagent cap among them — halts honestly naming
//!    itself with the readings still carried ([FR-CF-06]).
//!
//! [S-481]: ../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
//! [S-431]: ../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
//! [FR-WS-34]: ../../docs/specs/requirements/FR-WS-34.md
//! [FR-WS-29]: ../../docs/specs/requirements/FR-WS-29.md
//! [ADR-71]: ../../docs/specs/architecture/decisions/ADR-71.md
//! [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
//! [BR-53]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-59]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
//! [FR-CF-06]: ../../docs/specs/requirements/FR-CF-06.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{
    addressed_toolset, governance_toolset, graph_toolset, source_toolset, MockCompletionModel,
    MockTurn, Sandbox, ToolDomain, XserviceBacking, WORKSPACE_TOOL_NAMES,
};
use chat_agent::orchestrator::{
    BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, Planner, RoleModels, StepRole,
    SubagentRoster, TurnOutcome, WorkspaceRoster, DEFAULT_PLANNER_PREAMBLE,
    GOVERNANCE_ANALYST_PREAMBLE, GRAPH_NAVIGATOR_PREAMBLE, SOURCE_READER_PREAMBLE,
    SYNTHESIZER_PREAMBLE, WORKSPACE_ANALYST_PREAMBLE, WORKSPACE_GOVERNANCE_ANALYST_PREAMBLE,
    WORKSPACE_GRAPH_NAVIGATOR_PREAMBLE, WORKSPACE_SOURCE_READER_PREAMBLE,
};
use logos_core::config::ChatConfig;
use logos_core::federation::{
    Backing, ContractBridge, EngineRegistry, Federation, Member, MemberKind, RegistryMode,
};
use logos_core::Engine;
use tempfile::TempDir;

/// The Graph-Navigator's registered tools as they stood before S-431 — the
/// literal list the single-backing roster must still register, in order.
const TODAYS_GRAPH_TOOLS: [&str; 8] = [
    "search", "context", "node", "callers", "callees", "impact", "explore", "affected",
];

/// The four `xservice_*` tools, in order — S-431's, now the Workspace-Analyst's,
/// after its five workspace tools.
const XSERVICE_TOOLS: [&str; 4] = [
    "xservice_route_providers",
    "xservice_callers",
    "xservice_impact",
    "xservice_search",
];

/// One runtime-composed HTTP client call: the client-call arm captures it and
/// records one keyless `base-url-runtime` refusal (S-374) — the fixture's known,
/// non-zero residue, the same shape `logos-core/tests/xservice_egress_residue.rs`
/// pins at exactly one unresolved site.
const API_RUNTIME_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_dynamic(client: Client, url: String) {
    let _ = client.get(url).await;
}
"#;

/// `shared` is defined in BOTH members, so an unscoped search must answer it
/// twice, once per member.
const API_LIB: &str = "pub fn shared() {}\npub fn api_only() { shared(); }\n";
const WEB_LIB: &str = "pub fn shared() {}\npub fn render() { shared(); }\n";

/// A literal client call in `web` and the axum route in `api` it binds — one
/// resolved cross-service edge, so the non-empty path is exercised on real data.
const WEB_BOUND_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}
"#;
const API_ROUTE: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// Index a member into its own store, then close it so the registry re-opens it.
fn index(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// A two-member federation (`api`, `web`) over real indexed stores. With
/// `residue`, `api` carries the runtime-composed client call; without it, the
/// twin is identical but has nothing unresolved.
struct Workspace {
    _tmp: TempDir,
    xservice: XserviceBacking,
    /// `api`'s `shared` as its canonical symbol — the form the cross-service
    /// tier matches on (a bare name reads NOT CHECKED).
    shared: String,
}

/// What the fixture's members carry beyond `shared`.
#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// Nothing cross-service: a zero residue and no edge.
    Plain,
    /// `api` makes one runtime-composed call: a non-zero residue, no edge.
    Residue,
    /// `web` calls `api`'s route with a literal target: one resolved edge.
    Bound,
}

fn workspace(residue: bool) -> Workspace {
    workspace_of(if residue { Shape::Residue } else { Shape::Plain })
}

/// The canonical symbol of the function `name` in the (closed) store at `root`.
fn canonical(root: &Path, name: &str) -> String {
    let engine = Engine::start(root).expect("engine starts");
    engine
        .search(name, Some(logos_core::model::NodeKind::Function), None)
        .hits
        .into_iter()
        .find(|hit| hit.name == name)
        .unwrap_or_else(|| panic!("{name} is indexed"))
        .symbol
}

fn workspace_of(shape: Shape) -> Workspace {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "src/lib.rs", API_LIB);
    write(&web, "src/lib.rs", WEB_LIB);
    match shape {
        Shape::Plain => {}
        Shape::Residue => write(&api, "src/client.rs", API_RUNTIME_CLIENT),
        Shape::Bound => {
            write(&api, "src/main.rs", API_ROUTE);
            write(&web, "src/client.rs", WEB_BOUND_CLIENT);
        }
    }
    index(&api);
    index(&web);
    let shared = canonical(&api, "shared");

    let xservice = federated(federation(root, &[("api", None), ("web", None)], None));
    Workspace { _tmp: tmp, xservice, shared }
}

/// A federation named `shop` over `members` (name, declared kind), each rooted
/// at `root/<name>`; no store is opened by building it.
fn federation(root: &Path, members: &[(&str, Option<MemberKind>)], default: Option<&str>) -> Federation {
    Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members: members
            .iter()
            .map(|(name, _)| Member { name: name.to_string(), root: root.join(name) })
            .collect(),
        default: default.map(str::to_string),
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: members
            .iter()
            .filter_map(|(name, kind)| kind.map(|kind| (name.to_string(), kind)))
            .collect(),
    }
}

/// The xservice backing over a lazy registry of `federation`.
fn federated(federation: Federation) -> XserviceBacking {
    let backing = Arc::new(Backing::Federated(Box::new(EngineRegistry::<Engine>::new(
        federation,
        RegistryMode::Lazy,
    ))));
    XserviceBacking::federated(backing, Arc::new(ContractBridge::new()))
        .expect("a federated backing mints an xservice backing")
}

/// JSON the mock planner returns for a single workspace_analyst step.
fn plan(instruction: &str) -> MockTurn {
    plan_for("workspace_analyst", instruction)
}

/// JSON the mock planner returns for a single step routed to `role`.
fn plan_for(role: &str, instruction: &str) -> MockTurn {
    MockTurn::text(format!(
        r#"{{"action":"plan","steps":[{{"role":"{role}","instruction":"{instruction}"}}]}}"#
    ))
}

fn finalize() -> MockTurn {
    MockTurn::text(r#"{"action":"final","grounded":true}"#)
}

/// A workspace roster whose Workspace-Analyst runs `analyst`.
fn workspace_roster(ws: &Workspace, analyst: Vec<MockTurn>) -> WorkspaceRoster<MockCompletionModel> {
    WorkspaceRoster::with_models(
        ws.xservice.clone(),
        RoleModels {
            graph_navigator: MockCompletionModel::new([]),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([MockTurn::text("synthesized answer")]),
        },
        MockCompletionModel::new(analyst),
    )
}

/// The names of a registered definition list, in order.
fn names(defs: &[agent_core::rig::completion::ToolDefinition]) -> Vec<&str> {
    defs.iter().map(|d| d.name.as_str()).collect()
}

/// The observations a turn recorded for `role`.
fn observations_of(sink: &CapturingSink, role: StepRole) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter_map(|e| match e {
            OrchestratorEvent::StepObserved { role: observed, summary, .. } if observed == role => {
                Some(summary)
            }
            _ => None,
        })
        .collect()
}

/// The Workspace-Analyst observations a turn recorded.
fn analyst_observations(sink: &CapturingSink) -> Vec<String> {
    observations_of(sink, StepRole::WorkspaceAnalyst)
}

/// Run one planner→analyst→final turn under the shipped `[chat]` budget
/// defaults, returning the analyst's observation and the global calls used.
async fn run_turn(ws: &Workspace, question: &str, analyst: Vec<MockTurn>) -> (String, usize) {
    let orchestrator = Orchestrator::new(
        MockCompletionModel::new([plan(question), finalize()]),
        workspace_roster(ws, analyst),
        BudgetTree::from(&ChatConfig::default()),
    );
    let sink = CapturingSink::new();
    let outcome = orchestrator.run(question, &sink).await.expect("the turn runs");
    assert!(matches!(outcome, TurnOutcome::Answered(_)), "{outcome:?}");
    let observations = analyst_observations(&sink);
    assert_eq!(observations.len(), 1, "one analyst step: {observations:?}");
    (observations[0].clone(), orchestrator.budget().global_used())
}

// ── 1. The registered rosters (ADR-52, ADR-71) ───────────────────────────────

#[tokio::test]
async fn a_single_backing_registers_todays_graph_tools_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/lib.rs", "pub fn alpha() {}\n");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine"));
    let sandbox = Arc::new(Sandbox::new(dir.path(), std::iter::empty()).expect("sandbox"));

    // What a single-root backing hands a workspace roster: nothing — `federated`
    // refuses it, so no workspace roster can be built over a single root.
    let single = Arc::new(Backing::Single(Arc::clone(&engine)));
    let xservice = XserviceBacking::federated(single, Arc::new(ContractBridge::new()));
    assert!(xservice.is_none(), "a single root mints no xservice backing");

    let roster = SubagentRoster::new(Arc::clone(&engine), sandbox, MockCompletionModel::new([]));
    let registered = roster.registered_tools(StepRole::GraphNavigator).await;
    assert_eq!(names(&registered), TODAYS_GRAPH_TOOLS);
    assert_eq!(TODAYS_GRAPH_TOOLS, ToolDomain::Graph.tool_names());

    // Byte-for-byte: every registered definition (name, description, schema) is
    // exactly what the graph domain alone registers.
    let graph_only = graph_toolset(engine).get_tool_definitions().await.unwrap();
    assert_eq!(
        serde_json::to_string(&registered).unwrap(),
        serde_json::to_string(&graph_only).unwrap()
    );
    assert_eq!(roster.preamble(StepRole::GraphNavigator), GRAPH_NAVIGATOR_PREAMBLE);
    assert_eq!(roster.preamble(StepRole::Synthesizer), SYNTHESIZER_PREAMBLE);
    assert_eq!(roster.planner_preamble(), DEFAULT_PLANNER_PREAMBLE);
}

/// [S-481]: the member roster's federated branch is retired. Built over a
/// member engine the federated backing itself resolved — what `chat_for` hands
/// it for a `?repo=` member — it registers, role by role, exactly what the
/// single-backing domains register, under exactly today's preambles: the
/// federated backing adds nothing. (No roster parameter accepts an
/// `XserviceBacking` any more; that half of the claim is the type's.)
#[tokio::test]
async fn a_federated_backing_handed_to_the_member_roster_adds_nothing() {
    let ws = workspace(false);
    let engine = ws.xservice.registry().engine_for("api").expect("api resolves");
    let api_root = ws.xservice.registry().federation().members[0].root.clone();
    let sandbox = Arc::new(Sandbox::new(&api_root, std::iter::empty()).expect("sandbox"));
    let roster = SubagentRoster::new(Arc::clone(&engine), Arc::clone(&sandbox), MockCompletionModel::new([]));

    for (role, single) in [
        (StepRole::GraphNavigator, graph_toolset(Arc::clone(&engine))),
        (StepRole::GovernanceAnalyst, governance_toolset(Arc::clone(&engine))),
        (StepRole::SourceReader, source_toolset(Arc::clone(&sandbox))),
    ] {
        let registered = roster.registered_tools(role).await;
        assert_eq!(
            serde_json::to_string(&registered).unwrap(),
            serde_json::to_string(&single.get_tool_definitions().await.unwrap()).unwrap(),
            "{role:?}"
        );
        assert!(!names(&registered).iter().any(|n| n.starts_with("xservice_")), "{role:?}");
    }
    assert!(roster.registered_tools(StepRole::Synthesizer).await.is_empty());
    assert!(roster.registered_tools(StepRole::WorkspaceAnalyst).await.is_empty());
    for (role, today) in [
        (StepRole::GraphNavigator, GRAPH_NAVIGATOR_PREAMBLE),
        (StepRole::GovernanceAnalyst, GOVERNANCE_ANALYST_PREAMBLE),
        (StepRole::SourceReader, SOURCE_READER_PREAMBLE),
        (StepRole::Synthesizer, SYNTHESIZER_PREAMBLE),
    ] {
        assert_eq!(roster.preamble(role), today, "{role:?}");
    }
    assert_eq!(roster.planner_preamble(), DEFAULT_PLANNER_PREAMBLE);
}

/// Re-targeted from S-431's `a_federated_backing_adds_the_four_xservice_tools_after_the_graph_tools`:
/// the `xservice_*` tools now follow the five workspace tools on the
/// Workspace-Analyst, and the other three tool-bearing roles are repo-addressed.
#[tokio::test]
async fn the_workspace_roster_registers_the_workspace_analyst_and_the_repo_addressed_roles() {
    let ws = workspace(false);
    let roster = workspace_roster(&ws, Vec::new());

    let expected: Vec<&str> =
        WORKSPACE_TOOL_NAMES.iter().chain(XSERVICE_TOOLS.iter()).copied().collect();
    assert_eq!(
        expected[..5],
        ["workspace_status", "workspace_reachability", "workspace_check", "xservice_build_deps", "workspace_roster"]
    );
    assert_eq!(names(&roster.registered_tools(StepRole::WorkspaceAnalyst).await), expected);
    // Each member role carries its domain's tools, in the domain's order, each
    // exactly S-480's repo-addressed definition — `repo` required.
    for (role, domain) in [
        (StepRole::GraphNavigator, ToolDomain::Graph),
        (StepRole::GovernanceAnalyst, ToolDomain::Governance),
        (StepRole::SourceReader, ToolDomain::Source),
    ] {
        let registered = roster.registered_tools(role).await;
        assert_eq!(names(&registered), domain.tool_names(), "{role:?}");
        let addressed = addressed_toolset(domain, ws.xservice.clone())
            .get_tool_definitions()
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_string(&registered).unwrap(),
            serde_json::to_string(&addressed).unwrap(),
            "{role:?}"
        );
        for def in &registered {
            let required = def.parameters["required"].as_array().cloned().unwrap_or_default();
            assert!(required.iter().any(|r| r == "repo"), "{role:?} {}: {required:?}", def.name);
        }
        assert!(!names(&registered).iter().any(|n| n.starts_with("xservice_")), "{role:?}");
    }
    assert!(roster.registered_tools(StepRole::Synthesizer).await.is_empty());

    // Each tool-bearing role runs under its workspace preamble, and the analyst's
    // carries the cross-service guidance the member Graph-Navigator's addendum did.
    for (role, preamble) in [
        (StepRole::WorkspaceAnalyst, WORKSPACE_ANALYST_PREAMBLE),
        (StepRole::GraphNavigator, WORKSPACE_GRAPH_NAVIGATOR_PREAMBLE),
        (StepRole::GovernanceAnalyst, WORKSPACE_GOVERNANCE_ANALYST_PREAMBLE),
        (StepRole::SourceReader, WORKSPACE_SOURCE_READER_PREAMBLE),
    ] {
        assert_eq!(roster.preamble(role), preamble, "{role:?}");
    }
    let analyst = roster.preamble(StepRole::WorkspaceAnalyst);
    for tool in &expected {
        assert!(analyst.contains(tool), "the analyst's preamble names {tool}: {analyst}");
    }
    assert!(analyst.contains("UNRESOLVED") && analyst.contains("canonical `symbol`"), "{analyst}");
    for role in [StepRole::GraphNavigator, StepRole::GovernanceAnalyst, StepRole::SourceReader] {
        let preamble = roster.preamble(role);
        assert!(preamble.contains("required `repo`"), "{role:?}: {preamble}");
    }
    // The Synthesizer writes the answer from the readings, so it is told that an
    // UNRESOLVED reading beats a subagent's "none".
    let synthesizer = roster.preamble(StepRole::Synthesizer);
    assert!(
        synthesizer.contains("the reading wins") && synthesizer.contains("UNRESOLVED"),
        "{synthesizer}"
    );
    assert!(synthesizer.contains("\"Workspace readings\""), "{synthesizer}");
}

/// The planner and Synthesizer preambles name every member with its declared
/// kind, and the default member — and rendering them, like listing every role's
/// tools, opens no member ([NFR-PE-10]).
#[tokio::test]
async fn the_workspace_preambles_name_every_member_with_its_declared_kind() {
    let tmp = TempDir::new().unwrap();
    let xservice = federated(federation(
        tmp.path(),
        &[("api", None), ("web", Some(MemberKind::Platform)), ("docs", Some(MemberKind::Documentation))],
        Some("api"),
    ));
    let roster = WorkspaceRoster::new(xservice.clone(), MockCompletionModel::new([]));

    let planner = roster.planner_preamble();
    let synthesizer = roster.preamble(StepRole::Synthesizer);
    for preamble in [&planner, &synthesizer] {
        assert!(preamble.contains("WORKSPACE \"shop\""), "{preamble}");
        assert!(preamble.contains("The workspace has 3 member(s)"), "{preamble}");
        assert!(preamble.contains("\n- api: no declared kind (the default member)"), "{preamble}");
        assert!(preamble.contains("\n- web: platform\n"), "{preamble}");
        assert!(preamble.contains("\n- docs: documentation"), "{preamble}");
    }
    assert_eq!(planner, chat_agent::workspace_planner_preamble(xservice.registry().federation()));
    // The planner is told of every role the roster carries, by its wire name.
    for role in ["workspace_analyst", "graph_navigator", "governance_analyst", "source_reader", "synthesizer"] {
        assert!(planner.contains(&format!("- {role}:")), "{role}: {planner}");
    }

    for role in [
        StepRole::WorkspaceAnalyst,
        StepRole::GraphNavigator,
        StepRole::GovernanceAnalyst,
        StepRole::SourceReader,
    ] {
        let _ = roster.registered_tools(role).await;
        let _ = roster.preamble(role);
    }
    assert_eq!(xservice.registry().resident_count(), 0, "rendering and listing opens no member");
}

/// [BR-59], pinned on the text: a cross-member answer ranks members by their own
/// named signals and never states a mean, sum or workspace score.
#[tokio::test]
async fn the_workspace_synthesizer_preamble_carries_the_br59_ranking_clause() {
    let ws = workspace(false);
    let synthesizer = workspace_roster(&ws, Vec::new()).preamble(StepRole::Synthesizer);
    assert!(
        synthesizer.contains(
            "When you compare members, rank them by their own named signals or findings, citing \
             each with the member it belongs to and its baseline."
        ),
        "{synthesizer}"
    );
    assert!(
        synthesizer.contains(
            "Never state a mean, sum, weighted figure or workspace score of per-member signals"
        ),
        "{synthesizer}"
    );
    assert!(
        synthesizer.contains(
            "A workspace-level figure enters the answer only when a workspace tool computed it \
             from workspace-level evidence."
        ),
        "{synthesizer}"
    );
    // The member Synthesizer is told none of it.
    assert!(!SYNTHESIZER_PREAMBLE.contains("rank"), "{SYNTHESIZER_PREAMBLE}");
}

/// Re-homed from `web`'s `a_workspace_turn_dispatches_xservice_through_launch`
/// (S-431), whose member-service wiring S-481 retired: what the models are
/// actually sent. The planner runs under the workspace planner preamble, the
/// analyst under its own, and the Synthesizer (the last request) under the
/// workspace Synthesizer's.
#[tokio::test]
async fn a_workspace_turn_runs_the_planner_and_synthesizer_under_the_workspace_preambles() {
    let ws = workspace(false);
    // One model backs the planner and every role, consumed in order:
    // plan → the analyst's tool call → its summary → final → answer.
    let model = MockCompletionModel::new([
        plan("which services define shared?"),
        MockTurn::tool_call("x1", "xservice_search", serde_json::json!({ "query": "shared" })),
        MockTurn::text("searched."),
        finalize(),
        MockTurn::text("answer"),
    ]);
    let roster = WorkspaceRoster::new(ws.xservice.clone(), model.clone());
    let planner = Planner::with_preamble(model.clone(), roster.planner_preamble());
    let orchestrator =
        Orchestrator::with_planner(planner, roster, BudgetTree::from(&ChatConfig::default()));
    let sink = CapturingSink::new();
    let outcome = orchestrator.run("which services define shared?", &sink).await.expect("runs");
    assert!(matches!(outcome, TurnOutcome::Answered(_)), "{outcome:?}");

    let federation = ws.xservice.registry().federation();
    let prompts = model.system_prompts();
    assert_eq!(
        prompts.first().cloned().flatten(),
        Some(chat_agent::workspace_planner_preamble(federation))
    );
    let analyst = prompts.get(1).cloned().flatten().unwrap_or_default();
    assert!(analyst.starts_with(WORKSPACE_ANALYST_PREAMBLE), "{analyst}");
    assert_eq!(
        prompts.last().cloned().flatten(),
        Some(chat_agent::workspace_synthesizer_preamble(federation))
    );
    let observation = &analyst_observations(&sink)[0];
    assert!(observation.contains("xservice_search \"shared\" over 2 member(s)"), "{observation}");
}

/// A repo-addressed role narrows to the member the step names: a Graph-Navigator
/// `search` with `repo: "api"` opens exactly `api`, and a member tool's output
/// carries no workspace reading.
#[tokio::test]
async fn a_repo_addressed_step_opens_only_the_member_it_names() {
    let ws = workspace(false);
    let registry = ws.xservice.registry();
    let orchestrator = Orchestrator::new(
        MockCompletionModel::new([plan_for("graph_navigator", "find shared in api"), finalize()]),
        WorkspaceRoster::with_models(
            ws.xservice.clone(),
            RoleModels {
                graph_navigator: MockCompletionModel::new([
                    MockTurn::tool_call(
                        "g1",
                        "search",
                        serde_json::json!({ "query": "shared", "repo": "api" }),
                    ),
                    MockTurn::text("api defines shared."),
                ]),
                governance_analyst: MockCompletionModel::new([]),
                source_reader: MockCompletionModel::new([]),
                synthesizer: MockCompletionModel::new([MockTurn::text("answer")]),
            },
            MockCompletionModel::new([]),
        ),
        BudgetTree::from(&ChatConfig::default()),
    );
    let sink = CapturingSink::new();
    orchestrator.run("where is shared in api?", &sink).await.expect("runs");

    let observations = observations_of(&sink, StepRole::GraphNavigator);
    assert_eq!(observations, vec!["api defines shared.".to_string()]);
    assert_eq!(orchestrator.budget().global_used(), 1);
    assert_eq!(registry.resident_members(), vec!["api".to_string()]);
}

// ── 2. Repo-qualified results ────────────────────────────────────────────────

#[tokio::test]
async fn a_cross_service_turn_dispatches_xservice_and_cites_each_member_separately() {
    let ws = workspace(false);
    let (observation, used) = run_turn(
        &ws,
        "which services define shared?",
        vec![
            MockTurn::tool_call("x1", "xservice_search", serde_json::json!({ "query": "shared" })),
            MockTurn::text("shared is defined in the workspace."),
        ],
    )
    .await;

    assert_eq!(used, 1, "one xservice call was dispatched and charged");
    assert!(observation.contains("xservice_search \"shared\" over 2 member(s)"), "{observation}");
    // The same symbol in two members is two repo-qualified results, never one.
    assert!(observation.contains("api:shared (src/lib.rs:1)"), "{observation}");
    assert!(observation.contains("web:shared (src/lib.rs:1)"), "{observation}");
}

/// The workspace read-models' readings ride the observation the way the
/// `xservice_*` ones do: `workspace_roster`'s line — engine-free — is appended
/// verbatim under the readings heading, and opens no member.
#[tokio::test]
async fn a_workspace_tool_reading_reaches_the_observation_verbatim() {
    let ws = workspace(false);
    let (observation, used) = run_turn(
        &ws,
        "which members does this workspace have?",
        vec![
            MockTurn::tool_call("w1", "workspace_roster", serde_json::json!({})),
            MockTurn::text("two members."),
        ],
    )
    .await;
    assert_eq!(used, 1);
    assert_eq!(
        observation,
        "two members.\n\nWorkspace readings (verbatim tool results):\n\
         - workspace_roster \"shop\" — 2 member(s): api; web"
    );
    assert_eq!(ws.xservice.registry().resident_count(), 0, "the roster tool opens no member");
}

// ── 3. The residue reaches the answer (BR-53) ────────────────────────────────

/// The model's own summary claims "none"; the observation must not let it stand.
fn callers_turn(symbol: &str) -> Vec<MockTurn> {
    vec![
        MockTurn::tool_call("x1", "xservice_callers", serde_json::json!({ "symbol": symbol })),
        MockTurn::text("No other service calls shared."),
    ]
}

#[tokio::test]
async fn an_empty_answer_over_a_non_zero_residue_reaches_the_observation_as_unresolved() {
    let ws = workspace(true);
    let (observation, _) = run_turn(&ws, "which services call shared?", callers_turn(&ws.shared)).await;

    assert!(
        observation.contains("cross-service: UNRESOLVED, not an absence — no resolved cross-service callers; 1 of 1 captured outbound site"),
        "the empty answer is rendered unresolved, naming the count: {observation}"
    );
    assert!(
        observation.contains("base-url-runtime"),
        "…and its reason: {observation}"
    );
    assert!(
        observation.contains("api:api_only"),
        "the intra-repo callers stay repo-qualified: {observation}"
    );
}

#[tokio::test]
async fn an_empty_answer_over_a_zero_residue_stands_unqualified() {
    let ws = workspace(false);
    let (observation, _) = run_turn(&ws, "which services call shared?", callers_turn(&ws.shared)).await;

    assert!(
        observation.contains("cross-service: no resolved cross-service callers |"),
        "{observation}"
    );
    assert!(
        !observation.contains("UNRESOLVED") && !observation.contains("unresolved outbound"),
        "a zero residue adds no qualification: {observation}"
    );
}

#[tokio::test]
async fn impact_carries_the_residue_the_same_way() {
    let ws = workspace(true);
    let (observation, _) = run_turn(
        &ws,
        "what breaks across services if shared changes?",
        vec![
            MockTurn::tool_call("x1", "xservice_impact", serde_json::json!({ "symbol": ws.shared })),
            MockTurn::text("Nothing outside this service."),
        ],
    )
    .await;
    assert!(
        observation.contains("— cross-service: UNRESOLVED, not an absence — no resolved cross-service impact"),
        "{observation}"
    );
}

/// The residue lives in `api` (its runtime-composed call); scoped to `web`, the
/// empty answer must say it measured only web's residue — never a clean absence.
#[tokio::test]
async fn a_scoped_empty_answer_names_whose_residue_it_measured() {
    let ws = workspace(true);
    let (observation, _) = run_turn(
        &ws,
        "who calls shared from web?",
        vec![
            MockTurn::tool_call(
                "x1",
                "xservice_callers",
                serde_json::json!({ "symbol": ws.shared, "repo": "web" }),
            ),
            MockTurn::text("nobody."),
        ],
    )
    .await;
    assert!(
        observation.contains(
            "cross-service: no resolved cross-service callers (residue measured over web's own outbound calls only"
        ),
        "{observation}"
    );
}

#[tokio::test]
async fn a_bare_name_is_reported_not_checked_never_as_an_absence() {
    let ws = workspace(false);
    let (observation, _) = run_turn(&ws, "which services call shared?", callers_turn("shared")).await;
    assert!(
        observation.contains("cross-service: NOT CHECKED, not an absence — \"shared\" is not a canonical symbol"),
        "{observation}"
    );
}

/// The non-empty path on a real edge: `xservice_route_providers` names the
/// binding, and `xservice_callers` over the provider's canonical symbol reports
/// the consumer in the other member — while the bare handler name, which the
/// cross-service tier cannot match, reads NOT CHECKED rather than "none".
#[tokio::test]
async fn a_resolved_edge_is_cited_with_both_members_and_a_bare_name_is_not_an_absence() {
    let ws = workspace_of(Shape::Bound);
    let providers = agent_core::xservice_toolset(ws.xservice.clone())
        .call("xservice_route_providers", "{}".to_string())
        .await
        .expect("route providers");
    let providers: serde_json::Value = serde_json::from_str(&providers).unwrap();
    let route = providers["providers"][0]["to"]["symbol"]
        .as_str()
        .unwrap_or_else(|| panic!("the fixture binds one route: {providers}"))
        .to_string();

    let (observation, used) = run_turn(
        &ws,
        "which services call the users route?",
        vec![
            MockTurn::tool_call("x1", "xservice_route_providers", serde_json::json!({})),
            MockTurn::tool_call("x2", "xservice_callers", serde_json::json!({ "symbol": route })),
            MockTurn::tool_call("x3", "xservice_callers", serde_json::json!({ "symbol": "get_user" })),
            MockTurn::text("web calls it."),
        ],
    )
    .await;
    assert_eq!(used, 3);
    assert!(
        observation.contains("xservice_route_providers — 1 resolved cross-service binding(s): web:"),
        "{observation}"
    );
    assert!(
        observation.contains("cross-service: 1 resolved cross-service caller(s) (web:")
            && observation.contains(&format!("→ api:{route} [route]")),
        "the consumer and the provider are each named with their member: {observation}"
    );
    assert!(
        observation.contains("cross-service: NOT CHECKED, not an absence — \"get_user\""),
        "{observation}"
    );
}

// ── 4. Lazy construction (NFR-PE-10) ─────────────────────────────────────────

/// Re-targeted from S-431's `the_federated_roster_opens_no_member_and_only_a_scoped_search_opens_just_one`.
#[tokio::test]
async fn the_workspace_roster_opens_no_member_and_only_a_scoped_search_opens_just_one() {
    let ws = workspace(false);
    let registry = ws.xservice.registry();

    let roster = workspace_roster(&ws, Vec::new());
    let _ = roster.registered_tools(StepRole::WorkspaceAnalyst).await;
    assert_eq!(
        registry.resident_count(),
        0,
        "building and listing the xservice tools opens no member"
    );

    let (observation, _) = run_turn(
        &ws,
        "where is shared in api?",
        vec![
            MockTurn::tool_call(
                "x1",
                "xservice_search",
                serde_json::json!({ "query": "shared", "repo": "api" }),
            ),
            MockTurn::text("shared is in api."),
        ],
    )
    .await;
    assert!(observation.contains("api:shared"), "{observation}");
    assert_eq!(
        registry.resident_members(),
        vec!["api".to_string()],
        "a repo-scoped call opens exactly that member"
    );
    assert!(registry.live_read_connections() <= registry.budget().total_read_connections());

    // The bridge-backed tools are NOT lazy per `repo`: the contract bridge reads
    // every member's sync-stamp whatever the scope, as the MCP twin does. Pinned
    // so the tool text cannot drift back to promising otherwise — and the
    // residency still stays inside the registry's budget.
    let (observation, _) = run_turn(
        &ws,
        "who calls shared in api?",
        vec![
            MockTurn::tool_call(
                "x1",
                "xservice_callers",
                serde_json::json!({ "symbol": ws.shared, "repo": "api" }),
            ),
            MockTurn::text("shared is called in api."),
        ],
    )
    .await;
    assert!(
        observation.contains("per member — api:") && !observation.contains("web:"),
        "`repo` scopes the per-member answer to api: {observation}"
    );
    assert_eq!(
        registry.resident_members(),
        vec!["api".to_string(), "web".to_string()],
        "a scoped reachability call still opens every member (the bridge walk)"
    );
    assert!(registry.resident_count() <= registry.budget().max_resident_members());
    assert!(registry.live_read_connections() <= registry.budget().total_read_connections());
}

// ── 5. The budget tree (FR-CF-06, CR-137 CRA-04) ─────────────────────────────

/// The representative cross-service turn the CRA-04 finding is measured on:
/// "which services call X?" — locate X across the workspace, then ask for its
/// cross-service callers, then answer. Two tool calls, against the shipped
/// `[chat]` ceilings.
#[tokio::test]
async fn a_representative_cross_service_turn_stays_well_inside_the_shipped_ceilings() {
    let ws = workspace(true);
    let defaults = BudgetTree::from(&ChatConfig::default());
    let (observation, used) = run_turn(
        &ws,
        "which services call shared?",
        vec![
            MockTurn::tool_call("x1", "xservice_search", serde_json::json!({ "query": "shared" })),
            MockTurn::tool_call("x2", "xservice_callers", serde_json::json!({ "symbol": ws.shared })),
            MockTurn::text("shared has no resolved cross-service caller."),
        ],
    )
    .await;

    assert_eq!(used, 2, "search + callers");
    assert!(used <= defaults.max_subagent_tool_calls(), "{used} of {}", defaults.max_subagent_tool_calls());
    assert!(used <= defaults.global_limit(), "{used} of {}", defaults.global_limit());
    eprintln!(
        "CRA-04: representative cross-service turn = {used} tool call(s) of a {}-call \
         per-subagent cap and a {}-call global ceiling",
        defaults.max_subagent_tool_calls(),
        defaults.global_limit()
    );
    // Both readings ride the one observation, in dispatch order.
    let search = observation.find("xservice_search").expect("search reading");
    let callers = observation.find("xservice_callers").expect("callers reading");
    assert!(search < callers, "{observation}");
}

/// The Workspace-Analyst's own per-subagent cap: its step draws a budget of its
/// own, and reaching it soft-closes that step with the reading kept.
#[tokio::test]
async fn a_subagent_cap_halts_the_step_honestly_and_keeps_the_readings() {
    let ws = workspace(true);
    let orchestrator = Orchestrator::new(
        MockCompletionModel::new([plan("which services call shared?"), finalize()]),
        workspace_roster(
            &ws,
            vec![
                MockTurn::tool_call("x1", "xservice_callers", serde_json::json!({ "symbol": ws.shared })),
                // Over the 1-call cap below: refused, the step soft-closes.
                MockTurn::tool_call("x2", "xservice_impact", serde_json::json!({ "symbol": ws.shared })),
                MockTurn::text("partial summary"),
            ],
        ),
        BudgetTree::new(48, 1, 3),
    );
    let sink = CapturingSink::new();
    orchestrator.run("which services call shared?", &sink).await.expect("the turn runs");

    let observations = analyst_observations(&sink);
    assert_eq!(observations.len(), 1, "{observations:?}");
    let observation = &observations[0];
    assert!(
        observation.starts_with("[bounded — reached the 1-tool-call subagent cap"),
        "the bound is named: {observation}"
    );
    assert!(
        observation.contains("— cross-service: UNRESOLVED"),
        "the reading gathered before the bound survives the close-out: {observation}"
    );
    assert_eq!(orchestrator.budget().global_used(), 1, "the refused call charged nothing");
}

/// The other two soft close-outs carry the readings too: a run of tool errors
/// (out-of-domain requests after the reading was gathered), and a spent global
/// ceiling. Each is named in the marker, and the UNRESOLVED reading survives.
#[tokio::test]
async fn the_tool_error_and_global_ceiling_close_outs_keep_the_readings() {
    for (budget, misroutes, marker) in [
        (BudgetTree::new(48, 16, 3), 3, "[bounded — hit 3 consecutive tool errors"),
        (BudgetTree::new(1, 16, 3), 1, "[bounded — reached the turn's 1-tool-call ceiling"),
    ] {
        let ws = workspace(true);
        let mut analyst = vec![MockTurn::tool_call(
            "x0",
            "xservice_callers",
            serde_json::json!({ "symbol": ws.shared }),
        )];
        for i in 0..misroutes {
            // `read` is a source tool, outside the Workspace-Analyst's domain.
            analyst.push(MockTurn::tool_call(
                format!("m{i}"),
                "read",
                serde_json::json!({ "path": "src/lib.rs" }),
            ));
        }
        analyst.push(MockTurn::text("partial summary"));
        let orchestrator = Orchestrator::new(
            MockCompletionModel::new([plan("which services call shared?"), finalize()]),
            workspace_roster(&ws, analyst),
            budget,
        );
        let sink = CapturingSink::new();
        let _ = orchestrator.run("which services call shared?", &sink).await;

        let observations = analyst_observations(&sink);
        assert_eq!(observations.len(), 1, "{marker}: {observations:?}");
        let observation = &observations[0];
        assert!(observation.starts_with(marker), "the bound is named: {observation}");
        assert!(
            observation.contains("— cross-service: UNRESOLVED"),
            "{marker}: the reading survives the close-out: {observation}"
        );
    }
}

/// `repo` reaches every tool that takes it: impact's seed and route-providers'
/// provider filter are scoped to the named member.
#[tokio::test]
async fn repo_scopes_impact_and_route_providers() {
    let ws = workspace_of(Shape::Bound);
    let (observation, _) = run_turn(
        &ws,
        "what does changing shared in api break, and which routes do web and api provide?",
        vec![
            MockTurn::tool_call(
                "x1",
                "xservice_impact",
                serde_json::json!({ "symbol": ws.shared, "repo": "api" }),
            ),
            MockTurn::tool_call("x2", "xservice_route_providers", serde_json::json!({ "repo": "web" })),
            MockTurn::tool_call("x3", "xservice_route_providers", serde_json::json!({ "repo": "api" })),
            MockTurn::text("done."),
        ],
    )
    .await;
    let seed = observation
        .split("seed per member — ")
        .nth(1)
        .unwrap_or_else(|| panic!("an impact reading: {observation}"));
    let seed = seed.lines().next().unwrap_or_default();
    assert!(seed.starts_with("api:") && !seed.contains("web:"), "{seed}");
    assert!(
        observation.contains("xservice_route_providers — no resolved cross-service bindings provided by web"),
        "{observation}"
    );
    assert!(
        observation.contains("xservice_route_providers — 1 resolved cross-service binding(s) provided by api:"),
        "{observation}"
    );
}
