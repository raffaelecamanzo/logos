//! `logos --json improvements` end to end through the shipped binary, and its
//! agreement with `GET /api/v1/health` (S-505, [FR-QM-17]).
//!
//! The "where to start" list rides three surfaces over one read of the last
//! snapshot: the Health payload (`scan.where_to_start`), the MCP `improvements`
//! tool and this CLI twin. `CARGO_BIN_EXE_logos` and the axum router are both
//! reachable only from this crate, so the binary-vs-router pair is pinned here;
//! the MCP-vs-engine pair, which also meets the Health value on
//! `Engine::latest_health()`, is `mcp/tests/health_parity.rs`. Each pair is
//! compared where both of its surfaces are reachable (`xservice_surface.rs`'s
//! convention), and the pairs meet on one `Value`.
//!
//! [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The binary over `root`, never escaping into an enclosing repository.
fn logos(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(root)
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("logos runs")
}

fn ok_json(out: &Output, what: &str) -> Value {
    assert!(out.status.success(), "{what}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{what} is JSON: {e}"))
}

/// Eight shallow functions and one nested six deep: the deep one is a ranked
/// fix unit and a nesting outlier against this member's nine-function
/// distribution, so the list holds real entries.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let mut source: String = (0..8)
        .map(|i| format!("pub fn shallow_{i}(x: i32) -> i32 {{\n    if x > {i} {{\n        return x;\n    }}\n    0\n}}\n"))
        .collect();
    let open: String = (0..6).map(|i| format!("if x > {i} {{\n")).collect();
    source.push_str(&format!("pub fn deep_one(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(6)));
    write(tmp.path(), "src/lib.rs", &source);
    let index = logos(tmp.path(), &["index"]);
    assert!(index.status.success(), "index: {}", String::from_utf8_lossy(&index.stderr));
    tmp
}

#[test]
fn improvements_is_not_recorded_before_a_scan_then_lists_the_ranked_units_byte_identically() {
    let tmp = fixture();
    let root = tmp.path();

    let never = ok_json(&logos(root, &["--json", "improvements"]), "improvements before a scan");
    assert_eq!(never, json!({"recorded": false, "units": [], "outliers": []}), "never an empty 'clean' list");

    ok_json(&logos(root, &["--json", "scan"]), "scan");
    let (first, second) = (logos(root, &["--json", "improvements"]), logos(root, &["--json", "improvements"]));
    assert_eq!(first.stdout, second.stdout, "a rerun on an unchanged snapshot prints the same bytes");
    let list = ok_json(&first, "improvements");
    assert_eq!(list["recorded"], true, "{list}");
    assert_eq!(list["units"][0]["name"], "deep_one", "{list}");
    assert_eq!(list["units"][0]["kind"], "function", "{list}");
    assert!(list["units"][0]["points"].as_i64().is_some_and(|p| p >= 1), "{list}");
    assert!(list["basis"].as_str().is_some_and(|b| b.starts_with("estimate")), "{list}");
}

/// The CLI list and the Health payload's `scan.where_to_start` are one value,
/// field for field ([FR-QM-17] AC 3) — compared as parsed JSON, so a field
/// added, dropped, renamed or revalued on either side fails it, and neither
/// shape is written down here to drift.
#[cfg(feature = "ui")]
#[tokio::test]
async fn the_cli_list_equals_the_one_get_api_v1_health_carries() {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let tmp = fixture();
    let root = tmp.path();
    ok_json(&logos(root, &["--json", "scan"]), "scan");
    let cli = ok_json(&logos(root, &["--json", "improvements"]), "improvements");

    let engine = std::sync::Arc::new(logos_core::Engine::start(root).expect("engine starts"));
    let router = web::router(engine);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/health")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .expect("a well-formed request");
    let resp = router.oneshot(request).await.expect("route responds");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let health: Value = serde_json::from_slice(&bytes).expect("health is JSON");

    // Non-trivial first: two empty lists must not pass by agreeing.
    assert_eq!(cli["units"][0]["name"], "deep_one", "{cli}");
    assert_eq!(health["scan"]["where_to_start"], cli, "Health and the CLI twin carry one list");
}
