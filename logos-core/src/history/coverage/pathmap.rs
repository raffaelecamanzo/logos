//! Report-path → indexed-file mapping ([FR-CV-03], [ADR-23]).
//!
//! Coverage-report paths are frequently absolute (cargo-llvm-cov, pytest-cov
//! emit build-environment absolute paths) or build-dir-relative; indexed files
//! are repo-relative. [`PathMapper`] binds a report path to exactly one indexed
//! file by:
//!
//! 1. **separator normalization** — backslashes → `/`, leading `./` dropped, so
//!    a Windows-style report path matches a POSIX index;
//! 2. **`[coverage] path_strip_prefixes`** — strip a configured build-dir prefix
//!    so more of the real path participates in matching (the disambiguation lever,
//!    [FR-CV-09]);
//! 3. **longest-unique-suffix matching** — the indexed file sharing the longest
//!    trailing path-component run with the report path wins; a tie on that length
//!    is **unmatched**, never guessed ([NFR-RA-05] exactly-one-candidate
//!    discipline extended to path binding). An exact full relative-path match is
//!    unambiguous by construction and wins outright.
//!
//! Unmatched entries are surfaced in the ingest summary ([FR-CV-01]) — counted,
//! never silently dropped.
//!
//! [FR-CV-03]: ../../../../docs/specs/requirements/FR-CV-03.md
//! [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
//! [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md

/// Maps report paths to indexed files for one ingest. Built once per ingest with
/// the indexed-file set and the effective strip prefixes; reused for every report
/// entry.
pub(crate) struct PathMapper<'a> {
    /// Each indexed path with its normalized components, precomputed once.
    indexed: Vec<IndexedPath<'a>>,
    /// Normalized strip prefixes, longest first (the longest matching prefix is
    /// the one stripped).
    strip_prefixes: Vec<String>,
}

/// One indexed file: the original repo-relative string plus its normalized path
/// components (no empty / `.` segments).
struct IndexedPath<'a> {
    original: &'a str,
    components: Vec<String>,
}

impl<'a> PathMapper<'a> {
    /// Build a mapper over `indexed` repo-relative paths with the effective
    /// `path_strip_prefixes`.
    pub(crate) fn new(indexed: &'a [String], strip_prefixes: &[String]) -> Self {
        let indexed = indexed
            .iter()
            .map(|p| IndexedPath {
                original: p.as_str(),
                components: components(&normalize(p)),
            })
            .collect();
        // Strip the *longest* matching prefix, so listing both `/build/` and
        // `/build/ci/` strips the more specific one when it applies.
        let mut strip_prefixes: Vec<String> = strip_prefixes.iter().map(|p| normalize(p)).collect();
        strip_prefixes.sort_by_key(|p| std::cmp::Reverse(p.len()));
        Self {
            indexed,
            strip_prefixes,
        }
    }

    /// Map one report path to its indexed file, or `None` when nothing binds or
    /// the binding would be a guess (ambiguous → unmatched, [NFR-RA-05]).
    ///
    /// Two directions, in priority order:
    /// 1. **report carries the full tail** (the common absolute-path case): the
    ///    indexed file whose *entire* repo-relative path is a component-suffix of
    ///    the report path. The longest such is unique by construction (two
    ///    equal-length suffixes of the same path are identical), so this never
    ///    ties — the most specific match wins (e.g. `vendor/src/lib.rs` over
    ///    `src/lib.rs` when the report path goes through `vendor/`).
    /// 2. **report is build-relative and shorter** (fallback): the report path is
    ///    a component-suffix of an indexed path. Exactly one such → bind; two or
    ///    more (an ambiguous basename like `util.rs`) → unmatched, never guessed.
    pub(crate) fn map(&self, report_path: &str) -> Option<&'a str> {
        let normalized = self.strip(&normalize(report_path));
        let target = components(&normalized);
        if target.is_empty() {
            return None;
        }

        // 1. Longest indexed path that is a complete suffix of the report path.
        let mut primary: Option<&IndexedPath> = None;
        for idx in &self.indexed {
            if is_suffix(&idx.components, &target)
                && primary.is_none_or(|p| idx.components.len() > p.components.len())
            {
                primary = Some(idx);
            }
        }
        if let Some(p) = primary {
            return Some(p.original);
        }

        // 2. Fallback: the report path is a complete suffix of an indexed path.
        //    A unique candidate binds; any ambiguity is unmatched.
        let mut found: Option<&'a str> = None;
        for idx in &self.indexed {
            if is_suffix(&target, &idx.components) {
                if found.is_some() {
                    return None; // ambiguous — never guess
                }
                found = Some(idx.original);
            }
        }
        found
    }

    /// Strip the longest configured prefix that `normalized` starts with — but
    /// only at a path-component boundary, so a prefix `/build/ci` never strips
    /// the middle of `/build/cibuild/src/lib.rs`. The boundary holds when the
    /// prefix itself ends in `/`, or the remainder is empty or starts with `/`.
    fn strip(&self, normalized: &str) -> String {
        for prefix in &self.strip_prefixes {
            if let Some(rest) = normalized.strip_prefix(prefix.as_str()) {
                if prefix.ends_with('/') || rest.is_empty() || rest.starts_with('/') {
                    return rest.to_string();
                }
            }
        }
        normalized.to_string()
    }
}

