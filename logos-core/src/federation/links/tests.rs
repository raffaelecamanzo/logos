//! Unit tests for `[[links]]` resolution ([FR-WS-41]): one fixture per valid and
//! invalid link shape, the redundant link, and the two ambiguity-preservation
//! cases, over fake member engines whose surfaces and lookups the test states
//! exactly. The real engine lookup is driven end-to-end in
//! `tests/workspace_links.rs`.
//!
//! [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::*;
use crate::federation::bridge::{ContractBridge, ContractNode};
use crate::federation::coverage::cross_service_coverage;
use crate::federation::registry::{EngineRegistry, RegistryMode};
use crate::federation::{Exclusion, Member};
use crate::model::NodeKind;

/// One fake member: its contract surface and what each target resolves to.
#[derive(Default, Clone)]
struct FakeMember {
    surface: Vec<ContractNode>,
    targets: HashMap<String, Vec<&'static str>>,
}

thread_local! {
    static FIXTURES: RefCell<HashMap<String, FakeMember>> = RefCell::new(HashMap::new());
    /// Every member read, of any kind — to prove a link-less workspace reads none.
    static READS: Cell<usize> = const { Cell::new(0) };
    /// `(member, symbol)` → the route/handler counterparts its store states.
    static COUNTERPARTS: RefCell<HashMap<(String, String), Vec<&'static str>>> = RefCell::new(HashMap::new());
}

fn reset() {
    FIXTURES.with(|f| f.borrow_mut().clear());
    COUNTERPARTS.with(|c| c.borrow_mut().clear());
    READS.with(|c| c.set(0));
}

/// State that `symbol` in `member` is one endpoint with `counterparts` — a route
/// and the handler(s) it serves, or a handler and its route(s).
fn set_counterparts(member: &str, symbol: &str, counterparts: &[&'static str]) {
    COUNTERPARTS.with(|c| {
        c.borrow_mut().insert((member.to_string(), symbol.to_string()), counterparts.to_vec());
    });
}

fn set_member(name: &str, member: FakeMember) {
    FIXTURES.with(|f| {
        f.borrow_mut().insert(name.to_string(), member);
    });
}

/// A fake member engine. A member named `"broken"` fails to start and one named
/// `"unreadable"` starts but fails its lookup — the two `member-degraded` arms.
#[derive(Debug)]
struct FakeEngine {
    member: String,
}

impl MemberEngine for FakeEngine {
    type Watcher = ();
    fn start(
        root: &Path,
        _read_connections: usize,
        _worker_pool: crate::SharedWorkerPool,
    ) -> Result<Arc<Self>> {
        let member = root
            .strip_prefix("/ws")
            .expect("a member root sits under the workspace")
            .to_string_lossy()
            .into_owned();
        if member == "broken" {
            anyhow::bail!("store is corrupt");
        }
        Ok(Arc::new(FakeEngine { member }))
    }
    fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
        Ok(())
    }
}

impl MemberContracts for FakeEngine {
    fn contract_surface(&self) -> Result<Vec<ContractNode>> {
        READS.with(|c| c.set(c.get() + 1));
        Ok(FIXTURES.with(|f| f.borrow().get(&self.member).map(|m| m.surface.clone()).unwrap_or_default()))
    }
    fn contract_stamp(&self) -> u64 {
        0
    }
    fn route_counterparts(&self, symbol: &LogosSymbol) -> Result<Vec<LogosSymbol>> {
        READS.with(|c| c.set(c.get() + 1));
        if self.member == "routes-unreadable" {
            anyhow::bail!("edge read failed");
        }
        Ok(COUNTERPARTS.with(|c| {
            c.borrow()
                .get(&(self.member.clone(), symbol.as_str().to_string()))
                .map(|symbols| symbols.iter().map(|s| sym(s)).collect())
                .unwrap_or_default()
        }))
    }
    fn resolve_target(&self, target: &str) -> Result<Vec<LogosSymbol>> {
        READS.with(|c| c.set(c.get() + 1));
        if self.member == "unreadable" {
            anyhow::bail!("store read failed");
        }
        Ok(FIXTURES.with(|f| {
            f.borrow()
                .get(&self.member)
                .and_then(|m| m.targets.get(target))
                .map(|symbols| symbols.iter().map(|s| sym(s)).collect())
                .unwrap_or_default()
        }))
    }
}

