//! An LRU-evicted member engine is indistinguishable from a never-evicted one
//! (S-324, [NFR-PE-11], [FR-DB-01], [ADR-63]).
//!
//! [ADR-63] justifies eviction on the grounds that "an evicted member
//! reconstructs on next touch and is indistinguishable from a never-evicted one,
//! because the store is canonical ([FR-DB-01]) and engines hold no authoritative
//! state". That is a claim about **real** engines over **real** stores, so the
//! spy-engine unit tests in `federation::registry` cannot settle it — this can.
//!
//! The fixture is deliberately tiny (three one-file members) and the budget
//! deliberately hostile (two resident members), because what has to be large
//! here is the *ratio* of members to budget, not the workspace.
//!
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md
//! [FR-DB-01]: ../../docs/specs/requirements/FR-DB-01.md
//! [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md
#![cfg(feature = "lang-rust")]

use std::path::{Path, PathBuf};

use logos_core::federation::{
    ConnectionBudget, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::Engine;

/// A federation over `members`, rooted at `root`.
fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "workspace".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
    }
}

/// A descriptor limit tight enough to budget exactly two resident members, so a
/// three-member walk must evict. Stated as a *limit* rather than as a capacity
/// because that is the whole point: residency is derived, never configured.
const HOSTILE_FD_LIMIT: u64 = 78;

/// Distinct content per member, so a payload that leaked between members —
/// the failure mode "reconstruction returns something else" would look like —
/// is visible rather than masked by identical fixtures.
fn source(member: &str) -> String {
    format!(
        "pub fn {member}_entry() -> u32 {{ {member}_helper() }}\n\
         pub fn {member}_helper() -> u32 {{ 7 }}\n"
    )
}

fn indexed_member(root: &Path, name: &str) -> Member {
    let repo = root.join(name);
    std::fs::create_dir_all(repo.join("src")).expect("src dir");
    std::fs::write(repo.join("src").join("lib.rs"), source(name)).expect("fixture source");
    let engine = Engine::start(&repo).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
    Member {
        name: name.to_string(),
        root: repo,
    }
}

/// The bytes one member answers with, for the query the identity claim is about.
fn answer(registry: &EngineRegistry<Engine>, member: &str) -> String {
    let engine = registry.engine_for(member).expect("member engine starts");
    serde_json::to_string(&engine.search("entry", None, None)).expect("search json")
}

/// [NFR-PE-11] acceptance: an evicted member engine, reconstructed on its next
/// touch, returns results identical to a never-evicted one for the same query.
#[test]
fn an_evicted_member_answers_identically_once_reconstructed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let names = ["alpha", "beta", "gamma"];
    let members: Vec<Member> = names
        .iter()
        .map(|name| indexed_member(root, name))
        .collect();

    let budget = ConnectionBudget::from_limits(HOSTILE_FD_LIMIT, 12);
    assert!(
        budget.max_resident_members() < names.len(),
        "the fixture must exceed the budget ({} members, {} resident) or nothing \
         is ever evicted",
        names.len(),
        budget.max_resident_members(),
    );

    let registry =
        EngineRegistry::<Engine>::with_budget(federation(root, members), RegistryMode::Lazy, budget);

    // The one line that wires the budget to a real engine —
    // `MemberEngine::start` → `Engine::start_with_pools` — asserted here against
    // a live runtime. Reverting it to `Engine::start` would leave every other
    // test in this story green on a host with few enough cores. (The worker-pool
    // half of that same share is pinned in `workspace_shared_worker_pool.rs`.)
    let budgeted = registry.engine_for("alpha").expect("member engine starts");
    assert_eq!(
        budgeted
            .runtime()
            .expect("a started member engine owns a runtime")
            .reader_pool_size(),
        budget.per_member_read_connections(),
        "a resident member must open its budgeted share of read connections, not \
         the core-sized pool an engine sizes for itself"
    );
    drop(budgeted);

    // Answer from a never-evicted engine: `alpha` is the first member touched.
    let never_evicted: Vec<String> = names.iter().map(|n| answer(&registry, n)).collect();

    // Touching every member evicted the earlier ones — `alpha` above all.
    assert!(
        !registry.resident_members().contains(&"alpha".to_string()),
        "alpha was expected to be evicted by the walk; residency = {:?}",
        registry.resident_members()
    );
    let before_reconstruction = registry.reconstructions();

    // Re-touch every member, now from reconstructed engines.
    let reconstructed: Vec<String> = names.iter().map(|n| answer(&registry, n)).collect();
    assert!(
        registry.reconstructions() > before_reconstruction,
        "no engine was actually rebuilt, so this proves nothing"
    );

    for (name, (before, after)) in names
        .iter()
        .zip(never_evicted.iter().zip(reconstructed.iter()))
    {
        assert_eq!(
            before, after,
            "member {name} answered differently after being evicted and rebuilt"
        );
    }

    // …and the members really are distinguishable, so the equality above is not
    // three copies of one empty answer.
    assert_ne!(
        never_evicted[0], never_evicted[1],
        "the fixture members must answer differently from one another"
    );
    assert!(
        never_evicted[0].contains("alpha_entry"),
        "the query must actually return that member's own symbols: {}",
        never_evicted[0]
    );
}

