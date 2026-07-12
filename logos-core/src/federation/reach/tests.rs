//! Unit tests for the app-wide cross-service reachability union view
//! ([FR-WS-12], [ADR-56]).
//!
//! Driven with fake member engines (mirroring [`super::super::bridge`]'s doubles)
//! so the union composition — promotion, monotonicity, tri-state preservation,
//! the coverage rider — is exercised over surfaces the test states exactly. The
//! real engine path (a 2-repo workspace, real index) is proven end-to-end in
//! `tests/xservice_reachability.rs`.
//!
//! [FR-WS-12]: ../../../../docs/specs/requirements/FR-WS-12.md
//! [ADR-56]: ../../../../docs/specs/architecture/decisions/ADR-56.md

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::*;
use crate::federation::bridge::{BridgeEndpoint, ContractNode};
use crate::federation::registry::RegistryMode;
use crate::federation::{Federation, Member};
use crate::graph_store::NodeRow;

thread_local! {
    static FIXTURES: RefCell<HashMap<String, ReachabilitySurface>> =
        RefCell::new(HashMap::new());
}

fn reset() {
    FIXTURES.with(|f| f.borrow_mut().clear());
}

/// Give a member the reachability surface the union view will read.
fn set_surface(member: &str, surface: ReachabilitySurface) {
    FIXTURES.with(|f| {
        f.borrow_mut().insert(member.to_string(), surface);
    });
}

/// A fake member engine serving its reachability surface from the thread-local
/// fixtures. A member named `"broken"` fails to start and a member named
/// `"unreadable"` fails its read — the two degrade arms [`read_members`] handles.
#[derive(Debug)]
struct FakeEngine {
    member: String,
}

impl MemberEngine for FakeEngine {
    type Watcher = ();
    fn start(root: &Path) -> Result<Arc<Self>> {
        let member = root
            .file_name()
            .expect("a member root has a final component")
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
    /// No contract surface: the union view takes its edges from the caller, so the
    /// fakes need only serve reachability. This also keeps the coverage rider's
    /// reference counts at zero unless a test says otherwise — an honest "nothing
    /// to bind" ([ADR-53]).
    fn contract_surface(&self) -> Result<Vec<ContractNode>> {
        if self.member == "unreadable" {
            anyhow::bail!("store read failed");
        }
        Ok(Vec::new())
    }
    fn contract_stamp(&self) -> u64 {
        0
    }
    fn reachability_surface(&self) -> Result<ReachabilitySurface> {
        if self.member == "unreadable" {
            anyhow::bail!("store read failed");
        }
        Ok(FIXTURES.with(|f| f.borrow().get(&self.member).cloned().unwrap_or_default()))
    }
}

fn registry(names: &[&str]) -> EngineRegistry<FakeEngine> {
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
        links: Vec::new(),
        governance: Default::default(),
    };
    EngineRegistry::new(federation, RegistryMode::Lazy)
}

fn sym(symbol: &str) -> LogosSymbol {
    LogosSymbol::parse(symbol).expect("a parseable test symbol")
}

/// A callable surface node with an explicit tri-state per-repo verdict.
fn callable(name: &str, symbol: &str, is_dead: Option<bool>) -> ReachNode {
    ReachNode {
        kind: NodeKind::Function,
        name: name.to_string(),
        symbol: sym(symbol),
        is_dead,
    }
}

/// A cross-service edge whose provider (`to`) endpoint is `(member, symbol)` —
/// the only part of the edge the union view roots on.
fn edge_to(member: &str, symbol: &str) -> BridgeEdge {
    BridgeEdge {
        relation: "broker-topic".to_string(),
        from: BridgeEndpoint {
            member: "web".to_string(),
            symbol: sym("local publish_order"),
        },
        to: BridgeEndpoint {
            member: member.to_string(),
            symbol: sym(symbol),
        },
    }
}