fn sym(symbol: &str) -> LogosSymbol {
    LogosSymbol::parse(symbol).expect("a parseable test symbol")
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

fn registry(names: &[&str], links: Vec<Link>) -> EngineRegistry<FakeEngine> {
    registry_excluding(names, links, Exclusion::default())
}

fn registry_excluding(names: &[&str], links: Vec<Link>, exclusion: Exclusion) -> EngineRegistry<FakeEngine> {
    let root = PathBuf::from("/ws");
    let federation = Federation {
        name: "w".to_string(),
        members: names
            .iter()
            .map(|name| Member {
                name: (*name).to_string(),
                root: root.join(name),
            })
            .collect(),
        root,
        default: None,
        links,
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
        exclusion,
    };
    EngineRegistry::new(federation, RegistryMode::Lazy)
}

/// A member whose targets each resolve to the listed symbols.
fn member_with(targets: &[(&str, &[&'static str])]) -> FakeMember {
    FakeMember {
        surface: Vec::new(),
        targets: targets.iter().map(|(t, s)| ((*t).to_string(), s.to_vec())).collect(),
    }
}

/// `web` calls `fetch_cart`, `api` serves `get_cart` — each exactly one node.
fn web_and_api() {
    set_member("web", member_with(&[("fetch_cart", &["local fetch_cart"])]));
    set_member("api", member_with(&[("get_cart", &["local get_cart"])]));
}

/// Every link's row, over `registry`, with the evidence the bridge computes.
fn statuses(registry: &EngineRegistry<FakeEngine>) -> Vec<LinkStatus> {
    workspace_links(&registry.answer())
}

fn invalid(status: &LinkStatus) -> &InvalidLink {
    match &status.verdict {
        LinkVerdict::Invalid(invalid) => invalid,
        other => panic!("expected an invalid link, got {other:?}"),
    }
}

/// **A valid link yields exactly one asserted edge** with the mapped relation
/// class, both endpoints resolved in their own member ([FR-WS-41] AC2).
///
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
#[test]
fn a_valid_link_yields_one_asserted_edge_with_the_mapped_relation_class() {
    for (relation, class) in [
        (LinkRelation::HttpCall, "route"),
        (LinkRelation::GrpcCall, "grpc-call"),
        (LinkRelation::Publishes, "broker-topic"),
    ] {
        reset();
        web_and_api();
        let registry = registry(&["web", "api"], vec![link(relation, "web::fetch_cart", "api::get_cart")]);
        let rows = statuses(&registry);
        assert_eq!(rows.len(), 1);
        let edges = asserted_edges(&rows);
        assert_eq!(edges.len(), 1, "{relation:?}: one link, one edge");
        let edge = &edges[0];
        assert_eq!(edge.intake, BridgeIntake::Asserted, "{relation:?}");
        assert_eq!(edge.relation, class, "{relation:?}");
        assert_eq!(
            (edge.from.member.as_str(), edge.from.symbol.as_str()),
            ("web", "local fetch_cart")
        );
        assert_eq!((edge.to.member.as_str(), edge.to.symbol.as_str()), ("api", "local get_cart"));
        // Both ends are read verbatim from the manifest: no configuration was
        // consulted, so neither carries `config-bound` evidence it does not have.
        assert_eq!(
            (&edge.from_value, &edge.to_value),
            (&Provenance::Literal, &Provenance::Literal),
            "{relation:?}"
        );
        assert_eq!(rows[0].verdict, LinkVerdict::Valid { edge: edge.clone() });
    }
}

/// The wire shape of a valid row: the declaration as written, `status`, and the
/// edge labelled `asserted`.
#[test]
fn a_valid_row_serializes_the_declaration_its_status_and_an_asserted_edge() {
    reset();
    web_and_api();
    let registry = registry(&["web", "api"], vec![http("web::fetch_cart", "api::get_cart")]);
    let row = serde_json::to_value(&statuses(&registry)[0]).unwrap();
    assert_eq!(row["relation"], "http_call");
    assert_eq!(row["from"], "web::fetch_cart");
    assert_eq!(row["to"], "api::get_cart");
    assert_eq!(row["status"], "valid");
    assert_eq!(row["edge"]["intake"], "asserted");
    assert_eq!(row["edge"]["relation"], "route");
    assert_eq!(row["edge"]["to"]["member"], "api");
}

/// **unknown-member**: an undeclared member, an endpoint with no `<member>::`,
/// and a member `[workspace] exclude` removed ([FR-WS-40]) — each yields no
/// edge, names the endpoint, and says which of the three it is.
///
/// [FR-WS-40]: ../../../../docs/specs/requirements/FR-WS-40.md
#[test]
fn an_endpoint_naming_no_member_is_unknown_member() {
    reset();
    web_and_api();
    let exclusion = Exclusion {
        globs: vec!["legacy*".to_string()],
        excluded: vec!["legacy-api".to_string()],
        conflicts: Vec::new(),
    };
    let registry = registry_excluding(
        &["web", "api"],
        vec![
            http("web::fetch_cart", "ghost::get_cart"),
            http("fetch_cart", "api::get_cart"),
            http("web::fetch_cart", "legacy-api::get_cart"),
            // A near miss: a member name that is a prefix of a real one.
            http("we::fetch_cart", "api::get_cart"),
            // The other near miss: a real member name that is a prefix of the
            // endpoint's member segment, and a member named with no `::` at all.
            http("webx::fetch_cart", "api::get_cart"),
            http("web", "api::get_cart"),
        ],
        exclusion,
    );
    let rows = statuses(&registry);
    assert!(asserted_edges(&rows).is_empty());
    assert_eq!(rows.len(), 6);
    let expected = [
        (LinkEnd::To, "member `ghost`, which is not a member of workspace `w`"),
        (LinkEnd::From, "is not `<member>::<target>`"),
        (LinkEnd::To, "member `legacy-api`, which `[workspace] exclude` removed"),
        (LinkEnd::From, "member `we`, which is not a member"),
        (LinkEnd::From, "member `webx`, which is not a member"),
        (LinkEnd::From, "is not `<member>::<target>`"),
    ];
    for (row, (end, says)) in rows.iter().zip(expected) {
        let invalid = invalid(row);
        assert_eq!(invalid.reason, InvalidReason::UnknownMember, "{row:?}");
        assert_eq!(invalid.endpoint, Some(end), "{row:?}");
        assert!(invalid.detail.contains(says), "{}: {}", says, invalid.detail);
    }
}

/// **member-degraded**: a member that will not open, and one whose lookup read
/// fails, are each reported degraded — never `no-match`, which would claim the
/// target absent from a member nobody read.
#[test]
fn a_member_that_cannot_be_read_is_member_degraded_never_no_match() {
    reset();
    web_and_api();
    let registry = registry(
        &["web", "api", "broken", "unreadable"],
        vec![
            http("web::fetch_cart", "broken::get_cart"),
            http("unreadable::fetch_cart", "api::get_cart"),
        ],
    );
    let rows = statuses(&registry);
    assert!(asserted_edges(&rows).is_empty());
    for (row, end, cause) in [
        (&rows[0], LinkEnd::To, "store is corrupt"),
        (&rows[1], LinkEnd::From, "store read failed"),
    ] {
        let invalid = invalid(row);
        assert_eq!(invalid.reason, InvalidReason::MemberDegraded, "{row:?}");
        assert_eq!(invalid.endpoint, Some(end));
        assert!(invalid.detail.contains(cause), "{}", invalid.detail);
    }
}

/// **no-match**: the member was read and nothing in it is the target — including
/// an endpoint with nothing after `::`.
#[test]
fn a_target_matching_nothing_is_no_match() {
    reset();
    web_and_api();
    let registry = registry(
        &["web", "api"],
        vec![http("web::fetch_cart", "api::get_kart"), http("web::fetch_cart", "api::")],
    );
    let rows = statuses(&registry);
    assert!(asserted_edges(&rows).is_empty());
    for row in &rows {
        let invalid = invalid(row);
        assert_eq!(invalid.reason, InvalidReason::NoMatch, "{row:?}");
        assert_eq!(invalid.endpoint, Some(LinkEnd::To));
        assert!(invalid.candidates.is_none());
    }
    assert!(invalid(&rows[0]).detail.contains("`get_kart` matches no declaration in member `api`"));
}

/// **ambiguous**: a target matching several nodes yields no edge and lists its
/// candidates — all of them under the bound with `omitted: 0` stated, and at
/// most `CANDIDATE_LIMIT` above it with the remainder counted.
#[test]
fn an_ambiguous_target_lists_bounded_candidates_and_counts_the_rest() {
    const MANY: [&str; 10] = [
        "local c0", "local c1", "local c2", "local c3", "local c4", "local c5", "local c6",
        "local c7", "local c8", "local c9",
    ];
    reset();
    web_and_api();
    set_member(
        "other",
        member_with(&[("get_cart", &["local get_cart_b", "local get_cart_a"]), ("handler", &MANY)]),
    );
    let registry = registry(
        &["web", "api", "other"],
        vec![http("web::fetch_cart", "other::get_cart"), http("web::fetch_cart", "other::handler")],
    );
    let rows = statuses(&registry);
    assert!(asserted_edges(&rows).is_empty());

    let two = invalid(&rows[0]);
    assert_eq!(two.reason, InvalidReason::Ambiguous);
    assert_eq!(two.endpoint, Some(LinkEnd::To));
    let candidates = two.candidates.as_ref().expect("an ambiguous link lists its candidates");
    assert_eq!(
        candidates.symbols.iter().map(LogosSymbol::as_str).collect::<Vec<_>>(),
        ["local get_cart_a", "local get_cart_b"],
        "listed in symbol order"
    );
    assert_eq!((candidates.total, candidates.omitted), (2, 0));
    assert!(two.detail.contains("matches 2 nodes in member `other` (2 listed)"), "{}", two.detail);

    let ten = invalid(&rows[1]);
    let candidates = ten.candidates.as_ref().unwrap();
    assert_eq!(candidates.symbols.len(), CANDIDATE_LIMIT);
    assert_eq!((candidates.total, candidates.omitted), (10, 10 - CANDIDATE_LIMIT as u64));
    assert_eq!(candidates.symbols[0].as_str(), "local c0");
}

/// **same-member**: both endpoints in one member is not a cross-service edge,
/// decided before any lookup — even when both targets resolve.
#[test]
fn both_endpoints_in_one_member_is_same_member() {
    reset();
    set_member("web", member_with(&[("a", &["local a"]), ("b", &["local b"])]));
    let registry = registry(&["web", "api"], vec![http("web::a", "web::b")]);
    // `resolve_links` directly, without the evidence walk `workspace_links`
    // makes for any declared link: the claim is about this link's own lookups.
    let rows = resolve_links(&registry.answer(), &[]);
    assert!(asserted_edges(&rows).is_empty());
    let invalid = invalid(&rows[0]);
    assert_eq!(invalid.reason, InvalidReason::SameMember);
    assert_eq!(invalid.endpoint, None, "same-member is about the pair");
    assert_eq!(READS.with(Cell::get), 0, "no member read for a link judged on its names alone");
}

/// The member is the **longest** member name the endpoint starts with, so a
/// nested member path and a target holding `::` both parse.
///
/// Two members can only both prefix an endpoint when one name itself holds
/// `::` (a member is a directory path; `svc` does not prefix `svc/web::…` with
/// `::`), so the overlap is staged with a directory named `api::v2`: the
/// endpoint `api::v2::get` is member `api::v2`, target `get` — not member `api`,
/// target `v2::get`, which resolves to something else.
#[test]
fn the_longest_member_prefix_wins_and_the_target_may_hold_a_path_separator() {
    reset();
    set_member("api", member_with(&[("v2::get", &["local wrong_member"])]));
    set_member("api::v2", member_with(&[("get", &["local v2_get"])]));
    set_member("svc/web", member_with(&[("fetch::inner", &["local fetch_inner"])]));
    let registry = registry(
        &["api", "api::v2", "svc/web"],
        vec![http("svc/web::fetch::inner", "api::v2::get")],
    );
    let edges = asserted_edges(&statuses(&registry));
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].from.member.as_str(), edges[0].from.symbol.as_str()),
        ("svc/web", "local fetch_inner"),
        "a nested member path, a target holding `::`"
    );
    assert_eq!(
        (edges[0].to.member.as_str(), edges[0].to.symbol.as_str()),
        ("api::v2", "local v2_get"),
        "the longer of two overlapping member names wins"
    );
}

/// Only a declaration a coupling can start or end at is a link endpoint: never a
/// documentation node or a whole file, and every code and contract-surface kind
/// otherwise.
#[test]
fn documentation_and_whole_file_nodes_are_never_link_endpoints() {
    for kind in NodeKind::ALL {
        let expected = !(kind.is_documentation() || matches!(kind, NodeKind::Module | NodeKind::ConfigFile));
        assert_eq!(is_link_endpoint(kind), expected, "{kind:?}");
    }
    for kind in [NodeKind::Function, NodeKind::Method, NodeKind::Route, NodeKind::ApiOperation, NodeKind::ProtoService, NodeKind::Topic] {
        assert!(is_link_endpoint(kind), "{kind:?} is a coupling endpoint");
    }
}

fn op(name: &str, symbol: &str) -> ContractNode {
    ContractNode {
        kind: NodeKind::ApiOperation,
        name: name.to_string(),
        symbol: sym(symbol),
    }
}

fn route(name: &str, symbol: &str) -> ContractNode {
    ContractNode {
        kind: NodeKind::Route,
        name: name.to_string(),
        symbol: sym(symbol),
    }
}

/// `web`'s operation `GET /users/{id}` and `api`'s matching route: the bridge
/// binds them, one evidence edge.
fn bound_pair() {
    set_member(
        "web",
        FakeMember {
            surface: vec![op("GET /users/{id}", "local get_user_op")],
            targets: [("get_user_op".to_string(), vec!["local get_user_op"])].into(),
        },
    );
    set_member(
        "api",
        FakeMember {
            surface: vec![route("GET /users/{user_id}", "local users_route")],
            targets: [("users_route".to_string(), vec!["local users_route"])].into(),
        },
    );
}

/// **redundant**: a link duplicating the evidence edge the bridge already holds
/// yields no second edge and names the one it duplicates ([FR-WS-41] AC4).
///
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
#[test]
fn a_link_duplicating_an_evidence_edge_is_redundant_naming_it() {
    reset();
    bound_pair();
    let registry = registry(&["web", "api"], vec![http("web::get_user_op", "api::users_route")]);
    let evidence = ContractBridge::new().edges(&registry);
    assert_eq!(evidence.len(), 1, "the fixture binds one evidence edge");

    let rows = statuses(&registry);
    assert!(asserted_edges(&rows).is_empty(), "no second edge");
    let LinkVerdict::Redundant { duplicates } = &rows[0].verdict else {
        panic!("expected redundant, got {:?}", rows[0].verdict);
    };
    assert_eq!(duplicates, &evidence[0], "names the evidence edge");
    assert!(duplicates.intake.is_evidence());
    let row = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(row["status"], "redundant");
    // An operation→route match: the evidence edge it names says so.
    assert_eq!(row["duplicates"]["intake"], "contract-surface");
}

/// Redundancy needs **both** endpoints: a link sharing only its `to`, or only its
/// `from`, with the evidence edge is a different coupling and stays valid.
#[test]
fn a_link_sharing_one_endpoint_with_an_evidence_edge_is_not_redundant() {
    reset();
    bound_pair();
    set_member(
        "web",
        FakeMember {
            surface: vec![op("GET /users/{id}", "local get_user_op")],
            targets: [
                ("get_user_op".to_string(), vec!["local get_user_op"]),
                ("other_caller".to_string(), vec!["local other_caller"]),
            ]
            .into(),
        },
    );
    set_member(
        "api",
        FakeMember {
            surface: vec![route("GET /users/{user_id}", "local users_route")],
            targets: [
                ("users_route".to_string(), vec!["local users_route"]),
                ("other_route".to_string(), vec!["local other_route"]),
            ]
            .into(),
        },
    );
    let registry = registry(
        &["web", "api"],
        vec![
            http("web::other_caller", "api::users_route"),
            http("web::get_user_op", "api::other_route"),
        ],
    );
    assert_eq!(ContractBridge::new().edges(&registry).len(), 1, "one evidence edge");
    let rows = statuses(&registry);
    for row in &rows {
        assert!(matches!(row.verdict, LinkVerdict::Valid { .. }), "{row:?}");
    }
    assert_eq!(asserted_edges(&rows).len(), 2);
}

/// A link repeating an earlier valid link is redundant too, naming the first
/// one's asserted edge — two declarations of one coupling are one edge.
#[test]
fn a_link_repeating_an_earlier_link_is_redundant_naming_its_edge() {
    reset();
    web_and_api();
    let registry = registry(
        &["web", "api"],
        vec![http("web::fetch_cart", "api::get_cart"), http("web::fetch_cart", "api::get_cart")],
    );
    let rows = statuses(&registry);
    let edges = asserted_edges(&rows);
    assert_eq!(edges.len(), 1);
    assert_eq!(rows[1].verdict, LinkVerdict::Redundant { duplicates: edges[0].clone() });
}

/// Only the **evidence** edges of the set handed in judge redundancy: an asserted
/// edge passed in (a caller holding both) does not make a link redundant.
#[test]
fn only_evidence_edges_in_the_given_set_make_a_link_redundant() {
    reset();
    web_and_api();
    let registry = registry(&["web", "api"], vec![http("web::fetch_cart", "api::get_cart")]);
    let answer = registry.answer();
    let asserted = asserted_edges(&resolve_links(&answer, &[]));
    assert_eq!(asserted.len(), 1);
    let again = resolve_links(&answer, &asserted);
    assert_eq!(asserted_edges(&again), asserted, "an asserted input is not evidence");
}

/// **Asserted edges never enter the exactly-one matcher — an evidence tie stays a
/// tie** ([FR-WS-41] AC5, [NFR-RA-05]). `web`'s operation ties between `api`'s
/// and `other`'s routes; a link naming one of them is valid, yet the evidence
/// edge set stays empty and the coverage row stays ambiguous, exactly as
/// without the link.
///
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_link_naming_one_tied_provider_leaves_the_evidence_tie_ambiguous() {
    reset();
    bound_pair();
    set_member(
        "other",
        FakeMember {
            surface: vec![route("GET /users/{uid}", "local other_route")],
            targets: HashMap::new(),
        },
    );
    let names = ["web", "api", "other"];
    let without = registry(&names, Vec::new());
    let with = registry(&names, vec![http("web::get_user_op", "api::users_route")]);

    let evidence = ContractBridge::new().edges(&with);
    assert!(evidence.is_empty(), "the tie binds nothing: {evidence:?}");
    assert_eq!(evidence, ContractBridge::new().edges(&without));
    let coverage = |r: &EngineRegistry<FakeEngine>| serde_json::to_value(cross_service_coverage(&r.answer())).unwrap();
    assert_eq!(coverage(&with), coverage(&without), "coverage is byte-identical with the link");
    assert_eq!(coverage(&with)["ambiguous"], 1);

    let rows = statuses(&with);
    assert_eq!(asserted_edges(&rows).len(), 1, "the link itself is valid");
}

/// **Adding a link never turns an evidence binding ambiguous** ([FR-WS-41] AC5):
/// a link from the bound operation to a *second* member's node — a node that,
/// were it fed to the matcher as a provider of the key, would make two providers
/// and break the binding — leaves the evidence edge and coverage exactly as they
/// were.
///
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
#[test]
fn adding_a_link_never_turns_an_evidence_binding_ambiguous() {
    reset();
    bound_pair();
    set_member("other", member_with(&[("users_v2", &["local users_v2"])]));
    let names = ["web", "api", "other"];
    let without = registry(&names, Vec::new());
    let with = registry(&names, vec![http("web::get_user_op", "other::users_v2")]);

    let evidence = ContractBridge::new().edges(&with);
    assert_eq!(evidence.len(), 1, "still bound to `api`'s route alone");
    assert_eq!(evidence, ContractBridge::new().edges(&without));
    let coverage = |r: &EngineRegistry<FakeEngine>| serde_json::to_value(cross_service_coverage(&r.answer())).unwrap();
    assert_eq!(coverage(&with), coverage(&without));
    assert_eq!(coverage(&with)["bound"], 1);
    assert_eq!(coverage(&with)["ambiguous"], 0);

    let rows = statuses(&with);
    assert!(matches!(rows[0].verdict, LinkVerdict::Valid { .. }), "{rows:?}");
}

/// A workspace with no `[[links]]` reads **no member at all** for them — the
/// evidence edges a link is judged against are computed only when there is a
/// link to judge.
#[test]
fn a_workspace_without_links_reads_nothing_for_them() {
    reset();
    bound_pair();
    let registry = registry(&["web", "api"], Vec::new());
    assert!(statuses(&registry).is_empty());
    assert_eq!(READS.with(Cell::get), 0);
}

// ── S-709: a route and the handler it serves are one endpoint ───────────────

/// `bound_pair`, with `api`'s route serving the handler `get_user` (and
/// `other_handler` serving no bound route).
fn bound_pair_with_handler() {
    bound_pair();
    set_member(
        "api",
        FakeMember {
            surface: vec![route("GET /users/{user_id}", "local users_route")],
            targets: [
                ("users_route".to_string(), vec!["local users_route"]),
                ("get_user".to_string(), vec!["local get_user"]),
                ("other_handler".to_string(), vec!["local other_handler"]),
            ]
            .into(),
        },
    );
    set_counterparts("api", "local users_route", &["local get_user"]);
    set_counterparts("api", "local get_user", &["local users_route"]);
}

/// **A link naming the handler of a route a measured edge binds is redundant,
/// naming that edge** — and so never listed beside it ([FR-WS-41], S-709): a
/// measured HTTP edge ends at the route node, and a route and the handler it
/// serves are one endpoint.
///
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
#[test]
fn a_link_naming_the_handler_of_a_bound_route_is_redundant_naming_that_edge() {
    reset();
    bound_pair_with_handler();
    let registry = registry(&["web", "api"], vec![http("web::get_user_op", "api::get_user")]);
    let evidence = ContractBridge::new().edges(&registry);
    assert_eq!(evidence.len(), 1, "the fixture binds one evidence edge, at the route");
    assert_eq!(evidence[0].to.symbol, sym("local users_route"));

    let rows = statuses(&registry);
    assert_eq!(
        rows[0].verdict,
        LinkVerdict::Redundant { duplicates: evidence[0].clone() },
        "the handler behind the bound route is that route's endpoint"
    );
    assert!(asserted_edges(&rows).is_empty(), "never listed beside the measured edge");
}

/// The lookup is symmetric: an edge already held at the **handler** (an earlier
/// link's) makes a link naming its **route** redundant.
#[test]
fn a_link_naming_the_route_of_a_held_handler_edge_is_redundant() {
    reset();
    bound_pair_with_handler();
    set_member(
        "web",
        FakeMember {
            surface: Vec::new(),
            targets: [("fetch".to_string(), vec!["local fetch"])].into(),
        },
    );
    let registry = registry(
        &["web", "api"],
        vec![http("web::fetch", "api::get_user"), http("web::fetch", "api::users_route")],
    );
    let rows = statuses(&registry);
    let edges = asserted_edges(&rows);
    assert_eq!(edges.len(), 1, "{rows:#?}");
    assert_eq!(edges[0].to.symbol, sym("local get_user"));
    assert_eq!(rows[1].verdict, LinkVerdict::Redundant { duplicates: edges[0].clone() });
}

/// The near miss: a handler **no** bound route serves is a different endpoint,
/// so its link is valid — the route↔handler lookup widens an endpoint to the
/// route serving it, never to every handler in the member.
#[test]
fn a_handler_no_bound_route_serves_is_not_redundant() {
    reset();
    bound_pair_with_handler();
    let registry = registry(&["web", "api"], vec![http("web::get_user_op", "api::other_handler")]);
    let rows = statuses(&registry);
    assert!(matches!(rows[0].verdict, LinkVerdict::Valid { .. }), "{rows:#?}");
}

/// A member whose routes cannot be read leaves redundancy unknown, so the link is
/// `member-degraded` — never a valid edge that might sit beside a measured one,
/// never redundant against an edge nobody read ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn a_route_lookup_that_fails_is_member_degraded() {
    reset();
    web_and_api();
    set_member("routes-unreadable", member_with(&[("get_cart", &["local get_cart"])]));
    let registry = registry(&["web", "routes-unreadable"], vec![http("web::fetch_cart", "routes-unreadable::get_cart")]);
    let rows = statuses(&registry);
    let invalid = invalid(&rows[0]);
    assert_eq!(invalid.reason, InvalidReason::MemberDegraded);
    assert_eq!(invalid.endpoint, Some(LinkEnd::To));
    assert!(invalid.detail.contains("edge read failed"), "{}", invalid.detail);
}

