//! End-to-end test for asserted `[[links]]` in the answers (S-709, [CR-224],
//! [FR-WS-41], [BR-75]).
//!
//! S-708 validates a link into an `asserted` edge; this drives what the answers
//! do with it, over member repositories indexed by their own [`Engine`] with the
//! real `tree-sitter-rust` grammar, behind an [`EngineRegistry`] and one
//! [`ContractBridge`]. The fixture is the shape a link exists for: `web` binds
//! `api`'s route statically (`fetch_user`, the measured edge) and calls `api`'s
//! cart handler through a runtime-composed URL (`fetch_cart`, `base-url-runtime`
//! residue) that a link declares. `get_cart` and the `load_cart` it calls are
//! private and uncalled, so their own graph verdicts them dead. `fork` holds a
//! copy of `api`'s source under another route path — the same handlers at the
//! same descriptors, under its own coordinate ([CR-222]), so a symbol written
//! without a coordinate names the handler in both.
//!
//! It proves, per acceptance criterion:
//!
//! - `xservice callers` / `impact` carry the asserted edge in `via_asserted_link`,
//!   `intake: "asserted"` and `reached: "(asserted)"`, matched on the
//!   `(member, symbol)` pair: `api`'s symbol never answers for the fork's link;
//! - app-wide reachability makes the link's target and its callee
//!   `live-via-asserted-link`, never `live-via-cross-service`, and a `publishes`
//!   link seeds no root;
//! - `resolved_cross_service_edges`, egress resolution and
//!   `spec_conformance_ratio` — on `workspace status` and on the reachability
//!   rider — and every evidence tier of the answers are byte-identical with and
//!   without `[[links]]`;
//! - `workspace status` reports `asserted_edges` beside the headline, and the
//!   residue row the link covers keeps `base-url-runtime` and points at it;
//! - a link naming the handler of the route the measured edge binds is
//!   `redundant`, naming that edge, and is never listed beside it;
//! - renaming the link's target and syncing makes the link `no-match`, and its
//!   roots disappear on the next answer of the **same** bridge.
//!
//! [CR-222]: ../../docs/requests/CR-222-workspace-safe-symbol-identity.md
//! [CR-224]: ../../docs/requests/CR-224-asserted-cross-service-links.md
//! [FR-WS-41]: ../../docs/specs/requirements/FR-WS-41.md
//! [BR-75]: ../../docs/specs/software-spec.md#327-workspace-federation
//!
//! Gated on the Rust grammar so a build excluding it does not run it.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    app_wide_reachability, query, workspace_status, ContractBridge, EngineRegistry, Federation,
    Link, LinkRelation, Member, RegistryMode,
};
use logos_core::Engine;
use serde_json::Value;

#[path = "support/coordinate.rs"]
mod coordinate;

/// `web`: one static call the bridge binds, one runtime-composed call it cannot.
const WEB_LIB: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}

pub async fn fetch_cart(client: Client, url: String) {
    let _ = client.get(url).await;
}
"#;

/// `api` (and `fork`, whose route path differs so the measured edge stays exactly
/// one): the route `fetch_user` binds, served by `get_user`, and the private cart
/// handler a link names, calling `load_cart`.
const API_LIB: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

async fn get_cart() {
    load_cart();
}

fn load_cart() {}

pub fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own store, then drop the engine so the store is
/// closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

fn fixture(tmp: &Path) -> Vec<Member> {
    let members: Vec<Member> = [("web", WEB_LIB.to_string()), ("api", API_LIB.to_string()), ("fork", API_LIB.replace("/users/", "/forked/"))]
        .into_iter()
        .map(|(name, source)| {
            let root = tmp.join(name);
            write(&root, "src/lib.rs", &source);
            coordinate::pin_as(&root, &member_coordinate(name));
            index_member(&root);
            Member {
                name: name.to_string(),
                root,
            }
        })
        .collect();
    members
}

fn link(relation: LinkRelation, from: &str, to: &str) -> Link {
    Link {
        relation,
        from: from.to_string(),
        to: to.to_string(),
    }
}

fn http(from: &str, to: &str) -> Link {
    link(LinkRelation::HttpCall, from, to)
}

fn registry(root: &Path, members: &[Member], links: Vec<Link>) -> EngineRegistry<Engine> {
    EngineRegistry::<Engine>::new(
        Federation {
            name: "shop".to_string(),
            root: root.to_path_buf(),
            members: members.to_vec(),
            default: None,
            links,
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
            exclusion: Default::default(),
        },
        RegistryMode::Lazy,
    )
}

