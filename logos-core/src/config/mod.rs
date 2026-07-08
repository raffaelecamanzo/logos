//! Configuration loading and validation ([config component], S-006).
//!
//! Loads and validates the two checked-in policy files and exposes them as
//! read-models for the rest of the core:
//!
//! - [`Config`] (`.logos/config.toml`) → consumed by the [pipeline-orchestrator]
//!   ([FR-CF-01]).
//! - [`Rules`] (`.logos/rules.toml`) → the architecture contract consumed by the
//!   [governance-engine] ([FR-CF-03]).
//!
//! Plus discovery ([`discover`]): the gitignore-aware, root-contained walk that
//! composes gitignore ∪ `exclude` ∪ `ignored_dirs` ([FR-IX-02], [FR-CF-02]) and
//! skips oversized files with a notice ([FR-CF-04]).
//!
//! # Failure posture
//! Every load/validation fault is a [`ConfigError`] the surfaces map to **exit
//! code 2** ([`ConfigError::EXIT_CODE`]): invalid TOML, an unknown key
//! (`#[serde(deny_unknown_fields)]`), a non-compiling glob, or a glob that would
//! escape the project root ([NFR-SE-04]). Missing files are *not* a fault — they
//! resolve to defaults ([`Config::default`] / [`Rules::default`]), since policy
//! travels into worktrees but need not exist in every one ([NFR-DM-04]).
//!
//! [config component]: ../../../docs/specs/architecture/components/config.md
//! [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
//! [governance-engine]: ../../../docs/specs/architecture/components/governance-engine.md
//! [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
//! [FR-CF-02]: ../../../docs/specs/requirements/FR-CF-02.md
//! [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
//! [FR-CF-04]: ../../../docs/specs/requirements/FR-CF-04.md
//! [FR-IX-02]: ../../../docs/specs/requirements/FR-IX-02.md
//! [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md

use std::fs;
use std::path::Path;

mod admission;
mod chat;
mod discovery;
mod error;
mod globs;
mod rules;
mod secrets;
mod settings;
mod wiki;
mod writeback;

pub use admission::AdmissionAuthority;
pub use chat::{
    ChatConfig, ChatModelOverrides, ChatProvider, ChatRole, DEFAULT_CHAT_BASE_URL,
    DEFAULT_MAX_REPLANS, DEFAULT_MAX_SUBAGENT_TOOL_CALLS, DEFAULT_MAX_TOOL_CALLS,
};
pub use discovery::{
    discover, unindexed_doc_symlinks, DiscoveryReport, DocSymlinkDrop, OversizeSkip,
    UnindexedDocSymlink,
};
pub use error::ConfigError;
/// The validated glob compiler, shared with the annotation engine's layer
/// matching (S-014, [FR-AN-03]) so layer globs obey the same containment
/// rules ([NFR-SE-04]) as include/exclude patterns.
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
/// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
pub(crate) use globs::compile as compile_globs;
/// The compiled config-artifact include/exclude matcher (S-062), consumed by the
/// [pipeline-orchestrator] to decide which artifact files discovery admits, on
/// top of the per-descriptor extension/basename claim ([FR-CG-02], [CR-010]).
///
/// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
/// [FR-CG-02]: ../../../docs/specs/requirements/FR-CG-02.md
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
pub(crate) use globs::ConfigGlobs;
/// The compiled documentation include/exclude matcher (S-034), consumed by the
/// [pipeline-orchestrator] to decide which markdown files discovery admits as
/// documentation ([FR-DG-01], [CR-003]).
///
/// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
/// [FR-DG-01]: ../../../docs/specs/requirements/FR-DG-01.md
/// [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
pub(crate) use globs::DocGlobs;
pub use rules::{
    Boundary, Constraints, Coverage, EffectiveCoverage, EffectiveHistory, ForbiddenImport, History,
    Layer, MaxDead, MaxDeadBaseline, MetricThresholds, RequireDocumented, RequireTested, Rules,
};
pub use secrets::{load_secrets_from_root, ChatSecrets, MaskedSecret, Secrets};
pub use settings::{
    BindingPolicy, Config, ConfigArtifacts, CoverageIngest, Documentation, EffectiveCoverageIngest,
    Resolution, Semantics, TypedEnrichment, Watcher, DEFAULT_MAX_FILE_SIZE,
};
pub use wiki::{EffectiveWikiModel, WikiConfig};
pub use writeback::{
    read_documents, write_config, write_rules, write_secret, ConfigApplyOutcome, ConfigFileView,
    ConfigReadModel, ConfigWriteOutcome, FileView, PolicyFile, RulesFileView, SecretWriteOutcome,
    RULES_PROVENANCE_STAMP,
};

