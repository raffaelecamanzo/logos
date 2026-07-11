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

/// S-257 acceptance through the real binary: `workspace reachability` emits the
/// app-wide union view — explicitly labeled advisory, with a coverage rider on
/// every claim, and a dead set that never exceeds what each repo already called
/// dead ([FR-WS-12], [ADR-56]).
#[test]
fn workspace_reachability_is_labeled_advisory_and_riders_every_claim() {
    let tmp = workspace();
    let view = logos_json(tmp.path(), &["workspace", "reachability"]);

    assert_eq!(view["view"], "cross-service-union", "the view is explicitly labeled");
    assert_eq!(view["advisory"], true, "never a gate input (ADR-56)");

    // The rider the whole view rests on — the same coverage `workspace status`
    // reports, so a reachability claim can never be read without it.
    let rider = &view["coverage"];
    assert_eq!(rider["bound"], 1, "the GET operation bound its cross-member route");
    assert_eq!(rider["members_read"], 2);
    assert_eq!(rider["members_total"], 2);

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
