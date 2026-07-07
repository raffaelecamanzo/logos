//! Glob validation and compilation — the single place patterns become matchers.
//!
//! Two responsibilities, both fail-loud with [`ConfigError`] (exit 2):
//!
//! 1. **Containment validation** ([NFR-SE-04]) — reject any pattern that could
//!    steer the walk outside the project root: a `..` path component or an
//!    absolute path. This is the enforcement point named in
//!    [`docs/security/trusted-input-boundary.md`].
//! 2. **Compilation** — compile to a [`globset::GlobSet`] once
//!    ([config component] "Globs: `globset` (compiled once)"); a malformed
//!    pattern is a hard error ([FR-CF-01]).
//!
//! Patterns are matched against the **project-root-relative** path of each
//! discovered file. `globset`'s default semantics apply (notably `*` matches
//! across `/`), so a directory exclude should be written `dir/**`.
//!
//! [NFR-SE-04]: ../../../../docs/specs/requirements/NFR-SE-04.md
//! [FR-CF-01]: ../../../../docs/specs/requirements/FR-CF-01.md
//! [config component]: ../../../../docs/specs/architecture/components/config.md
//! [`docs/security/trusted-input-boundary.md`]: ../../../../docs/security/trusted-input-boundary.md

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::error::ConfigError;

/// Reject a pattern that would escape the project root ([NFR-SE-04]).
///
/// A pattern is rejected if it is absolute (starts with `/`) or contains a `..`
/// path component. Bare `..` mid-pattern (`a/../b`) is caught by the
/// component scan.
fn validate_containment(pattern: &str) -> Result<(), ConfigError> {
    let escapes = pattern.starts_with('/') || pattern.split('/').any(|component| component == "..");
    if escapes {
        return Err(ConfigError::EscapingPattern {
            pattern: pattern.to_string(),
        });
    }
    Ok(())
}

/// Validate (containment) and compile a list of glob patterns into a [`GlobSet`].
///
/// Compiles once; the resulting set is the matcher used for the whole walk. An
/// empty pattern list yields an empty set (matches nothing) — the caller decides
/// what an empty include/exclude means.
///
/// `*` matches across `/` (the `globset` default), so a directory exclude must
/// be written `dir/**`. This is the code-discovery semantics; doc globs use
/// [`compile_anchored`] instead.
///
/// # Errors
/// - [`ConfigError::EscapingPattern`] if any pattern escapes the root.
/// - [`ConfigError::BadGlob`] if any pattern fails to compile.
pub(crate) fn compile(patterns: &[String]) -> Result<GlobSet, ConfigError> {
    compile_with(patterns, false)
}

/// Validate (containment) and compile a list of glob patterns into a [`GlobSet`]
/// with **path-separator-aware** semantics for the documentation globs (S-034,
/// [FR-DG-01], [CR-003]).
///
/// Unlike [`compile`], `*`/`?` do **not** match across `/` here
/// (`literal_separator(true)`), so the default doc include `*.md` means a
/// *top-level* markdown file — `notes/scratch.md` is not a top-level match — and
/// `README*` matches only a root README, not `docs/README.md`. `**` still
/// crosses directories, so `docs/**/*.md` matches `docs/a.md` and any nested
/// `docs/x/y.md`. This is the "top-level `*.md`" / `docs/**/*.md` / `README*`
/// scoping [FR-DG-01] specifies, which `*`-crosses-`/` semantics could not
/// express.
///
/// [FR-DG-01]: ../../../../docs/specs/requirements/FR-DG-01.md
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
///
/// # Errors
/// As [`compile`].
pub(crate) fn compile_anchored(patterns: &[String]) -> Result<GlobSet, ConfigError> {
    compile_with(patterns, true)
}

