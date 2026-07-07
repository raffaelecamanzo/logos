//! Coverage-artifact **conventions, admission matcher, and discovery**
//! ([FR-CV-10], [CR-036], [ADR-38]).
//!
//! Coverage is a *runtime* artifact — Logos never produces it (never runs a test
//! suite, [NFR-SE-01], [ADR-38]); it only ingests one an external run wrote. This
//! module names the conventional places that artifact lands and compiles the
//! matcher the watcher uses to recognize it as it changes:
//!
//! - [`CONVENTIONS`] — the built-in artifact paths watched out of the box, zero
//!   config in the common case.
//! - [`ArtifactMatcher`] — conventions ∪ the optional `[coverage_ingest]
//!   .artifact_glob` override, compiled once into a [`globset::GlobSet`]. The
//!   watcher consults it to admit an artifact as an **allow-list exception** over
//!   the `target/`-class ignore filter ([FR-CF-05]) — *without* ever weakening the
//!   `.logos/`/`.git/` feedback-loop filters, which the watcher checks first.
//! - [`discover`] — the opt-in `coverage refresh` resolves which artifact a just-
//!   run `refresh_cmd` produced (newest existing convention/literal path), to
//!   ingest it.
//!
//! Several conventions live under directories the indexer ignores (`target/`,
//! `coverage/` are in `ignored_dirs`); that is the whole point — the artifact is
//! build output, and re-admitting *only* the artifact path is the bounded hole
//! [ADR-38] sanctions.
//!
//! [FR-CV-10]: ../../../../docs/specs/requirements/FR-CV-10.md
//! [FR-CF-05]: ../../../../docs/specs/requirements/FR-CF-05.md
//! [NFR-SE-01]: ../../../../docs/specs/requirements/NFR-SE-01.md
//! [ADR-38]: ../../../../docs/specs/architecture/decisions/ADR-38.md
//! [CR-036]: ../../../../docs/requests/CR-036-automatic-coverage-ingest-and-coverage-cross-quadrant.md

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::GlobSet;

use crate::config::EffectiveCoverageIngest;

/// The built-in conventional coverage-artifact paths the watcher admits with no
/// configuration ([FR-CV-10]): the common cargo-llvm-cov / lcov / pytest-cov /
/// nyc output locations. Root-relative, `/`-joined. Several sit under a
/// `target/`-class ignored dir on purpose — the artifact is build output, and the
/// allow-list exception re-admits exactly these paths ([ADR-38]).
pub(crate) const CONVENTIONS: &[&str] = &[
    "lcov.info",
    "coverage.xml",
    "coverage/cobertura-coverage.xml",
    "coverage/lcov.info",
    "target/llvm-cov/cobertura.xml",
    "target/llvm-cov/lcov.info",
];

/// A compiled matcher recognizing a coverage artifact by its root-relative path:
/// the built-in [`CONVENTIONS`] unioned with the optional `[coverage_ingest]
/// .artifact_glob` override, compiled once ([FR-CV-10]).
#[derive(Debug, Clone)]
pub(crate) struct ArtifactMatcher {
    set: GlobSet,
}

impl ArtifactMatcher {
    /// Compile the conventions ∪ configured globs into one matcher.
    ///
    /// The conventions are exact relative paths (no wildcards); the configured
    /// globs use the non-anchored code-glob semantics (`*` crosses `/`, so a
    /// directory artifact set is written `dir/**`), validated for containment at
    /// config load ([NFR-SE-04]).
    ///
    /// # Errors
    /// Propagates a [`ConfigError`](crate::config::ConfigError) if a configured
    /// glob fails to compile — the watcher treats that as a degraded start and
    /// serves without the artifact hook ([FR-SY-06]).
    pub(crate) fn compile(cfg: &EffectiveCoverageIngest) -> Result<Self, crate::config::ConfigError> {
        let mut patterns: Vec<String> = CONVENTIONS.iter().map(|s| (*s).to_string()).collect();
        patterns.extend(cfg.artifact_glob.iter().cloned());
        Ok(Self {
            set: crate::config::compile_globs(&patterns)?,
        })
    }

    /// Whether the root-relative `rel` path is a coverage artifact.
    pub(crate) fn matches_relative(&self, rel: &Path) -> bool {
        // globset matches against a `/`-joined string; normalize separators so a
        // Windows-style path still compares against the `/`-written conventions.
        let joined = rel
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => s.to_str(),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        self.set.is_match(&joined)
    }
}

