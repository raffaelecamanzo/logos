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

    let registry = EngineRegistry::<Engine>::with_budget(
        Federation {
            name: "workspace".to_string(),
            root: root.to_path_buf(),
            members,
            default: None,
            links: Vec::new(),
            governance: Default::default(),
        },
        RegistryMode::Lazy,
        budget,
    );

    // The one line that wires the budget to a real engine —
    // `MemberEngine::start` → `Engine::start_with_read_pool` — asserted here
    // against a live runtime. Reverting it to `Engine::start` would leave every
    // other test in this story green on a host with few enough cores.
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
