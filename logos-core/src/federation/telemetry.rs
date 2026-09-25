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
//! constructs no engine.
//!
//! "Constructs none" is the claim, deliberately, and not the stronger "touches
//! none": [NFR-PE-10] bounds *construction* cost, and construction is what the
//! registry's monotone `engine_starts` counter can witness. Reaching into an
//! already-resident engine would move no counter any test can read, so a
//! stronger sentence here would be prose no assertion stands behind.
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
//! - **The tool×origin cross-tab and the class×origin roll-up** ([FR-OB-11]).
//!   Unlike the percentiles these *would* sum cleanly; they are absent because
//!   [FR-UI-37] names four projections and neither is among them. If a later
//!   story wants them app-wide they are the same fold as [`sum_by_tool`].
//!
//! [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
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
use crate::models::outcome::OutcomeCounts;
use crate::models::quality::{
    AttributionCoverage, DailyActivity, OriginUsage, StatsInfo, ToolUsage,
};
use crate::observability::{
    attribution_coverage, read_stats, DEFAULT_STATS_WINDOW_DAYS, NO_TELEMETRY_YET,
};

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
/// # The awaiting-data state is `calls_total == 0` — the member-scoped predicate
/// [FR-UI-37] requires the workspace to render "the honest awaiting-data state
/// the member-scoped view already renders". That view has exactly one such
/// predicate, `isStatsEmpty(stats) => stats.calls_total === 0`
/// (`web/ui/src/views/statistics/statsModel.ts`), shared by its empty state and
/// by the sidebar's nav muting. The app-scoped twin is therefore
/// [`calls_total`](Self::calls_total)`== 0`, and **not**
/// [`members_read`](Self::members_read)`== 0`.
///
/// The two are not the same test, and the gap between them is an ordinary
/// workspace: every member has a migrated store, none recorded anything in the
/// window. There `members_read == members_total` while `calls_total == 0` — a
/// consumer keying on `members_read` would render a grid of zeros, which is
/// precisely the "zeros read as measurements" failure [NFR-CC-04] forbids.
/// `members_read == 0` implies `calls_total == 0`, so the member-scoped
/// predicate already subsumes it.
///
/// [`members_read`](Self::members_read) answers a different question — *how many
/// members is this a sum over* — and governs the denominator line, not the empty
/// state. Both are needed: a member whose store exists and recorded nothing *is*
/// read and belongs in the denominator, even though it contributes no calls.
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
/// never named in the body, and a spy registry can therefore assert the
/// zero-construction property directly, over real member stores. The genericity
/// is a testability seam, not a proof — `engine_for` is available for any
/// `E: MemberEngine`, so the signature does not by itself forbid a construction;
/// `engine_starts()` is what forbids it, and it is asserted in both this
/// module's tests and `tests/workspace_statistics_engine_free.rs`.
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
                detail: NO_TELEMETRY_YET.to_string(),
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
    //
    // Every count the cell carries is summed here — `ok_calls` and both outcome
    // counts ([FR-OB-14]) alike. A field this fold forgets is not dropped from
    // the payload: it is serialised as a zero, which is why each has a fixture
    // in which it differs from every other.
    //
    // [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    let mut by_tool: BTreeMap<(&str, &str), (String, u64, u64, OutcomeCounts)> = BTreeMap::new();
    for info in read {
        for usage in &info.calls_by_tool {
            let entry = by_tool
                .entry((usage.surface.as_str(), usage.tool.as_str()))
                .or_insert_with(|| (usage.class.clone(), 0, 0, OutcomeCounts::default()));
            entry.1 += usage.calls;
            entry.2 += usage.ok_calls;
            entry.3 += usage.outcomes;
        }
    }
    by_tool
        .into_iter()
        .map(|((surface, tool), (class, calls, ok_calls, outcomes))| ToolUsage {
            surface: surface.to_string(),
            tool: tool.to_string(),
            class,
            calls,
            ok_calls,
            outcomes,
        })
        .collect()
}