/// The `orders` member: a broker-subscribe handler `on_order` that is **dead in
/// its own repo** (nothing intra-repo calls it — the framework dispatches it) and
/// calls a private helper `render`, which is dead for the same reason. Plus
/// `orphan`, dead and reachable from nothing at all.
fn orders_surface() -> ReachabilitySurface {
    ReachabilitySurface {
        nodes: vec![
            callable("on_order", "local on_order", Some(true)),
            callable("render", "local render", Some(true)),
            callable("orphan", "local orphan", Some(true)),
        ],
        edges: vec![(0, 1)], // on_order --Calls--> render
    }
}

fn names(claims: &[ReachabilityClaim]) -> Vec<&str> {
    claims.iter().map(|c| c.name.as_str()).collect()
}

/// FR-WS-12 acceptance: a handler reachable **only** via a matched cross-service
/// call is live in the union view — and the promotion walks transitively into the
/// callees the handler alone kept alive.
#[test]
fn a_handler_reachable_only_via_a_cross_service_call_is_live() {
    reset();
    set_surface("orders", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local on_order")],
    );

    assert_eq!(
        names(&view.live_via_cross_service),
        ["on_order", "render"],
        "the bound handler AND its transitive callees are live app-wide"
    );
    assert_eq!(
        names(&view.dead),
        ["orphan"],
        "a callable no cross-service root reaches stays dead"
    );
}

/// FR-WS-12 acceptance: a handler with **no** matched inbound edge is never
/// marked dead *on that basis* — with no bridge edges at all, the app-wide dead
/// set is exactly the per-repo dead set. The union view never manufactures
/// deadness; it can only fail to lift it.
#[test]
fn no_matched_edge_never_demotes_anything() {
    reset();
    set_surface("orders", orders_surface());

    let view = app_wide_reachability(&registry(&["orders"]), &[]);

    assert!(
        view.live_via_cross_service.is_empty(),
        "no bridge edges → no promotions"
    );
    assert_eq!(
        names(&view.dead),
        ["on_order", "orphan", "render"],
        "the app-wide dead set is exactly the per-repo dead set — nothing added"
    );
}

/// NFR-CC-04: the tri-state is preserved. A node whose per-repo verdict is `NULL`
/// ("not computed" — its language does not declare the reachability capability)
/// earns **no claim in either direction**, even when a cross-service root reaches
/// it. Likewise a node that is already live is never re-verdicted.
#[test]
fn a_null_verdict_is_preserved_and_a_live_node_is_never_demoted() {
    reset();
    set_surface(
        "orders",
        ReachabilitySurface {
            nodes: vec![
                callable("not_computed", "local not_computed", None),
                callable("already_live", "local already_live", Some(false)),
                callable("dead", "local dead", Some(true)),
            ],
            // The cross-service root lands on `not_computed`, which calls both
            // others — so every node here is *reached*, and only the dead one may
            // acquire a verdict.
            edges: vec![(0, 1), (0, 2)],
        },
    );

    let view = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local not_computed")],
    );

    let claimed: Vec<&str> = names(&view.live_via_cross_service)
        .into_iter()
        .chain(names(&view.dead))
        .collect();
    assert_eq!(
        claimed,
        ["dead"],
        "only the node its own repo verdicted dead is claimable: NULL stays NULL, \
         live stays live"
    );
    assert_eq!(view.members[0].dead_per_repo, 1);
}

/// ADR-56: the composition is monotone toward live. Whatever the bridge supplies,
/// the app-wide dead set is a **subset** of the per-repo dead set, and the
/// per-member arithmetic proves it: `dead_per_repo == promoted + dead_app_wide`.
#[test]
fn the_app_wide_dead_set_is_a_subset_of_the_per_repo_dead_set() {
    reset();
    set_surface("orders", orders_surface());

    let with_edges = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local on_order")],
    );
    let without = app_wide_reachability(&registry(&["orders"]), &[]);

    let dead_with: HashSet<&str> = names(&with_edges.dead).into_iter().collect();
    let dead_without: HashSet<&str> = names(&without.dead).into_iter().collect();
    assert!(
        dead_with.is_subset(&dead_without),
        "adding a cross-service edge can only shrink the dead set: \
         {dead_with:?} ⊄ {dead_without:?}"
    );

    let tally = &with_edges.members[0];
    assert_eq!(
        tally.dead_per_repo,
        tally.live_via_cross_service + tally.dead_app_wide,
        "every per-repo dead callable is either promoted or still dead — never lost"
    );
    assert_eq!((tally.live_via_cross_service, tally.dead_app_wide), (2, 1));
}

