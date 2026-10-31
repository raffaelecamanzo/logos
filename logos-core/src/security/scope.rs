//! A finding's code scope ([FR-SC-28], [ADR-79] §3a, S-795): `test`, `dev` or
//! `production`, decided each time the finding is read, like its OWASP class,
//! and never stored — so it follows the graph, and history.db gains nothing.
//!
//! A finding is `test` when either holds, checked in this order:
//!
//! - **`test_symbol`** — its enclosing graph node is `is_test` ([FR-AN-05]).
//!   The engine answers this from logos.db
//!   ([`in_test_symbols`](super::context::in_test_symbols)), by the same span
//!   lookup that binds the finding's enclosing symbol ([FR-SC-13]).
//! - **`test_path`** — the security path rule matches its project-relative
//!   location: the test path conventions ([`is_test_path`]) plus a `fixtures`,
//!   `testdata` or `__fixtures__` directory segment ([`is_test_location`]).
//!   The fixture segments are test code for security only: the rule metrics,
//!   hotspots and dead code read is unchanged.
//!
//! A finding that is not `test` is `dev` when it sits on a dependency its
//! lockfile marks dev-only ([`DevDependency`](ScopeReason::DevDependency),
//! [FR-SC-30], S-810): the dependency scope, read from the lockfile each time
//! (`security/lockfile.rs`). Test code wins, so a dev package's finding
//! in a fixtures lockfile reads `test`.
//!
//! Otherwise it is `production`, with no reason. A report path that is
//! absolute and under the root is made project-relative first, as a path
//! suppression reads it ([`project_location`]); a finding with no location, or
//! one outside the root, is `production`.
//!
//! **Documented limitation.** A historical, unbound or stale finding has no
//! current enclosing node, so the path rule alone classifies it: a secret
//! found only in history in a `src/` test module reads `production`.
//!
//! Every list defaults to production behind a [`ScopeFilter`], and every count
//! leaves test code out by [`counts`](crate::history::security::counts) and
//! states how many it left out. The default scope also leaves out a finding its
//! scanner marks low-confidence, "needs review" ([FR-SC-31]), and states how
//! many; `--scope all` lists it, labelled. So does a `dev` finding ([FR-SC-30]):
//! "N more in dev-only dependencies". The gate, the ratchet, the checked-in
//! baseline and the branch delta never read the scope ([BR-98], [BR-100]).
//!
//! [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
//! [FR-SC-13]: ../../../docs/specs/requirements/FR-SC-13.md
//! [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
//! [FR-SC-30]: ../../../docs/specs/requirements/FR-SC-30.md
//! [FR-SC-31]: ../../../docs/specs/requirements/FR-SC-31.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
//! [BR-98]: ../../../docs/specs/software-spec.md#328-security-findings
//! [BR-100]: ../../../docs/specs/software-spec.md#328-security-findings

use std::path::{Component, Path};

use anyhow::Result;
use serde::Serialize;

use crate::config::ConfigError;
use crate::history::security::{project_location, CodeScope};
use crate::history::SecurityFinding;
use crate::navigate::is_test_path;

use super::lockfile::LockfileCache;

/// Why a finding reads `test` or `dev`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeReason {
    /// Its enclosing graph node is test code.
    TestSymbol,
    /// Its project-relative location matches the security path rule.
    TestPath,
    /// Its lockfile marks the package it is on dev-only ([FR-SC-30]).
    ///
    /// [FR-SC-30]: ../../../docs/specs/requirements/FR-SC-30.md
    DevDependency,
}

/// A finding's code scope and why: what every finding payload carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ScopeLabel {
    pub code_scope: CodeScope,
    /// `None` exactly when the scope is `production`.
    pub code_scope_reason: Option<ScopeReason>,
}

/// The directory segments that mark test code for security only, beside the
/// test path conventions.
const FIXTURE_SEGMENTS: [&str; 3] = ["fixtures", "testdata", "__fixtures__"];

