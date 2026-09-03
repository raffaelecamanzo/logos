//! End-to-end tests for the `logos xservice` cross-service query group and
//! `logos workspace status` (S-248, [FR-WS-05]), driven through the **real**
//! `logos` binary over a two-member workspace fixture so repo-qualification,
//! `--repo` scoping, and machine-clean `--json` are asserted exactly as a user
//! (or the web API in S-249) sees them.
//!
//! The fixture mirrors the bridge integration test: an OpenAPI operation in
//! member `api` binds a framework route in member `web` via `route_key`, so the
//! coverage summary reports one bound reference and `route-providers` reports
//! one cross-service edge. Gated on `lang-all` (the default) so the OpenAPI +
//! axum grammars are present.
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// An OpenAPI spec whose `/users/{user_id}` `get` operation matches the axum
/// route's `/users/{id}` (the `route_key` param-drift erasure); its `delete`
/// has no provider anywhere in the workspace.
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
    delete:
      summary: Delete a user
";

/// An axum app registering exactly one route, `GET /users/{id}`. `orphan` is
/// called by nobody — the annotation pass verdicts it dead, giving `workspace
/// reachability` (S-257, [FR-WS-12]) a real per-repo dead callable to claim over.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn orphan() -> i32 {
    41 + 1
}

fn app() -> Router {
    Router::new().route("/users/{id}", get(get_user))
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// Run a git command in `cwd`, panicking on failure — fixtures only.
fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run the built `logos` binary against `project`, returning its output.
fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

/// Run `logos --json <args>` and parse the single machine-clean stdout line as
/// JSON, asserting exit 0 and that stdout carries JSON only (FR-CL-02).
fn logos_json(project: &Path, args: &[&str]) -> Value {
    logos_json_exiting(project, args, 0)
}

/// As [`logos_json`], but for a command whose exit code is deliberately not 0.
///
/// [FR-WS-16] makes an unopenable member a **non-zero** exit, so a fixture with
/// a broken member can no longer go through `logos_json` — and the payload must
/// still be machine-clean JSON on stdout, which is exactly what this asserts.
fn logos_json_exiting(project: &Path, args: &[&str], code: i32) -> Value {
    let mut full = args.to_vec();
    full.push("--json");
    let out = logos(project, &full);
    assert_eq!(
        out.status.code(),
        Some(code),
        "logos {args:?} exited {:?}, expected {code}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("`logos {args:?} --json` stdout is not machine-clean JSON: {e}\nstdout: {stdout}")
    })
}

/// A committed git repo — `discover` keeps only members that are distinct git
/// roots (FR-WS-01), so each member must be its own repository.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join(".gitkeep"), "").unwrap();
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// Build the two-member workspace: `api` (OpenAPI consumer) + `web` (axum
/// provider), each an indexed git repo, with the manifest at the parent.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let api = root.join("api");
    let web = root.join("web");

    init_repo(&api);
    init_repo(&web);
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&web, "src/main.rs", AXUM_MAIN);

    // Index each member through the real binary (E2E, no library dependency).
    assert!(logos(&api, &["index"]).status.success(), "index api");
    assert!(logos(&web, &["index"]).status.success(), "index web");

    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    tmp
}