/// FR-WS-12: the view is explicitly labeled and advisory, and **every** claim
/// carries the coverage rider — it cannot be read apart from the coverage it
/// rests on.
#[test]
fn the_view_is_labeled_advisory_and_every_claim_carries_the_rider() {
    reset();
    set_surface("orders", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local on_order")],
    );

    assert_eq!(view.view, UNION_VIEW);
    assert!(view.advisory, "the union view is never a gate input (ADR-56)");
    assert_eq!(view.coverage.members_read, 1);
    assert_eq!(view.coverage.members_total, 1);

    let claims = view
        .live_via_cross_service
        .iter()
        .chain(view.dead.iter())
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 3, "every dead-per-repo callable is claimed");
    for claim in claims {
        assert_eq!(
            claim.coverage, view.coverage,
            "claim {} must carry the view's coverage rider",
            claim.name
        );
    }
}

/// ADR-53: a member that fails to start (`broken`) or fails its read
/// (`unreadable`) is skipped — it contributes no nodes and no roots, so it can
/// only suppress a promotion, never fabricate a dead verdict. The rider reports
/// the shortfall rather than hiding it.
#[test]
fn a_degraded_member_is_skipped_and_the_rider_says_so() {
    reset();
    set_surface("orders", orders_surface());
    // Fixtures for the degraded members exist but are never readable.
    set_surface("broken", orders_surface());
    set_surface("unreadable", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders", "broken", "unreadable"]),
        &[edge_to("orders", "local on_order")],
    );

    assert_eq!(view.coverage.members_read, 1);
    assert_eq!(view.coverage.members_total, 3);
    assert_eq!(
        view.members.iter().map(|m| m.member.as_str()).collect::<Vec<_>>(),
        ["orders"],
        "a degraded member contributes no tally — and so no claims"
    );
    assert!(
        view.dead.iter().all(|c| c.member == "orders"),
        "a member that could not be read is never verdicted dead"
    );
}

/// NFR-RA-05: a bridge root naming a symbol the member's surface does not carry
/// (a stale endpoint) is **reported**, not silently dropped — and it promotes
/// nothing, rather than approximately matching something.
#[test]
fn an_unresolved_root_is_counted_not_silently_dropped() {
    reset();
    set_surface("orders", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local ghost_handler")],
    );

    let tally = &view.members[0];
    assert_eq!(tally.extra_roots, 0);
    assert_eq!(tally.unresolved_roots, 1);
    assert!(
        view.live_via_cross_service.is_empty(),
        "an unresolved root promotes nothing — never an approximate match"
    );
}

/// Determinism ([NFR-RA-06]): claims are sorted by `(member, symbol)` regardless
/// of member fan-out order or surface order.
#[test]
fn claims_are_deterministically_ordered() {
    reset();
    set_surface(
        "zeta",
        ReachabilitySurface {
            nodes: vec![callable("z", "local z", Some(true))],
            edges: Vec::new(),
        },
    );
    set_surface(
        "alpha",
        ReachabilitySurface {
            nodes: vec![
                callable("b", "local b", Some(true)),
                callable("a", "local a", Some(true)),
            ],
            edges: Vec::new(),
        },
    );

    let view = app_wide_reachability(&registry(&["zeta", "alpha"]), &[]);

    assert_eq!(
        view.dead
            .iter()
            .map(|c| (c.member.as_str(), c.symbol.as_str()))
            .collect::<Vec<_>>(),
        [
            ("alpha", "local a"),
            ("alpha", "local b"),
            ("zeta", "local z"),
        ]
    );
    assert_eq!(
        view.members.iter().map(|m| m.member.as_str()).collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
}

// ── surface_from: the store read → surface projection ───────────────────────

fn nrow(id: i64, kind: NodeKind, name: &str, symbol: &str) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: sym(symbol),
        kind,
        name: name.to_string(),
        file_path: None,
        start_line: None,
        end_line: None,
    }
}

