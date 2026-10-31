//! The **thin-graph helper** ([FR-QM-18], [CR-171], S-523): which languages
//! the graph cannot see across files, and the one coverage warning every
//! scoring surface carries about them.
//!
//! A language whose resolver binds no call and no import across a file
//! boundary contributes no edge between modules, so the three graph dimensions
//! — Modularity, Acyclicity and Depth — cannot see its structure. When such a
//! language is a large part of the production code, a signal that reads
//! healthy says nothing about it. The metrics engine already drops the graph
//! dimensions when the whole graph is too thin
//! ([`MIN_CROSS_MODULE_EDGES`](crate::metrics::MIN_CROSS_MODULE_EDGES)); this
//! names the languages behind a thin graph, and a thin corner of a graph that
//! is otherwise not.
//!
//! # One helper, every surface
//!
//! `scan` (and so `scan --json` and the MCP `scan`), `gate`, `session-start`
//! and the session-start readout all take their warning from
//! [`ThinGraph::warnings`], so no two surfaces can phrase it, threshold it or
//! order it differently. The per-language verdict is public too
//! ([`ThinGraph::same_file_only`]): the branch review labels each entry with
//! the trust its language earns from the same reading ([S-677]).
//!
//! # What decides it
//!
//! Both halves are read from the store, never re-derived:
//!
//! - **Binds same-file only** — the language's `Calls` and `Imports` rows of
//!   the per-language resolution readout ([FR-RS-09],
//!   [`coverage_by_language`](crate::resolve::coverage_by_language)) carry no
//!   cross-file figure, and at least one names the absence
//!   `SameFileOnly` or `NoResolvedEdges` — references were recorded and none
//!   bound across a file. A language that recorded nothing to resolve
//!   (`NoReferencesRecorded` on both) is not reported: there is nothing the
//!   resolver failed to see.
//! - **Large enough to name** — it holds at least
//!   [`COVERAGE_MIN_SHARE_PERCENT`] of the production callables
//!   ([`GraphStore::production_callables_by_language`]), compared in exact
//!   integer arithmetic so a 9.99% share never rounds up into a warning.
//!
//! The warning never changes an exit code and never feeds the signal: it is
//! advisory text beside a verdict computed without it.
//!
//! [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
//! [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
//! [CR-171]: ../../../docs/requests/CR-171-the-quality-signal-and-test-classification-stay-honest-on-a-thin-graph.md
//! [S-677]: ../../../docs/planning/journal.md#s-677-logos-review---base-classifies-every-offender-per-symbol-against-the-merge-base

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use crate::graph_store::GraphStore;
use crate::models::navigation::{LanguageResolution, RelationResolution};
use crate::models::quality::CrossFileAbsence;

/// The share of the production callables, in percent, at or above which a
/// language that binds same-file only is named in the coverage warning
/// ([FR-QM-18]). A fixed constant of the semantics, stated in the warning
/// itself so a reader knows a smaller language may sit below it.
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
pub const COVERAGE_MIN_SHARE_PERCENT: u64 = 10;

/// One language's production size and whether the graph sees it across files —
/// the **measured** reach, as opposed to the reach a plugin declares
/// ([`crate::models::quality::LanguageReach`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LanguageCoverage {
    /// The language as `files.language` records it, e.g. `"python"`.
    pub language: String,
    /// Its production callables ([`GraphStore::production_callables_by_language`]).
    pub production_callables: u64,
    /// `Some` when the language binds same-file only (see the module docs): the
    /// `Calls` absence when that is what establishes it, otherwise the
    /// `Imports` one. `None` when it binds across files, or recorded nothing
    /// to bind.
    pub same_file_only: Option<CrossFileAbsence>,
}

/// The per-language cross-file reach of one store, and the warnings it earns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThinGraph {
    /// Production callables over every language — the share's denominator.
    pub production_callables: u64,
    /// One row per language the resolution readout reports, in language order.
    pub languages: Vec<LanguageCoverage>,
}

impl ThinGraph {
    /// Read both halves from `store` — the resolution readout and the
    /// production callables per language — and combine them.
    ///
    /// # Errors
    /// Returns an error if the graph cannot be read.
    pub fn read(store: &dyn GraphStore) -> Result<Self> {
        let resolution = crate::resolve::coverage_by_language(store)?;
        let callables = store.production_callables_by_language()?;
        Ok(Self::from_rows(&resolution, &callables))
    }