/// The coordinate a member is indexed under (S-703): its own, `fork` included —
/// two members sharing one would leave the later degraded (S-706).
fn member_coordinate(name: &str) -> String {
    format!("example.com/{name}")
}

/// The canonical symbol of `name` in `src/lib.rs`, under the coordinate of the
/// member declaring it: `web` declares the `fetch_*` clients, `api` every
/// handler (and `fork` its copy of each, see [`symbol_in`]).
fn symbol(name: &str) -> String {
    let member = if name.starts_with("fetch_") { "web" } else { "api" };
    symbol_in(member, name)
}

/// The canonical symbol of `name` in `member`'s `src/lib.rs`.
fn symbol_in(member: &str, name: &str) -> String {
    format!("logos . {} . src/`lib.rs`/{name}().", member_coordinate(member))
}

/// Every answer one surface would give over `registry`: status, callers and
/// impact on each cart symbol, and the reachability view.
struct Answers {
    status: Value,
    /// Unscoped over `get_cart` written without a coordinate, which `api` and
    /// `fork` both answer to: the grouped callers answer ([BR-73]).
    callers_get_cart: Value,
    /// Unscoped over `api`'s canonical `get_cart`: `api`'s alone.
    callers_get_cart_canonical: Value,
    callers_get_cart_in_api: Value,
    callers_get_cart_in_fork: Value,
    callers_get_user: Value,
    impact_fetch_cart: Value,
    impact_get_cart: Value,
    /// Unscoped over `get_cart` written without a coordinate, which `api` and
    /// `fork` both answer to: the grouped impact answer ([BR-73]) a link must
    /// not move.
    impact_get_cart_unscoped: Value,
    reach: Value,
}

fn answers(registry: &EngineRegistry<Engine>, bridge: &ContractBridge) -> Answers {
    let inputs = query::reachability_inputs(bridge, registry);
    let json = |value: &dyn erased::Json| value.json();
    Answers {
        status: json(&workspace_status(registry)),
        callers_get_cart: json(&query::xservice_callers(registry, &inputs, &old_form("get_cart"), None, None)),
        callers_get_cart_canonical: json(&query::xservice_callers(registry, &inputs, &symbol("get_cart"), None, None)),
        callers_get_cart_in_api: json(&query::xservice_callers(
            registry,
            &inputs,
            &symbol("get_cart"),
            None,
            Some("api"),
        )),
        callers_get_cart_in_fork: json(&query::xservice_callers(
            registry,
            &inputs,
            &symbol_in("fork", "get_cart"),
            None,
            Some("fork"),
        )),
        callers_get_user: json(&query::xservice_callers(registry, &inputs, &symbol("get_user"), None, None)),
        impact_fetch_cart: json(&query::xservice_impact(registry, &inputs, &symbol("fetch_cart"), None, None)),
        impact_get_cart: json(&query::xservice_impact(registry, &inputs, &symbol("get_cart"), None, Some("api"))),
        impact_get_cart_unscoped: json(&query::xservice_impact(registry, &inputs, &old_form("get_cart"), None, None)),
        reach: json(&app_wide_reachability(registry, &query::reach_edges(bridge, registry))),
    }
}

/// A `Serialize` value as JSON, behind a trait object so `answers` can name one
/// closure for every read-model type.
mod erased {
    pub trait Json {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).expect("read-model serializes")
        }
    }
}

/// `value` with the named keys removed, at the top level.
fn without(value: &Value, keys: &[&str]) -> String {
    let mut value = value.clone();
    for key in keys {
        value.as_object_mut().expect("an object").shift_remove(*key);
    }
    serde_json::to_string(&value).unwrap()
}

/// The three evidence figures a link must never move ([BR-75]), from one
/// coverage object (the status headline or the reachability rider).
fn evidence_figures(coverage: &Value) -> String {
    [
        "resolved_cross_service_edges",
        "egress_resolution",
        "egress_resolution_measured",
        "spec_conformance_ratio",
        "spec_conformance_measured",
    ]
    .iter()
    .map(|key| format!("{key}={}", coverage[key]))
    .collect::<Vec<_>>()
    .join(" ")
}

fn names(claims: &Value) -> Vec<&str> {
    claims
        .as_array()
        .map(|claims| claims.iter().map(|claim| claim["name"].as_str().unwrap()).collect())
        .unwrap_or_default()
}

