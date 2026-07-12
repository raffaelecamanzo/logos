//! End-to-end test for the app-wide cross-service reachability union view
//! (S-257, [CR-061], [FR-WS-12], [ADR-56], [NFR-CC-04]).
//!
//! Where the unit tests drive the union composition with fake surfaces, this
//! drives the **real** path: two member repositories, each indexed by its own
//! [`Engine`] over the real grammars — so the `is_dead` column the view reads is
//! the one the annotation pass actually wrote, and the bridge edges are the ones
//! the contract bridge actually matched.
//!
//! The load-bearing guarantees it proves are the *honesty* ones, which is where
//! this view can only fail silently:
//!
//! - the app-wide dead set never exceeds the per-repo dead set (monotone toward
//!   live — a missing invocation edge cannot mark anything dead);
//! - a `NULL` verdict survives the view untouched (tri-state preserved);
//! - the per-repo gated signal is byte-for-byte unchanged with the view computed
//!   vs. not computed (advisory, never a gate input);
//! - every claim carries its coverage rider.
//!
//! Gated on both grammars so a build excluding either does not run it.
#![cfg(all(feature = "lang-yaml", feature = "lang-rust"))]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    app_wide_reachability, AppWideVerdict, ContractBridge, EngineRegistry, Federation, Member,
    RegistryMode, UNION_VIEW,
};
use logos_core::model::NodeKind;
use logos_core::Engine;

/// The `api` member's OpenAPI spec: its `get` operation is the cross-service
/// consumer that binds `web`'s route; its `delete` binds nothing.
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
    delete:
      summary: Delete a user
";

/// The `web` member: an axum route to `get_user`, which calls a private helper —
/// both live per-repo (the framework `route` node roots the handler). Alongside
/// them `orphan` is called by nobody at all: the annotation pass verdicts it
/// `is_dead = true`, and **no** cross-service edge reaches it, so the union view
/// must leave it dead. That is the "never promoted on no evidence" half of the
/// honesty contract.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

fn render(name: &str) -> String {
    format!("user: {name}")
}

async fn get_user() -> String {
    render("ada")
}

fn orphan() -> i32 {
    41 + 1
}

fn app() -> Router {
    Router::new().route("/users/{id}", get(get_user))
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own `.logos/logos.db`, then drop the engine so
/// the store is closed before the registry re-opens it.
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

/// The bytes of a member's persisted graph store — **including the WAL sidecars**.
///
/// The store runs in WAL mode (`PRAGMA journal_mode = WAL`), so a write can land
/// in `logos.db-wal` and leave `logos.db` byte-identical until a checkpoint. Reading
/// only the main file would let a real write slip past the "nothing was written"
/// assertion; the sidecars close that hole.
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

/// A two-member workspace (`api` + `web`), both indexed.
fn workspace(root: &Path) -> (PathBuf, PathBuf) {
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&web, "src/main.rs", AXUM_MAIN);
    index_member(&api);
    index_member(&web);
    (api, web)
}

