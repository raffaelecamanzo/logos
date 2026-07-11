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

/// An axum app registering exactly one route, `GET /users/{id}`.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

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
    let mut full = args.to_vec();
    full.push("--json");
    let out = logos(project, &full);
    assert!(
        out.status.success(),
        "logos {args:?} exited {:?}: {}",
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
