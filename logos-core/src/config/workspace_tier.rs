//! The **workspace root as a config root** ([FR-WS-30], [ADR-67]) — the
//! engine-free seam the web surface's `/api/v1/workspace/config*` routes call.
//!
//! The workspace root holds the second tier [`resolve_chat`](super::resolve_chat)
//! inherits from, and it holds no graph: constructing an [`Engine`](crate::Engine)
//! there would open a store at the root, which is the fault the ADR-67 exception
//! exists to avoid. So these functions reach the **same** parser and writers the
//! engine façade's `config_read` / `config_write` / `config_write_secret` reach
//! ([`parse_config`](super::parse_config), [`write_config`], [`write_secret`]) —
//! validate-before-write, the atomic replace and the credential's 0o600 mode
//! come unchanged — and emit the **same** telemetry event each of those façade
//! methods emits, through the one emission point
//! ([`traced`](crate::observability::traced)). That is the "equivalent
//! surface-side scope" ADR-67 promises in place of the façade's tracing, so
//! observability stays whole: a workspace-tier save is booked exactly as a
//! member's save is, under whatever surface the caller entered.
//!
//! # The read is the repair path; the save refuses a clobber (S-451 T2)
//! The read and the policy save carry the workspace manifest's two editor
//! properties ([FR-UI-38], [CR-145]), mirroring
//! [`manifest::read_document`](crate::federation::manifest::read_document) /
//! [`manifest::save_document`](crate::federation::manifest::save_document):
//!
//! - a present-but-invalid file is **delivered**, not refused — the literal
//!   document, its load fingerprint and a `None` parse — because the editor is
//!   the one surface built to write it, and it cannot repair a document it was
//!   refused. A member's config read stays fail-loud; this one does not.
//! - a save carries the load's fingerprint and is **refused** when the file
//!   changed on disk since ([`TierSaveOutcome::Conflict`]), since the file is
//!   edited by hand while a tab is open.
//!
//! A fault is stated by **file and position (or key) only** ([`read_fault`]),
//! never with the parser's rendering, which quotes the offending line, nor with a
//! validation message, which quotes the rejected value: in `secrets.toml` either
//! can be the key ([NFR-SE-07]). The rule is kept uniform across both files
//! rather than argued per file.
//!
//! Both writers also maintain `<root>/.logos/.gitignore` — the same managed
//! block `logos init` writes into a member's `.logos/` ([FR-IN-04]) — before
//! anything else lands there. A member's `.logos/` always has it, because `init`
//! created the directory; the workspace root's is created by the first save
//! here, so the credential ([NFR-SE-07]) must not depend on `logos init
//! --workspace` having run with a build that knew about it, nor on the root
//! being the top level of its own repository.
//!
//! [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
//! [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
//! [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
//! [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
//! [CR-145]: ../../../docs/requests/CR-145-workspace-level-chat-configuration.md
//! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use super::atomic::fingerprint;
use super::{load_secrets_from_root, parse_config, resolve_chat, write_config, write_secret};
use super::{Config, ConfigError, EffectiveChat, MaskedSecret, SecretWriteOutcome};
use super::{CONFIG_RELPATH, SECRETS_RELPATH};
use crate::observability::{traced, Tool};

/// Create `<root>/.logos/` and its managed `.gitignore` before a workspace-tier
/// write lands in it. A failure is a [`ConfigError::Write`] — an I/O fault on
/// the server's side, never the client's edit — so the web surface answers it
/// `500` exactly as it answers a failed atomic write.
fn prepare_tier_dir(root: &Path) -> Result<(), ConfigError> {
    let dir = root.join(".logos");
    let write_err = |source: std::io::Error| ConfigError::Write {
        path: dir.join(".gitignore"),
        source,
    };
    std::fs::create_dir_all(&dir).map_err(write_err)?;
    crate::init::logos_dir_gitignore(root)
        .map(|_| ())
        .map_err(|err| write_err(std::io::Error::other(format!("{err:#}"))))
}

