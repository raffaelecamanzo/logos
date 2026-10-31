//! The **parse-damage warning** ([FR-IX-14], [CR-181], S-571): the languages
//! a large share of whose files were extracted from a damaged parse, named in
//! one `scan` warning.
//!
//! Tree-sitter recovers from a syntax error and the file is still extracted,
//! but around the damage a declaration's span and figures may be artefacts of
//! the misparse. Unpreprocessed C and C++ macros break a third of real files
//! this way; a per-file warning says so one file at a time, and this names the
//! language when the damage is widespread. `status` states every language's
//! figure ([`ParseDamage`]); the entries a damaged file contributes to the
//! worst-offender and "where to start" lists read `partial_parse: true`.
//!
//! # What decides it
//!
//! Both halves are read from the store, never re-derived, and both compared in
//! exact integer arithmetic, so a 9.99% share never rounds up into a warning:
//!
//! - **Widely damaged** — at least [`PARSE_DAMAGE_MIN_SHARE_PERCENT`] of the
//!   language's files that record their parse were extracted from a damaged
//!   one (its `status` row's `parse_damage`; a file indexed before the record
//!   existed counts toward neither side).
//! - **Large enough to name** — it holds at least
//!   [`COVERAGE_MIN_SHARE_PERCENT`](super::thin_graph::COVERAGE_MIN_SHARE_PERCENT)
//!   of the production callables: the bar the coverage warning names a
//!   language at, asked of the same [`ThinGraph`]
//!   ([`ThinGraph::large_enough_to_name`]), so the two cannot drift apart.
//!
//! `scan` reads the rows and the callables once and hands both warnings the
//! same reading.
//!
//! The warning never changes an exit code and never feeds the signal.
//!
//! [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
//! [CR-181]: ../../../docs/requests/CR-181-cpp-is-declared-as-limited-support-and-parse-damage-is-reported.md

use super::thin_graph::ThinGraph;
use crate::models::navigation::{LanguageResolution, ParseDamage};

/// The share of a language's files, in percent, at or above which its parse
/// damage is warned about ([FR-IX-14]). A fixed constant of the semantics,
/// stated in the warning itself.
///
/// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
pub const PARSE_DAMAGE_MIN_SHARE_PERCENT: u64 = 10;

/// One warning per language whose `status` row states a damaged share of at
/// least [`PARSE_DAMAGE_MIN_SHARE_PERCENT`] and which `thin` finds
/// [large enough to name](ThinGraph::large_enough_to_name), in row order —
/// pure, so every case is a constructible fixture. A row with no
/// `parse_damage` states none, and is never warned about.
#[must_use]
pub fn warnings(rows: &[LanguageResolution], thin: &ThinGraph) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let damage = row.parse_damage?;
            // The share is floored over the recorded files, so comparing it to
            // an integer threshold is the exact `damaged · 100 ≥ k · recorded`.
            let widely_damaged = damage.files > 0 && damage.share_percent >= PARSE_DAMAGE_MIN_SHARE_PERCENT;
            (widely_damaged && thin.large_enough_to_name(&row.language)).then(|| warning(&row.language, row.files - damage.unrecorded, damage))
        })
        .collect()
}

fn warning(language: &str, files: u64, damage: ParseDamage) -> String {
    format!(
        "parse damage: {} of {files} {language} files ({}%, warned at \
         {PARSE_DAMAGE_MIN_SHARE_PERCENT}% or more) were extracted from a parse with syntax \
         errors — their declarations' spans and figures may be artefacts of the misparse, \
         and their worst-offender and where-to-start entries read `partial_parse: true`",
        damage.files, damage.share_percent
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::navigation::RelationResolution;

    fn row(language: &str, files: u64, damaged: u64) -> LanguageResolution {
        LanguageResolution {
            language: language.to_string(),
            files,
            calls: RelationResolution::measured(0, 0, 0, 0),
            imports: RelationResolution::measured(0, 0, 0, 0),
            call_residue: None,
            header_ownership: None,
            parse_damage: Some(ParseDamage::of(damaged, files, 0)),
        }
    }

    /// The warnings over `rows`, with `callables` production callables per
    /// language — read into a [`ThinGraph`] as `scan` reads them.
    fn warned(rows: &[LanguageResolution], callables: &[(&str, u64)]) -> Vec<String> {
        let callables: Vec<(String, u64)> =
            callables.iter().map(|(language, count)| ((*language).to_string(), *count)).collect();
        warnings(rows, &ThinGraph::from_rows(rows, &callables))
    }

    #[test]
    fn a_large_widely_damaged_language_is_named_with_its_figures() {
        let warned = warned(&[row("cpp", 208, 67), row("rust", 10, 0)], &[("cpp", 90), ("rust", 10)]);
        assert_eq!(warned.len(), 1, "{warned:?}");
        assert!(
            warned[0].starts_with("parse damage: 67 of 208 cpp files (32%, warned at 10% or more)"),
            "{}",
            warned[0]
        );
    }

    /// Each half probed one step from qualifying: exactly 10% of the files
    /// warns and one file fewer does not; exactly 10% of the callables warns
    /// and one fewer does not.
    #[test]
    fn each_threshold_is_inclusive_and_exact() {
        let named = |damaged: u64, own: u64| {
            !warned(&[row("cpp", 100, damaged)], &[("cpp", own), ("rust", 100 - own)]).is_empty()
        };
        assert!(named(10, 50), "10 of 100 files is the threshold");
        assert!(!named(9, 50), "9 of 100 files is below it");
        assert!(named(50, 10), "10 of 100 callables is the threshold");
        assert!(!named(50, 9), "9 of 100 callables is below it");
    }

    #[test]
    fn the_share_is_floored_and_a_row_with_no_file_states_zero() {
        assert_eq!(ParseDamage::of(67, 208, 0).share_percent, 32);
        assert_eq!(ParseDamage::of(2, 3, 0).share_percent, 66, "floored, never rounded up");
        assert_eq!(ParseDamage::of(0, 0, 0), ParseDamage { files: 0, unrecorded: 0, share_percent: 0 });
        // Over the recorded files only: 1 of the 2 that record, not of all 10.
        assert_eq!(ParseDamage::of(1, 10, 8).share_percent, 50);
        assert_eq!(ParseDamage::of(0, 4, 4).share_percent, 0, "none records: no share");
    }

    #[test]
    fn a_small_language_a_clean_one_or_an_unstated_row_is_not_named() {
        // Damaged throughout, but a single callable of 1000.
        assert!(warned(&[row("c", 4, 4)], &[("c", 1), ("rust", 999)]).is_empty());
        // Every callable, no damage.
        assert!(warned(&[row("rust", 600, 0)], &[("rust", 5000)]).is_empty());
        // A language with files but no callables at all.
        assert!(warned(&[row("markdown", 10, 10)], &[("rust", 5)]).is_empty());
        // A row that states no parse damage (a relational answer's).
        let mut unstated = row("cpp", 10, 10);
        unstated.parse_damage = None;
        assert!(warned(&[unstated], &[("cpp", 5)]).is_empty());
    }
}