/// Eviction must work under [`RegistryMode::Serve`] too, against **real**
/// engines — the mode the shipped `serve` surface actually runs.
///
/// The spy engines in `federation::registry`'s unit tests carry a watcher that
/// holds nothing, so they cannot see this: a real [`Engine::watch`] spawns a sync
/// worker, and while that worker held a strong `Arc<Engine>` for its whole
/// lifetime, **every** serve-mode resident had a second holder and was therefore
/// skipped by the LRU filter for ever. Residency was unbounded, `evict_to_capacity`
/// returned nothing, and the shared worker pool was never torn down — the whole
/// [NFR-PE-11] budget was inert on the one path that ships it.
///
/// This is the guard that keeps a future watcher (or any other component that
/// captures a member engine) from silently reinstating that.
#[test]
fn a_serve_mode_workspace_evicts_to_its_budget_like_a_lazy_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let budget = ConnectionBudget::from_limits(HOSTILE_FD_LIMIT, 12);

    let names = ["delta", "epsilon", "zeta"];
    assert!(
        budget.max_resident_members() < names.len(),
        "the fixture must exceed the budget ({} members, {} resident) or nothing \
         is ever evicted",
        names.len(),
        budget.max_resident_members(),
    );

    let members: Vec<Member> = names.iter().map(|name| indexed_member(root, name)).collect();
    let registry = EngineRegistry::<Engine>::with_budget(
        federation(root, members),
        RegistryMode::Serve,
        budget,
    );

    // Touch every member. Admission evicts to the budget on each miss, so
    // residency must land at the cap — not at the member count.
    for name in names {
        registry.engine_for(name).expect("member engine starts");
    }
    assert_eq!(
        registry.resident_count(),
        budget.max_resident_members(),
        "serve-mode residency is {} against a budgeted {}; the watcher is pinning \
         member engines against eviction (residency = {:?})",
        registry.resident_count(),
        budget.max_resident_members(),
        registry.resident_members(),
    );

    // An explicit trim must actually evict, and the shared worker pool must go
    // with the last resident engine rather than outliving the workspace.
    let evicted = registry.evict_to_capacity(0);
    assert_eq!(
        evicted.len(),
        budget.max_resident_members(),
        "evict_to_capacity(0) freed {evicted:?} of {} residents under serve",
        budget.max_resident_members(),
    );
    assert_eq!(registry.resident_count(), 0);
    assert_eq!(
        registry.shared_worker_threads(),
        0,
        "the shared pool outlived every resident engine under serve"
    );

    // …and a reconstructed member still answers, so eviction under serve costs a
    // rebuild and nothing else.
    let rebuilt = registry.engine_for(names[0]).expect("member engine starts");
    let answer = serde_json::to_string(&rebuilt.search("entry", None, None)).expect("search json");
    assert!(
        answer.contains(&format!("{}_entry", names[0])),
        "a serve-mode member rebuilt after eviction lost its own symbols: {answer}"
    );
}
