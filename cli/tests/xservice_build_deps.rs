//! End-to-end tests for `logos xservice build-deps` and the `build_dependency`
//! section of `logos workspace status` (S-464, [FR-WS-05], [FR-WS-33]), driven
//! through the **real** `logos` binary over members indexed with Maven poms.
//!
//! The fixture is the reference estate's shape in miniature: a parent POM
//! (`starter`) every member inherits, two bounded contexts' model libraries
//! (repositories `archive-kafka-models` and `mailbox-kafka-models`, producing
//! `com.acme.archive:kafka-models` and `com.acme.mailbox:kafka-models` — the
//! estate's only coordinate shape), and an `adapter` that
//! depends on both — the cross-context hint's case. Beside it, `web` serves a
//! route `api`'s OpenAPI document declares, so the runtime figures the build
//! relation must never move are non-trivial ([BR-58]).
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
//! [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
";

const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{id}", get(get_user))
}
"#;

const G: &str = "com.acme";

/// A model library's coordinate in the reference estate's only shape:
/// `<group>.<context>:kafka-models`, produced by a repository named
/// `<context>-kafka-models`.
fn model_group(member: &str) -> Option<String> {
    member.strip_suffix("-kafka-models").map(|context| format!("{G}.{context}"))
}

/// `groupId`, `artifactId` for what `member` produces.
fn coordinate(member: &str) -> (String, String) {
    model_group(member).map_or((G.to_string(), member.to_string()), |g| (g, "kafka-models".to_string()))
}

/// A pom producing `artifact`'s coordinate, inheriting `starter` unless it is the
/// starter, with `(member, scope)` dependencies on what those members produce —
/// `None` leaves the scope undeclared.
fn pom(artifact: &str, dependencies: &[(&str, Option<&str>)]) -> String {
    let parent = if artifact == "starter" {
        String::new()
    } else {
        format!("  <parent><groupId>{G}</groupId><artifactId>starter</artifactId><version>1</version></parent>\n")
    };
    let deps: String = dependencies
        .iter()
        .map(|(member, scope)| {
            let (g, a) = coordinate(member);
            let scope = scope.map_or(String::new(), |s| format!("<scope>{s}</scope>"));
            format!("    <dependency><groupId>{g}</groupId><artifactId>{a}</artifactId>{scope}</dependency>\n")
        })
        .collect();
    let (group, artifact) = coordinate(artifact);
    format!(
        "<project>\n{parent}  <groupId>{group}</groupId>\n  <artifactId>{artifact}</artifactId>\n  \
         <version>1</version>\n  <dependencies>\n{deps}  </dependencies>\n</project>\n"
    )
}

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

/// `logos <args> --json`, asserting exit 0 and one machine-clean JSON line.
fn logos_json(project: &Path, args: &[&str]) -> Value {
    let mut full = args.to_vec();
    full.push("--json");
    let out = logos(project, &full);
    assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim().lines().count(), 1, "--json is one line: {stdout}");
    serde_json::from_str(stdout.trim()).expect("--json stdout is JSON")
}

/// The human rendering: the same read-model, pretty-printed ([FR-CL-02]).
///
/// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
fn logos_human(project: &Path, args: &[&str]) -> (String, Value) {
    let out = logos(project, args);
    assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let value = serde_json::from_str(&stdout).expect("the human rendering is the read-model");
    (stdout, value)
}

const MEMBERS: [&str; 6] = ["starter", "archive-kafka-models", "mailbox-kafka-models", "adapter", "api", "web"];

