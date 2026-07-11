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
