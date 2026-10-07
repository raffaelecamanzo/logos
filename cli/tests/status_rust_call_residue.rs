//! `logos status --json` carries the Rust row's `call_residue` (S-589,
//! [CR-188], [FR-RS-42], [FR-RS-09]): the unbound `Calls` rows as its
//! denominator, the count per reason and `unclassified`, with the readout's
//! internals left out. The MCP rendering is
//! `mcp/tests/status_rust_call_residue.rs`: that crate owns the protocol client.
//!
//! [CR-188]: ../../docs/requests/CR-188-rust-receiver-typing.md
//! [FR-RS-42]: ../../docs/specs/requirements/FR-RS-42.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

/// `x.run()` binds. `s.len()` is on a std type, `x.absent()` names a method
/// `S` lacks, the chained `.run()` proves no receiver, and the bare `make()`
/// takes no receiver walk at all, so it is unclassified.
const LIB_RS: &str = "\
pub struct S;
impl S { pub fn run(&self) {} }
pub fn external(s: &String) { s.len(); }
pub fn missing(x: &S) { x.absent(); }
pub fn chain(x: &S) { x.run(); make().run(); }
";

fn logos_json(project: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .arg("--json")
        .output()
        .expect("the logos binary runs");
    assert_eq!(
        out.status.code(),
        Some(0),
        "logos {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("`logos {args:?} --json` is not JSON: {e}\n{stdout}"))
}

#[test]
fn status_json_carries_the_rust_rows_call_residue() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/lib.rs"), LIB_RS).unwrap();
    logos_json(tmp.path(), &["index"]);
    let status = logos_json(tmp.path(), &["status"]);

    let rust = status["resolution_by_language"]
        .as_array()
        .expect("a row array")
        .iter()
        .find(|row| row["language"] == json!("rust"))
        .expect("a rust row");
    assert_eq!(
        rust["call_residue"],
        json!({
            "unbound": 4,
            "reasons": {
                "external-type": 1,
                "no-receiver-evidence": 1,
                "overload-ambiguous": 0,
                "supertype-unreached": 1,
                "type-ambiguous": 0
            },
            "unclassified": 1,
            "scope": "repository"
        }),
        "{rust}"
    );
    let calls = &rust["calls"];
    assert_eq!(
        calls["references"].as_u64().unwrap() - calls["bound"].as_u64().unwrap(),
        4,
        "the denominator is the row's own unbound count: {calls}"
    );
}

/// HTTP states the CLI's residue on the first `GET /api/v1/status` and on the
/// second, which the engine answers from its memo (S-605, CR-201): one engine
/// behind the router serves both, so a cached figure that drifted from a
/// fresh one would show here ([ADR-01] surface parity).
///
/// [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
#[cfg(feature = "ui")]
#[tokio::test]
async fn http_states_the_clis_residue_on_a_miss_and_on_a_memo_hit() {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/lib.rs"), LIB_RS).unwrap();
    logos_json(tmp.path(), &["index"]);
    let cli = logos_json(tmp.path(), &["status"])["resolution_by_language"].clone();

    let engine = std::sync::Arc::new(logos_core::Engine::start(tmp.path()).expect("engine starts"));
    let router = web::router(engine);
    let mut answers = Vec::new();
    for _ in 0..2 {
        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/status")
            .header(header::HOST, "127.0.0.1:4983")
            .body(Body::empty())
            .expect("a well-formed request");
        let resp = router.clone().oneshot(request).await.expect("route responds");
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let status: Value = serde_json::from_slice(&bytes).expect("/api/v1/status is JSON");
        answers.push(status["resolution_by_language"].clone());
    }
    assert_eq!(answers[0], cli, "a miss states the CLI's figures");
    assert_eq!(answers[1], cli, "a memo hit states the CLI's figures");
}