/// The security path rule over a project-relative, `/`-separated path: the
/// test path conventions ([`is_test_path`]), or a [fixture
/// segment](FIXTURE_SEGMENTS) among its directories — never its file name, so
/// `src/fixtures.rs` is production code while `src/fixtures/x.rs` is not.
#[must_use]
pub fn is_test_location(path: &str) -> bool {
    let directories = Path::new(path).parent().into_iter().flat_map(Path::components);
    is_test_path(path)
        || directories.into_iter().any(|c| matches!(c, Component::Normal(seg) if FIXTURE_SEGMENTS.iter().any(|f| seg == *f)))
}

/// The scope of `finding` in the project at `root`, `in_test_symbol` being
/// whether its enclosing graph node is test code ([module docs](self)).
/// `lockfiles` holds the lockfiles already read, so one is parsed once however
/// many findings sit on it.
#[must_use]
pub(crate) fn scope_of(root: &Path, finding: &SecurityFinding, in_test_symbol: bool, lockfiles: &mut LockfileCache) -> ScopeLabel {
    let location = finding.location().and_then(|location| project_location(root, &location));
    let test = if in_test_symbol {
        Some(ScopeReason::TestSymbol)
    } else {
        location.as_deref().filter(|path| is_test_location(path)).map(|_| ScopeReason::TestPath)
    };
    if let Some(reason) = test {
        return ScopeLabel { code_scope: CodeScope::Test, code_scope_reason: Some(reason) };
    }
    match location {
        Some(path) if lockfiles.is_dev(root, &path, finding) => {
            ScopeLabel { code_scope: CodeScope::Dev, code_scope_reason: Some(ScopeReason::DevDependency) }
        }
        _ => ScopeLabel { code_scope: CodeScope::Production, code_scope_reason: None },
    }
}

/// The scope of each finding, in order. `in_test_symbols` is the graph's
/// answer — whether each finding's enclosing node is test code, one per
/// finding in the same order — asked once, for every finding at once.
///
/// # Errors
/// `in_test_symbols`' own error, or an answer of the wrong length.
pub fn scopes(
    root: &Path,
    findings: &[&SecurityFinding],
    in_test_symbols: impl FnOnce(&[&SecurityFinding]) -> Result<Vec<bool>>,
) -> Result<Vec<ScopeLabel>> {
    let in_test = in_test_symbols(findings)?;
    anyhow::ensure!(
        in_test.len() == findings.len(),
        "the graph answered whether {} findings sit in test code for {} read",
        in_test.len(),
        findings.len()
    );
    let mut lockfiles = LockfileCache::default();
    Ok(findings.iter().zip(in_test).map(|(f, in_test)| scope_of(root, f, in_test, &mut lockfiles)).collect())
}

/// The answer of a read with no graph behind it: no finding sits in a test
/// symbol, so the path rule alone classifies each — what a read that never
/// shows a count passes, such as the gate's scanner states.
///
/// # Errors
/// Never; fallible to match the graph's answer.
pub fn no_test_symbols(findings: &[&SecurityFinding]) -> Result<Vec<bool>> {
    Ok(vec![false; findings.len()])
}

/// Which code scopes a findings list keeps ([FR-SC-08]).
///
/// The surfaces' tokens default to [`Production`](Self::Production)
/// ([`parse`](Self::parse) of no token); the type's own default keeps every
/// scope, as every other filter left unset keeps everything, so a read-model
/// built over `FindingsFilter::default()` — the branch delta's, the Security
/// page's, the roll-up's — sees every finding.
///
/// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeFilter {
    /// Production code only: what a surface lists when no scope is named. It
    /// leaves out a `dev` finding too ([FR-SC-30]), and states how many.
    ///
    /// [FR-SC-30]: ../../../docs/specs/requirements/FR-SC-30.md
    Production,
    /// Test code only.
    Test,
    /// Every finding.
    #[default]
    All,
}

