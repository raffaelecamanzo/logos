//! Query resolution + compilation ([FR-PL-02], [FR-PL-04], [FR-PL-05]).
//!
//! A capability's `.scm` query has two possible sources, in priority order:
//!
//! 1. **On-disk override** — `<project>/.logos/plugins/<lang>/<relative path>`,
//!    if present, *shadows* the embedded query. This is what lets a maintainer
//!    tune extraction for an already-compiled grammar without a rebuild
//!    ([FR-PL-04], [FR-PL-05], [UAT-PL-03], [NFR-MA-05]).
//! 2. **Embedded** — the `include_str!`-embedded source shipped in the binary.
//!
//! Whichever source wins, its label (the on-disk path or the embedded asset
//! name) is threaded into a compile error so the message always points at the
//! source the operator can actually edit ([FR-PL-02]).
//!
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [FR-PL-05]: ../../../docs/specs/requirements/FR-PL-05.md
//! [UAT-PL-03]: ../../../docs/specs/requirements/UAT-PL-03.md
//! [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md

use std::path::Path;

use tree_sitter::{Language, Query};

use super::error::PluginError;

/// A query whose source has been resolved (override-or-embedded), ready to
/// compile.
#[derive(Debug, Clone)]
pub struct ResolvedQuery {
    /// The capability this query backs (e.g. `"symbols"`).
    pub capability: String,
    /// Human-facing label of the *winning* source: the on-disk override path
    /// when overridden, else the embedded asset name. Used in compile errors.
    pub file_label: String,
    /// The query text to compile.
    pub source: String,
    /// `true` when the source came from an on-disk override (for observability
    /// and tests), `false` when it is the embedded default.
    pub overridden: bool,
}

/// Resolve a single capability's query source, preferring an on-disk override.
///
/// - `capability` / `relative_path` come from the descriptor's `[queries]`.
/// - `embedded_label` is the embedded asset name used in error messages.
/// - `embedded_source` is the `include_str!`-embedded query text.
/// - `override_dir`, when `Some`, is `<project>/.logos/plugins/<lang>/`; the
///   override file is `override_dir.join(relative_path)`.
///
/// # Errors
/// Returns [`PluginError::Io`] if an override file exists but cannot be read.
pub fn resolve_query(
    capability: &str,
    relative_path: &str,
    embedded_label: &str,
    embedded_source: &str,
    override_dir: Option<&Path>,
) -> Result<ResolvedQuery, PluginError> {
    if let Some(dir) = override_dir {
        let candidate = dir.join(relative_path);
        if candidate.is_file() {
            let source = std::fs::read_to_string(&candidate).map_err(|e| PluginError::Io {
                file: candidate.display().to_string(),
                detail: e.to_string(),
            })?;
            return Ok(ResolvedQuery {
                capability: capability.to_string(),
                file_label: candidate.display().to_string(),
                source,
                overridden: true,
            });
        }
    }
    Ok(ResolvedQuery {
        capability: capability.to_string(),
        file_label: embedded_label.to_string(),
        source: embedded_source.to_string(),
        overridden: false,
    })
}

/// Compile a resolved query against the built `Language`, failing fast and
/// naming the source on error ([FR-PL-02]).
///
/// # Errors
/// Returns [`PluginError::QueryCompile`] with `file` set to the resolved
/// query's `file_label` when the query has a syntax or node-type error.
pub fn compile(language: &Language, resolved: &ResolvedQuery) -> Result<Query, PluginError> {
    Query::new(language, &resolved.source).map_err(|e| PluginError::QueryCompile {
        file: resolved.file_label.clone(),
        detail: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_embedded_when_no_override_dir() {
        let r = resolve_query(
            "symbols",
            "queries/symbols.scm",
            "rust/queries/symbols.scm",
            "(identifier) @x",
            None,
        )
        .unwrap();
        assert!(!r.overridden);
        assert_eq!(r.file_label, "rust/queries/symbols.scm");
        assert_eq!(r.source, "(identifier) @x");
    }

    #[test]
    fn resolves_embedded_when_override_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolve_query(
            "symbols",
            "queries/symbols.scm",
            "rust/queries/symbols.scm",
            "(identifier) @x",
            Some(dir.path()),
        )
        .unwrap();
        assert!(!r.overridden, "absent override must fall back to embedded");
    }

    #[test]
    fn override_file_shadows_embedded() {
        let dir = tempfile::tempdir().unwrap();
        let qdir = dir.path().join("queries");
        std::fs::create_dir_all(&qdir).unwrap();
        std::fs::write(qdir.join("symbols.scm"), "(type_identifier) @overridden").unwrap();

        let r = resolve_query(
            "symbols",
            "queries/symbols.scm",
            "rust/queries/symbols.scm",
            "(identifier) @embedded",
            Some(dir.path()),
        )
        .unwrap();

        assert!(r.overridden, "present override must win over embedded");
        assert_eq!(r.source, "(type_identifier) @overridden");
        assert_eq!(r.file_label, qdir.join("symbols.scm").display().to_string());
    }
}
