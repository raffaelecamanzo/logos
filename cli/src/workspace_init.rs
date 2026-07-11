//! `logos init --workspace` (FR-WS-02) — the CLI-surface pieces that must
//! not live in the core: the interactive per-candidate approval gate
//! (terminal I/O) and the best-effort background index warm (this binary
//! re-invoked as a detached child process). Candidate discovery, the
//! non-clobber per-member `init`, the incremental manifest write, and the
//! workspace MCP injection are all core business logic
//! ([`logos_core::federation::enable`]) — this module only resolves the
//! anchor, gates, wires the warm, and reports.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Result;
use logos_core::federation::{self, enable, Member};

use crate::{ask, Output};

/// `logos init --workspace [--yes] [--exclude <glob>]...` (FR-WS-02).
///
/// Anchors on an existing manifest via [`federation::discover`] when one is
/// found up-tree (the incremental re-run case — its already-resolved,
/// already-pruned `members` are kept without re-prompting); otherwise `root`
/// itself becomes the new workspace root. Newly discovered candidates go
/// through the approval gate (or `--yes`/`--exclude`), then
/// [`enable::enable`] runs the non-clobber per-member `init`, the manifest
/// upsert, and the workspace MCP injection.
///
/// Returns without blocking on indexing: each approved member's index is
/// warmed by a detached background process ([`spawn_background_warm`]) —
/// best-effort, never awaited. A member whose warm never starts (or hasn't
/// finished before first real use) still indexes correctly via the engine's
/// lazy `ensure_indexed` fallback (FR-IX-07), so this command never needs to
/// wait on it.
pub(crate) fn run(root: &Path, yes: bool, exclude: &[String], out: &Output) -> Result<i32> {
    let existing = federation::discover(root)?;
    let workspace_root = existing
        .as_ref()
        .map_or_else(|| root.to_path_buf(), |f| f.root.clone());
    let default_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();
    let (name, already) = match existing {
        Some(f) => (f.name, f.members),
        None => (default_name, Vec::new()),
    };
    let already_names: Vec<String> = already.iter().map(|m| m.name.clone()).collect();

    let proposed = enable::candidates_for_approval(&workspace_root, &already_names, exclude)?;
    let mut members = already;
    members.extend(gate(&proposed, yes, |m| {
        ask(
            &format!(
                "include member \"{}\" ({}) in the workspace?",
                m.name,
                m.root.display()
            ),
            true,
        )
    }));

    if members.is_empty() {
        eprintln!(
            "logos init --workspace: no candidate member repositories found under {} — nothing to do",
            workspace_root.display()
        );
        return Ok(0);
    }

    let report = enable::enable(&workspace_root, &name, &members)?;
    for member in &members {
        spawn_background_warm(&member.root);
    }

    out.print(&report)?;
    Ok(0)
}

/// Filter `candidates` down to the approved set: `--yes` accepts every one
/// without prompting (the scriptable path); otherwise each is offered to
/// `approve` (the real gate calls [`ask`] — a stderr y/n prompt, defaulting
/// to include, that degrades to `true` without printing anything on a
/// non-TTY stdin). Taking `approve` as a parameter keeps this filtering logic
/// testable without a real terminal.
fn gate(candidates: &[Member], yes: bool, mut approve: impl FnMut(&Member) -> bool) -> Vec<Member> {
    candidates
        .iter()
        .filter(|m| yes || approve(m))
        .cloned()
        .collect()
}

/// Best-effort background index warm for one member (FR-WS-02, hybrid
/// indexing): spawn this same binary as `logos --project <member> --quiet
/// index`, detached from this process — an independent child, not a thread,
/// so it keeps running after `init --workspace` returns (a thread would be
/// killed with the process; a spawned child is reparented, not terminated).
/// A spawn failure (the binary not resolvable, the OS out of processes, …)
/// just leaves that member on the lazy `ensure_indexed` fallback — never
/// fatal to the command.
fn spawn_background_warm(member_root: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .arg("--project")
        .arg(member_root)
        .arg("--quiet")
        .arg("index")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn member(name: &str) -> Member {
        Member {
            name: name.to_string(),
            root: PathBuf::from(name),
        }
    }

    #[test]
    fn yes_includes_every_candidate_without_prompting() {
        let candidates = vec![member("a"), member("b")];
        let mut asked = 0;
        let approved = gate(&candidates, true, |_| {
            asked += 1;
            false
        });
        assert_eq!(approved.len(), 2, "every candidate included under --yes");
        assert_eq!(asked, 0, "the predicate is never consulted under --yes");
    }

    #[test]
    fn gate_filters_by_the_approval_predicate() {
        let candidates = vec![member("a"), member("b")];
        let approved = gate(&candidates, false, |m| m.name == "a");
        let names: Vec<&str> = approved.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a"]);
    }

    #[test]
    fn gate_over_no_candidates_is_empty() {
        assert!(gate(&[], false, |_| true).is_empty());
    }
}