/// The six members, indexed through the real binary; `with_poms = false`
/// writes every member's source and no manifest, so the two workspaces differ
/// only in their build manifests.
fn workspace(with_poms: bool) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for name in MEMBERS {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        sh_git(&dir, &["init", "-q", "-b", "main"]);
        match name {
            "api" => write(&dir, "api/openapi.yaml", OPENAPI_YAML),
            "web" => write(&dir, "src/main.rs", AXUM_MAIN),
            _ => write(&dir, "src/lib.rs", "pub fn f() {}\n"),
        }
        if with_poms {
            let deps: &[(&str, Option<&str>)] = match name {
                "adapter" => &[("archive-kafka-models", None), ("mailbox-kafka-models", Some("test"))],
                "web" => &[("commons-lang3", None)],
                _ => &[],
            };
            if name != "api" {
                write(&dir, "pom.xml", &pom(name, deps));
            }
        }
        sh_git(&dir, &["add", "."]);
        sh_git(&dir, &["commit", "-q", "-m", "init"]);
        assert!(logos(&dir, &["index"]).status.success(), "index {name}");
    }
    let members = MEMBERS.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ");
    std::fs::write(
        root.join("logos.workspace.toml"),
        format!("[workspace]\nname = \"acme\"\nmembers = [{members}]\ndefault = \"api\"\n"),
    )
    .unwrap();
    tmp
}

fn member<'a>(payload: &'a Value, name: &str) -> &'a Value {
    payload["members"]
        .as_array()
        .expect("members array")
        .iter()
        .find(|m| m["member"] == name)
        .unwrap_or_else(|| panic!("member {name} in {payload}"))
}

