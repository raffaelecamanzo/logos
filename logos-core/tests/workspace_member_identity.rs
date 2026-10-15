//! S-706 ([FR-WS-39], [FR-WS-16], [ADR-85]): every workspace member carries the
//! repository coordinate it resolves to, and two members resolving to one
//! coordinate are never folded — the later in manifest order is reported
//! degraded, naming both checkouts and the `[project] id` override, and is
//! excluded from fan-out, the bridge and coverage like any member that will not
//! open.
//!
//! The fixture is the one the failure comes from: **two clones of one remote**
//! in one workspace. Each clone is a real git repository whose `origin` names
//! the same hosted URL, so the identity is derived by the production path (the
//! normalised remote) and not pinned by the test. It is built under a temporary
//! directory outside every work tree, with `GIT_CEILING_DIRECTORIES` set so no
//! enclosing repository can be adopted.
//!
//! [FR-WS-16]: ../../docs/specs/requirements/FR-WS-16.md
//! [FR-WS-39]: ../../docs/specs/requirements/FR-WS-39.md
//! [ADR-85]: ../../docs/specs/architecture/decisions/ADR-85.md

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::{
    discover, workspace_status, ContractBridge, EngineRegistry, RegistryMode, WorkspaceStatus,
    MANIFEST_FILENAME,
};
use logos_core::Engine;
use tempfile::TempDir;

/// The hosted URL both clones name as `origin`; the coordinate it derives.
const REMOTE: &str = "https://github.com/acme/orders.git";
const REMOTE_COORDINATE: &str = "github.com/acme/orders";

/// Run `git` in `dir` with background maintenance off and no enclosing
/// repository reachable, failing the test on a non-zero exit.
fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap_or(dir))
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A repository with one committed source file and `origin` = `remote`.
fn repo(dir: &Path, remote: &str) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    write(&dir.join("src/index.ts"), "export function start(): void {}\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    git(dir, &["remote", "add", "origin", remote]);
}

/// A workspace of two clones of one remote (`orders-a`, `orders-b`) and an
/// unrelated service (`billing`), declared in that manifest order.
///
/// The second clone is a real `git clone` of the first, then pointed at the
/// shared hosted URL, so the pair is exactly what a developer's second checkout
/// of one repository looks like.
fn two_clone_workspace() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp dir");
    let ws = tmp.path().canonicalize().unwrap().join("ws");
    fs::create_dir_all(&ws).unwrap();
    repo(&ws.join("orders-a"), REMOTE);
    git(&ws, &["clone", "-q", "orders-a", "orders-b"]);
    git(&ws.join("orders-b"), &["remote", "set-url", "origin", REMOTE]);
    repo(&ws.join("billing"), "https://github.com/acme/billing.git");
    write(
        &ws.join(MANIFEST_FILENAME),
        "[workspace]\nname = \"shop\"\nmembers = [\"orders-a\", \"orders-b\", \"billing\"]\n",
    );
    (tmp, ws)
}

fn registry(ws: &Path) -> EngineRegistry<Engine> {
    let federation = discover(ws).expect("manifest").expect("a workspace");
    EngineRegistry::new(federation, RegistryMode::Lazy)
}

fn row<'a>(status: &'a serde_json::Value, member: &str) -> &'a serde_json::Value {
    status["members"]
        .as_array()
        .expect("member rows")
        .iter()
        .find(|row| row["member"] == member)
        .unwrap_or_else(|| panic!("no row for {member}: {status}"))
}

fn status_json(status: &WorkspaceStatus) -> serde_json::Value {
    serde_json::to_value(status).expect("status serialises")
}

/// AC1 and AC2: the second clone is degraded and names both paths and the
/// override; the first answers normally; every row lists its coordinate.
#[test]
fn the_second_clone_of_one_remote_is_degraded_and_the_first_answers() {
    let (_tmp, ws) = two_clone_workspace();
    let registry = registry(&ws);
    let status = status_json(&workspace_status(&registry));

    // The identity is the production derivation: the normalised remote.
    for (member, coordinate) in [
        ("orders-a", REMOTE_COORDINATE),
        ("orders-b", REMOTE_COORDINATE),
        ("billing", "github.com/acme/billing"),
    ] {
        assert_eq!(row(&status, member)["coordinate"], coordinate, "{status}");
    }

    let first = row(&status, "orders-a");
    assert_eq!(first["open_state"], "opened", "the first holder answers: {first}");
    assert!(first["result"].is_object(), "and reports its index: {first}");
    assert_eq!(row(&status, "billing")["open_state"], "opened", "{status}");

    let second = row(&status, "orders-b");
    assert_eq!(second["open_state"], "degraded", "{second}");
    assert_eq!(second["degraded_cause"], "duplicate-coordinate", "{second}");
    let reason = second["degraded_reason"].as_str().expect("a reason");
    let first_root = ws.join("orders-a");
    let second_root = ws.join("orders-b");
    for needle in [
        first_root.to_str().unwrap(),
        second_root.to_str().unwrap(),
        REMOTE_COORDINATE,
        "[project] id",
    ] {
        assert!(reason.contains(needle), "{needle:?} missing from: {reason}");
    }
    assert_eq!(second["error"], reason, "`error` carries the same sentence: {second}");

    assert_eq!(status["degraded_rollup"]["degraded_members"], serde_json::json!(["orders-b"]));
    assert!(registry.open_states().iter().any(|open| open.state.is_degraded()));
}

