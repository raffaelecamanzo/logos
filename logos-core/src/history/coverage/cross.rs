//! The symbol-level **reachability × runtime-coverage cross** read-model
//! ([FR-UI-17], [CR-036], [ADR-23] symbol-level attribution).
//!
//! For each non-test function/method symbol this computes the pair
//! `(reachable_from_test, runtime_exec_fraction)` and classifies it into one of
//! four quadrants, numbered to the **Gartner convention** (Q4 best, top-right;
//! Q1 worst, top-left) so the `/quadrant` 2×2 reads "best top-right" ([CR-040]):
//!
//! - **Q4** reachable + executed — *trust* (best);
//! - **Q2** reachable + 0% executed — a *dead / guarded test edge* the static
//!   graph cannot see;
//! - **Q1** not-reachable + executed — *incidental / false-green* (worst) the flat
//!   coverage % cannot see;
//! - **Q3** not-reachable + 0% executed — a *true gap*.
//!
//! Severity (desirability) runs Q4 > Q3 > Q2 > Q1; the classification `(reachable,
//! executed)` pair and the `n/a` rule are unchanged — only the labels/positions
//! flipped ([CR-040]).
//!
//! # The decoupling that protects the tier boundary ([UAT-GH-02])
//! Reachability comes from the governance `test_gaps` BFS ([FR-GV-08]). Like the
//! `indexed_paths` argument to [`super::ingest`], that signal is supplied **by the
//! `api` layer as a plain boolean per symbol**, never read here — so the
//! history-engine never links the governance-engine and the
//! [history-engine ↛ governance-engine] boundary stays one-directional, the same
//! way "the gate cannot see history" is physical, not conventional ([BR-26],
//! [BR-28]).
//!
//! # Never fabricate ([NFR-RA-05])
//! The runtime axis is `n/a` — never a guessed `0` — whenever a symbol's span
//! cannot be resolved, its file is not freshly covered, or its span contains no
//! instrumented line. Only a span with at least one instrumented line yields a
//! fraction, and only then is the symbol placed in a quadrant.
//!
//! # Determinism ([NFR-RA-06])
//! All arithmetic is integer basis points (`/10000`, [ADR-08]); symbols are
//! emitted in a canonical `(file, start_line, name, symbol)` order; the coverage
//! line maps are `BTreeMap`s. The output is byte-identical at a fixed HEAD + store
//! state.
//!
//! # Tier boundary ([BR-28])
//! This is advisory evidence read-side only. It reads `history.db` (+ the
//! `api`-supplied graph spans) and **persists nothing**; the gated metric path
//! never calls it, so `gate`/`scan` are byte-identical with or without it.
//!
//! [FR-UI-17]: ../../../../docs/specs/requirements/FR-UI-17.md
//! [FR-GV-08]: ../../../../docs/specs/requirements/FR-GV-08.md
//! [CR-036]: ../../../../docs/requests/CR-036-automatic-coverage-ingest-and-coverage-cross-quadrant.md
//! [CR-040]: ../../../../docs/requests/CR-040-quadrant-true-2x2-and-trust-card-legibility.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
//! [ADR-08]: ../../../../docs/specs/architecture/decisions/ADR-08.md
//! [BR-26]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
//! [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
//! [UAT-GH-02]: ../../../../docs/specs/requirements/UAT-GH-02.md
//! [history-engine ↛ governance-engine]: ../../../../docs/specs/architecture/components/history-engine.md

use std::path::Path;

use anyhow::Result;

use super::read;

/// One symbol's place in the reachability × runtime-coverage cross ([FR-UI-17]),
/// numbered to the Gartner convention so the 2×2 reads "best top-right" ([CR-040]).
///
/// `Q4` trust (best), `Q2` dead/guarded test edge, `Q3` true gap, `Q1`
/// incidental/false-green (worst). Serialized lowercase (`"q1"`..`"q4"`) for the
/// view payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Quadrant {
    /// **Not** reachable from any test but executed — incidental / false-green
    /// (the worst cell: coverage looks green but no test reaches it).
    Q1,
    /// Reachable from a test but **0%** executed — a dead or guarded test edge.
    Q2,
    /// Neither reachable nor executed — a true gap.
    Q3,
    /// Reachable from a test **and** executed at runtime — trust (the best cell).
    Q4,
}

