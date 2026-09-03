//! Resident member engines share **one** bounded worker pool (S-325,
//! [NFR-PE-11], [NFR-PE-08], [NFR-PE-01], [BR-45], [ADR-63]).
//!
//! The unit tests in `federation::registry` prove the sharing policy against spy
//! engines; `tests/workspace_connection_budget.rs` proves the ceiling at the
//! 72-member size [NFR-PE-11]'s measurable target names. This proves the
//! properties that only **real** engines over **real** stores can settle, and
//! that a count alone would not:
//!
//! 1. Real member [`Engine`]s injected by the registry submit to one pool, and
//!    that pool is the budget's size rather than one-per-core-per-member.
//! 2. A long-running job occupying **every** worker of the shared pool does not
//!    deadlock, or even delay, another member's navigation query on an
//!    **already-indexed** member — because navigation reads on the calling thread
//!    through the read pool and never enters the worker pool ([ADR-11]). That is
//!    the answer to [ADR-63]'s "a shared worker pool couples members"
//!    consequence, and it is measured here rather than argued. The fixture is
//!    indexed on purpose: a member's *first* navigation call runs the [FR-IX-07]
//!    auto-index prologue, which is a full index on this pool and therefore the
//!    one navigation path that genuinely does queue behind another member.
//!    A real cross-member **worker-pool** job submitted while the pool is
//!    saturated queues, and runs once the long jobs release — no deadlock.
//! 3. Sharing composes with eviction: an evicted member rejoins the same pool.
//! 4. The pool is torn down with the last engine holding it — no orphan threads.
//!
//! # Why this file holds exactly one test
//! [`live_worker_threads`] is process-wide and `cargo` runs a binary's `#[test]`s
//! on parallel threads of one process, so a sibling test building or dropping a
//! pool would move the gauge under this one. The same reason keeps
//! `workspace_connection_budget.rs` to a single test.
//!
//! [NFR-PE-01]: ../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-PE-08]: ../../docs/specs/requirements/NFR-PE-08.md
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md
//! [BR-45]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [FR-IX-07]: ../../docs/specs/requirements/FR-IX-07.md
//! [ADR-11]: ../../docs/specs/architecture/decisions/ADR-11.md
//! [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md
#![cfg(feature = "lang-rust")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use logos_core::federation::{WorkspaceBudget, EngineRegistry, Federation, Member, RegistryMode};
use logos_core::{live_worker_threads, Engine, Runtime};

/// The live runtime of a started member engine.
fn runtime(engine: &Arc<Engine>) -> &Runtime {
    engine
        .runtime()
        .expect("a started member engine owns a runtime")
}

/// Members in the fixture. Three is enough: what has to be large for a sharing
/// claim is the number of engines *alive at once*, not the workspace.
const MEMBERS: [&str; 3] = ["alpha", "beta", "gamma"];

/// Cores the budget is derived for. Stated rather than taken from the host so
/// the pool this test saturates has a known, small size — saturating a
/// 16-worker pool would prove nothing extra and cost sixteen parked threads.
const BUDGET_CORES: usize = 2;

/// A descriptor allowance roomy enough that every member stays resident, so the
/// engines whose pools are compared are all alive at the same moment.
const ROOMY_FD_LIMIT: u64 = 65_536;

/// The [NFR-PE-01] point-query budget. A navigation query issued while every
/// worker of the shared pool is blocked must still land inside it — that is the
/// starvation regression this test exists to catch.
const POINT_QUERY_MS: u128 = 100;

/// How long a blocked-on-the-pool operation is given before the test calls it a
/// deadlock. Generous: it is a liveness bound, not a latency one — the latency
/// bound is [`POINT_QUERY_MS`].
const TIMEOUT: Duration = Duration::from_secs(30);

/// Distinct content per member, so an answer that leaked between members is
/// visible rather than masked by identical fixtures.
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

/// Poll `condition` until it holds or a generous deadline passes.
///
/// `rayon` starts and terminates workers asynchronously, so the thread gauge is
/// eventually — not immediately — consistent with the pools that exist.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

/// A latch the pool-occupying jobs park on until the test releases them.
#[derive(Default)]
struct Latch {
    released: Mutex<bool>,
    signal: Condvar,
}

