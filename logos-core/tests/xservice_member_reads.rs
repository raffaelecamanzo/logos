//! **The cross-service bridge reads only the members a query needs, and names
//! them** (S-484, [FR-WS-05], [NFR-PE-10], [NFR-CC-04]) — end to end, over
//! real member stores.
//!
//! The bridge's own unit tests count stamp reads at a fake engine; this file
//! asks the same question of the real `Engine` and the three read-models the
//! surfaces serve: on five members where the answer involves two, a warm
//! `callers`, `impact` and `route-providers` start no engine, and each names
//! exactly the two members it read. A member whose store will not open is
//! named with its reason, cold and warm.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    query, ContractBridge, EngineRegistry, Federation, Member, MemberReads, RegistryMode, WorkspaceBudget,
};
use logos_core::Engine;

#[path = "support/bridge_reads.rs"]
mod bridge_reads;

/// `web`'s static outbound call `GET /users/{id}` — the consumer end.
const CLIENT_STATIC: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}
"#;

/// `api`'s axum route `GET /users/{user_id}` — the provider end.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

/// The three members the answer has nothing to do with.
const UNRELATED: [&str; 3] = ["billing", "search", "audit"];

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member into its own store, then drop the engine so the registry
/// re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// Five indexed members — `api`, `web` and the three [`UNRELATED`] — under a
/// registry that holds two resident at once, as a large workspace's budget
/// holds a few of many.
fn five_members(root: &Path) -> EngineRegistry<Engine> {
    write(&root.join("api"), "src/main.rs", AXUM_MAIN);
    write(&root.join("web"), "src/client.rs", CLIENT_STATIC);
    for name in UNRELATED {
        write(&root.join(name), "src/lib.rs", &format!("pub fn {name}_only() {{}}\n"));
    }
    let names = ["api", "web"].into_iter().chain(UNRELATED);
    let members: Vec<Member> = names
        .map(|name| {
            let dir = root.join(name);
            index_member(&dir);
            Member { name: name.to_string(), root: dir }
        })
        .collect();
    let federation = Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
    };
    let tight = WorkspaceBudget::from_limits(64, 1);
    assert_eq!(tight.max_resident_members(), 2, "the fixture's residency ceiling");
    EngineRegistry::with_budget(federation, RegistryMode::Lazy, tight)
}

/// Overwrite a member's store with bytes that are not a database, leaving a
/// regular file at the store path.
fn corrupt_store(member: &Path) {
    let db = member.join(".logos/logos.db");
    for sidecar in ["logos.db-wal", "logos.db-shm"] {
        let _ = fs::remove_file(member.join(".logos").join(sidecar));
    }
    fs::write(&db, b"this is not a sqlite database, whatever its name says").expect("overwrite the store");
}

/// Replace a member's store with a directory — nothing can open it.
fn obstruct_store(member: &Path) {
    let db = member.join(".logos/logos.db");
    fs::remove_file(&db).expect("clear the store file");
    fs::create_dir_all(&db).expect("a directory where the store must be");
}

fn names(members: &[&str]) -> BTreeSet<String> {
    members.iter().map(|m| (*m).to_string()).collect()
}

