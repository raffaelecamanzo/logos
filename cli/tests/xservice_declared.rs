//! End-to-end tests for the declared relations on `logos xservice route-providers`
//! and `logos workspace status` (S-461, [FR-WS-05], [FR-WS-31], [BR-57]), driven
//! through the **real** `logos` binary over members really indexed.
//!
//! The fixture is the reference estate's shape in miniature:
//!
//! - `facade` vendors the PSS spec (`api/pss.yaml`) and implements none of it,
//!   and its one client call composes `${pec-server.uri-get-mailbox}` from
//!   committed configuration — so the call has no provider in the workspace, and
//!   the external join binds it to PSS under the `/prov` base path its deploy
//!   overlay commits (`pecserver-facade`'s 20 rows, at one row);
//! - `webmail` holds a second PSS copy (one external, two declarers — the
//!   estate's `webmail → PSS`) and a copy of `mbx`'s own spec, which document
//!   identity resolves to `mbx` (the estate's `webmail → mailbox-aggregator-api`);
//! - `mbx` serves its own spec, so it declares nothing.
//!
//! A second fixture holds no vendored spec and declares no `kind`: every surface
//! must print it exactly as it did before the relations existed.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-31]: ../../docs/specs/requirements/FR-WS-31.md
//! [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

/// The external's spec: two operations under the `/prov` base path.
const PSS_YAML: &str = "\
openapi: 3.0.3
info:
  title: PSS
  version: 1.0.0
paths:
  /prov/domain/{d}/user/{u}:
    get:
      summary: user
  /prov/session/authenticate:
    post:
      summary: auth
";

/// `mbx`'s own spec, which its axum route serves.
const MAILBOX_YAML: &str = "\
openapi: 3.0.3
info:
  title: Mailbox API
  version: 1.0.0
paths:
  /folders/{id}:
    get:
      summary: folder
";

const MBX_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_folder() {}

fn app() -> Router {
    Router::new().route("/folders/{id}", get(get_folder))
}
"#;

/// The call whose target is composed from committed configuration.
const FACADE_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_mailbox(client: Client) {
    let _ = client.get("${pec-server.uri-get-mailbox}").await;
}
"#;

const FACADE_APPLICATION: &str =
    "pec-server:\n  base-url: http://localhost:8082\n  uri-get-mailbox: /domain/{domain}/user/{user}\n";

const FACADE_OVERLAY: &str = "envFrom:\n  PECSERVER_BASEURL: 'https://pss.example/prov'\n";

/// A client that calls `mbx`'s route literally — a binding, and nothing vendored.
const WEB_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_folder(client: Client) {
    let _ = client.get("/folders/{id}").await;
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
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
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

/// Commit `files` into a fresh member repo and index it through the binary.
fn member(root: &Path, name: &str, files: &[(&str, &str)]) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    sh_git(&dir, &["init", "-q", "-b", "main"]);
    for (rel, contents) in files {
        write(&dir, rel, contents);
    }
    sh_git(&dir, &["add", "."]);
    sh_git(&dir, &["commit", "-q", "-m", "init"]);
    assert!(logos(&dir, &["index"]).status.success(), "index {name}");
}

fn manifest(root: &Path, members: &[&str]) {
    let list = members.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ");
    std::fs::write(
        root.join("logos.workspace.toml"),
        format!("[workspace]\nname = \"pec\"\nmembers = [{list}]\n"),
    )
    .unwrap();
}

/// The vendored-spec workspace (see the module docs).
fn declaring_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    member(
        root,
        "facade",
        &[
            ("api/pss.yaml", PSS_YAML),
            ("src/client.rs", FACADE_CLIENT),
            ("src/main/resources/application.yml", FACADE_APPLICATION),
            ("deploy-coll/values.yaml", FACADE_OVERLAY),
        ],
    );
    member(root, "mbx", &[("api/openapi.yaml", MAILBOX_YAML), ("src/main.rs", MBX_MAIN)]);
    member(
        root,
        "webmail",
        &[
            ("specs/PSS-API.yaml", PSS_YAML),
            ("specs/mailbox.yaml", MAILBOX_YAML),
            ("src/lib.rs", "pub fn ui() {}\n"),
        ],
    );
    manifest(root, &["facade", "mbx", "webmail"]);
    tmp
}

/// Own specs and literal calls only: nothing vendored, no `kind` declared.
fn undeclaring_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    member(root, "mbx", &[("api/openapi.yaml", MAILBOX_YAML), ("src/main.rs", MBX_MAIN)]);
    member(root, "web", &[("src/client.rs", WEB_CLIENT)]);
    manifest(root, &["mbx", "web"]);
    tmp
}