impl Quadrant {
    /// Classify a symbol from its `(reachable, executed)` pair. The mapping is
    /// unchanged from [CR-036]; only the Gartner labels each pair carries flipped
    /// ([CR-040]) — so the read-model and gate stay byte-identical.
    fn classify(reachable: bool, executed: bool) -> Self {
        match (reachable, executed) {
            (true, true) => Quadrant::Q4,   // trust (best)
            (true, false) => Quadrant::Q2,  // dead / guarded test edge
            (false, true) => Quadrant::Q1,  // incidental / false-green (worst)
            (false, false) => Quadrant::Q3, // true gap
        }
    }
}

/// One non-test function/method symbol fed into the cross, assembled at the `api`
/// layer from a graph read ([FR-UI-17]). The span comes from the graph
/// (`logos.db`); `reachable_from_test` is the governance `test_gaps` BFS verdict
/// ([FR-GV-08]) supplied here as a plain boolean so the history-engine never
/// links governance ([UAT-GH-02]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossSymbolInput {
    /// The canonical SCIP symbol identity.
    pub symbol: String,
    /// The human-facing symbol name.
    pub name: String,
    /// The defining file's repo-relative path (empty when the node has none).
    pub file: String,
    /// 1-based declaration start line; `None` ⇒ unresolvable span ⇒ runtime `n/a`.
    pub start_line: Option<i64>,
    /// 1-based declaration end line; `None` ⇒ unresolvable span ⇒ runtime `n/a`.
    pub end_line: Option<i64>,
    /// `true` when a test transitively calls this symbol over `Calls` edges
    /// ([FR-GV-08]).
    pub reachable_from_test: bool,
}

/// One symbol's resolved cross row — a `Serialize` read-model ([ADR-01]) the
/// `/quadrant` view and the Dashboard trust-score card render ([FR-UI-17]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CrossSymbol {
    /// The canonical SCIP symbol identity.
    pub symbol: String,
    /// The human-facing symbol name.
    pub name: String,
    /// The defining file's repo-relative path.
    pub file: String,
    /// 1-based declaration start line, when recorded.
    pub start_line: Option<i64>,
    /// 1-based declaration end line, when recorded.
    pub end_line: Option<i64>,
    /// The static-reachability axis (Y): a test transitively calls it ([FR-GV-08]).
    pub reachable_from_test: bool,
    /// The runtime-execution axis (X): covered ÷ instrumented lines over the
    /// symbol's span, in basis points (0–10000). `None` is `n/a` — an
    /// unresolvable span, a non-fresh-covered file, or a span with no
    /// instrumented line — never a guessed `0` ([NFR-RA-05]).
    pub runtime_exec_bp: Option<i64>,
    /// The quadrant, or `None` exactly when [`runtime_exec_bp`](Self::runtime_exec_bp)
    /// is `n/a` (a symbol with no runtime axis cannot be placed).
    pub quadrant: Option<Quadrant>,
}

/// Quadrant tallies over the resolved symbols — raw counts for the Dashboard
/// trust-score card and `/quadrant` summary ([FR-UI-17]). The
/// *architecturally-weighted* Q4 share (the headline trust %) is composed in the
/// view from these plus the hotspot weighting ([FR-GH-06]); these are the
/// unweighted basis. Fields are named for the Gartner labels ([CR-040]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct CrossTotals {
    /// Not-reachable + executed — false-green (worst).
    pub q1: usize,
    /// Reachable + 0% executed — dead / guarded test edge.
    pub q2: usize,
    /// Not-reachable + 0% executed — true gap.
    pub q3: usize,
    /// Reachable + executed — trust (best).
    pub q4: usize,
    /// Symbols with no runtime axis (`n/a` — unresolvable span / not freshly
    /// covered / no instrumented line in the span).
    pub na_runtime: usize,
    /// Every non-test function/method considered (the four quadrants + `n/a`).
    pub total: usize,
}

