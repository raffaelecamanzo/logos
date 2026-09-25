//! The registered tool set and its per-event classification ([FR-OB-09]).
//!
//! # Why an enum and not a string
//!
//! Every telemetry event names a `tool`. Until [FR-OB-09] that name was a bare
//! `&'static str` passed to [`traced`](super::traced), so *adding* a tool was a
//! one-line change nothing checked — and the read-model's exclusion rule was a
//! closed list of surfaces that could not notice a new case. That is precisely
//! the failure mode [CR-091] was filed to fix, so it must not be reintroduced
//! one layer down.
//!
//! [`Tool`] is therefore a closed enum and it is the **only** thing the emission
//! helpers accept. Two build-time properties follow:
//!
//! 1. A new engine chokepoint cannot emit telemetry without adding a variant —
//!    `traced*` accepts nothing else, so there is no `&str` door left open on
//!    the chokepoint path.
//!
//!    The **one** sanctioned exception is the debounced watcher
//!    ([`crate::watch`]), which emits `watch_sync` and `watch_coverage_ingest`
//!    with a bare `tracing::info!` because it needs a per-event `surface` field
//!    the helpers cannot express (S-022). Those two sites name
//!    `Tool::…::as_str()` rather than a literal, so the registry is still the
//!    single source of tool names — but there the compiler does not enforce it,
//!    and a future third raw site would not be caught. Prefer `traced*`; if you
//!    genuinely cannot, name a `Tool` and say why here.
//! 2. [`Tool::event_class`] is an **exhaustive match with no wildcard arm**, so
//!    a new variant fails the build until it is classified. `unclassified_tool_
//!    fails_the_build` (in [`super::tests`]) additionally scans this file's
//!    source and fails if a `_ =>` arm is ever added back, because a wildcard
//!    would silently restore the closed-list defect while still compiling.
//!
//! # Extension point
//!
//! To register a new tool — this is the whole procedure, and sessions adding a
//! tool later in Sprint 64 follow exactly these three steps:
//!
//! 1. add one `Variant => "wire_name"` line to the `registered_tools!`
//!    invocation below, in the block for its area;
//! 2. classify it **twice** — one of [`Tool::event_class`]'s two arms and one of
//!    [`Tool::tool_class`]'s five; the compiler refuses to build until you do
//!    both;
//! 3. call `traced(Tool::Variant, …)` from the chokepoint.
//!
//! Nothing else needs touching: the read-model derives its SQL predicate from
//! the classification (see [`self_referential_tools`]), so a newly-classified
//! tool is filtered correctly the moment it exists.
//!
//! # What the classification means
//!
//! [`EventClass::EngineQuery`] — the request's subject is **the code Logos
//! indexes**: navigation, quality/governance reads, and the indexing work that
//! builds the graph. These are the events a usage figure is *about*.
//!
//! [`EventClass::ReadModelRequest`] — the request is **self-referential**: its
//! subject is Logos's own state, so counting it means the measurement observes
//! itself. These are excluded from the usage figures on **every** surface — a
//! CLI `logos stats` invocation is no less self-referential than a dashboard
//! render ([FR-OB-09]).
//!
//! # Two classifications, two questions
//!
//! [`Tool::event_class`] answers *"does this event count as usage at all?"* — a
//! binary exclusion ([FR-OB-09]). [`Tool::tool_class`] answers a different
//! question, *"what kind of work was it?"*, and is the five-way
//! navigation / quality-gate / session-gate / engine-internal / read-model
//! breakdown the stats read-model reports ([FR-OB-11]).
//!
//! They are **independent axes** and deliberately do not agree member-for-member:
//! `languages` is a [`ToolClass::ReadModel`] call (it reports Logos's own
//! grammar registry) yet an [`EventClass::EngineQuery`] event (it is a command
//! the user typed, and it reads back nothing this measurement produced — see
//! `event_class`'s last arm). The one direction that *is* an invariant runs the
//! other way: everything excluded as self-referential is a read-model call, and
//! `the_two_classifications_agree_where_they_must` pins it.
//!
//! Both matches are exhaustive with no wildcard arm, so a new variant fails the
//! build twice over until it is classified on both axes.
//!
//! [CR-091]: ../../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
//! [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
//! [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md

