//! Validated atomic config write-back ([FR-UI-12], [CR-025], [ADR-31]).
//!
//! The write side of the [config component]: read both checked-in policy files
//! (raw content + parsed model) for the served root, and replace one of them
//! with an edited candidate — but only after the candidate clears the **exact
//! same load-path validation** the CLI runs ([`super::parse_config`] /
//! [`super::parse_rules`]: `#[serde(deny_unknown_fields)]`, glob compilation,
//! range checks), and only via a **write-temp-then-rename** atomic swap so an
//! invalid edit is rejected with **no partial write** and the original file is
//! left byte-identical ([NFR-RA-07]).
//!
//! This is the engine seam the web config editor ([FR-UI-12]) builds on; it
//! invents no new validation logic — it reuses the loader's — and it is the only
//! programmatic writer of the policy files besides the `logos init` templates.
//!
//! A `rules.toml` write additionally stamps a provenance comment
//! ([`RULES_PROVENANCE_STAMP`]) into the written file so a `check_rules`-gate
//! contract edited from the browser is visibly machine-written in VCS ([BR-35]);
//! the stamp is a TOML comment block, so the stamped file still parses through
//! the standard [`super::load_rules`] path.
//!
//! [config component]: ../../../docs/specs/architecture/components/config.md
//! [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
//! [CR-025]: ../../../docs/requests/CR-025-interactive-config-editing.md
//! [BR-35]: ../../../docs/specs/software-spec.md#326-web-ui

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::error::ConfigError;
use super::secrets::SECRETS_RELPATH;
use super::{
    load_config_from_root, load_rules_from_root, load_secrets_from_root, parse_config, parse_rules,
    Config, Constraints, MaskedSecret, MetricThresholds, Rules, CONFIG_RELPATH, RULES_RELPATH,
};

/// Which checked-in policy file a [`read_documents`]/write targets.
///
/// The two files the [config component] owns: `.logos/config.toml` (the
/// [`Config`] indexing policy) and `.logos/rules.toml` (the [`Rules`] governance
/// contract). The façade's `config_write` dispatches on this.
///
/// [config component]: ../../../docs/specs/architecture/components/config.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyFile {
    /// `.logos/config.toml` — the indexing policy.
    Config,
    /// `.logos/rules.toml` — the architecture contract enforced by the gate.
    Rules,
}

/// The provenance comment block stamped at the top of a `rules.toml` written
/// through [`write_rules`] ([FR-UI-12], [BR-35]).
///
/// A pure TOML comment block (every line begins with `#`, terminated by a blank
/// line), so a stamped contract still parses through [`super::load_rules`]. It is
/// deliberately **wall-clock-free** so a write is deterministic and reproducible
/// (the byte-identical contract is about the *rejected* path, but a deterministic
/// stamp also keeps an accepted re-save idempotent). [`write_rules`] strips a
/// pre-existing copy of this exact block before re-stamping, so repeated saves
/// never accumulate stamps.
pub const RULES_PROVENANCE_STAMP: &str = "\
# Written by the Logos web config editor (CR-025, FR-UI-12).
# This file is the architecture contract enforced by `check_rules` and the gate
# (FR-CF-03); edits here change what the gate enforces — review the VCS diff.

";

/// Both policy files for `root`, each as raw content + the parsed/validated
/// model — the read half of the config-editor seam ([FR-UI-12]).
#[derive(Debug, Clone, Serialize)]
pub struct ConfigReadModel {
    /// `.logos/config.toml`.
    pub config: ConfigFileView,
    /// `.logos/rules.toml`.
    pub rules: RulesFileView,
    /// The **masked** chat API key (presence + last-4) loaded from the
    /// gitignored `.logos/secrets.toml` (S-169, [FR-CF-06], [NFR-SE-07]). This
    /// is the *only* form of the key in any read-model — the raw value is never
    /// placed here, so it can never be serialised into a response by
    /// construction; the editor renders it write-only/masked.
    ///
    /// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
    /// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
    pub chat_key: MaskedSecret,

    /// The server-computed default / recommended-baseline projection ([CR-067],
    /// [FR-UI-12], [BR-37]) the Config editor renders beside each field's
    /// current value. Sourced only from Rust code — the editor never fabricates
    /// a default ([NFR-RA-05]).
    ///
    /// [CR-067]: ../../../docs/requests/CR-067-config-default-surfacing.md
    /// [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
    /// [BR-37]: ../../../docs/specs/software-spec.md#326-web-ui
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub defaults: ConfigDefaults,
}

/// The `defaults` projection ([CR-067], [BR-37]): every value traces to a Rust
/// code default, never a client-authored one ([NFR-RA-05]).
///
/// [CR-067]: ../../../docs/requests/CR-067-config-default-surfacing.md
/// [BR-37]: ../../../docs/specs/software-spec.md#326-web-ui
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, Serialize)]
pub struct ConfigDefaults {
    /// `config.toml`'s real code defaults ([`Config::default`]), serialized
    /// **whole** — the SPA reads only the fields it formifies ([CR-067] CRA-02),
    /// so no field is hand-picked and none can silently go stale.
    pub config: Config,
    /// `rules.toml`'s default / recommended-baseline projection.
    pub rules: RulesDefaults,
}

/// `rules.toml`'s default / recommended-baseline projection: real code
/// defaults for `[metric_thresholds]`, and curated recommended baselines for
/// `[constraints]` (which carry no code default — omitted = not enforced,
/// [FR-CF-03]).
///
/// [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
#[derive(Debug, Clone, Serialize)]
pub struct RulesDefaults {
    /// `[metric_thresholds]` real defaults — [`Thresholds::default`] resolved
    /// through [`MetricThresholds::effective`], keyed by the rules.toml key
    /// names ([FR-QM-14]).
    ///
    /// [`Thresholds::default`]: crate::metrics::Thresholds
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    pub metric_thresholds: MetricThresholdDefaults,
    /// `[constraints]` curated recommended baselines ([`Constraints::recommended`]).
    /// **Advisory display only** — never written, never enforced, never moves
    /// the gate ([BR-37]).
    ///
    /// [BR-37]: ../../../docs/specs/software-spec.md#326-web-ui
    pub constraints: Constraints,
}