/// AC5: a degraded duplicate is excluded from fan-out, the bridge and coverage
/// exactly as any other degraded member — unread, named, and absent from the
/// denominators' numerators.
#[test]
fn the_duplicate_is_excluded_from_fan_out_the_bridge_and_coverage() {
    let (_tmp, ws) = two_clone_workspace();
    let registry = registry(&ws);

    // Fan-out: one `Err` row for the duplicate, a result for each other member.
    let answer = registry.answer();
    let rows = answer.fan_out(|member, _engine| member.name.clone());
    let outcome: Vec<(&str, bool)> =
        rows.iter().map(|row| (row.member.as_str(), row.value.is_ok())).collect();
    assert_eq!(outcome, [("orders-a", true), ("orders-b", false), ("billing", true)]);
    assert!(registry.engine_for("orders-b").is_err(), "and a named request is refused too");

    // Bridge: the duplicate is a member the read could not take.
    let (_edges, reads) = ContractBridge::new().edges_read(&registry);
    assert!(!reads.read.contains("orders-b"), "{reads:?}");
    assert!(reads.unread.contains_key("orders-b"), "{reads:?}");
    assert!(reads.read.contains("orders-a") && reads.read.contains("billing"), "{reads:?}");

    // Coverage: read over two of three members, and says so.
    let status = workspace_status(&registry);
    assert_eq!(
        (status.coverage.members_read, status.coverage.members_total),
        (2, 3),
        "the duplicate is a member the coverage tier did not read"
    );
    assert!(!status.coverage.covers_all_members);
    assert!(!status.degraded_rollup.covers_all_members);
}

/// AC7: a distinct `[project] id` in the duplicate, and a re-opened workspace,
/// clears the degradation — and the first clone, which never moved, is unchanged.
#[test]
fn overriding_the_duplicates_project_id_and_reopening_clears_the_degradation() {
    let (_tmp, ws) = two_clone_workspace();
    {
        let before = status_json(&workspace_status(&registry(&ws)));
        assert_eq!(row(&before, "orders-b")["open_state"], "degraded", "{before}");
    }

    write(
        &ws.join("orders-b/.logos/config.toml"),
        "[project]\nid = \"github.com/acme/orders-fork\"\n",
    );
    let reopened = registry(&ws);
    let after = status_json(&workspace_status(&reopened));

    assert_eq!(row(&after, "orders-a")["coordinate"], REMOTE_COORDINATE, "{after}");
    assert_eq!(row(&after, "orders-b")["coordinate"], "github.com/acme/orders-fork", "{after}");
    for member in ["orders-a", "orders-b", "billing"] {
        assert_eq!(row(&after, member)["open_state"], "opened", "{member}: {after}");
    }
    assert_eq!(after["degraded_rollup"]["degraded_members"], serde_json::json!([]), "{after}");
    assert!(reopened.open_states().iter().all(|open| !open.state.is_degraded()));
    assert!(after["coverage"]["covers_all_members"].as_bool().unwrap(), "{after}");
}

/// AC2's other half: a workspace of distinct coordinates is not degraded and
/// reports each one, in manifest order.
#[test]
fn a_workspace_of_distinct_coordinates_degrades_nothing() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().canonicalize().unwrap().join("ws");
    for name in ["api", "web"] {
        repo(&ws.join(name), &format!("https://github.com/acme/{name}.git"));
    }
    write(&ws.join(MANIFEST_FILENAME), "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\n");
    let registry = registry(&ws);
    let status = status_json(&workspace_status(&registry));

    assert_eq!(row(&status, "api")["coordinate"], "github.com/acme/api");
    assert_eq!(row(&status, "web")["coordinate"], "github.com/acme/web");
    assert_eq!(status["degraded_rollup"]["degraded_members"], serde_json::json!([]), "{status}");
    assert_eq!(registry.member_coordinates(), ["github.com/acme/api", "github.com/acme/web"]);
}

/// A member whose config does not load keeps the coordinate its file declares:
/// the engine opens without reading the config, so the member still serves a
/// store whose symbols carry that coordinate, and it must still collide with the
/// first holder rather than escape under its derived identity.
#[test]
fn a_member_whose_config_does_not_load_keeps_its_declared_coordinate() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().canonicalize().unwrap().join("ws");
    for name in ["svc-a", "svc-b"] {
        repo(&ws.join(name), &format!("https://github.com/acme/{name}.git"));
        write(&ws.join(name).join(".logos/config.toml"), "[project]\nid = \"shared.example/svc\"\n");
    }
    // The later member's config now fails to load (not TOML after its table).
    write(
        &ws.join("svc-b/.logos/config.toml"),
        "[project]\nid = \"shared.example/svc\"\nthis is not toml\n",
    );
    write(&ws.join(MANIFEST_FILENAME), "[workspace]\nname = \"w\"\nmembers = [\"svc-a\", \"svc-b\"]\n");

    let registry = registry(&ws);
    assert_eq!(registry.member_coordinates(), ["shared.example/svc", "shared.example/svc"]);
    let status = status_json(&workspace_status(&registry));
    assert_eq!(row(&status, "svc-a")["open_state"], "opened", "{status}");
    assert_eq!(row(&status, "svc-b")["degraded_cause"], "duplicate-coordinate", "{status}");

    // With no valid declared id either, the member takes the derived identity.
    write(&ws.join("svc-b/.logos/config.toml"), "[project]\nid = \"bad id\"\nthis is not toml\n");
    assert_eq!(
        self::registry(&ws).member_coordinates(),
        ["shared.example/svc", "github.com/acme/svc-b"]
    );
}