/// Whether an event counts as tool use, or is Logos measuring itself
/// ([FR-OB-09]).
///
/// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventClass {
    /// Work the graph actually did on the indexed code — counted.
    EngineQuery,
    /// A self-referential read of Logos's own state — excluded from the usage
    /// figures on every surface.
    ReadModelRequest,
}

/// What kind of work a call was ([FR-OB-11]) — the five-way breakdown the stats
/// read-model reports, orthogonal to [`EventClass`]'s binary exclusion.
///
/// The distinction existed before this enum, but only as the hardcoded weight
/// table behind the reads-saved estimate (`stats::reads_saved_per_call`), so
/// every sprint dogfood table re-derived it by hand in prose — wasted effort and
/// a place for the classification to drift between reports. Promoting it to a
/// read-model field makes the table `stats` output ([NFR-CC-04]).
///
/// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ToolClass {
    /// Answers a structural question about the indexed code — the calls that
    /// substitute for an agent reading files, which is the whole token-saving
    /// thesis (AS-02). This is the class the reads-saved weights are non-zero on.
    Navigation,
    /// Produces or reports a quality/governance verdict **about the code**:
    /// rules, cycles, hotspots, coverage, health, history.
    QualityGate,
    /// The session lifecycle bracket (`session_start` / `session_end`).
    SessionGate,
    /// Work on one of Logos's own artifacts — building the graph, the wiki, the
    /// config store — rather than a question anybody asked about the code.
    EngineInternal,
    /// Reports Logos's **own** state (index freshness, telemetry, capabilities,
    /// wiki/config state) rather than the code's.
    ReadModel,
}

impl ToolClass {
    /// The wire label carried on the read-model.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            ToolClass::Navigation => "navigation",
            ToolClass::QualityGate => "quality-gate",
            ToolClass::SessionGate => "session-gate",
            ToolClass::EngineInternal => "engine-internal",
            ToolClass::ReadModel => "read-model",
        }
    }
}

/// The class label reported for a tool name **read back from the store**.
///
/// A name today's registry does not know was written by an older build and since
/// retired. It is counted rather than dropped ([`engine_query_predicate`] keeps
/// it), so it needs a label — and the only honest one is that the class is
/// unknown, not a guess ([NFR-CC-04]). It is deliberately outside [`ToolClass`]:
/// the five classes are exhaustive over the *registry*, and inventing a sixth
/// variant for "not in the registry" would let a live tool fall into it.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) const UNREGISTERED_CLASS: &str = "unregistered";