fn arow(id: i64, is_dead: Option<bool>) -> AnnotationNodeRow {
    AnnotationNodeRow {
        id: NodeId(id),
        kind: NodeKind::Function,
        name: String::new(),
        exported: false,
        derived: false,
        fingerprint: None,
        test_evidence: false,
        file_id: None,
        file_path: None,
        is_dead,
        is_duplicate: None,
        is_test: false,
        layer_membership: None,
        clone_group: None,
    }
}

fn erow(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind,
    }
}

/// The projection keeps **only** `Calls`/`RoutesTo` (the same adjacency the
/// per-repo live-set walk uses, [FR-AN-01]) and joins the tri-state `is_dead`
/// verdict on by node id.
#[test]
fn surface_from_keeps_only_the_live_set_adjacency_and_the_tri_state_verdict() {
    let nodes = [
        nrow(1, NodeKind::Route, "GET /x", "local route_x"),
        nrow(2, NodeKind::Function, "handler", "local handler"),
        nrow(3, NodeKind::Function, "helper", "local helper"),
    ];
    let annotations = [arow(2, Some(true)), arow(3, None)];
    // Every kind that is NOT `Calls`/`RoutesTo` must be excluded. This list is the
    // load-bearing pin: the module's "no re-walk needed" argument holds only if this
    // adjacency is *exactly* the one `annotate::live_set` walks. If `annotate` ever
    // admits another kind (say `Implements`, for trait dispatch) and `reach` does
    // not, the two silently diverge and the monotonicity premise breaks — so this
    // enumerates the plausible-inclusion kinds rather than one token exclusion.
    let edges = [
        erow(1, 2, EdgeKind::RoutesTo),
        erow(2, 3, EdgeKind::Calls),
        erow(3, 1, EdgeKind::Imports),
        erow(3, 1, EdgeKind::References),
        erow(3, 1, EdgeKind::Contains),
        erow(3, 1, EdgeKind::Implements),
        erow(3, 1, EdgeKind::Extends),
        erow(3, 1, EdgeKind::Instantiates),
        erow(3, 1, EdgeKind::TypeUses),
        erow(3, 1, EdgeKind::Accesses),
        erow(3, 1, EdgeKind::DocReference),
        erow(3, 1, EdgeKind::ForbiddenDependency),
        erow(3, 1, EdgeKind::ArtifactBinding),
    ];

    let surface = super::surface_from(&nodes, &annotations, &edges);

    assert_eq!(
        surface.nodes.iter().map(|n| n.is_dead).collect::<Vec<_>>(),
        [None, Some(true), None],
        "node 1 has no annotation row (NULL), node 2 is dead, node 3 is NULL"
    );
    assert_eq!(
        surface.edges,
        [(0, 1), (1, 2)],
        "ONLY RoutesTo + Calls survive — the same two kinds annotate::live_set walks"
    );
}

/// An edge whose endpoint is not among the read nodes is dropped, never
/// fabricated into a phantom vertex ([NFR-RA-05]).
#[test]
fn surface_from_drops_an_edge_with_an_unknown_endpoint() {
    let nodes = [nrow(1, NodeKind::Function, "f", "local f")];
    let edges = [erow(1, 99, EdgeKind::Calls)];

    let surface = super::surface_from(&nodes, &[], &edges);

    assert_eq!(surface.nodes.len(), 1);
    assert!(surface.edges.is_empty(), "the dangling edge is dropped");
    assert_eq!(
        surface.nodes[0].is_dead, None,
        "no annotation row → NULL, never a fabricated `false`"
    );
}

