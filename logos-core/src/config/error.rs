//! [`ConfigError`] — the fail-loud error for configuration loading/validation.
//!
//! Every variant is a *Correctness/usage* fault ([ADR-14]) that the surfaces map
//! to **exit code 2** (`DV-05`/`DV-07`,
//! [FR-CF-01](../../../../docs/specs/requirements/FR-CF-01.md),
//! [FR-CF-03](../../../../docs/specs/requirements/FR-CF-03.md)): an invalid
//! `config.toml`/`rules.toml` (or a `logos.workspace.toml` workspace manifest,
//! which reuses this error via the [`federation`](../../federation) module,
//! [FR-WS-01](../../../../docs/specs/requirements/FR-WS-01.md)), an unknown key,
//! a non-compiling glob, or a glob that would escape the project root
//! ([NFR-SE-04](../../../../docs/specs/requirements/NFR-SE-04.md)). Messages are
//! actionable — they name the offending path or pattern
//! ([NFR-UX-02](../../../../docs/specs/requirements/NFR-UX-02.md)).
//!
//! Hand-rolled (rather than `thiserror`) to match the crate's existing
//! `UnknownKind` error and to avoid a new proc-macro dependency.
//!
//! [ADR-14]: ../../../../docs/specs/architecture/decisions/ADR-14.md

use std::fmt;
use std::path::PathBuf;

/// A configuration load/validation failure (`config.toml`, `rules.toml`, or the
/// `logos.workspace.toml` workspace manifest, which reuses this error).
///
/// All variants are usage faults the adapters surface as [`ConfigError::EXIT_CODE`].
#[derive(Debug)]
pub enum ConfigError {
    /// The config/rules file could not be read from disk.
    Io {
        /// The path that failed to read.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// A validated candidate could not be written back to disk: the
    /// write-temp-then-rename atomic write failed (the temp write, the
    /// `rename`, or the directory create). The original file is left
    /// **byte-identical** — validation precedes any write, and the rename is the
    /// only mutation, so a failure never leaves a partial write ([NFR-RA-07],
    /// [FR-UI-12], [ADR-31]).
    ///
    /// [NFR-RA-07]: ../../../../docs/specs/requirements/NFR-RA-07.md
    /// [FR-UI-12]: ../../../../docs/specs/requirements/FR-UI-12.md
    /// [ADR-31]: ../../../../docs/specs/architecture/decisions/ADR-31.md
    Write {
        /// The target path that failed to be written.
        path: PathBuf,
        /// The underlying I/O error (temp write, rename, or dir create).
        source: std::io::Error,
    },
    /// TOML was syntactically invalid, or contained an unknown key/section
    /// (`#[serde(deny_unknown_fields)]`). Satisfies the "fail loud on unknown
    /// keys" half of [FR-CF-01]/[FR-CF-03].
    ///
    /// [FR-CF-01]: ../../../../docs/specs/requirements/FR-CF-01.md
    /// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
    Parse {
        /// The path that failed to parse.
        path: PathBuf,
        /// The `toml` deserialiser error (carries line/column + the bad key).
        source: toml::de::Error,
    },
    /// A configured glob pattern did not compile.
    BadGlob {
        /// The offending pattern.
        pattern: String,
        /// The `globset` compilation error.
        source: globset::Error,
    },
    /// A glob pattern would let the walk escape the project root: it contains a
    /// `..` component or is an absolute path. Rejected to honour the contained
    /// filesystem walk ([NFR-SE-04]).
    ///
    /// [NFR-SE-04]: ../../../../docs/specs/requirements/NFR-SE-04.md
    EscapingPattern {
        /// The rejected pattern.
        pattern: String,
    },
    /// The resolved project root is missing or is not a directory.
    InvalidRoot {
        /// The path that could not be resolved to a directory.
        path: PathBuf,
    },
    /// A numeric config value is outside its valid range — e.g. a CR-005 metric
    /// threshold ≤ 0, or `max_clone_ratio` outside `[0.0, 1.0]` ([FR-QM-14],
    /// [FR-GV-11]). Fail loud at load so a misconfiguration never silently skews
    /// the gated signal ([FR-CF-03], [NFR-UX-02]).
    ///
    /// [FR-QM-14]: ../../../../docs/specs/requirements/FR-QM-14.md
    /// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
    /// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
    /// [NFR-UX-02]: ../../../../docs/specs/requirements/NFR-UX-02.md
    InvalidValue {
        /// The offending key (e.g. `constraints.max_clone_ratio`).
        key: String,
        /// What is wrong with the value (carries the rejected value).
        message: String,
    },
}

impl ConfigError {
    /// The process exit code every configuration fault maps to (`DV-05`/`DV-07`).
    ///
    /// Owned here in the core so the contract is tested where the error
    /// originates; the thin adapters merely forward it (ADR-01, ADR-14).
    pub const EXIT_CODE: i32 = 2;

    /// The exit code for this error — always [`ConfigError::EXIT_CODE`].
    pub const fn exit_code(&self) -> i32 {
        Self::EXIT_CODE
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "could not read config file {}: {source}", path.display())
            }
            ConfigError::Write { path, source } => write!(
                f,
                "could not write config file {}: {source} (the original file is unchanged)",
                path.display()
            ),
            ConfigError::Parse { path, source } => {
                // `toml::de::Error` renders the line/column and, for an unknown
                // key, the offending field name — already actionable.
                write!(f, "invalid TOML in {}: {source}", path.display())
            }
            ConfigError::BadGlob { pattern, source } => {
                write!(f, "invalid glob pattern {pattern:?}: {source}")
            }
            ConfigError::EscapingPattern { pattern } => write!(
                f,
                "glob pattern {pattern:?} escapes the project root: '..' \
                 components and absolute paths are not allowed (NFR-SE-04)"
            ),
            ConfigError::InvalidRoot { path } => {
                write!(
                    f,
                    "project root {} is not an accessible directory",
                    path.display()
                )
            }
            ConfigError::InvalidValue { key, message } => {
                write!(f, "invalid value for {key}: {message}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Write { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
            ConfigError::BadGlob { source, .. } => Some(source),
            ConfigError::EscapingPattern { .. }
            | ConfigError::InvalidRoot { .. }
            | ConfigError::InvalidValue { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_to_exit_code_2() {
        // DV-05 / DV-07: configuration faults are exit 2, uniformly.
        let errs = [
            ConfigError::EscapingPattern {
                pattern: "../x".into(),
            },
            ConfigError::InvalidRoot {
                path: PathBuf::from("/nope"),
            },
            // S-096: a failed atomic write-back is a usage fault like the rest.
            ConfigError::Write {
                path: PathBuf::from("/x/config.toml"),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
        ];
        for e in &errs {
            assert_eq!(e.exit_code(), 2);
        }
        assert_eq!(ConfigError::EXIT_CODE, 2);
    }

    #[test]
    fn escaping_pattern_message_is_actionable() {
        let e = ConfigError::EscapingPattern {
            pattern: "../secrets/**".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("../secrets/**"));
        assert!(msg.contains("NFR-SE-04"));
    }
}
