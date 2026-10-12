//! `logos hook --emit [--force]` — the session-start quality-report hook as a
//! core command, outside the wiki (S-638, FR-IN-07, ADR-49) — and its hidden
//! alias `logos wiki hook --emit [--force]`.
//!
//! The hook code moved from `logos-core/src/wiki/` to `logos-core/src/hooks/`
//! without changing a byte of what it writes, so the acceptance test is
//! byte-identity: both spellings are driven against twin projects, step by
//! step, and every observable (exit code, stdout, stderr, the hook script, the
//! merged `.claude/settings.json`) must be equal after each step.
//!
//! Fixtures are `TempDir`s under the OS temp directory — outside this checkout,
//! so the root never climbs to the enclosing repository.
//!
//! The alias is the `wiki` extension's since S-644, so the twin comparisons run
//! only in a build that composes it; without it the core `logos hook` tests
//! remain, and the twin helpers go unused.

#![cfg_attr(not(feature = "wiki"), allow(dead_code))]

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

/// Every file under `root` as `(relative path, bytes, mode)`, sorted.
fn tree(root: &Path) -> Vec<(String, Vec<u8>, u32)> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>, u32)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                let mode = fs::metadata(&path).unwrap().permissions().mode();
                out.push((rel, fs::read(&path).unwrap(), mode));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// One twin pair: the same starting state, one driven by `logos hook`, the
/// other by `logos wiki hook`.
struct Twins {
    core: TempDir,
    alias: TempDir,
}

impl Twins {
    fn new(settings: Option<&str>) -> Self {
        let twins = Twins {
            core: TempDir::new().unwrap(),
            alias: TempDir::new().unwrap(),
        };
        if let Some(settings) = settings {
            for root in [twins.core.path(), twins.alias.path()] {
                fs::create_dir_all(root.join(".claude")).unwrap();
                fs::write(root.join(SETTINGS_REL), settings).unwrap();
            }
        }
        twins
    }

    /// Run `tail` (e.g. `["--emit", "--force"]`) through both spellings, with
    /// `global` flags (e.g. `--json`) ahead of the verb, and assert every
    /// observable is byte-identical. Returns the core spelling's stdout.
    fn step(&self, global: &[&str], tail: &[&str]) -> String {
        let mut core_args: Vec<&str> = global.to_vec();
        core_args.push("hook");
        core_args.extend_from_slice(tail);
        let mut alias_args: Vec<&str> = global.to_vec();
        alias_args.extend_from_slice(&["wiki", "hook"]);
        alias_args.extend_from_slice(tail);

        let core = logos(self.core.path(), &core_args);
        let alias = logos(self.alias.path(), &alias_args);
        let label = format!("{global:?} hook {tail:?}");

        assert_eq!(exit_code(&core), exit_code(&alias), "exit code, {label}");
        assert_eq!(core.stdout, alias.stdout, "stdout, {label}");
        assert_eq!(core.stderr, alias.stderr, "stderr, {label}");
        // The whole observable tree — every path, its bytes and its mode — not
        // just the two files the hook is known to write: a side effect only one
        // spelling has (a stray file, a different script mode) must fail too.
        assert_eq!(
            tree(self.core.path()),
            tree(self.alias.path()),
            "the project tree, {label}"
        );
        String::from_utf8_lossy(&core.stdout).into_owned()
    }
}

/// A fresh project, then an already-installed one, with and without `--force`:
/// the two spellings never diverge, and the script is really written.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn hook_and_wiki_hook_write_byte_identical_artifacts_fresh_and_installed() {
    let twins = Twins::new(None);

    let fresh = twins.step(&["--json"], &["--emit"]);
    assert!(fresh.contains("\"action\":\"created\""), "{fresh}");
    let script = bytes(twins.core.path(), SCRIPT_REL).expect("the script is written");
    assert!(
        String::from_utf8_lossy(&script).contains("logos:quality-report:managed"),
        "the managed marker is in the emitted script"
    );

    let installed = twins.step(&["--json"], &["--emit"]);
    assert!(installed.contains("\"action\":\"skipped\""), "{installed}");

    let forced = twins.step(&["--json"], &["--emit", "--force"]);
    assert!(forced.contains("\"action\":\"forced\""), "{forced}");
}

/// `--force` over a drifted managed entry: both spellings overwrite it and say
/// `forced` (an unforced run would say `reconciled`), leaving identical bytes.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn hook_and_wiki_hook_agree_on_a_forced_emit_over_a_drifted_entry() {
    let twins = Twins::new(None);
    twins.step(&["--json"], &["--emit"]);
    for root in [twins.core.path(), twins.alias.path()] {
        let path = root.join(SETTINGS_REL);
        let mut settings: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        settings["hooks"]["SessionStart"][0]["hooks"][0]["timeout"] = serde_json::json!(1);
        fs::write(&path, serde_json::to_string_pretty(&settings).unwrap()).unwrap();
    }
    let out = twins.step(&["--json"], &["--emit", "--force"]);
    assert!(out.contains("\"action\":\"forced\""), "{out}");
}