/// **Each of the three queries, warm, starts no engine and names exactly the
/// two members its answer involves** ([NFR-PE-10]). The two are the workspace's
/// working set — what a chat that asked about them through the member-addressed
/// tools leaves resident — and the other three were opened once, by the cold
/// read, and evicted. The old stamp check started all three again on every
/// query, whatever `repo` said.
///
/// [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
#[test]
fn each_warm_bridge_query_reads_only_the_two_members_its_answer_involves() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = five_members(tmp.path());
    let bridge = ContractBridge::new();

    let cold = query::reachability_inputs(&bridge, &registry);
    assert_eq!(cold.edges.len(), 1, "guard the guard: web's call binds api's route: {:?}", cold.edges);
    assert_eq!(
        cold.reads.read,
        names(&["api", "audit", "billing", "search", "web"]),
        "cold, every member is read — an edge binds the SOLE provider of its key"
    );
    let route = cold.edges[0].to.symbol.as_str().to_string();

    let working_set = || {
        registry.evict_to_capacity(0);
        registry.engine_for("api").expect("api opens");
        registry.engine_for("web").expect("web opens");
        registry.engine_starts()
    };
    let two = names(&["api", "web"]);

    let starts = working_set();
    let callers = query::xservice_callers(&registry, &query::reachability_inputs(&bridge, &registry), &route, None, Some("api"));
    assert_eq!(callers.cross_service.len(), 1, "web is the cross-service caller");
    assert_eq!(callers.member_reads, MemberReads { read: two.clone(), unread: Default::default() });
    assert_eq!(registry.engine_starts(), starts, "callers started no engine");

    let starts = working_set();
    let impact = query::xservice_impact(&registry, &query::reachability_inputs(&bridge, &registry), &route, None, Some("api"));
    assert_eq!(impact.cross_service.len(), 1, "web is reached across the edge");
    assert_eq!(impact.member_reads, MemberReads { read: two.clone(), unread: Default::default() });
    assert_eq!(registry.engine_starts(), starts, "impact started no engine");

    let starts = working_set();
    let providers = query::xservice_route_providers(&query::bridge_read(&bridge, &registry), Some("api"));
    assert_eq!(providers.providers.len(), 1);
    assert_eq!(providers.member_reads, MemberReads { read: two, unread: Default::default() });
    assert_eq!(registry.engine_starts(), starts, "route-providers started no engine");

    // And the answers are the cold ones, on this fixture too.
    bridge_reads::assert_narrowed_read_changes_no_answer(&registry);
}

/// **The new field is on the wire, always** — the three read-models serialize
/// `member_reads` with its `read` list, and `unread` only when a member was not
/// read, so a fully-read answer's shape is `{"read": [...]}`.
#[test]
fn every_bridge_answer_serializes_its_member_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = five_members(tmp.path());
    let bridge = ContractBridge::new();
    let inputs = query::reachability_inputs(&bridge, &registry);
    let route = inputs.edges[0].to.symbol.as_str().to_string();

    for answer in [
        serde_json::to_value(query::xservice_callers(&registry, &inputs, &route, None, None)).unwrap(),
        serde_json::to_value(query::xservice_impact(&registry, &inputs, &route, None, None)).unwrap(),
        // A cold bridge: this one is warm now, and its read would name only the
        // members resident after the two answers above.
        serde_json::to_value(query::xservice_route_providers(&query::bridge_read(&ContractBridge::new(), &registry), None))
            .unwrap(),
    ] {
        let reads = &answer["member_reads"];
        assert_eq!(
            reads["read"],
            serde_json::json!(["api", "audit", "billing", "search", "web"]),
            "{answer:#}"
        );
        assert!(reads.get("unread").is_none(), "absent when every member was read: {reads}");
    }
}

