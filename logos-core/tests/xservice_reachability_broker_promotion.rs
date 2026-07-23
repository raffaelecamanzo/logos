//! End-to-end proof of the app-wide cross-service reachability **promotion** path
//! (S-291, [CR-081], [FR-WS-12] AC1, [ADR-56]).
//!
//! This is the real-path E2E the former
//! `federation::reach::tests::the_broker_promotion_path_is_still_blocked_by_the_capability_matrix`
//! tripwire promised: the day a language declared both the `brokers` capability
//! and `reachability`, the union view's promotion path became reachable on a real
//! index and someone had to prove it. S-291 gives **Rust** both, so this test
//! drives the whole pipeline over two real Rust member repositories:
//!
//! - `api` publishes to `orders` (a `bus.publish("orders", …)` producer);
//! - `web` has a **package-private, uncalled** `on_order` handler that subscribes
//!   to `orders` — dead in its own repo (Rust's `visibility-modifier` export rule
//!   makes a non-`pub` fn a non-root, and nothing calls it), and reachable **only**
//!   across the cross-member broker edge.
//!
//! The union view must promote `on_order` to `live_via_cross_service` — the
//! promotion [FR-WS-12] AC1 exists for — while preserving every honesty invariant
//! ([ADR-56]): monotone toward live (a still-unreached dead callable stays dead),
//! no demotion, the per-repo gated signal byte-for-byte unchanged, and a coverage
//! rider on every claim.
//!
//! Gated on the Rust grammar — the language that carries both capabilities.
//!
//! [FR-WS-12]: ../../docs/specs/requirements/FR-WS-12.md
//! [ADR-56]: ../../docs/specs/architecture/decisions/ADR-56.md
//! [CR-081]: ../../docs/requests/CR-081-reachability-capability-matrix-gap.md
#![cfg(feature = "lang-rust")]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    app_wide_reachability, AppWideVerdict, BridgeIntake, ContractBridge, EngineRegistry, Federation,
    Member, RegistryMode, UNION_VIEW,
};
use logos_core::model::NodeKind;
use logos_core::Engine;

/// The `api` member: an exported producer that publishes to `orders`. `pub`, so it
/// is a per-repo live root — its own deadness is irrelevant here; it exists only
/// to emit the cross-service publish that roots `web`'s subscriber.
const PUBLISHER: &str = r#"
pub fn emit_order(bus: &Bus, payload: &str) {
    bus.publish("orders", payload);
}
"#;

/// The `web` member. `on_order` is package-private (no `pub`) and called by
/// nobody, so the annotation pass verdicts it `is_dead = true`; it is reachable
/// only via `api`'s publish on the `orders` topic. `orphan` is likewise dead but
/// no cross-service edge reaches it — the union view must leave it dead (the
/// "never promoted on no evidence" half of the honesty contract). `serve` is an
/// exported live root that touches nothing broker-related.
const SUBSCRIBER: &str = r#"
fn on_order(bus: &Bus) {
    bus.subscribe("orders");
}

fn orphan() -> i32 {
    41 + 1
}

pub fn serve() -> i32 {
    7
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own `.logos/logos.db`, then drop the engine so the
/// store is closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
    }
}

/// The bytes of a member's persisted graph store — including the WAL sidecars, so
/// a write parked in `logos.db-wal` cannot slip past a "nothing was written" check.
fn db_bytes(root: &Path) -> Vec<u8> {
    let logos = root.join(".logos");
    let mut bytes = fs::read(logos.join("logos.db")).expect("member db exists");
    for sidecar in ["logos.db-wal", "logos.db-shm"] {
        if let Ok(extra) = fs::read(logos.join(sidecar)) {
            bytes.extend_from_slice(&extra);
        }
    }
    bytes
}

/// Read one member's per-repo dead-code verdicts directly from its store — the
/// `is_dead` column `scan`/`gate` score against. Only definite verdicts are
/// returned; a `NULL` (not-computed) node contributes nothing.
fn per_repo_dead_verdicts(root: &Path) -> Vec<(String, bool)> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join(".logos").join("logos.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("member db opens read-only");
    let mut stmt = conn
        .prepare("SELECT name, is_dead FROM nodes WHERE is_dead IS NOT NULL ORDER BY id")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    rows
}

/// A two-member Rust workspace (`api` publishes, `web` subscribes), both indexed.
fn workspace(root: &Path) -> (PathBuf, PathBuf) {
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "src/main.rs", PUBLISHER);
    write(&web, "src/main.rs", SUBSCRIBER);
    index_member(&api);
    index_member(&web);
    (api, web)
}