/// Declare the registered tool set: one `Variant => "wire_name"` line per tool.
///
/// Generates the [`Tool`] enum, [`Tool::ALL`] and [`Tool::as_str`] from a single
/// list, so the enum and the exhaustive iteration the read-model derives its
/// filter from can never drift apart. The classification itself is deliberately
/// **not** generated here — it is a hand-written exhaustive match below, so that
/// adding a line to this list breaks the build until the new tool is classified.
macro_rules! registered_tools {
    ($( $(#[$attr:meta])* $variant:ident => $wire:literal ),+ $(,)?) => {
        /// One registered tool — the `tool` column of a telemetry event.
        ///
        /// The single argument type the emission helpers accept, so every
        /// telemetry-producing call site is a member of this set by
        /// construction. See the module docs for the extension procedure.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub(crate) enum Tool {
            $( $(#[$attr])* $variant, )+
        }

        impl Tool {
            /// Every registered tool. Generated with the enum, so it cannot
            /// fall behind it.
            pub(crate) const ALL: &'static [Tool] = &[ $( Tool::$variant, )+ ];

            /// The wire name written to `events.tool`.
            pub(crate) const fn as_str(self) -> &'static str {
                match self { $( Tool::$variant => $wire, )+ }
            }
        }
    };
}

registered_tools! {
    // ── Navigation (FR-NV-*) ────────────────────────────────────────────────
    Search => "search",
    Context => "context",
    Explore => "explore",
    Node => "node",
    Callers => "callers",
    Callees => "callees",
    Impact => "impact",
    /// Impact-set intersection across a set of planned work items ([FR-NV-11],
    /// CR-114) — the scheduling question, asked of the graph instead of the
    /// architecture prose.
    ImpactIntersection => "impact_intersection",
    /// Structural precedent — the nodes analogous to a symbol or file
    /// ([FR-NV-12], CR-114): the "show me the sibling that already does this"
    /// question a plan-driven workflow asks once scope is settled.
    Precedent => "precedent",
    /// Branch and merge symbol overlap across a set of git refs ([FR-NV-13],
    /// CR-114) — the integration question: which refs contend, and what a clean
    /// merge did not carry.
    BranchOverlap => "branch_overlap",
    Implements => "implements",
    ReferencingDocs => "referencing_docs",
    Affected => "affected",
    GraphElements => "graph_elements",

    // ── Governance / quality reads (FR-GV-*, FR-GH-*, FR-CV-*) ──────────────
    Scan => "scan",
    Rescan => "rescan",
    Gate => "gate",
    CheckRules => "check_rules",
    Evolution => "evolution",
    Dsm => "dsm",
    DocGaps => "doc_gaps",
    Health => "health",
    Doctor => "doctor",
    Verify => "verify",
    Hotspots => "hotspots",
    LatestMetrics => "latest_metrics",
    LatestScan => "latest_scan",
    LatestGate => "latest_gate",
    /// The Health bundle's two snapshot-derived fields from **one** read of the
    /// last persisted snapshot ([FR-UI-04], CR-135) — the seam that replaced a
    /// `latest_gate` + `latest_scan` pair whose two reads could straddle a
    /// `scan`. Registered in its own right because it is a distinct chokepoint,
    /// not either of the two it projects.
    LatestHealth => "latest_health",
    LatestTemporalReport => "latest_temporal_report",
    LatestHotspots => "latest_hotspots",
    QualityReadout => "quality_readout",
    LanguageComposition => "language_composition",
    CoverageIngest => "coverage_ingest",
    CoverageIngestAuto => "coverage_ingest_auto",
    CoverageRefresh => "coverage_refresh",
    CoverageStatus => "coverage_status",

    // ── Session gate (FR-SG-*) ──────────────────────────────────────────────
    SessionStart => "session_start",
    SessionEnd => "session_end",

    // ── Indexing / sync pipeline (FR-IX-*, FR-SY-*) ─────────────────────────
    Init => "init",
    Index => "index",
    Sync => "sync",
    EnsureIndexed => "ensure_indexed",
    Discover => "discover",
    Load => "load",
    Extract => "extract",
    Resolve => "resolve",
    Annotate => "annotate",
    Persist => "persist",
    /// The debounced watcher's per-window sync *trigger*, emitted under the
    /// `watcher` surface override — distinct from `sync` so the trigger is
    /// attributable without double-counting the operation (S-022).
    WatchSync => "watch_sync",
    /// The watcher's automatic coverage-artifact ingest (same override).
    WatchCoverageIngest => "watch_coverage_ingest",
    WorktreeSeed => "worktree_seed",
    NavProloguePurge => "nav_prologue_purge",

    // ── Wiki (FR-WK-*) ──────────────────────────────────────────────────────
    WikiWrite => "wiki_write",
    WikiRead => "wiki_read",
    WikiDelete => "wiki_delete",
    WikiPrunedLog => "wiki_pruned_log",
    WikiSearch => "wiki_search",
    WikiStatus => "wiki_status",
    WikiGenerate => "wiki_generate",
    WikiNative => "wiki_native",
    WikiDocCategoryPresent => "wiki_doc_category_present",
    WikiSrsMode => "wiki_srs_mode",
    WikiGuidePages => "wiki_guide_pages",
    WikiReconcile => "wiki_reconcile",
    WikiMaterialize => "wiki_materialize",
    WikiSkillEmit => "wiki_skill_emit",
    WikiQualityReportHookEmit => "wiki_quality_report_hook_emit",

    // ── Configuration (FR-CF-*) ─────────────────────────────────────────────
    ConfigRead => "config_read",
    ConfigWrite => "config_write",
    ConfigWriteSecret => "config_write_secret",
    ConfigApply => "config_apply",

    // ── Read-models of Logos's own state (FR-OB-04, FR-NV-07) ───────────────
    Stats => "stats",
    Status => "status",
    Languages => "languages",
}

impl Tool {
    /// This tool's [`EventClass`] ([FR-OB-09]).
    ///
    /// # This match must stay exhaustive
    ///
    /// **Never add a `_ =>` arm.** The wildcard is the whole defect [CR-091]
    /// exists to fix: it turns "every registered tool has a classification"
    /// into "every tool I remembered has a classification, and the rest get a
    /// silent default". Without a wildcard the compiler names each new tool
    /// for you; with one it says nothing and the read-model quietly misreports.
    /// `unclassified_tool_fails_the_build` scans this function's source and
    /// fails if a wildcard reappears.
    ///
    /// The `ReadModelRequest` set is deliberately **minimal**: a tool belongs
    /// there only when its subject is Logos's own state rather than the code
    /// Logos indexes, and each member below cites the criterion that put it
    /// there. Excluding more than that would understate genuine tool use just
    /// as surely as the blanket surface filter overstated it ([NFR-CC-04]).
    ///
    /// [CR-091]: ../../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
    /// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub(crate) const fn event_class(self) -> EventClass {
        match self {
            // ── Self-referential: the subject is Logos's own state ──────────
            //
            // `stats` reads the very telemetry store the usage figure is
            // computed from, so counting it means the figure counts its own
            // observation — the Statistics tab inflating the Statistics tab,
            // and equally a CLI `logos stats` inflating what it prints
            // ([FR-OB-09] AC 1).
            Tool::Stats
            // `status` is the graph-state readout ([FR-NV-07]) the app shell
            // re-issues on **every** client-side navigation ([FR-UI-34]). It
            // asks nothing about the code and answers no question the user
            // put — a request the user's own navigation caused incidentally
            // ([BR-42]) — so the shell's own read must not become the loudest
            // event in the store.
            | Tool::Status => EventClass::ReadModelRequest,

            // ── Engine queries: the subject is the indexed code ─────────────
            //
            // Navigation.
            Tool::Search
            | Tool::Context
            | Tool::Explore
            | Tool::Node
            | Tool::Callers
            | Tool::Callees
            | Tool::Impact
            | Tool::ImpactIntersection
            | Tool::Precedent
            | Tool::BranchOverlap
            | Tool::Implements
            | Tool::ReferencingDocs
            | Tool::Affected
            | Tool::GraphElements
            // Governance / quality reads — every one answers a question about
            // the repository (rules, cycles, hotspots, coverage, history).
            | Tool::Scan
            | Tool::Rescan
            | Tool::Gate
            | Tool::CheckRules
            | Tool::Evolution
            | Tool::Dsm
            | Tool::DocGaps
            | Tool::Health
            | Tool::Doctor
            | Tool::Verify
            | Tool::Hotspots
            | Tool::LatestMetrics
            | Tool::LatestScan
            | Tool::LatestGate
            | Tool::LatestHealth
            | Tool::LatestTemporalReport
            | Tool::LatestHotspots
            | Tool::QualityReadout
            | Tool::LanguageComposition
            | Tool::CoverageIngest
            | Tool::CoverageIngestAuto
            | Tool::CoverageRefresh
            | Tool::CoverageStatus
            // Session gate.
            | Tool::SessionStart
            | Tool::SessionEnd
            // Indexing / sync — the work that builds the graph over the code.
            | Tool::Init
            | Tool::Index
            | Tool::Sync
            | Tool::EnsureIndexed
            | Tool::Discover
            | Tool::Load
            | Tool::Extract
            | Tool::Resolve
            | Tool::Annotate
            | Tool::Persist
            | Tool::WatchSync
            | Tool::WatchCoverageIngest
            | Tool::WorktreeSeed
            | Tool::NavProloguePurge
            // Wiki — documentation *of the code*, written and read from it.
            | Tool::WikiWrite
            | Tool::WikiRead
            | Tool::WikiDelete
            | Tool::WikiPrunedLog
            | Tool::WikiSearch
            | Tool::WikiStatus
            | Tool::WikiGenerate
            | Tool::WikiNative
            | Tool::WikiDocCategoryPresent
            | Tool::WikiSrsMode
            | Tool::WikiGuidePages
            | Tool::WikiReconcile
            | Tool::WikiMaterialize
            | Tool::WikiSkillEmit
            | Tool::WikiQualityReportHookEmit
            // Configuration — a deliberate action on the project's policy, and
            // the read that renders it; the user asked for both.
            | Tool::ConfigRead
            | Tool::ConfigWrite
            | Tool::ConfigWriteSecret
            | Tool::ConfigApply
            // `languages` enumerates the compiled grammar plugins. It is a
            // capability question rather than a graph query, but it is one the
            // user asked — `logos languages` is a command, not shell furniture —
            // and it is not self-*referential*: it does not read back anything
            // this measurement produced.
            | Tool::Languages => EventClass::EngineQuery,
        }
    }
}

impl Tool {
    /// This tool's [`ToolClass`] ([FR-OB-11]).
    ///
    /// # This match must stay exhaustive
    ///
    /// **Never add a `_ =>` arm**, for exactly the reason spelled out on
    /// [`Tool::event_class`]: [FR-OB-11] requires that "an unclassified tool
    /// fails the build rather than defaulting silently", and a wildcard turns
    /// that into a silent default while still compiling.
    /// `unclassified_tool_fails_the_build` scans this function's source too.
    ///
    /// The arms follow the `registered_tools!` blocks where the block *is* the
    /// class; the arms that cross a block boundary say why.
    ///
    /// [FR-OB-11]: ../../../docs/specs/requirements/FR-OB-11.md
    pub(crate) const fn tool_class(self) -> ToolClass {
        match self {
            // ── Navigation: traverses the graph to answer a question about the
            // code, replacing ad-hoc file reads (AS-02) ─────────────────────
            Tool::Search
            | Tool::Context
            | Tool::Explore
            | Tool::Node
            | Tool::Callers
            | Tool::Callees
            | Tool::Impact
            | Tool::ImpactIntersection
            | Tool::Precedent
            | Tool::BranchOverlap
            | Tool::Implements
            | Tool::ReferencingDocs
            | Tool::Affected
            | Tool::GraphElements
            // The two *reads* over the wiki. They sit in the wiki block because
            // that is where the store lives, but their subject is the codebase
            // and they substitute for opening source — the navigation criterion.
            // The rest of the wiki block writes or rebuilds the store, and is
            // engine-internal below.
            | Tool::WikiRead
            | Tool::WikiSearch => ToolClass::Navigation,

            // ── Quality gate: a verdict about the repository ────────────────
            Tool::Scan
            | Tool::Rescan
            | Tool::Gate
            | Tool::CheckRules
            | Tool::Evolution
            | Tool::Dsm
            | Tool::DocGaps
            | Tool::Health
            | Tool::Doctor
            | Tool::Verify
            | Tool::Hotspots
            | Tool::LatestMetrics
            | Tool::LatestScan
            | Tool::LatestGate
            | Tool::LatestHealth
            | Tool::LatestTemporalReport
            | Tool::LatestHotspots
            | Tool::QualityReadout
            | Tool::LanguageComposition
            // `coverage_status` reports the coverage verdict *over the code*;
            // its three ingest/refresh siblings mutate the graph from an
            // artifact and are engine-internal.
            | Tool::CoverageStatus => ToolClass::QualityGate,

            // ── Session gate ────────────────────────────────────────────────
            Tool::SessionStart | Tool::SessionEnd => ToolClass::SessionGate,

            // ── Engine-internal: work on Logos's own artifacts ──────────────
            //
            // The indexing / sync pipeline that builds the graph.
            Tool::Init
            | Tool::Index
            | Tool::Sync
            | Tool::EnsureIndexed
            | Tool::Discover
            | Tool::Load
            | Tool::Extract
            | Tool::Resolve
            | Tool::Annotate
            | Tool::Persist
            | Tool::WatchSync
            | Tool::WatchCoverageIngest
            | Tool::WorktreeSeed
            | Tool::NavProloguePurge
            // Coverage ingestion: folds an external artifact into the graph.
            | Tool::CoverageIngest
            | Tool::CoverageIngestAuto
            | Tool::CoverageRefresh
            // The wiki store's writes, generation and maintenance.
            | Tool::WikiWrite
            | Tool::WikiDelete
            | Tool::WikiPrunedLog
            | Tool::WikiGenerate
            | Tool::WikiNative
            | Tool::WikiDocCategoryPresent
            | Tool::WikiSrsMode
            | Tool::WikiGuidePages
            | Tool::WikiReconcile
            | Tool::WikiMaterialize
            | Tool::WikiSkillEmit
            | Tool::WikiQualityReportHookEmit
            // Mutations of the project's own policy store.
            | Tool::ConfigWrite
            | Tool::ConfigWriteSecret
            | Tool::ConfigApply => ToolClass::EngineInternal,

            // ── Read-model: reports Logos's own state ───────────────────────
            //
            // `stats` and `status` are also the two `EventClass::
            // ReadModelRequest` tools, so they never reach the cross-tab at all
            // (FR-OB-09 excludes them). The other three are counted events —
            // the two axes are independent, see the module docs.
            Tool::Stats
            | Tool::Status
            // Which grammar plugins compiled in — a capability readout.
            | Tool::Languages
            // The wiki store's own freshness/anchor state, and the rendered
            // configuration; neither asks anything about the code.
            | Tool::WikiStatus
            | Tool::ConfigRead => ToolClass::ReadModel,
        }
    }

    /// The registered tool with this wire name, or `None` for a name today's
    /// registry does not know (see [`UNREGISTERED_CLASS`]).
    ///
    /// Linear over [`Tool::ALL`]: the registry is a few dozen entries and this
    /// runs once per distinct tool name in a stats window, never on the emission
    /// path ([NFR-OO-02]).
    ///
    /// [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
    pub(crate) fn from_wire(name: &str) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.as_str() == name)
    }
}

