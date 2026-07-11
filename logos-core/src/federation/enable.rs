//! `logos init --workspace` enablement orchestration ([FR-WS-02]): candidate
//! discovery for the approval gate, the non-clobber per-member `init` loop,
//! and the idempotent workspace MCP injection. The manifest write itself is
//! owned by [`super::manifest::upsert`]; the interactive gate and the
//! background index warm are CLI-surface concerns (terminal I/O / process
//! spawning) that live in the `cli` crate, not here.
//!
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::config::globs;
use crate::init::{self, InitOptions};
use crate::models::pipeline::{InitResult, InitStep};

use super::{discover_candidates, Member};

/// The `.mcp.json` server key for the workspace-wide entry — distinct from
/// the per-repo `"logos"` key so a client walking `.mcp.json` up-tree from a
/// member sees two named servers, never a silent shadow ([FR-WS-02] Notes).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub const WORKSPACE_MCP_SERVER_KEY: &str = "logos-workspace";

/// One member's non-clobber `init` outcome during workspace enablement: the
/// full step report on success, or `Degraded` with the reason — a member
/// failure never aborts the rest of the command ([FR-WS-02], [NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum MemberOutcome {
    Ready(InitResult),
    Degraded { reason: String },
}

/// One member's line in a [`WorkspaceEnableReport`].
#[derive(Debug, Serialize)]
pub struct MemberReport {
    pub name: String,
    pub root: String,
    #[serde(flatten)]
    pub outcome: MemberOutcome,
}

/// The full `logos init --workspace` report ([FR-WS-02]).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
#[derive(Debug, Serialize)]
pub struct WorkspaceEnableReport {
    pub workspace: String,
    pub root: String,
    pub members: Vec<MemberReport>,
    pub manifest: InitStep,
    pub mcp: InitStep,
}

/// Candidate members not already part of the workspace, after dropping
/// anything matching an `--exclude` glob ([FR-WS-02]): the approval-gate
/// input. Pure and testable — no interactivity, no writes.
///
/// Reuses the crate's single glob compiler ([`globs::compile`]) rather than
/// hand-rolling a second one — the same "fail loud on a bad glob" primitive
/// `config_artifacts`/`documentation` already share.
///
/// # Errors
/// A malformed `--exclude` glob ([`crate::config::ConfigError::BadGlob`]).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub fn candidates_for_approval(
    root: &Path,
    already_included: &[String],
    exclude: &[String],
) -> Result<Vec<Member>> {
    let excluded = globs::compile(exclude)?;

    Ok(discover_candidates(root)
        .into_iter()
        .filter(|m| !already_included.contains(&m.name))
        .filter(|m| !excluded.is_match(&m.name))
        .collect())
}

