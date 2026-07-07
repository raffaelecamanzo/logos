//! The typed core error contract and its [`Severity`] classification
//! ([ADR-14], S-026).
//!
//! # The contract: fail-soft-where-safe / fail-loud-on-correctness
//!
//! Every failure Logos can encounter is one of two kinds, and the kind — not
//! the surface — decides what happens ([FR-EH-01], [DL-06]):
//!
//! - [`Severity::Degraded`] — a *local, recoverable* fault: one unparseable
//!   file, a grammar whose ABI does not match the linked runtime, an oversized
//!   file, or a tail of references that could not be resolved. The run **warns
//!   and continues**; the partial result is honest, not fatal ([NFR-RA-11]).
//! - [`Severity::Correctness`] — a fault that would make the answer *wrong* if
//!   swallowed: a missing or corrupt index, FTS desync, a failed migration. The
//!   run **aborts loud** with a non-zero exit (CLI) or a structured protocol
//!   error (MCP) — never a silent empty result, never a panic across the surface
//!   boundary ([NFR-RA-12]).
//!
//! The classification lives here in the core via [`CoreError::severity`], so the
//! two thin surfaces never re-decide it — they only *project* a [`Severity`]
//! onto an exit code ([`exit_code`]) or an MCP error tag ([`classify`]). This is
//! the single auditable home [ADR-14] mandates; a new failure mode is classified
//! once, in one place, and both surfaces inherit it.
//!
//! Hand-rolled (rather than `thiserror`) to match the crate's existing
//! [`ConfigError`](crate::config::ConfigError) / `PluginError` convention and to
//! avoid a new proc-macro dependency (the offline/no-network fitness tests guard
//! the dependency graph).
//!
//! [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
//! [FR-EH-01]: ../../../docs/specs/requirements/FR-EH-01.md
//! [DL-06]: ../../../docs/specs/software-spec.md#11-decision-log
//! [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
//! [NFR-RA-12]: ../../../docs/specs/requirements/NFR-RA-12.md

use std::fmt;
use std::path::PathBuf;

use crate::config::ConfigError;

// ── Severity: the one policy axis ───────────────────────────────────────────

/// The fail-soft / fail-loud classification every [`CoreError`] carries
/// ([ADR-14](../../../docs/specs/architecture/decisions/ADR-14.md)).
///
/// This is the *policy* axis — abort vs. continue — distinct from the exit code,
/// which is the CLI's *projection* of it (a `Correctness` fault can be exit 2 or
/// exit 3; see [`CoreError::exit_code`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Fail loud: the run must abort rather than return a result that could be
    /// wrong. Non-zero exit (CLI), structured error (MCP).
    Correctness,
    /// Fail soft: warn, attach an advisory, and continue with an honest partial
    /// result ([NFR-RA-11](../../../docs/specs/requirements/NFR-RA-11.md)).
    Degraded,
}

