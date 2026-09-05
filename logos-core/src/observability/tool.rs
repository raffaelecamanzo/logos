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
//!    there is no `&str` door left open.
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
//! 2. add the variant to one of [`Tool::event_class`]'s two arms — the compiler
//!    refuses to build until you do;
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
//! [CR-091]: ../../../docs/requests/CR-091-telemetry-surface-classification-and-usage-attribution.md
//! [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md

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
/// whose `tool` column is in scope ([FR-OB-09]).
///
/// # Why interpolation is safe here
///
/// The interpolated values are `&'static str` literals from a closed enum —
/// never user input, never a stored value. `tool_wire_names_are_sql_safe` pins
/// them to `[a-z][a-z0-9_]*`, so the fragment cannot carry a quote, and the
/// guard fails the build's test run if a future wire name ever could.
///
/// An **unrecognised** stored tool (a name registered by an older build and
/// since retired) is *kept*, not dropped: history is reported as it was
/// recorded rather than silently rewritten by today's registry ([NFR-CC-04]).
///
/// [FR-OB-09]: ../../../docs/specs/requirements/FR-OB-09.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn engine_query_predicate() -> String {
    let excluded = self_referential_tools();
    if excluded.is_empty() {
        // Nothing is classified self-referential: an always-true predicate, so
        // the caller's `AND` composition stays valid SQL rather than `IN ()`.
        return "1 = 1".to_string();
    }
    let names = excluded
        .into_iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("tool NOT IN ({names})")
}
