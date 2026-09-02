//! Fitness function for the workspace-wide read-connection budget (S-324,
//! [CR-100], [NFR-PE-11], [BR-45], [ADR-63]), and for the warm read-model that
//! rides the same walk without widening it (S-323, [FR-WS-15], [NFR-PE-10]).
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
//! [FR-WS-15]: ../../docs/specs/requirements/FR-WS-15.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
#![cfg(unix)]

use std::path::{Path, PathBuf};

use logos_core::federation::{
    workspace_status, ConnectionBudget, EngineRegistry, Federation, Member, MemberWarmState,
    RegistryMode,
};
use logos_core::Engine;

/// Members in the fixture — the size of the workspace that failed ([CR-100] §2).
const MEMBERS: usize = 72;

/// The stock macOS `RLIMIT_NOFILE` soft limit, and the one the failure was
/// measured under.
const STOCK_SOFT_LIMIT: u64 = 256;

/// All-member walks one `workspace status` performs today, verified by tracing
/// `federation::query::workspace_status`: the per-member status read-model
/// (`query.rs`, via `fan`), the cross-service coverage tier's **two** passes over
/// the contract surface and the invocation references (`coverage.rs`), and the
/// topic inventory (`topics.rs`). The contract bridge's own stamp/read passes are
/// *not* on this path — they belong to `ContractBridge::edges`, which
/// `workspace_status` never calls.
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
// `rlim_t` is `u64` on every target this `#![cfg(unix)]` binary is built for, so
// the `try_from` below is a no-op there.
#[allow(clippy::useless_conversion)]
fn current_soft_limit() -> u64 {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` writes a fully-initialised `rlimit` through a pointer
    // to our own stack slot; it neither escapes nor aliases.
    assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) }, 0);
    u64::try_from(limit.rlim_cur).expect("a soft limit fits in u64")
}

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

/// Restores the process's `RLIMIT_NOFILE` soft limit on **every** exit path.
///
/// The limit is process-wide and the code under test runs between lowering and
/// restoring, so a panic in `workspace_status` — the very failure this test
/// exists to detect — would otherwise unwind past a manual restore and leave the
/// rest of the binary, including the teardown of 72 member stores, running at 256
/// descriptors.
struct SoftLimitGuard(u64);

impl Drop for SoftLimitGuard {
    fn drop(&mut self) {
        lower_soft_limit(self.0);
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
    let _restore = SoftLimitGuard(lower_soft_limit(STOCK_SOFT_LIMIT));

    // `lower_soft_limit` installs `min(requested, hard)`, so on a host whose hard
    // limit is below 256 the fixture would silently be testing a tighter envelope
    // than the budget below is derived for — reporting an environment problem as
    // a budget failure. Assert what is actually in force.
    let installed = current_soft_limit();
    assert_eq!(
        installed, STOCK_SOFT_LIMIT,
        "this host's hard limit is below {STOCK_SOFT_LIMIT}; the fixture cannot \
         mean what it claims"
    );

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

    let failures: Vec<&str> = status
        .members
        .iter()
        .filter_map(|m| m.status.error.as_deref())
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
    // `status.members` carries the errors of the FIRST walk only — the coverage
    // and topic tiers have no per-member error channel, so a member that failed
    // to open during walks 2-4 would vanish silently. That is the exact defect
    // class CR-100 §2 reports ("exited 0 with 63 members unopened"), so the
    // zero-failure claim is asserted registry-wide, across every walk.
    assert_eq!(
        registry.start_failures(),
        0,
        "{} engine start(s) failed across the {WALKS_PER_STATUS} all-member \
         walks, {} of which no read-model would have reported",
        registry.start_failures(),
        registry.start_failures() - failures.len() as u64,
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

    // ── S-323: the warm read-model over the same N = 72 walk ──────────────
    //
    // The fixture indexes nothing, so every member is genuinely un-indexed and
    // never attempted — the all-`deferred` workspace [FR-WS-15] must report
    // honestly, and the exact state the bounded warm ([FR-WS-14]) makes normal.
    // Asserted here rather than in a fixture of its own because the interesting
    // claim is that labelling all N members costs **nothing** on top of the walk
    // that was already happening ([NFR-PE-10]), which needs the real N = 72
    // registry: the `resident_count()`/`reconstructions()` bounds above are
    // unchanged by this story, so the labels rode along on freshness the fan-out
    // had already produced rather than opening anything of their own.
    let deferred = status
        .members
        .iter()
        .filter(|m| m.warm == MemberWarmState::Deferred)
        .count();
    assert_eq!(
        deferred, MEMBERS,
        "every un-indexed, never-attempted member reads `deferred`, not `warm`          and not `degraded`"
    );
    assert_eq!(status.warm_rollup.members, MEMBERS);
    assert_eq!(
        (status.warm_rollup.warm, status.warm_rollup.deferred, status.warm_rollup.degraded),
        (0, MEMBERS, 0),
        "the roll-up partitions the workspace and agrees with the rows"
    );
    assert_eq!(
        status.warm_rollup.warming, None,
        "no live warming signal exists, so the count is OMITTED rather than          inferred ([NFR-CC-04])"
    );
    let wire = serde_json::to_value(&status).expect("the read-model serialises");
    assert!(
        wire["warm_rollup"].get("warming").is_none(),
        "`--json` must carry no `warming` key at all: {}",
        wire["warm_rollup"]
    );
    assert_eq!(wire["members"][0]["warm_state"], "deferred");
    // [NFR-PE-10], instrumented: labelling N members constructed no engine of
    // its own, so live residency after a full `status` is still far BELOW N.
    assert!(
        registry.resident_count() < MEMBERS,
        "{} of {MEMBERS} engines resident after a warm-labelled status — the          read-model must not eagerly hold all N",
        registry.resident_count(),
    );

    for repo in &member_roots {
        assert!(
            repo.join(".logos").join("logos.db").exists(),
            "member {} got no store of its own",
            repo.display()
        );
    }
}