/// AC3's CLI half ([BR-57]): `route-providers` carries the declared-contract
/// relation and the external join **beside** its bindings — byte for byte the
/// relations `workspace status` publishes, so the two commands cannot state
/// two different relations — and neither adds a binding.
///
/// [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn route_providers_carry_the_declared_relations_beside_the_bindings() {
    let tmp = declaring_workspace();
    let providers = logos_json(tmp.path(), &["xservice", "route-providers"]);
    let status = logos_json(tmp.path(), &["workspace", "status"]);

    let declared = &providers["declared_contracts"];
    assert_eq!(*declared, status["coverage"]["declared_contracts"], "one relation, two commands");
    assert_eq!(providers["bound_external"], status["coverage"]["bound_external"]);

    // Not vacuous: both relation classes and the join's evidence are on the wire.
    assert_eq!(declared["headline"]["declared_contract_pairs"], 3, "{declared}");
    assert_eq!((&declared["headline"]["to_member"], &declared["headline"]["to_external"]), (&1.into(), &2.into()));
    let identity = declared["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["target"]["kind"] == "member")
        .expect("webmail's mailbox copy resolves by document identity");
    assert_eq!(
        (&identity["holder"], &identity["document"], &identity["target"]["member"]),
        (&"webmail".into(), &"specs/mailbox.yaml".into(), &"mbx".into())
    );
    assert_eq!((&identity["target"]["shared"], &identity["target"]["total"]), (&1.into(), &1.into()));
    let [pss] = declared["externals"].as_array().unwrap().as_slice() else {
        panic!("two PSS copies are one external: {declared}");
    };
    assert_eq!((&pss["id"], &pss["name"]), (&"facade:api/pss.yaml".into(), &"PSS".into()));
    assert_eq!(pss["declared_by"], serde_json::json!(["facade", "webmail"]));

    let join = &providers["bound_external"];
    let [row] = join["rows"].as_array().unwrap().as_slice() else { panic!("{join}") };
    assert_eq!(row["state"], "bound-external");
    assert_eq!(row["operation"], "GET /prov/domain/{}/user/{}");
    assert_eq!(row["base"]["path"], "/prov");
    assert_eq!(row["base"]["origin"], "deploy-overlay");
    assert_eq!(row["base"]["sources"][0]["file"], "deploy-coll/values.yaml");

    // Beside, never inside: no binding reaches or leaves the external's side.
    let edges = providers["providers"].as_array().unwrap();
    assert!(
        edges.iter().all(|e| e["from"]["member"] != "facade" && e["to"]["member"] != "facade"),
        "a bound external is never a binding: {edges:?}"
    );
    assert!(providers.get("declared_scope_note").is_none(), "unscoped: {providers}");

    let (_, human) = logos_human(tmp.path(), &["xservice", "route-providers"]);
    assert_eq!(human, providers, "human and --json print one read-model");
}

/// Under `--repo` the bindings narrow and the relations do not; the answer
/// says so, so a workspace-wide relation is never read as the member's.
#[test]
fn a_repo_scope_narrows_the_bindings_and_names_the_relations_workspace_wide() {
    let tmp = declaring_workspace();
    let all = logos_json(tmp.path(), &["xservice", "route-providers"]);
    let scoped = logos_json(tmp.path(), &["xservice", "route-providers", "--repo", "facade"]);
    assert_eq!(scoped["scope"], "facade");
    assert!(scoped["providers"].as_array().unwrap().is_empty(), "facade provides no route");
    assert_eq!(scoped["declared_contracts"], all["declared_contracts"]);
    assert_eq!(scoped["bound_external"], all["bound_external"]);
    let note = scoped["declared_scope_note"].as_str().expect("the scope states its reach");
    assert!(note.contains("`--repo facade`") && note.contains("workspace-wide"), "{note}");
}

