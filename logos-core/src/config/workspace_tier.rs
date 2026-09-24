//! The **workspace root as a config root** ([FR-WS-30], [ADR-67]) — the
//! engine-free seam the web surface's `/api/v1/workspace/config*` routes call.
//!
//! The workspace root holds the second tier [`resolve_chat`](super::resolve_chat)
//! inherits from, and it holds no graph: constructing an [`Engine`](crate::Engine)
//! there would open a store at the root, which is the fault the ADR-67 exception
//! exists to avoid. So these functions reach the **same** read and writers the
//! engine façade's `config_read` / `config_write` / `config_write_secret` reach
//! ([`read_documents`], [`write_config`], [`write_secret`]) — validate-before-write,
//! the atomic replace and the credential's 0o600 mode come unchanged — and emit
//! the **same** telemetry event each of those façade methods emits, through the
//! one emission point ([`traced`](crate::observability::traced)). That is the
//! "equivalent surface-side scope" ADR-67 promises in place of the façade's
//! tracing, so observability stays whole: a workspace-tier save is booked
//! exactly as a member's save is, under whatever surface the caller entered.
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
//! [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
//! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md

use std::path::Path;

use anyhow::Result;

use super::{read_documents, write_config, write_secret};
use super::{ConfigError, ConfigReadModel, ConfigWriteOutcome, SecretWriteOutcome};
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

/// The workspace root's config read-model: [`read_documents`] at `root` with
/// **no** tier above it, because the workspace root has none. The effective-chat
/// slice's origins are therefore relative to this root — `member` means
/// *declared here*, `unset` that it is not, and `workspace` never appears.
///
/// # Errors
/// A present-but-invalid file at `root` fails loud, as [`read_documents`] does.
pub fn read_workspace_documents(root: &Path) -> Result<ConfigReadModel> {
    traced(Tool::ConfigRead, || Ok(read_documents(root, None)?))
}

/// [`write_config`] at the workspace root: validate the candidate, never the
/// file it replaces, and only then swap it in atomically — so a save over a
/// broken workspace `config.toml` succeeds and repairs it.
///
/// # Errors
/// The [`ConfigError`](super::ConfigError) [`write_config`] returns, carried in
/// the `anyhow` chain so a caller can still map validation vs I/O faults.
pub fn write_workspace_config(root: &Path, candidate: &str) -> Result<ConfigWriteOutcome> {
    traced(Tool::ConfigWrite, || {
        // Validate first, so a refused candidate leaves the root untouched —
        // not even a `.logos/` created for it.
        super::parse_config(candidate, &root.join(super::CONFIG_RELPATH))?;
        prepare_tier_dir(root)?;
        Ok(write_config(root, candidate)?)
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

        write_workspace_config(&root, "[chat]\nmodel = \"ws/m\"\n").expect("writes");
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
        assert!(write_workspace_config(root.path(), "modle = 1\n").is_err());
        assert!(!root.path().join(".logos").exists(), "no directory created for a refused save");
    }
}