/// The [`ToolClass`] label for a wire name read back from the store, or
/// [`UNREGISTERED_CLASS`] when the registry does not know it.
pub(crate) fn class_of_wire(name: &str) -> &'static str {
    Tool::from_wire(name).map_or(UNREGISTERED_CLASS, |t| t.tool_class().as_str())
}

/// The wire names of every self-referential tool, in registration order.
///
/// Derived from [`Tool::event_class`] over [`Tool::ALL`], so it is exactly the
/// classification and cannot drift from it. This is what the read-model builds
/// its exclusion predicate from ([`super::stats`]), which is why the exclusion
/// applies uniformly to raw events, rolled-up days, and rows written before the
/// classification existed — a stored per-row class could not have done that.
pub(crate) fn self_referential_tools() -> Vec<&'static str> {
    Tool::ALL
        .iter()
        .filter(|t| matches!(t.event_class(), EventClass::ReadModelRequest))
        .map(|t| t.as_str())
        .collect()
}

/// A SQL predicate keeping only [`EventClass::EngineQuery`] rows, over a table
/// whose `tool` **and** `surface` columns are in scope ([FR-OB-09]) — `events`
/// and `daily_rollup`, the two the read-model reads.
///
/// # Two axes, because two things can be self-referential
///
/// A **tool** is self-referential when its subject is Logos's own state
/// whoever calls it ([`Tool::event_class`]); a **surface** is when every event
/// it can carry is of that kind by construction
/// ([`Surface::event_class`](super::Surface::event_class), widened for shell
/// chrome by [CR-097]). The second axis is what an adapter uses to classify a
/// read the tool arm cannot: the app header calling a *navigation* tool is
/// still nobody's question ([BR-42]). Both lists are derived from the
/// classifications rather than written out here, so neither can drift from it.
///
/// # Why interpolation is safe here
///
/// The interpolated values are `&'static str` literals from closed enums —
/// never user input, never a stored value.
/// `every_registered_tool_is_classified_and_sql_safe` and
/// `every_surface_is_classified_and_sql_safe` pin them to `[a-z][a-z0-9_]*`, so
/// the fragment cannot carry a quote, and the guards fail the build's test run
/// if a future wire name ever could.
///
/// An **unrecognised** stored tool (a name registered by an older build and
/// since retired) is *kept*, not dropped: history is reported as it was
/// recorded rather than silently rewritten by today's registry ([NFR-CC-04]).
/// The same holds for an unrecognised stored `surface`.
///
/// [CR-097]: ../../../docs/requests/CR-097-header-graph-state-readout.md
/// [BR-42]: ../../../docs/specs/software-spec.md#316-observability--telemetry
/// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// `column NOT IN ('a', 'b')`, or `None` when the classification excludes nothing
/// on that axis — so an empty axis contributes no clause at all rather than the
/// invalid `NOT IN ()`.
///
/// At module scope rather than nested inside [`engine_query_predicate`] so its
/// empty branch is reachable from a test. Both axes are non-empty today, so
/// that branch cannot otherwise be exercised without editing the
/// classification — and a review demonstrated the cost of that: the sibling
/// `"1 = 1"` fallback could be replaced with literal non-SQL and the whole
/// 2241-test crate stayed green.
pub(crate) fn not_in(column: &str, excluded: Vec<&'static str>) -> Option<String> {
    if excluded.is_empty() {
        return None;
    }
    let names = excluded
        .into_iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("{column} NOT IN ({names})"))
}

