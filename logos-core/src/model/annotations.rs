//! [`Annotations`] — free-form key→value metadata attached to a node.
//!
//! This is the *generic* annotation bag (the codec-level model type). The
//! typed, first-class annotation columns the analysis passes compute
//! (`cyclomatic_complexity`, `line_count`, `is_dead`, `is_duplicate`,
//! `layer_membership` — [FR-AN-04]) live with `graph-store`/`annotation-engine`
//! in later stories; this map carries any additional plugin- or
//! extractor-supplied attributes that do not warrant a dedicated column.
//!
//! Backed by a [`BTreeMap`] rather than a `HashMap`: Logos requires
//! deterministic, reproducible output ([NFR-RA-06]), and `HashMap`'s
//! per-process-randomised iteration order would leak non-determinism into any
//! serialised annotation set. `BTreeMap` yields stable, sorted-by-key order for
//! free.
//!
//! [FR-AN-04]: ../../../../docs/specs/requirements/FR-AN-04.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// An ordered map of annotation `key → value` pairs attached to a node.
///
/// Serialises as a plain JSON object with keys in sorted order (deterministic,
/// [NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Annotations(BTreeMap<String, String>);

impl Annotations {
    /// Create an empty annotation set.
    pub fn new() -> Self {
        Annotations(BTreeMap::new())
    }

    /// Insert or overwrite an annotation, returning the previous value if any.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.0.insert(key.into(), value.into())
    }

    /// Look up an annotation value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// `true` if no annotations are present.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of annotations.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate annotations in deterministic (sorted-by-key) order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl FromIterator<(String, String)> for Annotations {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Annotations(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_empty() {
        let ann = Annotations::new();
        assert!(ann.is_empty());
        assert_eq!(ann.len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut ann = Annotations::new();
        assert_eq!(ann.insert("visibility", "pub"), None);
        assert_eq!(
            ann.insert("visibility", "pub(crate)"),
            Some("pub".to_string())
        );
        assert_eq!(ann.get("visibility"), Some("pub(crate)"));
        assert_eq!(ann.get("missing"), None);
        assert_eq!(ann.len(), 1);
    }

    #[test]
    fn iteration_is_sorted_by_key_for_determinism() {
        let mut ann = Annotations::new();
        ann.insert("zeta", "1");
        ann.insert("alpha", "2");
        ann.insert("mu", "3");
        let keys: Vec<&str> = ann.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["alpha", "mu", "zeta"]);
    }

    #[test]
    fn serialises_as_a_sorted_object() {
        let mut ann = Annotations::new();
        ann.insert("zeta", "1");
        ann.insert("alpha", "2");
        let json = serde_json::to_string(&ann).unwrap();
        // BTreeMap guarantees alpha before zeta regardless of insertion order.
        assert_eq!(json, r#"{"alpha":"2","zeta":"1"}"#);
    }

    #[test]
    fn round_trips_through_serde() {
        let mut ann = Annotations::new();
        ann.insert("k", "v");
        let json = serde_json::to_string(&ann).unwrap();
        let back: Annotations = serde_json::from_str(&json).unwrap();
        assert_eq!(ann, back);
    }

    #[test]
    fn collects_from_pairs() {
        let ann: Annotations = [
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(ann.len(), 2);
        assert_eq!(ann.get("a"), Some("1"));
    }
}
