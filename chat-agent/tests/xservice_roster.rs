//! The Graph-Navigator's cross-service tool surface ([S-431], [FR-WS-29],
//! [ADR-52], [BR-53], [NFR-PE-10], [FR-CF-06]).
//!
//! Driven through the **public** roster/orchestrator API with the offline mock
//! `CompletionModel` over a real two-member federation — each member a real,
//! indexed store — so every `xservice_*` answer below is the thick-core
//! `federation::query` read-model the MCP tools return, not a stub.
//!
//! What is pinned, in the order the acceptance criteria state it:
//!
//! 1. the **registered** tool list: today's eight graph tools byte-for-byte under
//!    a single backing, those eight then the four `xservice_*` tools under a
//!    federated one ([ADR-52]);
//! 2. a cross-service turn dispatches an `xservice_*` tool and its observation
//!    names every hit with its member — a symbol in two members is two results;
//! 3. **the residue reaches the answer** ([BR-53]): an empty cross-service answer
//!    over the fixture's known, non-zero residue is carried into the observation
//!    as `UNRESOLVED` with its count and reason, even though the mock model's own
//!    summary says "none"; the zero-residue twin stands unqualified;
//! 4. the tools inherit the registry's laziness: building the federated roster
//!    opens no member, and a `repo`-scoped call opens exactly that one
//!    ([NFR-PE-10], counted on the registry the way
//!    `logos-core/tests/workspace_connection_budget.rs` counts it);
//! 5. the turn stays inside the shipped budget tree, and a bound halts honestly
//!    naming itself with the readings still carried ([FR-CF-06]).
//!
//! [S-431]: ../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
//! [FR-WS-29]: ../../docs/specs/requirements/FR-WS-29.md
//! [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
//! [BR-53]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
//! [FR-CF-06]: ../../docs/specs/requirements/FR-CF-06.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{
    graph_toolset, MockCompletionModel, MockTurn, Sandbox, ToolDomain, XserviceBacking,
};
use chat_agent::orchestrator::{
    BudgetTree, CapturingSink, Orchestrator, OrchestratorEvent, RoleModels, StepRole,
    SubagentRoster, TurnOutcome, GRAPH_NAVIGATOR_PREAMBLE, SYNTHESIZER_PREAMBLE,
};
use logos_core::config::ChatConfig;
use logos_core::federation::{
    Backing, ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::Engine;
use tempfile::TempDir;

/// The Graph-Navigator's registered tools as they stood before S-431 — the
/// literal list the single-backing roster must still register, in order.
const TODAYS_GRAPH_TOOLS: [&str; 8] = [
    "search", "context", "node", "callers", "callees", "impact", "explore", "affected",
];

/// The four tools S-431 composes on under a federated backing, in order.
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
    /// The member-scoped engine behind the eight graph tools — a separate
    /// project, so the registry's resident count reflects only what the
    /// `xservice_*` tools opened.
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
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

    let local = root.join("local");
    write(&local, "src/lib.rs", "pub fn alpha() {}\n");
    let engine = Arc::new(Engine::start(&local).expect("local engine"));
    let sandbox = Arc::new(Sandbox::new(&local, std::iter::empty()).expect("sandbox"));

    let federation = Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members: vec![
            Member { name: "api".to_string(), root: api },
            Member { name: "web".to_string(), root: web },
        ],
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    };
    let backing = Arc::new(Backing::Federated(Box::new(EngineRegistry::<Engine>::new(
        federation,
        RegistryMode::Lazy,
    ))));
    let xservice = XserviceBacking::federated(backing, Arc::new(ContractBridge::new()))
        .expect("a federated backing mints an xservice backing");
    Workspace { _tmp: tmp, xservice, engine, sandbox, shared }
}

/// JSON the mock planner returns for a single graph_navigator step.
fn plan(instruction: &str) -> MockTurn {
    MockTurn::text(format!(
        r#"{{"action":"plan","steps":[{{"role":"graph_navigator","instruction":"{instruction}"}}]}}"#
    ))
}

