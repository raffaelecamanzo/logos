//! **The binding criterion of [S-429]**: loading the workspace Statistics
//! aggregate leaves the resident-engine count exactly where a
//! [`workspace_status`] over the same workspace left it — asserted on
//! **connection count**, in the manner of `workspace_connection_budget.rs`
//! ([FR-UI-37], [NFR-PE-10], [NFR-PE-11]).
//!
//! The story it guards is a cost, not an output. Telemetry lives in a store
//! separate from the graph ([ADR-13]), but its only public accessor is
//! `Engine::stats(window)` — so the obvious fan-out constructs one engine per
//! member on a single view load and undoes the warm-only-the-default policy the
//! federated serve exists to keep. A view that produced correct figures at the
//! cost of N engines does not satisfy this story, so the assertion here is an
//! **equality on registry counters across the aggregate call**, not a bound on
//! the figures it returns.
//!
//! # Why the counters are read as a delta, not as an absolute
//! `workspace_status` legitimately builds every member (it reads each one's
//! index freshness and the cross-service coverage). The claim is not that the
//! workspace is cheap; it is that *this* read adds nothing to it. So the
//! measurement is: walk `workspace_status` first, snapshot
//! `engine_starts` / `reconstructions` / `resident_count` /
//! `live_read_connections`, run the aggregate, and require every one of them to
//! be unchanged. A fan-out through `Engine::stats` moves `engine_starts` on the
//! very first member and fails here.
//!
//! # Why this file is separate from `workspace_connection_budget.rs`
//! That binary lowers this **process's** `RLIMIT_NOFILE`, which is process-wide
//! and shared by every `#[test]` thread cargo runs in it — its own module docs
//! say it therefore holds exactly one test. This one must not be added there,
//! and does not need to be: it derives its budget from an explicit 256-descriptor
//! limit ([`WorkspaceBudget::from_limits`]) without installing one, so it
//! exercises eviction at the same arithmetic while leaving the process's real
//! descriptor table alone.
//!
//! [S-429]: ../../docs/planning/sprints/sprint-74.md
//! [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
//! [FR-UI-37]: ../../docs/specs/requirements/FR-UI-37.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md

use std::path::Path;

use logos_core::federation::{
    workspace_statistics, workspace_status, EngineRegistry, Federation, Member, RegistryMode,
    UnreadReason, WorkspaceBudget,
};
use logos_core::Engine;

/// Members in the fixture. Large enough that the budget below holds fewer than
/// all of them resident — so the aggregate runs against a registry that is
/// actively evicting, the state in which a stray engine construction is easiest
/// to hide — and small enough to stay a fast test.
const MEMBERS: usize = 16;

/// The descriptor limit the budget is *derived for*. Not installed: see the
/// module docs. It is the stock macOS soft limit, the same figure
/// `workspace_connection_budget.rs` derives its budget from, so both files
/// exercise the same arithmetic.
const DERIVED_FOR_SOFT_LIMIT: u64 = 256;

fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// A minimal member repo: one tiny source file, no index. The budget is a
/// property of residency, so the fixture needs many members, not large ones.
fn member_repo(root: &Path, name: &str) -> Member {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).expect("member dir");
    std::fs::write(repo.join("lib.rs"), "pub fn f() {}\n").expect("member source");
    Member {
        name: name.to_string(),
        root: repo,
    }
}

/// Every registry counter the resident-engine claim is measured on, read
/// together so the snapshot is one consistent observation rather than four.
#[derive(Debug, PartialEq, Eq)]
struct Cost {
    engine_starts: u64,
    reconstructions: u64,
    resident_members: usize,
    live_read_connections: usize,
}

fn cost(registry: &EngineRegistry<Engine>) -> Cost {
    Cost {
        engine_starts: registry.engine_starts(),
        reconstructions: registry.reconstructions(),
        resident_members: registry.resident_count(),
        live_read_connections: registry.live_read_connections(),
    }
}

/// [FR-UI-37] acceptance: the aggregate costs **nothing** on top of what a
/// `workspace status` over the same workspace already paid, and names every
/// member it could not read.
#[test]
fn the_statistics_aggregate_adds_no_engine_and_no_connection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let members: Vec<Member> = (0..MEMBERS)
        .map(|i| member_repo(root, &format!("svc{i:03}")))
        .collect();

    // One member's store is present and is not a database: the `unreadable`
    // reason must survive the real fan-out, not only the unit fixture. The
    // remaining members have never run telemetry, so they read `absent`.
    let broken = root.join("svc000").join(".logos");
    std::fs::create_dir_all(&broken).expect("member .logos");
    std::fs::write(broken.join("telemetry.db"), b"not a database").expect("broken store");

    let budget = WorkspaceBudget::from_limits(DERIVED_FOR_SOFT_LIMIT, cores());
    assert!(
        budget.max_resident_members() < MEMBERS,
        "the fixture must actually exercise eviction: {MEMBERS} members against a \
         budget of {}",
        budget.max_resident_members(),
    );
    let federation = Federation {
        name: "workspace".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    };
    let registry = EngineRegistry::<Engine>::with_budget(federation, RegistryMode::Lazy, budget);

    // What a `workspace status` over this workspace pays. This is the baseline
    // the acceptance criterion names — not zero.
    let status = workspace_status(&registry);
    assert_eq!(status.members.len(), MEMBERS, "every member is reported");
    let before = cost(&registry);
    assert!(
        before.engine_starts >= MEMBERS as u64,
        "the baseline walk really did build the members it is being compared \
         against: {before:?}"
    );

    let agg = workspace_statistics(&registry, None);

    // ── The binding criterion ────────────────────────────────────────────────
    let after = cost(&registry);
    assert_eq!(
        after, before,
        "loading the aggregate moved the workspace's engine cost; a fan-out \
         through `Engine::stats` would show here as {} extra engine start(s) \
         ([NFR-PE-10])",
        after.engine_starts.saturating_sub(before.engine_starts),
    );

    // ── And it still answered, over the full roster ──────────────────────────
    assert_eq!(agg.members_total, MEMBERS as u64);
    assert_eq!(
        agg.members_read, 0,
        "no member of this fixture has a readable store: {:?}",
        agg.unread
    );
    assert!(!agg.covers_all_members);
    assert_eq!(
        agg.unread.len(),
        MEMBERS,
        "every member that contributed nothing is named ([NFR-CC-04])"
    );
    let unreadable: Vec<&str> = agg
        .unread
        .iter()
        .filter(|u| u.reason == UnreadReason::Unreadable)
        .map(|u| u.member.as_str())
        .collect();
    assert_eq!(
        unreadable,
        ["svc000"],
        "the non-database store is named `unreadable`, apart from the {} members \
         that simply never ran telemetry",
        MEMBERS - 1,
    );
    assert!(
        agg.unread
            .iter()
            .filter(|u| u.member != "svc000")
            .all(|u| u.reason == UnreadReason::Absent),
        "a member with no store is `absent`, not a fault: {:?}",
        agg.unread
    );

    eprintln!(
        "statistics aggregate over {MEMBERS} members: {:?} before, {:?} after",
        before, after
    );

    // A second read is idempotent on cost too — the property must hold per call,
    // not merely on the first one after a walk that had already built everything.
    let _ = workspace_statistics(&registry, Some(30));
    assert_eq!(cost(&registry), before, "a second aggregate read costs nothing either");
}