/// **A member whose store will not open is named with its reason, never
/// omitted** ([NFR-CC-04]) — on the cold read that computed the edge set and
/// on the warm one served from it, and in every read-model's `member_reads`.
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn a_member_whose_store_is_unreadable_is_named_with_its_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let registry = five_members(root);
    // `audit`'s store becomes a directory before the registry ever opens it.
    obstruct_store(&root.join("audit"));
    let bridge = ContractBridge::new();

    for pass in ["cold", "warm"] {
        let inputs = query::reachability_inputs(&bridge, &registry);
        assert_eq!(inputs.edges.len(), 1, "{pass}: the healthy members still answer");
        let route = inputs.edges[0].to.symbol.as_str().to_string();
        for (read_model, reads) in [
            ("callers", query::xservice_callers(&registry, &inputs, &route, None, Some("api")).member_reads),
            ("impact", query::xservice_impact(&registry, &inputs, &route, None, Some("api")).member_reads),
            ("route-providers", query::xservice_route_providers(&query::bridge_read(&bridge, &registry), None).member_reads),
        ] {
            let reason = reads.unread.get("audit").unwrap_or_else(|| panic!("{pass} {read_model}: audit is named: {reads:?}"));
            assert!(
                reason.contains("starting the engine for workspace member \"audit\""),
                "{pass} {read_model}: the reason is the engine's own diagnostic: {reason}"
            );
            assert!(!reads.read.contains("audit"), "{pass} {read_model}: {reads:?}");
            assert!(reads.read.contains("api"), "{pass} {read_model}: {reads:?}");
        }
    }

    // And on the wire: the reason rides the serialized payload every surface
    // prints, keyed by the member, beside `read` ([NFR-CC-04]).
    let inputs = query::reachability_inputs(&bridge, &registry);
    let route = inputs.edges[0].to.symbol.as_str().to_string();
    for answer in [
        serde_json::to_value(query::xservice_callers(&registry, &inputs, &route, None, Some("api"))).unwrap(),
        serde_json::to_value(query::xservice_impact(&registry, &inputs, &route, None, Some("api"))).unwrap(),
        serde_json::to_value(query::xservice_route_providers(&query::bridge_read(&bridge, &registry), None)).unwrap(),
    ] {
        let reason = answer["member_reads"]["unread"]["audit"].as_str().unwrap_or_default();
        assert!(reason.contains("starting the engine for workspace member \"audit\""), "{answer:#}");
        assert!(answer["member_reads"]["read"].as_array().is_some_and(|read| !read.contains(&"audit".into())));
    }
}

/// **Each tier names the member it opened itself** — the reads the bridge's
/// stamp check did not make. With only `web` resident, a scoped `callers`
/// opens `api` for its intra-repo fan-out; with only `api` resident, `impact`
/// opens `web` across the edge; and a far member that will not start any more
/// is skipped in `cross_service`, as it always was, and named with its reason.
#[test]
fn each_tier_names_the_member_it_opened_and_the_one_that_would_not_open() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let registry = five_members(root);
    let bridge = ContractBridge::new();
    let route = query::reachability_inputs(&bridge, &registry).edges[0].to.symbol.as_str().to_string();
    let only = |member: &str| {
        registry.evict_to_capacity(0);
        registry.engine_for(member).expect("opens");
        registry.engine_starts()
    };
    let two = names(&["api", "web"]);

    let starts = only("web");
    let callers = query::xservice_callers(&registry, &query::reachability_inputs(&bridge, &registry), &route, None, Some("api"));
    assert_eq!(registry.engine_starts(), starts + 1, "the intra-repo fan-out opened api");
    assert_eq!(callers.member_reads.read, two, "and the answer names it");

    let starts = only("api");
    let impact = query::xservice_impact(&registry, &query::reachability_inputs(&bridge, &registry), &route, None, Some("api"));
    assert_eq!(registry.engine_starts(), starts + 1, "the far side opened web");
    assert_eq!(impact.member_reads.read, two, "and the answer names it");

    // `web` was opened before and its store file is still there, so the stamp
    // check states its restart stamp without starting it. Corrupt contents are
    // what that cannot see; the far side is the first to find them.
    only("api");
    corrupt_store(&root.join("web"));
    let impact = query::xservice_impact(&registry, &query::reachability_inputs(&bridge, &registry), &route, None, Some("api"));
    assert!(impact.cross_service.is_empty(), "the far member is skipped, not fatal");
    let reason = impact.member_reads.unread.get("web").expect("…and named");
    assert!(reason.contains("starting the engine for workspace member \"web\""), "{reason}");
    assert_eq!(impact.member_reads.read, names(&["api"]));
}

