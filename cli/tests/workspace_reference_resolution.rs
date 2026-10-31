//! End-to-end tests for the `reference_resolution` section of `logos workspace
//! status` (S-637, [FR-RS-49], [BR-56]), driven through the **real** `logos`
//! binary over two indexed Rust members.
//!
//! `client` calls `Profile::load`, a type only `server` declares, and makes an
//! HTTP client call `server`'s axum route serves. Its own figure counts the
//! first provably external — nothing `client` declares is `Profile` or `load` —
//! and leaves the second unbound; the workspace counts the first an internal
//! candidate and the second resolved, through the bridge. The control
//! workspace's `server` declares no `Profile`, so the difference between the two
//! is that one call.
//!
//! [FR-RS-49]: ../../docs/specs/requirements/FR-RS-49.md
//! [FR-WS-16]: ../../docs/specs/requirements/FR-WS-16.md
//! [BR-56]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}

pub fn build() {
    let _ = Profile::load();
}
"#;

const SERVER_ROUTE: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{id}", get(get_user))
}
"#;

const PROFILE: &str = r#"
pub struct Profile;

impl Profile {
    pub fn load() -> Self {
        Profile
    }
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

/// `logos workspace status --json`, asserting exit 0 and one JSON line.
fn status_json(project: &Path) -> Value {
    let out = logos(project, &["workspace", "status", "--json"]);
    assert!(out.status.success(), "workspace status: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim().lines().count(), 1, "--json is one line: {stdout}");
    serde_json::from_str(stdout.trim()).expect("--json stdout is JSON")
}

/// A two-member workspace, `client` and `server`, each indexed through the
/// real binary; `server` declares `Profile` only when `profile`.
fn workspace(profile: bool) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for name in ["client", "server"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        sh_git(&dir, &["init", "-q", "-b", "main"]);
        if name == "client" {
            write(&dir, "src/lib.rs", CLIENT);
        } else {
            write(&dir, "src/main.rs", SERVER_ROUTE);
            if profile {
                write(&dir, "src/profile.rs", PROFILE);
            }
        }
        sh_git(&dir, &["add", "."]);
        sh_git(&dir, &["commit", "-q", "-m", "init"]);
        assert!(logos(&dir, &["index"]).status.success(), "index {name}");
    }
    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"client\", \"server\"]\ndefault = \"client\"\n",
    )
    .unwrap();
    tmp
}

/// A member row's own `internal_resolution`.
fn own<'a>(status: &'a Value, member: &str) -> &'a Value {
    let row = status["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|row| row["member"] == member)
        .unwrap_or_else(|| panic!("no {member} row: {status}"));
    &row["result"]["internal_resolution"]
}

fn count(value: &Value, field: &str) -> u64 {
    value[field].as_u64().unwrap_or_else(|| panic!("{field}: {value}"))
}

/// The pooled figure is the members' own populations summed, with the rows
/// the workspace re-read moved: never a mean of their ratios ([BR-56]).
///
/// [BR-56]: ../../docs/specs/software-spec.md#327-workspace-federation
fn assert_pooled(status: &Value) {
    let section = &status["reference_resolution"];
    let (figure, cross) = (&section["internal_resolution"], &section["cross_member"]);
    let sum = |field| count(own(status, "client"), field) + count(own(status, "server"), field);
    assert_eq!(count(figure, "code_references"), sum("code_references"), "{section}");
    assert_eq!(count(figure, "resolved"), sum("resolved") + count(cross, "bridge_bound"), "{section}");
    let reread = count(cross, "named_by_another_member") + count(cross, "type_in_another_member");
    assert_eq!(count(figure, "provably_external"), sum("provably_external") - reread, "{section}");
    let ratio = count(figure, "resolved") as f64 / count(figure, "internal_candidates") as f64;
    assert_eq!(figure["lower_bound"].as_f64(), Some(ratio), "{section}");
    assert_eq!(section["members"], serde_json::json!({"members": 2, "read": 2}), "{section}");
}

#[test]
fn a_call_into_another_members_type_is_internal_and_a_bridge_bound_call_resolved() {
    let with = workspace(true);
    let without = workspace(false);
    let (status, control) = (status_json(with.path()), status_json(without.path()));
    assert_pooled(&status);
    assert_pooled(&control);

    // `client`'s own figure is its own graph's, whatever its siblings declare.
    assert_eq!(own(&status, "client"), own(&control, "client"));
    let (cross, control_cross) =
        (&status["reference_resolution"]["cross_member"], &control["reference_resolution"]["cross_member"]);
    // `Profile::load`: provably external in `client`, an internal candidate in
    // the workspace whose `server` declares it — and in no other.
    assert_eq!(
        count(cross, "named_by_another_member"),
        count(control_cross, "named_by_another_member") + 1,
        "{cross} vs {control_cross}"
    );
    // The client call `server`'s route serves: unbound in `client`, resolved by
    // the bridge in either workspace.
    assert_eq!((count(cross, "bridge_bound"), count(control_cross, "bridge_bound")), (1, 1));
}

#[test]
fn the_human_rendering_states_the_same_numbers_and_its_summary_line() {
    let tmp = workspace(true);
    let json = status_json(tmp.path());
    let out = logos(tmp.path(), &["workspace", "status"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let human: Value = serde_json::from_str(&stdout).expect("the human rendering is the read-model");
    assert_eq!(human["reference_resolution"], json["reference_resolution"]);
    let summary = json["reference_resolution"]["summary"].as_str().expect("summary");
    assert!(stdout.contains(summary), "{stdout}");
    let figure = &json["reference_resolution"]["internal_resolution"];
    assert!(
        summary.starts_with(&format!(
            "lower bound {:.3} ({} of {} internal candidates resolved;",
            figure["lower_bound"].as_f64().expect("a ratio"),
            figure["resolved"],
            figure["internal_candidates"]
        )),
        "{summary}"
    );
    assert!(summary.contains("summed over 2 of 2 members"), "{summary}");
}

#[test]
fn a_member_that_cannot_be_read_is_named_and_not_summed() {
    let tmp = workspace(true);
    // A directory where the store must be: `server` is attempted and fails to
    // open, so the command exits 1 ([FR-WS-16]) and still answers.
    let db = tmp.path().join("server/.logos/logos.db");
    std::fs::remove_file(&db).expect("clear the store file");
    std::fs::create_dir_all(&db).expect("a directory where the store must be");
    let out = logos(tmp.path(), &["workspace", "status", "--json"]);
    assert_eq!(out.status.code(), Some(1), "one unopenable member");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let status: Value = serde_json::from_str(stdout.trim()).expect("--json stdout is JSON");
    let section = &status["reference_resolution"];
    let members = &section["members"];
    assert_eq!((count(members, "members"), count(members, "read")), (2, 1), "{section}");
    assert_eq!(members["unread"], serde_json::json!(["server"]), "{section}");
    assert!(members["unread_reasons"]["server"].as_str().is_some_and(|r| !r.is_empty()), "{section}");
    assert_eq!(section["internal_resolution"], *own(&status, "client"), "only client is summed: {section}");
    assert!(section["summary"].as_str().expect("summary").ends_with("; not read: server"), "{section}");
}