#[test]
fn an_asserted_link_joins_callers_impact_and_reachability_labelled_and_moves_no_evidence() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);
    let links = vec![
        http("web::fetch_cart", "api::get_cart"),
        // The handler behind the route `fetch_user` already binds: one endpoint.
        http("web::fetch_user", "api::get_user"),
        // A publish reaches a topic, never a callable: no root.
        link(LinkRelation::Publishes, "web::fetch_user", "fork::get_cart"),
    ];
    let plain = answers(&registry(&root, &members, Vec::new()), &ContractBridge::new());
    let linked = answers(&registry(&root, &members, links), &ContractBridge::new());

    // ── callers: the asserted rows, labelled, keyed on (member, symbol) ──────
    // `api` and `fork` both answer to `get_cart` written without a coordinate:
    // unscoped, each link's row names its own member and that member's symbol;
    // scoped, or by `api`'s canonical symbol, each member answers for its own
    // link alone.
    let ends = |answer: &Value| -> Vec<(String, String)> {
        answer["via_asserted_link"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        assert_eq!(row["reached"], "(asserted)", "{row}");
                        assert_eq!(row["intake"], "asserted", "{row}");
                        assert_eq!(row["from"]["member"], "web", "{row}");
                        let member = row["to"]["member"].as_str().unwrap();
                        assert_eq!(row["to"]["symbol"], symbol_in(member, "get_cart"), "{row}");
                        (member.to_string(), row["relation"].as_str().unwrap().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let pair = |member: &str, relation: &str| (member.to_string(), relation.to_string());
    assert_eq!(ends(&linked.callers_get_cart), [pair("api", "route"), pair("fork", "broker-topic")]);
    assert_eq!(ends(&linked.callers_get_cart_canonical), [pair("api", "route")]);
    assert!(linked.callers_get_cart_canonical.get("ambiguity_warning").is_none(), "one holder");
    assert_eq!(ends(&linked.callers_get_cart_in_api), [pair("api", "route")]);
    assert_eq!(ends(&linked.callers_get_cart_in_fork), [pair("fork", "broker-topic")]);
    // Never in the measured tier, and never in its resolved count.
    assert_eq!(linked.callers_get_cart_in_api["cross_service"], Value::Array(Vec::new()));
    assert!(
        linked.callers_get_cart["unresolved_egress"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("no resolved cross-service callers"),
        "{}",
        linked.callers_get_cart["unresolved_egress"]
    );

    // ── the redundant handler link is never listed beside the measured edge ──
    assert!(
        linked.callers_get_user.get("via_asserted_link").is_none(),
        "a link naming the handler of a bound route adds no row: {}",
        linked.callers_get_user
    );
    let status_links = linked.status["links"].as_array().unwrap();
    assert_eq!(status_links[1]["status"], "redundant", "{status_links:#?}");
    assert_eq!(status_links[1]["duplicates"]["intake"], "invocation");
    assert!(
        status_links[1]["duplicates"]["to"]["symbol"].as_str().unwrap().contains("route/"),
        "the duplicated edge ends at the route serving the handler: {}",
        status_links[1]
    );

    // ── impact: stitched across the asserted edge, from either end ──────────
    let far = linked.impact_fetch_cart["via_asserted_link"].as_array().expect("stitched from the consumer");
    assert_eq!(far.len(), 1);
    assert_eq!(far[0]["reached"], "(asserted)");
    assert_eq!(far[0]["via"]["intake"], "asserted");
    assert_eq!(far[0]["member"], "api");
    assert_eq!(far[0]["impact"]["resolved"]["name"], "get_cart");
    let near = linked.impact_get_cart["via_asserted_link"].as_array().expect("stitched from the provider");
    assert_eq!(near[0]["member"], "web");
    assert_eq!(linked.impact_fetch_cart["cross_service"], Value::Array(Vec::new()));

    // ── reachability: a distinct verdict; a publish seeds nothing ────────────
    assert_eq!(names(&linked.reach["live_via_asserted_link"]), ["get_cart", "load_cart"], "{}", linked.reach);
    for claim in linked.reach["live_via_asserted_link"].as_array().unwrap() {
        assert_eq!(claim["verdict"], "live-via-asserted-link");
        assert_eq!(claim["member"], "api", "the publish into `fork` seeded no root there");
    }
    assert!(names(&linked.reach["live_via_cross_service"]).is_empty());
    assert_eq!(linked.reach["coverage"]["asserted_call_edges"], 1, "the call link alone");
    let fork = linked.reach["members"].as_array().unwrap().iter().find(|m| m["member"] == "fork").unwrap();
    assert!(fork.get("asserted_roots").is_none(), "{fork}");
    let api_dead: Vec<&str> = plain.reach["dead"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|claim| claim["member"] == "api")
        .map(|claim| claim["name"].as_str().unwrap())
        .collect();
    assert_eq!(api_dead, ["get_cart", "load_cart"], "without the link, both are dead app-wide");
    assert!(plain.reach.get("live_via_asserted_link").is_none());

    // Unscoped, the two holders' impact is grouped per member, and the asserted
    // rows stitched from both holders stay out of each holder's share.
    let grouped = &linked.impact_get_cart_unscoped;
    assert_eq!(grouped["resolved_by_member"].as_array().map(Vec::len), Some(2), "{grouped}");
    assert_eq!(grouped["via_asserted_link"].as_array().map(Vec::len), Some(2), "one row per holder's link: {grouped}");

    // ── the evidence figures and tiers are byte-identical ([BR-75]) ──────────
    assert_eq!(evidence_figures(&linked.status["coverage"]), evidence_figures(&plain.status["coverage"]));
    assert_eq!(evidence_figures(&linked.reach["coverage"]), evidence_figures(&plain.reach["coverage"]));
    assert_eq!(plain.status["coverage"]["resolved_cross_service_edges"], 1, "the figure is not vacuous");
    for (with, without_links) in [
        (&linked.callers_get_cart, &plain.callers_get_cart),
        (&linked.callers_get_cart_canonical, &plain.callers_get_cart_canonical),
        (&linked.callers_get_cart_in_api, &plain.callers_get_cart_in_api),
        (&linked.callers_get_cart_in_fork, &plain.callers_get_cart_in_fork),
        (&linked.callers_get_user, &plain.callers_get_user),
        (&linked.impact_fetch_cart, &plain.impact_fetch_cart),
        (&linked.impact_get_cart, &plain.impact_get_cart),
        (&linked.impact_get_cart_unscoped, &plain.impact_get_cart_unscoped),
    ] {
        assert_eq!(without(with, &["via_asserted_link"]), without(without_links, &[]), "only the asserted tier is added");
    }

    // ── status: counted beside the headline; the residue row points at it ───
    let tally = &linked.status["coverage"]["asserted_edges"];
    assert_eq!((tally["valid"].as_u64(), tally["redundant"].as_u64(), tally["invalid"].as_u64()), (Some(2), Some(1), Some(0)), "{tally}");
    assert!(plain.status["coverage"].get("asserted_edges").is_none());
    let residue = linked.status["coverage"]["references"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["from"]["symbol"] == symbol("fetch_cart"))
        .expect("the runtime-composed call is a residue row");
    assert_eq!(residue["reason"], "base-url-runtime", "it keeps its reason");
    assert_eq!(
        residue["asserted_by"],
        serde_json::json!([{ "relation": "http_call", "from": "web::fetch_cart", "to": "api::get_cart" }])
    );
}

/// **A renamed target invalidates its link, and its root disappears on the next
/// answer** — of the same bridge, whose cache the sync's advanced stamp misses
/// ([FR-WS-41], [CR-224] §7).
///
/// [FR-WS-41]: ../../docs/specs/requirements/FR-WS-41.md
/// [CR-224]: ../../docs/requests/CR-224-asserted-cross-service-links.md
#[test]
fn a_renamed_link_target_turns_no_match_and_its_root_disappears_on_the_next_answer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);
    let registry = registry(&root, &members, vec![http("web::fetch_cart", "api::get_cart")]);
    let bridge = ContractBridge::new();
    let reach = |registry: &EngineRegistry<Engine>| {
        serde_json::to_value(app_wide_reachability(registry, &query::reach_edges(&bridge, registry))).unwrap()
    };

    let before = reach(&registry);
    assert_eq!(names(&before["live_via_asserted_link"]), ["get_cart", "load_cart"], "{before}");

    let source = root.join("api/src/lib.rs");
    fs::write(&source, API_LIB.replace("get_cart", "get_basket")).unwrap();
    let api = registry.engine_for("api").expect("api starts");
    let _ = api.sync(&[source]);

    let after = reach(&registry);
    assert!(after.get("live_via_asserted_link").is_none(), "the root is gone: {after}");
    let link = &query::reachability_inputs(&bridge, &registry).links[0];
    let row = serde_json::to_value(link).unwrap();
    assert_eq!((row["status"].as_str(), row["reason"].as_str()), (Some("invalid"), Some("no-match")), "{row}");
}

/// The `api` route `fetch_user` binds — the measured edge's provider end.
const USERS_ROUTE: &str = "logos . example.com/api . src/`lib.rs`/route/`GET /users/{user_id}`#";

/// **A redundant link is never listed beside the edge it repeats** ([BR-75],
/// S-709): querying either end of the measured `fetch_user → route` edge, which
/// a link naming the route's handler duplicates, answers the measured row once
/// and no asserted row at all.
///
/// [BR-75]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn a_redundant_link_adds_no_asserted_row_on_either_end_of_the_edge_it_repeats() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);
    let registry = registry(&root, &members, vec![http("web::fetch_user", "api::get_user")]);
    let bridge = ContractBridge::new();
    let inputs = query::reachability_inputs(&bridge, &registry);
    let row = serde_json::to_value(&inputs.links[0]).unwrap();
    assert_eq!(row["status"], "redundant", "the fixture's link repeats the measured edge: {row}");

    let callers =
        serde_json::to_value(query::xservice_callers(&registry, &inputs, USERS_ROUTE, None, Some("api"))).unwrap();
    let impact =
        serde_json::to_value(query::xservice_impact(&registry, &inputs, &symbol("fetch_user"), None, Some("web")))
            .unwrap();
    for (answer, name) in [(&callers, "callers on the route"), (&impact, "impact on fetch_user")] {
        assert_eq!(answer["cross_service"].as_array().map(Vec::len), Some(1), "{name}: the measured row, once: {answer}");
        assert!(answer.get("via_asserted_link").is_none(), "{name}: no asserted row beside it: {answer}");
    }
}