fn finalize() -> MockTurn {
    MockTurn::text(r#"{"action":"final","grounded":true}"#)
}

/// A roster whose Graph-Navigator runs `navigator`, over the federated backing.
fn federated_roster(ws: &Workspace, navigator: Vec<MockTurn>) -> SubagentRoster<MockCompletionModel> {
    SubagentRoster::with_models(
        Arc::clone(&ws.engine),
        Arc::clone(&ws.sandbox),
        RoleModels {
            graph_navigator: MockCompletionModel::new(navigator),
            governance_analyst: MockCompletionModel::new([]),
            source_reader: MockCompletionModel::new([]),
            synthesizer: MockCompletionModel::new([MockTurn::text("synthesized answer")]),
        },
    )
    .with_xservice(Some(ws.xservice.clone()))
}

/// The names of a registered definition list, in order.
fn names(defs: &[agent_core::rig::completion::ToolDefinition]) -> Vec<&str> {
    defs.iter().map(|d| d.name.as_str()).collect()
}

/// The Graph-Navigator observations a turn recorded.
fn navigator_observations(sink: &CapturingSink) -> Vec<String> {
    sink.events()
        .into_iter()
        .filter_map(|e| match e {
            OrchestratorEvent::StepObserved {
                role: StepRole::GraphNavigator,
                summary,
                ..
            } => Some(summary),
            _ => None,
        })
        .collect()
}

/// Run one planner→navigator→final turn under the shipped `[chat]` budget
/// defaults, returning the navigator's observation and the global calls used.
async fn run_turn(ws: &Workspace, question: &str, navigator: Vec<MockTurn>) -> (String, usize) {
    let orchestrator = Orchestrator::new(
        MockCompletionModel::new([plan(question), finalize()]),
        federated_roster(ws, navigator),
        BudgetTree::from(&ChatConfig::default()),
    );
    let sink = CapturingSink::new();
    let outcome = orchestrator.run(question, &sink).await.expect("the turn runs");
    assert!(matches!(outcome, TurnOutcome::Answered(_)), "{outcome:?}");
    let observations = navigator_observations(&sink);
    assert_eq!(observations.len(), 1, "one navigator step: {observations:?}");
    (observations[0].clone(), orchestrator.budget().global_used())
}

// ── 1. The registered roster (ADR-52) ────────────────────────────────────────

#[tokio::test]
async fn a_single_backing_registers_todays_graph_tools_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/lib.rs", "pub fn alpha() {}\n");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine"));
    let sandbox = Arc::new(Sandbox::new(dir.path(), std::iter::empty()).expect("sandbox"));

    // What a single-root backing hands the roster: nothing — `federated` refuses it.
    let single = Arc::new(Backing::Single(Arc::clone(&engine)));
    let xservice = XserviceBacking::federated(single, Arc::new(ContractBridge::new()));
    assert!(xservice.is_none(), "a single root mints no xservice backing");

    let roster = SubagentRoster::new(Arc::clone(&engine), sandbox, MockCompletionModel::new([]))
        .with_xservice(xservice);
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
}

#[tokio::test]
async fn a_federated_backing_adds_the_four_xservice_tools_after_the_graph_tools() {
    let ws = workspace(false);
    let roster = federated_roster(&ws, Vec::new());

    let expected: Vec<&str> =
        TODAYS_GRAPH_TOOLS.iter().chain(XSERVICE_TOOLS.iter()).copied().collect();
    assert_eq!(names(&roster.registered_tools(StepRole::GraphNavigator).await), expected);
    // No other role gains anything: the xservice tools are the Graph-Navigator's.
    for (role, domain) in [
        (StepRole::GovernanceAnalyst, ToolDomain::Governance),
        (StepRole::SourceReader, ToolDomain::Source),
    ] {
        assert_eq!(names(&roster.registered_tools(role).await), domain.tool_names(), "{role:?}");
    }
    assert!(roster.registered_tools(StepRole::Synthesizer).await.is_empty());
    // The preamble is today's, extended — never replaced.
    let preamble = roster.preamble(StepRole::GraphNavigator);
    assert!(preamble.starts_with(GRAPH_NAVIGATOR_PREAMBLE), "{preamble}");
    assert!(preamble.contains("xservice_callers") && preamble.contains("UNRESOLVED"));
    // The Synthesizer writes the answer from the readings, so it is told that an
    // UNRESOLVED reading beats a subagent's "none".
    let synthesizer = roster.preamble(StepRole::Synthesizer);
    assert!(synthesizer.starts_with(SYNTHESIZER_PREAMBLE), "{synthesizer}");
    assert!(
        synthesizer.contains("the reading wins") && synthesizer.contains("UNRESOLVED"),
        "{synthesizer}"
    );
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

#[tokio::test]
async fn the_federated_roster_opens_no_member_and_only_a_scoped_search_opens_just_one() {
    let ws = workspace(false);
    let registry = ws.xservice.registry();

    let roster = federated_roster(&ws, Vec::new());
    let _ = roster.registered_tools(StepRole::GraphNavigator).await;
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

#[tokio::test]
async fn a_subagent_cap_halts_the_step_honestly_and_keeps_the_readings() {
    let ws = workspace(true);
    let orchestrator = Orchestrator::new(
        MockCompletionModel::new([plan("which services call shared?"), finalize()]),
        federated_roster(
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

    let observations = navigator_observations(&sink);
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