pub(crate) fn engine_query_predicate() -> String {
    compose_predicate(
        [
            not_in("tool", self_referential_tools()),
            not_in("surface", super::self_referential_surfaces()),
        ]
        .into_iter()
        .flatten()
        .collect(),
    )
}

/// Join the per-axis clauses into the fragment the read-model interpolates.
///
/// `AND` throughout, so the fragment needs no parentheses of its own to survive
/// the caller's own `AND` composition. It must stay that way: an `OR`
/// introduced here without wrapping would silently widen every interpolating
/// query's `WHERE`.
///
/// Separated from [`engine_query_predicate`] for the same reason as
/// [`not_in`]: the no-clauses branch is unreachable while any tool or surface
/// is classified self-referential, and a review showed what an unreachable
/// branch costs — the `"1 = 1"` fallback could be replaced with literal non-SQL
/// and the whole crate's 2241 tests still passed. A branch that guards invalid
/// SQL is worth being able to test.
pub(crate) fn compose_predicate(clauses: Vec<String>) -> String {
    if clauses.is_empty() {
        // Nothing is classified self-referential: an always-true predicate, so
        // the caller's `AND` composition stays valid SQL rather than a dangling
        // `WHERE … AND`.
        return "1 = 1".to_string();
    }
    clauses.join(" AND ")
}

