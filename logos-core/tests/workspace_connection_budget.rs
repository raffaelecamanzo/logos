//! Fitness function for the workspace-wide resource budget — read connections
//! (S-324) and worker threads (S-325) — under [CR-100], [NFR-PE-11], [BR-45],
//! [ADR-63], and for the warm read-model that rides the same walk without
//! widening it (S-323, [FR-WS-15], [NFR-PE-10]).
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
//! It is also where [NFR-PE-11]'s **thread** half is asserted at the size its
//! measurable target names: 72 resident-or-evicted members must cost the host's
//! core count in `rayon` workers, not `members × cores` (~864 on the measured
//! 12-core host), with per-member query latency measured in the same breath so a
//! starvation regression against [NFR-PE-01] fails here rather than passing
//! quietly.
//!
//! [NFR-PE-01]: ../../docs/specs/requirements/NFR-PE-01.md
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
    workspace_status, WorkspaceBudget, EngineRegistry, Federation, Member, MemberWarmState,
    RegistryMode,
};
use logos_core::{live_worker_threads, Engine};

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

/// The [NFR-PE-01] point-query budget, applied to the far smaller fixtures here
/// as a *starvation* alarm rather than as a benchmark.
///
/// What it guards is the **connection** half of the budget: a resident member
/// answers on its budgeted share of read connections, so a share squeezed too
/// small shows up as query latency here. It does not guard the worker pool —
/// steady-state navigation never enters it ([ADR-11]) — which is asserted
/// directly, under a saturated pool, in `workspace_shared_worker_pool.rs`.
///
/// [ADR-11]: ../../docs/specs/architecture/decisions/ADR-11.md
const POINT_QUERY_MS: u128 = 100;

