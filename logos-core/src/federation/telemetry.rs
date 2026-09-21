//! The workspace **telemetry aggregate** — every member's usage summed over one
//! window, at the cost of N small store reads and **no** member engine
//! ([FR-UI-37], [FR-OB-04], [NFR-PE-10], [NFR-CC-04]).
//!
//! # The constraint is the whole read-model ([CR-137] CRA-01)
//! Telemetry lives in `.logos/telemetry.db`, a store separate from the graph
//! ([ADR-13]) — but its only *public* accessor is
//! [`Engine::stats`](crate::Engine::stats), so the obvious fan-out would
//! construct one engine per member on a single view load and undo the
//! warm-only-the-default policy the federated serve exists to keep. The
//! enabling assumption behind this module is that the store is reachable
//! without an engine; it is, and
//! `observability::read_stats` is where that
//! is stated and proven. Everything here rides that one free function over a
//! `&Path`.
//!
//! This is the same posture as the two engine-free reads beside it: the
//! manifest-only roster ([`super::query::workspace_roster`], [FR-WS-06]) and the
//! warm-outcome sidecar read ([FR-WS-17]). Like the roster, it takes the
//! registry only to reach [`EngineRegistry::federation`] — the member set — and
//! touches no engine at all.
//!
//! # It states the population it summed over ([NFR-CC-04])
//! A total over "the workspace" that quietly covered three of five members is
//! the unlabelled figure this project refuses everywhere else. So
//! [`WorkspaceStatistics`] carries its denominator
//! ([`members_read`](WorkspaceStatistics::members_read) of
//! [`members_total`](WorkspaceStatistics::members_total)) and **names** every
//! member it could not read, with the reason —
//! [absent](UnreadReason::Absent), [locked](UnreadReason::Locked) or
//! [unreadable](UnreadReason::Unreadable). A member whose store could not be
//! read contributes nothing and is never imputed ([NFR-RA-05]).
//!
//! # What it deliberately does not carry
//! - **Latency percentiles.** `p50`/`p95`/`p99` are distribution statistics;
//!   summing or averaging them across members would be a fabricated figure, not
//!   a coarser one. They are absent rather than wrong ([NFR-CC-04]).
//! - **Artifact binding counts.** A live-*graph* property that
//!   [`Engine::stats`](crate::Engine::stats) merges from `logos.db`; reaching it
//!   is exactly what would need an engine.
//! - **Any quality signal.** This aggregates usage, not quality: no per-member
//!   gate verdict, score or violation count is rolled up here ([BR-56],
//!   [ADR-56]).
//!
//! [CR-137]: ../../../docs/requests/CR-137-a-view-declares-the-scope-it-answers-for.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md
//! [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
//! [BR-56]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
//! [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
//! [FR-WS-06]: ../../../docs/specs/requirements/FR-WS-06.md
//! [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::BTreeMap;

use serde::Serialize;

use super::registry::{EngineRegistry, MemberEngine};
use crate::models::quality::{
    AttributionCoverage, DailyActivity, OriginUsage, StatsInfo, ToolUsage,
};
use crate::observability::{attribution_coverage, read_stats, DEFAULT_STATS_WINDOW_DAYS};

/// Why one member contributed nothing to the aggregate ([FR-UI-37],
/// [NFR-CC-04]).
///
/// Three states, not a boolean, because they call for different reactions: an
/// [absent](Self::Absent) store is the normal state of a member nobody has run
/// Logos in yet, a [locked](Self::Locked) one is transient and worth retrying,
/// and an [unreadable](Self::Unreadable) one is a fault someone must look at.
/// Collapsing them to "unavailable" would report the first as an incident and
/// the last as routine.
///
/// [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnreadReason {
    /// No `telemetry.db` at the member's resolved store directory — telemetry
    /// has never run there. Not a fault.
    Absent,
    /// The store is there but a concurrent writer holds it past the read's busy
    /// timeout. Transient.
    Locked,
    /// The store is there and could not be opened or queried — corrupt,
    /// permission-denied, or not a database.
    Unreadable,
}