// ── The call outcome ([FR-OB-14]) ────────────────────────────────────────────
//
// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md

/// What one traced call **answered** ([FR-OB-14]) — a closed four-value
/// vocabulary, stored in `events.outcome` from migration v4.
///
/// `NULL` in that column is not a fifth value. It means exactly "this tool has
/// no outcome vocabulary, or this row predates v4", and nothing else: a
/// classified tool's `Err(_)` records [`Outcome::Failed`] rather than `NULL`
/// ([`traced_with`](super::traced_with)), so an absent outcome is never a
/// disguised failure. A pre-v4 row is never imputed one.
///
/// Reason codes (`precedent`'s eight [`EmptyPrecedentCode`]s) are deliberately
/// **not** carried: the column is four values so it stays cheap to aggregate,
/// and folding the codes in would be a high-cardinality schema decision nobody
/// has asked for ([CR-144] §3.2).
///
/// [`EmptyPrecedentCode`]: crate::models::navigation::EmptyPrecedentCode
/// [CR-144]: ../../../docs/requests/CR-144-telemetry-records-the-answer-not-only-the-call.md
/// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The call returned something to act on.
    Answered,
    /// The call resolved its input and there is legitimately nothing to report.
    Empty,
    /// The call could not answer: its input named nothing the graph holds, or
    /// the graph it would have answered over is not there.
    Unresolved,
    /// The call returned `Err(_)`. Recorded by [`traced_with`](super::traced_with)
    /// itself, never by a classifier — the classifier only ever sees an `Ok`
    /// value.
    Failed,
}