/// `(to|from, kind, scope, artifact)` for a member's rows in one direction.
fn rows(member: &Value, direction: &str, other: &str) -> Vec<(String, String, Value, String)> {
    member[direction]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| {
            (
                r[other].as_str().unwrap().to_string(),
                r["kind"].as_str().unwrap().to_string(),
                r["scope"].clone(),
                r["artifact"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// AC1: per member, `builds_against` and `built_against_by` rows each naming
/// kind, scope and artifact; `--repo` scopes to one member; an unknown member
/// is named, never an empty "no edges"; the human rendering is the same
/// read-model; and the cross-context hint names the adapter's libraries.
#[test]
fn build_deps_lists_rows_naming_kind_scope_and_artifact_and_repo_scopes_to_one_member() {
    let tmp = workspace(true);
    let all = logos_json(tmp.path(), &["xservice", "build-deps"]);

    assert_eq!(all["members"].as_array().unwrap().len(), MEMBERS.len(), "every member read: {all}");
    assert!(all.get("scope").is_none() && all.get("scope_note").is_none());
    assert_eq!(
        rows(member(&all, "adapter"), "builds_against", "to"),
        vec![
            ("archive-kafka-models".to_string(), "dependency".to_string(), Value::Null, format!("{G}.archive:kafka-models")),
            ("mailbox-kafka-models".to_string(), "dependency".to_string(), Value::from("test"), format!("{G}.mailbox:kafka-models")),
            ("starter".to_string(), "parent".to_string(), Value::Null, format!("{G}:starter")),
        ],
        "an undeclared scope is null, never defaulted to compile",
    );
    let starter_in = rows(member(&all, "starter"), "built_against_by", "from");
    let from: Vec<&str> = starter_in.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(from, ["adapter", "archive-kafka-models", "mailbox-kafka-models", "web"]);
    assert!(member(&all, "api")["builds_against"].as_array().unwrap().is_empty(), "no pom, no row");

    assert_eq!(
        all["cross_context"],
        serde_json::json!([{
            "member": "adapter",
            "contexts": ["archive", "mailbox"],
            "libraries": [
                { "context": "archive", "artifact": format!("{G}.archive:kafka-models"), "member": "archive-kafka-models" },
                { "context": "mailbox", "artifact": format!("{G}.mailbox:kafka-models"), "member": "mailbox-kafka-models" },
            ],
        }]),
    );
    assert!(
        all["headline"]["summary"].as_str().unwrap().contains("never a runtime coupling"),
        "{}",
        all["headline"]["summary"]
    );

    let one = logos_json(tmp.path(), &["xservice", "build-deps", "--repo", "archive-kafka-models"]);
    assert_eq!(one["scope"], "archive-kafka-models");
    assert_eq!(one["members"].as_array().unwrap().len(), 1);
    assert_eq!(
        rows(&one["members"][0], "built_against_by", "from"),
        vec![("adapter".to_string(), "dependency".to_string(), Value::Null, format!("{G}.archive:kafka-models"))],
    );
    assert_eq!(one["headline"], all["headline"], "the denominator stays workspace-wide under a scope");
    assert_eq!(one["cross_context"], serde_json::json!([]), "the hint is scoped with the rows");

    let unknown = logos_json(tmp.path(), &["xservice", "build-deps", "--repo", "nope"]);
    assert_eq!(unknown["members"], serde_json::json!([]));
    let note = unknown["scope_note"].as_str().unwrap();
    assert!(
        note.starts_with("`nope` is not a member the build relation was read over"),
        "an empty list is never presented as \"no build dependencies\": {note}"
    );

    let (_, human) = logos_human(tmp.path(), &["xservice", "build-deps"]);
    assert_eq!(human, all, "human and --json print one read-model");
    // The snake-case alias reaches the same command.
    assert_eq!(logos_json(tmp.path(), &["xservice", "build_deps"]), all);
}

/// AC2 ([BR-51], [BR-58]): `workspace status`, human and `--json`, carries
/// `build_dependency_pairs` by kind beside its denominator, as its own
/// top-level section — and every runtime figure is byte-identical to the same
/// workspace with no manifest.
///
/// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn workspace_status_states_build_pairs_by_kind_beside_the_denominator_apart_from_runtime() {
    let with = workspace(true);
    let status = logos_json(with.path(), &["workspace", "status"]);
    let section = &status["build_dependency"];
    assert_eq!(
        section["build_dependency_pairs"],
        serde_json::json!({ "pairs": 6, "parent": 4, "dependency": 2, "managed": 0, "bom-import": 0 }),
        "{section}",
    );
    let references = &section["references"];
    assert_eq!(references["references"], 7, "every referenced fact, commons-lang3 included: {references}");
    assert_eq!(references["to_member"], 6);
    assert_eq!(references["external"], 1);
    let summary = section["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("6 pairs (parent 4 · dependency 2 · managed 0 · bom-import 0) built against another member, from 6 of 7 referenced artifacts"),
        "the pairs by kind beside the denominator, in one line: {summary}",
    );
    assert!(summary.contains("a build dependency, never a runtime coupling"), "{summary}");
    assert!(
        !status["coverage"].to_string().contains("build"),
        "no build figure inside the runtime coverage section: {}",
        status["coverage"]
    );

    let (text, human) = logos_human(with.path(), &["workspace", "status"]);
    assert_eq!(human["build_dependency"], *section, "human and --json carry one section");
    let (coverage_at, build_at) = (text.find("\"coverage\"").unwrap(), text.find("\"build_dependency\"").unwrap());
    assert!(coverage_at < build_at, "the build section follows the runtime ones, apart from them");

    let without = workspace(false);
    let bare = logos_json(without.path(), &["workspace", "status"]);
    for runtime in ["coverage", "topics", "warm_rollup", "degraded_rollup"] {
        assert_eq!(status[runtime], bare[runtime], "`{runtime}` moved when build manifests were present");
    }
}

/// A workspace with no build manifest prints `workspace status` exactly as
/// before the relation existed: the pre-S-463 top-level keys, no build key
/// anywhere in either rendering.
#[test]
fn a_workspace_without_build_manifests_prints_status_with_no_build_section() {
    let tmp = workspace(false);
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    let keys: Vec<&str> = status.as_object().unwrap().keys().map(String::as_str).collect();
    let mut keys = keys;
    keys.sort_unstable();
    // `kind_candidates` is S-457's hint (`api` holds an OpenAPI document and no
    // runnable source) — part of the payload before the relation existed.
    assert_eq!(
        keys,
        ["coverage", "degraded_rollup", "kind_candidates", "members", "topics", "warm_rollup", "workspace"],
    );
    let (text, _) = logos_human(tmp.path(), &["workspace", "status"]);
    assert!(!text.contains("build_dependency"), "{text}");

    let deps = logos_json(tmp.path(), &["xservice", "build-deps"]);
    assert_eq!(deps["headline"]["members"]["with_manifests"], 0, "{deps}");
    assert_eq!(deps["cross_context"], serde_json::json!([]));
}