/// The workspace tier's `config.toml` as the editor loads it — the manifest
/// editor's [`ManifestDocument`](crate::federation::manifest::ManifestDocument)
/// shape, plus `exists`, because unlike the manifest this file is routinely
/// absent (a tier nobody has declared yet).
///
/// `content` is what the raw pane shows and what a save posts back, never a
/// re-serialisation of `parsed`. An absent file reads as the empty document:
/// `content` empty, `fingerprint` the empty document's, and `parsed` the
/// effective default ([NFR-DM-04]) — so a first save against that fingerprint is
/// refused if someone created the file meanwhile.
///
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
#[derive(Debug, Clone, Serialize)]
pub struct TierConfigFile {
    /// Relative to the workspace root (`.logos/config.toml`).
    pub path: String,
    /// Whether the file exists on disk.
    pub exists: bool,
    /// The literal on-disk document (empty when absent).
    pub content: String,
    /// The load fingerprint of `content`'s bytes — posted back with a save.
    pub fingerprint: String,
    /// The parsed, load-path-validated model, or `None` when `content` does not
    /// parse or validate.
    pub parsed: Option<Config>,
    /// Why `content` does not parse or validate — the file and the position or
    /// key only, never a fragment of it ([`read_fault`]); `None` exactly when
    /// `parsed` is `Some`.
    pub error: Option<String>,
}

/// The workspace tier as its editor group loads it ([FR-WS-30], [FR-UI-38]).
///
/// Each half is reported on its own, so a fault in one never hides the other:
/// a broken `secrets.toml` leaves the policy document editable, and a broken
/// `config.toml` leaves the key state readable. No `rules.toml` is read — a
/// rules document at the workspace root is read by nothing (workspace rules
/// are the manifest's `[governance]`), so it must not be able to fail this read
/// either.
///
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
/// [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceTierDocument {
    /// `<root>/.logos/config.toml`.
    pub config: TierConfigFile,
    /// The **masked** credential (presence + last-4, [NFR-SE-07]), or `None` when
    /// `secrets.toml` cannot be read — then [`chat_key_error`](Self::chat_key_error)
    /// says why. No key state is invented for a store that was not read.
    ///
    /// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
    pub chat_key: Option<MaskedSecret>,
    /// Why `secrets.toml` cannot be read — the file and the position (or the I/O
    /// error) only, never a fragment of the store ([`read_fault`]); `None` exactly when `chat_key` is
    /// `Some`.
    pub chat_key_error: Option<String>,
    /// The effective chat resolution at this root, with no tier above it, so its
    /// origins are relative to this root: `member` means *declared here*, `unset`
    /// that it is not, and `workspace` never appears. `None` when either file
    /// faults, since the resolution reads both.
    pub effective_chat: Option<EffectiveChat>,
}

/// What a [`write_workspace_config`] did — the same arms, and the same wire shape
/// (internally tagged on `outcome`), as the manifest editor's
/// [`ManifestSaveOutcome`](crate::federation::manifest::ManifestSaveOutcome), so
/// the SPA reads one save-outcome type for both files. Each arm names the
/// fingerprint the editor must hold from now on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TierSaveOutcome {
    /// The candidate replaced `config.toml` atomically.
    Written {
        /// Relative to the workspace root (`.logos/config.toml`).
        path: String,
        /// The size of the document now on disk.
        bytes_written: u64,
        /// The fingerprint of the document now on disk.
        fingerprint: String,
    },
    /// The candidate is byte-identical to the file on disk (an absent file reads
    /// as the empty document), so **nothing was written**.
    Unchanged {
        /// Relative to the workspace root.
        path: String,
        /// The fingerprint of the (untouched) document on disk.
        fingerprint: String,
    },
    /// The file changed on disk since the editor loaded it — edited, created or
    /// removed — so the save was **refused** and nothing was written. Carries
    /// what is on disk now (empty for a removed file).
    Conflict {
        /// Relative to the workspace root.
        path: String,
        /// The fingerprint the save was made against (the editor's load).
        loaded_fingerprint: String,
        /// The fingerprint of the document on disk now.
        disk_fingerprint: String,
        /// The document on disk now.
        disk_content: String,
    },
}

