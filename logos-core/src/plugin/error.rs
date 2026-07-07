//! Plugin substrate error and skip types ([plugin-registry], [ADR-09]).
//!
//! Two distinct failure modes, deliberately handled differently:
//!
//! - **ABI mismatch** ([`SkipReason`]) is a *soft* failure. A grammar whose ABI
//!   is incompatible with the linked tree-sitter runtime is skipped with a
//!   warning and the run continues — one bad grammar never aborts the binary
//!   ([FR-PL-03], [NFR-PC-03], [UAT-PL-02]). Skips are recorded on the registry,
//!   not returned as errors.
//! - **Query compile failure** ([`PluginError::QueryCompile`]) and a malformed
//!   descriptor ([`PluginError::Manifest`]) are *hard* failures that name the
//!   offending file ([FR-PL-02]).
//!
//! [plugin-registry]: ../../../docs/specs/architecture/components/plugin-registry.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-03]: ../../../docs/specs/requirements/FR-PL-03.md
//! [NFR-PC-03]: ../../../docs/specs/requirements/NFR-PC-03.md
//! [UAT-PL-02]: ../../../docs/specs/requirements/UAT-PL-02.md

use std::fmt;

/// A hard plugin-substrate failure that aborts registry construction.
///
/// Carries the offending file path so the operator can fix the asset
/// ([FR-PL-02] "fails ... naming the file").
#[derive(Debug)]
pub enum PluginError {
    /// A `plugin.toml` descriptor failed to parse or was missing a field.
    Manifest {
        /// The descriptor path (embedded asset name or on-disk override path).
        file: String,
        /// The underlying parse / validation message.
        detail: String,
    },
    /// A `.scm` query failed to compile against the built `Language`.
    ///
    /// The `file` is the embedded asset name or the on-disk override path that
    /// was actually used (an override shadows the embedded source), so the
    /// message always points at the source the operator can edit.
    QueryCompile {
        /// The query file whose source failed to compile.
        file: String,
        /// The `tree_sitter::QueryError` rendered for humans.
        detail: String,
    },
    /// An on-disk override directory entry could not be read.
    Io {
        /// The path that could not be read.
        file: String,
        /// The underlying I/O error message.
        detail: String,
    },
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::Manifest { file, detail } => {
                write!(f, "invalid plugin descriptor '{file}': {detail}")
            }
            PluginError::QueryCompile { file, detail } => {
                write!(f, "query '{file}' failed to compile: {detail}")
            }
            PluginError::Io { file, detail } => {
                write!(f, "could not read plugin override '{file}': {detail}")
            }
        }
    }
}

impl std::error::Error for PluginError {}

/// Why a grammar was skipped at load — the soft-failure path ([FR-PL-03]).
///
/// A skipped grammar is recorded on the [`crate::plugin::LanguageRegistry`] and
/// surfaced as a warning; it never propagates as a [`PluginError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The compiled grammar's ABI version is outside the linked runtime's
    /// supported `[min, max]` range — loading it would risk undefined behaviour.
    AbiUnsupported {
        /// The grammar's compiled ABI version.
        compiled: usize,
        /// The runtime's minimum compatible ABI version.
        min: usize,
        /// The runtime's maximum (current) ABI version.
        max: usize,
    },
    /// The descriptor's declared `abi_version` disagrees with the compiled
    /// grammar's actual ABI — an integrity mismatch between the asset and the
    /// linked grammar, treated as an ABI mismatch.
    DescriptorAbiDisagrees {
        /// The `abi_version` declared in `plugin.toml`.
        declared: usize,
        /// The compiled grammar's actual ABI version.
        compiled: usize,
    },
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkipReason::AbiUnsupported { compiled, min, max } => write!(
                f,
                "grammar ABI {compiled} is outside the supported range \
                 [{min}, {max}] of the linked tree-sitter runtime"
            ),
            SkipReason::DescriptorAbiDisagrees { declared, compiled } => write!(
                f,
                "descriptor declares abi_version={declared} but the compiled \
                 grammar reports ABI {compiled}"
            ),
        }
    }
}

/// A grammar that was skipped at load, with the language name and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedGrammar {
    /// The descriptor `name` of the skipped grammar.
    pub name: String,
    /// Why it was skipped.
    pub reason: SkipReason,
}

impl fmt::Display for SkippedGrammar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "skipping grammar '{}': {}", self.name, self.reason)
    }
}