impl ScopeFilter {
    /// The filter a surface's token names; no token is `production`.
    ///
    /// # Errors
    /// [`ConfigError::InvalidValue`] (exit 2) naming a token that is not
    /// `production`, `test` or `all`.
    pub fn parse(token: Option<&str>) -> Result<Self, ConfigError> {
        match token.map(str::to_ascii_lowercase).as_deref() {
            None | Some("production") => Ok(Self::Production),
            Some("test") => Ok(Self::Test),
            Some("all") => Ok(Self::All),
            Some(_) => Err(ConfigError::InvalidValue {
                key: "scope".to_string(),
                message: format!("{:?} is not one of production, test, all", token.unwrap_or_default()),
            }),
        }
    }

    /// Whether the listing leaves out a production finding its scanner marks
    /// low-confidence ([FR-SC-31]): the default scope does, so a list and a
    /// count read the same ("N more need review"); `test` and `all` do not.
    ///
    /// [FR-SC-31]: ../../../docs/specs/requirements/FR-SC-31.md
    #[must_use]
    pub fn hides_needs_review(self) -> bool {
        self == Self::Production
    }

    /// Whether the listing leaves out a finding on a dev-only dependency
    /// ([FR-SC-30]) and states it ("N more in dev-only dependencies"): the
    /// default scope does; `all` lists it; `test` lists test code alone, and
    /// counts it among the findings the scope hid.
    ///
    /// [FR-SC-30]: ../../../docs/specs/requirements/FR-SC-30.md
    #[must_use]
    pub fn hides_dev(self) -> bool {
        self == Self::Production
    }

