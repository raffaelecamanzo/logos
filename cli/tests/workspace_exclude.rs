//! End-to-end tests for `[workspace] exclude` (S-707, [CR-223], [FR-WS-40]),
//! exercised through the real binary:
//!
//! - `init --workspace --exclude <glob>` writes the glob into the manifest, and
//!   a later `init --workspace --yes` that does **not** repeat the flag neither
//!   proposes nor adds the excluded directory;
//! - `workspace status` (human and `--json`) lists the directories it excluded;
//! - an explicit `members` entry that matches a glob is kept, and both the
//!   command's stderr and `workspace status` name the entry and the glob;
//! - a malformed glob exits 2 naming the key;
//! - a manifest without the key reports nothing new.
//!
//! Every fixture lives in a fresh temporary directory with
//! `GIT_CEILING_DIRECTORIES` set to its parent, so no command can resolve a
//! repository above the fixture, and every git call turns auto-maintenance off.
//!
//! [CR-223]: ../../docs/requests/CR-223-workspace-manifest-excludes-members.md
//! [FR-WS-40]: ../../docs/specs/requirements/FR-WS-40.md

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// A fixture workspace: the `TempDir` plus the ceiling that fences it in.
struct Fixture {
    tmp: TempDir,
    ceiling: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let ceiling = tmp
            .path()
            .canonicalize()
            .unwrap()
            .parent()
            .expect("a temp dir has a parent")
            .to_path_buf();
        Self { tmp, ceiling }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn logos(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(self.root())
            .args(args)
            .env("GIT_CEILING_DIRECTORIES", &self.ceiling)
            .output()
            .expect("the logos binary runs")
    }