/// One member whose telemetry could not be read, **named** with its reason
/// ([FR-UI-37], [NFR-RA-05]).
///
/// [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnreadMember {
    /// The member's repo-qualified name ([`Member::name`](super::Member::name)).
    pub member: String,
    /// Which of the three states this is.
    pub reason: UnreadReason,
    /// The diagnostic as the reader would want it — the error chain for a
    /// locked or unreadable store, a fixed sentence for an absent one. Present
    /// for every row so a surface never has to synthesise wording for one of
    /// the three cases ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub detail: String,
}

/// Usage across the whole workspace over one trailing window — the app-scoped
/// Statistics read-model ([FR-UI-37]).
///
/// Every figure below is a **sum over [`members_read`](Self::members_read)
/// members**, not over the roster, and the two are not the same number whenever
/// [`unread`](Self::unread) is non-empty. See the module docs for what this
/// deliberately does not carry.
///
/// # The awaiting-data state
/// `members_read == 0` means no member has a readable telemetry store, so there
/// is nothing to sum and no denominator to state. A consumer renders the honest
/// awaiting-data state there — never the zeros below, which in every other case
/// are genuine measurements ([NFR-CC-04]). The distinction is exactly
/// `members_read`: a member whose store exists and recorded nothing *is* read,
/// and contributes real zeros.
///
/// [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Serialize)]
pub struct WorkspaceStatistics {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// The trailing window every member was read over, in days ([FR-OB-04]
    /// default 7).
    pub window_days: u32,
    /// Members on the roster — the population the aggregate *could* have
    /// covered.
    pub members_total: u64,
    /// Members whose telemetry was actually read — the denominator every figure
    /// below is a sum over.
    pub members_read: u64,
    /// `members_read == members_total`. Published rather than left to the
    /// consumer's arithmetic, so the marker and the figures it governs cannot
    /// be read apart (the [`WorkspaceStatus`](super::WorkspaceStatus) posture).
    pub covers_all_members: bool,
    /// Every member that contributed nothing, named with why, in roster order.
    pub unread: Vec<UnreadMember>,
    /// Total calls across the read members.
    pub calls_total: u64,
    /// Per-`(surface, tool)` usage summed across the read members, sorted by
    /// surface then tool — the "top tools" source.
    pub calls_by_tool: Vec<ToolUsage>,
    /// Per-UTC-day activity summed across the read members, oldest day first.
    pub activity_by_day: Vec<DailyActivity>,
    /// The dev-vs-`main` split summed across the read members, `"dev"` first.
    /// Inherits [`attribution_coverage`](Self::attribution_coverage)'s
    /// raw-events-only limit from every member it sums.
    pub calls_by_origin: Vec<OriginUsage>,
    /// Estimated ad-hoc file reads avoided, summed — an estimate, honestly
    /// labeled ([NFR-CC-04]).
    pub reads_saved_estimate: u64,
    /// The headline value estimate, summed ([NFR-OO-03]).
    ///
    /// [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
    pub tokens_saved_estimate: u64,
    /// What the origin split covers, stated in the payload exactly as the
    /// member-scoped read-model states it. Structural — a function of the
    /// requested window and `daily_rollup`'s schema, identical for every member
    /// — so it is computed once here rather than merged from N copies that
    /// could only ever agree.
    pub attribution_coverage: AttributionCoverage,
}