// ── S-709: counted beside the headline, pointed to from the residue ─────────

/// **`asserted_edges` counts every verdict** — valid, invalid by reason,
/// redundant — and its line names them as declared ([FR-WS-41], [BR-75]).
/// Absent without a link.
///
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
/// [BR-75]: ../../../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn asserted_edges_counts_valid_invalid_by_reason_and_redundant() {
    reset();
    web_and_api();
    let registry = registry(
        &["web", "api"],
        vec![
            http("web::fetch_cart", "api::get_cart"),
            http("web::fetch_cart", "api::get_cart"),
            http("web::fetch_cart", "api::nothing"),
            http("web::fetch_cart", "ghost::get_cart"),
            http("web::fetch_cart", "web::fetch_cart"),
        ],
    );
    let tally = AssertedEdges::tally(&statuses(&registry)).expect("links are declared");
    assert_eq!((tally.valid, tally.invalid, tally.redundant), (1, 3, 1));
    assert_eq!(
        tally.invalid_by_reason,
        BTreeMap::from([
            (InvalidReason::UnknownMember, 1),
            (InvalidReason::NoMatch, 1),
            (InvalidReason::SameMember, 1),
        ])
    );
    assert_eq!(
        tally.summary,
        "1 asserted edge from 5 declared links (3 invalid: unknown-member 1, no-match 1, \
         same-member 1; 1 redundant); declared, never counted as evidence"
    );
    let wire = serde_json::to_value(&tally).unwrap();
    assert_eq!(wire["invalid_by_reason"]["no-match"], 1, "{wire}");
    assert_eq!(AssertedEdges::tally(&[]), None, "no link, no tally");

    // The line's other shapes: one link, nothing invalid; two valid edges.
    let one = AssertedEdges::tally(&statuses(&registry_one())).unwrap();
    assert_eq!(
        one.summary,
        "1 asserted edge from 1 declared link (0 invalid; 0 redundant); declared, never counted as evidence"
    );
    reset();
    web_and_api();
    set_member("api", member_with(&[("get_cart", &["local get_cart"]), ("put_cart", &["local put_cart"])]));
    let two = self::registry(
        &["web", "api"],
        vec![http("web::fetch_cart", "api::get_cart"), http("web::fetch_cart", "api::put_cart")],
    );
    assert_eq!(
        AssertedEdges::tally(&statuses(&two)).unwrap().summary,
        "2 asserted edges from 2 declared links (0 invalid; 0 redundant); declared, never counted as evidence"
    );
}