    /// Combine a resolution readout with the production callables per language
    /// — pure, so every case is a constructible fixture. A language with
    /// callables but no resolution row binds nothing it recorded and is kept
    /// with no verdict; one with a row but no callables counts zero.
    #[must_use]
    pub fn from_rows(resolution: &[LanguageResolution], callables: &[(String, u64)]) -> Self {
        let mut languages: BTreeMap<&str, LanguageCoverage> = BTreeMap::new();
        for row in resolution {
            languages.insert(
                &row.language,
                LanguageCoverage {
                    language: row.language.clone(),
                    production_callables: 0,
                    same_file_only: same_file_only(&row.calls, &row.imports),
                },
            );
        }
        for (language, count) in callables {
            languages
                .entry(language)
                .or_insert_with(|| LanguageCoverage {
                    language: language.clone(),
                    production_callables: 0,
                    same_file_only: None,
                })
                .production_callables = *count;
        }
        Self {
            production_callables: callables.iter().map(|(_, count)| count).sum(),
            languages: languages.into_values().collect(),
        }
    }

    /// The verdict for `language`: `Some` with the absence that establishes it
    /// when the language binds same-file only, `None` when it binds across
    /// files, recorded nothing to bind, or is not in the store. The trust label
    /// a per-entry report attaches ([S-677]).
    ///
    /// [S-677]: ../../../docs/planning/journal.md#s-677-logos-review---base-classifies-every-offender-per-symbol-against-the-merge-base
    #[must_use]
    pub fn same_file_only(&self, language: &str) -> Option<CrossFileAbsence> {
        self.languages
            .iter()
            .find(|row| row.language == language)
            .and_then(|row| row.same_file_only)
    }

    /// The languages the coverage warning names: same-file only **and** at
    /// least [`COVERAGE_MIN_SHARE_PERCENT`] of the production callables, in
    /// language order.
    pub fn warned(&self) -> impl Iterator<Item = &LanguageCoverage> {
        self.languages
            .iter()
            .filter(|row| row.same_file_only.is_some() && self.large_enough(row))
    }

    /// Whether `language` holds at least [`COVERAGE_MIN_SHARE_PERCENT`] of the
    /// production callables — the bar every warning naming a language holds
    /// it to, the parse-damage warning's ([`super::parse_damage`]) included.
    /// `false` for a language not in the store.
    #[must_use]
    pub fn large_enough_to_name(&self, language: &str) -> bool {
        self.languages
            .iter()
            .find(|row| row.language == language)
            .is_some_and(|row| self.large_enough(row))
    }

    fn large_enough(&self, row: &LanguageCoverage) -> bool {
        row.production_callables > 0
            && row.production_callables * 100 >= self.production_callables * COVERAGE_MIN_SHARE_PERCENT
    }

    /// One coverage warning per [`warned`](Self::warned) language, naming the
    /// language, its share and the threshold ([FR-QM-18]) — the text every
    /// surface carries. Empty when no language qualifies.
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        self.warned()
            .map(|row| {
                // Floored, never rounded: a qualifying share prints at least the
                // threshold, and no figure is ever stated higher than it is.
                let share = row.production_callables * 100 / self.production_callables;
                let cause = row.same_file_only.map(absence_tag).unwrap_or_default();
                format!(
                    "coverage: {} is {share}% of production callables (warned at \
                     {COVERAGE_MIN_SHARE_PERCENT}% or more) and binds no call or import across \
                     files ({cause}) — Modularity, Acyclicity and Depth cannot see its \
                     cross-module structure",
                    row.language
                )
            })
            .collect()
    }
}

/// The same-file-only verdict over one language's two relation classes: no
/// cross-file figure in either, and an absence that says references went
/// unbound across files — `Calls` first, the class `callers` and `impact`
/// traverse.
fn same_file_only(calls: &RelationResolution, imports: &RelationResolution) -> Option<CrossFileAbsence> {
    if calls.cross_file_edges.is_some() || imports.cross_file_edges.is_some() {
        return None;
    }
    [calls, imports]
        .into_iter()
        .filter_map(|class| class.cross_file_absence)
        .find(|absence| {
            matches!(
                absence,
                CrossFileAbsence::SameFileOnly { .. } | CrossFileAbsence::NoResolvedEdges { .. }
            )
        })
}

