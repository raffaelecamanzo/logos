//! The outcome counts every `stats` usage cell carries ([FR-OB-14]).
//!
//! A telemetry event records what a call **answered**, not only that it ran:
//! one of a closed four-value vocabulary (`answered` / `empty` / `unresolved` /
//! `failed`), or `NULL` when the tool has no outcome vocabulary or the row
//! predates the column. The read-model folds that into two counts per cell —
//! how many calls answered, and how many were classified at all — and ships
//! **both**, never a rate. A consumer divides `answered_calls` by
//! `classified_calls`; dividing by `calls` instead would make every
//! unclassified tool drag the figure towards zero, so the figure would measure
//! how many tools opted in rather than how often the graph answered.
//!
//! A cell with nothing classified has no rate to show, and says so in words
//! from the closed absence vocabulary ([`absence`]) rather than as a `0%` the
//! data does not support ([NFR-CC-04]).
//!
//! The vocabulary itself is telemetry's, not the model's: it lives beside the
//! emission seam in `observability::tool`, because no read-model ships an
//! individual outcome — only these two counts.
//!
//! [`absence`]: crate::models::quality::absence
//! [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::ops::AddAssign;

use serde::ser::{Serialize, SerializeStruct, Serializer};

/// What a usage cell with no classified call says in place of a rate.
///
/// It is the lexicon's `none recorded` as written ([`absence`]): the condition
/// `classified_calls == 0` establishes that no outcome was recorded for any
/// call in the cell, and nothing about **why** — the tool may have no outcome
/// vocabulary, or every call in the window may predate migration v4, or the
/// window may reach only rolled-up days from before it. Rule R1 therefore
/// names no cause.
///
/// [`absence`]: crate::models::quality::absence
pub const OUTCOME_ABSENCE: &str = "none recorded";

/// The answered and classified counts of one usage cell ([FR-OB-14]).
///
/// Serialised **flattened** into its cell as three fields: `answered_calls`,
/// `classified_calls`, and `outcome_absence` — `null` when the cell has a
/// classified call, [`OUTCOME_ABSENCE`] when it has none. The absence is derived
/// from the count at serialisation rather than stored beside it, so the two can
/// never disagree however a cell was assembled, summed or merged.
///
/// Invariant, held by every producer: `answered_calls <= classified_calls <=`
/// the cell's `calls`.
///
/// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutcomeCounts {
    /// Calls whose recorded outcome is `answered`.
    pub answered_calls: u64,
    /// Calls with **any** recorded outcome — the denominator an answered rate
    /// is divided by.
    pub classified_calls: u64,
}

impl OutcomeCounts {
    /// The named absence this cell renders in place of a rate, if it has one.
    pub fn absence(&self) -> Option<&'static str> {
        (self.classified_calls == 0).then_some(OUTCOME_ABSENCE)
    }
}

impl AddAssign for OutcomeCounts {
    fn add_assign(&mut self, other: Self) {
        self.answered_calls += other.answered_calls;
        self.classified_calls += other.classified_calls;
    }
}

impl Serialize for OutcomeCounts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut cell = serializer.serialize_struct("OutcomeCounts", 3)?;
        cell.serialize_field("answered_calls", &self.answered_calls)?;
        cell.serialize_field("classified_calls", &self.classified_calls)?;
        cell.serialize_field("outcome_absence", &self.absence())?;
        cell.end()
    }
}
