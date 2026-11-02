//! The soundness check of the name test (i) (S-635, S-835, [FR-RS-49]): a row
//! the binder bound is never one the name test would call provably external.
//!
//! A bound row whose target names nothing in the universe is a counterexample:
//! the classifier would call a reference of that shape external when it is
//! unbound, though a binder rule just proved such a shape can bind here. The
//! check reads one graph through the universe the classifier itself builds
//! ([`read_population`]), so it judges exactly the rule `status` applies.
//!
//! Two callers share this one check: the reach-fixture test
//! `bound_rows_are_never_named_nowhere`, run in every gate, and the
//! `LOGOS_REF_WORKSPACE` estate harness `bound_row_soundness_estate`, run by
//! hand over copies of real indexed stores.
//!
//! [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use super::{read_population, NAME_BOUND};
use crate::graph_store::GraphStore;
use crate::model::{EdgeKind, RefForm};
use crate::resolve::package_key::PackageLayout;

/// How many paths one `file_languages` read binds — well under SQLite's
/// 32,766 host parameters per statement.
const LANGUAGE_CHUNK: usize = 10_000;

/// One language's reading of the check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LanguageSoundness {
    /// Bound rows of a name-bound kind — the rows the check judges.
    pub bound_judged: u64,
    /// Of those, the ones the name test would call provably external.
    pub named_nowhere: u64,
    /// Reported `Calls` rows, bound or not, carrying an alias: a receiver
    /// binding or a marker. A store indexed before calls recorded one reads 0,
    /// and then says nothing about whether call aliases are needed.
    pub call_aliases: u64,
}

impl std::ops::AddAssign<&LanguageSoundness> for LanguageSoundness {
    /// Add `other`'s readings to these: the one sum of a reading's fields,
    /// for [`BoundRowSoundness::total`] and for a caller summing stores.
    fn add_assign(&mut self, other: &LanguageSoundness) {
        self.bound_judged += other.bound_judged;
        self.named_nowhere += other.named_nowhere;
        self.call_aliases += other.call_aliases;
    }
}

/// A bound row the name test would call provably external.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedNowhereRow {
    /// Its file's language, if the file records one.
    pub language: Option<String>,
    /// The edge kind it bound as.
    pub kind: EdgeKind,
    /// Its target text.
    pub target: String,
}

/// One graph's reading of the check, per language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundRowSoundness {
    /// Per file language; rows of a file recording none are keyed `None`.
    pub by_language: BTreeMap<Option<String>, LanguageSoundness>,
    /// Every bound row named nowhere, in ledger order.
    pub named_nowhere: Vec<NamedNowhereRow>,
}

impl BoundRowSoundness {
    /// The readings of every language, summed.
    #[must_use]
    pub fn total(&self) -> LanguageSoundness {
        self.by_language.values().fold(LanguageSoundness::default(), |mut sum, row| {
            sum += row;
            sum
        })
    }
}

/// Run the check over one graph: every reported ledger row (capture-before-
/// delete rows excluded, as the classifier excludes them) that is bound and of
/// a name-bound kind is judged against the universe [`read_population`]
/// builds with `layout`'s file facades.
///
/// # Errors
/// Returns an error if the graph cannot be read.
#[doc(hidden)]
pub fn bound_row_soundness(store: &dyn GraphStore, layout: &PackageLayout) -> Result<BoundRowSoundness> {
    let (_, universe) = read_population(store, layout)?;
    let files = store.indexed_files()?;
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    // `file_languages` binds one SQL parameter per path, and SQLite binds at
    // most 32,766 in one statement: a larger store is read in chunks.
    let mut by_path = BTreeMap::new();
    for chunk in paths.chunks(LANGUAGE_CHUNK) {
        by_path.extend(store.file_languages(chunk)?);
    }
    let language_of: HashMap<i64, Option<String>> = files
        .into_iter()
        .map(|f| {
            let language = by_path.get(&f.path).cloned().flatten();
            (f.id, language)
        })
        .collect();
    let mut soundness = BoundRowSoundness::default();
    for row in store.unresolved_refs()? {
        if row.form == RefForm::Symbol {
            continue;
        }
        let language = row.file_id.and_then(|id| language_of.get(&id).cloned().flatten());
        let reading = soundness.by_language.entry(language.clone()).or_default();
        if row.kind == EdgeKind::Calls && row.alias.is_some() {
            reading.call_aliases += 1;
        }
        if !row.resolved || !NAME_BOUND.contains(&row.kind) {
            continue;
        }
        reading.bound_judged += 1;
        if !universe.could_be_internal(language.as_deref(), i64::from(row.kind.as_i32()), &row.target) {
            reading.named_nowhere += 1;
            soundness.named_nowhere.push(NamedNowhereRow {
                language,
                kind: row.kind,
                target: row.target,
            });
        }
    }
    Ok(soundness)
}
