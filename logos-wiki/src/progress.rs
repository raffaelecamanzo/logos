//! The wiki generation run's progress events ([FR-WK-18], [FR-UI-19]).
//!
//! Defined here, beside the wiki's web surface that streams them, rather than in
//! the `wiki-agent` that emits them: the agent depends on this crate, so the web
//! surface's run registry and SSE mapping (`web::generate`) can name the event
//! without depending on the agent, and the in-app generator joins the `wiki`
//! extension through its run service alone (S-644). `wiki-agent` re-exports it.
//!
//! [FR-WK-18]: ../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-19]: ../../docs/specs/requirements/FR-UI-19.md

/// A per-page progress event emitted across a generation run ([FR-WK-18]). The
/// wiki-agent emits them; the wiki's web surface maps them onto the SSE stream
/// (S-178); the `event`-tagged serialization keeps the wire form
/// self-describing.
///
/// [FR-WK-18]: ../../docs/specs/requirements/FR-WK-18.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
/// [CR-059]: ../../docs/requests/CR-059-wiki-generation-grounding-and-write-guard.md
/// [CR-093]: ../../docs/requests/CR-093-wiki-progress-denominator-honesty.md
/// [S-239]: ../../docs/planning/journal.md#s-239-wiki-run-state-legibility-and-no-redundant-re-trigger-optional
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum WikiProgress {
    /// The run started over a non-empty work-list of `total` pages. Emitted
    /// exactly once per run; a scope that grows later is carried on
    /// [`PageStarted`](Self::PageStarted)'s `total`, never by a second `Started`.
    Started {
        /// The opening work-list size — the run's initial scope estimate.
        total: usize,
        /// The configured per-page synthesis liveness timeout, in whole seconds
        /// (the wiki agent's `DEFAULT_SYNTHESIS_TIMEOUT`, overridable per agent) —
        /// carried so the surface can make the bound VISIBLE (a long-running page is
        /// a liveness guard counting down, not an unexplained stall) without the
        /// frontend duplicating the value ([CR-059], [S-239]).
        synthesis_timeout_secs: u64,
    },
    /// Synthesis of one page began.
    PageStarted {
        /// The target wiki slug.
        slug: String,
        /// The page title.
        title: String,
        /// The 1-based position of this page in the queue.
        index: usize,
        /// The run's scope as of this page: the pages attempted so far plus the
        /// unattempted items the latest queue re-read surfaced ([CR-093]). Equal to
        /// `Started`'s total until a re-read surfaces work the opening read did not
        /// contain; it may also shrink when an item leaves the work-list unattempted,
        /// so a surface folds it as a monotonic maximum. Never below `index`.
        total: usize,
    },
    /// One page was synthesized and persisted via [`wiki write`](crate::WikiEngine::wiki_write).
    PageWritten {
        /// The target wiki slug.
        slug: String,
        /// The number of anchors resolved and stored at write time.
        anchor_count: usize,
        /// `true` when the write replaced an existing page (a refresh).
        replaced: bool,
    },
    /// One page's write was rejected (e.g. an over-cap body or an unknown anchor);
    /// the run continues with the next page ([NFR-CC-04] — no fabrication).
    PageFailed {
        /// The target wiki slug.
        slug: String,
        /// The honest failure reason.
        error: String,
    },
    /// The run halted early — a spent per-run budget or a provider failure —
    /// reported honestly ([NFR-CC-04]). Pages already written persist.
    Halted {
        /// Which limit or failure halted the run.
        reason: String,
    },
    /// The run finished (whether completed or halted); the terminal event.
    Completed {
        /// Pages persisted this run.
        pages_written: usize,
        /// Pages whose write was rejected this run.
        pages_failed: usize,
    },
}