impl Outcome {
    /// Every value, in declaration order — what [`Outcome::from_wire`] searches,
    /// and what the tests pin the migration's `CHECK` list against.
    pub(crate) const ALL: [Outcome; 4] = [
        Outcome::Answered,
        Outcome::Empty,
        Outcome::Unresolved,
        Outcome::Failed,
    ];

    /// The stored spelling. Exhaustive with no wildcard arm, like
    /// [`Tool::event_class`], so a fifth value fails the build here first.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Outcome::Answered => "answered",
            Outcome::Empty => "empty",
            Outcome::Unresolved => "unresolved",
            Outcome::Failed => "failed",
        }
    }

    /// The inverse of [`Outcome::as_str`], or `None` for any other string — so
    /// the telemetry layer cannot record a value outside the closed set, the
    /// same guard [`Surface::from_wire`](super::Surface::from_wire) gives the
    /// `surface` field.
    pub(crate) fn from_wire(value: &str) -> Option<Outcome> {
        Outcome::ALL.into_iter().find(|o| o.as_str() == value)
    }
}

/// A result type that knows what it answered ([FR-OB-14]) — implemented
/// **only** for the result types that have an outcome to report, and consulted
/// only by [`traced_with`](super::traced_with).
///
/// Opt-in rather than a bound on `traced`: Rust has no specialisation, so a
/// `T: CallOutcome` bound would force an impl on every one of the ~140 traced
/// result types, pipeline passes and `()` included. A tool that never calls
/// [`traced_with`](super::traced_with) records `NULL` by construction.
///
/// # The contract every impl keeps
///
/// - **O(1) field reads** ([NFR-OO-02]): `is_empty()`, an `Option` test, a
///   `match` on a code. The call runs inside the measured span, so an impl that
///   walked its payload would bill the walk to the tool's latency.
/// - **No payload content** ([NFR-CC-03]): the answer is one of four words;
///   nothing the result holds — a symbol, a path, a count — leaves it.
///
/// [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
/// [NFR-CC-03]: ../../../docs/specs/requirements/NFR-CC-03.md
/// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
pub(crate) trait CallOutcome {
    /// What this `Ok` value answered.
    fn outcome(&self) -> Outcome;
}