/// NFR-CC-04: `extra_roots` counts **distinct provider endpoints**, not inbound
/// edges. Two consumers calling one handler is one extra live root — reporting
/// two would make the tally mean something other than what its name promises.
#[test]
fn two_consumers_of_one_endpoint_are_one_root() {
    reset();
    set_surface("orders", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders"]),
        // The same provider endpoint, reached by two different consumer call sites.
        &[
            edge_to("orders", "local on_order"),
            edge_to("orders", "local on_order"),
        ],
    );

    assert_eq!(
        view.members[0].extra_roots, 1,
        "one provider endpoint = one root, however many consumers call it"
    );
    assert_eq!(names(&view.live_via_cross_service), ["on_order", "render"]);
}

/// The same discipline for the unresolved tally: one stale endpoint referenced by
/// three consumers is **one** unresolved root, not three.
#[test]
fn repeated_unresolved_endpoints_are_counted_once() {
    reset();
    set_surface("orders", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders"]),
        &[
            edge_to("orders", "local ghost"),
            edge_to("orders", "local ghost"),
            edge_to("orders", "local ghost"),
        ],
    );

    assert_eq!(view.members[0].unresolved_roots, 1);
    assert_eq!(view.members[0].extra_roots, 0);
}

/// AR-05 false-live bias: one symbol may back more than one node. A root on that
/// symbol must root **all** of them — resolving the ambiguity toward live, never
/// toward a fabricated dead verdict. A regression to a last-write-wins
/// `HashMap<&LogosSymbol, u32>` would be invisible without this.
#[test]
fn a_symbol_backing_several_nodes_roots_all_of_them() {
    reset();
    set_surface(
        "orders",
        ReachabilitySurface {
            nodes: vec![
                callable("handler", "local handler", Some(true)),
                callable("handler", "local handler", Some(true)), // same symbol
                callable("helper", "local helper", Some(true)),
            ],
            edges: vec![(1, 2)], // only the SECOND `handler` node calls the helper
        },
    );

    let view = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local handler")],
    );

    assert_eq!(
        names(&view.live_via_cross_service),
        ["handler", "handler", "helper"],
        "both nodes behind the symbol are rooted, so the helper is reached too"
    );
    assert!(view.dead.is_empty());
    assert_eq!(
        view.members[0].extra_roots, 1,
        "one distinct provider endpoint, even though it backs two nodes"
    );
}

/// The BFS terminates on a cyclic call graph, and on the `RoutesTo` **self-edge**
/// the dispatch pass plants as a live-root marker — a shape that genuinely occurs
/// in production graphs, and which `surface_from` preserves verbatim.
#[test]
fn the_walk_terminates_on_cycles_and_self_loops() {
    reset();
    set_surface(
        "orders",
        ReachabilitySurface {
            nodes: vec![
                callable("a", "local a", Some(true)),
                callable("b", "local b", Some(true)),
                callable("selfie", "local selfie", Some(true)),
            ],
            // a ⇄ b is a cycle; `selfie` carries a dispatch-marker self-loop.
            edges: vec![(0, 1), (1, 0), (2, 2)],
        },
    );

    let view = app_wide_reachability(&registry(&["orders"]), &[edge_to("orders", "local a")]);
    assert_eq!(
        names(&view.live_via_cross_service),
        ["a", "b"],
        "the cycle is walked once and terminates"
    );
    assert_eq!(names(&view.dead), ["selfie"]);

    let via_loop = app_wide_reachability(
        &registry(&["orders"]),
        &[edge_to("orders", "local selfie")],
    );
    assert_eq!(
        names(&via_loop.live_via_cross_service),
        ["selfie"],
        "a self-loop root terminates and promotes exactly itself"
    );
}