/// Sum per-UTC-day activity across the read members, oldest day first.
///
/// `'YYYY-MM-DD'` sorts lexicographically into calendar order, so the
/// [`BTreeMap`] key gives the oldest-first contract for free.
fn sum_by_day(read: &[StatsInfo]) -> Vec<DailyActivity> {
    let mut by_day: BTreeMap<&str, (u64, u64, OutcomeCounts)> = BTreeMap::new();
    for info in read {
        for day in &info.activity_by_day {
            let entry = by_day.entry(day.day.as_str()).or_default();
            entry.0 += day.calls;
            entry.1 += day.ok_calls;
            entry.2 += day.outcomes;
        }
    }
    by_day
        .into_iter()
        .map(|(day, (calls, ok_calls, outcomes))| DailyActivity {
            day: day.to_string(),
            calls,
            ok_calls,
            outcomes,
        })
        .collect()
}

/// Sum the dev-vs-`main` split across the read members, `"dev"` before
/// `"main"` — the order a single member's `calls_by_origin` already uses, which
/// a [`BTreeMap`] over the two bucket names reproduces.
fn sum_by_origin(read: &[StatsInfo]) -> Vec<OriginUsage> {
    let mut by_origin: BTreeMap<&str, (u64, u64, OutcomeCounts)> = BTreeMap::new();
    for info in read {
        for origin in &info.calls_by_origin {
            let entry = by_origin.entry(origin.origin.as_str()).or_default();
            entry.0 += origin.calls;
            entry.1 += origin.ok_calls;
            entry.2 += origin.outcomes;
        }
    }
    by_origin
        .into_iter()
        .map(|(origin, (calls, ok_calls, outcomes))| OriginUsage {
            origin: origin.to_string(),
            calls,
            ok_calls,
            outcomes,
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
                // The one FAILED call in the fixture: without it `ok_calls` is
                // byte-identical to `calls` everywhere and the merge could drop
                // or zero it undetected.
                ("cli", "context", 20, false, at, "main"),
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
        // Both halves of the pair, on a tool whose `ok_calls` is NON-zero. The
        // failed-`context` assertion below cannot carry this on its own: its
        // expected `ok_calls` is 0, so a merge that zeroed the field everywhere
        // would match it exactly — a proxy for the property, not the property.
        let (searches, searches_ok) = agg
            .calls_by_tool
            .iter()
            .filter(|u| u.tool == "search")
            .fold((0u64, 0u64), |(c, ok), u| (c + u.calls, ok + u.ok_calls));
        assert_eq!(
            (searches, searches_ok),
            (3, 3),
            "search is summed ACROSS members, ok_calls included: {:?}",
            agg.calls_by_tool
        );
        let context = agg
            .calls_by_tool
            .iter()
            .find(|u| u.tool == "context")
            .expect("a tool only one member reports still appears");
        // `(calls, ok_calls)` as a PAIR: the failed call must survive the merge
        // as a failure, not be rounded up into the total or zeroed out of it.
        assert_eq!(
            (context.calls, context.ok_calls),
            (1, 0),
            "the failed call is merged as failed: {:?}",
            agg.calls_by_tool
        );
        assert!(
            agg.calls_by_tool.iter().all(|u| !u.class.is_empty()),
            "every merged tool keeps its class — the label `sum_by_tool` carries \
             over from the first member that reported the tool: {:?}",
            agg.calls_by_tool
        );
        // The dev-vs-main split sums across members, keeps "dev" first, and
        // carries its own ok/total split.
        let origins: Vec<(&str, u64, u64)> = agg
            .calls_by_origin
            .iter()
            .map(|o| (o.origin.as_str(), o.calls, o.ok_calls))
            .collect();
        assert_eq!(
            origins,
            [("dev", 1, 1), ("main", 3, 2)],
            "{:?}",
            agg.calls_by_origin
        );
        // The daily series is merged across members too — all four events share
        // one timestamp, so two members collapse to one day carrying both.
        assert_eq!(
            agg.activity_by_day.len(),
            1,
            "one seeded instant is one day: {:?}",
            agg.activity_by_day
        );
        assert_eq!(
            (agg.activity_by_day[0].calls, agg.activity_by_day[0].ok_calls),
            (4, 3),
            "the day's calls are summed across BOTH members: {:?}",
            agg.activity_by_day
        );
        assert_eq!(
            agg.activity_by_day[0].calls, agg.calls_total,
            "the daily series and the headline count the same population"
        );
        // The estimates are summed per member, exactly. `search` is weighted 2
        // reads and `context` 5, at 1500 tokens an avoided read: api contributes
        // 2*2 + 1*5 = 9, web 1*2 = 2. Asserted as exact values rather than
        // "non-zero", so neither a dropped member nor a swap of the two fields
        // can survive.
        assert_eq!(agg.reads_saved_estimate, 11, "9 from api + 2 from web");
        assert_eq!(agg.tokens_saved_estimate, 11 * 1_500);
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

    /// A `telemetry.db` that is **present but not a regular file** is named
    /// `unreadable`, never `absent` ([FR-UI-37], [NFR-CC-04]).
    ///
    /// The distinction is the one a bare `is_file()` cannot draw. `is_file()`
    /// answers `false` for every `stat` outcome that is not a regular file —
    /// including a directory of that name, and including an unreadable parent —
    /// and the fan-out would then tell the user *"no telemetry recorded yet"*
    /// about a member that is in fact broken, under the one reason variant
    /// documented as "Not a fault". The sentence would also be false: the path
    /// exists.
    #[test]
    fn a_present_non_file_store_is_named_unreadable_not_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["api"]);
        // A DIRECTORY named `telemetry.db` — present, and not a store.
        std::fs::create_dir_all(logos_dir(root, "api").join("telemetry.db")).unwrap();

        let agg = workspace_statistics(&registry_of(fed), None);

        assert_eq!(agg.members_read, 0);
        assert_eq!(agg.unread.len(), 1, "{:?}", agg.unread);
        assert_eq!(
            agg.unread[0].reason,
            UnreadReason::Unreadable,
            "a present-but-broken store is a fault, not an empty one: {}",
            agg.unread[0].detail
        );
        assert!(
            !agg.unread[0].detail.contains("not found"),
            "the diagnostic must not claim the file is missing when it is there: {}",
            agg.unread[0].detail
        );
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

        let week = workspace_statistics(&registry_of(fed.clone()), Some(7));
        let quarter = workspace_statistics(&registry_of(fed.clone()), Some(90));

        assert_eq!(week.calls_total, 1, "the 30-day-old call is outside a 7-day window");
        assert_eq!(quarter.calls_total, 2, "and inside a 90-day one");
        assert_eq!(week.window_days, 7);
        assert_eq!(quarter.window_days, 90);
        // Asserted on the window that is NOT the default: 7 is
        // `DEFAULT_STATS_WINDOW_DAYS`, so a coverage block computed from the
        // default instead of the request would pass the 7-day check and ship a
        // payload reading `window_days: 90` beside `requested_window_days: 7` —
        // a document contradicting itself in two adjacent fields ([NFR-CC-04]).
        assert_eq!(quarter.attribution_coverage.requested_window_days, 90);
        assert_eq!(
            quarter.attribution_coverage.covered_window_days, 90,
            "90 days is inside the raw-retention horizon, so coverage is not truncated"
        );
        assert_eq!(week.attribution_coverage.requested_window_days, 7);
    }

    /// The workspace aggregate reports the outcome counts **summed**, never
    /// zeroed ([FR-OB-14] AC 4): the `ok_calls` trap one field later.
    ///
    /// Across the two members `precedent` has 5 calls, 4 ok, 3 classified and 2
    /// answered — four different numbers — and each member contributes to every
    /// one of them, so a fold that dropped a member's counts, zeroed a field, or
    /// copied one field into another cannot match. `search` is unclassified in
    /// both, so its merged cell must still name the absence.
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[test]
    fn the_aggregate_sums_answered_and_classified_as_their_own_figures() {
        use crate::models::outcome::{OutcomeCounts, OUTCOME_ABSENCE};
        use crate::observability::{seed_store_with_outcomes_for_tests, Outcome};

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["api", "web"]);
        let at = now() - 3_600;
        seed_store_with_outcomes_for_tests(
            &logos_dir(root, "api"),
            &[
                ("cli", "precedent", 5, true, at, "main", Some(Outcome::Answered)),
                ("cli", "precedent", 5, true, at, "main", Some(Outcome::Empty)),
                ("cli", "precedent", 5, true, at, "main", None),
                ("cli", "search", 5, true, at, "main", None),
            ],
        )
        .expect("api store seeds");
        seed_store_with_outcomes_for_tests(
            &logos_dir(root, "web"),
            &[
                ("cli", "precedent", 5, true, at, "main", Some(Outcome::Answered)),
                ("cli", "precedent", 5, false, at, "main", None),
            ],
        )
        .expect("web store seeds");

        let registry = EngineRegistry::<SpyEngine>::new(fed, RegistryMode::Lazy);
        let agg = workspace_statistics(&registry, None);
        assert_eq!((agg.members_read, agg.members_total), (2, 2));

        let summed = OutcomeCounts {
            answered_calls: 2,
            classified_calls: 3,
        };
        let precedent = agg
            .calls_by_tool
            .iter()
            .find(|u| u.tool == "precedent")
            .expect("precedent merged");
        assert_eq!(
            (precedent.calls, precedent.ok_calls, precedent.outcomes),
            (5, 4, summed),
            "calls, ok, classified and answered are four figures, merged as four"
        );
        let search = agg
            .calls_by_tool
            .iter()
            .find(|u| u.tool == "search")
            .expect("search merged");
        assert_eq!(search.outcomes.absence(), Some(OUTCOME_ABSENCE));

        // The day and origin series fold the same counts: 6 calls, 5 ok, of
        // which precedent's are the only classified ones.
        assert_eq!(agg.activity_by_day.len(), 1, "{:?}", agg.activity_by_day);
        let day = &agg.activity_by_day[0];
        assert_eq!((day.calls, day.ok_calls, day.outcomes), (6, 5, summed));
        let main = agg
            .calls_by_origin
            .iter()
            .find(|o| o.origin == "main")
            .expect("main bucket merged");
        assert_eq!((main.calls, main.ok_calls, main.outcomes), (6, 5, summed));
    }

    /// A member whose store is still at **v3** — no v4 process has opened it,
    /// and the fan-out's read-only open never migrates — is read, not named
    /// unreadable: its calls join the aggregate and it contributes nothing
    /// classified ([FR-OB-14]).
    ///
    /// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
    #[test]
    fn a_member_still_at_v3_is_read_and_contributes_no_outcome() {
        use crate::models::outcome::OutcomeCounts;
        use crate::observability::{
            seed_pre_outcome_store_for_tests, seed_store_with_outcomes_for_tests, Outcome,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let fed = federation(root, &["legacy", "current"]);
        let at = now() - 3_600;
        seed_pre_outcome_store_for_tests(
            &logos_dir(root, "legacy"),
            &[("precedent", true, at), ("precedent", true, at)],
        )
        .expect("v3 store seeds");
        seed_store_with_outcomes_for_tests(
            &logos_dir(root, "current"),
            &[("cli", "precedent", 5, true, at, "main", Some(Outcome::Answered))],
        )
        .expect("v4 store seeds");

        let registry = EngineRegistry::<SpyEngine>::new(fed, RegistryMode::Lazy);
        let agg = workspace_statistics(&registry, None);
        assert_eq!(
            (agg.members_read, agg.members_total),
            (2, 2),
            "an older store is not an unreadable one: {:?}",
            agg.unread
        );
        let precedent = agg
            .calls_by_tool
            .iter()
            .find(|u| u.tool == "precedent")
            .expect("precedent merged");
        assert_eq!(
            (precedent.calls, precedent.outcomes),
            (
                3,
                OutcomeCounts {
                    answered_calls: 1,
                    classified_calls: 1
                }
            ),
            "the v3 member's two calls count; only the v4 member's one is classified"
        );
    }
}