impl Severity {
    /// The stable lowercase tag used in the MCP error payload's `severity`
    /// field and in human-facing advisories.
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Correctness => "correctness",
            Severity::Degraded => "degraded",
        }
    }

    /// Whether this severity aborts the run. `Correctness` is fatal; `Degraded`
    /// is not (it warns and continues).
    pub const fn is_fatal(self) -> bool {
        matches!(self, Severity::Correctness)
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── CoreError: the typed, severity-carrying error enum ──────────────────────

/// A classified Logos failure: every variant carries a [`Severity`] via
/// [`CoreError::severity`], the single auditable home for the fail-soft /
/// fail-loud boundary ([ADR-14]).
///
/// Correctness variants propagate to the surface and abort; degraded variants
/// name the recoverable conditions the pipeline skips-and-warns over — they give
/// those existing fail-soft paths an auditable, surface-consistent vocabulary
/// and message ([FR-EH-02]).
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
/// [FR-EH-02]: ../../../docs/specs/requirements/FR-EH-02.md
#[derive(Debug)]
pub enum CoreError {
    // — Correctness (fail loud) —
    /// No index exists under the resolved root and none can be seeded — a
    /// navigation/quality command was run before `logos index`. Names the
    /// remedy ([FR-EH-01]: "missing/corrupt DB → actionable error naming
    /// `logos index`, exit 3").
    ///
    /// [FR-EH-01]: ../../../docs/specs/requirements/FR-EH-01.md
    NoIndex {
        /// The resolved project root that has no `.logos/logos.db`.
        root: PathBuf,
    },
    /// The index store is present but unusable — DB corruption, an FTS5 desync,
    /// or a failed/partial migration. Re-indexing rebuilds it ([FR-EH-01],
    /// [NFR-RA-12](../../../docs/specs/requirements/NFR-RA-12.md)).
    ///
    /// [FR-EH-01]: ../../../docs/specs/requirements/FR-EH-01.md
    CorruptIndex {
        /// What was detected (the integrity-check or migration message).
        detail: String,
    },
    /// A configuration/usage fault ([`ConfigError`], exit 2) — invalid
    /// `config.toml`/`rules.toml`, an unknown key, or an escaping glob. A
    /// correctness fault that maps to the usage exit code, not the internal one.
    Config(ConfigError),

    // — Degraded (fail soft: warn + continue) —
    /// One source file could not be parsed; it is skipped and the rest of the
    /// index proceeds ([FR-IX-04](../../../docs/specs/requirements/FR-IX-04.md)).
    UnparseableFile {
        /// The file that failed to parse (project-relative).
        path: PathBuf,
        /// The parser's reason.
        detail: String,
    },
    /// A grammar's ABI is incompatible with the linked tree-sitter runtime; that
    /// one grammar is skipped and every other language still indexes
    /// ([FR-PL-03](../../../docs/specs/requirements/FR-PL-03.md)).
    GrammarAbiMismatch {
        /// The skipped grammar's language name.
        grammar: String,
        /// The ABI mismatch detail (compiled vs supported range).
        detail: String,
    },
    /// A file exceeds the configured size cap and is skipped with a notice
    /// ([FR-CF-04](../../../docs/specs/requirements/FR-CF-04.md)).
    OversizedFileSkipped {
        /// The skipped file (project-relative).
        path: PathBuf,
        /// The file's size in bytes.
        bytes: u64,
        /// The configured cap in bytes.
        limit: u64,
    },
    /// References remain unresolved after a resolution pass; queries still return
    /// the bound results and surface a coverage number rather than failing
    /// ([NFR-RA-11](../../../docs/specs/requirements/NFR-RA-11.md)).
    PartialResolution {
        /// How many references stayed unresolved.
        unresolved: usize,
    },
}

impl CoreError {
    /// The exit code for a config/usage fault — shared with
    /// [`ConfigError::EXIT_CODE`] so the contract has one source.
    pub const EXIT_USAGE: i32 = ConfigError::EXIT_CODE;
    /// The exit code for an internal/correctness fault (missing/corrupt index,
    /// engine failure) — `FR-CL-03`'s "internal" code.
    pub const EXIT_INTERNAL: i32 = 3;

    /// This error's [`Severity`] — the single classification of the fail-soft /
    /// fail-loud boundary ([ADR-14]).
    ///
    /// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
    pub const fn severity(&self) -> Severity {
        match self {
            CoreError::NoIndex { .. }
            | CoreError::CorruptIndex { .. }
            | CoreError::Config(_) => Severity::Correctness,
            CoreError::UnparseableFile { .. }
            | CoreError::GrammarAbiMismatch { .. }
            | CoreError::OversizedFileSkipped { .. }
            | CoreError::PartialResolution { .. } => Severity::Degraded,
        }
    }

    /// The CLI exit code this error maps to: config faults are usage (2),
    /// other correctness faults are internal (3), and degraded faults never
    /// abort so they report success (0) — they ride inside the read-model as a
    /// warning ([FR-CL-03], [ADR-14]).
    ///
    /// [FR-CL-03]: ../../../docs/specs/requirements/FR-CL-03.md
    /// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
    pub const fn exit_code(&self) -> i32 {
        match self {
            CoreError::Config(_) => Self::EXIT_USAGE,
            CoreError::NoIndex { .. } | CoreError::CorruptIndex { .. } => Self::EXIT_INTERNAL,
            // Degraded conditions warn-and-continue: the command still succeeds.
            CoreError::UnparseableFile { .. }
            | CoreError::GrammarAbiMismatch { .. }
            | CoreError::OversizedFileSkipped { .. }
            | CoreError::PartialResolution { .. } => 0,
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::NoIndex { root } => write!(
                f,
                "no Logos index found under {}: run `logos index` to build one",
                root.display()
            ),
            CoreError::CorruptIndex { detail } => write!(
                f,
                "the Logos index is corrupt or unreadable ({detail}): \
                 delete `.logos/logos.db` and run `logos index` to rebuild it"
            ),
            CoreError::Config(err) => write!(f, "{err}"),
            CoreError::UnparseableFile { path, detail } => write!(
                f,
                "skipped unparseable file {} ({detail}); the rest of the index \
                 is unaffected",
                path.display()
            ),
            CoreError::GrammarAbiMismatch { grammar, detail } => write!(
                f,
                "skipped grammar '{grammar}' ({detail}); other languages still \
                 index — rebuild Logos against a matching tree-sitter to enable it"
            ),
            CoreError::OversizedFileSkipped { path, bytes, limit } => write!(
                f,
                "skipped oversized file {} ({bytes} bytes > {limit} cap); raise \
                 `max_file_bytes` in .logos/config.toml to include it",
                path.display()
            ),
            CoreError::PartialResolution { unresolved } => write!(
                f,
                "{unresolved} references remain unresolved; results are partial \
                 but bound — run `logos sync` after the targets land"
            ),
        }
    }
}

impl std::error::Error for CoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CoreError::Config(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ConfigError> for CoreError {
    fn from(err: ConfigError) -> Self {
        CoreError::Config(err)
    }
}

// ── Surface mapping: the one place a Severity becomes an exit code / tag ─────

/// Project an error caught at a surface boundary onto its CLI exit code
/// ([ADR-14]): a typed [`CoreError`] or [`ConfigError`] reports its own code; an
/// untyped `anyhow` error is an unclassified internal fault → exit 3.
///
/// Both surfaces call this rather than re-deciding the mapping, so the
/// fail-loud boundary stays auditable in one place.
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
pub fn exit_code(err: &anyhow::Error) -> i32 {
    if let Some(core) = err.downcast_ref::<CoreError>() {
        return core.exit_code();
    }
    if let Some(config) = err.downcast_ref::<ConfigError>() {
        return config.exit_code();
    }
    CoreError::EXIT_INTERNAL
}

/// Classify an error caught at a surface boundary by [`Severity`] ([ADR-14]) —
/// used by the MCP surface to tag the structured error payload. A typed
/// [`CoreError`] reports its own severity; a [`ConfigError`] is a correctness
/// (usage) fault; an untyped `anyhow` error is an unclassified correctness
/// fault (fail loud by default — never silently swallowed).
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
pub fn classify(err: &anyhow::Error) -> Severity {
    err.downcast_ref::<CoreError>()
        .map_or(Severity::Correctness, CoreError::severity)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The heart of ADR-14: every degraded condition classifies as `Degraded`
    /// (warn + continue) and every correctness condition as `Correctness`
    /// (abort) — the severity-typed contract, asserted in one place.
    #[test]
    fn severity_partitions_degraded_from_correctness() {
        let degraded = [
            CoreError::UnparseableFile {
                path: "src/a.rs".into(),
                detail: "syntax error".into(),
            },
            CoreError::GrammarAbiMismatch {
                grammar: "toml".into(),
                detail: "ABI 13 outside [14, 15]".into(),
            },
            CoreError::OversizedFileSkipped {
                path: "vendor/big.js".into(),
                bytes: 2_000_000,
                limit: 1_000_000,
            },
            CoreError::PartialResolution { unresolved: 42 },
        ];
        for e in &degraded {
            assert_eq!(e.severity(), Severity::Degraded, "{e}");
            assert!(!e.severity().is_fatal(), "{e}");
            assert_eq!(e.exit_code(), 0, "degraded never aborts: {e}");
        }

        let correctness = [
            CoreError::NoIndex {
                root: "/proj".into(),
            },
            CoreError::CorruptIndex {
                detail: "malformed database disk image".into(),
            },
            CoreError::Config(ConfigError::InvalidRoot {
                path: "/nope".into(),
            }),
        ];
        for e in &correctness {
            assert_eq!(e.severity(), Severity::Correctness, "{e}");
            assert!(e.severity().is_fatal(), "{e}");
            assert_ne!(e.exit_code(), 0, "correctness aborts: {e}");
        }
    }

    /// Exit codes map per FR-CL-03: config faults are usage (2), other
    /// correctness faults internal (3).
    #[test]
    fn correctness_exit_codes_split_usage_from_internal() {
        assert_eq!(
            CoreError::Config(ConfigError::InvalidRoot {
                path: "/nope".into()
            })
            .exit_code(),
            2,
        );
        assert_eq!(
            CoreError::NoIndex {
                root: "/proj".into()
            }
            .exit_code(),
            3,
        );
        assert_eq!(
            CoreError::CorruptIndex {
                detail: "x".into()
            }
            .exit_code(),
            3,
        );
    }

    /// FR-EH-02 / NFR-UX-02: every message states what failed AND names the
    /// next step (a command or a config key to edit).
    #[test]
    fn every_message_is_actionable() {
        let cases: [(CoreError, &str); 6] = [
            (
                CoreError::NoIndex {
                    root: "/proj".into(),
                },
                "logos index",
            ),
            (
                CoreError::CorruptIndex {
                    detail: "disk image malformed".into(),
                },
                "logos index",
            ),
            (
                CoreError::UnparseableFile {
                    path: "src/a.rs".into(),
                    detail: "bad".into(),
                },
                "rest of the index",
            ),
            (
                CoreError::GrammarAbiMismatch {
                    grammar: "toml".into(),
                    detail: "abi".into(),
                },
                "other languages still index",
            ),
            (
                CoreError::OversizedFileSkipped {
                    path: "big.js".into(),
                    bytes: 2,
                    limit: 1,
                },
                "max_file_bytes",
            ),
            (
                CoreError::PartialResolution { unresolved: 3 },
                "logos sync",
            ),
        ];
        for (err, remedy) in &cases {
            let msg = err.to_string();
            assert!(
                msg.contains(remedy),
                "message {msg:?} should name the remedy {remedy:?}",
            );
        }
    }

    /// The surface-mapping helpers downcast typed errors and fall back to
    /// fail-loud-internal for an unclassified `anyhow` error.
    #[test]
    fn surface_helpers_project_severity_and_exit_code() {
        let no_index: anyhow::Error = CoreError::NoIndex {
            root: "/proj".into(),
        }
        .into();
        assert_eq!(exit_code(&no_index), 3);
        assert_eq!(classify(&no_index), Severity::Correctness);

        let config: anyhow::Error = ConfigError::InvalidRoot {
            path: "/nope".into(),
        }
        .into();
        assert_eq!(exit_code(&config), 2);
        assert_eq!(classify(&config), Severity::Correctness);

        // An untyped failure is fail-loud by default: internal exit, correctness.
        let opaque = anyhow::anyhow!("boom");
        assert_eq!(exit_code(&opaque), 3);
        assert_eq!(classify(&opaque), Severity::Correctness);

        // A degraded CoreError that reaches a boundary reports a non-aborting code.
        let degraded: anyhow::Error = CoreError::PartialResolution { unresolved: 1 }.into();
        assert_eq!(exit_code(&degraded), 0);
        assert_eq!(classify(&degraded), Severity::Degraded);
    }

    /// The MCP severity tag is the stable lowercase string both surfaces share.
    #[test]
    fn severity_tags_are_stable() {
        assert_eq!(Severity::Correctness.as_str(), "correctness");
        assert_eq!(Severity::Degraded.as_str(), "degraded");
        assert_eq!(Severity::Degraded.to_string(), "degraded");
    }
}
