//! Taking back what `init` wrote ([FR-IN-12]): the legacy entries the Claude
//! Code plugin replaces, and everything `init --uninstall` removes.
//!
//! Every remover here follows the writers' ownership rules in reverse, so
//! "remove what Logos wrote" can never reach the user's content:
//! - a JSON entry goes only while it still holds exactly the value `init`
//!   writes; a container goes only when this call emptied it, and a file only
//!   when nothing else is left in it;
//! - a managed block goes by its markers, and unbalanced markers leave the
//!   file alone (DL-07);
//! - a script or skill copy goes only when it carries the header Logos stamps
//!   on what it writes, and a pointer only when it points where Logos pointed it.
//!
//! Each remover returns an [`InitStep`], and a second run finds nothing and
//! reports `Unchanged`. `.logos/` is never touched: its data stays.
//!
//! [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use super::{managed_span, mcp_server_entry, ManagedSpan, read_optional, step, write_json, HOOKS_RELDIR, MD_BEGIN, MD_END};
use crate::models::pipeline::{InitAction, InitStep};

/// The project MCP host config, repo-relative.
const MCP_REL: &str = ".mcp.json";

/// Where `init -i` once materialized skills ([FR-WK-08]): the content
/// directory, and the `.claude/skills` pointer to it.
///
/// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
const SKILLS_REL: &str = ".agents/skills";
const SKILL_POINTERS_REL: &str = ".claude/skills";

/// The directories `init` creates for the files above, innermost first:
/// [`uninstall`] removes each one it left empty.
const TIDIED_DIRS: [&str; 5] = [".claude/hooks", SKILL_POINTERS_REL, ".claude", SKILLS_REL, ".agents"];

/// The one file a materialized skill directory holds.
const SKILL_FILE: &str = "SKILL.md";

/// Remove the `logos` server entry `init -i` wrote into `.mcp.json`.
///
/// Only the exact entry ([`mcp_server_entry`]) is Logos's: an entry someone
/// edited (other args, an `env`) is reported `Skipped` and kept, and `doctor`
/// goes on naming the duplicate until they remove it. A file left holding
/// nothing is deleted.
///
/// # Errors
/// Only on an I/O failure reading, writing or removing `.mcp.json`.
pub(crate) fn mcp_entry(root: &Path) -> Result<InitStep> {
    let path = root.join(MCP_REL);
    let none = || Ok(step(MCP_REL, InitAction::Unchanged, "no `logos` server entry"));
    let Some(text) = read_optional(&path)? else {
        return none();
    };
    let Ok(Value::Object(mut config)) = serde_json::from_str::<Value>(&text) else {
        return Ok(step(
            MCP_REL,
            InitAction::Skipped,
            "existing .mcp.json is not a JSON object — left untouched",
        ));
    };
    let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return none();
    };
    match servers.get("logos") {
        None => return none(),
        Some(entry) if *entry != mcp_server_entry() => {
            return Ok(step(
                MCP_REL,
                InitAction::Skipped,
                "the `logos` server entry is not the one `logos init` writes — left untouched; \
                 remove it by hand",
            ))
        }
        Some(_) => {}
    }
    servers.shift_remove("logos");
    if servers.is_empty() {
        config.shift_remove("mcpServers");
    }
    if config.is_empty() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        return Ok(step(
            MCP_REL,
            InitAction::Removed,
            "held only the `logos` server entry",
        ));
    }
    write_json(&path, &Value::Object(config))?;
    Ok(step(MCP_REL, InitAction::Updated, "`logos` server entry removed"))
}