impl Latch {
    fn wait(&self) {
        let mut released = self.released.lock().unwrap_or_else(|e| e.into_inner());
        while !*released {
            released = self
                .signal
                .wait(released)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.signal.notify_all();
    }
}

/// The whole S-325 claim against real engines: one pool, no starvation, no
/// deadlock, eviction-safe, and no orphaned threads.
#[test]
fn resident_member_engines_share_one_bounded_worker_pool() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let members: Vec<Member> = MEMBERS.iter().map(|n| indexed_member(root, n)).collect();

    // Indexing above ran on each member's own private pool and dropped it; the
    // registry's shared pool is the only one from here on.
    assert!(
        wait_until(|| live_worker_threads() == 0),
        "the fixture's indexing pools left {} worker(s) behind",
        live_worker_threads()
    );

    let budget = WorkspaceBudget::from_limits(ROOMY_FD_LIMIT, BUDGET_CORES);
    assert_eq!(
        budget.worker_threads(),
        BUDGET_CORES,
        "the shared pool must be sized by the host's cores"
    );
    assert!(
        budget.max_resident_members() >= MEMBERS.len(),
        "the fixture needs every member resident at once; the budget holds {}",
        budget.max_resident_members()
    );

    let registry = EngineRegistry::<Engine>::with_budget(
        Federation {
            name: "workspace".to_string(),
            root: root.to_path_buf(),
            members,
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
        },
        RegistryMode::Lazy,
        budget,
    );

    // ── 1. one pool, sized by the budget ─────────────────────────────────
    let engines: Vec<Arc<Engine>> = MEMBERS
        .iter()
        .map(|m| registry.engine_for(m).expect("member engine starts"))
        .collect();
    for other in &engines[1..] {
        assert!(
            runtime(&engines[0]).shares_worker_pool_with(runtime(other)),
            "two member engines were given different worker pools; the thread \
             cost is still a multiple of the member count"
        );
    }
    assert_eq!(
        registry.shared_worker_threads(),
        budget.worker_threads(),
        "the resident set is not running on one budgeted pool"
    );
    assert!(
        wait_until(|| live_worker_threads() == budget.worker_threads()),
        "{} rayon workers are live for {} member engines; the budget is {} \
         (private pools would have cost {})",
        live_worker_threads(),
        MEMBERS.len(),
        budget.worker_threads(),
        MEMBERS.len() * budget.worker_threads(),
    );

    // Extraction runs on the shared pool too, and produces the same graph a
    // private pool would: `index` fans its tree-sitter passes out across
    // `worker_pool()`, so a shared pool that ran those jobs wrongly — or not at
    // all — shows up here as a missing symbol rather than as a thread count.
    let alpha_root = &registry.members()[0].root;
    std::fs::write(
        alpha_root.join("src").join("lib.rs"),
        format!("{}pub fn alpha_added() -> u32 {{ 3 }}\n", source(MEMBERS[0])),
    )
    .expect("append a symbol");
    engines[0].index();
    let after_index =
        serde_json::to_string(&engines[0].search("alpha_added", None, None)).expect("json");
    assert!(
        after_index.contains("alpha_added"),
        "extraction on the shared worker pool did not produce the new symbol: \
         {after_index}"
    );

    // ── 2. a long job on every worker starves no other member ────────────
    //
    // `spawn` (not `install`) so the jobs are queued without blocking this
    // thread; one per worker, so the pool is genuinely saturated rather than
    // merely busy.
    let latch = Arc::new(Latch::default());
    let occupied = Arc::new(AtomicUsize::new(0));
    for _ in 0..budget.worker_threads() {
        let latch = Arc::clone(&latch);
        let occupied = Arc::clone(&occupied);
        runtime(&engines[0]).worker_pool().spawn(move || {
            occupied.fetch_add(1, Ordering::SeqCst);
            latch.wait();
        });
    }
    assert!(
        wait_until(|| occupied.load(Ordering::SeqCst) == budget.worker_threads()),
        "only {} of {} workers took a long job; the pool was never saturated, so \
         the latency assertion below would prove nothing",
        occupied.load(Ordering::SeqCst),
        budget.worker_threads(),
    );