/// **A member opened before whose store is gone since is not served from the
/// cache** (review finding A). Over five real stores: a warm bridge, every
/// engine evicted, then `web`'s store replaced by a directory. The stamp
/// check cannot state the restart of a member with no store, so it attempts
/// `web`, which fails: `route-providers` — which has no per-member tier that
/// would have opened it — no longer lists `web`'s binding, and names `web`
/// with its reason, as a fresh bridge does.
#[test]
fn an_evicted_member_whose_store_is_gone_is_not_served_from_the_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let registry = five_members(root);
    let bridge = ContractBridge::new();
    assert_eq!(query::bridge_read(&bridge, &registry).edges.len(), 1, "guard the guard: the warm set has web's edge");

    registry.evict_to_capacity(0);
    obstruct_store(&root.join("web"));
    let warm = query::xservice_route_providers(&query::bridge_read(&bridge, &registry), None);
    let fresh = query::xservice_route_providers(&query::bridge_read(&ContractBridge::new(), &registry), None);
    assert!(warm.providers.is_empty(), "web's binding is no longer served: {:?}", warm.providers);
    assert_eq!(warm.providers.len(), fresh.providers.len(), "the warm answer is the fresh one");
    let reason = warm.member_reads.unread.get("web").expect("web is named");
    assert!(reason.contains("starting the engine for workspace member \"web\""), "{reason}");
}

/// **A per-member row that carries an error is unread, never read** — the
/// intra-repo fan-out of `callers` and the seed of `impact` record their own
/// rows. A `repo` naming no member is the error row every surface can reach:
/// the answer names it with the registry's reason.
#[test]
fn a_per_member_row_that_failed_is_named_unread_not_read() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = five_members(tmp.path());
    let bridge = ContractBridge::new();
    let inputs = query::reachability_inputs(&bridge, &registry);
    let route = inputs.edges[0].to.symbol.as_str().to_string();

    for (read_model, rows, reads) in [
        {
            let answer = query::xservice_callers(&registry, &inputs, &route, None, Some("ghost"));
            ("callers", answer.members.len(), answer.member_reads)
        },
        {
            let answer = query::xservice_impact(&registry, &inputs, &route, None, Some("ghost"));
            ("impact", answer.seed.len(), answer.member_reads)
        },
    ] {
        assert_eq!(rows, 1, "{read_model}: guard the guard — the one error row");
        let reason = reads.unread.get("ghost").unwrap_or_else(|| panic!("{read_model}: ghost is named: {reads:?}"));
        assert!(reason.contains("no such workspace member"), "{read_model}: {reason}");
        assert!(!reads.read.contains("ghost"), "{read_model}: an error row is never read: {reads:?}");
    }
}

/// **A resident member that re-syncs invalidates the warm bridge** — over real
/// stores, which nothing else here exercises: the fixture helper compares
/// answers over an unchanged workspace, so it cannot tell a working cache key
/// from one that never misses. `api` is resident and re-indexes an edited
/// file (its stamp advances, as `serve`'s watcher makes it); the next read is
/// a miss, and a miss reads every member.
#[test]
fn a_resident_member_that_re_syncs_invalidates_the_warm_bridge() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let registry = five_members(root);
    let bridge = ContractBridge::new();
    let _ = query::bridge_read(&bridge, &registry);
    registry.evict_to_capacity(0);
    let api = registry.engine_for("api").expect("api opens");
    assert_eq!(query::bridge_read(&bridge, &registry).reads.read, names(&["api"]), "guard the guard: a warm hit");

    write(&root.join("api"), "src/main.rs", &format!("{AXUM_MAIN}\npub fn added() {{}}\n"));
    let _ = api.sync(&[root.join("api/src/main.rs")]);
    let after = query::bridge_read(&bridge, &registry);
    assert_eq!(
        after.reads.read,
        names(&["api", "audit", "billing", "search", "web"]),
        "the advance is a miss, and the recompute reads every member"
    );
    assert_eq!(after.edges.len(), 1, "web's call still binds api's route");
}