/// Aggregate every member's telemetry over one window, **constructing no member
/// engine** ([FR-UI-37], [NFR-PE-10]).
///
/// Generic over [`MemberEngine`] for the same reason
/// [`workspace_roster`](super::query::workspace_roster) is: the engine type is
/// never named in the body, so the signature states the engine-free claim
/// rather than merely honouring it, and a spy registry can assert the
/// zero-construction property directly.
///
/// The fan-out walks [`EngineRegistry::federation`]'s member set and reads each
/// member's store through
/// `observability::read_stats`, which
/// resolves that member's own store by the ordinary per-repository rule —
/// including a linked worktree's write-through to its primary checkout
/// ([ADR-50]). That rule aggregates nothing; this fan-out is over *members*,
/// each resolving its own store by it.
///
/// Infallible at the surface: a member that cannot be read is named in
/// [`WorkspaceStatistics::unread`] rather than failing the whole answer
/// ([NFR-RA-05], the [ADR-14] posture).
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
/// [ADR-50]: ../../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub fn workspace_statistics<E>(
    registry: &EngineRegistry<E>,
    window_days: Option<u32>,
) -> WorkspaceStatistics
where
    E: MemberEngine,
{
    let window_days = window_days.unwrap_or(DEFAULT_STATS_WINDOW_DAYS);
    let federation = registry.federation();

    let mut unread = Vec::new();
    let mut read: Vec<StatsInfo> = Vec::new();
    for member in &federation.members {
        match read_stats(&member.root, window_days) {
            Ok(Some(info)) => read.push(info),
            Ok(None) => unread.push(UnreadMember {
                member: member.name.clone(),
                reason: UnreadReason::Absent,
                detail: "no telemetry recorded yet (telemetry.db not found)".to_string(),
            }),
            Err(err) => unread.push(UnreadMember {
                member: member.name.clone(),
                reason: classify(&err),
                detail: format!("{err:#}"),
            }),
        }
    }

    let members_total = federation.members.len() as u64;
    let members_read = read.len() as u64;
    WorkspaceStatistics {
        workspace: federation.name.clone(),
        window_days,
        members_total,
        members_read,
        covers_all_members: members_read == members_total,
        unread,
        calls_total: read.iter().map(|s| s.calls_total).sum(),
        calls_by_tool: sum_by_tool(&read),
        activity_by_day: sum_by_day(&read),
        calls_by_origin: sum_by_origin(&read),
        reads_saved_estimate: read.iter().map(|s| s.reads_saved_estimate).sum(),
        tokens_saved_estimate: read.iter().map(|s| s.tokens_saved_estimate).sum(),
        attribution_coverage: attribution_coverage(window_days),
    }
}

/// Which of the three [`UnreadReason`]s a failed read was.
///
/// Walks the whole `anyhow` chain rather than inspecting only the outermost
/// error, because `read_stats` wraps the
/// `rusqlite` failure in its own context — the SQLite code that distinguishes a
/// transient lock from a real fault is always a *source*, never the top.
fn classify(err: &anyhow::Error) -> UnreadReason {
    for cause in err.chain() {
        if let Some(rusqlite::Error::SqliteFailure(e, _)) = cause.downcast_ref::<rusqlite::Error>()
        {
            if matches!(
                e.code,
                rusqlite::ffi::ErrorCode::DatabaseBusy | rusqlite::ffi::ErrorCode::DatabaseLocked
            ) {
                return UnreadReason::Locked;
            }
        }
    }
    UnreadReason::Unreadable
}

/// Sum per-`(surface, tool)` usage across the read members.
///
/// Keyed by a [`BTreeMap`] so the output order — surface, then tool — is the
/// same one a single member's `calls_by_tool` already carries, rather than a
/// second ordering rule the consumer would have to learn ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn sum_by_tool(read: &[StatsInfo]) -> Vec<ToolUsage> {
    // `class` is a pure function of the tool name (`tool::class_of_wire`), so
    // every member reporting the same tool reports the same class; the first
    // one encountered is therefore not a choice between disagreeing labels.
    let mut by_tool: BTreeMap<(&str, &str), (String, u64, u64)> = BTreeMap::new();
    for info in read {
        for usage in &info.calls_by_tool {
            let entry = by_tool
                .entry((usage.surface.as_str(), usage.tool.as_str()))
                .or_insert_with(|| (usage.class.clone(), 0, 0));
            entry.1 += usage.calls;
            entry.2 += usage.ok_calls;
        }
    }
    by_tool
        .into_iter()
        .map(|((surface, tool), (class, calls, ok_calls))| ToolUsage {
            surface: surface.to_string(),
            tool: tool.to_string(),
            class,
            calls,
            ok_calls,
        })
        .collect()
}