/// The human rendering (no `--json`) is identical too — it is what a person
/// running either spelling sees.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn hook_and_wiki_hook_render_the_same_human_output() {
    let twins = Twins::new(None);
    let human = twins.step(&[], &["--emit"]);
    assert!(!human.is_empty(), "the human rendering names the outcome");
    twins.step(&[], &["--emit", "--force"]);
}

/// The settings merge is non-clobbering and identical over an existing,
/// foreign `.claude/settings.json` — a foreign key survives in both.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn hook_and_wiki_hook_merge_identically_into_an_existing_settings_file() {
    let twins = Twins::new(Some(
        "{\n  \"permissions\": { \"allow\": [\"Bash(ls:*)\"] }\n}\n",
    ));
    twins.step(&["--json"], &["--emit"]);
    let merged: serde_json::Value =
        serde_json::from_slice(&bytes(twins.core.path(), SETTINGS_REL).unwrap()).unwrap();
    assert_eq!(
        merged["permissions"]["allow"][0], "Bash(ls:*)",
        "the foreign key survives the merge"
    );
    assert_eq!(merged["hooks"]["SessionStart"].as_array().map(Vec::len), Some(1));
    twins.step(&["--json"], &["--emit", "--force"]);
}

/// A foreign, unparseable settings file is never overwritten — by either
/// spelling — and both report the same notice.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn hook_and_wiki_hook_both_leave_an_unparseable_settings_file_alone() {
    let twins = Twins::new(Some("{ not json"));
    let out = twins.step(&["--json"], &["--emit"]);
    assert!(out.contains("\"action\":\"skipped\""), "{out}");
    assert_eq!(
        bytes(twins.core.path(), SETTINGS_REL).unwrap(),
        b"{ not json",
        "the foreign file is untouched"
    );
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

/// `hook` is a public command; `wiki hook` is hidden from `logos wiki --help`
/// yet still runs, and still documents itself under `--help`.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn wiki_hook_is_hidden_from_wiki_help_but_still_runs() {
    let tmp = TempDir::new().unwrap();

    let top = logos(tmp.path(), &["--help"]);
    assert_eq!(exit_code(&top), 0);
    let top = String::from_utf8_lossy(&top.stdout);
    assert!(
        top.lines().any(|l| l.starts_with("  hook ")),
        "`logos --help` lists the core `hook` command:\n{top}"
    );

    let wiki = logos(tmp.path(), &["wiki", "--help"]);
    assert_eq!(exit_code(&wiki), 0);
    let wiki = String::from_utf8_lossy(&wiki.stdout);
    assert!(
        !wiki.lines().any(|l| l.starts_with("  hook ")),
        "`logos wiki --help` no longer lists `hook`:\n{wiki}"
    );

    let direct = logos(tmp.path(), &["wiki", "hook", "--help"]);
    assert_eq!(exit_code(&direct), 0, "the hidden alias still parses");
    assert!(
        String::from_utf8_lossy(&direct.stdout).contains("--emit"),
        "the hidden alias still documents itself under --help"
    );
    let ran = logos(tmp.path(), &["wiki", "hook", "--emit"]);
    assert_eq!(exit_code(&ran), 0, "the hidden alias still runs");
    assert!(tmp.path().join(SCRIPT_REL).exists());
}

/// `hook` without `--emit` is a clap usage error (exit 2), like its alias.
#[test]
fn hook_requires_emit() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["hook"])), 2);
    assert_eq!(exit_code(&logos(tmp.path(), &["wiki", "hook"])), 2);
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

/// Both spellings report under the one telemetry name the install has always
/// had, `wiki_quality_report_hook_emit`: recorded history stays attributed and
/// the alias is not a second tool. The classification census pins the variant;
/// this pins which call site emits it.
#[cfg(feature = "wiki")] // `wiki hook` is the `wiki` extension's alias (S-644)
#[test]
fn both_spellings_report_under_the_one_telemetry_name() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["init"])), 0, "a `.logos/` to record into");
    assert_eq!(exit_code(&logos(tmp.path(), &["hook", "--emit"])), 0);
    assert_eq!(exit_code(&logos(tmp.path(), &["wiki", "hook", "--emit"])), 0);

    let stats = logos(tmp.path(), &["--json", "stats"]);
    assert_eq!(exit_code(&stats), 0);
    let stats: serde_json::Value = serde_json::from_slice(&stats.stdout).expect("stats JSON");
    let rows: Vec<&serde_json::Value> = stats["calls_by_tool"]
        .as_array()
        .expect("the per-tool usage")
        .iter()
        .filter(|row| row["tool"] == "wiki_quality_report_hook_emit")
        .collect();
    assert_eq!(rows.len(), 1, "one row for both spellings: {rows:?}");
    assert_eq!(rows[0]["surface"], "cli");
    assert_eq!(rows[0]["calls"], 2, "the core spelling and the alias");
}
