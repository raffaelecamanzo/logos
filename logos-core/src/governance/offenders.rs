//! `logos offenders` and its MCP twin `offenders` ([FR-QM-19], S-676): every
//! offender the last snapshot recorded, uncapped, narrowed by path and
//! dimension.
//!
//! A pure read of the last snapshot ([ADR-28]): it reconciles, scores and
//! persists nothing, so it answers "what did the last measurement flag", and a
//! rerun over an unchanged store is byte-identical ([NFR-RA-06]). The capped
//! lists, the signal, the gate and the thresholds hash are untouched — this
//! reads rows the snapshot path already wrote.
//!
//! [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
//! [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use anyhow::Result;
use globset::GlobSet;

use crate::config::ConfigError;
use crate::engine::Engine;
use crate::models::quality::{ListedOffender, OffenderListing, WorstOffenders};

use super::quality_runtime;

/// The narrowing a surface asked for: a path glob and a dimension, both
/// optional; when both are given an entry must satisfy both.
#[derive(Debug, Clone, Default)]
pub struct OffendersFilter {
    /// The glob as given, and its compiled matcher.
    path: Option<(String, GlobSet)>,
    /// One of [`WorstOffenders::DIMENSIONS`].
    dimension: Option<&'static str>,
}

impl OffendersFilter {
    /// A filter from the tokens a surface was given.
    ///
    /// The glob is matched against the project-relative file of each entry
    /// with the documentation-glob semantics: `*` stays within one directory
    /// and `**` crosses them, so `src/a/**` holds everything under `src/a` and
    /// nothing under `src/ab`. A Depth row names directories, not a file, so a
    /// path filter never lists one.
    ///
    /// The token is normalised first, as [`FindingsFilter`] normalises its
    /// path: `\` reads `/`, and a leading `./` and a trailing `/` are dropped.
    /// An empty token is no filter. A token without glob metacharacters names a
    /// file or a directory and holds that file and everything under it, so
    /// `src/a` and `src/a/` mean `src/a/**`. Without this, each of those reads
    /// as a filter that matches nothing, and its empty `recorded` list would
    /// read as clean ([NFR-CC-04]).
    ///
    /// [`FindingsFilter`]: crate::security::FindingsFilter
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] (exit 2) for a dimension that is not one
    /// of the nine with a list; [`ConfigError::BadGlob`] or
    /// [`ConfigError::EscapingPattern`] (exit 2) for a glob that does not
    /// compile or leaves the project root.
    pub fn parse(path: Option<&str>, dimension: Option<&str>) -> Result<Self, ConfigError> {
        let dimension = dimension
            .map(|token| {
                let lowered = token.to_ascii_lowercase();
                WorstOffenders::DIMENSIONS
                    .into_iter()
                    .find(|d| *d == lowered)
                    .ok_or_else(|| ConfigError::InvalidValue {
                        key: "dimension".to_string(),
                        message: format!(
                            "{token:?} is not one of {}",
                            WorstOffenders::DIMENSIONS.join(", ")
                        ),
                    })
            })
            .transpose()?;
        let path = path
            .map(|token| token.replace('\\', "/"))
            .map(|token| token.trim_start_matches("./").trim_end_matches('/').to_string())
            .filter(|glob| !glob.is_empty())
            .map(|glob| {
                let patterns = if glob.contains(['*', '?', '[', '{']) {
                    vec![glob.clone()]
                } else {
                    vec![glob.clone(), format!("{glob}/**")]
                };
                crate::config::globs::compile_anchored(&patterns).map(|set| (glob, set))
            })
            .transpose()?;
        Ok(Self { path, dimension })
    }

    /// Whether an entry of `dimension` defined in `file` passes the filter.
    fn admits(&self, dimension: &str, file: &str) -> bool {
        self.dimension.is_none_or(|d| d == dimension)
            && self
                .path
                .as_ref()
                .is_none_or(|(_, set)| !file.is_empty() && set.is_match(file))
    }
}

/// The listing behind [`Engine::offenders`]: the last snapshot's uncapped
/// lists, flattened in [`WorstOffenders::DIMENSIONS`] order and each list's
/// computed order, then narrowed by `filter`.
pub(crate) fn offenders(engine: &Engine, filter: &OffendersFilter) -> Result<OffenderListing> {
    let latest = quality_runtime(engine)?.submit_read(|store| store.latest_uncapped_offenders())?;
    Ok(listing(latest, filter))
}