/// `web_and_api` with the one valid link between them.
fn registry_one() -> EngineRegistry<FakeEngine> {
    reset();
    web_and_api();
    registry(&["web", "api"], vec![http("web::fetch_cart", "api::get_cart")])
}

/// A residue row is covered by a **valid** link whose asserted edge starts at
/// the row's own consumer endpoint, in the row's relation class — and by
/// nothing else: not an invalid or redundant link, not another endpoint, not
/// another class (the near misses), each one character from matching.
#[test]
fn only_a_valid_link_from_the_rows_endpoint_and_class_covers_it() {
    reset();
    web_and_api();
    set_member(
        "web",
        member_with(&[("fetch_cart", &["local fetch_cart"]), ("fetch_carts", &["local fetch_carts"])]),
    );
    let registry = registry(
        &["web", "api"],
        vec![
            http("web::fetch_cart", "api::get_cart"),
            http("web::fetch_cart", "api::nothing"),
            http("web::fetch_carts", "api::get_cart"),
        ],
    );
    let rows = statuses(&registry);
    let from = BridgeEndpoint {
        member: "web".to_string(),
        symbol: sym("local fetch_cart"),
    };
    assert_eq!(
        links_covering(&rows, "route", &from),
        [Link {
            relation: LinkRelation::HttpCall,
            from: "web::fetch_cart".to_string(),
            to: "api::get_cart".to_string(),
        }]
    );
    assert!(links_covering(&rows, "grpc-call", &from).is_empty(), "another class");
    let elsewhere = BridgeEndpoint {
        member: "api".to_string(),
        symbol: sym("local fetch_cart"),
    };
    assert!(links_covering(&rows, "route", &elsewhere).is_empty(), "same symbol, another member");
}