/// `[metric_thresholds]` real code defaults, keyed by the **rules.toml key
/// names** — resolving the naming asymmetry with the engine's
/// [`Thresholds`](crate::metrics::Thresholds) field names so the projection
/// matches the editor's keys 1:1 ([FR-QM-14], [CR-067]).
///
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [CR-067]: ../../../docs/requests/CR-067-config-default-surfacing.md
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct MetricThresholdDefaults {
    /// `nesting_depth` ↔ [`Thresholds::nest`](crate::metrics::Thresholds::nest).
    pub nesting_depth: i64,
    /// `brain_complexity` ↔ [`Thresholds::brain_cc`](crate::metrics::Thresholds::brain_cc).
    pub brain_complexity: i64,
    /// `brain_lines` ↔ [`Thresholds::brain_loc`](crate::metrics::Thresholds::brain_loc).
    pub brain_lines: i64,
    /// `brain_nesting` ↔ [`Thresholds::brain_nest`](crate::metrics::Thresholds::brain_nest).
    pub brain_nesting: i64,
    /// `god_methods` (same name on both sides).
    pub god_methods: i64,
    /// `god_span` (same name on both sides).
    pub god_span: i64,
    /// `clone_similarity` (same name on both sides).
    pub clone_similarity: f64,
    /// `clone_min_tokens` (same name on both sides).
    pub clone_min_tokens: i64,
}

impl MetricThresholdDefaults {
    /// Build the projection from the engine's
    /// [`Thresholds`](crate::metrics::Thresholds) via an **exhaustive**
    /// destructure (no `..`): a field added to or renamed in `Thresholds`
    /// fails this to *compile* rather than silently shipping a
    /// stale/incomplete projection over the wire — the [FR-QM-14] drift guard
    /// [CR-067] calls for, enforced at build time rather than only at test
    /// time.
    ///
    /// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
    /// [CR-067]: ../../../docs/requests/CR-067-config-default-surfacing.md
    fn from_thresholds(t: crate::metrics::Thresholds) -> Self {
        let crate::metrics::Thresholds {
            nest,
            brain_cc,
            brain_loc,
            brain_nest,
            god_methods,
            god_span,
            clone_similarity,
            clone_min_tokens,
        } = t;
        Self {
            nesting_depth: nest,
            brain_complexity: brain_cc,
            brain_lines: brain_loc,
            brain_nesting: brain_nest,
            god_methods,
            god_span,
            clone_similarity,
            clone_min_tokens,
        }
    }

    /// Reconstruct a [`Thresholds`](crate::metrics::Thresholds) from this
    /// projection, exhaustively (no `..`) — the round-trip half of the drift
    /// test: [`from_thresholds`](Self::from_thresholds) then this must be the
    /// identity, and both directions fail to compile on an added/renamed field.
    #[cfg(test)]
    fn to_thresholds(self) -> crate::metrics::Thresholds {
        crate::metrics::Thresholds {
            nest: self.nesting_depth,
            brain_cc: self.brain_complexity,
            brain_loc: self.brain_lines,
            brain_nest: self.brain_nesting,
            god_methods: self.god_methods,
            god_span: self.god_span,
            clone_similarity: self.clone_similarity,
            clone_min_tokens: self.clone_min_tokens,
        }
    }
}

/// One policy file's current state: repo-relative path, on-disk presence, raw
/// content, and the parsed/validated model `parsed` (a [`Config`] or [`Rules`]).
///
/// The two files share this shape exactly, differing only in `parsed`'s type —
/// so they are one generic struct rather than two hand-kept-in-sync copies. The
/// stable public names are the [`ConfigFileView`]/[`RulesFileView`] aliases.
#[derive(Debug, Clone, Serialize)]
pub struct FileView<T> {
    /// Repo-relative path (e.g. `.logos/config.toml`).
    pub path: String,
    /// Whether the file exists on disk. When `false`, `content` is empty and
    /// `parsed` is the effective default — policy travels but need not exist in
    /// every worktree ([NFR-DM-04]).
    ///
    /// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
    pub exists: bool,
    /// The raw file bytes as text (empty when absent). Every rendered field
    /// traces to this real content — the editor never fabricates ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub content: String,
    /// The parsed, load-path-validated model (the effective default when absent).
    pub parsed: T,
}

/// The current `.logos/config.toml` view ([`FileView`] of a [`Config`]).
pub type ConfigFileView = FileView<Config>;

/// The current `.logos/rules.toml` view ([`FileView`] of a [`Rules`]).
pub type RulesFileView = FileView<Rules>;

/// The outcome of a [`write_config`]/[`write_rules`] — what was replaced.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigWriteOutcome {
    /// Which policy file was written.
    pub file: PolicyFile,
    /// Repo-relative path of the replaced file.
    pub path: String,
    /// Bytes written (the final on-disk size, including any provenance stamp).
    pub bytes_written: u64,
    /// Whether a provenance comment was stamped (only ever `true` for
    /// `rules.toml`, [FR-UI-12]).
    pub provenance_stamped: bool,
}

