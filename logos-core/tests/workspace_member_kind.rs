//! S-457 ([FR-WS-32]) end to end: a manifest-declared member kind, read by
//! `discover`, sets that member's contract-surface rows apart on the real
//! `workspace status` read-model — and the candidate hint names a member that
//! looks like a documentation repo while classifying nothing.
//!
//! Real members, really indexed: an `api` member holding only an OpenAPI
//! document (no runnable source), and a `web` member serving the route in Rust.
//! The workspace is entered through [`discover`] over a written manifest — the
//! path `logos workspace status` takes — so the `[workspace.member.<name>]`
//! parse, its resolution onto the member set, and the read-model are exercised
//! as one.
//!
//! [FR-WS-32]: ../../docs/specs/requirements/FR-WS-32.md
#![cfg(all(feature = "lang-yaml", feature = "lang-rust"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::{discover, workspace_status, EngineRegistry, RegistryMode};
use logos_core::Engine;

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
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// A member is a distinct git root ([FR-WS-01]); `discover` drops anything else.
///
/// [FR-WS-01]: ../../docs/specs/requirements/FR-WS-01.md
fn git_init(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir member");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git init {}", dir.display());
}

fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// `api` holds only the spec, `web` serves `GET /users/{id}` in Rust.
fn docs_and_web(root: &Path) {
    let api = root.join("api");
    let web = root.join("web");
    git_init(&api);
    git_init(&web);
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&web, "src/main.rs", AXUM_MAIN);
    index_member(&api);
    index_member(&web);
}

fn status_over(root: &Path, manifest: &str) -> serde_json::Value {
    fs::write(root.join("logos.workspace.toml"), manifest).expect("write manifest");
    let federation = discover(root).expect("manifest parses").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    serde_json::to_value(workspace_status(&registry)).expect("status serializes")
}

/// The buckets of `member`'s contract-surface rows, sorted — the rows themselves
/// sort by endpoint symbol, which is not what these assertions are about.
fn contract_surface_rows_from<'a>(rows: &'a serde_json::Value, member: &str) -> Vec<&'a str> {
    let mut buckets: Vec<&str> = rows
        .as_array()
        .expect("an array of rows")
        .iter()
        .filter(|row| row["from"]["member"] == member && row["intake"] == "contract-surface")
        .map(|row| row["bucket"].as_str().expect("bucket"))
        .collect();
    buckets.sort_unstable();
    buckets
}

/// **The candidate stays in the headline until declared.** Undeclared, `api`
/// — an OpenAPI document and no runnable source — is listed as a candidate
/// *and* its two rows are still in the headline: the hint classified nothing.
/// Declared `documentation` in the manifest, the same rows leave the headline
/// for `declared_apart`, stated over their denominator, and `api` is no longer
/// a candidate. `web` is never one: it holds runnable source.
#[test]
fn a_candidate_stays_in_the_headline_until_its_kind_is_declared() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    docs_and_web(root);

    let undeclared = status_over(
        root,
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\n",
    );
    assert_eq!(
        undeclared["kind_candidates"]["members"],
        serde_json::json!(["api"]),
        "api holds API documents and no runnable source; web holds Rust"
    );
    assert_eq!(undeclared["kind_candidates"]["members_total"], 2);
    assert_eq!(
        contract_surface_rows_from(&undeclared["coverage"]["references"], "api"),
        ["bound", "unbound"],
        "a candidate's rows stay in the headline — the hint moves nothing"
    );
    assert_eq!(undeclared["coverage"]["by_intake"]["contract_surface"]["bound"], 1);
    assert!(undeclared["coverage"].get("declared_apart").is_none());

    let declared = status_over(
        root,
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\n\n\
         [workspace.member.api]\nkind = \"documentation\"\n",
    );
    assert!(
        declared.get("kind_candidates").is_none(),
        "a declared member is no longer a candidate: {}",
        declared["kind_candidates"]
    );
    let coverage = &declared["coverage"];
    assert!(
        contract_surface_rows_from(&coverage["references"], "api").is_empty(),
        "the declared member's rows left the headline"
    );
    assert_eq!(coverage["by_intake"]["contract_surface"]["bound"], 0);
    assert!(
        coverage.get("spec_conformance_ratio").is_none(),
        "nothing left to measure is absent, never 1.0: {coverage}"
    );
    assert_eq!(
        coverage["spec_conformance_summary"],
        "0 of 0 measured; 0 excluded as no-provider-in-workspace; \
         2 of 2 contract-surface rows reported apart by declared member kind",
        "the line every surface renders states the rows set apart (BR-51)"
    );
    let apart = &coverage["declared_apart"];
    assert_eq!(apart["rows"], 2);
    assert_eq!(apart["contract_surface_rows"], 2);
    assert_eq!(apart["members"][0]["member"], "api");
    assert_eq!(apart["members"][0]["kind"], "documentation");
    assert_eq!(
        apart["summary"],
        "2 of 2 contract-surface rows reported apart from 1 declared member \
         (documentation: 1); the headline and spec_conformance_ratio exclude them"
    );
    assert_eq!(
        contract_surface_rows_from(&apart["references"], "api"),
        ["bound", "unbound"],
        "the same two rows, moved and not dropped"
    );
}

/// A member holding a spec **and** runnable source is not a candidate — the
/// hint is "API documents and no runnable source", not "API documents".
#[test]
fn a_member_with_a_spec_and_runnable_source_is_no_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let status = spec_and_source_workspace_status(tmp.path());
    assert!(
        status.get("kind_candidates").is_none(),
        "no candidate when every member holds runnable source: {}",
        status["kind_candidates"]
    );
    assert_eq!(
        contract_surface_rows_from(&status["coverage"]["references"], "api"),
        ["bound", "unbound"]
    );
}


/// The four per-run facts a status payload carries — the temp path, the store's
/// size on disk and two timestamps — replaced by a marker, so what remains is
/// the payload's whole shape and every figure in it.
fn masked(mut value: serde_json::Value) -> String {
    fn walk(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    if matches!(
                        key.as_str(),
                        "db_path" | "db_size_bytes" | "last_full_index_at" | "last_sync_at"
                    ) {
                        *child = serde_json::Value::String("<per-run>".to_string());
                    } else {
                        walk(child);
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut value);
    serde_json::to_string(&value).expect("serializes")
}

/// Every member holds runnable source and none declares a kind: the workspace
/// the byte-for-byte guarantee is stated over (no declaration, no candidate).
fn spec_and_source_workspace_status(root: &Path) -> serde_json::Value {
    docs_and_web(root);
    write(&root.join("api"), "src/lib.rs", "pub fn handler() {}\n");
    index_member(&root.join("api"));
    status_over(
        root,
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\n",
    )
}

/// **No declaration and no candidate: every status surface is byte-for-byte
/// what it was.** The CLI (human and `--json`), the MCP tool and the web route
/// all serialize this one read-model, so pinning it pins them.
///
/// The golden is what this same fixture produced under the code at `96b7a66`,
/// before S-457 — generated by running this test's body against that tree — so
/// the comparison is against a vintage the new code cannot have written. Only
/// the four per-run facts are masked ([`masked`]); every key, every figure and
/// the key order are compared.
#[test]
fn a_workspace_without_declarations_or_candidates_renders_status_byte_for_byte_as_before() {
    let tmp = tempfile::tempdir().unwrap();
    let status = masked(spec_and_source_workspace_status(tmp.path()));
    assert_eq!(
        status,
        include_str!("golden/workspace_status_pre_s457.json"),
        "a workspace with no `kind` declaration and no candidate must not move a byte"
    );
}
