//! The trend warning on the Health payload (S-785, [CR-243], [FR-GV-27]):
//! `/api/v1/health`'s gate carries the structured `trend` and its warning
//! exactly as the persisting gate (`gate --json`, the MCP `session_end`) does,
//! and serving it records no snapshot ([ADR-28]).
//!
//! [CR-243]: ../../docs/requests/CR-243-governance-signals-reach-every-surface.md
//! [FR-GV-27]: ../../docs/specs/requirements/FR-GV-27.md
//! [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md

use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `name` with `n` sequential `if`s: cyclomatic complexity `n + 1`.
fn branches(name: &str, n: usize) -> String {
    let body: String = (0..n).map(|i| format!("    if x == {i} {{\n        return {i};\n    }}\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    x\n}}\n")
}

fn metric_count(root: &Path) -> i64 {
    let conn = rusqlite::Connection::open(root.join(".logos/logos.db")).unwrap();
    conn.query_row("SELECT count(*) FROM metric_snapshots", [], |r| r.get(0)).unwrap()
}

async fn health(engine: &Arc<Engine>) -> Value {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/health")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap();
    let response = web::router(Arc::clone(engine)).oneshot(request).await.expect("route responds");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

/// Two scans that each lower Equality under `[review]`: the Health payload's
/// gate carries the persisting gate's `trend` and trend warning, byte for
/// byte, and loading it writes no snapshot.
#[tokio::test]
async fn the_health_gate_carries_the_trend_the_gate_reports() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, ".logos/rules.toml", "[review]\n");
    let engine = Arc::new(Engine::start(root).expect("engine starts"));
    let many: String = (0..200).map(|i| branches(&format!("f{i}"), 2 + i % 2)).collect();
    for extra in 0..3 {
        let added: String = (0..extra).map(|i| branches(&format!("g{i}"), 5 + i)).collect();
        write(root, "src/lib.rs", &format!("{many}{added}"));
        engine.scan(true).expect("scan runs");
    }
    let gate = serde_json::to_value(engine.gate(None, false, true).expect("gate runs")).unwrap();
    assert_eq!(gate["trend"][0]["dimension"], "equality", "{gate}");

    let recorded = metric_count(root);
    let served = health(&engine).await;
    assert_eq!(metric_count(root), recorded, "the Health read records no snapshot");
    assert_eq!(served["gate"]["trend"], gate["trend"]);
    let trend_warnings = |v: &Value| -> Vec<Value> {
        v["warnings"].as_array().unwrap().iter().filter(|w| w.as_str().unwrap().contains("review.trend_window")).cloned().collect()
    };
    assert_eq!(trend_warnings(&served["gate"]), trend_warnings(&gate));
    assert_eq!(trend_warnings(&gate).len(), 1, "{gate}");

    std::fs::remove_file(root.join(".logos/rules.toml")).unwrap();
    let served = health(&engine).await;
    assert!(served["gate"].get("trend").is_none(), "no table, no trend: {}", served["gate"]);
}