/// Shared compiler for [`compile`] and [`compile_anchored`]: validate each
/// pattern's containment, then compile it with the chosen separator semantics.
fn compile_with(patterns: &[String], literal_separator: bool) -> Result<GlobSet, ConfigError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        validate_containment(pattern)?;
        let glob = GlobBuilder::new(pattern)
            .literal_separator(literal_separator)
            .build()
            .map_err(|source| ConfigError::BadGlob {
                pattern: pattern.clone(),
                source,
            })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| ConfigError::BadGlob {
        pattern: patterns.join(", "),
        source,
    })
}

/// Validate a pattern list without retaining the compiled set.
///
/// Used by the loaders to fail fast at load time (so a bad `config.toml` is
/// rejected on read, not on first discovery), discarding the matcher that
/// discovery rebuilds.
pub(crate) fn validate(patterns: &[String]) -> Result<(), ConfigError> {
    compile(patterns).map(|_set| ())
}

/// Validate a documentation glob list (anchored semantics) without retaining the
/// compiled set — the doc-glob analogue of [`validate`], used by the loader so a
/// bad `[documentation]` glob is rejected on read (S-034, [FR-DG-01]).
///
/// [FR-DG-01]: ../../../../docs/specs/requirements/FR-DG-01.md
pub(crate) fn validate_anchored(patterns: &[String]) -> Result<(), ConfigError> {
    compile_anchored(patterns).map(|_set| ())
}

/// A compiled include/exclude matcher for the documentation globs (S-034,
/// [CR-003]): the [FR-DG-01] scoping that decides which markdown files discovery
/// admits as documentation, compiled once per index/sync.
///
/// Built from the `[documentation]` config table via
/// [`Documentation::compile`](super::settings::Documentation::compile); a `None`
/// from there means documentation is disabled (no doc file is admitted).
///
/// [FR-DG-01]: ../../../../docs/specs/requirements/FR-DG-01.md
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
#[derive(Debug, Clone)]
pub(crate) struct DocGlobs {
    include: GlobSet,
    exclude: GlobSet,
}

impl DocGlobs {
    /// Compile the include/exclude doc globs with anchored semantics.
    ///
    /// # Errors
    /// As [`compile_anchored`] (a bad or escaping doc glob).
    pub(crate) fn compile(include: &[String], exclude: &[String]) -> Result<Self, ConfigError> {
        Ok(Self {
            include: compile_anchored(include)?,
            exclude: compile_anchored(exclude)?,
        })
    }

    /// Whether `rel` (a root-relative, `/`-joined path) is admitted as
    /// documentation: it matches an include glob and no exclude glob.
    pub(crate) fn admits(&self, rel: &str) -> bool {
        self.include.is_match(rel) && !self.exclude.is_match(rel)
    }
}

/// A compiled include/exclude matcher for the **config-artifact** globs (S-062,
/// [CR-010]): the [FR-CG-02] scoping that, on top of the per-descriptor
/// extension/basename claim, decides which artifact files discovery admits —
/// notably subtracting the default lock-file excludes ([BR-30]).
///
/// Built from the `[config_artifacts]` config table via
/// [`ConfigArtifacts::compile`](super::settings::ConfigArtifacts::compile); a
/// `None` from there means the config layer is disabled (no artifact admitted).
///
/// Unlike [`DocGlobs`], these use the **non-anchored** code-glob semantics
/// (`*` crosses `/`), so the default excludes are written `**/name` to match a
/// lock file at any depth (root included, since `**/` collapses zero directories).
///
/// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [FR-CG-02]: ../../../../docs/specs/requirements/FR-CG-02.md
#[derive(Debug, Clone)]
pub(crate) struct ConfigGlobs {
    include: GlobSet,
    exclude: GlobSet,
}

impl ConfigGlobs {
    /// Compile the include/exclude config globs with non-anchored (code-glob)
    /// semantics.
    ///
    /// # Errors
    /// As [`compile`] (a bad or escaping config glob).
    pub(crate) fn compile(include: &[String], exclude: &[String]) -> Result<Self, ConfigError> {
        Ok(Self {
            include: compile(include)?,
            exclude: compile(exclude)?,
        })
    }

