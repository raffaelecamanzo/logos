//! Direct unit tests over the shared [`reconcile`](super::reconcile) primitive
//! (S-292, [CR-082]): stale-delete, id-stable survivor reuse, owned-edge
//! reconciliation (with foreign edges left untouched), and the deterministic
//! symbol-sorted insert order — against a real in-memory writer batch, with a
//! synthetic edge vocabulary that exercises both keying strategies (a
//! pre-resolved [`NodeId`] target like the framework pass, and a symbol-keyed
//! target resolved inside the batch like the topics pass).
//!
//! [CR-082]: ../../../../docs/requests/CR-082-shared-promotion-primitive.md

use super::*;

use tempfile::TempDir;

use crate::graph_store::{EdgeRow, NewNode, NodeRow};
use crate::model::{EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::runtime::Runtime;

/// A test edge vocabulary spanning both real passes' keying strategies.
enum TestEdge {
    /// `parent --Contains--> self`, target already a resolved id (framework-style).
    Under(NodeId),
    /// `self --RoutesTo--> node named by symbol`, resolved inside the batch and
    /// **dropped** if the target is not in the desired set (topics-style).
    RouteTo(String),
}

/// The edge kinds this synthetic pass owns; every other kind is foreign and
/// must survive reconciliation untouched.
fn owned(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Contains | EdgeKind::RoutesTo)
}

/// Map one [`TestEdge`] to a concrete promoted edge — the seam both real passes
/// parameterise `reconcile` over.
fn resolve(self_id: NodeId, edge: &TestEdge, ids: &std::collections::HashMap<String, NodeId>) -> Option<PromotedEdge> {
    match edge {
        TestEdge::Under(parent) => Some(PromotedEdge::new(*parent, self_id, EdgeKind::Contains)),
        TestEdge::RouteTo(target) => ids
            .get(target.as_str())
            .map(|&t| PromotedEdge::new(self_id, t, EdgeKind::RoutesTo)),
    }
}

fn sym(name: &str) -> LogosSymbol {
    LogosSymbol::parse(&format!("local {name}")).expect("the fixture symbol parses")
}

fn runtime() -> (TempDir, Runtime) {
    let tmp = TempDir::new().expect("tempdir");
    let rt = Runtime::open(tmp.path().join("graph.db")).expect("runtime opens");
    (tmp, rt)
}

/// Seed a node directly through the writer and return its assigned id.
fn seed_node(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
    let symbol = sym(name);
    let name = name.to_string();
    rt.submit_write(move |w| {
        let symbol_id = w.upsert_symbol(&symbol)?;
        w.insert_node(&NewNode::plain(symbol_id, kind, &name))
    })
    .expect("seed node")
}

/// Seed an indexed file and return its id (a promoted node's `file_id` is a
/// foreign key into `files`).
fn seed_file(rt: &Runtime, path: &str) -> i64 {
    let path = path.to_string();
    rt.submit_write(move |w| w.insert_file(&path, None, None))
        .expect("seed file")
}

fn seed_edge(rt: &Runtime, source: NodeId, target: NodeId, kind: EdgeKind) {
    rt.submit_write(move |w| {
        w.insert_edge_if_absent(source, target, kind)?;
        Ok(())
    })
    .expect("seed edge");
}

fn all_nodes(rt: &Runtime) -> Vec<NodeRow> {
    rt.submit_read(|store| store.all_nodes()).expect("read nodes")
}

fn all_edges(rt: &Runtime) -> Vec<EdgeRow> {
    rt.submit_read(|store| store.all_edges()).expect("read edges")
}

fn id_of(nodes: &[NodeRow], name: &str) -> Option<NodeId> {
    nodes.iter().find(|n| n.name == name).map(|n| n.id)
}

fn has_edge(edges: &[EdgeRow], source: NodeId, target: NodeId, kind: EdgeKind) -> bool {
    edges
        .iter()
        .any(|e| e.source == source && e.target == target && e.kind == kind)
}

fn promoted<'a>(nodes: &'a [NodeRow], names: &[&str]) -> Vec<&'a NodeRow> {
    nodes
        .iter()
        .filter(|n| names.contains(&n.name.as_str()))
        .collect()
}