/// The conventional location of the two policy files within a project root.
const CONFIG_RELPATH: &str = ".logos/config.toml";
const RULES_RELPATH: &str = ".logos/rules.toml";

/// Load and validate a `config.toml` from an explicit path.
///
/// # Errors
/// [`ConfigError::Io`] if the file cannot be read, [`ConfigError::Parse`] on
/// invalid TOML or an unknown key, or [`ConfigError::BadGlob`] /
/// [`ConfigError::EscapingPattern`] if an include/exclude glob is invalid.
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_config(&text, path)
}

/// Parse and validate a `config.toml` from already-read `text`; `path` is for
/// error attribution only.
///
/// The single-read seam for callers that hold the candidate document in memory
/// — notably the validated atomic write-back path ([`write_config`]) the web
/// config editor builds on ([FR-UI-12], [ADR-31]): it runs exactly the same
/// `#[serde(deny_unknown_fields)]` parse + [`Config::validate`] glob/containment
/// checks as the loader, so a candidate accepted here is byte-for-byte as safe
/// as a file the CLI loads — no second validation path is invented.
///
/// # Errors
/// [`ConfigError::Parse`] on invalid TOML or an unknown key ([FR-CF-01]), or
/// [`ConfigError::BadGlob`] / [`ConfigError::EscapingPattern`] if an
/// include/exclude glob is invalid.
///
/// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
/// [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
/// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
pub fn parse_config(text: &str, path: &Path) -> Result<Config, ConfigError> {
    let config: Config = toml::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    config.validate()?;
    Ok(config)
}

/// Load `config.toml` from `<root>/.logos/config.toml`, or [`Config::default`]
/// if it is absent ([FR-CF-01], [NFR-DM-04]).
///
/// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
pub fn load_config_from_root(root: &Path) -> Result<Config, ConfigError> {
    let path = root.join(CONFIG_RELPATH);
    if path.exists() {
        load_config(&path)
    } else {
        Ok(Config::default())
    }
}

/// Load and validate a `rules.toml` from an explicit path.
///
/// # Errors
/// [`ConfigError::Io`] if the file cannot be read, [`ConfigError::Parse`] on
/// invalid TOML or an unknown key ([FR-CF-03]), or [`ConfigError::BadGlob`] /
/// [`ConfigError::EscapingPattern`] if a layer glob is invalid.
///
/// [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
pub fn load_rules(path: &Path) -> Result<Rules, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_rules(&text, path)
}

/// Parse and validate a `rules.toml` from already-read `text`; `path` is for
/// error attribution only.
///
/// The single-read seam for callers that hash the content first (the
/// governance rules cache, S-020, [FR-GV-01]): hashing and parsing the same
/// string means a concurrent edit can never produce an incoherent
/// (hash, parse) cache pair.
///
/// # Errors
/// [`ConfigError::Parse`] on invalid TOML or an unknown key ([FR-CF-03]), or
/// [`ConfigError::BadGlob`] / [`ConfigError::EscapingPattern`] if a layer
/// glob is invalid.
///
/// [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
/// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
pub fn parse_rules(text: &str, path: &Path) -> Result<Rules, ConfigError> {
    let rules: Rules = toml::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    rules.validate()?;
    Ok(rules)
}

/// Load `rules.toml` from `<root>/.logos/rules.toml`, or [`Rules::default`] if it
/// is absent ([FR-CF-03], [NFR-DM-04]).
///
/// [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
pub fn load_rules_from_root(root: &Path) -> Result<Rules, ConfigError> {
    let path = root.join(RULES_RELPATH);
    if path.exists() {
        load_rules(&path)
    } else {
        Ok(Rules::default())
    }
}