/// The outcome of a [`write_secret`] — the **masked** new key state (S-169).
///
/// Carries presence + last-4 only ([NFR-SE-07]); the raw key is never a field,
/// so a `write_secret` response can never echo the secret the editor just sent.
#[derive(Debug, Clone, Serialize)]
pub struct SecretWriteOutcome {
    /// Repo-relative path of the secret store (`.logos/secrets.toml`).
    pub path: String,
    /// The masked state of the chat key after the write (presence + last-4).
    pub chat_key: MaskedSecret,
}

/// The outcome of an [`Engine::config_apply`] — what the explicit Apply did
/// after a validated config edit ([FR-UI-13], [ADR-31]).
///
/// Two shapes for the two policy files, discriminated on the wire by the
/// internally-tagged `action` field:
/// - a `config.toml` apply **reconciles** the graph to the new admission policy
///   through the existing admission-fingerprint reconciliation ([FR-SY-07]) — an
///   O(changed) reconcile that purges now-unadmitted files, never a re-evaluation
///   of governance;
/// - a `rules.toml` apply **re-evaluates** governance / the gate against the
///   *unchanged* graph (no reindex), so the gate and quality views reflect the
///   new contract.
///
/// Every field traces to a real reconcile/scan result — the summary fabricates
/// nothing ([NFR-RA-05]).
///
/// [`Engine::config_apply`]: crate::Engine::config_apply
/// [FR-UI-13]: ../../../docs/specs/requirements/FR-UI-13.md
/// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
/// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConfigApplyOutcome {
    /// A `config.toml` apply: the pipeline reconciled the graph to the new
    /// admission policy ([FR-SY-07]). No full reindex — the reconcile is
    /// O(changed) and purges now-unadmitted files.
    Reconciled {
        /// Files (re-)entered into or removed from the graph by the reconcile —
        /// the `reconciled N files` count of the freshness line ([FR-RC-03]).
        reconciled_files: u64,
        /// `true` when a never-indexed tree degraded to a full index ([FR-RC-02]).
        full_index: bool,
        /// Reference-ledger rows still unbound after the reconcile — inbound
        /// references into purged files re-enter here, never fabricated
        /// ([NFR-RA-05]).
        unresolved_refs: u64,
        /// Files that could not be read/extracted — the `INCOMPLETE` input
        /// ([NFR-RA-11]); empty on a clean reconcile.
        files_failed: Vec<String>,
        /// Degradations folded from the reconcile — never an error.
        warnings: Vec<String>,
    },
    /// A `rules.toml` apply: governance / the gate was re-evaluated against the
    /// current graph with **no** reindex, so the new contract is reflected.
    Reevaluated {
        /// The post-apply 0–10000 quality signal ([ADR-12]); `None` = "n/a"
        /// (empty graph).
        signal: Option<u32>,
        /// Count of `rules.toml` violations the re-evaluation found ([FR-GV-02]).
        violations: usize,
        /// The freshness line of the re-evaluation ([FR-RC-03]); marked
        /// assumed-fresh because a rules change reconciles no graph.
        freshness: String,
        /// Degradations folded from the scan — never an error.
        warnings: Vec<String>,
    },
}

/// Read both policy files for `root` (the façade's `config_read`, [FR-UI-12]).
///
/// Each file is returned with its raw content and parsed model. An absent file
/// is reported `exists = false` with empty content and the effective default
/// model ([NFR-DM-04]) — not an error.
///
/// # Errors
/// A present-but-invalid file fails loud through the load path
/// ([`load_config_from_root`] / [`load_rules_from_root`]): an unknown key, a
/// non-compiling glob, or an out-of-range value is a [`ConfigError`] (exit 2),
/// exactly as the CLI would report it.
pub fn read_documents(root: &Path) -> Result<ConfigReadModel, ConfigError> {
    let config_path = root.join(CONFIG_RELPATH);
    let rules_path = root.join(RULES_RELPATH);

    let config = ConfigFileView {
        path: CONFIG_RELPATH.to_string(),
        exists: config_path.exists(),
        content: read_to_string_if_present(&config_path)?,
        parsed: load_config_from_root(root)?,
    };
    let rules = RulesFileView {
        path: RULES_RELPATH.to_string(),
        exists: rules_path.exists(),
        content: read_to_string_if_present(&rules_path)?,
        parsed: load_rules_from_root(root)?,
    };
    // The chat key is loaded only to be **masked** — the raw value never enters
    // the read-model (S-169, [NFR-SE-07]). A present-but-invalid secrets.toml
    // fails loud through the load path, like the policy files.
    let chat_key = load_secrets_from_root(root)?.chat_key_masked();
    // The defaults projection is computed purely from Rust code — never from
    // the just-loaded documents above — so it can never echo a user's current
    // value back as a "default" (CR-067, NFR-RA-05).
    let defaults = ConfigDefaults {
        config: Config::default(),
        rules: RulesDefaults {
            metric_thresholds: MetricThresholdDefaults::from_thresholds(
                MetricThresholds::default().effective(),
            ),
            constraints: Constraints::recommended(),
        },
    };
    Ok(ConfigReadModel {
        config,
        rules,
        chat_key,
        defaults,
    })
}

/// Validate a candidate `config.toml` document and, only if valid, atomically
/// replace `<root>/.logos/config.toml` ([FR-UI-12], [ADR-31]).
///
/// `candidate` is the full TOML document the editor assembled. It is run through
/// [`parse_config`] — the **same** `deny_unknown_fields` parse + glob/containment
/// validation the loader runs — so an invalid edit (unknown key, malformed TOML,
/// non-compiling/escaping glob) is rejected and the on-disk file is left
/// **byte-identical**; nothing is written until validation passes.
///
/// # Errors
/// [`ConfigError`] (exit 2) for an invalid candidate, or [`ConfigError::Write`]
/// if the atomic write itself fails (original unchanged).
pub fn write_config(root: &Path, candidate: &str) -> Result<ConfigWriteOutcome, ConfigError> {
    let path = root.join(CONFIG_RELPATH);
    // Validate-before-write: the candidate must clear the load path verbatim.
    parse_config(candidate, &path)?;
    // Checked-in policy → default (world-readable) perms; it travels in VCS.
    let bytes_written = atomic_write(&path, candidate.as_bytes(), None)?;
    Ok(ConfigWriteOutcome {
        file: PolicyFile::Config,
        path: CONFIG_RELPATH.to_string(),
        bytes_written,
        provenance_stamped: false,
    })
}

