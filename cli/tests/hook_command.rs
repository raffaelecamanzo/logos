//! `logos hook --emit [--force]` — the session-start quality-report hook as a
//! core command, outside the wiki (S-638, FR-IN-07, ADR-49).
//!
//! The hook code moved from `logos-core/src/wiki/` to `logos-core/src/hooks/`
//! without changing a byte of what it writes. Its hidden alias `logos wiki hook
//! --emit` left with the wiki: `logos hook --emit` is the only spelling.
//!
//! Fixtures are `TempDir`s under the OS temp directory — outside this checkout,
//! so the root never climbs to the enclosing repository.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const SCRIPT_REL: &str = ".claude/hooks/logos-quality-open.sh";
const SETTINGS_REL: &str = ".claude/settings.json";

fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("no signal termination")
}

/// The bytes of a file under `root`, or `None` when it is absent.
fn bytes(root: &Path, rel: &str) -> Option<Vec<u8>> {
    fs::read(root.join(rel)).ok()
}

/// `logos init -i` installs the very bytes `logos hook --emit` writes: one
/// hook, one source.
#[test]
fn init_i_installs_the_same_hook_as_hook_emit() {
    let by_init = TempDir::new().unwrap();
    let by_hook = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(by_init.path(), &["init", "-i"])), 0);
    assert_eq!(exit_code(&logos(by_hook.path(), &["hook", "--emit"])), 0);
    for rel in [SCRIPT_REL, SETTINGS_REL] {
        let init = bytes(by_init.path(), rel).unwrap_or_else(|| panic!("init -i wrote {rel}"));
        assert_eq!(Some(init), bytes(by_hook.path(), rel), "{rel}");
    }
}

/// The hidden `logos wiki hook` alias is gone with the wiki: `logos hook --emit`
/// is the only spelling, and the old one is a usage error that writes nothing.
#[test]
fn the_wiki_hook_alias_is_gone() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["wiki", "hook", "--emit"]);
    assert_eq!(
        exit_code(&out),
        2,
        "`logos wiki hook --emit` is not a command: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!tmp.path().join(SCRIPT_REL).exists(), "nothing was written");
}

/// Every instruction the hook writes or prints names the one spelling that exists,
/// `logos hook --emit`: the emitted script's regenerate line, and the notice for a
/// settings file it leaves alone. A user following either must not land on a
/// command this build does not have.
#[test]
fn the_hook_names_only_its_own_spelling() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["hook", "--emit"])), 0);
    let script = String::from_utf8(bytes(tmp.path(), SCRIPT_REL).expect("the script is written")).unwrap();
    assert!(script.contains("Regenerate with `logos hook --emit --force`"), "{script}");
    assert!(!script.contains("wiki"), "the emitted script names no wiki command:\n{script}");

    let foreign = TempDir::new().unwrap();
    fs::create_dir_all(foreign.path().join(".claude")).unwrap();
    fs::write(foreign.path().join(SETTINGS_REL), "{ not json").unwrap();
    let out = logos(foreign.path(), &["--json", "hook", "--emit"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("run `logos hook --emit` after fixing it"), "the notice names the command: {stdout}");
    assert!(!stdout.contains("wiki"), "the notice names no wiki command: {stdout}");
}

/// `hook` without `--emit` is a clap usage error (exit 2).
#[test]
fn hook_requires_emit() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["hook"])), 2);
    assert!(!tmp.path().join(SCRIPT_REL).exists(), "nothing was written");
}

/// Whether a line of Rust code names the wiki module through a path: the word
/// `wiki` as a whole identifier on a line that is a `use` or spells a `crate::`
/// or `super::` path — `crate::wiki::x`, `use crate::{wiki as w}`, `super::wiki`.
/// Comments and string literals that merely say "wiki" are not paths.
fn names_the_wiki_module(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") {
        return false;
    }
    let is_path = code.starts_with("use ") || code.contains("crate::") || code.contains("super::");
    is_path
        && code
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|token| token == "wiki")
}

/// The matcher is probed with the shapes that evaded the first version (an
/// aliased import, a braced import) and with the near misses it must admit.
#[test]
fn the_wiki_reference_matcher_catches_aliases_and_admits_near_misses() {
    for hit in [
        "use crate::wiki::SKILL_DIR_REL;",
        "use crate::{wiki as w};",
        "use crate::{hooks, wiki};",
        "    let x = crate::wiki::latest_version();",
        "use super::wiki::EmitAction;",
    ] {
        assert!(names_the_wiki_module(hit), "must catch: {hit}");
    }
    for miss in [
        "//! Like the wiki's embedded skill emit this is local I/O.",
        "use crate::wikipedia::Page;",
        "use crate::hooks::EmitAction;",
        "tracing::info!(\"wiki hook materialized\");",
        "# Regenerate with `logos wiki hook --emit --force`",
        "        run `logos wiki hook --emit` after fixing it\"",
        "# Regenerate with `logos hook --emit --force` (or re-run `logos init -i`).",
    ] {
        assert!(!names_the_wiki_module(miss), "must admit: {miss}");
    }
}

/// No non-test source file of the hook code under `logos-core/src/hooks/` names
/// a `wiki` module — the hook survives the wiki leaving the public build.
#[test]
fn the_hook_code_does_not_reference_the_wiki_modules() {
    let hooks = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/src/hooks");
    let mut scanned = Vec::new();
    for entry in fs::read_dir(&hooks).expect("the hooks module directory is readable") {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name == "tests.rs" {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            assert!(
                !names_the_wiki_module(line),
                "{name}:{} references the wiki module: {line}",
                n + 1
            );
        }
        scanned.push(name);
    }
    scanned.sort();
    assert_eq!(
        scanned,
        ["mod.rs", "quality_report.rs"],
        "the scan saw the moved hook and the module root"
    );
    assert!(
        !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../logos-core/src/wiki/hook.rs")
            .exists(),
        "the hook source no longer lives under the wiki module"
    );
}

/// `logos hook --emit` reports under the one telemetry name the install has
/// always had, `wiki_quality_report_hook_emit`, so recorded history stays
/// attributed. The classification census pins the variant; this pins which call
/// site emits it.
#[test]
fn hook_emit_reports_under_the_one_telemetry_name() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["init"])), 0, "a `.logos/` to record into");
    assert_eq!(exit_code(&logos(tmp.path(), &["hook", "--emit"])), 0);
    assert_eq!(exit_code(&logos(tmp.path(), &["hook", "--emit", "--force"])), 0);

    let stats = logos(tmp.path(), &["--json", "stats"]);
    assert_eq!(exit_code(&stats), 0);
    let stats: serde_json::Value = serde_json::from_slice(&stats.stdout).expect("stats JSON");
    let rows: Vec<&serde_json::Value> = stats["calls_by_tool"]
        .as_array()
        .expect("the per-tool usage")
        .iter()
        .filter(|row| row["tool"] == "wiki_quality_report_hook_emit")
        .collect();
    assert_eq!(rows.len(), 1, "one row for the command: {rows:?}");
    assert_eq!(rows[0]["surface"], "cli");
    assert_eq!(rows[0]["calls"], 2, "both runs, the plain emit and the forced one");
}