    fn repo(&self, name: &str) {
        let dir = self.root().join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("lib.rs"), "pub fn member_entry() -> u32 { 7 }\n").unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["add", "."][..],
            &["commit", "-q", "-m", "init"][..],
        ] {
            let out = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
                .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
                .args(args)
                .env("GIT_CEILING_DIRECTORIES", &self.ceiling)
                .output()
                .expect("git is on PATH");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    fn manifest(&self) -> String {
        fs::read_to_string(self.root().join("logos.workspace.toml")).unwrap()
    }
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("no signal termination")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

fn member_names(report: &serde_json::Value) -> Vec<String> {
    report["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect()
}

/// AC: `init --workspace --exclude svc-fork` writes the glob; a later
/// `init --workspace --yes` without `--exclude` neither proposes nor adds it.
#[test]
fn init_exclude_persists_and_a_later_yes_run_does_not_bring_the_directory_back() {
    let fx = Fixture::new();
    for name in ["api", "web", "svc-fork"] {
        fx.repo(name);
    }

    let first = fx.logos(&["--json", "init", "--workspace", "--yes", "--exclude", "svc-fork"]);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    assert_eq!(member_names(&json(&first)), ["api", "web"]);
    let manifest = fx.manifest();
    assert!(manifest.contains("exclude = [\"svc-fork\"]"), "the glob is written:\n{manifest}");
    assert!(manifest.contains("members = [\n    \"api\",\n    \"web\",\n]"), "{manifest}");

    // The scripted re-run: `--yes`, no `--exclude`.
    let rerun = fx.logos(&["--json", "init", "--workspace", "--yes"]);
    assert_eq!(code(&rerun), 0, "{}", stderr(&rerun));
    assert_eq!(member_names(&json(&rerun)), ["api", "web"], "svc-fork is not added");
    assert_eq!(fx.manifest(), manifest, "the manifest is byte-identical after the re-run");
    assert!(
        !fx.root().join("svc-fork").join(".logos").exists(),
        "an excluded directory is never initialised"
    );

    // Control: the same re-run on a manifest WITHOUT the key does add it, so the
    // assertions above are about the key and not about the fixture.
    let control = Fixture::new();
    for name in ["api", "web", "svc-fork"] {
        control.repo(name);
    }
    let out = control.logos(&["--json", "init", "--workspace", "--yes"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(member_names(&json(&out)), ["api", "svc-fork", "web"]);
}

/// AC: with autodiscover on and `exclude = ["*-fork"]`, `svc-fork` is absent from
/// the member set, and `workspace status` (human and `--json`) lists it as excluded.
#[test]
fn workspace_status_lists_the_excluded_directories_in_both_forms() {
    let fx = Fixture::new();
    for name in ["api", "svc-fork"] {
        fx.repo(name);
    }
    fs::write(
        fx.root().join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nexclude = [\"*-fork\"]\n\n[workspace.autodiscover]\n",
    )
    .unwrap();

    let machine = fx.logos(&["--json", "workspace", "status"]);
    assert_eq!(code(&machine), 0, "{}", stderr(&machine));
    let status = json(&machine);
    let roster: Vec<&str> = status["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["member"].as_str().unwrap())
        .collect();
    assert_eq!(roster, ["api"], "the fork is absent from the fan-out");
    assert_eq!(
        status["exclusion"],
        serde_json::json!({ "globs": ["*-fork"], "excluded": ["svc-fork"], "conflicts": [] })
    );

    let human = fx.logos(&["workspace", "status"]);
    assert_eq!(code(&human), 0, "{}", stderr(&human));
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("\"exclusion\"") && text.contains("\"svc-fork\""), "{text}");
}

/// AC: an explicit `members` entry matching a glob is kept, and every workspace
/// load and `workspace status` warn naming the entry and the glob.
#[test]
fn an_explicit_member_matching_a_glob_is_kept_and_warned_on_every_load() {
    let fx = Fixture::new();
    for name in ["api", "svc-fork"] {
        fx.repo(name);
    }
    fs::write(
        fx.root().join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"svc-fork\"]\nexclude = [\"*-fork\"]\n",
    )
    .unwrap();

    let out = fx.logos(&["--json", "workspace", "status"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let status = json(&out);
    let roster: Vec<&str> = status["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["member"].as_str().unwrap())
        .collect();
    assert_eq!(roster, ["api", "svc-fork"], "the explicit entry is kept");
    let conflicts = &status["exclusion"]["conflicts"];
    assert_eq!(conflicts[0]["entry"], "svc-fork");
    assert_eq!(conflicts[0]["glob"], "*-fork");
    assert_eq!(status["exclusion"]["excluded"], serde_json::json!([]));
    let warning = conflicts[0]["warning"].as_str().unwrap();
    assert!(warning.contains("svc-fork") && warning.contains("*-fork"), "{warning}");

    // Every load warns, not only status: an ordinary workspace command does too.
    let other = fx.logos(&["--json", "workspace", "check"]);
    let log = stderr(&other);
    assert!(
        log.contains("svc-fork") && log.contains("*-fork") && log.contains("explicit member wins"),
        "the load warning names the entry and the glob:\n{log}"
    );
}

/// AC: a malformed glob exits 2 naming the key — on every command that loads the
/// workspace, because the walk is on the path of all of them.
#[test]
fn a_malformed_exclude_glob_exits_2_naming_the_key() {
    let fx = Fixture::new();
    fx.repo("api");
    fs::write(
        fx.root().join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nexclude = [\"[svc\"]\n\n[workspace.autodiscover]\n",
    )
    .unwrap();
    for args in [&["workspace", "status"][..], &["init", "--workspace", "--yes"][..]] {
        let out = fx.logos(args);
        assert_eq!(code(&out), 2, "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains("workspace.exclude"), "{args:?}: {}", stderr(&out));
    }
}

/// A manifest without the key reports nothing: no `exclusion` block in status
/// and no `exclude` line after an init re-run.
#[test]
fn a_manifest_without_exclude_is_untouched_and_status_has_no_block() {
    let fx = Fixture::new();
    for name in ["api", "web"] {
        fx.repo(name);
    }
    let first = fx.logos(&["--json", "init", "--workspace", "--yes"]);
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let manifest = fx.manifest();
    assert!(!manifest.contains("exclude"), "{manifest}");

    let rerun = fx.logos(&["--json", "init", "--workspace", "--yes"]);
    assert_eq!(code(&rerun), 0, "{}", stderr(&rerun));
    assert_eq!(fx.manifest(), manifest);

    let status = json(&fx.logos(&["--json", "workspace", "status"]));
    assert!(status.get("exclusion").is_none(), "{status}");
}

/// Review finding (S-707): `init --workspace --exclude X` over an autodiscovered
/// workspace must not pin `X` as an explicit member beside the glob that excludes
/// it. The autodiscovered fork leaves the written `members`, the glob lands in
/// `exclude`, and the next load reports no contradiction.
#[test]
fn init_exclude_takes_an_autodiscovered_member_out_instead_of_pinning_it() {
    let fx = Fixture::new();
    for name in ["api", "web", "svc-fork"] {
        fx.repo(name);
    }
    fs::write(
        fx.root().join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\n\n[workspace.autodiscover]\n",
    )
    .unwrap();

    let out = fx.logos(&["--json", "init", "--workspace", "--yes", "--exclude", "svc-fork"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let manifest = fx.manifest();
    assert!(manifest.contains("exclude = [\"svc-fork\"]"), "{manifest}");
    assert!(!manifest.contains("\"svc-fork\",\n") && !manifest.contains("\"svc-fork\"\n]"),
        "svc-fork is not written into members:\n{manifest}");

    let status = json(&fx.logos(&["--json", "workspace", "status"]));
    let roster: Vec<&str> = status["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["member"].as_str().unwrap())
        .collect();
    assert_eq!(roster, ["api", "web"]);
    assert_eq!(status["exclusion"]["excluded"], serde_json::json!(["svc-fork"]));
    assert_eq!(status["exclusion"]["conflicts"], serde_json::json!([]));
}

/// The other half: an **explicit** `members` entry that a new `--exclude` glob
/// matches stays (an explicit member wins), and the run says so.
#[test]
fn init_exclude_keeps_an_explicit_member_and_warns() {
    let fx = Fixture::new();
    for name in ["api", "svc-fork"] {
        fx.repo(name);
    }
    fs::write(
        fx.root().join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"svc-fork\"]\n",
    )
    .unwrap();

    let out = fx.logos(&["--json", "init", "--workspace", "--yes", "--exclude", "svc-fork"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(member_names(&json(&out)), ["api", "svc-fork"], "the explicit member is kept");
    let log = stderr(&out);
    assert!(
        log.contains("svc-fork") && log.contains("explicit member wins"),
        "the run names the conflict it just persisted:\n{log}"
    );
}