/// [FR-WS-12] AC1, end to end: a real 2-repo index promotes an otherwise-dead
/// subscribe handler reachable only via a cross-member edge to live — the union
/// view's promotion bucket is non-empty on a real index for the first time.
#[test]
fn a_cross_member_publish_promotes_an_otherwise_dead_subscribe_handler_to_live() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (api, web) = workspace(root);

    // The handler really is dead in web's own graph before the union view runs —
    // the precondition the promotion is meaningful against.
    let before = per_repo_dead_verdicts(&web);
    assert!(
        before.iter().any(|(name, dead)| name == "on_order" && *dead),
        "the fixture's `on_order` must be dead in web's own graph: {before:?}"
    );

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("api", &api), member("web", &web)]),
        RegistryMode::Lazy,
    );
    let bridge = ContractBridge::new();

    // Warm the engines before snapshotting so the checksum isolates the view's own
    // reads from the one-time store open.
    let _ = bridge.edges(&registry);
    let api_before = db_bytes(&api);
    let web_before = db_bytes(&web);

    // ── The cross-member broker edge, through the LIVE bridge ─────────────────
    let edges = bridge.edges(&registry);
    assert_eq!(
        edges.len(),
        1,
        "api's publish binds web's subscribe on the shared `orders` topic: {edges:?}"
    );
    let edge = &edges[0];
    assert_eq!(edge.relation, "broker-topic");
    assert_eq!(edge.from.member, "api", "the publish is the edge source");
    assert!(
        edge.from.symbol.as_str().contains("emit_order"),
        "the near endpoint is the real publishing fn: {}",
        edge.from.symbol.as_str()
    );
    assert_eq!(edge.to.member, "web", "the subscribe is the edge target");
    assert!(
        edge.to.symbol.as_str().contains("on_order"),
        "the far endpoint is the real subscribing handler: {}",
        edge.to.symbol.as_str()
    );
    // A broker edge is an INVOCATION edge — it seeds a reachability root (unlike a
    // contract-surface edge). Without this the union_roots filter ([CR-083]) would
    // decline to seed it and the promotion below could never happen.
    assert_eq!(
        edge.intake,
        BridgeIntake::Invocation,
        "a broker publish/subscribe is a captured invocation, not a contract surface"
    );
    assert!(edge.intake.seeds_reachability_root());

    // ── The promotion [FR-WS-12] AC1 exists for ───────────────────────────────
    let view = app_wide_reachability(&registry, &edges);
    assert_eq!(view.view, UNION_VIEW);
    assert!(view.advisory);

    let promoted: Vec<&str> = view
        .live_via_cross_service
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(
        promoted,
        ["on_order"],
        "the dead subscribe handler is promoted to live via the cross-member edge — \
         the real-path promotion is non-empty: {:?}",
        view.live_via_cross_service
    );
    let claim = &view.live_via_cross_service[0];
    assert_eq!(claim.member, "web");
    assert_eq!(claim.verdict, AppWideVerdict::LiveViaCrossService);
    assert_eq!(claim.kind, NodeKind::Function);

    // web resolved exactly one extra root (the subscribe provider) and no root
    // went unresolved — the BridgeEndpoint.symbol / NodeRow.symbol spellings agree.
    let web_tally = view
        .members
        .iter()
        .find(|m| m.member == "web")
        .expect("web has a tally");
    assert_eq!(
        (web_tally.extra_roots, web_tally.unresolved_roots),
        (1, 0),
        "web resolves exactly the subscribe provider as a root"
    );
    assert_eq!(web_tally.live_via_cross_service, 1);

    // ── Honesty: monotone toward live, nothing promoted on no evidence ────────
    // `orphan` is dead per-repo and no cross-service edge reaches it — it stays dead.
    let dead: HashSet<&str> = view.dead.iter().map(|c| c.name.as_str()).collect();
    assert!(
        dead.contains("orphan"),
        "an unreached dead callable stays dead app-wide: {dead:?}"
    );
    assert!(
        !dead.contains("on_order"),
        "the promoted handler must not also appear in the dead bucket"
    );
    // The exported `serve` is live per-repo, so the union view never claims it.
    for live in ["serve", "emit_order"] {
        assert!(
            !dead.contains(live),
            "{live} is live per-repo — the union view must not verdict it dead"
        );
    }

    // Monotonicity, in arithmetic: every per-repo dead callable is accounted for as
    // either promoted or still-dead — the view can neither add nor lose one.
    for tally in &view.members {
        assert_eq!(
            tally.dead_per_repo,
            tally.live_via_cross_service + tally.dead_app_wide,
            "member {} loses or invents a dead callable",
            tally.member
        );
    }

    // Every claim carries the coverage rider it rests on ([FR-WS-05]).
    for claim in view.live_via_cross_service.iter().chain(view.dead.iter()) {
        assert_eq!(
            claim.coverage, view.coverage,
            "claim {} must carry the coverage rider it rests on",
            claim.name
        );
    }
    assert_eq!(view.coverage.members_read, 2, "both members read");
    assert_eq!(view.coverage.bound, 1, "the publish bound its subscriber");

    // ── No demotion / advisory: the per-repo gated signal is unchanged ────────
    let after = per_repo_dead_verdicts(&web);
    assert_eq!(
        before, after,
        "computing the advisory union view must leave web's per-repo dead-code signal \
         byte-for-byte unchanged (ADR-56) — even the promoted `on_order` is still \
         `is_dead = true` in its own repo"
    );
    // The app-wide dead set is a strict subset of the per-repo dead set (monotone).
    let per_repo_dead: HashSet<&str> = before
        .iter()
        .filter(|(_, dead)| *dead)
        .map(|(name, _)| name.as_str())
        .collect();
    let app_wide_dead: HashSet<&str> = view
        .dead
        .iter()
        .filter(|c| c.member == "web")
        .map(|c| c.name.as_str())
        .collect();
    assert!(
        app_wide_dead.is_subset(&per_repo_dead),
        "app-wide dead {app_wide_dead:?} must be a subset of per-repo dead \
         {per_repo_dead:?} — the union view is monotone toward live"
    );

    // ── The view is a pure read: no member DB was written ([ADR-52]) ──────────
    assert_eq!(db_bytes(&api), api_before, "member `api` DB unchanged");
    assert_eq!(db_bytes(&web), web_before, "member `web` DB unchanged");
}