/// The symbol-level reachability × runtime-coverage cross read-model ([FR-UI-17]).
///
/// Shared byte-for-byte by the `/quadrant` view, the Dashboard trust-score card,
/// and any CLI/MCP twin ([NFR-CC-01]). With no coverage ingested, every symbol is
/// `n/a` on the runtime axis (the Y reachability axis still exists) and
/// [`notice`](Self::notice) carries the honest empty-state prompt ([NFR-CC-04]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoverageCrossReport {
    /// The ingest-time HEAD SHA the coverage snapshot is anchored to; `None` when
    /// no coverage exists.
    pub head_sha: Option<String>,
    /// The effective `[coverage]` config hash recorded at ingest ([FR-CV-09]);
    /// `None` when no coverage exists.
    pub config_hash: Option<String>,
    /// `true` when the latest snapshot has at least one **fresh** covered file —
    /// i.e. the runtime axis carries real data for some symbol. `false` for both
    /// no-coverage and all-stale snapshots: the surface shows the honest
    /// empty/partial state ([NFR-CC-04]).
    pub has_fresh_coverage: bool,
    /// One row per non-test function/method, in canonical `(file, start_line,
    /// name, symbol)` order ([NFR-RA-06]).
    pub symbols: Vec<CrossSymbol>,
    /// Quadrant tallies over [`symbols`](Self::symbols).
    pub totals: CrossTotals,
    /// The `n/a` empty-state notice when no coverage has ever been ingested;
    /// `None` once a snapshot exists ([NFR-CC-04]).
    pub notice: Option<String>,
}

/// Compute the cross read-model under `root` for the `api`-supplied `symbols`
/// ([FR-UI-17]).
///
/// `symbols` is the full non-test function/method set with graph spans and the
/// governance reachability verdict already attached ([UAT-GH-02] one-directional
/// boundary). This reads the latest coverage snapshot's per-line hits for the
/// fresh files ([FR-CV-05]) and intersects each symbol's span with them; persists
/// nothing ([BR-28], [ADR-28]).
///
/// # Errors
/// Returns an error only on an unexpected `history.db` read failure.
pub fn coverage_cross(
    root: &Path,
    mut symbols: Vec<CrossSymbolInput>,
) -> Result<CoverageCrossReport> {
    let view = read::read_latest_line_hits(root)?;

    // Canonical, byte-identical order ([NFR-RA-06]).
    symbols.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.start_line.cmp(&b.start_line))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });

    let mut totals = CrossTotals::default();
    let mut out = Vec::with_capacity(symbols.len());
    for sym in symbols {
        let (runtime_exec_bp, quadrant) = match &view {
            // A snapshot exists: attribute hits onto the span (or `n/a`).
            Some(v) => attribute(v, &sym),
            // No coverage ingested at all → the runtime axis is `n/a` for every
            // symbol; the reachability axis still stands.
            None => (None, None),
        };
        totals.total += 1;
        match quadrant {
            Some(Quadrant::Q1) => totals.q1 += 1,
            Some(Quadrant::Q2) => totals.q2 += 1,
            Some(Quadrant::Q3) => totals.q3 += 1,
            Some(Quadrant::Q4) => totals.q4 += 1,
            None => totals.na_runtime += 1,
        }
        out.push(CrossSymbol {
            symbol: sym.symbol,
            name: sym.name,
            file: sym.file,
            start_line: sym.start_line,
            end_line: sym.end_line,
            reachable_from_test: sym.reachable_from_test,
            runtime_exec_bp,
            quadrant,
        });
    }

    let head_sha = view.as_ref().map(|v| v.head_sha.clone());
    let config_hash = view.as_ref().map(|v| v.config_hash.clone());
    let has_fresh_coverage = view.as_ref().is_some_and(|v| !v.fresh_files.is_empty());
    // The empty-state notice is for *no coverage ever ingested* only — a populated
    // but all-stale snapshot is a partial state, not an empty one.
    let notice = view
        .is_none()
        .then(|| super::NO_COVERAGE_NOTICE.to_string());

    Ok(CoverageCrossReport {
        head_sha,
        config_hash,
        has_fresh_coverage,
        symbols: out,
        totals,
        notice,
    })
}