/// Remove the session-start readout hook: its settings entry, the retired
/// SessionEnd one, and their scripts ([`crate::hooks::retire_quality_report_hook`]).
///
/// # Errors
/// Only on an I/O failure reading or writing `.claude/settings.json`.
pub(crate) fn readout_hook(root: &Path) -> Result<InitStep> {
    let summary = crate::hooks::retire_quality_report_hook(root)?;
    let target = summary.script.as_str();
    if let Some(notice) = summary.notice {
        return Ok(step(target, InitAction::Skipped, notice));
    }
    if !summary.removed.is_empty() {
        // The script's directory goes too once this emptied it; anything the
        // user keeps there keeps it (`remove_dir` refuses a non-empty one).
        if let Some(dir) = Path::new(target).parent() {
            let _ = fs::remove_dir(root.join(dir));
        }
    }
    let left = if summary.skipped.is_empty() {
        String::new()
    } else {
        format!("; left {}", summary.skipped.join(", "))
    };
    Ok(match (summary.removed.is_empty(), summary.skipped.is_empty()) {
        (true, true) => step(target, InitAction::Unchanged, "no session-start readout hook"),
        (true, false) => step(target, InitAction::Skipped, left.trim_start_matches("; ")),
        (false, _) => step(
            target,
            InitAction::Removed,
            format!("removed {}{left}", summary.removed.join(", ")),
        ),
    })
}

/// Remove the managed block from `CLAUDE.md`. The blank line `init` put
/// between the user's content and an appended block goes with it, and a file
/// left with nothing but whitespace is deleted.
///
/// # Errors
/// Only on an I/O failure reading, writing or removing the file.
pub(crate) fn claude_md_block(root: &Path) -> Result<InitStep> {
    const TARGET: &str = "CLAUDE.md";
    let path = root.join(TARGET);
    let none = || Ok(step(TARGET, InitAction::Unchanged, "no managed block"));
    let Some(existing) = read_optional(&path)? else {
        return none();
    };
    let (begin_at, span_end) = match managed_span(&existing, MD_BEGIN, MD_END) {
        ManagedSpan::Absent => return none(),
        ManagedSpan::Malformed => {
            return Ok(step(TARGET, InitAction::Skipped, "managed-block markers are unbalanced — left untouched"))
        }
        ManagedSpan::At { begin_at, span_end } => (begin_at, span_end),
    };
    let after = &existing[span_end..];
    let mut before = &existing[..begin_at];
    if after.is_empty() && before.ends_with("\n\n") {
        before = &before[..before.len() - 1];
    }
    let rest = format!("{before}{after}");
    if rest.trim().is_empty() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        return Ok(step(TARGET, InitAction::Removed, "held only the managed block"));
    }
    fs::write(&path, rest).with_context(|| format!("writing {}", path.display()))?;
    Ok(step(TARGET, InitAction::Updated, "managed block removed"))
}

/// The skills the core itself emitted before the split ([FR-WK-08]): a closed
/// list. `init` retires a copy only under one of these names, never under any
/// other, whatever its header says — an extension stamps the same header on a
/// skill it owns, and that skill is not the core's to take back (HF-10).
///
/// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
const LEGACY_CORE_SKILLS: [&str; 1] = ["logos-wiki"];

/// Is `name` one of the skills the core itself once emitted ([`LEGACY_CORE_SKILLS`])?
pub(super) fn legacy_core_skill(name: &str) -> bool {
    LEGACY_CORE_SKILLS.contains(&name)
}

/// The header line Logos stamped on a skill it materialized ([FR-WK-08]):
/// `<!-- <name> skill — emitted from Logos v<version>…`. Matched against the
/// directory's own name; which names count is [`emitted_skills`]'s `removable`.
///
/// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
fn emission_header(name: &str) -> String {
    format!("<!-- {name} skill — emitted from Logos v")
}