/// **The store reads a route and its handler as one endpoint, both ways**
/// (S-709): an indexed route's counterparts are exactly the handler it routes to
/// (out of the route), and the handler's are exactly that route (into it). This
/// fixture's graph carries no self-`RoutesTo` dispatch marker, so the self-loop
/// exclusion is not exercised here.
#[test]
fn an_indexed_route_and_its_handler_are_each_others_counterparts() {
    use logos_core::federation::MemberContracts;
    use logos_core::model::LogosSymbol;
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);
    let registry = registry(&root, &members, Vec::new());
    let api = registry.engine_for("api").expect("api starts");
    let sym = |s: &str| LogosSymbol::parse(s).unwrap();
    assert_eq!(api.route_counterparts(&sym(USERS_ROUTE)).unwrap(), [sym(&symbol("get_user"))]);
    assert_eq!(api.route_counterparts(&sym(&symbol("get_user"))).unwrap(), [sym(USERS_ROUTE)]);
    assert!(api.route_counterparts(&sym(&symbol("get_cart"))).unwrap().is_empty(), "no route serves it");
}

/// `name` in `src/lib.rs` as a symbol written before symbols carried their
/// repository coordinate (S-705, [FR-EX-36]).
///
/// [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md
fn old_form(name: &str) -> String {
    format!("logos . . . src/`lib.rs`/{name}().")
}