/// Stale-delete + id-stable reuse + owned-edge reconciliation: a survivor keeps
/// its id, a node absent from the desired set is retired, a stale *owned* edge
/// on the survivor is deleted, a desired owned edge is (re)proved, and a foreign
/// edge incident to the survivor is left untouched.
#[test]
fn reconcile_retires_stale_reuses_survivors_and_reconciles_only_owned_edges() {
    let (_tmp, rt) = runtime();
    let file = seed_file(&rt, "src/fixture.rs");

    // A pre-existing, non-promoted anchor + handler (ordinary code nodes).
    let anchor = seed_node(&rt, "anchor", NodeKind::Module);
    let handler = seed_node(&rt, "handler", NodeKind::Function);
    // Two already-promoted nodes: one survives this run, one becomes stale.
    let keep = seed_node(&rt, "keep", NodeKind::Route);
    let drop = seed_node(&rt, "drop", NodeKind::Route);

    // The survivor carries: an owned Contains from the anchor (stays desired),
    // an owned RoutesTo to the handler (NOT desired this run → deleted), and a
    // foreign Calls to the handler (never owned → untouched).
    seed_edge(&rt, anchor, keep, EdgeKind::Contains);
    seed_edge(&rt, keep, handler, EdgeKind::RoutesTo);
    seed_edge(&rt, keep, handler, EdgeKind::Calls);
    // The stale node's owned edge cascades away with the node delete.
    seed_edge(&rt, anchor, drop, EdgeKind::Contains);

    let nodes = all_nodes(&rt);
    let existing = promoted(&nodes, &["keep", "drop"]);
    let edges = all_edges(&rt);

    let desired = vec![
        Promoted {
            symbol: sym("keep"),
            kind: NodeKind::Route,
            name: "keep".to_string(),
            file_id: Some(file),
            start_line: None,
            end_line: None,
            edges: vec![TestEdge::Under(anchor)],
        },
        Promoted {
            symbol: sym("added"),
            kind: NodeKind::Route,
            name: "added".to_string(),
            file_id: Some(file),
            start_line: None,
            end_line: None,
            edges: vec![TestEdge::Under(anchor)],
        },
    ];

    reconcile(&rt, &existing, &edges, desired, owned, resolve).expect("reconcile");

    let nodes = all_nodes(&rt);
    let edges = all_edges(&rt);

    // Stale-delete: `drop` is retired (its incident edges cascade with it in the
    // store — asserting on the edge here would be confounded by rowid reuse).
    assert!(id_of(&nodes, "drop").is_none(), "the stale node is retired");

    // Id-stable reuse: the survivor keeps its original id (not delete+reinsert).
    assert_eq!(
        id_of(&nodes, "keep"),
        Some(keep),
        "the survivor's id is stable across the reconcile"
    );

    // The new node was inserted.
    let added = id_of(&nodes, "added").expect("the newly desired node is inserted");

    // Owned-edge reconciliation.
    assert!(
        has_edge(&edges, anchor, keep, EdgeKind::Contains),
        "the still-desired owned edge on the survivor is kept"
    );
    assert!(
        has_edge(&edges, anchor, added, EdgeKind::Contains),
        "the desired owned edge on the new node is proved"
    );
    assert!(
        !has_edge(&edges, keep, handler, EdgeKind::RoutesTo),
        "a stale OWNED edge on the survivor is deleted"
    );
    assert!(
        has_edge(&edges, keep, handler, EdgeKind::Calls),
        "a FOREIGN edge incident to the survivor is left untouched"
    );
}

/// Sorted insert + in-batch symbol resolution + drop-on-unresolved: brand-new
/// nodes are created in ascending symbol order (so their ids are monotone in
/// that order), a symbol-keyed edge to a sibling created in the same batch
/// resolves, and a symbol-keyed edge to an absent target is dropped rather than
/// pointed at a fabricated node.
#[test]
fn reconcile_inserts_in_symbol_order_and_drops_unresolved_targets() {
    let (_tmp, rt) = runtime();
    let file = seed_file(&rt, "src/fixture.rs");
    let anchor = seed_node(&rt, "anchor", NodeKind::Module);

    // Deliberately out of symbol order in the input vector.
    let desired = vec![
        Promoted {
            symbol: sym("zzz"),
            kind: NodeKind::Route,
            name: "zzz".to_string(),
            file_id: Some(file),
            start_line: None,
            end_line: None,
            edges: vec![TestEdge::Under(anchor)],
        },
        Promoted {
            symbol: sym("aaa"),
            kind: NodeKind::Route,
            name: "aaa".to_string(),
            file_id: Some(file),
            start_line: None,
            end_line: None,
            // Resolves to `mmm`, created in this same batch; and `ghost`, which
            // is not in the desired set → dropped.
            edges: vec![
                TestEdge::RouteTo("local mmm".to_string()),
                TestEdge::RouteTo("local ghost".to_string()),
            ],
        },
        Promoted {
            symbol: sym("mmm"),
            kind: NodeKind::Route,
            name: "mmm".to_string(),
            file_id: Some(file),
            start_line: None,
            end_line: None,
            edges: vec![TestEdge::Under(anchor)],
        },
    ];

    reconcile(&rt, &[], &all_edges(&rt), desired, owned, resolve).expect("reconcile");

    let nodes = all_nodes(&rt);
    let aaa = id_of(&nodes, "aaa").expect("aaa inserted");
    let mmm = id_of(&nodes, "mmm").expect("mmm inserted");
    let zzz = id_of(&nodes, "zzz").expect("zzz inserted");

    // Sorted insert: ids are monotone in symbol order (aaa < mmm < zzz).
    assert!(
        aaa < mmm && mmm < zzz,
        "new nodes are inserted in ascending symbol order: aaa={aaa:?} mmm={mmm:?} zzz={zzz:?}"
    );

    let edges = all_edges(&rt);
    // In-batch symbol resolution: aaa --RoutesTo--> mmm proved.
    assert!(
        has_edge(&edges, aaa, mmm, EdgeKind::RoutesTo),
        "a symbol-keyed edge to a sibling created in the same batch resolves"
    );
    // Drop-on-unresolved: exactly one RoutesTo edge — the `ghost` target was
    // dropped, never fabricated.
    assert_eq!(
        edges
            .iter()
            .filter(|e| e.kind == EdgeKind::RoutesTo)
            .count(),
        1,
        "the edge to an unresolved target is dropped, not fabricated"
    );
}

