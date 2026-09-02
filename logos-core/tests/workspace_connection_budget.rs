//! Fitness function for the workspace-wide read-connection budget (S-324,
//! [CR-100], [NFR-PE-11], [BR-45], [ADR-63]).
//!
//! The unit tests in `federation::registry` prove the ceiling against spy
//! engines. This proves it against the **operating system**: 72 real member
//! [`Engine`]s over 72 real stores, walked by the real
//! [`workspace_status`](logos_core::federation::workspace_status) read-model,
//! with this process's `RLIMIT_NOFILE` soft limit deliberately lowered to the
//! stock macOS 256 — the exact envelope in which the measured workspace
//! exhausted its descriptors at member 10 and left **63 of 72 members
//! unopened**. Under the budget every member must open.
//!
//! # Why this file holds exactly one test
//! Lowering `RLIMIT_NOFILE` is **process-wide**, and cargo runs a test binary's
//! `#[test]`s on parallel threads of one process. A second test here would run
//! under the lowered limit and compete for the same descriptor table, so its
//! failures would be an artefact of this test rather than of the code. The
//! single-root byte-for-byte pin therefore lives in its own binary
//! (`single_root_pool_invariant.rs`).
//!
//! [CR-100]: ../../docs/requests/CR-100-workspace-resource-budget.md
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md
//! [BR-45]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md
#![cfg(unix)]

use std::path::{Path, PathBuf};

use logos_core::federation::{
    workspace_status, ConnectionBudget, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::Engine;

/// Members in the fixture — the size of the workspace that failed ([CR-100] §2).
const MEMBERS: usize = 72;

/// The stock macOS `RLIMIT_NOFILE` soft limit, and the one the failure was
/// measured under.
const STOCK_SOFT_LIMIT: u64 = 256;

/// All-member walks one `workspace status` performs today: the per-member
/// status read-model, the contract bridge's stamp and read passes, and the topic
/// inventory.
///
/// Named because it is the multiplier on the budget's cost: a workspace larger
/// than the budget rebuilds every member on every walk after the first, so each
/// extra walk is another N cold starts. Asserting against it turns "someone
/// added a fan-out to `workspace status`" from a silent latency rise into a
/// failing test.
const WALKS_PER_STATUS: u64 = 4;

/// Lower this process's `RLIMIT_NOFILE` soft limit to `soft`, returning the
/// limit that was in force before.
///
/// Deliberately local to this test rather than a `logos_core` API: nothing in
/// the product ever *lowers* its own descriptor allowance, and a public
/// mutator would be a foot-gun that exists only for one fitness function.
// `rlim_t` is already `u64` on every target we ship, so the widening below is a
// no-op there — but it is platform-defined and narrower on some, and writing it
// explicitly is what keeps this compiling on those.
#[allow(clippy::useless_conversion)]
fn lower_soft_limit(soft: u64) -> u64 {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: both calls read/write a fully-initialised `rlimit` through a
    // pointer to our own stack slot; neither pointer escapes or aliases.
    let previous = unsafe {
        assert_eq!(
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit),
            0,
            "the platform must report RLIMIT_NOFILE for this test to mean anything"
        );
        let previous = limit.rlim_cur;
        limit.rlim_cur = soft.min(limit.rlim_max);
        assert_eq!(
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit),
            0,
            "lowering our own soft limit must be permitted"
        );
        previous
    };
    u64::try_from(previous).expect("a soft limit fits in u64")
}

/// A minimal member repo: one tiny source file, no index. The budget is a
/// property of *residency*, so the fixture needs many members, not large ones —
/// a real store per member is what puts real descriptors on the table.
fn member_repo(root: &Path, name: &str) -> Member {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).expect("member dir");
    std::fs::write(repo.join("lib.rs"), "pub fn f() {}\n").expect("member source");
    Member {
        name: name.to_string(),
        root: repo,
    }
}

fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// [NFR-PE-11] acceptance: `workspace status` over a 72-member workspace
/// completes with **zero** engine-start failures under a 256-descriptor soft
/// limit, holding no more than the budgeted live read connections throughout.
#[test]
fn workspace_status_opens_every_member_under_a_256_fd_limit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let members: Vec<Member> = (0..MEMBERS)
        .map(|i| member_repo(root, &format!("svc{i:03}")))
        .collect();
    let member_roots: Vec<PathBuf> = members.iter().map(|m| m.root.clone()).collect();

    // Lower the limit only once the fixture is on disk, so directory creation
    // is not the thing that runs out of descriptors.
    let restore_to = lower_soft_limit(STOCK_SOFT_LIMIT);

    // The budget this host would derive *for a 256-fd limit*, stated explicitly
    // so the assertion is about the budget's arithmetic and not about whatever
    // limit the CI runner happens to grant.
    let budget = ConnectionBudget::from_limits(STOCK_SOFT_LIMIT, cores());
    let federation = Federation {
        name: "workspace".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
    };
    let registry =
        EngineRegistry::<Engine>::with_budget(federation, RegistryMode::Lazy, budget);

    let status = workspace_status(&registry);

    // Restore before asserting: a panic here must not leave the rest of the
    // binary's teardown running under a 256-descriptor limit.
    lower_soft_limit(restore_to);

    let failures: Vec<&str> = status
        .members
        .iter()
        .filter_map(|m| m.error.as_deref())
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {MEMBERS} members failed to open under a {STOCK_SOFT_LIMIT}-fd \
         limit; first failure: {}",
        failures.len(),
        failures.first().copied().unwrap_or_default(),
    );
    assert_eq!(
        status.members.len(),
        MEMBERS,
        "every member is reported, not merely the ones that opened"
    );

    // The ceiling held: residency never exceeded the budget, and every member
    // really did get its own store rather than sharing one root.
    assert!(
        registry.resident_count() <= budget.max_resident_members(),
        "{} members stayed resident, over the budgeted {}",
        registry.resident_count(),
        budget.max_resident_members(),
    );
    assert!(registry.live_read_connections() <= budget.total_read_connections());
    assert!(
        budget.max_resident_members() < MEMBERS,
        "the fixture must actually exercise eviction: {MEMBERS} members against \
         a budget of {}",
        budget.max_resident_members(),
    );
    // Reconstruction is the budget's cost, so it is measured rather than
    // absorbed ([NFR-PE-11] acceptance). The first walk builds each member
    // fresh; every later walk rebuilds all of them, because the budget holds far
    // fewer than MEMBERS. Anything above that means a single walk is
    // double-building, or a walk was added.
    let reconstructions = registry.reconstructions();
    let explained_by_eviction = (WALKS_PER_STATUS - 1) * MEMBERS as u64;
    assert!(
        reconstructions <= explained_by_eviction,
        "{reconstructions} reconstructions over {WALKS_PER_STATUS} all-member \
         walks of {MEMBERS} members — above the {explained_by_eviction} that \
         {WALKS_PER_STATUS} walks under this budget explain"
    );
    eprintln!(
        "workspace status over {MEMBERS} members: {reconstructions} engine \
         reconstructions, {} resident of a budgeted {}, {} live read connections \
         of a budgeted {}",
        registry.resident_count(),
        budget.max_resident_members(),
        registry.live_read_connections(),
        budget.total_read_connections(),
    );

    for repo in &member_roots {
        assert!(
            repo.join(".logos").join("logos.db").exists(),
            "member {} got no store of its own",
            repo.display()
        );
    }
}