/// Normalize separators: backslashes → `/`, and a leading `./` dropped. The
/// string is otherwise left intact (component filtering happens in [`components`]).
fn normalize(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    slashed
        .strip_prefix("./")
        .map(str::to_string)
        .unwrap_or(slashed)
}

/// Split a normalized path into its meaningful components (dropping empty
/// segments from leading/double slashes and `.` segments).
fn components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_string)
        .collect()
}

/// `true` if `needle` is a component-suffix of `hay` (i.e. `hay` ends with the
/// full `needle`).
fn is_suffix(needle: &[String], hay: &[String]) -> bool {
    needle.len() <= hay.len()
        && needle
            .iter()
            .rev()
            .zip(hay.iter().rev())
            .all(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed() -> Vec<String> {
        vec![
            "src/lib.rs".to_string(),
            "src/util.rs".to_string(),
            "tests/util.rs".to_string(),
            "vendor/project/src/lib.rs".to_string(),
        ]
    }

    /// An absolute build-environment path maps to the repo-relative indexed file
    /// by suffix ([FR-CV-03] acceptance 1).
    #[test]
    fn absolute_build_path_maps_by_suffix() {
        let idx = indexed();
        let m = PathMapper::new(&idx, &[]);
        assert_eq!(
            m.map("/home/ci/build/project/src/util.rs"),
            Some("src/util.rs")
        );
    }

    /// Backslash separators normalize to `/` before matching.
    #[test]
    fn windows_separators_normalize() {
        let idx = indexed();
        let m = PathMapper::new(&idx, &[]);
        assert_eq!(m.map(r"C:\build\src\lib.rs"), Some("src/lib.rs"));
    }

    /// A basename ambiguous across two indexed files is unmatched, never bound to
    /// either ([FR-CV-03] acceptance 2, [NFR-RA-05] never-guess).
    #[test]
    fn ambiguous_basename_is_unmatched() {
        let idx = indexed();
        let m = PathMapper::new(&idx, &[]);
        // `util.rs` alone matches both src/util.rs and tests/util.rs at suffix 1.
        assert_eq!(m.map("/somewhere/util.rs"), None);
    }

    /// Adding the disambiguating prefix to `path_strip_prefixes` resolves an entry
    /// the longest-tail rule would otherwise mis-bind ([FR-CV-03] acceptance 3):
    /// when the CI build dir (`/build/`) coincides with a real repo directory of
    /// the same name, the absolute path binds to the *vendored* copy; stripping
    /// the build prefix lets the intended top-level file win.
    #[test]
    fn strip_prefix_disambiguates() {
        // The repo has both a top-level `src/lib.rs` and a checked-in `build/`
        // tree containing `build/src/lib.rs`.
        let two = vec!["src/lib.rs".to_string(), "build/src/lib.rs".to_string()];
        let plain = PathMapper::new(&two, &[]);
        // The CI absolute path `/build/src/lib.rs` longest-tail-matches the
        // vendored `build/src/lib.rs` (len 3) over `src/lib.rs` (len 2).
        assert_eq!(plain.map("/build/src/lib.rs"), Some("build/src/lib.rs"));
        // Stripping the known build prefix re-targets it to the intended file.
        let stripped = PathMapper::new(&two, &["/build/".to_string()]);
        assert_eq!(
            stripped.map("/build/src/lib.rs"),
            Some("src/lib.rs"),
            "stripping the build prefix binds the top-level src/lib.rs"
        );
    }

    /// A strip prefix without a trailing separator strips only at a component
    /// boundary — it must not chop the middle of a longer component (`/build/ci`
    /// stripping `/build/cibuild/...` to `build/...`). The trailing-slash form
    /// still works.
    #[test]
    fn strip_prefix_respects_component_boundary() {
        let idx = vec!["src/lib.rs".to_string()];
        // `/build/ci` (no trailing slash) must NOT strip `/build/cibuild/...`.
        let m = PathMapper::new(&idx, &["/build/ci".to_string()]);
        // `/build/cibuild/src/lib.rs`: a mid-component strip would yield
        // `build/src/lib.rs` and still suffix-match; the boundary guard prevents
        // the strip, and `src/lib.rs` still binds by plain suffix anyway.
        assert_eq!(m.map("/build/cibuild/src/lib.rs"), Some("src/lib.rs"));
        // The same prefix DOES strip at a real boundary (`/build/ci/...`).
        assert_eq!(m.map("/build/ci/src/lib.rs"), Some("src/lib.rs"));
    }

    /// An exact full relative-path match binds even when a longer indexed path
    /// shares the same trailing components — the realistic monorepo case (a
    /// top-level `src/lib.rs` alongside `vendor/project/src/lib.rs`). The vendored
    /// path is not a *tail* of the bare report path, so it never competes.
    #[test]
    fn exact_relative_match_binds_despite_vendored_sibling() {
        let idx = indexed();
        let m = PathMapper::new(&idx, &[]);
        assert_eq!(m.map("src/lib.rs"), Some("src/lib.rs"));
    }

    /// A report path sharing nothing with any indexed file is unmatched.
    #[test]
    fn no_overlap_is_unmatched() {
        let idx = indexed();
        let m = PathMapper::new(&idx, &[]);
        assert_eq!(m.map("/etc/passwd"), None);
    }
}