/// Resolve which coverage artifact to ingest after `coverage refresh` ran the
/// `refresh_cmd` ([FR-CV-10]): the newest existing file among the built-in
/// conventions and any **literal** (wildcard-free) `[coverage_ingest]
/// .artifact_glob` entries, under `root`.
///
/// Returns the absolute path of the newest candidate, or `None` when none of the
/// known locations exist on disk (the caller turns that into a loud "ran but
/// produced no recognizable artifact" error — never a fabricated ingest,
/// [NFR-RA-05]). Wildcard globs are not walked here — a `refresh_cmd` pairs with a
/// known output path, which is literal — so discovery stays a bounded set of stat
/// calls off the serve path.
pub(crate) fn discover(root: &Path, cfg: &EffectiveCoverageIngest) -> Option<PathBuf> {
    let mut candidates: Vec<String> = CONVENTIONS.iter().map(|s| (*s).to_string()).collect();
    // Literal (wildcard-free) configured globs are concrete artifact paths.
    for glob in &cfg.artifact_glob {
        if !glob.contains(['*', '?', '[', '{']) {
            candidates.push(glob.clone());
        }
    }

    candidates
        .into_iter()
        .map(|rel| root.join(rel))
        .filter(|p| p.is_file())
        .max_by_key(|p| {
            // Newest by mtime; an unreadable mtime sorts oldest (UNIX_EPOCH) so a
            // readable candidate always wins. Deterministic given the tree state.
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(globs: &[&str]) -> ArtifactMatcher {
        ArtifactMatcher::compile(&EffectiveCoverageIngest {
            artifact_glob: globs.iter().map(|s| (*s).to_string()).collect(),
            format: "auto".to_string(),
            refresh_cmd: None,
        })
        .expect("matcher compiles")
    }

    #[test]
    fn conventions_match_out_of_the_box() {
        let m = matcher(&[]);
        for conv in CONVENTIONS {
            assert!(
                m.matches_relative(Path::new(conv)),
                "the built-in convention {conv} must be recognized"
            );
        }
        // A llvm-cov artifact under the ignored `target/` dir is still recognized
        // — the allow-list exception (the admission filter applies it, FR-CV-10).
        assert!(m.matches_relative(Path::new("target/llvm-cov/cobertura.xml")));
    }

    #[test]
    fn non_artifacts_do_not_match() {
        let m = matcher(&[]);
        assert!(!m.matches_relative(Path::new("src/main.rs")));
        assert!(!m.matches_relative(Path::new("target/debug/deps/foo.rlib")));
        // A same-named file in the wrong place is not a convention match.
        assert!(!m.matches_relative(Path::new("src/coverage.xml")));
    }

    #[test]
    fn configured_glob_extends_the_conventions() {
        let m = matcher(&["ci/**/*.lcov"]);
        // Conventions still match…
        assert!(m.matches_relative(Path::new("lcov.info")));
        // …and the configured glob admits a custom artifact path.
        assert!(m.matches_relative(Path::new("ci/reports/unit.lcov")));
        assert!(!m.matches_relative(Path::new("ci/reports/unit.txt")));
    }

    fn cfg(globs: &[&str]) -> EffectiveCoverageIngest {
        EffectiveCoverageIngest {
            artifact_glob: globs.iter().map(|s| (*s).to_string()).collect(),
            format: "auto".to_string(),
            refresh_cmd: None,
        }
    }

    #[test]
    fn discover_returns_none_when_no_artifact_exists() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            discover(tmp.path(), &cfg(&[])).is_none(),
            "nothing on disk → no artifact to ingest (the caller errs, never guesses)"
        );
    }

    #[test]
    fn discover_finds_a_conventional_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("lcov.info"), "x").unwrap();
        assert_eq!(
            discover(tmp.path(), &cfg(&[])),
            Some(tmp.path().join("lcov.info")),
            "an existing convention artifact is discovered with no config"
        );
    }

    #[test]
    fn discover_includes_a_literal_configured_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("build")).unwrap();
        std::fs::write(tmp.path().join("build/cov.xml"), "x").unwrap();
        assert_eq!(
            discover(tmp.path(), &cfg(&["build/cov.xml"])),
            Some(tmp.path().join("build/cov.xml")),
            "a literal (wildcard-free) configured artifact path is a candidate"
        );
    }

    #[test]
    fn discover_excludes_wildcard_globs_from_candidates() {
        // The literal-only rule (line ~111): a wildcard `artifact_glob` is NOT
        // walked by discover. A file matching only the wildcard — with no
        // convention and no literal candidate present — yields None, so `coverage
        // refresh` errs loudly rather than guessing. Regression guard for that rule.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("ci/reports")).unwrap();
        std::fs::write(tmp.path().join("ci/reports/unit.xml"), "x").unwrap();
        assert!(
            discover(tmp.path(), &cfg(&["ci/**/*.xml"])).is_none(),
            "a wildcard glob is not a discover candidate (only conventions + literals)"
        );
    }
}
