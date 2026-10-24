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
//! # One binder for every evidence kind ([FR-SC-02], [ADR-79])
//! Security findings bind their SARIF paths with this same mapper rather than a
//! second spelling of the rule. [`PathMapper::bind`] is the general answer — it
//! says *why* a path did not bind (no candidate, or how many candidates tied),
//! which the security ingest reports as "unmatched with its candidates count";
//! [`PathMapper::map`] is the coverage view of it, the bound path or nothing.
//!
//! [FR-CV-03]: ../../../../docs/specs/requirements/FR-CV-03.md
//! [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
//! [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md

use crate::resolve::go_module::GoModule;

/// How one report path bound — the general answer [`PathMapper::bind`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Binding<'a> {
    /// Exactly one indexed file binds.
    Bound(&'a str),
    /// No indexed file shares a complete suffix with the report path.
    Unmatched,
    /// `candidates` (two or more) indexed files tie — never guessed between.
    Ambiguous {
        /// How many indexed files the report path is a complete suffix of.
        candidates: usize,
    },
}

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
        match self.bind(report_path) {
            Binding::Bound(path) => Some(path),
            Binding::Unmatched | Binding::Ambiguous { .. } => None,
        }
    }

    /// Bind one report path, saying why when it does not: the rule [`map`] applies,
    /// with the tie count kept instead of collapsed to "nothing".
    ///
    /// [`map`]: Self::map
    pub(crate) fn bind(&self, report_path: &str) -> Binding<'a> {
        let normalized = self.strip(&normalize(report_path));
        let target = components(&normalized);
        if target.is_empty() {
            return Binding::Unmatched;
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
            return Binding::Bound(p.original);
        }

        // 2. Fallback: the report path is a complete suffix of an indexed path.
        //    A unique candidate binds; any ambiguity is unmatched, never guessed.
        let mut candidates = self
            .indexed
            .iter()
            .filter(|idx| is_suffix(&target, &idx.components));
        match (candidates.next(), candidates.count()) {
            (None, _) => Binding::Unmatched,
            (Some(only), 0) => Binding::Bound(only.original),
            (Some(_), others) => Binding::Ambiguous {
                candidates: others + 1,
            },
        }
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

/// Rewrite a Go coverprofile path — an import path, `<module path>/<dir>/<file>.go`
/// — to the project-relative path it names, using the `go.mod` modules the tree
/// declares ([FR-CV-12]): the module path is stripped at a component boundary and
/// the module's own directory put in its place, so a nested module's file reads
/// `svc/pkg/a.go`, not a bare `pkg/a.go` that a sibling module could share.
/// `modules` is longest module path first ([`go_module::discover`]), so the most
/// specific module claims the path. `None` when no declared module is a prefix;
/// [`super::ingest`] then reports the path unmatched when the tree declares any
/// module (a foreign module's file is not a project file), and hands it verbatim
/// to the suffix mapper only when the tree has no `go.mod` at all.
///
/// [FR-CV-12]: ../../../../docs/specs/requirements/FR-CV-12.md
/// [`go_module::discover`]: crate::resolve::go_module::discover
pub(crate) fn strip_go_module(modules: &[GoModule], report_path: &str) -> Option<String> {
    modules.iter().find_map(|m| {
        let rest = report_path
            .strip_prefix(m.path.as_str())?
            .strip_prefix('/')?;
        (!rest.is_empty()).then(|| {
            if m.root.is_empty() {
                rest.to_string()
            } else {
                format!("{}/{rest}", m.root)
            }
        })
    })
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

    /// `bind` keeps the reason `map` collapses: an ambiguous basename reports how
    /// many indexed files tied ([FR-SC-02] "unmatched with its candidates count"),
    /// a path sharing nothing reports no candidate, and a bound path is the same
    /// one `map` returns.
    #[test]
    fn bind_reports_the_tie_count_map_collapses() {
        let idx = indexed();
        let m = PathMapper::new(&idx, &[]);
        // A bare basename is a complete suffix of both `src/util.rs` and
        // `tests/util.rs`: a real tie.
        assert_eq!(m.bind("util.rs"), Binding::Ambiguous { candidates: 2 });
        // A directory no indexed file has is no candidate at all — `map` reads
        // `None` here for that reason, not for a tie.
        assert_eq!(m.bind("/somewhere/util.rs"), Binding::Unmatched);
        assert_eq!(m.bind("/etc/passwd"), Binding::Unmatched);
        assert_eq!(m.bind(""), Binding::Unmatched, "an empty path has nothing to bind");
        assert_eq!(
            m.bind("/home/ci/build/project/src/util.rs"),
            Binding::Bound("src/util.rs")
        );
        // Three same-suffix files tie three ways, not "two or more".
        let three = vec![
            "a/mod.py".to_string(),
            "b/mod.py".to_string(),
            "c/mod.py".to_string(),
        ];
        assert_eq!(
            PathMapper::new(&three, &[]).bind("mod.py"),
            Binding::Ambiguous { candidates: 3 }
        );
    }

    fn module(root: &str, path: &str) -> GoModule {
        GoModule {
            root: root.to_string(),
            path: path.to_string(),
        }
    }

    /// The module prefix is stripped at a component boundary and the module's own
    /// directory restored; the longest module claims a path two modules prefix;
    /// a module whose text merely *starts* the path (`app` vs `app-tools`) does
    /// not; no declared module means no rewrite ([FR-CV-12]).
    #[test]
    fn go_module_prefix_strips_at_a_component_boundary() {
        // Longest module path first, as `go_module::discover` returns them.
        let nested = [
            module("tools", "example.com/app/tools"),
            module("", "example.com/app"),
        ];
        assert_eq!(
            strip_go_module(&nested, "example.com/app/pkg/a.go").as_deref(),
            Some("pkg/a.go"),
            "root module: prefix stripped"
        );
        assert_eq!(
            strip_go_module(&nested, "example.com/app/tools/gen/g.go").as_deref(),
            Some("tools/gen/g.go"),
            "the nested module claims it and restores its directory"
        );
        let root_only = [module("", "example.com/app")];
        assert_eq!(
            strip_go_module(&root_only, "example.com/app-tools/gen/g.go"),
            None,
            "`app` is not a component-boundary prefix of `app-tools`"
        );
        assert_eq!(
            strip_go_module(&root_only, "example.com/app"),
            None,
            "no file part"
        );
        assert_eq!(strip_go_module(&[], "example.com/app/pkg/a.go"), None);
        assert_eq!(
            strip_go_module(
                &[module("svc", "example.com/svc-two")],
                "example.com/svc-two/p/a.go"
            )
            .as_deref(),
            Some("svc/p/a.go"),
            "a module path that does not spell its directory still resolves"
        );
    }
}