/// A read fault stated the way a surface may render it — by the file (`rel`,
/// relative to the workspace root) and where it went wrong, **never a fragment of
/// the file** ([NFR-SE-07]):
///
/// - a **parse** fault by its position only, because the TOML error's rendering
///   quotes the offending line and its message can quote a value — in
///   `secrets.toml` either can be the key;
/// - a **validation** fault by the key it names, because its message carries the
///   rejected value (`InvalidValue`) or the pattern itself (`BadGlob`,
///   `EscapingPattern`);
/// - an **I/O** fault by its own message, which is the path and the OS error.
///
/// The rule `resolution_fault` in the web crate's chat module applies to the
/// same errors; the position is read from the error's rendering, where both the
/// native TOML error (`TOML parse error at line L, column C`) and the redacted
/// `secrets.toml` one (`… at line L, column C (…)`) state it.
///
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
fn read_fault(rel: &str, err: &ConfigError) -> String {
    match err {
        ConfigError::Parse { source, .. } => {
            let at = parse_position(&source.to_string())
                .map(|position| format!(" (at {position})"))
                .unwrap_or_default();
            format!(
                "{rel} is not valid TOML with only known keys{at}. The parser's detail is not \
                 shown, because it can quote the file."
            )
        }
        ConfigError::InvalidValue { key, .. } => format!(
            "{rel} declares an invalid value for `{key}`. The value is not shown; the raw pane \
             holds the document."
        ),
        ConfigError::BadGlob { .. } | ConfigError::EscapingPattern { .. } => format!(
            "{rel} declares a glob pattern that does not compile or escapes the project root. The \
             pattern is not shown; the raw pane holds the document."
        ),
        io @ (ConfigError::Io { .. } | ConfigError::Write { .. } | ConfigError::InvalidRoot { .. }) => {
            format!("{rel}: {io}")
        }
    }
}

/// The first `line L, column C` in `rendered`, verbatim — or `None` when it
/// states no position.
fn parse_position(rendered: &str) -> Option<String> {
    let digits = |s: &str| s.chars().take_while(char::is_ascii_digit).count();
    let mut rest = rendered;
    while let Some(at) = rest.find("line ") {
        let tail = &rest[at + "line ".len()..];
        let line = digits(tail);
        if line > 0 {
            if let Some(after) = tail[line..].strip_prefix(", column ") {
                let column = digits(after);
                if column > 0 {
                    return Some(format!("line {}, column {}", &tail[..line], &after[..column]));
                }
            }
        }
        rest = tail;
    }
    None
}