/// The parts of a cross-service answer that must not depend on how the symbol
/// was spelled: the bound and asserted rows, each member's share, the residue
/// figure, and whether the [BR-73] notice is raised. The per-member rows echo
/// their query and carry the note, so they are left out.
///
/// [BR-73]: ../../docs/specs/software-spec.md#327-workspace-federation
fn cross_service_part(answer: &Value) -> String {
    format!(
        "{} {} {} {} warned={}",
        answer["cross_service"],
        answer["via_asserted_link"],
        answer["resolved_by_member"],
        answer["unresolved_egress"],
        answer["ambiguity_warning"].is_string(),
    )
}

/// **A symbol written without a coordinate answers `xservice callers` and
/// `impact` as its canonical form does** (S-705, [FR-EX-36]): each member's row
/// upgrades it to that member's coordinate, and the bound rows, the asserted
/// rows and the [BR-73] grouping follow the same upgrade. Scoped to `api`, it is
/// `api`'s canonical symbol. Unscoped it names `get_cart` in `api` and in `fork`,
/// each under its own coordinate, so it is grouped over both holders — rows
/// unchanged, `fork` at zero — where `api`'s canonical symbol names `api` alone.
///
/// [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md
/// [BR-73]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn an_old_form_symbol_answers_cross_service_callers_and_impact_as_its_canonical_form() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);
    let registry = registry(&root, &members, vec![http("web::fetch_cart", "api::get_cart")]);
    let bridge = ContractBridge::new();
    let inputs = query::reachability_inputs(&bridge, &registry);
    let json = |value: &dyn erased::Json| value.json();
    let old_route = USERS_ROUTE.replace("logos . example.com/api . ", "logos . . . ");

    for (canonical, old, repo) in [
        (USERS_ROUTE.to_string(), old_route, Some("api")),
        (symbol("get_cart"), old_form("get_cart"), Some("api")),
    ] {
        let callers = |s: &str| json(&query::xservice_callers(&registry, &inputs, s, None, repo));
        let impact = |s: &str| json(&query::xservice_impact(&registry, &inputs, s, None, repo));
        let (canonical_callers, canonical_impact) = (callers(&canonical), impact(&canonical));
        assert!(
            canonical_callers["cross_service"].as_array().is_some_and(|r| !r.is_empty())
                || canonical_callers["via_asserted_link"].as_array().is_some_and(|r| !r.is_empty()),
            "{canonical} {repo:?}: the canonical answer has cross-service rows: {canonical_callers}"
        );
        assert_eq!(
            cross_service_part(&callers(&old)),
            cross_service_part(&canonical_callers),
            "callers {repo:?}: {old}"
        );
        assert_eq!(
            cross_service_part(&impact(&old)),
            cross_service_part(&canonical_impact),
            "impact {repo:?}: {old}"
        );
    }
    let rows = |answer: &Value| format!("{} {}", answer["cross_service"], answer["via_asserted_link"]);
    let holders = serde_json::json!([{"member": "api", "resolved": 0}, {"member": "fork", "resolved": 0}]);
    for verb in ["callers", "impact"] {
        let answer = |s: &str| match verb {
            "callers" => json(&query::xservice_callers(&registry, &inputs, s, None, None)),
            _ => json(&query::xservice_impact(&registry, &inputs, s, None, None)),
        };
        let (grouped, canonical) = (answer(&old_form("get_cart")), answer(&symbol("get_cart")));
        assert!(canonical["via_asserted_link"].as_array().is_some_and(|r| !r.is_empty()), "{verb}: {canonical}");
        assert_eq!(rows(&grouped), rows(&canonical), "{verb}: the same rows");
        assert_eq!(grouped["resolved_by_member"], holders, "{verb}: {grouped}");
        let warning = grouped["ambiguity_warning"].as_str().unwrap_or_else(|| panic!("{verb}: {grouped}"));
        assert!(warning.contains("api, fork"), "{verb}: {warning}");
        assert!(canonical.get("ambiguity_warning").is_none() && canonical.get("resolved_by_member").is_none(), "{verb}: {canonical}");
    }
}