/// Sum per-UTC-day activity across the read members, oldest day first.
///
/// `'YYYY-MM-DD'` sorts lexicographically into calendar order, so the
/// [`BTreeMap`] key gives the oldest-first contract for free.
fn sum_by_day(read: &[StatsInfo]) -> Vec<DailyActivity> {
    let mut by_day: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for info in read {
        for day in &info.activity_by_day {
            let entry = by_day.entry(day.day.as_str()).or_default();
            entry.0 += day.calls;
            entry.1 += day.ok_calls;
        }
    }
    by_day
        .into_iter()
        .map(|(day, (calls, ok_calls))| DailyActivity {
            day: day.to_string(),
            calls,
            ok_calls,
        })
        .collect()
}

/// Sum the dev-vs-`main` split across the read members, `"dev"` before
/// `"main"` — the order a single member's `calls_by_origin` already uses, which
/// a [`BTreeMap`] over the two bucket names reproduces.
fn sum_by_origin(read: &[StatsInfo]) -> Vec<OriginUsage> {
    let mut by_origin: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for info in read {
        for origin in &info.calls_by_origin {
            let entry = by_origin.entry(origin.origin.as_str()).or_default();
            entry.0 += origin.calls;
            entry.1 += origin.ok_calls;
        }
    }
    by_origin
        .into_iter()
        .map(|(origin, (calls, ok_calls))| OriginUsage {
            origin: origin.to_string(),
            calls,
            ok_calls,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use anyhow::Result;

    use super::*;
    use crate::federation::registry::RegistryMode;
    use crate::federation::{Federation, Member};
    use crate::observability::seed_store_for_tests;
    use crate::SharedWorkerPool;

    /// A member engine that does nothing, standing in for a real
    /// [`Engine`](crate::Engine) so the fan-out runs against a registry with no
    /// on-disk store behind it — the `FakeEngine` shape `coverage.rs` already
    /// uses for the same purpose.
    ///
    /// It deliberately carries **no construction counter of its own**. The
    /// registry already keeps one
    /// ([`EngineRegistry::engine_starts`](super::EngineRegistry::engine_starts)),
    /// it is the instrument `tests/workspace_connection_budget.rs` asserts this
    /// project's engine-cost claims on, and unlike a spy-side counter it
    /// observes a construction made on any thread — so a fan-out someone later
    /// parallelises cannot read as zero.
    #[derive(Debug)]
    struct SpyEngine;
    struct SpyWatcher;

    impl MemberEngine for SpyEngine {
        type Watcher = SpyWatcher;

        fn start(
            _root: &Path,
            _read_connections: usize,
            _worker_pool: SharedWorkerPool,
        ) -> Result<Arc<Self>> {
            Ok(Arc::new(SpyEngine))
        }

        fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
            Ok(SpyWatcher)
        }
    }

    /// Unix seconds "now", so seeded events land inside the default window.
    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs() as i64
    }

    /// A workspace of `names` members rooted at `root`, with no store seeded.
    fn federation(root: &Path, names: &[&str]) -> Federation {
        Federation {
            name: "shop".to_string(),
            root: root.to_path_buf(),
            members: names
                .iter()
                .map(|n| {
                    let repo = root.join(n);
                    std::fs::create_dir_all(&repo).expect("member dir");
                    Member {
                        name: (*n).to_string(),
                        root: repo,
                    }
                })
                .collect(),
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
        }
    }

    /// `<root>/<member>/.logos`, the directory a member's telemetry resolves to
    /// when it is a plain (non-worktree) checkout.
    fn logos_dir(root: &Path, member: &str) -> PathBuf {
        root.join(member).join(".logos")
    }

    /// Two members with telemetry and one that cannot be read: the aggregate
    /// sums exactly the two, says so, and names the third with its reason
    /// ([FR-UI-37], [NFR-CC-04]).
    #[test]
    fn sums_the_readable_members_states_the_denominator_and_names_the_rest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["api", "web", "billing"]);

        let at = now() - 3_600;
        seed_store_for_tests(
            &logos_dir(root, "api"),
            &[
                ("cli", "search", 10, true, at, "main"),
                ("cli", "search", 12, true, at, "dev-branch"),
                ("cli", "context", 20, true, at, "main"),
            ],
        )
        .expect("api store seeds");
        seed_store_for_tests(
            &logos_dir(root, "web"),
            &[("cli", "search", 8, true, at, "main")],
        )
        .expect("web store seeds");
        // `billing` gets a file that is not a database: present, so not absent,
        // and unopenable-as-SQL, so genuinely unreadable.
        std::fs::create_dir_all(logos_dir(root, "billing")).unwrap();
        std::fs::write(logos_dir(root, "billing").join("telemetry.db"), b"not a database").unwrap();

        let registry = EngineRegistry::<SpyEngine>::new(fed, RegistryMode::Lazy);
        let agg = workspace_statistics(&registry, None);

        assert_eq!(
            (agg.members_read, agg.members_total),
            (2, 3),
            "the aggregate states the population it summed over, not the roster"
        );
        assert!(!agg.covers_all_members);
        assert_eq!(agg.unread.len(), 1, "{:?}", agg.unread);
        assert_eq!(agg.unread[0].member, "billing");
        assert_eq!(agg.unread[0].reason, UnreadReason::Unreadable);
        assert!(
            !agg.unread[0].detail.is_empty(),
            "an unread member carries the diagnostic, not just a label"
        );

        // 4 calls across the two readable members — the third contributes
        // nothing rather than being imputed ([NFR-RA-05]).
        assert_eq!(agg.calls_total, 4);
        let searches: u64 = agg
            .calls_by_tool
            .iter()
            .filter(|u| u.tool == "search")
            .map(|u| u.calls)
            .sum();
        assert_eq!(searches, 3, "search is summed ACROSS members: {:?}", agg.calls_by_tool);
        assert!(
            agg.calls_by_tool.iter().any(|u| u.tool == "context" && u.calls == 1),
            "a tool only one member reports still appears: {:?}",
            agg.calls_by_tool
        );
        // The dev-vs-main split sums across members and keeps "dev" first.
        let origins: Vec<(&str, u64)> = agg
            .calls_by_origin
            .iter()
            .map(|o| (o.origin.as_str(), o.calls))
            .collect();
        assert_eq!(origins, [("dev", 1), ("main", 3)], "{:?}", agg.calls_by_origin);
        assert_eq!(agg.window_days, DEFAULT_STATS_WINDOW_DAYS);
        assert_eq!(agg.workspace, "shop");

        // The binding criterion, on the instrument that moves: reading real,
        // populated member stores constructed no engine ([NFR-PE-10]).
        assert_eq!(
            registry.engine_starts(),
            0,
            "the telemetry fan-out constructed a member engine"
        );
        assert_eq!(registry.resident_count(), 0);
    }

    /// A workspace where no member has telemetry reports `members_read == 0` —
    /// the awaiting-data signal — and names every member absent rather than
    /// summing zeros over a roster it never read ([NFR-CC-04]).
    #[test]
    fn a_workspace_with_no_telemetry_reads_zero_members_and_names_them_all() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let fed = federation(tmp.path(), &["api", "web"]);
        let registry = EngineRegistry::<SpyEngine>::new(fed, RegistryMode::Lazy);

        let agg = workspace_statistics(&registry, None);

        assert_eq!(agg.members_read, 0, "nothing was read, so nothing was summed");
        assert_eq!(agg.members_total, 2);
        assert!(!agg.covers_all_members);
        let named: Vec<(&str, UnreadReason)> = agg
            .unread
            .iter()
            .map(|u| (u.member.as_str(), u.reason))
            .collect();
        assert_eq!(
            named,
            [("api", UnreadReason::Absent), ("web", UnreadReason::Absent)],
            "every member is named, in roster order, with the absent reason"
        );
        assert_eq!(agg.calls_total, 0);
        assert!(agg.calls_by_tool.is_empty());
        assert_eq!(registry.engine_starts(), 0);
    }

    /// A store held by an exclusive writer past the read's busy timeout is named
    /// [`Locked`](UnreadReason::Locked), not `Unreadable` ([FR-UI-37]).
    ///
    /// This is the branch of [`classify`] that only a real SQLite lock reaches,
    /// so it is driven by taking one: without it the transient state and the
    /// genuine fault would be indistinguishable to a reader, which is the whole
    /// reason [`UnreadReason`] has three variants rather than two.
    #[test]
    fn a_store_held_by_an_exclusive_writer_is_named_locked_not_unreadable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["api"]);
        let dir = logos_dir(root, "api");
        seed_store_for_tests(&dir, &[("cli", "search", 5, true, now(), "main")])
            .expect("api store seeds");

        // Hold the store exclusively for the whole read. `open_readonly`'s busy
        // timeout is 250 ms, so the query below gives up and reports BUSY.
        let writer = rusqlite::Connection::open(dir.join("telemetry.db")).expect("writer opens");
        // `locking_mode = EXCLUSIVE`, not merely `BEGIN EXCLUSIVE`: the store is
        // WAL (`db::open`'s pragma contract), and WAL exists precisely so a
        // reader never blocks on a writer's transaction. Only an exclusive
        // *file* lock over the WAL index shuts a second connection out, and that
        // is what a reader then reports as BUSY.
        writer
            .execute_batch(
                "PRAGMA locking_mode = EXCLUSIVE;
                 BEGIN IMMEDIATE;
                 INSERT INTO schema_versions (version, applied_at) VALUES (999, 0);",
            )
            .expect("the exclusive lock is taken");

        let agg = workspace_statistics(&registry_of(fed), None);

        assert_eq!(agg.members_read, 0, "the locked member contributed nothing");
        assert_eq!(agg.unread.len(), 1, "{:?}", agg.unread);
        assert_eq!(
            agg.unread[0].reason,
            UnreadReason::Locked,
            "a transient lock must not be reported as a fault: {}",
            agg.unread[0].detail
        );
        drop(writer);
    }

    /// A member whose store exists and recorded **nothing** is READ, not
    /// unread: its zeros are genuine measurements, and conflating the two is
    /// exactly what would make the awaiting-data state fire over a live
    /// workspace ([NFR-CC-04]).
    #[test]
    fn an_empty_store_counts_as_read_not_as_unread() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["api"]);
        seed_store_for_tests(&logos_dir(root, "api"), &[]).expect("an empty store still migrates");

        let agg = workspace_statistics(&registry_of(fed), None);

        assert_eq!(agg.members_read, 1, "a store with no events was still read");
        assert!(agg.unread.is_empty(), "{:?}", agg.unread);
        assert!(agg.covers_all_members);
        assert_eq!(agg.calls_total, 0, "and its zero is a measurement");
    }

    fn registry_of(fed: Federation) -> EngineRegistry<SpyEngine> {
        EngineRegistry::<SpyEngine>::new(fed, RegistryMode::Lazy)
    }

    /// The window is honoured per member: an event older than the requested
    /// window is excluded from the aggregate exactly as it is from a single
    /// member's read-model.
    #[test]
    fn the_window_scopes_every_members_read() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["api"]);
        let recent = now() - 3_600;
        let ancient = now() - 30 * 86_400;
        seed_store_for_tests(
            &logos_dir(root, "api"),
            &[
                ("cli", "search", 10, true, recent, "main"),
                ("cli", "search", 10, true, ancient, "main"),
            ],
        )
        .expect("api store seeds");

        let week = workspace_statistics(&registry_of(federation_reuse(&fed)), Some(7));
        let quarter = workspace_statistics(&registry_of(federation_reuse(&fed)), Some(90));

        assert_eq!(week.calls_total, 1, "the 30-day-old call is outside a 7-day window");
        assert_eq!(quarter.calls_total, 2, "and inside a 90-day one");
        assert_eq!(week.window_days, 7);
        assert_eq!(quarter.window_days, 90);
        assert_eq!(week.attribution_coverage.requested_window_days, 7);
    }

    /// Clone a fixture federation — `EngineRegistry::new` takes ownership, and
    /// this test builds two registries over the same members.
    fn federation_reuse(fed: &Federation) -> Federation {
        Federation {
            name: fed.name.clone(),
            root: fed.root.clone(),
            members: fed.members.clone(),
            default: fed.default.clone(),
            links: fed.links.clone(),
            governance: fed.governance.clone(),
            warm_concurrency: fed.warm_concurrency,
        }
    }
}