/// AC1 ([BR-51], [BR-57]): `workspace status`, human and `--json`, states
/// `declared_contract_pairs` and `bound_external` inside `coverage` beside the
/// invocation and contract-surface headlines, each with its denominator — and
/// the bound call's own coverage row still reads `no-provider-in-workspace`.
///
/// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-57]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn workspace_status_states_both_relations_beside_the_headlines_with_their_denominators() {
    let tmp = declaring_workspace();
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    let coverage = &status["coverage"];
    for headline in ["resolved_edges_summary", "spec_conformance_summary", "by_intake"] {
        assert!(coverage.get(headline).is_some(), "the runtime headline `{headline}` is beside them");
    }

    let declared = &coverage["declared_contracts"]["headline"];
    let documents = &declared["documents"];
    assert_eq!(documents["documents"], 4, "the denominator: every spec document read: {declared}");
    let buckets: u64 = ["own", "vendored", "partial", "unjudged", "mock", "documentation"]
        .iter()
        .map(|b| documents[b].as_u64().unwrap())
        .sum();
    assert_eq!(buckets, 4, "the buckets sum to the denominator");
    let summary = declared["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("3 declared contract pairs (1 by document identity, 2 to named externals) from 3 vendored of 4 spec documents"),
        "the figure beside its denominator, in one line: {summary}"
    );
    assert!(summary.contains("never observed calls"), "{summary}");

    let bound = &coverage["bound_external"]["headline"];
    assert_eq!((&bound["bound_external"], &bound["no_provider_rows"]), (&1.into(), &1.into()));
    let summary = bound["summary"].as_str().unwrap();
    assert!(summary.starts_with("1 of 1 invocation no-provider-in-workspace REST row"), "{summary}");
    assert!(summary.contains("never a cross-service edge"), "{summary}");

    let call = coverage["references"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["intake"] == "invocation" && r["from"]["member"] == "facade")
        .expect("facade's call is a coverage row");
    assert_eq!(call["reason"], "no-provider-in-workspace", "the bound row does not move: {call}");

    let (text, human) = logos_human(tmp.path(), &["workspace", "status"]);
    assert_eq!(human["coverage"]["declared_contracts"], coverage["declared_contracts"]);
    assert_eq!(human["coverage"]["bound_external"], coverage["bound_external"]);
    assert!(text.contains("\"declared_contract_pairs\": 3") && text.contains("\"bound_external\": 1"), "{text}");
}

/// AC4: a workspace with no vendored spec and no `kind` declaration prints both
/// commands with exactly the keys they had before the relations existed — no
/// relation key, not a `null` one, in either rendering. `route-providers` has
/// since gained one key of its own, last: S-484's `member_reads`.
#[test]
fn a_workspace_without_vendored_specs_prints_both_commands_unchanged() {
    let tmp = undeclaring_workspace();

    let providers = logos_json(tmp.path(), &["xservice", "route-providers"]);
    let keys: Vec<&String> = providers.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["providers", "member_reads"], "{providers}");
    assert_eq!(providers["providers"].as_array().map(Vec::len), Some(1), "not vacuous: web binds mbx");
    let scoped = logos_json(tmp.path(), &["xservice", "route-providers", "--repo", "mbx"]);
    let keys: Vec<&String> = scoped.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["scope", "providers", "member_reads"],
        "no scope note without a relation, keys in their old order: {scoped}"
    );

    let status = logos_json(tmp.path(), &["workspace", "status"]);
    let mut keys: Vec<&str> = status["coverage"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "ambiguous",
            "bound",
            "by_intake",
            "covers_all_members",
            "egress_resolution",
            "egress_resolution_measured",
            "members_read",
            "members_total",
            "no_provider_in_workspace",
            "references",
            "resolved_cross_service_edges",
            "resolved_edges_summary",
            "spec_conformance_measured",
            "spec_conformance_ratio",
            "spec_conformance_summary",
            "unbound",
        ],
        "the pre-S-458 coverage keys"
    );

    for args in [&["xservice", "route-providers"][..], &["workspace", "status"][..]] {
        let (text, _) = logos_human(tmp.path(), args);
        for key in ["declared_contracts", "bound_external", "declared_scope_note"] {
            assert!(!text.contains(key), "`logos {args:?}` carries no `{key}`: {text}");
        }
    }
}

/// AC3's CLI half: `--help` names both relations as declared, not observed —
/// the MCP descriptions are pinned by `mcp/tests/xservice_roster.rs`, and this
/// pins the CLI twin so the two cannot drift, as
/// `xservice_surface.rs`'s route-providers help guard does for the S-420 claim.
#[test]
fn the_help_names_both_relations_as_declared_not_observed() {
    let tmp = TempDir::new().unwrap();
    let help = |args: &[&str]| {
        let out = logos(tmp.path(), args);
        assert!(out.status.success(), "`logos {args:?}` exits 0");
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    let route = help(&["xservice", "route-providers", "--help"]);
    for clause in [
        "`declared_contracts`",
        "`bound_external`",
        "`declared_scope_note`",
        "DECLARED by vendored specs, not observed calls",
        "never among them",
    ] {
        assert!(route.contains(clause), "route-providers --help names {clause}:\n{route}");
    }

    let status = help(&["workspace", "status", "--help"]);
    for clause in ["`coverage.declared_contracts`", "`coverage.bound_external`", "DECLARED by vendored specs"] {
        assert!(status.contains(clause), "workspace status --help names {clause}:\n{status}");
    }
}