/// Read `<root>/.logos/config.toml` to its literal text, or the empty document
/// when it is absent.
fn read_tier_config(root: &Path) -> Result<(bool, String), ConfigError> {
    let path = root.join(CONFIG_RELPATH);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((true, text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok((false, String::new())),
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

/// The workspace tier's editor read-model ([`WorkspaceTierDocument`]).
///
/// A file that reads but does not parse is **not** an error: it is reported in
/// its half, so the editor can open over it and repair it.
///
/// # Errors
/// [`ConfigError::Io`] when `config.toml` exists but cannot be read (its content
/// cannot be delivered, so there is nothing to repair from).
pub fn read_workspace_documents(root: &Path) -> Result<WorkspaceTierDocument> {
    traced(Tool::ConfigRead, || {
        let (exists, content) = read_tier_config(root)?;
        let (parsed, error) = if exists {
            match parse_config(&content, &root.join(CONFIG_RELPATH)) {
                Ok(config) => (Some(config), None),
                Err(err) => (None, Some(read_fault(CONFIG_RELPATH, &err))),
            }
        } else {
            (Some(Config::default()), None)
        };
        let (chat_key, chat_key_error) = match load_secrets_from_root(root) {
            Ok(secrets) => (Some(secrets.chat_key_masked()), None),
            Err(err) => (None, Some(read_fault(SECRETS_RELPATH, &err))),
        };
        // Only over two readable halves: the resolution reads both.
        let effective_chat = match (&error, &chat_key_error) {
            (None, None) => Some(resolve_chat(root, None)?.into()),
            _ => None,
        };
        Ok(WorkspaceTierDocument {
            config: TierConfigFile {
                path: CONFIG_RELPATH.to_string(),
                exists,
                fingerprint: fingerprint(content.as_bytes()),
                content,
                parsed,
                error,
            },
            chat_key,
            chat_key_error,
            effective_chat,
        })
    })
}

/// Replace `<root>/.logos/config.toml` with `candidate`, made against the load
/// whose fingerprint is `loaded_fingerprint` — the order and posture of
/// [`manifest::save_document`](crate::federation::manifest::save_document):
///
/// 1. **Validate the candidate**, never the file it replaces — so a save over a
///    broken file succeeds and repairs it, and a refused candidate leaves the
///    root untouched (not even a `.logos/` created for it).
/// 2. **Byte-identical to disk ⇒ [`Unchanged`](TierSaveOutcome::Unchanged)**,
///    nothing written; decided before the fingerprint, since a candidate equal to
///    what is on disk overwrites nobody's edit.
/// 3. **The disk moved since the load ⇒ [`Conflict`](TierSaveOutcome::Conflict)**,
///    nothing written.
/// 4. Otherwise [`write_config`] swaps the candidate in atomically, verbatim.
///
/// Steps 3 and 4 are a compare-then-swap without a lock, as the manifest's are:
/// a write landing between the read and the rename is not detected.
///
/// # Errors
/// The [`ConfigError`] the parser raises for a rejected candidate, carried in
/// the `anyhow` chain so a caller can still map validation vs I/O faults;
/// [`ConfigError::Io`] reading the current file; [`ConfigError::Write`] when the
/// atomic replace fails (the file is then unchanged).
pub fn write_workspace_config(
    root: &Path,
    candidate: &str,
    loaded_fingerprint: &str,
) -> Result<TierSaveOutcome> {
    traced(Tool::ConfigWrite, || {
        let path = root.join(CONFIG_RELPATH);
        parse_config(candidate, &path)?;
        // Bytes, not text: the comparison is of what is on disk, whatever it holds.
        let current = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(ConfigError::Io { path, source }.into()),
        };
        let disk_fingerprint = fingerprint(&current);
        if current == candidate.as_bytes() {
            return Ok(TierSaveOutcome::Unchanged {
                path: CONFIG_RELPATH.to_string(),
                fingerprint: disk_fingerprint,
            });
        }
        if disk_fingerprint != loaded_fingerprint {
            return Ok(TierSaveOutcome::Conflict {
                path: CONFIG_RELPATH.to_string(),
                loaded_fingerprint: loaded_fingerprint.to_string(),
                disk_fingerprint,
                disk_content: String::from_utf8_lossy(&current).into_owned(),
            });
        }
        prepare_tier_dir(root)?;
        let written = write_config(root, candidate)?;
        Ok(TierSaveOutcome::Written {
            path: written.path,
            bytes_written: written.bytes_written,
            fingerprint: fingerprint(candidate.as_bytes()),
        })
    })
}

/// [`write_secret`] at the workspace root: write (or, blank, clear) the
/// credential every member that declares none inherits.
///
/// # Errors
/// The [`ConfigError`](super::ConfigError) [`write_secret`] returns (an
/// unparsable existing store is refused, never overwritten).
pub fn write_workspace_secret(root: &Path, api_key: &str) -> Result<SecretWriteOutcome> {
    traced(Tool::ConfigWriteSecret, || {
        prepare_tier_dir(root)?;
        Ok(write_secret(root, api_key)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;

    /// `git` isolated from the host's global/system config and excludes file,
    /// so git's verdict below is about this code and nothing else.
    fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "core.excludesFile=/dev/null"])
            .args(args)
            .output()
            .expect("git is on PATH")
    }

    /// A workspace root nested inside an enclosing repository is not the top
    /// level of one, so enablement writes no root `.gitignore` there — and the
    /// credential written into it must still be ignored by the enclosing
    /// repository ([NFR-SE-07]). The writer's own `.logos/.gitignore` is what
    /// does it; the policy beside it still travels.
    #[test]
    fn a_credential_at_a_root_nested_in_an_enclosing_repository_is_ignored_by_it() {
        let outer = tempfile::tempdir().unwrap();
        assert!(git(outer.path(), &["init", "-q", "-b", "main"]).status.success());
        let root = outer.path().join("estate");
        std::fs::create_dir_all(&root).unwrap();

        write_workspace_config(&root, "[chat]\nmodel = \"ws/m\"\n", &fingerprint(b"")).expect("writes");
        write_workspace_secret(&root, "sk-nested-root-key-nr06").expect("writes");

        let ignores = |rel: &str| git(outer.path(), &["check-ignore", "-q", rel]).status.success();
        assert!(ignores("estate/.logos/secrets.toml"), "the enclosing repository ignores the key");
        assert!(!ignores("estate/.logos/config.toml"), "the workspace chat policy still travels");
    }

    /// A refused save touches nothing: the candidate is validated before
    /// `.logos/` or its `.gitignore` is created ([NFR-RA-07]).
    #[test]
    fn a_refused_workspace_save_creates_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(write_workspace_config(root.path(), "modle = 1\n", &fingerprint(b"")).is_err());
        assert!(!root.path().join(".logos").exists(), "no directory created for a refused save");
    }

    /// Seed `<root>/.logos/<name>` with `contents`.
    fn seed(root: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(root.join(".logos")).unwrap();
        std::fs::write(root.join(".logos").join(name), contents).unwrap();
    }

    /// A broken `config.toml` is delivered for repair: the literal bytes, their
    /// fingerprint, no parse, and a fault naming the file and position only.
    #[test]
    fn a_broken_tier_config_is_read_with_its_fingerprint_and_a_position_only_fault() {
        let root = tempfile::tempdir().unwrap();
        let broken = "[chat]\nmodel = \"ws/kept\"\nmodle = \"ws/typo\"\n";
        seed(root.path(), "config.toml", broken);

        let doc = read_workspace_documents(root.path()).expect("a parse fault is not a read fault");
        assert_eq!(doc.config.content, broken);
        assert!(doc.config.exists);
        assert_eq!(doc.config.fingerprint, fingerprint(broken.as_bytes()));
        assert!(doc.config.parsed.is_none());
        let error = doc.config.error.expect("the fault is stated");
        assert!(error.starts_with(".logos/config.toml is not valid TOML"), "{error}");
        assert!(error.contains("(at line 3, column 1)"), "{error}");
        assert!(!error.contains("modle") && !error.contains("ws/typo"), "no snippet: {error}");
        assert!(doc.effective_chat.is_none(), "no resolution over an unreadable half");
        assert_eq!(doc.chat_key, Some(MaskedSecret::from_key(None)), "the key half is still read");
    }

    /// A broken `secrets.toml` whose broken line IS the key: the key half is
    /// `None`, the fault names file and position, and no fragment of the store
    /// reaches the serialized read-model ([NFR-SE-07]).
    #[test]
    fn a_broken_tier_secret_store_is_named_by_file_and_position_and_never_serialized() {
        let root = tempfile::tempdir().unwrap();
        let key = "sk-unquoted-core-fixture-cf19";
        seed(root.path(), "secrets.toml", &format!("[chat]\napi_key = {key}\n"));
        seed(root.path(), "config.toml", "[chat]\nmodel = \"ws/policy\"\n");

        let doc = read_workspace_documents(root.path()).expect("a broken store is not a read fault");
        assert!(doc.chat_key.is_none());
        let error = doc.chat_key_error.clone().expect("the fault is stated");
        assert!(error.starts_with(".logos/secrets.toml is not valid TOML"), "{error}");
        assert!(error.contains("(at line 2, column"), "{error}");
        let json = serde_json::to_string(&doc).unwrap();
        for fragment in [key, "unquoted", "cf19", "api_key = "] {
            assert!(!json.contains(fragment), "`{fragment}` serialized: {json}");
        }
        assert!(doc.config.parsed.is_some(), "the policy half is unaffected");
    }

    /// The position is read from both renderings a parse fault can have, and a
    /// near miss that is not a position is not one.
    #[test]
    fn parse_position_reads_both_renderings_and_rejects_near_misses() {
        assert_eq!(
            parse_position("TOML parse error at line 3, column 1\n  |\n3 | modle = 1\n").as_deref(),
            Some("line 3, column 1")
        );
        assert_eq!(
            parse_position("invalid string at line 2, column 11 (the secret store's contents are not echoed)").as_deref(),
            Some("line 2, column 11")
        );
        for miss in ["line , column 1", "line 2 column 1", "line 2, column x", "line 2, col 1", "no position"] {
            assert_eq!(parse_position(miss), None, "{miss:?} is not a position");
        }
        // A non-position `line …` before the real one is skipped, not a stop.
        assert_eq!(parse_position("the line x, then line 4, column 2").as_deref(), Some("line 4, column 2"));
    }

    /// The save's four steps, in order: a stale load conflicts and writes
    /// nothing; identity writes nothing whatever the load; a fresh load writes.
    #[test]
    fn a_tier_save_conflicts_on_a_stale_load_and_writes_nothing_on_identity() {
        let root = tempfile::tempdir().unwrap();
        let loaded = "[chat]\nmodel = \"ws/loaded\"\n";
        seed(root.path(), "config.toml", loaded);
        let at_load = fingerprint(loaded.as_bytes());
        let by_hand = "[chat]\nmodel = \"ws/by-hand\"\n";
        seed(root.path(), "config.toml", by_hand);
        let path = root.path().join(CONFIG_RELPATH);

        let outcome = write_workspace_config(root.path(), "[chat]\nmodel = \"ws/mine\"\n", &at_load).unwrap();
        assert_eq!(
            outcome,
            TierSaveOutcome::Conflict {
                path: CONFIG_RELPATH.to_string(),
                loaded_fingerprint: at_load.clone(),
                disk_fingerprint: fingerprint(by_hand.as_bytes()),
                disk_content: by_hand.to_string(),
            }
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), by_hand, "the hand edit survives");

        let outcome = write_workspace_config(root.path(), by_hand, &at_load).unwrap();
        assert_eq!(
            outcome,
            TierSaveOutcome::Unchanged { path: CONFIG_RELPATH.to_string(), fingerprint: fingerprint(by_hand.as_bytes()) }
        );

        let mine = "[chat]\nmodel = \"ws/mine\"\n";
        let outcome = write_workspace_config(root.path(), mine, &fingerprint(by_hand.as_bytes())).unwrap();
        assert_eq!(
            outcome,
            TierSaveOutcome::Written {
                path: CONFIG_RELPATH.to_string(),
                bytes_written: mine.len() as u64,
                fingerprint: fingerprint(mine.as_bytes()),
            }
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
    }

    /// An absent tier is the empty document: a first save against its
    /// fingerprint writes, and one made after someone created the file conflicts.
    #[test]
    fn an_absent_tier_reads_as_the_empty_document_and_a_created_file_conflicts() {
        let root = tempfile::tempdir().unwrap();
        let doc = read_workspace_documents(root.path()).unwrap();
        assert!(!doc.config.exists);
        assert_eq!(doc.config.fingerprint, fingerprint(b""));
        assert!(doc.config.parsed.is_some() && doc.config.error.is_none());

        seed(root.path(), "config.toml", "[chat]\nmodel = \"ws/by-hand\"\n");
        let outcome = write_workspace_config(root.path(), "[chat]\nmodel = \"ws/mine\"\n", &doc.config.fingerprint).unwrap();
        assert!(matches!(outcome, TierSaveOutcome::Conflict { .. }), "{outcome:?}");
    }

    /// A document that parses but fails validation is delivered for repair too,
    /// and its fault names the file and the key — never the rejected value or
    /// pattern, which is a fragment of the file ([NFR-SE-07]).
    #[test]
    fn a_tier_config_failing_validation_is_read_for_repair_without_its_value() {
        for (document, named, value) in [
            ("[chat]\nmodel = \"ws/m\"\ntemperature = 7.25\n", "`chat.temperature`", "7.25"),
            ("exclude = [\"../escape-sk-vf31\"]\n", "glob pattern", "escape-sk-vf31"),
            ("include = [\"src/[unclosed-sk-vf32\"]\n", "glob pattern", "unclosed-sk-vf32"),
        ] {
            let root = tempfile::tempdir().unwrap();
            seed(root.path(), "config.toml", document);
            let doc = read_workspace_documents(root.path()).expect("a validation fault is not a read fault");
            assert!(doc.config.parsed.is_none(), "{document:?}");
            assert_eq!(doc.config.content, document);
            let error = doc.config.error.expect("the fault is stated");
            assert!(error.starts_with(".logos/config.toml declares"), "{error}");
            assert!(error.contains(named), "{error}");
            assert!(!error.contains(value), "the rejected value is not echoed: {error}");
            assert!(doc.effective_chat.is_none());
        }
    }
}