/// The fileless-identity-node path: a repo-scoped node with `file_id: None`
/// (a broker `Topic` — the reason `Promoted::file_id` is `Option` rather than a
/// bare id) inserts and persists with no anchoring file, and a symbol-keyed edge
/// to it resolves inside the same batch (the topics-pass keying strategy).
#[test]
fn reconcile_inserts_a_fileless_identity_node_and_links_to_it_by_symbol() {
    let (_tmp, rt) = runtime();
    let file = seed_file(&rt, "src/fixture.rs");

    let desired = vec![
        // The fileless repo-scoped identity node (topics-style `Topic`).
        Promoted {
            symbol: sym("topic_orders"),
            kind: NodeKind::Topic,
            name: "topic_orders".to_string(),
            file_id: None,
            start_line: None,
            end_line: None,
            edges: vec![],
        },
        // A file-anchored producer that names the topic by symbol.
        Promoted {
            symbol: sym("producer_site"),
            kind: NodeKind::Producer,
            name: "producer_site".to_string(),
            file_id: Some(file),
            start_line: Some(3),
            end_line: Some(3),
            edges: vec![TestEdge::RouteTo("local topic_orders".to_string())],
        },
    ];

    reconcile(&rt, &[], &all_edges(&rt), desired, owned, resolve).expect("reconcile");

    let nodes = all_nodes(&rt);
    let topic = nodes
        .iter()
        .find(|n| n.name == "topic_orders")
        .expect("the fileless identity node is inserted");
    assert_eq!(topic.kind, NodeKind::Topic);
    assert!(
        topic.file_path.is_none(),
        "a repo-scoped identity node persists with no anchoring file (file_id None)"
    );

    let producer = id_of(&nodes, "producer_site").expect("the producer is inserted");
    assert!(
        has_edge(&all_edges(&rt), producer, topic.id, EdgeKind::RoutesTo),
        "a symbol-keyed edge to the fileless node resolves inside the batch"
    );
}

/// Idempotency: reconciling twice to the same desired set is a no-op on the
/// second run — identical node ids and identical edge set.
#[test]
fn reconcile_is_idempotent_across_repeated_runs() {
    let (_tmp, rt) = runtime();
    let file = seed_file(&rt, "src/fixture.rs");
    let anchor = seed_node(&rt, "anchor", NodeKind::Module);

    let build = |anchor: NodeId| {
        vec![
            Promoted {
                symbol: sym("route_a"),
                kind: NodeKind::Route,
                name: "route_a".to_string(),
                file_id: Some(file),
                start_line: None,
                end_line: None,
                edges: vec![TestEdge::Under(anchor)],
            },
            Promoted {
                symbol: sym("route_b"),
                kind: NodeKind::Route,
                name: "route_b".to_string(),
                file_id: Some(file),
                start_line: None,
                end_line: None,
                edges: vec![TestEdge::Under(anchor)],
            },
        ]
    };

    reconcile(&rt, &[], &all_edges(&rt), build(anchor), owned, resolve).expect("first run");
    let nodes_1 = all_nodes(&rt);
    let mut edges_1 = all_edges(&rt);

    // Second run against the now-promoted survivors.
    let existing = promoted(&nodes_1, &["route_a", "route_b"]);
    reconcile(&rt, &existing, &edges_1, build(anchor), owned, resolve).expect("second run");

    let nodes_2 = all_nodes(&rt);
    let mut edges_2 = all_edges(&rt);

    assert_eq!(
        id_of(&nodes_1, "route_a"),
        id_of(&nodes_2, "route_a"),
        "survivor ids are stable across an idempotent re-run"
    );
    assert_eq!(
        id_of(&nodes_1, "route_b"),
        id_of(&nodes_2, "route_b"),
        "survivor ids are stable across an idempotent re-run"
    );
    let key = |e: &EdgeRow| (e.source, e.target, e.kind.as_i32());
    edges_1.sort_by_key(key);
    edges_2.sort_by_key(key);
    assert_eq!(
        edges_1.len(),
        edges_2.len(),
        "no duplicate edges accumulate across a re-run"
    );
    for (a, b) in edges_1.iter().zip(&edges_2) {
        assert_eq!(key(a), key(b), "the edge set is byte-for-byte identical");
    }
}