/// FR-WS-12 / ADR-56 acceptance over the real path: the union view is labeled
/// advisory, carries a coverage rider on **every** claim, never exceeds the
/// per-repo dead set, and writes nothing to any member DB.
#[test]
fn the_union_view_is_advisory_riderd_and_never_exceeds_the_per_repo_dead_set() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (api, web) = workspace(root);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("api", &api), member("web", &web)]),
        RegistryMode::Lazy,
    );
    let bridge = ContractBridge::new();

    // Warm the engines before snapshotting (opening a store can checkpoint its
    // WAL into the main db file), so the checksum isolates the view's own reads.
    let _ = bridge.edges(&registry);
    let api_before = db_bytes(&api);
    let web_before = db_bytes(&web);

    let edges = bridge.edges(&registry);
    assert_eq!(edges.len(), 1, "the GET operation binds web's route: {edges:?}");

    let view = app_wide_reachability(&registry, &edges);

    // Explicitly labeled and advisory (ADR-56).
    assert_eq!(view.view, UNION_VIEW);
    assert!(view.advisory);

    // The coverage rider is present and rides every single claim (FR-WS-05).
    assert_eq!(view.coverage.members_read, 2, "both members read");
    assert_eq!(view.coverage.members_total, 2);
    assert_eq!(view.coverage.bound, 1, "the GET operation bound");
    for claim in view.live_via_cross_service.iter().chain(view.dead.iter()) {
        assert_eq!(
            claim.coverage, view.coverage,
            "claim {} must carry the coverage rider it rests on",
            claim.name
        );
    }

    // ── The one real integration seam ───────────────────────────────────────
    // A `BridgeEndpoint`'s symbol comes from the *contract surface* read; the
    // reachability surface's symbols come from `all_nodes`. `walk_union` matches
    // the two by symbol. If those spellings ever diverge, EVERY root silently
    // becomes unresolved, the view goes permanently inert — and every other
    // assertion in this file still passes green (the dead set would still equal
    // the per-repo dead set, monotonicity would still hold, the rider would still
    // be attached). This is the assertion that makes the feature falsifiable.
    let web_tally = view
        .members
        .iter()
        .find(|m| m.member == "web")
        .expect("web has a tally");
    assert_eq!(
        (web_tally.extra_roots, web_tally.unresolved_roots),
        (1, 0),
        "the bridge's provider endpoint must RESOLVE against web's reachability \
         surface — BridgeEndpoint.symbol and NodeRow.symbol are the same spelling"
    );
    assert!(view.skipped_members.is_empty(), "no member degraded");

    // The resolved root promotes nothing *in this fixture*, and that is the view
    // being correct, not broken: both provider endpoints here are framework `Route`
    // nodes, which the per-repo walk already roots (ADR-56's own Notes), so there was
    // nothing dead to lift.
    //
    // This assertion is scoped to THIS fixture and nothing more. It was originally
    // written as a tripwire meant to fail once S-256 landed a callable provider
    // endpoint (the broker subscribe side) — but it cannot serve that purpose: the
    // fixture is yaml + rust, and neither ships a `brokers.scm`, so no broker edge
    // can ever reach it. S-256 has since landed, the roots DO now resolve to real
    // subscriber methods, and this still passes.
    //
    // The real guard on the promotion path lives where the blocker actually is — the
    // plugin capability matrix — in
    // `federation::reach::tests::the_broker_promotion_path_is_still_blocked_by_the_capability_matrix`.
    assert!(
        view.live_via_cross_service.is_empty(),
        "this fixture's providers are all framework routes (already per-repo live roots), \
         so it can promote nothing: {:?}",
        view.live_via_cross_service
    );

    // `orphan` is dead per-repo and no cross-service edge reaches it — the union
    // view leaves it dead rather than promoting on no evidence (NFR-RA-05).
    let dead: HashSet<&str> = view.dead.iter().map(|c| c.name.as_str()).collect();
    assert!(
        dead.contains("orphan"),
        "an unreached dead callable stays dead app-wide: {dead:?}"
    );
    // ...and the route's handler + its helper are live per-repo already, so they
    // are never claimed at all (the union view only speaks about dead callables).
    for live in ["get_user", "render"] {
        assert!(
            !dead.contains(live),
            "{live} is live per-repo — the union view must not verdict it dead"
        );
    }

    // Monotonicity, in arithmetic: every per-repo dead callable is accounted for
    // as either promoted or still-dead — the view can neither add nor lose one.
    for tally in &view.members {
        assert_eq!(
            tally.dead_per_repo,
            tally.live_via_cross_service + tally.dead_app_wide,
            "member {} loses or invents a dead callable",
            tally.member
        );
    }

    // The view is a pure read: no member DB was written (ADR-52).
    assert_eq!(db_bytes(&api), api_before, "member `api` DB unchanged");
    assert_eq!(db_bytes(&web), web_before, "member `web` DB unchanged");
}

/// ADR-56 acceptance: computing the union view leaves the **per-repo gated
/// signal** byte-for-byte unchanged. Proven by comparing each member's own
/// dead-code verdicts (the `is_dead` column the gate and `scan` read) taken
/// before the view is computed against the same verdicts taken after — the view
/// is a read-model layered *over* them, never a writer of them.
#[test]
fn computing_the_view_leaves_the_per_repo_dead_code_signal_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (api, web) = workspace(root);

    // The per-repo signal as each member's own single-root engine reports it.
    let before = per_repo_dead_verdicts(&web);
    assert!(
        before.iter().any(|(name, dead)| name == "orphan" && *dead),
        "the fixture's `orphan` is dead in web's own graph: {before:?}"
    );

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("api", &api), member("web", &web)]),
        RegistryMode::Lazy,
    );
    let edges = ContractBridge::new().edges(&registry);
    let view = app_wide_reachability(&registry, &edges);
    assert!(!view.dead.is_empty(), "the view did compute something");

    let after = per_repo_dead_verdicts(&web);
    assert_eq!(
        before, after,
        "the per-repo dead-code signal must be byte-for-byte unchanged by the \
         advisory union view (ADR-56)"
    );

    // And the union view's own dead set is a subset of it — never a superset.
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
    // Every claim in the `dead` bucket really is a Dead verdict — a non-vacuous
    // check, because `dead` is non-empty here (asserted above). The mirror-image
    // assertion on `live_via_cross_service` would be vacuous today (that bucket is
    // provably empty until S-256 — see the sibling test), so it is deliberately
    // not made here: an `.all()` over an empty vector is an assertion that cannot
    // fail, which is worse than no assertion at all.
    assert!(
        view.dead.iter().all(|c| c.verdict == AppWideVerdict::Dead),
        "the dead bucket carries only Dead verdicts"
    );
    assert!(
        view.dead.iter().all(|c| c.kind == NodeKind::Function),
        "the per-repo dead verdict is only ever written for callables"
    );
}

/// Read one member's per-repo dead-code verdicts directly from its store — the
/// `is_dead` column `scan`/`gate` score against. Only definite verdicts are
/// returned; a `NULL` (not-computed) node contributes nothing, so a view that
/// fabricated a verdict for one would show up as a diff.
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