/// **Only a declared call seeds an asserted root** ([FR-WS-12], [FR-WS-41]):
/// an asserted `http_call` and `grpc_call` do, an asserted `publishes` and every
/// evidence intake do not — over every relation each could carry.
///
/// [FR-WS-12]: ../../../../docs/specs/requirements/FR-WS-12.md
/// [FR-WS-41]: ../../../../docs/specs/requirements/FR-WS-41.md
#[test]
fn only_an_asserted_call_seeds_an_asserted_root() {
    let edge = |intake, relation: &str| BridgeEdge {
        relation: relation.to_string(),
        from: BridgeEndpoint {
            member: "web".to_string(),
            symbol: sym("local a"),
        },
        to: BridgeEndpoint {
            member: "api".to_string(),
            symbol: sym("local b"),
        },
        intake,
        from_value: Provenance::Literal,
        to_value: Provenance::Literal,
    };
    for relation in LinkRelation::ALL {
        let class = relation.namespace().relation();
        assert_eq!(seeds_asserted_root(&edge(BridgeIntake::Asserted, class)), relation.is_call(), "{relation:?}");
        for evidence in [BridgeIntake::Invocation, BridgeIntake::ContractSurface] {
            assert!(!seeds_asserted_root(&edge(evidence, class)), "{evidence:?} over {class}");
        }
    }
    assert!(!seeds_asserted_root(&edge(BridgeIntake::Asserted, "routes")), "a near-miss class");
}

/// **`InvalidReason::as_str` is the token `serde` writes**, for every variant —
/// the composed `asserted_edges.summary` line spells reasons through it, and
/// `invalid_by_reason` keys them through serde, so the two must not drift. The
/// fixed-length array and the wildcard-free `match` make a new variant fail to
/// compile here until it is checked.
#[test]
fn every_invalid_reason_spells_its_serde_token() {
    const ALL: [InvalidReason; 5] = [
        InvalidReason::UnknownMember,
        InvalidReason::MemberDegraded,
        InvalidReason::NoMatch,
        InvalidReason::Ambiguous,
        InvalidReason::SameMember,
    ];
    for reason in ALL {
        match reason {
            InvalidReason::UnknownMember
            | InvalidReason::MemberDegraded
            | InvalidReason::NoMatch
            | InvalidReason::Ambiguous
            | InvalidReason::SameMember => {}
        }
        assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str(), "{reason:?}");
    }
}