/// An absence's wire tag, read from its own serialisation so the warning
/// speaks the one spelling `status` does rather than restating it.
pub(crate) fn absence_tag(absence: CrossFileAbsence) -> String {
    serde_json::to_value(absence)
        .ok()
        .and_then(|value| value.get("cause")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One relation class's row, through the one constructor the readout uses.
    fn class(references: u64, same_file: u64, cross_file: u64) -> RelationResolution {
        RelationResolution::measured(references, same_file + cross_file, same_file, cross_file)
    }

    fn language(name: &str, calls: RelationResolution, imports: RelationResolution) -> LanguageResolution {
        LanguageResolution {
            language: name.to_string(),
            files: 1,
            calls,
            imports,
            call_residue: None,
            header_ownership: None,
            parse_damage: None,
        }
    }

    /// A language that binds within files only, and one that binds across.
    fn mixed(python: u64, rust: u64) -> ThinGraph {
        ThinGraph::from_rows(
            &[
                language("python", class(40, 12, 0), class(9, 0, 0)),
                language("rust", class(80, 30, 25), class(10, 0, 7)),
            ],
            &[("python".to_string(), python), ("rust".to_string(), rust)],
        )
    }

    #[test]
    fn a_same_file_only_language_at_ten_percent_is_named_with_its_share_and_the_threshold() {
        let graph = mixed(10, 90);
        let warnings = graph.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("coverage: python is 10% of production callables"), "{}", warnings[0]);
        assert!(warnings[0].contains("10% or more"), "the threshold is stated: {}", warnings[0]);
        assert!(
            warnings[0].contains(&format!("({})", absence_tag(CrossFileAbsence::SameFileOnly { same_file_edges: 12 }))),
            "the cause is the status readout's own tag: {}",
            warnings[0]
        );
    }

    #[test]
    fn a_fractional_share_prints_floored_never_rounded_up() {
        // 3 of 28 is 10.7%: named, and printed as 10%, never 11%.
        let warnings = mixed(3, 25).warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("coverage: python is 10% of production callables"), "{}", warnings[0]);
    }

    #[test]
    fn a_share_one_callable_below_ten_percent_is_not_named() {
        // 9 of 91 is 9.89%: exact integer comparison, never rounded up.
        assert!(mixed(9, 82).warnings().is_empty());
        assert_eq!(mixed(10, 90).warned().count(), 1, "exactly 10% is named");
    }

    #[test]
    fn a_language_binding_any_call_or_import_across_files_is_not_same_file_only() {
        let graph = mixed(50, 50);
        assert_eq!(graph.same_file_only("rust"), None);
        // One cross-file import alone clears it, as one cross-file call does.
        let imports_only = ThinGraph::from_rows(
            &[language("ruby", class(40, 12, 0), class(9, 0, 1))],
            &[("ruby".to_string(), 10)],
        );
        assert_eq!(imports_only.same_file_only("ruby"), None);
        assert!(imports_only.warnings().is_empty());
    }

    #[test]
    fn the_verdict_names_the_calls_absence_first_and_unbound_references_qualify() {
        let graph = mixed(50, 50);
        assert_eq!(
            graph.same_file_only("python"),
            Some(CrossFileAbsence::SameFileOnly { same_file_edges: 12 }),
            "Calls establishes it"
        );
        // References recorded, none bound: the NoResolvedEdges arm qualifies.
        let unbound = ThinGraph::from_rows(
            &[language("c", class(30, 0, 0), class(0, 0, 0))],
            &[("c".to_string(), 30)],
        );
        assert_eq!(
            unbound.same_file_only("c"),
            Some(CrossFileAbsence::NoResolvedEdges { references: 30, bound: 0 })
        );
        assert_eq!(unbound.warnings().len(), 1);
    }

    #[test]
    fn a_language_that_recorded_nothing_to_bind_is_never_named() {
        let data = ThinGraph::from_rows(
            &[language("yaml", class(0, 0, 0), class(0, 0, 0))],
            &[("yaml".to_string(), 5)],
        );
        assert_eq!(data.same_file_only("yaml"), None);
        assert!(data.warnings().is_empty());
    }

    #[test]
    fn no_callables_names_no_language_and_the_order_is_by_language() {
        assert!(ThinGraph::from_rows(&[language("c", class(3, 1, 0), class(0, 0, 0))], &[]).warnings().is_empty());
        let two = ThinGraph::from_rows(
            &[
                language("ruby", class(5, 2, 0), class(0, 0, 0)),
                language("c", class(5, 2, 0), class(0, 0, 0)),
            ],
            &[("ruby".to_string(), 5), ("c".to_string(), 5)],
        );
        let named: Vec<&str> = two.warned().map(|row| row.language.as_str()).collect();
        assert_eq!(named, ["c", "ruby"]);
    }
}