/// Map the fresh per-line hits onto one symbol's span, returning
/// `(runtime_exec_bp, quadrant)` or `(None, None)` for an honest `n/a`.
///
/// `n/a` (never a guessed `0`, [NFR-RA-05]) when: the span is unresolvable or
/// inverted; the file is not freshly covered; or the span overlaps no
/// instrumented line. Otherwise the fraction is covered ÷ instrumented over the
/// span in nearest-bp ([ADR-08]) and the quadrant follows from
/// `(reachable, executed = bp > 0)`.
fn attribute(view: &read::LineHitsView, sym: &CrossSymbolInput) -> (Option<i64>, Option<Quadrant>) {
    let (Some(start), Some(end)) = (sym.start_line, sym.end_line) else {
        return (None, None);
    };
    if end < start {
        // A corrupt/inverted span attributes nothing rather than fabricating.
        return (None, None);
    }
    let Some(lines) = view.fresh_files.get(&sym.file) else {
        // The symbol's file is not freshly covered → no runtime data.
        return (None, None);
    };

    let mut instrumented = 0i64;
    let mut covered = 0i64;
    for (_line_no, &hits) in lines.range(start..=end) {
        instrumented += 1;
        if hits > 0 {
            covered += 1;
        }
    }
    if instrumented == 0 {
        // The span carries no instrumented line — `n/a`, never a guessed 0%.
        return (None, None);
    }

    // Nearest-bp rounding on integers (deterministic across targets, [ADR-08]).
    let bp = (covered * super::BP_SCALE + instrumented / 2) / instrumented;
    let quadrant = Quadrant::classify(sym.reachable_from_test, bp > 0);
    (Some(bp), Some(quadrant))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn view(files: &[(&str, &[(i64, i64)])]) -> read::LineHitsView {
        let mut fresh_files: BTreeMap<String, BTreeMap<i64, i64>> = BTreeMap::new();
        for (path, lines) in files {
            fresh_files.insert(
                (*path).to_string(),
                lines.iter().copied().collect::<BTreeMap<i64, i64>>(),
            );
        }
        read::LineHitsView {
            head_sha: "head".into(),
            config_hash: "cfg".into(),
            fresh_files,
        }
    }

    fn sym(file: &str, start: Option<i64>, end: Option<i64>, reachable: bool) -> CrossSymbolInput {
        CrossSymbolInput {
            symbol: format!("sym:{file}:{start:?}"),
            name: "f".into(),
            file: file.into(),
            start_line: start,
            end_line: end,
            reachable_from_test: reachable,
        }
    }

    /// The four Gartner-numbered quadrants from `(reachable, executed)` ([CR-040]):
    /// trust top-right (Q4), false-green top-left (Q1), dead edge bottom-right (Q2),
    /// true gap bottom-left (Q3).
    #[test]
    fn classify_covers_the_four_quadrants() {
        assert_eq!(Quadrant::classify(true, true), Quadrant::Q4);
        assert_eq!(Quadrant::classify(true, false), Quadrant::Q2);
        assert_eq!(Quadrant::classify(false, true), Quadrant::Q1);
        assert_eq!(Quadrant::classify(false, false), Quadrant::Q3);
    }

    /// A reachable symbol with some executed lines is Q4 (trust); the fraction is
    /// nearest-bp covered ÷ instrumented over the span ([ADR-08]).
    #[test]
    fn reachable_and_executed_is_trust_q4_with_a_real_fraction() {
        // Span 1..=4: lines 1,2 hit, 3 unhit, 4 hit → 3/4 instrumented covered.
        let v = view(&[("src/a.rs", &[(1, 5), (2, 1), (3, 0), (4, 2)])]);
        let (bp, q) = attribute(&v, &sym("src/a.rs", Some(1), Some(4), true));
        assert_eq!(bp, Some(7500), "3 of 4 instrumented lines covered");
        assert_eq!(q, Some(Quadrant::Q4));
    }

    /// Reachable but every line in the span is 0-hit → Q2 (dead/guarded edge),
    /// fraction 0 (a *measured* 0, distinct from `n/a`).
    #[test]
    fn reachable_but_unexecuted_is_dead_edge_q2_with_measured_zero() {
        let v = view(&[("src/a.rs", &[(1, 0), (2, 0)])]);
        let (bp, q) = attribute(&v, &sym("src/a.rs", Some(1), Some(2), true));
        assert_eq!(bp, Some(0), "instrumented lines exist but none executed");
        assert_eq!(q, Some(Quadrant::Q2));
    }

    /// Not reachable but executed → Q1 (incidental / false-green, the worst cell).
    #[test]
    fn unreachable_but_executed_is_false_green_q1() {
        let v = view(&[("src/a.rs", &[(1, 3)])]);
        let (bp, q) = attribute(&v, &sym("src/a.rs", Some(1), Some(1), false));
        assert_eq!(bp, Some(10_000));
        assert_eq!(q, Some(Quadrant::Q1));
    }

    /// Not reachable and unexecuted → Q3 (true gap).
    #[test]
    fn unreachable_and_unexecuted_is_true_gap_q3() {
        let v = view(&[("src/a.rs", &[(1, 0)])]);
        let (bp, q) = attribute(&v, &sym("src/a.rs", Some(1), Some(1), false));
        assert_eq!(bp, Some(0));
        assert_eq!(q, Some(Quadrant::Q3));
    }

    /// `n/a` (never a guessed 0) for an unresolvable span, a file that is not
    /// freshly covered, and a span with no instrumented line ([NFR-RA-05]).
    #[test]
    fn na_when_span_unresolvable_or_uncovered_or_empty() {
        let v = view(&[("src/a.rs", &[(10, 1), (11, 0)])]);

        // No start/end line.
        assert_eq!(attribute(&v, &sym("src/a.rs", None, Some(4), true)), (None, None));
        assert_eq!(attribute(&v, &sym("src/a.rs", Some(1), None, true)), (None, None));
        // Inverted span.
        assert_eq!(attribute(&v, &sym("src/a.rs", Some(4), Some(1), true)), (None, None));
        // File not freshly covered.
        assert_eq!(attribute(&v, &sym("src/other.rs", Some(1), Some(4), true)), (None, None));
        // Span (1..=4) overlaps no instrumented line (data is at 10,11).
        assert_eq!(attribute(&v, &sym("src/a.rs", Some(1), Some(4), true)), (None, None));
    }

    /// With no snapshot the runtime axis is `n/a` for every symbol, the
    /// reachability axis is preserved, and the empty-state notice is set.
    #[test]
    fn no_coverage_yields_na_runtime_and_the_notice() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = coverage_cross(
            tmp.path(),
            vec![
                sym("src/a.rs", Some(1), Some(2), true),
                sym("src/b.rs", Some(1), Some(2), false),
            ],
        )
        .expect("cross over an empty store");

        assert_eq!(report.symbols.len(), 2);
        assert!(report.symbols.iter().all(|s| s.runtime_exec_bp.is_none()));
        assert!(report.symbols.iter().all(|s| s.quadrant.is_none()));
        // The reachability axis still carries its verdict.
        assert!(report.symbols.iter().any(|s| s.reachable_from_test));
        assert_eq!(report.totals.na_runtime, 2);
        assert_eq!(report.totals.total, 2);
        assert!(!report.has_fresh_coverage);
        assert_eq!(report.notice.as_deref(), Some(super::super::NO_COVERAGE_NOTICE));
        assert!(report.head_sha.is_none());
    }

    /// Symbols are emitted in canonical `(file, start_line, name, symbol)` order
    /// regardless of input order ([NFR-RA-06]).
    #[test]
    fn symbols_are_canonically_ordered() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = coverage_cross(
            tmp.path(),
            vec![
                sym("src/b.rs", Some(1), Some(2), false),
                sym("src/a.rs", Some(9), Some(9), false),
                sym("src/a.rs", Some(1), Some(2), false),
            ],
        )
        .unwrap();
        let order: Vec<(&str, Option<i64>)> = report
            .symbols
            .iter()
            .map(|s| (s.file.as_str(), s.start_line))
            .collect();
        assert_eq!(
            order,
            vec![("src/a.rs", Some(1)), ("src/a.rs", Some(9)), ("src/b.rs", Some(1))]
        );
    }
}