/// AC2: `workspace status` reports per-member freshness and the 3-state
/// coverage summary — the GET operation is bound, DELETE has no provider.
#[test]
fn workspace_status_reports_freshness_and_three_state_coverage() {
    let tmp = workspace();
    let status = logos_json(tmp.path(), &["workspace", "status"]);

    assert_eq!(status["workspace"], "shop");

    // Per-member freshness: both members present, each carrying a status
    // read-model (the `indexed`/`node_count` freshness fields).
    let members = status["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2, "both members reported: {members:?}");
    let mut member_names: Vec<&str> = members.iter().map(|m| m["member"].as_str().unwrap()).collect();
    member_names.sort_unstable();
    assert_eq!(member_names, ["api", "web"]);
    for m in members {
        assert!(
            m["result"]["indexed"].as_bool().unwrap_or(false),
            "member {} carries index freshness: {m}",
            m["member"]
        );
    }

    // The 3-state coverage summary from S-247.
    let coverage = &status["coverage"];
    assert_eq!(coverage["bound"], 1, "the GET operation binds its cross-member route");
    assert_eq!(
        coverage["no_provider_in_workspace"], 1,
        "DELETE has no provider — bucketed separately"
    );
    assert_eq!(coverage["ambiguous"], 0);
    assert_eq!(
        coverage["bound_ratio"], 1.0,
        "no-provider references never depress the bound-ratio (ADR-53)"
    );
}

// ── S-323: per-member warm state and roll-up (FR-WS-15, BR-44, NFR-CC-04) ──

/// A workspace of `members`, each a committed git repo (so `discover` keeps it),
/// indexed only if named in `index`.
///
/// The warm state is a fact about *index presence*, so what the fixture varies
/// is exactly which members were indexed — never a stub or an injected label.
fn warm_fixture(members: &[&str], index: &[&str]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for member in members {
        let repo = root.join(member);
        init_repo(&repo);
        write(&repo, "src/lib.rs", "pub fn f() {}\n");
        if index.contains(member) {
            assert!(logos(&repo, &["index"]).status.success(), "index {member}");
        }
    }
    let list = members
        .iter()
        .map(|m| format!("\"{m}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        root.join("logos.workspace.toml"),
        format!("[workspace]\nname = \"shop\"\nmembers = [{list}]\n"),
    )
    .unwrap();
    tmp
}

/// Break a member so its engine cannot be opened at all: `.logos/logos.db` as a
/// **directory**, which no store open can succeed against. The one warm failure
/// that is durable today — the supervisor's own per-member failures reach a
/// `/dev/null` stderr, so `workspace status` cannot see them (see
/// `federation::warm_state`).
fn break_store(root: &Path, member: &str) {
    let db = root.join(member).join(".logos").join("logos.db");
    if db.exists() {
        std::fs::remove_file(&db).expect("clear the store file");
    }
    std::fs::create_dir_all(&db).expect("a directory where the store must be");
}

/// The per-member `warm_state` label keyed by member name.
fn warm_states(status: &Value) -> Vec<(String, String)> {
    let mut states: Vec<(String, String)> = status["members"]
        .as_array()
        .expect("members array")
        .iter()
        .map(|m| {
            (
                m["member"].as_str().expect("member name").to_string(),
                m["warm_state"]
                    .as_str()
                    .unwrap_or_else(|| panic!("member row carries a warm_state: {m}"))
                    .to_string(),
            )
        })
        .collect();
    states.sort();
    states
}

/// [FR-WS-15] AC1/AC2/AC4: a mixed workspace labels each member `warm` /
/// `deferred` / `degraded` and prints the roll-up — and `--json` carries the same
/// per-member field and roll-up the human rendering does.
///
/// `cold` is the load-bearing case: it is un-indexed and was never attempted, so
/// it must read `deferred` rather than merely "stale" — the whole point of the
/// story once the warm is deliberately bounded.
#[test]
fn workspace_status_labels_each_member_warm_deferred_or_degraded_with_a_rollup() {
    let tmp = warm_fixture(&["api", "cold", "broken"], &["api", "broken"]);
    break_store(tmp.path(), "broken");

    // `broken` cannot be opened, so the command exits 1 ([FR-WS-16]) — the warm
    // labelling below is unchanged by that.
    let status = logos_json_exiting(tmp.path(), &["workspace", "status"], 1);
    assert_eq!(
        warm_states(&status),
        [
            ("api".to_string(), "warm".to_string()),
            ("broken".to_string(), "degraded".to_string()),
            ("cold".to_string(), "deferred".to_string()),
        ],
        "each member carries its own state: {status}"
    );

    let rollup = &status["warm_rollup"];
    assert_eq!(rollup["members"], 3);
    assert_eq!(rollup["warm"], 1);
    assert_eq!(rollup["deferred"], 1);
    assert_eq!(rollup["degraded"], 1, "a failed member is degraded, never deferred (BR-44)");

    // The human rendering is the same read-model (pretty-printed, FR-CL-02), so
    // "both outputs carry the state" is asserted by reading the human stream and
    // finding the identical fields — not by trusting that they must match.
    let human = logos(tmp.path(), &["workspace", "status"]);
    assert_eq!(human.status.code(), Some(1), "one unopenable member (FR-WS-16)");
    let human: Value = serde_json::from_str(&String::from_utf8(human.stdout).unwrap())
        .expect("the human rendering is the same read-model, pretty-printed");
    assert_eq!(warm_states(&human), warm_states(&status));
    assert_eq!(human["warm_rollup"], status["warm_rollup"]);
}

/// [FR-WS-15] AC3: a fully warmed workspace reports every member `warm`, with no
/// `deferred` and no `warming`.
#[test]
fn a_fully_warmed_workspace_reports_every_member_warm() {
    let tmp = warm_fixture(&["api", "web"], &["api", "web"]);
    let status = logos_json(tmp.path(), &["workspace", "status"]);

    for (member, state) in warm_states(&status) {
        assert_eq!(state, "warm", "member {member} is indexed: {status}");
    }
    let rollup = &status["warm_rollup"];
    assert_eq!(rollup["members"], 2);
    assert_eq!(rollup["warm"], 2);
    assert_eq!(rollup["deferred"], 0);
    assert_eq!(rollup["degraded"], 0);
    assert!(rollup.get("warming").is_none(), "no warming key at all: {rollup}");
}

/// [FR-WS-15] AC4 + AC5 / [FR-WS-16] AC1 / [NFR-CC-04] over **one** fixture
/// matrix: the **warm** axis never moves the exit code, the **open** axis
/// always does, and `warming` is omitted rather than inferred — across every
/// combination, so all three are properties of the read-model and not of one
/// fixture.
///
/// The exit code is now the axis discriminator, which is the whole point of
/// [FR-WS-16] keeping `warm_state` and `open_state` separate: `all deferred` is
/// a workspace nobody has indexed yet and exits **0**, while `all degraded` is a
/// workspace nothing can open and exits **1**. Before this story both exited 0.
///
/// Each row **proves the state it names.** Asserting only the exit code would
/// let the coverage rot silently: if `break_store` ever stopped breaking (the
/// store filename moves, say) the `degraded` rows would collapse into duplicates
/// of the `warm` rows and this test would stay green while asserting nothing.
/// The realized label multiset is therefore asserted per row, and the expected
/// exit code is *derived from the fixture's broken set* rather than restated, so
/// the two cannot drift.
#[test]
fn the_warm_axis_never_moves_the_exit_code_and_the_open_axis_always_does() {
    for (label, members, index, broken, expect) in [
        ("all deferred", &["api", "web", "svc"][..], &[][..], &[][..], &["deferred", "deferred", "deferred"][..]),
        ("all warm", &["api", "web"][..], &["api", "web"][..], &[][..], &["warm", "warm"][..]),
        ("mixed", &["api", "web"][..], &["api"][..], &[][..], &["warm", "deferred"][..]),
        ("one degraded", &["api", "web"][..], &["api", "web"][..], &["web"][..], &["warm", "degraded"][..]),
        ("degraded + deferred", &["api", "web"][..], &["api"][..], &["api"][..], &["degraded", "deferred"][..]),
        // Every member broken — the [CR-100] shape, at the smallest N that
        // reproduces it.
        ("all degraded", &["api", "web"][..], &["api", "web"][..], &["api", "web"][..], &["degraded", "degraded"][..]),
    ] {
        let tmp = warm_fixture(members, index);
        for member in broken {
            break_store(tmp.path(), member);
        }
        // Derived, not restated: an unopenable member ⇒ 1, otherwise 0.
        let expected_code = i32::from(!broken.is_empty());

        // [FR-WS-16] AC1: the same exit code in BOTH output modes.
        for args in [&["workspace", "status"][..], &["workspace", "status", "--json"][..]] {
            let out = logos(tmp.path(), args);
            assert_eq!(
                out.status.code(),
                Some(expected_code),
                "{label} / {args:?} exited {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let status = logos_json_exiting(tmp.path(), &["workspace", "status"], expected_code);
        let states = warm_states(&status);

        // The row realized the combination it claims — so a no-op `break_store`
        // or a mislabelled member fails here rather than passing silently.
        let realized: Vec<&str> = states.iter().map(|(_, state)| state.as_str()).collect();
        assert_eq!(realized, expect, "{label}: fixture did not realize its named states: {status}");

        // [FR-WS-16] AC2: the degraded roll-up names exactly the broken members.
        let mut named: Vec<&str> = status["degraded_rollup"]["degraded_members"]
            .as_array()
            .expect("degraded_members array")
            .iter()
            .map(|m| m.as_str().expect("member name"))
            .collect();
        named.sort_unstable();
        let mut expected_named: Vec<&str> = broken.to_vec();
        expected_named.sort_unstable();
        assert_eq!(
            named, expected_named,
            "{label}: the roll-up names exactly the unopenable members: {status}"
        );

        // AC5 / NFR-CC-04: no `warming` label, and no `warming` key at all.
        assert!(
            status["warm_rollup"].get("warming").is_none(),
            "{label}: `warming` must be absent, not 0: {}",
            status["warm_rollup"]
        );
        for (member, state) in &states {
            assert_ne!(
                state, "warming",
                "{label}: member {member} was labeled `warming` with no live signal to derive it from"
            );
        }
    }
}

// ── S-326: degraded-member reporting and non-zero exit (FR-WS-16, BR-45) ──

/// The `degraded_rollup` and the per-member `open_state`s from one payload.
fn open_states(status: &Value) -> Vec<(String, String)> {
    let mut states: Vec<(String, String)> = status["members"]
        .as_array()
        .expect("members array")
        .iter()
        .map(|m| {
            (
                m["member"].as_str().expect("member name").to_string(),
                m["open_state"]
                    .as_str()
                    .unwrap_or_else(|| panic!("member row carries an open_state: {m}"))
                    .to_string(),
            )
        })
        .collect();
    states.sort();
    states
}

/// **[FR-WS-16] AC1/AC2/AC5 end to end.** A workspace with one unopenable
/// member exits **non-zero**, names that member in the `--json` payload *and*
/// in the human rendering, and marks every roll-up as covering fewer than all
/// members.
///
/// This is the [CR-100] failure at the smallest N that reproduces it: before
/// this story the identical fixture exited **0** with the member's failure
/// visible only as an `error` string buried in the payload.
///
/// [CR-100]: ../../docs/requests/CR-100-workspace-resource-budget.md
#[test]
fn an_unopenable_member_exits_non_zero_and_is_named_in_both_output_modes() {
    let tmp = warm_fixture(&["api", "broken"], &["api", "broken"]);
    break_store(tmp.path(), "broken");

    let status = logos_json_exiting(tmp.path(), &["workspace", "status"], 1);

    // AC2: named, and the row says which axis failed.
    let rollup = &status["degraded_rollup"];
    assert_eq!(rollup["members"], 2);
    assert_eq!(rollup["opened"], 1);
    assert_eq!(rollup["not_attempted"], 0);
    assert_eq!(
        rollup["degraded_members"].as_array().unwrap(),
        &vec![Value::from("broken")],
        "the unopenable member is NAMED, not merely counted: {status}"
    );
    // AC5: the roll-ups and the coverage summary are marked partial.
    assert_eq!(
        rollup["covers_all_members"], false,
        "every figure beside this roll-up covers 1 of 2 members"
    );
    assert_eq!(status["coverage"]["members_read"], 1);
    assert_eq!(status["coverage"]["members_total"], 2);
    assert_eq!(status["coverage"]["covers_all_members"], false);

    assert_eq!(
        open_states(&status),
        [
            ("api".to_string(), "opened".to_string()),
            ("broken".to_string(), "degraded".to_string()),
        ],
        "the open axis is per-member on the SAME rows as the warm axis: {status}"
    );

    // The existing per-member `error` detail is untouched, and the additive
    // degraded reason rides beside it — neither replaces the other.
    let broken = status["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["member"] == "broken")
        .expect("the broken member has a row");
    assert!(
        broken["error"].as_str().is_some_and(|e| !e.is_empty()),
        "the verbatim engine diagnostic still rides `error`: {broken}"
    );
    assert!(
        broken["degraded_reason"].as_str().is_some_and(|r| !r.is_empty()),
        "and the classified reason is added beside it: {broken}"
    );

    // AC2, human half: the same exit code, the member named on stderr (stdout
    // stays the machine-clean read-model, FR-CL-02), and the payload carries
    // the roll-up in the human rendering too.
    let human = logos(tmp.path(), &["workspace", "status"]);
    assert_eq!(human.status.code(), Some(1));
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(
        stderr.contains("broken") && stderr.contains("degraded"),
        "the human stream names the degraded member: {stderr}"
    );
    let human: Value = serde_json::from_str(&String::from_utf8(human.stdout).unwrap())
        .expect("the human rendering is the same read-model, pretty-printed");
    assert_eq!(human["degraded_rollup"], status["degraded_rollup"]);
    assert_eq!(open_states(&human), open_states(&status));
}

/// **[FR-WS-16] AC1, the other half.** A workspace where every member opens
/// exits **0** exactly as before, whatever its warm state — so the story adds a
/// failure signal without adding a false one.
#[test]
fn a_workspace_whose_members_all_open_still_exits_zero() {
    // Deliberately un-indexed: `deferred` on the warm axis, `opened` on the
    // open axis. A member with no index is not a member that failed.
    let tmp = warm_fixture(&["api", "cold"], &["api"]);
    let status = logos_json(tmp.path(), &["workspace", "status"]);

    let rollup = &status["degraded_rollup"];
    assert_eq!(rollup["opened"], 2);
    assert!(
        rollup["degraded_members"].as_array().unwrap().is_empty(),
        "nobody is degraded: {status}"
    );
    assert_eq!(rollup["covers_all_members"], true);
    assert_eq!(status["coverage"]["covers_all_members"], true);
    assert_eq!(
        open_states(&status),
        [
            ("api".to_string(), "opened".to_string()),
            ("cold".to_string(), "opened".to_string()),
        ]
    );
    // And nothing is warned about on stderr.
    let human = logos(tmp.path(), &["workspace", "status"]);
    let stderr = String::from_utf8(human.stderr).unwrap();
    assert!(
        !stderr.contains("degraded"),
        "a healthy workspace warns about nothing: {stderr}"
    );
}

/// **[FR-WS-16] AC3.** A member whose store is *missing* names a store cause
/// and a `logos index` remedy; the classification never sends a reader to a
/// re-index for a host-resource failure and never repeats the [FR-DB-02]
/// "unable to open database file" wording that reads as a corrupt store.
///
/// The fd-exhaustion arm itself is asserted in `federation::degraded`'s
/// classification table against [CR-100]'s verbatim diagnostic — exhausting the
/// host's descriptor table from an integration test would be flaky and would
/// bound this suite to one platform's `ulimit`.
///
/// [FR-DB-02]: ../../docs/specs/requirements/FR-DB-02.md
#[test]
fn a_degraded_member_states_a_cause_not_the_raw_sqlite_symptom() {
    let tmp = warm_fixture(&["api", "broken"], &["api", "broken"]);
    break_store(tmp.path(), "broken");

    let status = logos_json_exiting(tmp.path(), &["workspace", "status"], 1);
    let broken = status["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["member"] == "broken")
        .expect("the broken member has a row");

    let reason = broken["degraded_reason"].as_str().expect("a degraded reason");
    assert!(
        !reason.contains("FR-DB-02"),
        "the reason must not be the raw connection-contract symptom: {reason}"
    );
    // `break_store` puts a DIRECTORY at `.logos/logos.db`, which is the only
    // production path on which `store_file` distinguishes its three states — so
    // the cause and the remedy are pinned by value, not merely by presence.
    // Asserting `is_some()` alone would survive `is_file()` being changed to
    // `exists()`, which would silently reclassify this as a host resource limit.
    assert_eq!(
        broken["degraded_cause"], "store-obstructed",
        "a non-regular file at the store path is the obstructed cause: {broken}"
    );
    assert!(
        reason.contains("not a") && reason.contains("regular file"),
        "the reason states what is actually wrong: {reason}"
    );
    assert!(
        reason.contains("clear that path") && reason.contains("logos index"),
        "and gives the remedy that can actually work — clear the path, THEN \
         re-index; a bare `logos index` cannot fix an occupied path: {reason}"
    );
    // The verbatim engine diagnostic survives classification on its own key.
    assert!(
        broken["degraded_diagnostic"]
            .as_str()
            .is_some_and(|d| d.contains("unable to open database file")),
        "classifying a cause must not destroy the evidence it read: {broken}"
    );
}

/// **[FR-WS-16] AC6, pinned structurally.** `workspace status`'s top-level key
/// set, against literals.
///
/// The per-member row's key set is already pinned in `federation::query`, but the
/// criterion S-326 has to meet is that the degraded roll-up "composes with the
/// warm roll-up in ONE coherent payload, not two competing member tables" — and
/// nothing was asserting the *payload's* shape. Adding a second per-member array
/// beside `members` is exactly what that forbids, and it would have left every
/// other test green.
#[test]
fn the_workspace_status_payload_has_exactly_one_member_table() {
    let tmp = warm_fixture(&["api", "web"], &["api", "web"]);
    let status = logos_json(tmp.path(), &["workspace", "status"]);

    let mut keys: Vec<&str> = status
        .as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "coverage",
            "degraded_rollup",
            "members",
            "topics",
            "warm_rollup",
            "workspace"
        ],
        "one `members` table with two roll-ups projected FROM it — a second \
         per-member array here is the competing table FR-WS-16 forbids: {status}"
    );
    // And both roll-ups partition the same roster, so they cannot describe
    // different member sets.
    assert_eq!(status["warm_rollup"]["members"], status["degraded_rollup"]["members"]);
    assert_eq!(status["warm_rollup"]["members"], 2);
}

/// `--quiet` suppresses the human notice but **not** the exit code — the
/// documented contract of the flag ("Suppress non-essential output; exit codes
/// and --json still apply"). A script that silences the warning still fails.
#[test]
fn quiet_suppresses_the_degraded_notice_but_not_the_exit_code() {
    let tmp = warm_fixture(&["api", "broken"], &["api", "broken"]);
    break_store(tmp.path(), "broken");

    let out = logos(tmp.path(), &["--quiet", "workspace", "status"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "--quiet silences output, never the verdict"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        !stderr.contains("could not be opened"),
        "the notice is non-essential output and is suppressed: {stderr}"
    );
}

/// **The scope decision, pinned.** The `xservice` group keeps exit **0** even
/// over a workspace with an unopenable member: [CR-100] §4.4 and S-326 both
/// scope the exit-code change to the `workspace` group, and `run_xservice` does
/// not consult the open-state ledger at all.
///
/// Asserted so the boundary cannot move silently in either direction — the
/// engineer flagged widening it as a call for the sprint review, and a flag with
/// no test is a flag a later change walks past.
///
/// [CR-100]: ../../docs/requests/CR-100-workspace-resource-budget.md
#[test]
fn the_xservice_group_keeps_exit_zero_over_a_degraded_workspace() {
    let tmp = warm_fixture(&["api", "broken"], &["api", "broken"]);
    break_store(tmp.path(), "broken");

    // Same fixture, same broken member: `workspace status` exits 1 …
    assert_eq!(
        logos(tmp.path(), &["workspace", "status", "--json"]).status.code(),
        Some(1),
        "the workspace group gates on it"
    );
    // … and every `xservice` subcommand still exits 0.
    for args in [
        &["xservice", "search", "f", "--json"][..],
        &["xservice", "route-providers", "--json"][..],
        &["xservice", "callers", "f", "--json"][..],
    ] {
        let out = logos(tmp.path(), args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?} is outside CR-100 §4.4's scope and must not gate: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// **[FR-WS-16] AC1 across the whole `workspace` group.** `reachability` and
/// `check` exit non-zero on an unopenable member too, and name it on stderr —
/// the exit code is a property of the workspace, not of one subcommand.
///
/// `check` is the pointed case: its *governance* verdict is advisory and never
/// moves the exit code ([ADR-56]), and its payload is a bare `Option` that must
/// keep serialising as `null` with no rules declared ([NFR-CC-04]). Both hold
/// while the degraded exit applies.
#[test]
fn every_workspace_subcommand_exits_non_zero_on_an_unopenable_member() {
    let tmp = warm_fixture(&["api", "broken"], &["api", "broken"]);
    break_store(tmp.path(), "broken");

    for args in [
        &["workspace", "status"][..],
        &["workspace", "reachability"][..],
        &["workspace", "check"][..],
    ] {
        for mode in [&[][..], &["--json"][..]] {
            let full: Vec<&str> = args.iter().chain(mode.iter()).copied().collect();
            let out = logos(tmp.path(), &full);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{full:?} exited {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
            let stderr = String::from_utf8(out.stderr).unwrap();
            assert!(
                stderr.contains("broken"),
                "{full:?} names the degraded member on stderr: {stderr}"
            );
            // **[FR-WS-16] AC3 for `check` and `reachability`.** Neither payload
            // carries a member table, so stderr is their ONLY degraded channel —
            // naming members without their cause would leave them exiting 1 with
            // no diagnosis, which is the misdiagnosis this story removes made
            // silent instead of merely misleading.
            assert!(
                stderr.contains("regular file") && stderr.contains("clear that path"),
                "{full:?} states the CAUSE on stderr, not just the name: {stderr}"
            );
        }
    }

    // `check` with no rules declared still emits the honest empty payload —
    // `null`, not a fabricated zero-violation report — on machine-clean stdout.
    let out = logos(tmp.path(), &["workspace", "check", "--json"]);
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "null",
        "the non-zero exit did not turn the honest empty into a report"
    );
}

/// **[FR-WS-16] AC4 / [BR-45], end to end.** A member the answer never opened
/// because nothing needed it is `not-attempted`, and the command exits **0**.
///
/// `xservice search --repo api` touches exactly one member of a two-member
/// workspace ([NFR-PE-10]), so `web` is never attempted — and the `xservice`
/// group does not consult the open-state ledger at all, which is what this half
/// pins (see `the_xservice_group_keeps_exit_zero_over_a_degraded_workspace` for
/// the boundary itself).
///
/// The `not-attempted`-is-never-degraded derivation is asserted where it is
/// actually produced: `federation::registry`'s
/// `a_lazily_skipped_member_reads_not_attempted_and_is_never_degraded`. The
/// second half below is the real end-to-end claim — the same healthy workspace
/// keeps exit 0 with every member `opened`.
#[test]
fn a_lazily_skipped_member_never_makes_a_healthy_command_fail() {
    let tmp = workspace();

    let out = logos(tmp.path(), &["xservice", "search", "user", "--repo", "api", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a deliberately scoped query is not a degraded workspace: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // And the unscoped all-member commands over the same healthy workspace stay
    // at exit 0 with every member `opened`.
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    assert_eq!(status["degraded_rollup"]["opened"], 2);
    assert_eq!(status["degraded_rollup"]["covers_all_members"], true);
}

/// AC1: `xservice route-providers` returns repo-qualified cross-service
/// bindings, and `--repo` scopes to routes a single member provides.
#[test]
fn route_providers_are_repo_qualified_and_repo_scopes() {
    let tmp = workspace();

    let all = logos_json(tmp.path(), &["xservice", "route-providers"]);
    let providers = all["providers"].as_array().expect("providers array");
    assert_eq!(providers.len(), 1, "one resolved cross-service route binding: {providers:?}");
    let edge = &providers[0];
    assert_eq!(edge["relation"], "route");
    assert_eq!(edge["from"]["member"], "api", "the consumer endpoint is repo-qualified");
    assert_eq!(edge["to"]["member"], "web", "the provider endpoint is repo-qualified");
    // CR-083: the intake discriminator rides the real `route-providers` command
    // envelope (additive field, backward-compatible). This fixture's binding is an
    // OpenAPI operation → framework route — a contract-surface edge, not an
    // invocation — proving the discriminator is correct end-to-end on the wire.
    assert_eq!(
        edge["intake"], "contract-surface",
        "an OpenAPI operation → route surfaces as a contract-surface edge"
    );

    // `--repo web`: routes provided BY web → the one edge.
    let scoped_web = logos_json(tmp.path(), &["xservice", "route-providers", "--repo", "web"]);
    assert_eq!(scoped_web["scope"], "web");
    assert_eq!(scoped_web["providers"].as_array().unwrap().len(), 1);

    // `--repo api`: api provides no routes (only consumes) → empty.
    let scoped_api = logos_json(tmp.path(), &["xservice", "route-providers", "--repo", "api"]);
    assert_eq!(
        scoped_api["providers"].as_array().unwrap().len(),
        0,
        "api provides no routes, so scoping to it yields no providers"
    );
}

/// AC1: `xservice search` fans across members repo-qualified, and `--repo`
/// scopes the fan-out to one member.
#[test]
fn search_fans_repo_qualified_and_repo_scopes() {
    let tmp = workspace();

    let all = logos_json(tmp.path(), &["xservice", "search", "get_user"]);
    let members = all["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2, "search fans across both members");
    let web = members
        .iter()
        .find(|m| m["member"] == "web")
        .expect("web member present");
    assert!(
        !web["result"]["hits"].as_array().unwrap().is_empty(),
        "the get_user handler is found in web: {web}"
    );

    // `--repo web` scopes the fan-out to the one member.
    let scoped = logos_json(tmp.path(), &["xservice", "search", "get_user", "--repo", "web"]);
    assert_eq!(scoped["scope"], "web");
    let scoped_members = scoped["members"].as_array().unwrap();
    assert_eq!(scoped_members.len(), 1, "scoped to exactly one member");
    assert_eq!(scoped_members[0]["member"], "web");
}

/// AC1: `xservice callers` fans intra-repo callers across members and surfaces
/// the cross-service consumers that reach a provider symbol over a bridge edge;
/// `--repo` scopes the intra-repo fan-out.
#[test]
fn callers_lists_cross_service_consumers_and_repo_scopes() {
    let tmp = workspace();

    // Resolve the web route provider symbol via search.
    let hits = logos_json(
        tmp.path(),
        &["xservice", "search", "users", "--kind", "route", "--repo", "web"],
    );
    let route_symbol = hits["members"][0]["result"]["hits"][0]["symbol"]
        .as_str()
        .expect("the axum route node is indexed in web")
        .to_string();

    let callers = logos_json(tmp.path(), &["xservice", "callers", &route_symbol]);
    let members = callers["members"].as_array().expect("members array");
    assert_eq!(members.len(), 2, "callers fans across both members");

    // The cross-service consumer of the web-provided route is the api operation.
    let cross = callers["cross_service"].as_array().expect("cross_service array");
    assert_eq!(
        cross.len(),
        1,
        "the provider route has exactly one cross-service consumer: {cross:?}"
    );
    assert_eq!(
        cross[0]["from"]["member"], "api",
        "the cross-service caller is the consumer endpoint in api"
    );
    assert_eq!(cross[0]["to"]["member"], "web", "reaching the provider in web");

    // `--repo web` scopes the intra-repo fan-out to one member.
    let scoped = logos_json(
        tmp.path(),
        &["xservice", "callers", &route_symbol, "--repo", "web"],
    );
    assert_eq!(scoped["scope"], "web");
    assert_eq!(scoped["members"].as_array().unwrap().len(), 1);
}

/// AC1 (degrade-don't-abort): an unknown `--repo` surfaces as a single
/// per-member error, exit 0, machine-clean JSON — never a panic.
#[test]
fn unknown_repo_surfaces_a_per_member_error() {
    let tmp = workspace();
    let out = logos_json(tmp.path(), &["xservice", "search", "get_user", "--repo", "nope"]);
    let members = out["members"].as_array().expect("members array");
    assert_eq!(members.len(), 1, "an unknown repo yields exactly one member entry");
    assert_eq!(members[0]["member"], "nope");
    assert!(
        members[0]["error"].as_str().is_some(),
        "the unknown member surfaces an error channel: {}",
        members[0]
    );
    assert!(
        members[0].get("result").is_none(),
        "no result for the unknown member"
    );
}

/// AC1: `xservice impact` stitches per-member impact across the bridge edges —
/// impacting the provider route surfaces its cross-service consumer in `api`.
#[test]
fn impact_stitches_across_bridge_edges() {
    let tmp = workspace();

    // Find the web route node's canonical symbol via search.
    let hits = logos_json(
        tmp.path(),
        &["xservice", "search", "users", "--kind", "route", "--repo", "web"],
    );
    let route_symbol = hits["members"][0]["result"]["hits"][0]["symbol"]
        .as_str()
        .expect("the axum route node is indexed in web")
        .to_string();

    let impact = logos_json(tmp.path(), &["xservice", "impact", &route_symbol, "--repo", "web"]);
    assert_eq!(impact["scope"], "web");
    assert!(impact["seed"].as_array().is_some(), "seed impact is present");

    let cross = impact["cross_service"].as_array().expect("cross_service array");
    assert_eq!(
        cross.len(),
        1,
        "the provider route is reached by its one cross-service consumer edge: {cross:?}"
    );
    assert_eq!(
        cross[0]["member"], "api",
        "the far-side impact is the consumer in api, stitched across the bridge edge"
    );
    assert_eq!(cross[0]["via"]["to"]["member"], "web");
    assert_eq!(cross[0]["via"]["from"]["member"], "api");
}

/// S-257 acceptance through the real binary: `workspace reachability --all` emits
/// the app-wide union view — explicitly labeled advisory, with a coverage rider on
/// every claim, and a dead set that never exceeds what each repo already called
/// dead ([FR-WS-12], [ADR-56]). `--all` is required for the full dead set: the
/// default is bounded to promotions only (S-294, [CR-084], exercised below).
#[test]
fn workspace_reachability_is_labeled_advisory_and_riders_every_claim() {
    let tmp = workspace();
    let view = logos_json(tmp.path(), &["workspace", "reachability", "--all"]);

    assert_eq!(view["view"], "cross-service-union", "the view is explicitly labeled");
    assert_eq!(view["advisory"], true, "never a gate input (ADR-56)");

    // Every applied bound is stated: `--all` unscoped ⇒ the full dead set, no member
    // filter — so this response IS the complete dead-set and says so ([NFR-CC-04]).
    assert_eq!(view["scope"]["promotions_only"], false, "--all populates the dead set");
    assert!(view["scope"]["repo"].is_null(), "no member scope applied");

    // The rider the whole view rests on — the same coverage `workspace status`
    // reports, so a reachability claim can never be read without it.
    // Pinned to exactly the numbers `workspace status` reports for the same
    // fixture — so a field-swap in `CoverageRider::new` (e.g. `unbound:
    // coverage.ambiguous`) cannot pass. Without non-trivial values here, four of
    // the five copied fields would be asserted only as zero-vs-zero.
    let rider = &view["coverage"];
    assert_eq!(rider["bound"], 1, "the GET operation bound its cross-member route");
    assert_eq!(
        rider["no_provider_in_workspace"], 1,
        "DELETE has no provider — the same bucket `workspace status` reports"
    );
    assert_eq!(rider["ambiguous"], 0);
    assert_eq!(rider["unbound"], 0);
    assert_eq!(rider["bound_ratio"], 1.0);
    assert_eq!(rider["members_read"], 2);
    assert_eq!(rider["members_total"], 2);
    assert_eq!(view["skipped_members"].as_array().unwrap().len(), 0);

    // `orphan` is dead in web's own graph and no cross-service edge reaches it,
    // so it is dead app-wide too — and its claim carries the rider verbatim.
    let dead = view["dead"].as_array().expect("dead array");
    let orphan = dead
        .iter()
        .find(|c| c["name"] == "orphan")
        .unwrap_or_else(|| panic!("web's unreferenced `orphan` is claimed dead app-wide: {dead:?}"));
    assert_eq!(orphan["member"], "web");
    assert_eq!(orphan["verdict"], "dead");
    assert_eq!(&orphan["coverage"], rider, "every claim carries the coverage rider");

    // Monotone toward live: no claim invents deadness — the per-member tallies
    // account for every per-repo dead callable exactly once.
    for tally in view["members"].as_array().expect("members array") {
        let per_repo = tally["dead_per_repo"].as_u64().unwrap();
        let promoted = tally["live_via_cross_service"].as_u64().unwrap();
        let still_dead = tally["dead_app_wide"].as_u64().unwrap();
        assert_eq!(
            per_repo,
            promoted + still_dead,
            "member {} loses or invents a dead callable: {tally}",
            tally["member"]
        );
    }
}

/// CR-084 default through the real binary: `workspace reachability` (no `--all`) is
/// bounded to the cross-service promotions and STATES `promotions_only`, with the
/// per-repo-dead set suppressed to `null` — never `[]` — so a reader can never
/// mistake the bounded reply for a complete, empty dead-set ([NFR-CC-04]). The view
/// stays labeled, advisory, and carries the same coverage rider.
#[test]
fn workspace_reachability_default_is_promotions_only_and_states_the_bound() {
    let tmp = workspace();
    let view = logos_json(tmp.path(), &["workspace", "reachability"]);

    assert_eq!(view["view"], "cross-service-union", "still the labeled union view");
    assert_eq!(view["advisory"], true, "still advisory (ADR-56)");
    assert_eq!(view["scope"]["promotions_only"], true, "the default states its bound");
    assert!(view["scope"]["repo"].is_null(), "no member scope by default");
    assert!(
        view["dead"].is_null(),
        "the per-repo-dead set is SUPPRESSED to null, not emitted as [] that would read \
         as a complete, empty dead-set (NFR-CC-04): {}",
        view["dead"]
    );
    // The promotions are always carried (empty on this contract-surface-only fixture
    // per CR-083 — but present as an array, never suppressed).
    assert!(
        view["live_via_cross_service"].as_array().is_some(),
        "the promotions bucket is always carried: {}",
        view["live_via_cross_service"]
    );
    // The coverage rider still rides the bounded payload — same numbers as `--all`.
    assert_eq!(view["coverage"]["bound"], 1, "the GET operation bound its route");
    assert_eq!(view["coverage"]["members_read"], 2);
}

/// CR-084 `--repo` through the real binary: a member scope restricts every bucket to
/// that member and states the scope. `web` owns the dead `orphan`; scoping to `api`
/// (which has no per-repo dead callable) filters it out — a filter, never a cap.
#[test]
fn workspace_reachability_repo_scopes_to_one_member() {
    let tmp = workspace();

    let web = logos_json(tmp.path(), &["workspace", "reachability", "--all", "--repo", "web"]);
    assert_eq!(web["scope"]["repo"], "web", "the member scope is stated");
    for member in web["members"].as_array().expect("members array") {
        assert_eq!(member["member"], "web", "only the scoped member's tally is carried");
    }
    let web_dead = web["dead"].as_array().expect("dead array under --all");
    assert!(
        web_dead.iter().any(|c| c["name"] == "orphan" && c["member"] == "web"),
        "web's dead `orphan` is in scope: {web_dead:?}"
    );

    let api = logos_json(tmp.path(), &["workspace", "reachability", "--all", "--repo", "api"]);
    assert_eq!(api["scope"]["repo"], "api");
    let api_dead = api["dead"].as_array().expect("dead array under --all");
    assert!(
        !api_dead.iter().any(|c| c["name"] == "orphan"),
        "orphan belongs to web, so an api scope filters it out: {api_dead:?}"
    );
}

// ── Workspace governance over cross-service bindings (S-258, FR-WS-13) ──────
//
// The fixture's one bridge binding is `api` (the OpenAPI consumer) → `web` (the
// axum route provider), so a rule forbidding calls from `api`'s layer into
// `web`'s layer is breached by exactly that binding.

/// The `[governance]` section declaring `edge` (api) → `core` (web) forbidden,
/// appended to the workspace manifest the `workspace()` fixture wrote.
const GOVERNANCE: &str = "
[[governance.service_layers]]
name = \"edge\"
members = [\"api\"]

[[governance.service_layers]]
name = \"core\"
members = [\"web\"]

[[governance.boundaries]]
from = \"edge\"
to = \"core\"
reason = \"edge services must not call core services directly\"
";

/// Append the `[governance]` rule family to an existing workspace manifest.
fn declare_rules(root: &Path, rules: &str) {
    let manifest = root.join("logos.workspace.toml");
    let existing = std::fs::read_to_string(&manifest).expect("the fixture wrote a manifest");
    std::fs::write(&manifest, format!("{existing}{rules}")).expect("append governance");
}

/// AC (honest empty): with NO `[governance]` declared, `workspace check` produces
/// no governance output at all — `null`, not a zero-violation report. An
/// undeclared policy must never read as a *passing* one ([NFR-CC-04]).
#[test]
fn workspace_check_with_no_rules_produces_no_output() {
    let tmp = workspace();
    let report = logos_json(tmp.path(), &["workspace", "check"]);
    assert!(
        report.is_null(),
        "no declared rules ⇒ no workspace governance output: {report}"
    );
}

/// AC1: a workspace rule referencing service layers evaluates over the BRIDGE
/// bindings and reports the violation at the workspace level ([FR-WS-13]).
#[test]
fn a_service_layer_rule_reports_a_violating_bridge_binding() {
    let tmp = workspace();
    declare_rules(tmp.path(), GOVERNANCE);

    let report = logos_json(tmp.path(), &["workspace", "check"]);
    assert_eq!(report["workspace"], "shop");
    assert_eq!(report["rules_checked"], 1);
    assert_eq!(
        report["bindings_checked"], 1,
        "the rules quantified over the one matched bridge binding"
    );

    let violations = report["violations"].as_array().expect("violations array");
    assert_eq!(violations.len(), 1, "the edge→core binding breaches the rule: {violations:?}");
    let v = &violations[0];
    assert_eq!(v["rule"], "workspace-boundary:edge->core");
    assert_eq!(v["rule_type"], "workspace-boundary");
    assert_eq!(v["severity"], "error");
    // The endpoints are the real bridge binding, repo-qualified — not fabricated.
    assert_eq!(v["from"]["member"], "api", "the consumer side of the binding");
    assert_eq!(v["to"]["member"], "web", "the provider side of the binding");
    assert_eq!(v["relation"], "route");
    assert!(
        v["message"].as_str().unwrap().contains("edge services must not call core"),
        "the declared reason is surfaced: {}",
        v["message"]
    );
}

/// The workspace rule family is ADVISORY: a violation is *reported*, and the
/// command still exits 0 — it is not a gate ([ADR-56]).
#[test]
fn a_workspace_violation_is_advisory_and_exits_zero() {
    let tmp = workspace();
    declare_rules(tmp.path(), GOVERNANCE);

    let out = logos(tmp.path(), &["workspace", "check", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a workspace-rule violation is reported, never gated: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A per-repo contract for a member, with a constraint that is GUARANTEED to fire:
/// every function has cyclomatic complexity >= 1, so `max_cc = 0` always yields
/// violations. This gives the member a real, *failing* gated signal — the thing
/// the workspace tier must not be able to move.
const MEMBER_RULES: &str = "[constraints]\nmax_cc = 0\n";

/// AC2 — the load-bearing CR-061 invariant: workspace-rule violations are
/// reported SEPARATELY from the per-repo gate, and a member's gated signal is
/// **unchanged** by their existence.
///
/// The member under test is deliberately given a real `.logos/rules.toml` that
/// genuinely FAILS (`max_cc = 0`). Without it this test would be near-vacuous: the
/// `workspace()` fixture only runs `logos index` (never `logos init`), so a member
/// has no contract at all and `check` would return an empty, contract-less report
/// — and "two empty reports are equal" proves nothing. Here the member carries a
/// loaded contract with real violations and a real exit-1 verdict, and *that* is
/// what must survive the workspace rules byte-for-byte ([FR-WS-13], [ADR-56]).
#[test]
fn declaring_workspace_rules_leaves_the_member_gate_byte_identical() {
    let tmp = workspace();
    let web = tmp.path().join("web");
    write(&web, ".logos/rules.toml", MEMBER_RULES);

    let before = logos(&web, &["check", "--json"]);
    declare_rules(tmp.path(), GOVERNANCE);
    let after = logos(&web, &["check", "--json"]);

    // Anti-vacuity: the member must have evaluated a REAL contract that REALLY
    // fails — otherwise the byte-equality below is a comparison of two nothings.
    let report: Value = serde_json::from_slice(&before.stdout)
        .expect("the member's `check` emits a RulesReport");
    assert_eq!(
        report["rules_present"], true,
        "the member loaded its own rules.toml: {report}"
    );
    assert!(
        report["violations"].as_array().is_some_and(|v| !v.is_empty()),
        "the member's contract genuinely fires (max_cc = 0): {report}",
    );
    assert_eq!(report["passed"], false, "so its gated verdict is a real FAIL");
    assert_eq!(
        before.status.code(),
        Some(1),
        "and the per-repo gate exits 1 (FR-GV-03)",
    );

    // The invariant: that real, failing gated signal is untouched.
    assert_eq!(
        before.status.code(),
        after.status.code(),
        "the member's per-repo exit code is untouched by a workspace rule",
    );
    assert_eq!(
        String::from_utf8_lossy(&before.stdout),
        String::from_utf8_lossy(&after.stdout),
        "the member's gated signal is byte-for-byte unchanged (CR-061 invariant)",
    );

    // ...and the workspace tier DID fire, so the equality above is a real
    // separation, not both tiers being silent.
    let workspace_report = logos_json(tmp.path(), &["workspace", "check"]);
    assert_eq!(
        workspace_report["violations"].as_array().map(Vec::len),
        Some(1),
        "the workspace rule genuinely fired while the member gate stayed put",
    );
    // The two families never share a vocabulary: no per-repo violation is tagged
    // with a workspace rule_type, and vice versa.
    for v in report["violations"].as_array().expect("member violations") {
        assert!(
            !v["rule_type"]
                .as_str()
                .unwrap_or_default()
                .starts_with("workspace-"),
            "no workspace rule leaked into the member's per-repo report: {v}",
        );
    }
}

/// AC3: a "no cross-service callers" rule reads the BRIDGE — it names the real
/// consumer that binds the provider, never a fabricated caller set ([NFR-RA-05]).
#[test]
fn a_no_cross_service_callers_rule_reads_the_bridge() {
    let tmp = workspace();
    // The axum provider route is `GET /users/{id}` in member `web`; its symbol
    // carries the enclosing `get_user` handler name.
    declare_rules(
        tmp.path(),
        "
[[governance.no_cross_service_callers]]
member = \"web\"
symbol = \"*users*\"
reason = \"deprecated in v3\"
",
    );

    let report = logos_json(tmp.path(), &["workspace", "check"]);
    let violations = report["violations"].as_array().expect("violations array");
    assert_eq!(
        violations.len(),
        1,
        "the deprecated provider has exactly one cross-service caller: {violations:?}"
    );
    let v = &violations[0];
    assert_eq!(v["rule"], "no-cross-service-callers:*users*");
    assert_eq!(v["rule_type"], "workspace-no-cross-service-callers");
    assert_eq!(
        v["from"]["member"], "api",
        "the caller is read off the bridge binding, not synthesised"
    );
    assert_eq!(v["to"]["member"], "web");
    assert!(
        v["message"].as_str().unwrap().contains("deprecated in v3"),
        "the declared reason is surfaced: {}",
        v["message"]
    );
}

/// An out-of-range `[workspace.warm] concurrency` fails LOUD on a command that
/// has nothing to do with warming (S-322, FR-WS-01, [ADR-14]).
///
/// The rejection lives in `manifest::parse`, which `federation::discover` calls
/// on the path of *every* command — so proving it only through
/// `init --workspace` (where the key is actually used) would leave the far more
/// important half unstated: an operator who mistypes the bound is told so by
/// whatever they run next, not silently given a degraded workspace. This is the
/// same fail-loud posture the malformed-rule test below pins.
#[test]
fn an_out_of_range_warm_concurrency_fails_loud_on_an_unrelated_command() {
    let tmp = workspace();
    declare_rules(tmp.path(), "\n[workspace.warm]\nconcurrency = 0\n");

    let out = logos(tmp.path(), &["workspace", "check", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "a bad manifest value is a config fault on every command, not just the \
         one that reads it"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("workspace.warm.concurrency"),
        "and it names the offending key: {stderr}"
    );
}

/// A malformed rule fails LOUD (exit 2, the config-error code) rather than
/// silently matching nothing — a governance rule that quietly never fires would
/// report a false all-clear ([ADR-14]).
#[test]
fn a_malformed_workspace_rule_fails_loud() {
    let tmp = workspace();
    declare_rules(
        tmp.path(),
        "
[[governance.no_cross_service_callers]]
symbol = \"[unclosed\"
",
    );

    let out = logos(tmp.path(), &["workspace", "check", "--json"]);
    assert!(
        !out.status.success(),
        "an uncompilable rule glob must not report a clean workspace",
    );
}