/// Validate a candidate `rules.toml` document, stamp a provenance comment, and —
/// only if valid — atomically replace `<root>/.logos/rules.toml` ([FR-UI-12],
/// [ADR-31], [BR-35]).
///
/// A pre-existing [`RULES_PROVENANCE_STAMP`] at the head of `candidate` is
/// stripped first so repeated saves do not accumulate stamps; the fresh stamp is
/// then prepended and the **stamped** document is validated through
/// [`parse_rules`] (the same path [`super::load_rules`] uses) before any write —
/// so the written file is guaranteed to parse and an invalid candidate leaves the
/// file byte-identical.
///
/// # Errors
/// [`ConfigError`] (exit 2) for an invalid candidate, or [`ConfigError::Write`]
/// if the atomic write itself fails (original unchanged).
pub fn write_rules(root: &Path, candidate: &str) -> Result<ConfigWriteOutcome, ConfigError> {
    let path = root.join(RULES_RELPATH);
    let body = candidate
        .strip_prefix(RULES_PROVENANCE_STAMP)
        .unwrap_or(candidate);
    let stamped = format!("{RULES_PROVENANCE_STAMP}{body}");
    // Validate the EXACT bytes we will write, so "stamped file still parses via
    // load_rules" holds by construction and an invalid edit is rejected before
    // touching the file.
    parse_rules(&stamped, &path)?;
    // Checked-in policy → default (world-readable) perms; it travels in VCS.
    let bytes_written = atomic_write(&path, stamped.as_bytes(), None)?;
    Ok(ConfigWriteOutcome {
        file: PolicyFile::Rules,
        path: RULES_RELPATH.to_string(),
        bytes_written,
        provenance_stamped: true,
    })
}

/// Write (or clear) the chat API key in the gitignored `.logos/secrets.toml`
/// via the same validated atomic write the policy files use (S-169, [FR-CF-06],
/// [NFR-SE-07], [NFR-RA-07]).
///
/// `api_key` is the raw key the editor posted. A blank/whitespace value
/// **clears** the key (writes a `secrets.toml` with no `api_key`), so the
/// editor's "remove the key" action and a fresh install converge on the same
/// shape. Any existing secret store is loaded first so an unrelated future
/// secret is preserved; the chat key is then set/cleared, the document
/// re-serialised, and atomically swapped — never a partial write.
///
/// The returned [`SecretWriteOutcome`] carries only the **masked** new state
/// (presence + last-4), never the raw key ([NFR-SE-07]).
///
/// # Errors
/// [`ConfigError::Parse`] if an existing `secrets.toml` does not parse (exit 2),
/// or [`ConfigError::Write`] if the atomic write itself fails (original
/// unchanged).
///
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn write_secret(root: &Path, api_key: &str) -> Result<SecretWriteOutcome, ConfigError> {
    let path = root.join(SECRETS_RELPATH);
    // Load-merge so a present-but-invalid store fails loud rather than being
    // silently overwritten, and any unrelated secret survives.
    let mut secrets = load_secrets_from_root(root)?;
    let trimmed = api_key.trim();
    secrets.chat.api_key = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    // Serialising a `Secrets` cannot itself produce invalid TOML, but round-
    // tripping the exact bytes through the load-path parser keeps the "validate
    // before write" contract honest by construction.
    let document = toml::to_string(&secrets).map_err(|source| ConfigError::InvalidValue {
        key: "chat.api_key".to_string(),
        message: format!("could not serialise secrets.toml: {source}"),
    })?;
    super::secrets::parse_secrets(&document, &path)?;
    // The credential is written 0o600 (owner-only) so the at-rest key is never
    // group/world-readable, even for the brief temp-file window ([NFR-SE-07]).
    atomic_write(&path, document.as_bytes(), Some(0o600))?;
    Ok(SecretWriteOutcome {
        path: SECRETS_RELPATH.to_string(),
        chat_key: secrets.chat_key_masked(),
    })
}

/// Read a file to a string, or the empty string when it does not exist.
///
/// A genuinely unreadable present file (permissions, non-UTF-8) is a typed
/// [`ConfigError::Io`] — the same fault the loader raises.
fn read_to_string_if_present(path: &Path) -> Result<String, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Atomically replace `target` with `bytes` via write-temp-then-rename
/// ([NFR-RA-07]). Returns the number of bytes written.
///
/// The temp file is a **sibling** of `target` (same directory ⇒ same filesystem),
/// so the `rename` is an atomic in-place swap rather than a cross-device copy: a
/// concurrent reader and a crash both see either the whole old file or the whole
/// new one, never a partial write. The temp is `sync`-ed before the rename so the
/// new contents are durable on disk before they become visible. On any failure
/// the temp is cleaned up and `target` is left untouched.
///
/// `unix_mode` sets the **temp file's** Unix permission bits *at creation* (so
/// the perms hold for the whole window the bytes exist on disk, and `rename`
/// carries them to `target`). `None` leaves the OS default (umask-dependent) —
/// correct for the world-readable checked-in policy files. The secret store
/// passes `Some(0o600)` so the API key is never even briefly group/world-readable
/// ([NFR-SE-07], defense-in-depth on the at-rest key). On non-Unix the mode is a
/// no-op (the loopback-only host is the v1 trust boundary; OS-keychain/ACL
/// storage is the deferred follow-up the NFR names).
fn atomic_write(target: &Path, bytes: &[u8], unix_mode: Option<u32>) -> Result<u64, ConfigError> {
    let write_err = |source: std::io::Error| ConfigError::Write {
        path: target.to_path_buf(),
        source,
    };

    // Ensure the `.logos/` parent exists (a fresh root may not have it yet).
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(write_err)?;
    }

    // A sibling temp path, made unique by PID so two processes writing the same
    // root never clobber each other's temp (the rename is still the only swap).
    let tmp = sibling_tmp_path(target);

    // Write + flush + fsync, then rename. A failure at any step cleans up the
    // temp and propagates a typed Write error, leaving `target` byte-identical.
    let result = (|| {
        use std::io::Write as _;
        let mut file = create_temp(&tmp, unix_mode)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, target)
    })();

    if let Err(source) = result {
        let _ = fs::remove_file(&tmp);
        return Err(write_err(source));
    }
    Ok(bytes.len() as u64)
}