/// `callers` ([FR-NV-05]): unresolved when the query named no symbol, answered
/// when at least one caller came back.
///
/// Reads `total` rather than the returned page, so a `limit` that truncated
/// the list to nothing still counts as answered.
///
/// [FR-NV-05]: ../../../docs/specs/requirements/FR-NV-05.md
impl CallOutcome for crate::models::CallersResult {
    fn outcome(&self) -> Outcome {
        if self.resolved.is_none() {
            Outcome::Unresolved
        } else if self.total > 0 || !self.callers.is_empty() {
            Outcome::Answered
        } else {
            Outcome::Empty
        }
    }
}

/// `impact` ([FR-NV-06]): unresolved when the query named no symbol, answered
/// when either direction of the closure — or the documentation trace — has an
/// entry.
///
/// [FR-NV-06]: ../../../docs/specs/requirements/FR-NV-06.md
impl CallOutcome for crate::models::ImpactResult {
    fn outcome(&self) -> Outcome {
        if self.resolved.is_none() {
            Outcome::Unresolved
        } else if !self.upstream.is_empty() || !self.downstream.is_empty() || !self.docs.is_empty()
        {
            Outcome::Answered
        } else {
            Outcome::Empty
        }
    }
}

/// `precedent` ([FR-NV-12]): answered when any precedent was found; otherwise
/// the answer's own closed [`EmptyPrecedentCode`] decides which absence it is.
///
/// The split follows what each code establishes. The target named nothing, the
/// graph is empty, the target fell out of the compared view, or the query
/// itself degraded — the call **could not** answer (`unresolved`). The target
/// resolved and was compared, and nothing matched (no anchors, unshared
/// anchors, only ubiquitous anchors) — `empty`. [S-442]'s resolution
/// denominator will refine that second group; this impl is where it will.
///
/// [`EmptyPrecedentCode`]: crate::models::navigation::EmptyPrecedentCode
/// [S-442]: ../../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [FR-NV-12]: ../../../docs/specs/requirements/FR-NV-12.md
impl CallOutcome for crate::models::PrecedentResult {
    fn outcome(&self) -> Outcome {
        use crate::models::navigation::EmptyPrecedentCode as Code;
        if self.total_found > 0 || !self.precedents.is_empty() {
            return Outcome::Answered;
        }
        match self.empty_reason.as_ref().map(|reason| reason.code) {
            Some(
                Code::TargetUnresolved
                | Code::GraphEmpty
                | Code::TargetAbsentFromView
                | Code::QueryFailed
                | Code::ResultsUnavailable,
            ) => Outcome::Unresolved,
            Some(
                Code::NoStructuralAnchors | Code::AnchorsAreUnshared | Code::AnchorsAreUbiquitous,
            )
            | None => Outcome::Empty,
        }
    }
}

/// `affected` ([FR-CL-04]): answered when the closure holds a dependent file;
/// unresolved when none of the changed paths is in the indexed graph (the
/// closure had no seed to start from); otherwise empty.
///
/// [FR-CL-04]: ../../../docs/specs/requirements/FR-CL-04.md
impl CallOutcome for crate::models::AffectedResult {
    fn outcome(&self) -> Outcome {
        if !self.affected.is_empty() {
            Outcome::Answered
        } else if self.changed.is_empty() {
            Outcome::Unresolved
        } else {
            Outcome::Empty
        }
    }
}