/// ADR-53 / NFR-RA-05: a bridge edge naming a member that degraded, or one that
/// is not in the workspace at all, is dropped without panicking and without
/// touching any other member's verdicts. The skipped member is **named**, not just
/// counted, so the shortfall is diagnosable.
#[test]
fn edges_into_unknown_or_degraded_members_are_dropped_safely() {
    reset();
    set_surface("orders", orders_surface());
    set_surface("broken", orders_surface());

    let view = app_wide_reachability(
        &registry(&["orders", "broken"]),
        &[
            edge_to("orders", "local on_order"),
            edge_to("broken", "local on_order"), // member failed to start
            edge_to("ghost", "local whatever"),  // not a workspace member at all
        ],
    );

    assert_eq!(view.skipped_members, ["broken"], "the degraded member is named");
    assert_eq!(
        names(&view.live_via_cross_service),
        ["on_order", "render"],
        "the healthy member's promotions are unaffected by the dropped edges"
    );
    assert!(
        view.dead.iter().all(|c| c.member == "orders"),
        "no member that could not be read is verdicted dead"
    );
}

/// A workspace with no members yields an honest empty view — no NaN in the
/// bound-ratio, no panic.
#[test]
fn an_empty_workspace_yields_an_honest_empty_view() {
    reset();
    let view = app_wide_reachability(&registry(&[]), &[]);

    assert!(view.members.is_empty());
    assert!(view.dead.is_empty());
    assert!(view.live_via_cross_service.is_empty());
    assert_eq!(view.coverage.members_read, 0);
    assert_eq!(view.coverage.members_total, 0);
    assert_eq!(
        view.coverage.bound_ratio, 1.0,
        "nothing to bind is full coverage, honestly — never NaN"
    );
}

/// The verdict's JSON wire spelling is part of the contract every surface reads.
/// Asserted directly, because the promotion bucket is empty on the real path
/// today, so no E2E would catch a rename regression on it.
#[test]
fn the_verdict_wire_spellings_are_stable() {
    assert_eq!(
        serde_json::to_value(AppWideVerdict::LiveViaCrossService).unwrap(),
        "live-via-cross-service"
    );
    assert_eq!(serde_json::to_value(AppWideVerdict::Dead).unwrap(), "dead");
}

/// **The tripwire for [FR-WS-12] AC1's real-path proof.**
///
/// Every promotion test above drives a *fake* engine over a surface the test
/// types by hand. That is deliberate — but it means nothing in the suite proves
/// a promotion can happen on a real index, and the reason it cannot is not
/// stated anywhere the compiler or CI can see it.
///
/// A node is promotable only when it is (a) rooted by a bridge edge and (b)
/// verdicted `is_dead = Some(true)` per-repo. Since S-256 the broker arm roots
/// real subscriber methods, so (a) is satisfied — but a broker subscribe is only
/// *captured* by a language shipping a `brokers.scm` query, and `is_dead` is only
/// *computed* for a language declaring the `reachability` capability ([CR-043]).
/// Those two sets are disjoint today (`java` brokers, `rust` reachability), so
/// the promotion set is empty on every real workspace — for a reason that has
/// nothing to do with this module being wrong.
///
/// This test pins that disjointness against the **live plugin manifests**, not a
/// prose claim. The day a language declares both, it fails — and that is exactly
/// the day someone must write the real-path promotion E2E ([FR-WS-12] AC1) that
/// the fakes above only simulate.
#[test]
fn the_broker_promotion_path_is_still_blocked_by_the_capability_matrix() {
    let registry = crate::plugin::LanguageRegistry::load(std::env::temp_dir())
        .expect("embedded grammars load");

    let both: Vec<&str> = registry
        .iter()
        .filter(|p| {
            p.capabilities().iter().any(|c| c == "brokers") && p.supports_reachability()
        })
        .map(|p| p.name())
        .collect();

    assert!(
        both.is_empty(),
        "{both:?} now declares BOTH the `brokers` capability and `reachability`, so a \
         subscriber handler can finally be dead per-repo AND rooted by a cross-service \
         publish — the union view's promotion path is reachable on a real index for the \
         first time. Write the real-path promotion E2E that FR-WS-12 AC1 wants (index a \
         2-repo workspace, publish in one member, an uncalled package-private subscriber \
         in the other, assert it lands in `live_via_cross_service`), then delete this test. \
         NB: an `exported` listener is already a per-repo live root, so the fixture's \
         subscriber must not be exported under its language's export convention."
    );
}