/// Run the non-interactive half of `init --workspace` for the approved member
/// set: a non-clobber per-member `init`, the incremental manifest upsert, and
/// the idempotent workspace MCP injection ([FR-WS-02]). Never blocks on
/// indexing — kicking off the background warm is the caller's concern.
///
/// # Errors
/// Only if the manifest or `.mcp.json` cannot be written; a member's own
/// `init` failure is caught and reported [`MemberOutcome::Degraded`], never
/// fatal to the rest of the command.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub fn enable(root: &Path, name: &str, members: &[Member]) -> Result<WorkspaceEnableReport> {
    let reports: Vec<MemberReport> = members
        .iter()
        .map(|member| MemberReport {
            name: member.name.clone(),
            root: member.root.display().to_string(),
            outcome: match crate::Engine::init_with(&member.root, &InitOptions::default()) {
                Ok(result) => MemberOutcome::Ready(result),
                // `{err:#}` (not `{err}`/`.to_string()`) to keep the causal
                // chain: `Engine::init_with`'s steps wrap I/O failures with
                // `.with_context(...)` (e.g. "writing .logos/config.toml"),
                // so the bare Display would show only that wrapper and drop
                // the actual underlying cause (permission denied, disk
                // full, …) — the same convention `cli/src/main.rs` uses.
                Err(err) => MemberOutcome::Degraded {
                    reason: format!("{err:#}"),
                },
            },
        })
        .collect();

    let member_names: Vec<String> = members.iter().map(|m| m.name.clone()).collect();
    let manifest_step = super::manifest::upsert(root, name, &member_names)?;
    let mcp_step = init::inject_mcp_entry(root, WORKSPACE_MCP_SERVER_KEY, init::mcp_server_entry())?;

    Ok(WorkspaceEnableReport {
        workspace: name.to_string(),
        root: root.display().to_string(),
        members: reports,
        manifest: manifest_step,
        mcp: mcp_step,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    fn sh_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        sh_git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("f.txt"), "x\n").unwrap();
        sh_git(dir, &["add", "."]);
        sh_git(dir, &["commit", "-q", "-m", "init"]);
    }

    // ── candidates_for_approval (FR-WS-02) ────────────────────────────────

    #[test]
    fn candidates_excludes_already_included_and_globbed_members() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));
        init_repo(&tmp.path().join("legacy-billing"));

        let already = vec!["api".to_string()];
        let candidates = candidates_for_approval(tmp.path(), &already, &["legacy-*".to_string()])
            .expect("compiles and scans");

        let names: Vec<&str> = candidates.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["web"], "api already included, legacy-billing excluded");
    }

    #[test]
    fn candidates_rejects_a_malformed_exclude_glob() {
        let tmp = TempDir::new().unwrap();
        assert!(candidates_for_approval(tmp.path(), &[], &["a{b".to_string()]).is_err());
    }

    // ── enable (FR-WS-02) ──────────────────────────────────────────────────

    #[test]
    fn enable_initialises_each_member_writes_manifest_and_injects_one_mcp_entry() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));

        let members = discover_candidates(tmp.path());
        assert_eq!(members.len(), 2);

        let report = enable(tmp.path(), "shop", &members).expect("enables");
        assert_eq!(report.workspace, "shop");
        assert_eq!(report.members.len(), 2);
        for m in &report.members {
            assert!(matches!(m.outcome, MemberOutcome::Ready(_)), "{}: {:?}", m.name, m.outcome);
            assert!(tmp.path().join(&m.name).join(".logos/logos.db").is_file());
        }

        let manifest = fs::read_to_string(tmp.path().join(super::super::manifest::MANIFEST_FILENAME)).unwrap();
        assert!(manifest.contains("name = \"shop\""));

        let mcp: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert!(mcp["mcpServers"]["logos-workspace"].is_object());
        assert!(mcp["mcpServers"].get("logos").is_none(), "no per-repo entry at the parent");
    }

    #[test]
    fn enable_is_idempotent_on_a_second_run() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        let members = discover_candidates(tmp.path());

        enable(tmp.path(), "shop", &members).unwrap();
        // A second run over the same members must not duplicate the MCP entry
        // or clobber the member's own config.
        let config_path = tmp.path().join("api").join(".logos").join("config.toml");
        fs::write(&config_path, "# hand-edited\n").unwrap();

        let report = enable(tmp.path(), "shop", &members).expect("re-enables");
        assert_eq!(report.manifest.action, crate::models::pipeline::InitAction::Unchanged);
        assert_eq!(report.mcp.action, crate::models::pipeline::InitAction::Unchanged);
        assert_eq!(fs::read_to_string(&config_path).unwrap(), "# hand-edited\n", "never clobbered");

        let mcp: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1, "still exactly one entry");
    }

    #[test]
    fn enable_degrades_a_member_whose_init_fails_without_aborting_the_rest() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));

        // A member "root" that is actually a plain file: `Engine::init_with`
        // fails trying to create `.logos/` under it, the other member still
        // succeeds.
        let broken_root = tmp.path().join("ghost");
        fs::write(&broken_root, "not a directory").unwrap();
        let broken = Member {
            name: "ghost".to_string(),
            root: broken_root,
        };
        let ok = discover_candidates(tmp.path());
        let members: Vec<Member> = ok.into_iter().chain(std::iter::once(broken)).collect();

        let report = enable(tmp.path(), "shop", &members).expect("the command itself still succeeds");
        let ghost = report.members.iter().find(|m| m.name == "ghost").unwrap();
        assert!(matches!(ghost.outcome, MemberOutcome::Degraded { .. }));
        let api = report.members.iter().find(|m| m.name == "api").unwrap();
        assert!(matches!(api.outcome, MemberOutcome::Ready(_)));
    }
}