/// Poll `condition` until it holds or a generous deadline passes.
///
/// Needed only for thread counts: `rayon` starts and terminates workers
/// asynchronously, so the gauge is eventually — not immediately — consistent
/// with the pools that exist.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    condition()
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
    let budget = WorkspaceBudget::from_limits(STOCK_SOFT_LIMIT, cores());
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
        "every un-indexed, never-attempted member reads `deferred`, not `warm` \
         and not `degraded`"
    );
    assert_eq!(status.warm_rollup.members, MEMBERS);
    assert_eq!(
        (status.warm_rollup.warm, status.warm_rollup.deferred, status.warm_rollup.degraded),
        (0, MEMBERS, 0),
        "the roll-up partitions the workspace and agrees with the rows"
    );
    assert_eq!(
        status.warm_rollup.warming, None,
        "no live warming signal exists, so the count is OMITTED rather than \
         inferred ([NFR-CC-04])"
    );
    let wire = serde_json::to_value(&status).expect("the read-model serialises");
    assert!(
        wire["warm_rollup"].get("warming").is_none(),
        "`--json` must carry no `warming` key at all: {}",
        wire["warm_rollup"]
    );
    assert_eq!(wire["members"][0]["warm_state"], "deferred");

    // ── S-326: the OPEN-state axis over the same N = 72 walk ──────────────
    //
    // **The story's central claim, at the N where eviction is unavoidable.**
    // This budget holds far fewer than 72 members resident (asserted above), so
    // the four all-member walks evict continuously — and every member must still
    // read `opened`, because eviction reclaims a start that SUCCEEDED
    // ([BR-45]). An open state derived from residency rather than from open
    // attempts would report ~64 degraded members here and exit non-zero on a
    // completely healthy workspace, which is a worse defect than the exit-0 this
    // story removes.
    //
    // It also pins the other half of [FR-WS-16]: an all-`deferred` workspace —
    // nothing indexed, every member openable — is NOT degraded. The two axes
    // disagree here by design, which is why they are separate fields.
    let unopened: Vec<&str> = status
        .members
        .iter()
        .filter(|m| m.open.is_degraded())
        .map(|m| m.status.member.as_str())
        .collect();
    assert!(
        unopened.is_empty(),
        "{} of {MEMBERS} members read `degraded` on the open axis under \
         continuous eviction — eviction is not a failure ([BR-45]): {unopened:?}",
        unopened.len(),
    );
    assert_eq!(
        (
            status.degraded_rollup.members,
            status.degraded_rollup.opened,
            status.degraded_rollup.not_attempted,
        ),
        (MEMBERS, MEMBERS, 0),
        "every member was attempted and opened: {:?}",
        status.degraded_rollup,
    );
    assert!(
        status.degraded_rollup.degraded_members.is_empty(),
        "a healthy workspace names nobody degraded: {:?}",
        status.degraded_rollup.degraded_members,
    );
    assert!(
        status.degraded_rollup.covers_all_members,
        "and its figures cover all {MEMBERS} members"
    );
    assert_eq!(
        (status.coverage.members_read, status.coverage.members_total),
        (MEMBERS as u64, MEMBERS as u64),
        "the coverage summary was computed over every member, and says so"
    );
    assert!(status.coverage.covers_all_members);
    assert_eq!(wire["members"][0]["open_state"], "opened");
    assert_eq!(wire["degraded_rollup"]["covers_all_members"], true);

    // [NFR-PE-10], instrumented on CONSTRUCTION: the warm labelling opened
    // nothing of its own.
    //
    // Residency cannot carry this claim — eviction pins it to the budget however
    // many engines were built, so `resident_count() < MEMBERS` is already implied
    // by the two budget assertions above and would hold even if labelling opened
    // every member a second time. Neither can `reconstructions()`: a second open
    // *within* one fan-out iteration finds the member still resident, so it moves
    // no counter at all. Total starts is the quantity that moves, and it is
    // pinned to an EQUALITY: exactly one start per member per all-member walk.
    // A labelling that opened anything, or a fifth walk added to `status`,
    // both fail here.
    assert_eq!(
        registry.engine_starts(),
        WALKS_PER_STATUS * MEMBERS as u64,
        "{} engine starts over {WALKS_PER_STATUS} all-member walks of {MEMBERS} \
         members — the warm AND open-state labelling must construct no engine \
         of their own, and no walk may be added to `workspace status`",
        registry.engine_starts(),
    );

    for repo in &member_roots {
        assert!(
            repo.join(".logos").join("logos.db").exists(),
            "member {} got no store of its own",
            repo.display()
        );
    }

    // ── the thread half of the same budget (S-325, [NFR-PE-11], [ADR-63]) ──
    //
    // Every resident member engine runs on ONE injected pool, so the workspace's
    // whole thread cost is what a single engine would have spawned for itself.
    // Left to themselves the resident engines would have built one pool each.
    let per_member_pools = budget.max_resident_members() * budget.worker_threads();
    assert!(
        registry.resident_count() > 1,
        "the pool-sharing assertions need more than one resident member; only \
         {} are resident",
        registry.resident_count(),
    );
    assert_eq!(
        registry.shared_worker_threads(),
        budget.worker_threads(),
        "the {} resident member engines are not sharing one budgeted pool",
        registry.resident_count(),
    );
    assert!(
        wait_until(|| live_worker_threads() == budget.worker_threads()),
        "this process runs {} rayon workers for a {MEMBERS}-member workspace; \
         the budget is {} (per-member pools would have cost {per_member_pools} \
         for the resident set alone, and {} had every member stayed resident)",
        live_worker_threads(),
        budget.worker_threads(),
        MEMBERS * budget.worker_threads(),
    );

    // Per-member latency, measured alongside the thread count so a shared pool
    // that bounded threads by starving members would fail here rather than pass
    // quietly ([NFR-PE-01]). Timed on the members still resident after the walk,
    // so this is steady-state query cost and not another cold start; each is
    // warmed once first, because a member's first navigation call runs the
    // FR-IX-07 auto-index prologue.
    let resident = registry.resident_members();
    let mut latencies_ms: Vec<u128> = Vec::with_capacity(resident.len());
    for member in &resident {
        let engine = registry.engine_for(member).expect("a resident member");
        let _ = engine.search("f", None, None);
        let started = std::time::Instant::now();
        let _ = engine.search("f", None, None);
        latencies_ms.push(started.elapsed().as_millis());
    }
    // The **worst** of the resident set, not a percentile: the sample is the
    // budgeted resident count, so any p95 index rounds to the maximum anyway.
    // Naming it honestly keeps the assertion from reading as more statistically
    // forgiving than it is.
    latencies_ms.sort_unstable();
    let worst = latencies_ms.last().copied().unwrap_or_default();
    eprintln!(
        "shared pool: {} workers for {MEMBERS} members ({} resident); per-member \
         search worst {worst} ms of {latencies_ms:?}",
        registry.shared_worker_threads(),
        resident.len(),
    );
    assert!(
        worst < POINT_QUERY_MS,
        "a member's search took {worst} ms over {} resident members on a budgeted \
         {} read connections each — the budget is starving members \
         ([NFR-PE-01]); full distribution: {latencies_ms:?}",
        resident.len(),
        budget.per_member_read_connections(),
    );

    // Teardown: the pool belongs to the engines, so releasing the last of them
    // must leave no worker behind ([ADR-63]).
    drop(registry);
    assert!(
        wait_until(|| live_worker_threads() == 0),
        "{} rayon worker(s) outlived the workspace that built them",
        live_worker_threads()
    );
}