/// **A `[[links]]` endpoint written as a symbol without a coordinate stays
/// valid** (S-705, [FR-EX-36]): a 1.17.0 manifest naming its ends by full
/// symbol resolves each under its member's coordinate and asserts the very edge
/// the canonical spelling does. A coordinate the member does not carry is never
/// rewritten: that end is `no-match`.
///
/// [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md
#[test]
fn a_link_written_with_old_form_symbols_asserts_the_canonical_edge() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);
    // A fresh bridge per manifest: its link slot is keyed on the members'
    // stamps, which three manifests over one set of stores share.
    let link_row = |from: &str, to: &str| {
        let registry = registry(&root, &members, vec![http(from, to)]);
        serde_json::to_value(&query::reachability_inputs(&ContractBridge::new(), &registry).links[0]).unwrap()
    };

    let canonical = link_row(&format!("web::{}", symbol("fetch_cart")), &format!("api::{}", symbol("get_cart")));
    let old = link_row(&format!("web::{}", old_form("fetch_cart")), &format!("api::{}", old_form("get_cart")));
    assert_eq!(canonical["status"], "valid", "{canonical}");
    assert_eq!(old["status"], "valid", "{old}");
    assert_eq!(old["edge"], canonical["edge"], "the same asserted edge");

    let foreign = link_row(
        &format!("web::{}", symbol("fetch_cart")),
        "api::logos . example.com/other . src/`lib.rs`/get_cart().",
    );
    assert_eq!((foreign["status"].as_str(), foreign["reason"].as_str()), (Some("invalid"), Some("no-match")), "{foreign}");
}