    /// Whether a finding of `scope` passes. A `dev` finding passes `all` alone.
    #[must_use]
    pub fn keeps(self, scope: CodeScope) -> bool {
        match self {
            Self::All => true,
            Self::Production => scope == CodeScope::Production,
            Self::Test => scope == CodeScope::Test,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::security::FindingBinding;
    use crate::history::{FRESHNESS_FRESH, FRESHNESS_STALE};

    const ROOT: &str = "/work/project";

    fn finding(path: Option<&str>, report_path: Option<&str>) -> SecurityFinding {
        SecurityFinding {
            scanner_name: "Trivy".into(),
            scanner_version: "0.58.0".into(),
            head_sha: "0".repeat(40),
            fingerprint: "f".repeat(64),
            rule_id: "aws-access-key-id".into(),
            severity: crate::history::Severity::Critical,
            confidence: crate::history::ScannerConfidence::Unstated,
            cwes: Vec::new(),
            message: "secret-class finding redacted".into(),
            redacted: true,
            binding: if path.is_some() { FindingBinding::Bound } else { FindingBinding::Unmatched },
            report_path: report_path.or(path).map(str::to_string),
            path: path.map(str::to_string),
            start_line: Some(214),
            start_column: None,
            end_line: None,
            end_column: None,
            commits: Vec::new(),
            legacy_fingerprints: Vec::new(),
            anchor: None,
            freshness: if path.is_some() { FRESHNESS_FRESH } else { crate::history::FRESHNESS_NA },
            reporters: Vec::new(),
        }
    }

    fn read(f: &SecurityFinding, in_test_symbol: bool) -> (CodeScope, Option<ScopeReason>) {
        let label = scope_of(Path::new(ROOT), f, in_test_symbol, &mut LockfileCache::default());
        (label.code_scope, label.code_scope_reason)
    }

    const TEST_SYMBOL: (CodeScope, Option<ScopeReason>) = (CodeScope::Test, Some(ScopeReason::TestSymbol));
    const TEST_PATH: (CodeScope, Option<ScopeReason>) = (CodeScope::Test, Some(ScopeReason::TestPath));
    const PRODUCTION: (CodeScope, Option<ScopeReason>) = (CodeScope::Production, None);

    /// The shapes this repository's own findings take (CR-246 §2): a fake key
    /// in a `#[test]` fn in `src/`, a fixture Dockerfile, an absolute OSV
    /// lockfile path under the root, and a production file.
    #[test]
    fn each_shape_reads_its_scope_and_reason() {
        let mask = finding(Some("logos-core/src/history/security/mask.rs"), None);
        assert_eq!(read(&mask, true), TEST_SYMBOL, "an enclosing test fn");
        assert_eq!(read(&mask, false), PRODUCTION, "the same file, outside a test symbol");
        let dockerfile = finding(None, Some("logos-core/tests/fixtures/security-context/axum/Dockerfile"));
        assert_eq!(read(&dockerfile, false), TEST_PATH);
        let lockfile = finding(None, Some("/work/project/logos-core/tests/fixtures/security-context/axum/Cargo.lock"));
        assert_eq!(read(&lockfile, false), TEST_PATH, "an absolute path under the root is made relative first");
        let uri_spelling = finding(None, Some("./logos-core/tests/fixtures/axum/Cargo.lock"));
        assert_eq!(read(&uri_spelling, false), TEST_PATH, "a leading ./ is the same location");
        // Both hold: the symbol is the reason named.
        let in_tests = finding(Some("logos-core/tests/hotspots.rs"), None);
        assert_eq!(read(&in_tests, true), TEST_SYMBOL);
        assert_eq!(read(&in_tests, false), TEST_PATH);
    }

    /// No location, or one outside the root, is production — even when the
    /// path outside would match the rule.
    #[test]
    fn no_location_or_one_outside_the_root_is_production() {
        assert_eq!(read(&finding(None, None), false), PRODUCTION);
        let outside = finding(None, Some("/elsewhere/tests/fixtures/Cargo.lock"));
        assert_eq!(read(&outside, false), PRODUCTION, "outside the root");
        let sibling = finding(None, Some("/work/project-b/tests/Cargo.lock"));
        assert_eq!(read(&sibling, false), PRODUCTION, "a sibling sharing the root's prefix is outside it");
    }

    /// A historical or stale finding has no enclosing node, so the path rule
    /// alone decides: a secret found in history in `src/` reads production.
    #[test]
    fn a_historical_finding_in_src_reads_production_by_its_path() {
        let mut historical = finding(Some("logos-core/src/history/security/parse.rs"), None);
        historical.binding = FindingBinding::Historical;
        assert_eq!(read(&historical, false), PRODUCTION);
        let mut stale = finding(Some("src/tests/old.rs"), None);
        stale.freshness = FRESHNESS_STALE;
        assert_eq!(read(&stale, false), TEST_PATH, "a stale finding still has its path");
    }

    /// The fixture segments are directories only, and every near miss of one
    /// is production; the global test rule is unchanged by them.
    #[test]
    fn the_security_path_rule_adds_fixture_directories_and_nothing_else() {
        for test in [
            "src/fixtures/x.rs",
            "pkg/testdata/input.json",
            "web/__fixtures__/a.ts",
            "tests/unit.rs",
            "app/db_test.go",
            "a/b/fixtures/c/d.py",
        ] {
            assert!(is_test_location(test), "{test}");
        }
        for production in [
            "src/fixtures.rs",
            "src/fixture/x.rs",
            "src/myfixtures/x.rs",
            "src/fixtures_util/x.rs",
            "src/testdata.go",
            "src/__fixtures/x.ts",
            "fixtures",
            "src/main.rs",
        ] {
            assert!(!is_test_location(production), "{production}");
        }
        assert!(!is_test_path("src/fixtures/x.rs"), "metrics still count src/fixtures/x.rs as production");
    }

    /// Each token narrows to its scope; no token is production; any other
    /// token is the caller's mistake, named.
    #[test]
    fn the_scope_filter_parses_its_three_tokens_and_keeps_its_scope() {
        assert_eq!(ScopeFilter::parse(None).unwrap(), ScopeFilter::Production);
        for (token, filter) in [("production", ScopeFilter::Production), ("TEST", ScopeFilter::Test), ("All", ScopeFilter::All)] {
            assert_eq!(ScopeFilter::parse(Some(token)).unwrap(), filter, "{token}");
        }
        let bad = ScopeFilter::parse(Some("tests")).unwrap_err();
        assert!(bad.to_string().contains("\"tests\"") && bad.to_string().contains("production, test, all"), "{bad}");
        let keeps = |f: ScopeFilter| [CodeScope::Production, CodeScope::Test].map(|s| f.keeps(s));
        assert_eq!(keeps(ScopeFilter::Production), [true, false]);
        assert_eq!(keeps(ScopeFilter::Test), [false, true]);
        assert_eq!(keeps(ScopeFilter::All), [true, true]);
        let dev = |f: ScopeFilter| f.keeps(CodeScope::Dev);
        assert_eq!([ScopeFilter::Production, ScopeFilter::Test, ScopeFilter::All].map(dev), [false, false, true]);
        assert_eq!([ScopeFilter::Production, ScopeFilter::Test, ScopeFilter::All].map(ScopeFilter::hides_dev), [true, false, false]);
        assert_eq!(ScopeFilter::default(), ScopeFilter::All, "an unset filter keeps every scope");
    }

    /// S-810 (FR-SC-30): a finding on a package its lockfile marks dev-only reads
    /// `dev` with its reason; the same advisory on a production package, or in a
    /// lockfile that says nothing, reads `production`; a fixtures lockfile reads
    /// `test` first. The lockfile is the project's own, read from the root.
    #[test]
    fn a_dev_only_dependency_reads_dev_unless_it_is_test_code_or_the_lockfile_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let lock = r#"{"lockfileVersion":3,"packages":{"node_modules/vite":{"version":"5.0.0","dev":true},"node_modules/react":{"version":"18.2.0"}}}"#;
        std::fs::create_dir_all(dir.path().join("web/tests/fixtures")).unwrap();
        std::fs::write(dir.path().join("web/package-lock.json"), lock).unwrap();
        std::fs::write(dir.path().join("web/tests/fixtures/package-lock.json"), lock).unwrap();
        let on = |package: &str, report: &str| {
            let mut f = finding(None, Some(report));
            f.message = format!("Package '{package}' is vulnerable to 'GHSA-x' (also known as 'CVE-1').");
            f
        };
        let read = |f: &SecurityFinding| {
            let label = scope_of(dir.path(), f, false, &mut LockfileCache::default());
            (label.code_scope, label.code_scope_reason)
        };
        let dev = (CodeScope::Dev, Some(ScopeReason::DevDependency));
        assert_eq!(read(&on("vite@5.0.0", "web/package-lock.json")), dev);
        assert_eq!(read(&on("react@18.2.0", "web/package-lock.json")), PRODUCTION);
        let absolute = format!("{}/web/package-lock.json", dir.path().display());
        assert_eq!(read(&on("vite@5.0.0", &absolute)), dev, "an absolute report path under the root");
        assert_eq!(read(&on("vite@5.0.0", "web/Cargo.lock")), PRODUCTION, "another lockfile never guesses");
        assert_eq!(read(&on("vite@5.0.0", "web/tests/fixtures/package-lock.json")), TEST_PATH, "test code wins");
        assert_eq!(read(&finding(Some("web/src/app.ts"), None)), PRODUCTION, "a source finding has no dependency scope");
        let in_test = scope_of(dir.path(), &on("vite@5.0.0", "web/package-lock.json"), true, &mut LockfileCache::default());
        assert_eq!((in_test.code_scope, in_test.code_scope_reason), TEST_SYMBOL);
    }

    /// The graph is asked once, for every finding, and an answer of the wrong
    /// length is an error, never a silent misalignment.
    #[test]
    fn scopes_asks_the_graph_once_and_refuses_a_short_answer() {
        let a = finding(Some("src/a.rs"), None);
        let b = finding(Some("src/b.rs"), None);
        let mut asked = 0;
        let labels = scopes(Path::new(ROOT), &[&a, &b], |f| {
            asked += 1;
            Ok(vec![false, f.len() == 2])
        })
        .unwrap();
        assert_eq!(asked, 1);
        assert_eq!(labels.iter().map(|l| l.code_scope).collect::<Vec<_>>(), [CodeScope::Production, CodeScope::Test]);
        assert!(scopes(Path::new(ROOT), &[&a, &b], |_| Ok(vec![true])).is_err());
        assert_eq!(no_test_symbols(&[&a, &b]).unwrap(), [false, false]);
    }
}