/// Is `dir` a skill copy Logos materialized: a real directory holding exactly
/// one `SKILL.md`, which carries the emission header on a line of its own?
/// `None` when it is not a Logos copy at all; `Some(false)` when it is one the
/// user has since added files to.
fn emitted_copy(dir: &Path, name: &str) -> Option<bool> {
    let meta = fs::symlink_metadata(dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let body = fs::read_to_string(dir.join(SKILL_FILE)).ok()?;
    let header = emission_header(name);
    if !body.lines().any(|line| line.starts_with(&header)) {
        return None;
    }
    let only_skill = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .all(|entry| entry.file_name() == SKILL_FILE);
    Some(only_skill)
}

/// The names present under `dir`, sorted; empty when it is absent.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Remove each skill copy an earlier `init -i` materialized whose name
/// `removable` accepts, with its `.claude/skills` pointer: the symlink to `../../.agents/skills/<name>` it
/// created, or the copy it fell back to ([FR-WK-08]). One row per skill
/// found; none when there is nothing of Logos's there.
///
/// A copy the user has added files to is `Skipped` and kept, and so is a
/// pointer that points anywhere else. A skill whose name `removable` rejects
/// is not looked at: `init` passes [`legacy_core_skill`] (minus the names an
/// extension owns), `--uninstall` passes everything.
///
/// # Errors
/// Only on an I/O failure removing a Logos-written file.
///
/// [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
pub(crate) fn emitted_skills(root: &Path, removable: impl Fn(&str) -> bool) -> Result<Vec<InitStep>> {
    let skills = root.join(SKILLS_REL);
    let pointers = root.join(SKILL_POINTERS_REL);
    let mut names = names_in(&skills);
    names.extend(names_in(&pointers));
    names.sort();
    names.dedup();
    names.retain(|name| removable(name));
    let mut steps = Vec::new();
    for name in names {
        let content = skills.join(&name);
        let pointer = pointers.join(&name);
        let content_rel = format!("{SKILLS_REL}/{name}");
        let pointer_rel = format!("{SKILL_POINTERS_REL}/{name}");
        let content_state = emitted_copy(&content, &name);
        if content_state == Some(false) {
            steps.push(step(
                &content_rel,
                InitAction::Skipped,
                "a skill copy Logos wrote, but it now holds files Logos did not write — left untouched",
            ));
            continue;
        }
        let link_target = Path::new("../..").join(SKILLS_REL).join(&name);
        let our_link = fs::read_link(&pointer).is_ok_and(|target| target == link_target)
            && (content_state.is_some() || fs::symlink_metadata(&content).is_err());
        // A pointer reached through a symlinked `.claude/skills` directory is
        // the content directory itself, not a second copy: removing it twice
        // would fail half-way through.
        let same_dir = fs::read_link(&pointer).is_err()
            && fs::canonicalize(&pointer).is_ok_and(|p| fs::canonicalize(&content).is_ok_and(|c| c == p));
        let our_copy = !same_dir && emitted_copy(&pointer, &name) == Some(true);
        let mut removed: Vec<String> = Vec::new();
        if content_state == Some(true) {
            remove_skill_copy(&content)?;
            removed.push(content_rel.clone());
        }
        if our_link {
            fs::remove_file(&pointer).with_context(|| format!("removing {}", pointer.display()))?;
            removed.push(pointer_rel.clone());
        } else if our_copy {
            remove_skill_copy(&pointer)?;
            removed.push(pointer_rel.clone());
        }
        if !removed.is_empty() {
            let target = if content_state.is_some() { content_rel } else { pointer_rel };
            steps.push(step(
                &target,
                InitAction::Removed,
                format!("legacy skill copy removed ({})", removed.join(", ")),
            ));
        }
    }
    if steps.iter().any(|s| s.action == InitAction::Removed) {
        // The directories the copies lived in go once this emptied them.
        for dir in [SKILL_POINTERS_REL, SKILLS_REL, ".agents"] {
            let _ = fs::remove_dir(root.join(dir));
        }
    }
    Ok(steps)
}

/// Delete a skill copy [`emitted_copy`] confirmed holds only `SKILL.md`.
fn remove_skill_copy(dir: &Path) -> Result<()> {
    let file = dir.join(SKILL_FILE);
    fs::remove_file(&file).with_context(|| format!("removing {}", file.display()))?;
    fs::remove_dir(dir).with_context(|| format!("removing {}", dir.display()))
}

/// Remove the managed git hooks through [`crate::hooks::uninstall`], which
/// also clears the copies seeded into linked worktrees and unsets
/// `core.hooksPath` only if it still points at Logos's directory. Outside a
/// git work tree there is nothing to remove.
///
/// # Errors
/// When git fails part-way — the hooks are Logos's own, so a half-done
/// removal (scripts deleted, `core.hooksPath` still set) fails loud (DL-06).
fn git_hooks(root: &Path) -> Result<InitStep> {
    if !crate::hooks::is_git_worktree(root) {
        return Ok(step(HOOKS_RELDIR, InitAction::Unchanged, "not a git repository — no git hooks"));
    }
    let result = crate::hooks::uninstall(root)?;
    let mut done: Vec<String> = Vec::new();
    if !result.installed.is_empty() {
        done.push(format!("removed {}", result.installed.join(", ")));
    }
    if result.unset_hooks_path {
        done.push("core.hooksPath unset".to_string());
    }
    if !result.worktrees.is_empty() {
        done.push(format!("cleared {} linked worktree(s)", result.worktrees.len()));
    }
    let action = if done.is_empty() {
        done.push("no managed git hooks".to_string());
        InitAction::Unchanged
    } else {
        InitAction::Removed
    };
    done.extend(result.warnings);
    Ok(step(HOOKS_RELDIR, action, done.join("; ")))
}

/// `logos init --uninstall` ([FR-IN-12]): remove every artefact Logos writes
/// into a project — the `.mcp.json` entry, the `CLAUDE.md` block, the readout
/// hook and its script, the plugin enrolment, materialized skill copies and
/// their pointers, and the managed git hooks — leaving foreign content and
/// `.logos/` untouched. The directories those lived in go too, when this run
/// emptied them. A second run reports every row `Unchanged` and writes nothing.
///
/// # Errors
/// On an I/O failure on a Logos-written file, and when git fails part-way
/// through removing the managed git hooks.
///
/// [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
pub(crate) fn uninstall(root: &Path) -> Result<Vec<InitStep>> {
    let mut steps = vec![
        mcp_entry(root)?,
        claude_md_block(root)?,
        readout_hook(root)?,
        super::plugin::unenrol(root)?,
    ];
    steps.extend(emitted_skills(root, |_| true)?);
    steps.push(git_hooks(root)?);
    if steps.iter().any(|s| s.action == InitAction::Removed || s.action == InitAction::Updated) {
        // Innermost first; `remove_dir` only removes an empty directory, so
        // anything the user keeps there keeps its directory too.
        for dir in TIDIED_DIRS {
            let _ = fs::remove_dir(root.join(dir));
        }
    }
    Ok(steps)
}


#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    /// The block `init` appended comes out with its blank separator, leaving
    /// the user's file byte for byte; a file holding only the block goes.
    #[test]
    fn the_claude_md_block_comes_out_leaving_the_users_bytes() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for original in ["user notes\n", "# Title\n\nmore\n"] {
            write(root, "CLAUDE.md", original);
            super::super::upsert_block_file(root, "CLAUDE.md", "", MD_BEGIN, MD_END, super::super::CLAUDE_MD_BLOCK).unwrap();
            assert_ne!(read(root, "CLAUDE.md"), original);
            assert_eq!(claude_md_block(root).unwrap().action, InitAction::Updated);
            assert_eq!(read(root, "CLAUDE.md"), original, "back to the user's bytes");
            assert_eq!(claude_md_block(root).unwrap().action, InitAction::Unchanged, "second run");
        }
        // A block in the middle keeps both sides verbatim.
        write(root, "CLAUDE.md", &format!("top\n{MD_BEGIN}\nx\n{MD_END}\nbottom\n"));
        claude_md_block(root).unwrap();
        assert_eq!(read(root, "CLAUDE.md"), "top\nbottom\n");

        fs::remove_file(root.join("CLAUDE.md")).unwrap();
        super::super::upsert_block_file(root, "CLAUDE.md", "", MD_BEGIN, MD_END, super::super::CLAUDE_MD_BLOCK).unwrap();
        assert_eq!(claude_md_block(root).unwrap().action, InitAction::Removed);
        assert!(!root.join("CLAUDE.md").exists());
        assert_eq!(claude_md_block(root).unwrap().action, InitAction::Unchanged, "absent file");
    }

    #[test]
    fn unbalanced_claude_md_markers_are_left_alone() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for mangled in [format!("{MD_BEGIN}\nno end\n"), format!("no begin\n{MD_END}\n")] {
            write(root, "CLAUDE.md", &mangled);
            assert_eq!(claude_md_block(root).unwrap().action, InitAction::Skipped);
            assert_eq!(read(root, "CLAUDE.md"), mangled);
        }
    }

    /// Only the exact entry `init` writes is Logos's; the file goes when it
    /// held nothing else, and other servers keep their place.
    #[test]
    fn the_mcp_entry_goes_only_when_it_is_exactly_logos_s() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        assert_eq!(mcp_entry(root).unwrap().action, InitAction::Unchanged, "no file");

        super::super::inject_mcp(root).unwrap();
        assert_eq!(mcp_entry(root).unwrap().action, InitAction::Removed);
        assert!(!root.join(MCP_REL).exists(), "a file holding only our entry is deleted");

        write(root, MCP_REL, r#"{"mcpServers":{"a":{"command":"a"},"logos":{"command":"logos","args":["serve","--mcp"]},"m":{"command":"m"},"z":{"command":"z"}}}"#);
        assert_eq!(mcp_entry(root).unwrap().action, InitAction::Updated);
        let left: Value = serde_json::from_str(&read(root, MCP_REL)).unwrap();
        let names: Vec<&String> = left["mcpServers"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["a", "m", "z"], "others keep their order");

        let edited = r#"{"mcpServers":{"logos":{"command":"logos","args":["serve","--mcp"],"env":{"X":"1"}}}}"#;
        write(root, MCP_REL, edited);
        assert_eq!(mcp_entry(root).unwrap().action, InitAction::Skipped, "an edited entry is the user's");
        assert_eq!(read(root, MCP_REL), edited);

        write(root, MCP_REL, "[1]");
        assert_eq!(mcp_entry(root).unwrap().action, InitAction::Skipped);
    }

    /// A readout hook left in place is a `Skipped` row naming why, never an
    /// `Unchanged` one claiming there was nothing there.
    #[test]
    fn a_readout_hook_left_in_place_is_reported_skipped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        assert_eq!(readout_hook(root).unwrap().action, InitAction::Unchanged, "nothing there");

        write(root, crate::hooks::QUALITY_REPORT_HOOK_SCRIPT_REL, "#!/bin/sh\n# mine\n");
        let kept = readout_hook(root).unwrap();
        assert_eq!(kept.action, InitAction::Skipped, "{kept:?}");
        assert!(kept.detail.contains("not a Logos-authored script"), "{kept:?}");

        write(root, ".claude/settings.json", "{ not json");
        let foreign = readout_hook(root).unwrap();
        assert_eq!(foreign.action, InitAction::Skipped, "{foreign:?}");
        assert!(foreign.detail.contains("left untouched"), "{foreign:?}");
    }

    fn emitted(name: &str) -> String {
        format!("---\nname: {name}\n---\n\n<!-- {name} skill — emitted from Logos v1.2.3. After a binary\n     upgrade, refresh. -->\n\n# body\n")
    }

    #[cfg(unix)]
    #[test]
    fn an_emitted_skill_copy_and_its_symlink_are_removed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, ".agents/skills/gen/SKILL.md", &emitted("gen"));
        fs::create_dir_all(root.join(SKILL_POINTERS_REL)).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/gen", root.join(".claude/skills/gen")).unwrap();

        let steps = emitted_skills(root, |_| true).unwrap();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!((steps[0].target.as_str(), steps[0].action), (".agents/skills/gen", InitAction::Removed));
        assert!(fs::symlink_metadata(root.join(".agents/skills/gen")).is_err());
        assert!(fs::symlink_metadata(root.join(".claude/skills/gen")).is_err());
        assert!(emitted_skills(root, |_| true).unwrap().is_empty(), "second run finds nothing");
    }

    /// The copy fallback, and a dangling pointer left by a removed copy.
    #[cfg(unix)]
    #[test]
    fn a_copy_pointer_and_a_dangling_pointer_are_removed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, ".claude/skills/gen/SKILL.md", &emitted("gen"));
        std::os::unix::fs::symlink("../../.agents/skills/old", root.join(".claude/skills/old")).unwrap();
        let steps = emitted_skills(root, |_| true).unwrap();
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert!(steps.iter().all(|s| s.action == InitAction::Removed));
        assert!(names_in(&root.join(SKILL_POINTERS_REL)).is_empty());
    }

    /// `.claude/skills` itself a symlink to `.agents/skills`: the pointer is
    /// the content directory, so the copy goes once and the run completes.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_pointer_directory_is_not_removed_twice() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, ".agents/skills/gen/SKILL.md", &emitted("gen"));
        fs::create_dir_all(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink("../.agents/skills", root.join(SKILL_POINTERS_REL)).unwrap();

        let steps = emitted_skills(root, |_| true).unwrap();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!((steps[0].target.as_str(), steps[0].action), (".agents/skills/gen", InitAction::Removed));
        assert!(!root.join(".agents/skills/gen").exists());
        assert!(emitted_skills(root, |_| true).unwrap().is_empty(), "second run finds nothing");
    }

    /// Near misses, each left in place: a skill without the header, one whose
    /// header names another skill, an emitted copy the user added a file to,
    /// and a pointer aimed elsewhere.
    #[cfg(unix)]
    #[test]
    fn skills_logos_did_not_write_are_left() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, ".agents/skills/mine/SKILL.md", "---\nname: mine\n---\n# mine\n");
        write(root, ".agents/skills/other/SKILL.md", &emitted("gen"));
        write(root, ".agents/skills/gen/SKILL.md", &emitted("gen"));
        write(root, ".agents/skills/gen/notes.md", "mine\n");
        fs::create_dir_all(root.join(SKILL_POINTERS_REL)).unwrap();
        std::os::unix::fs::symlink("../../elsewhere/x", root.join(".claude/skills/x")).unwrap();
        // A pointer aimed at a foreign directory that looks emitted: a symlink
        // is not a copy, and what it points at is not Logos's to delete.
        write(root, "vendor/v/SKILL.md", &emitted("v"));
        std::os::unix::fs::symlink("../../vendor/v", root.join(".claude/skills/v")).unwrap();
        std::os::unix::fs::symlink("../../.agents/skills/mine", root.join(".claude/skills/mine")).unwrap();

        let steps = emitted_skills(root, |_| true).unwrap();
        assert_eq!(steps.len(), 1, "only the edited copy reports, as Skipped: {steps:?}");
        assert_eq!((steps[0].target.as_str(), steps[0].action), (".agents/skills/gen", InitAction::Skipped));
        for kept in ["mine/SKILL.md", "other/SKILL.md", "gen/notes.md"] {
            assert!(root.join(".agents/skills").join(kept).exists(), "{kept} kept");
        }
        assert!(fs::symlink_metadata(root.join(".claude/skills/x")).is_ok());
        assert!(root.join("vendor/v/SKILL.md").is_file(), "a symlinked foreign skill is not removed");
        assert!(fs::symlink_metadata(root.join(".claude/skills/mine")).is_ok(), "a pointer at a user skill");
    }

    /// The closed list names the one skill the core emitted, and nothing a
    /// character away from it.
    #[test]
    fn the_legacy_core_names_are_closed() {
        assert!(legacy_core_skill("logos-wiki"));
        for near in ["", "logos-wik", "logos-wiki2", "xlogos-wiki", "Logos-wiki", "logos", "wiki", "gen"] {
            assert!(!legacy_core_skill(near), "{near:?} is not a legacy core skill");
        }
    }

    /// `init`'s retirement: a Logos-stamped skill under a legacy core name goes;
    /// one under any other name stays, edits and pointer intact (HF-10).
    #[cfg(unix)]
    #[test]
    fn init_retires_only_a_legacy_core_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let edited = format!("{}\nmy own notes\n", emitted("gen"));
        write(root, ".agents/skills/logos-wiki/SKILL.md", &emitted("logos-wiki"));
        write(root, ".agents/skills/gen/SKILL.md", &edited);
        fs::create_dir_all(root.join(SKILL_POINTERS_REL)).unwrap();
        for name in ["logos-wiki", "gen"] {
            std::os::unix::fs::symlink(format!("../../.agents/skills/{name}"), root.join(SKILL_POINTERS_REL).join(name)).unwrap();
        }

        let steps = emitted_skills(root, legacy_core_skill).unwrap();
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!((steps[0].target.as_str(), steps[0].action), (".agents/skills/logos-wiki", InitAction::Removed));
        assert!(fs::symlink_metadata(root.join(".agents/skills/logos-wiki")).is_err());
        assert!(fs::symlink_metadata(root.join(".claude/skills/logos-wiki")).is_err());
        assert_eq!(read(root, ".agents/skills/gen/SKILL.md"), edited, "an unlisted name keeps its edits");
        assert!(fs::symlink_metadata(root.join(".claude/skills/gen")).is_ok(), "and its pointer");
    }
}