/// Project the store's answer through `filter` — pure, so the not-recorded
/// readings are testable without a store.
fn listing(latest: Option<WorstOffenders>, filter: &OffendersFilter) -> OffenderListing {
    let mut listing = OffenderListing {
        path: filter.path.as_ref().map(|(glob, _)| glob.clone()),
        dimension: filter.dimension,
        ..OffenderListing::default()
    };
    let Some(lists) = latest else {
        listing.not_recorded_because = Some(OffenderListing::NEVER_SCANNED);
        return listing;
    };
    if !lists.values_recorded {
        listing.not_recorded_because = Some(OffenderListing::PREDATES_UNCAPPED);
        return listing;
    }
    listing.recorded = true;
    listing.offenders = lists
        .lists()
        .into_iter()
        .flat_map(|(dimension, list)| {
            list.iter()
                .filter(move |o| filter.admits(dimension, &o.file))
                .map(move |o| ListedOffender {
                    dimension,
                    offender: o.clone(),
                })
        })
        .collect();
    listing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::quality::Offender;

    fn entry(name: &str, file: &str) -> Offender {
        Offender {
            name: name.to_string(),
            file: file.to_string(),
            node_id: Some(format!("id/{name}")),
            measure: Some("nesting_depth"),
            value: Some(5),
            threshold: Some(4),
            ..Offender::default()
        }
    }

    fn recorded() -> WorstOffenders {
        WorstOffenders {
            recorded: true,
            values_recorded: true,
            nesting: vec![entry("a1", "src/a/x.rs"), entry("ab", "src/ab/y.rs"), entry("a2", "src/a/b/z.rs")],
            equality: vec![entry("eq", "src/a/x.rs")],
            depth: vec![Offender {
                name: "src".to_string(),
                ..Offender::default()
            }],
            ..WorstOffenders::default()
        }
    }

    fn names(l: &OffenderListing) -> Vec<(&str, &str)> {
        l.offenders.iter().map(|o| (o.dimension, o.offender.name.as_str())).collect()
    }

    #[test]
    fn a_glob_admits_its_subtree_and_not_its_near_miss_sibling() {
        let filter = OffendersFilter::parse(Some("src/a/**"), None).unwrap();
        let l = listing(Some(recorded()), &filter);
        assert!(l.recorded);
        assert_eq!(names(&l), [("nesting", "a1"), ("nesting", "a2"), ("equality", "eq")]);
    }

    #[test]
    fn a_dimension_narrows_and_both_filters_intersect() {
        let filter = OffendersFilter::parse(Some("src/a/**"), Some("NESTING")).unwrap();
        let l = listing(Some(recorded()), &filter);
        assert_eq!(names(&l), [("nesting", "a1"), ("nesting", "a2")]);
        assert_eq!(l.dimension, Some("nesting"));
        assert_eq!(l.path.as_deref(), Some("src/a/**"));
    }

    #[test]
    fn no_filter_lists_every_dimension_in_canonical_order_including_depth() {
        let l = listing(Some(recorded()), &OffendersFilter::default());
        assert_eq!(
            names(&l),
            [("nesting", "a1"), ("nesting", "ab"), ("nesting", "a2"), ("depth", "src"), ("equality", "eq")]
        );
    }

    /// A path written the ways people write one names the same subtree as
    /// its glob: a leading `./`, a bare directory, a trailing `/`, backslashes.
    /// None of them may read as a filter that matches nothing, whose empty
    /// recorded list would read as clean (NFR-CC-04); an empty token is no
    /// filter at all.
    #[test]
    fn a_path_written_as_a_directory_or_with_separators_names_its_subtree() {
        let under_a = [("nesting", "a1"), ("nesting", "a2"), ("equality", "eq")];
        for token in ["./src/a/**", "src/a", "src/a/", "./src/a/", "src\\a", "src\\a\\**"] {
            let l = listing(Some(recorded()), &OffendersFilter::parse(Some(token), None).unwrap());
            assert_eq!(names(&l), under_a, "{token:?}");
        }
        let echoed = OffendersFilter::parse(Some("./src\\a/"), None).unwrap();
        assert_eq!(listing(Some(recorded()), &echoed).path.as_deref(), Some("src/a"));
        let file = OffendersFilter::parse(Some("src/a/x.rs"), None).unwrap();
        assert_eq!(names(&listing(Some(recorded()), &file)), [("nesting", "a1"), ("equality", "eq")]);
        let all = listing(Some(recorded()), &OffendersFilter::default());
        for empty in ["", "./"] {
            let l = listing(Some(recorded()), &OffendersFilter::parse(Some(empty), None).unwrap());
            assert_eq!((l.path.as_deref(), l.offenders.len()), (None, all.offenders.len()), "{empty:?}");
        }
    }

    /// A Depth row names a chain of directories and has no file, so even a
    /// catch-all glob never lists it — a path filter asks about files.
    #[test]
    fn a_catch_all_glob_never_lists_a_depth_row() {
        for glob in ["**", "*"] {
            let l = listing(Some(recorded()), &OffendersFilter::parse(Some(glob), None).unwrap());
            assert!(l.offenders.iter().all(|o| o.dimension != "depth"), "{glob:?}: {:?}", names(&l));
        }
        let everything = listing(Some(recorded()), &OffendersFilter::parse(Some("**"), None).unwrap());
        assert_eq!(everything.offenders.len(), 4, "every entry with a file: {:?}", names(&everything));
    }

    #[test]
    fn an_unknown_dimension_and_an_escaping_glob_are_refused() {
        let err = OffendersFilter::parse(None, Some("modularity")).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { .. }), "{err}");
        let err = OffendersFilter::parse(Some("../x/**"), None).unwrap_err();
        assert!(matches!(err, ConfigError::EscapingPattern { .. }), "{err}");
        let err = OffendersFilter::parse(Some("src/[a"), None).unwrap_err();
        assert!(matches!(err, ConfigError::BadGlob { .. }), "{err}");
    }

    #[test]
    fn a_missing_or_pre_migration_snapshot_reads_not_recorded() {
        let never = listing(None, &OffendersFilter::default());
        assert!(!never.recorded);
        assert_eq!(never.not_recorded_because, Some(OffenderListing::NEVER_SCANNED));

        // A pre-37 snapshot: capped lists recorded, values not.
        let old = WorstOffenders {
            recorded: true,
            values_recorded: false,
            nesting: vec![entry("a1", "src/a/x.rs")],
            ..WorstOffenders::default()
        };
        let old = listing(Some(old), &OffendersFilter::default());
        assert!(!old.recorded);
        assert_eq!(old.not_recorded_because, Some(OffenderListing::PREDATES_UNCAPPED));
        assert!(old.offenders.is_empty(), "nothing is listed from a snapshot that never persisted the list");
    }
}