    /// Whether `rel` (a root-relative, `/`-joined path) is admitted as a config
    /// artifact: it matches an include glob and no exclude glob. The caller has
    /// already established that an artifact plugin claims the file (by extension
    /// or basename); this layer applies the include/exclude policy on top.
    pub(crate) fn admits(&self, rel: &str) -> bool {
        self.include.is_match(rel) && !self.exclude.is_match(rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal() {
        let err = compile(&["../etc/passwd".into()]).unwrap_err();
        assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    }

    #[test]
    fn rejects_embedded_parent_traversal() {
        let err = compile(&["src/../../x".into()]).unwrap_err();
        assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    }

    #[test]
    fn rejects_absolute_pattern() {
        let err = compile(&["/etc/**".into()]).unwrap_err();
        assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    }

    #[test]
    fn rejects_malformed_glob() {
        // An unclosed alternate is a globset compile error, not a containment one.
        let err = compile(&["src/{a,b".into()]).unwrap_err();
        assert!(matches!(err, ConfigError::BadGlob { .. }));
    }

    #[test]
    fn compiles_valid_patterns_and_matches_relative_paths() {
        let set = compile(&["**/*.rs".into(), "docs/**".into()]).unwrap();
        assert!(set.is_match("src/main.rs"));
        assert!(set.is_match("docs/spec.md"));
        assert!(!set.is_match("README.txt"));
    }

    #[test]
    fn double_star_matches_everything() {
        let set = compile(&["**".into()]).unwrap();
        assert!(set.is_match("a.rs"));
        assert!(set.is_match("deeply/nested/file.py"));
    }

    #[test]
    fn empty_pattern_list_matches_nothing() {
        let set = compile(&[]).unwrap();
        assert!(!set.is_match("anything"));
    }

    #[test]
    fn dotdot_as_a_filename_substring_is_allowed() {
        // "..foo" is a legitimate filename, not a parent-dir traversal.
        let set = compile(&["**/..foo".into()]).unwrap();
        assert!(set.is_match("dir/..foo"));
    }

    // ── Anchored (documentation) glob semantics (S-034, FR-DG-01) ─────────────

    #[test]
    fn anchored_star_does_not_cross_a_separator() {
        // The whole point of the doc globs: top-level `*.md` is top-level only,
        // so a deeper `.md` is NOT matched — the property AC1's out-of-glob file
        // rests on. The non-anchored `compile` would match it.
        let set = compile_anchored(&["*.md".into()]).unwrap();
        assert!(set.is_match("README.md"));
        assert!(set.is_match("guide.md"));
        assert!(!set.is_match("notes/scratch.md"), "*.md is top-level only");
        assert!(!set.is_match("docs/spec.md"));
        // Contrast: non-anchored `*` crosses `/`, so the same pattern would
        // wrongly admit a nested file — the regression `compile_anchored` avoids.
        assert!(compile(&["*.md".into()])
            .unwrap()
            .is_match("notes/scratch.md"));
    }

    #[test]
    fn anchored_double_star_still_crosses_directories() {
        // `**` is unaffected by literal_separator, so the default doc include
        // matches both a direct child and an arbitrarily nested `.md`, and a
        // zero-directory `docs/a.md` (the `/**/ ` collapses).
        let set = compile_anchored(&["docs/**/*.md".into()]).unwrap();
        assert!(set.is_match("docs/spec.md"));
        assert!(set.is_match("docs/planning/sprints/sprint-7.md"));
        assert!(!set.is_match("docs/spec.txt"));
        assert!(!set.is_match("notes/spec.md"));
    }

    #[test]
    fn anchored_readme_glob_is_root_only() {
        // `README*` matches a root README of any extension, but not a nested one
        // (no separator in the pattern, and `*` cannot introduce one).
        let set = compile_anchored(&["README*".into()]).unwrap();
        assert!(set.is_match("README"));
        assert!(set.is_match("README.md"));
        assert!(set.is_match("README.markdown"));
        assert!(!set.is_match("docs/README.md"));
    }

    #[test]
    fn anchored_default_doc_set_scopes_exactly_the_fr_dg_01_globs() {
        // The shipped default: docs/**/*.md ∪ top-level *.md ∪ README*.
        let set =
            compile_anchored(&["docs/**/*.md".into(), "*.md".into(), "README*".into()]).unwrap();
        assert!(set.is_match("docs/spec.md"), "under docs/");
        assert!(set.is_match("README.md"), "root README");
        assert!(set.is_match("CHANGELOG.md"), "top-level .md");
        assert!(!set.is_match("notes/scratch.md"), "out-of-glob .md");
        assert!(!set.is_match("src/lib.rs"), "code is never a doc match");
    }

    #[test]
    fn doc_globs_exclude_wins_over_include() {
        // The `[documentation] exclude` globs subtract from the includes: a file
        // matching both an include and an exclude is not admitted. Exercises the
        // `&& !exclude.is_match` branch of `DocGlobs::admits`.
        let docs = DocGlobs::compile(&["docs/**/*.md".into()], &["docs/drafts/**".into()]).unwrap();
        assert!(docs.admits("docs/spec.md"), "included, not excluded");
        assert!(
            !docs.admits("docs/drafts/wip.md"),
            "matches the exclude → not admitted even though it matches the include"
        );
        assert!(!docs.admits("src/lib.rs"), "matches no include glob");
    }

    // ── Config-artifact glob semantics (S-062, CR-010, FR-CG-02) ──────────────

    #[test]
    fn config_globs_exclude_lock_files_by_default_at_any_depth() {
        // The shipped default excludes, written `**/name` so a lock file is
        // excluded at the root AND at any nested depth (BR-30). Include `**`.
        let excludes = [
            "**/package-lock.json",
            "**/Cargo.lock",
            "**/yarn.lock",
            "**/pnpm-lock.yaml",
            "**/*.min.json",
        ]
        .map(String::from);
        let cfg = ConfigGlobs::compile(&["**".to_string()], &excludes).unwrap();

        // An ordinary config file is admitted.
        assert!(cfg.admits("config.yaml"));
        assert!(cfg.admits("deploy/values.yaml"));
        // Lock files are excluded by default, at the root and nested.
        assert!(!cfg.admits("package-lock.json"), "root lock excluded");
        assert!(
            !cfg.admits("frontend/package-lock.json"),
            "nested lock excluded"
        );
        assert!(!cfg.admits("Cargo.lock"));
        assert!(!cfg.admits("pnpm-lock.yaml"));
        assert!(!cfg.admits("bundle.min.json"), "minified JSON excluded");
        assert!(!cfg.admits("dist/app.min.json"));
    }

    #[test]
    fn config_globs_override_can_re_admit_a_lock_file() {
        // Overriding the exclude set (e.g. dropping the package-lock exclude)
        // re-admits the file — the BR-30 "overridable in config.toml" rule.
        let cfg =
            ConfigGlobs::compile(&["**".to_string()], &["**/Cargo.lock".to_string()]).unwrap();
        assert!(
            cfg.admits("package-lock.json"),
            "no longer excluded after override"
        );
        assert!(
            !cfg.admits("Cargo.lock"),
            "still excluded by the retained glob"
        );
    }

    #[test]
    fn config_globs_obey_containment() {
        // Containment (NFR-SE-04) applies to config globs exactly as to code globs.
        assert!(matches!(
            ConfigGlobs::compile(&["../outside/**".to_string()], &[]).unwrap_err(),
            ConfigError::EscapingPattern { .. }
        ));
    }

    #[test]
    fn anchored_compile_rejects_escaping_patterns() {
        // Containment (NFR-SE-04) applies to doc globs exactly as to code globs.
        assert!(matches!(
            compile_anchored(&["../outside/*.md".into()]).unwrap_err(),
            ConfigError::EscapingPattern { .. }
        ));
    }
}