/// Create the temp file, honoring an explicit Unix permission mode at creation
/// time. On Unix with `Some(mode)`, the file is opened `create_new`-equivalent
/// (`create(true).truncate(true)`) with the mode applied via
/// [`std::os::unix::fs::OpenOptionsExt`], so it is never momentarily readable at
/// the default umask before a follow-up `chmod`. `None` (or non-Unix) falls back
/// to [`fs::File::create`].
fn create_temp(tmp: &Path, unix_mode: Option<u32>) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    {
        if let Some(mode) = unix_mode {
            use std::os::unix::fs::OpenOptionsExt as _;
            return fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(mode)
                .open(tmp);
        }
    }
    #[cfg(not(unix))]
    let _ = unix_mode; // mode is a Unix-only concept; ignored elsewhere.
    fs::File::create(tmp)
}

/// A sibling temp path for `target`: `<target>.<pid>.<tid>.tmp`, in the same
/// directory.
///
/// Keyed by **both** the process id and the thread id so two threads in the same
/// process writing the same root (e.g. two concurrent surface requests) never
/// compute the same temp path — without it, one thread's `File::create` would
/// truncate the file the other is mid-`write_all` into, and the rename would
/// publish corrupt bytes with no error. `rename` is still the only swap, so even
/// concurrent writers each publish their own whole document, last-writer-wins.
fn sibling_tmp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    let tmp_name = format!(
        ".{name}.{}.{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    );
    match target.parent() {
        Some(parent) => parent.join(tmp_name),
        None => PathBuf::from(tmp_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `contents` to `<root>/.logos/<name>`, creating `.logos/` as needed.
    fn seed(root: &Path, name: &str, contents: &str) {
        let dir = root.join(".logos");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), contents).unwrap();
    }

    // ── config_read: content + parsed model ──────────────────────────────────

    #[test]
    fn read_documents_returns_content_and_parsed_model() {
        // FR-UI-12 AC1: config_read returns the current config.toml and rules.toml
        // (content + parsed model) for the served root.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "config.toml", "max_file_size = 4096\n");
        seed(dir.path(), "rules.toml", "[constraints]\nmax_cycles = 2\n");

        let docs = read_documents(dir.path()).unwrap();

        assert!(docs.config.exists);
        assert_eq!(docs.config.path, ".logos/config.toml");
        assert!(docs.config.content.contains("max_file_size = 4096"));
        assert_eq!(docs.config.parsed.max_file_size, 4096);

        assert!(docs.rules.exists);
        assert_eq!(docs.rules.path, ".logos/rules.toml");
        assert!(docs.rules.content.contains("max_cycles = 2"));
        assert_eq!(docs.rules.parsed.constraints.max_cycles, Some(2));
    }

    #[test]
    fn read_documents_reports_absent_files_as_defaults() {
        // NFR-DM-04: an absent policy file is exists=false + empty content + the
        // effective default model — not an error.
        let dir = tempfile::tempdir().unwrap();
        let docs = read_documents(dir.path()).unwrap();

        assert!(!docs.config.exists);
        assert!(docs.config.content.is_empty());
        assert_eq!(docs.config.parsed, Config::default());

        assert!(!docs.rules.exists);
        assert!(docs.rules.content.is_empty());
        assert_eq!(docs.rules.parsed, Rules::default());
    }

    // ── CR-067 / BR-37: the `defaults` projection ─────────────────────────────

    /// FR-UI-12/BR-37 acceptance: `read_documents` always returns the same
    /// code-sourced `defaults`, independent of the on-disk documents — editing
    /// the live config never perturbs what the editor calls "default"
    /// ([NFR-RA-05]).
    #[test]
    fn defaults_projection_is_code_sourced_and_independent_of_the_live_documents() {
        let dir = tempfile::tempdir().unwrap();
        // A live config that departs from every default/recommended value.
        seed(dir.path(), "config.toml", "max_file_size = 1\n");
        seed(
            dir.path(),
            "rules.toml",
            "[constraints]\nmax_fan_in = 7\n[metric_thresholds]\nbrain_lines = 3\n",
        );

        let docs = read_documents(dir.path()).unwrap();

        // config.toml defaults equal Config::default() whole (CR-067 CRA-02),
        // not the just-loaded (departed) live document.
        assert_eq!(docs.defaults.config, Config::default());
        assert_ne!(
            docs.defaults.config.max_file_size, docs.config.parsed.max_file_size,
            "the default must not echo the live value"
        );

        // [metric_thresholds] defaults equal Thresholds::default(), keyed by the
        // rules.toml key names (FR-QM-14).
        let d = crate::metrics::Thresholds::default();
        assert_eq!(docs.defaults.rules.metric_thresholds.nesting_depth, d.nest);
        assert_eq!(
            docs.defaults.rules.metric_thresholds.brain_complexity,
            d.brain_cc
        );
        assert_eq!(docs.defaults.rules.metric_thresholds.brain_lines, d.brain_loc);
        assert_ne!(
            docs.defaults.rules.metric_thresholds.brain_lines,
            docs.rules.parsed.metric_thresholds.brain_lines.unwrap(),
            "the default must not echo the live (departed) brain_lines value"
        );
        assert_eq!(
            docs.defaults.rules.metric_thresholds.brain_nesting,
            d.brain_nest
        );
        assert_eq!(docs.defaults.rules.metric_thresholds.god_methods, d.god_methods);
        assert_eq!(docs.defaults.rules.metric_thresholds.god_span, d.god_span);
        assert_eq!(
            docs.defaults.rules.metric_thresholds.clone_similarity,
            d.clone_similarity
        );
        assert_eq!(
            docs.defaults.rules.metric_thresholds.clone_min_tokens,
            d.clone_min_tokens
        );

        // [constraints] carry the curated recommended baselines, never the live
        // (departed) value.
        assert_eq!(docs.defaults.rules.constraints, Constraints::recommended());
        assert_ne!(
            docs.defaults.rules.constraints.max_fan_in,
            docs.rules.parsed.constraints.max_fan_in,
            "the recommended baseline must not echo the live (departed) value"
        );
    }

    /// FR-QM-14 drift guard ([CR-067] risk mitigation): reconstructing a
    /// [`Thresholds`](crate::metrics::Thresholds) from the [`MetricThresholdDefaults`]
    /// projection and comparing it to `Thresholds::default()` must round-trip
    /// exactly. Because both [`MetricThresholdDefaults::from_thresholds`] and
    /// [`MetricThresholdDefaults::to_thresholds`] destructure/construct
    /// `Thresholds` exhaustively (no `..`), a field added to or renamed in
    /// `Thresholds` without updating this module fails to **compile**, not just
    /// to pass this test — the strongest form of "fails on drift".
    ///
    /// [CR-067]: ../../../../docs/requests/CR-067-config-default-surfacing.md
    #[test]
    fn metric_threshold_defaults_round_trip_thresholds_default() {
        let want = crate::metrics::Thresholds::default();
        let projection = MetricThresholdDefaults::from_thresholds(want);
        assert_eq!(
            projection.to_thresholds(),
            want,
            "the defaults projection must round-trip Thresholds::default() exactly"
        );
    }

    #[test]
    fn read_documents_fails_loud_on_an_invalid_on_disk_file() {
        // A present-but-invalid file fails through the load path (exit 2), exactly
        // as the CLI would report it.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "config.toml", "langauges = [\"rust\"]\n"); // typo'd key
        let err = read_documents(dir.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn read_documents_fails_loud_on_an_invalid_on_disk_rules_file() {
        // The rules half of read_documents also fails loud — both branches go
        // through the load path, so an unknown rules key is exit 2.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "rules.toml", "bogus_rules_key = 99\n");
        let err = read_documents(dir.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    // ── config_write: round-trip a valid edit ────────────────────────────────

    #[test]
    fn write_config_round_trips_a_valid_edit() {
        // FR-UI-12 AC1/AC2: a valid edit replaces the file and is reflected on
        // reload, with the parsed model preserved.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "config.toml", "max_file_size = 1\n");

        let candidate = "languages = [\"rust\"]\nmax_file_size = 8192\n";
        let outcome = write_config(dir.path(), candidate).unwrap();
        assert_eq!(outcome.file, PolicyFile::Config);
        assert!(!outcome.provenance_stamped);
        assert_eq!(outcome.bytes_written, candidate.len() as u64);

        // Reload through config_read: the new model is what we wrote.
        let docs = read_documents(dir.path()).unwrap();
        assert_eq!(docs.config.content, candidate);
        assert_eq!(docs.config.parsed.max_file_size, 8192);
        assert_eq!(docs.config.parsed.languages, vec!["rust".to_string()]);
    }

    #[test]
    fn write_config_creates_the_file_when_absent() {
        // Writing into a fresh root creates `.logos/config.toml` (the atomic write
        // creates the parent dir).
        let dir = tempfile::tempdir().unwrap();
        let candidate = "max_file_size = 2048\n";
        write_config(dir.path(), candidate).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap(),
            candidate
        );
    }

    // ── config_write: invalid candidate ⇒ typed error + byte-identical file ───

    #[test]
    fn write_config_rejects_an_unknown_key_and_leaves_the_file_byte_identical() {
        // FR-UI-12 AC2 / NFR-RA-07: an invalid candidate returns a typed error and
        // leaves the file byte-identical (no partial write).
        let dir = tempfile::tempdir().unwrap();
        let original = "max_file_size = 4096\n";
        seed(dir.path(), "config.toml", original);

        let err = write_config(dir.path(), "bogus_key = 1\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
        // The file on disk is untouched.
        assert_eq!(
            fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap(),
            original
        );
    }

    #[test]
    fn write_config_rejects_a_bad_glob_without_writing() {
        // A non-compiling include glob is rejected on the same validate-before-
        // write path; an absent file stays absent (no partial write).
        let dir = tempfile::tempdir().unwrap();
        let err = write_config(dir.path(), "include = [\"src/{a\"]\n").unwrap_err();
        assert!(matches!(err, ConfigError::BadGlob { .. }));
        assert!(!dir.path().join(".logos/config.toml").exists());
    }

    #[test]
    fn write_config_rejects_an_escaping_glob_and_preserves_bytes() {
        // NFR-SE-04: an escaping glob is rejected and the original is preserved.
        let dir = tempfile::tempdir().unwrap();
        let original = "include = [\"**\"]\n";
        seed(dir.path(), "config.toml", original);
        let err = write_config(dir.path(), "include = [\"../escape/**\"]\n").unwrap_err();
        assert!(matches!(err, ConfigError::EscapingPattern { .. }));
        assert_eq!(
            fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap(),
            original
        );
    }

    // ── rules write: provenance stamp parses via the load path ───────────────

    #[test]
    fn write_rules_stamps_provenance_and_still_parses_via_load_rules() {
        // FR-UI-12 AC3: a rules.toml write includes a provenance comment and the
        // resulting file still parses via the standard load path.
        let dir = tempfile::tempdir().unwrap();
        let candidate = "[constraints]\nmax_cycles = 0\nmax_cc = 12\n";
        let outcome = write_rules(dir.path(), candidate).unwrap();
        assert_eq!(outcome.file, PolicyFile::Rules);
        assert!(outcome.provenance_stamped);

        let written = fs::read_to_string(dir.path().join(".logos/rules.toml")).unwrap();
        assert!(written.starts_with(RULES_PROVENANCE_STAMP));
        assert!(written.contains("CR-025"));

        // The stamped file parses via the standard load path with the edit intact.
        let rules = load_rules_from_root(dir.path()).unwrap();
        assert_eq!(rules.constraints.max_cycles, Some(0));
        assert_eq!(rules.constraints.max_cc, Some(12));
    }

    #[test]
    fn write_rules_does_not_accumulate_stamps_on_re_save() {
        // Re-saving the content config_read returned (which already carries the
        // stamp) must not double-stamp.
        let dir = tempfile::tempdir().unwrap();
        write_rules(dir.path(), "[constraints]\nmax_cc = 10\n").unwrap();
        let once = fs::read_to_string(dir.path().join(".logos/rules.toml")).unwrap();

        // Feed the already-stamped document straight back in.
        write_rules(dir.path(), &once).unwrap();
        let twice = fs::read_to_string(dir.path().join(".logos/rules.toml")).unwrap();

        assert_eq!(once, twice, "a re-save is idempotent — one stamp, not two");
        assert_eq!(
            twice
                .matches("Written by the Logos web config editor")
                .count(),
            1,
            "exactly one provenance stamp"
        );
    }

    #[test]
    fn write_rules_rejects_an_invalid_candidate_and_leaves_the_file_byte_identical() {
        // FR-UI-12 AC2 for rules.toml: an out-of-range value is rejected and the
        // file is left byte-identical (snapshot the on-disk bytes, not the seed
        // literal, so any write-through would be caught).
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "rules.toml", "[constraints]\nmax_cc = 10\n");
        let path = dir.path().join(".logos/rules.toml");
        let before = fs::read(&path).unwrap();

        let err =
            write_rules(dir.path(), "[metric_thresholds]\nclone_similarity = 1.5\n").unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "byte-identical after reject"
        );
    }

    #[test]
    fn write_rules_rejects_an_unknown_key_and_leaves_the_file_byte_identical() {
        // The unknown-key (Parse) rejection path for rules.toml — distinct from
        // the InvalidValue path above — also leaves the file byte-identical.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "rules.toml", "[constraints]\nmax_cc = 10\n");
        let path = dir.path().join(".logos/rules.toml");
        let before = fs::read(&path).unwrap();

        let err = write_rules(dir.path(), "bogus_rules_key = true\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            fs::read(&path).unwrap(),
            before,
            "byte-identical after reject"
        );
    }

    // ── atomic write hygiene ─────────────────────────────────────────────────

    #[test]
    fn atomic_write_fully_replaces_and_leaves_no_temp_file() {
        // A shorter rewrite must not leave trailing bytes of the longer original
        // (write-temp-then-rename replaces the whole file), and no sibling temp
        // file is left behind.
        let dir = tempfile::tempdir().unwrap();
        seed(
            dir.path(),
            "config.toml",
            "max_file_size = 999999\n# a long trailer\n",
        );
        write_config(dir.path(), "max_file_size = 1\n").unwrap();

        let logos = dir.path().join(".logos");
        assert_eq!(
            fs::read_to_string(logos.join("config.toml")).unwrap(),
            "max_file_size = 1\n"
        );
        // No leftover *.tmp sibling.
        let leftovers: Vec<_> = fs::read_dir(&logos)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
    }

    // ── secret write-back: params → config.toml, key → secrets.toml ───────────

    #[test]
    fn write_secret_persists_the_key_and_reads_back_masked() {
        // FR-CF-06 AC: saving the key writes the gitignored secrets.toml and on
        // reload it renders masked (presence + last-4) — never in plaintext in
        // the read-model the editor consumes (NFR-SE-07).
        let dir = tempfile::tempdir().unwrap();

        let outcome = write_secret(dir.path(), "sk-or-v1-abcd1234").unwrap();
        assert_eq!(outcome.path, ".logos/secrets.toml");
        assert!(outcome.chat_key.present);
        assert_eq!(outcome.chat_key.last4.as_deref(), Some("1234"));

        // The key persists in secrets.toml on disk…
        let on_disk = fs::read_to_string(dir.path().join(".logos/secrets.toml")).unwrap();
        assert!(on_disk.contains("sk-or-v1-abcd1234"), "key persists at rest");

        // …but read_documents returns only the masked form (the raw key is never
        // in the read-model the surface serialises).
        let docs = read_documents(dir.path()).unwrap();
        assert!(docs.chat_key.present);
        assert_eq!(docs.chat_key.last4.as_deref(), Some("1234"));
        let serialised = serde_json::to_string(&docs).unwrap();
        assert!(
            !serialised.contains("sk-or-v1-abcd1234"),
            "the read-model must never serialise the raw key (NFR-SE-07): {serialised}"
        );
    }

    #[test]
    fn write_secret_to_config_and_key_to_secrets_are_separate_files() {
        // FR-CF-06 AC: non-secret params write to config.toml, the key to
        // secrets.toml — the key never lands in the checked-in policy file.
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            "[chat]\nmodel = \"anthropic/claude\"\nprovider = \"anthropic\"\n",
        )
        .unwrap();
        write_secret(dir.path(), "sk-secret-XYZ9").unwrap();

        let config = fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap();
        assert!(config.contains("anthropic/claude"));
        assert!(
            !config.contains("sk-secret-XYZ9"),
            "the key must NEVER appear in config.toml (FR-CF-06)"
        );
        let secrets = fs::read_to_string(dir.path().join(".logos/secrets.toml")).unwrap();
        assert!(secrets.contains("sk-secret-XYZ9"), "key lives in secrets.toml");
    }

    #[test]
    fn write_secret_blank_clears_the_key() {
        // A blank value clears the key — the editor's "remove the key" action.
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "sk-present-9999").unwrap();
        assert!(read_documents(dir.path()).unwrap().chat_key.present);

        let outcome = write_secret(dir.path(), "   ").unwrap();
        assert!(!outcome.chat_key.present, "blank clears the key");
        assert!(
            outcome.chat_key.last4.is_none(),
            "a cleared key reports no last-4"
        );
        assert!(!read_documents(dir.path()).unwrap().chat_key.present);
    }

    #[test]
    fn write_config_rejects_out_of_range_chat_value_and_leaves_file_byte_identical() {
        // FR-CF-06 AC: an out-of-range `[chat]` value is rejected through the
        // write-back validate path with no partial write — the file is left
        // byte-identical (the chat.rs unit test proves the validator; this proves
        // the writeback seam honors it before the atomic swap).
        let dir = tempfile::tempdir().unwrap();
        let original = "max_file_size = 4096\n";
        seed(dir.path(), "config.toml", original);

        let err = write_config(dir.path(), "[chat]\nmax_tool_calls = 0\n").unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { ref key, .. } if key == "chat.max_tool_calls"),
            "an out-of-range budget value is rejected naming chat.max_tool_calls: {err:?}"
        );
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap(),
            original,
            "no partial write — the file is byte-identical after the reject"
        );
    }

    // ── [wiki] write-back: rejection + round-trip persistence ([FR-CF-07]) ────

    #[test]
    fn write_config_rejects_unknown_wiki_key_and_leaves_file_byte_identical() {
        // FR-CF-07 AC: an unknown `[wiki]` key is rejected with no partial write.
        let dir = tempfile::tempdir().unwrap();
        let original = "max_file_size = 4096\n";
        seed(dir.path(), "config.toml", original);

        let err = write_config(dir.path(), "[wiki]\nbogus = 1\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap(),
            original,
            "no partial write — the file is byte-identical after the reject"
        );
    }

    #[test]
    fn write_config_rejects_blank_wiki_model_and_leaves_file_byte_identical() {
        // FR-CF-07 AC: an out-of-range (blank) `[wiki].model` is rejected with no
        // partial write, naming `wiki.model`.
        let dir = tempfile::tempdir().unwrap();
        let original = "max_file_size = 4096\n";
        seed(dir.path(), "config.toml", original);

        let err = write_config(dir.path(), "[wiki]\nmodel = \"   \"\n").unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { ref key, .. } if key == "wiki.model"),
            "a blank wiki.model is rejected naming wiki.model: {err:?}"
        );
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            fs::read_to_string(dir.path().join(".logos/config.toml")).unwrap(),
            original,
            "no partial write — the file is byte-identical after the reject"
        );
    }

    #[test]
    fn write_config_round_trips_a_wiki_model_across_a_reload() {
        // FR-CF-07 AC: setting `[wiki].model` persists across restarts — a fresh
        // `read_documents` (simulating a reload) sees the same resolved model,
        // independent of `[chat].model`.
        let dir = tempfile::tempdir().unwrap();
        let candidate = "[chat]\nmodel = \"chat/model\"\n\n[wiki]\nmodel = \"wiki/model\"\n";
        write_config(dir.path(), candidate).unwrap();

        let docs = read_documents(dir.path()).unwrap();
        assert_eq!(docs.config.parsed.wiki.model.as_deref(), Some("wiki/model"));
        assert_eq!(docs.config.parsed.chat.model.as_deref(), Some("chat/model"));

        let resolved = docs
            .config
            .parsed
            .effective_wiki_model(&load_secrets_from_root(dir.path()).unwrap());
        assert_eq!(resolved.model.as_deref(), Some("wiki/model"));
    }

    #[cfg(unix)]
    #[test]
    fn write_secret_is_owner_only_0600() {
        // NFR-SE-07 (defense-in-depth): the at-rest credential is never group/
        // world-readable. The policy files keep their default (umask) perms.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        write_secret(dir.path(), "sk-owner-only-1234").unwrap();
        let mode = fs::metadata(dir.path().join(".logos/secrets.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "secrets.toml must be owner read/write only");
    }

    #[test]
    fn write_secret_rejects_an_invalid_existing_store_without_clobbering() {
        // A present-but-invalid secrets.toml fails loud (exit 2) rather than being
        // silently overwritten — the load-merge surfaces the fault first.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), "secrets.toml", "[chat]\nbogus_secret = 1\n");
        let before = fs::read(dir.path().join(".logos/secrets.toml")).unwrap();

        let err = write_secret(dir.path(), "sk-new").unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            fs::read(dir.path().join(".logos/secrets.toml")).unwrap(),
            before,
            "an invalid store is left byte-identical"
        );
    }
}