    // With every worker of the shared pool blocked, another member's point query
    // must still answer inside the [NFR-PE-01] budget — navigation reads on the
    // calling thread through the read pool ([ADR-11]) and never enters the worker
    // pool at all. That is the whole reason a shared pool cannot starve a query,
    // and it is asserted here rather than assumed: the query runs on its own
    // thread behind a timeout, so if a future change ever routed navigation
    // through `worker_pool().install(…)` this FAILS instead of hanging.
    // Each result carries its own member name: the channel delivers in
    // completion order, not in the order the queries were spawned, so zipping
    // arrivals against `MEMBERS` would compare one member's answer with
    // another's name.
    let (query_tx, query_rx) = std::sync::mpsc::channel::<(&str, String, u128)>();
    let mut latencies_ms: Vec<u128> = Vec::new();
    std::thread::scope(|scope| {
        for (member, engine) in MEMBERS.iter().zip(engines.iter()).skip(1) {
            let query_tx = query_tx.clone();
            scope.spawn(move || {
                let started = Instant::now();
                let answer =
                    serde_json::to_string(&engine.search("entry", None, None)).expect("json");
                let _ = query_tx.send((member, answer, started.elapsed().as_millis()));
            });
        }
        drop(query_tx);

        for _ in MEMBERS.iter().skip(1) {
            let (member, answer, elapsed_ms) = query_rx.recv_timeout(TIMEOUT).expect(
                "a member's navigation query did not return while another member \
                 occupied every worker of the shared pool — navigation is being \
                 routed through the worker pool, which is the coupling ADR-63 warns \
                 about",
            );
            assert!(
                answer.contains(&format!("{member}_entry")),
                "member {member} answered without its own symbols while the shared \
                 pool was saturated: {answer}"
            );
            latencies_ms.push(elapsed_ms);
        }
    });

    let worst = latencies_ms.iter().copied().max().unwrap_or_default();
    eprintln!(
        "shared pool saturated ({} workers busy): per-member search {latencies_ms:?} ms",
        budget.worker_threads(),
    );
    assert!(
        worst < POINT_QUERY_MS,
        "a member's search took {worst} ms while another member occupied every \
         worker of the shared pool — that is the [NFR-PE-01] starvation [ADR-63] \
         trades private pools against; distribution: {latencies_ms:?}"
    );

    // A genuine WORKER-POOL job from another member, submitted while the pool is
    // still saturated, must queue and then run — not deadlock. Submitting it
    // after the release (as an earlier draft did) would only have proved that a
    // drained pool accepts work.
    let (job_tx, job_rx) = std::sync::mpsc::channel::<u64>();
    let third = Arc::clone(&engines[2]);
    let job = std::thread::spawn(move || {
        let value = third
            .runtime()
            .expect("a started member engine owns a runtime")
            .worker_pool()
            .install(|| 11_u64);
        let _ = job_tx.send(value);
    });
    assert!(
        job_rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "a third member's job ran while every worker was occupied — the members \
         are not sharing one pool"
    );

    latch.release();
    assert_eq!(
        job_rx
            .recv_timeout(TIMEOUT)
            .expect("the queued job must run once the long jobs release"),
        11
    );
    job.join().expect("the job thread joins");

    // ── 3. eviction and reconstruction rejoin the same pool ──────────────
    let baseline = serde_json::to_string(&engines[0].search("entry", None, None)).expect("json");
    drop(engines); // release the callers' Arcs so LRU may evict
    let before = registry.reconstructions();
    registry.evict_to_capacity(1);
    assert!(
        !registry.resident_members().contains(&MEMBERS[0].to_string()),
        "{} was expected to be evicted; residency = {:?}",
        MEMBERS[0],
        registry.resident_members()
    );

    let rebuilt = registry.engine_for(MEMBERS[0]).expect("member engine starts");
    assert!(
        registry.reconstructions() > before,
        "nothing was rebuilt, so the identity assertion below proves nothing"
    );
    let survivor = registry
        .engine_for(MEMBERS[2])
        .expect("member engine starts");
    assert!(
        runtime(&rebuilt).shares_worker_pool_with(runtime(&survivor)),
        "reconstruction built a SECOND worker pool instead of rejoining the \
         workspace's"
    );
    assert_eq!(
        registry.shared_worker_threads(),
        budget.worker_threads(),
        "the thread count moved when a member was rebuilt"
    );
    assert_eq!(
        serde_json::to_string(&rebuilt.search("entry", None, None)).expect("json"),
        baseline,
        "the reconstructed engine answered differently on the shared pool"
    );

    // ── 4. teardown leaves no orphan threads ─────────────────────────────
    drop(rebuilt);
    drop(survivor);
    drop(registry);
    assert!(
        wait_until(|| live_worker_threads() == 0),
        "{} rayon worker(s) outlived the workspace that built them",
        live_worker_threads()
    );
}
