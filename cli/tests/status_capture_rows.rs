//! `status` resolution figures agree across surfaces and between a synced store
//! and a cold reindex (S-598, [CR-195], [FR-RS-09], [FR-RS-04], [NFR-RA-06],
//! [ADR-01]).
//!
//! A capture-before-delete row ([ADR-10], `RefForm::Symbol`) is written when a
//! file is synced and duplicates a reference its source's own row records, so
//! counting it made a synced two-file Go module read calls and imports 1/1 → 2/2
//! (release 1.10.0). Here the real `logos` binary syncs the module and its
//! `--json` figures — and the HTTP `/api/v1/status` answered over the same store
//! — must equal a cold reindex's, with a capture row demonstrably in the
//! ledger. The MCP rendering is `mcp/tests/status_capture_rows.rs`: that crate
//! owns the protocol client.
//!
//! Since CR-187 that sync deletes the captures it spends, so the row this rule
//! still governs is the one a sync keeps — unbound, from a live source none of
//! whose rows was re-bound. It is planted through the store as such a sync
//! leaves it, and a bare `logos sync` runs over it.
//!
//! The comparison is `serde_json::Value` equality over every figure the readout
//! carries, so neither side's shape is written down here.
//!
//! [CR-195]: ../../docs/requests/CR-195-status-resolution-figures-exclude-capture-before-delete-rows.md
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [FR-RS-04]: ../../docs/specs/requirements/FR-RS-04.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::graph_store::NewUnresolvedRef;
use logos_core::model::{EdgeKind, RefForm};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;

const A_FILE: &str = "a/a.go";
const B_FILE: &str = "b/b.go";
const GO_MOD: &str = "module example.com/m\n\ngo 1.22\n";
const A_GO: &str = "package a\n\nfunc F() {}\n";
const B_GO: &str = "package b\n\nimport \"example.com/m/a\"\n\nfunc G() {\n\ta.F()\n}\n";

/// The status fields that are resolution figures — everything else (paths, sizes,
/// timestamps, revisions) legitimately differs between two stores.
const FIGURES: [&str; 5] = [
    "refs_total",
    "refs_resolved",
    "refs_unresolved",
    "resolution_coverage",
    "resolution_by_language",
];

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn module() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "go.mod", GO_MOD);
    write(tmp.path(), A_FILE, A_GO);
    write(tmp.path(), B_FILE, B_GO);
    tmp
}

/// Run `logos --project <project> <args> --json`, asserting exit 0 and a
/// machine-clean JSON stdout.
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

/// The resolution figures of a status payload, as one object.
fn figures(status: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in FIGURES {
        let value = status
            .get(key)
            .unwrap_or_else(|| panic!("status carries {key}: {status}"));
        out.insert(key.to_string(), value.clone());
    }
    Value::Object(out)
}

/// The capture rows the store behind `project` holds.
fn capture_rows(project: &Path) -> usize {
    let engine = Engine::start(project).expect("engine starts");
    engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .iter()
                .filter(|r| r.form == RefForm::Symbol)
                .count())
        })
        .expect("read runs")
}

/// Plant the capture a sync keeps (CR-187) in the store behind `project`: an
/// unbound `Symbol` row under the target's file `a/a.go`, from the live caller
/// `G`, awaiting a target nothing else in the ledger names.
fn plant_awaiting_capture(project: &Path) {
    let engine = Engine::start(project).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let caller = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .find(|n| n.name == "G")
                .map(|n| n.symbol.as_str().to_string())
                .expect("the G node"))
        })
        .expect("read runs");
    rt.submit_write(move |w| {
        w.insert_unresolved_ref(&NewUnresolvedRef {
            file_id: w.file_id(A_FILE)?,
            source_symbol: &caller,
            target: "planted vanished target",
            alias: None,
            form: RefForm::Symbol,
            kind: EdgeKind::Calls,
            line: None,
            payload: None,
            receiver: None,
            peeled: None,
            arg_count: None,
            exported: None,
        })
    })
    .expect("plant the awaiting capture");
}

#[cfg(feature = "ui")]
async fn http_status(project: &Path) -> Value {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let engine = std::sync::Arc::new(Engine::start(project).expect("engine starts"));
    let router = web::router(engine);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/status")
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .expect("a well-formed request");
    let resp = router.oneshot(request).await.expect("route responds");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let body = String::from_utf8(bytes.to_vec()).expect("utf8 body");
    assert_eq!(status, StatusCode::OK, "/api/v1/status answers 200: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("/api/v1/status is JSON: {e}\n{body}"))
}

/// The two-file Go module, indexed and then synced on `a/a.go` (the 1.10.0
/// reproduction), then holding the capture a sync keeps through a bare
/// `logos sync`: its directory, and the figures the CLI read before the sync.
fn synced_module() -> (TempDir, Value) {
    let tmp = module();
    logos_json(tmp.path(), &["index"]);
    let fresh = figures(&logos_json(tmp.path(), &["status"]));
    assert_eq!(capture_rows(tmp.path()), 0, "a fresh index holds no capture row");

    write(tmp.path(), A_FILE, &format!("// touched\n{A_GO}"));
    logos_json(tmp.path(), &["sync", A_FILE]);
    assert_eq!(capture_rows(tmp.path()), 0, "the sync spent the captures it wrote");

    plant_awaiting_capture(tmp.path());
    logos_json(tmp.path(), &["sync"]);
    assert_eq!(
        capture_rows(tmp.path()),
        1,
        "an awaiting capture outlives the sync, or this test pins nothing"
    );
    (tmp, fresh)
}

#[test]
fn a_synced_store_and_a_cold_reindex_report_the_same_figures_through_the_cli() {
    let (tmp, fresh) = synced_module();
    let synced = figures(&logos_json(tmp.path(), &["status"]));
    assert_eq!(fresh, synced, "a sync that changes no reference moves no figure");

    let cold = module();
    write(cold.path(), A_FILE, &format!("// touched\n{A_GO}"));
    logos_json(cold.path(), &["index"]);
    assert_eq!(synced, figures(&logos_json(cold.path(), &["status"])));

    // The 1.10.0 numbers, named: one call and one import, bound, either way.
    let go = &synced["resolution_by_language"][0];
    assert_eq!(go["language"], json!("go"));
    for class in ["calls", "imports"] {
        assert_eq!(
            (&go[class]["references"], &go[class]["bound"]),
            (&json!(1), &json!(1)),
            "{class}: {go}"
        );
    }
}

/// HTTP renders the figures the CLI renders over the same synced store — and
/// those are a cold reindex's, so the agreement is not two surfaces equally wrong.
#[cfg(feature = "ui")]
#[tokio::test]
async fn http_renders_the_figures_the_cli_renders_over_a_synced_store() {
    let (tmp, _) = synced_module();
    let cli = figures(&logos_json(tmp.path(), &["status"]));
    let http = figures(&http_status(tmp.path()).await);
    assert_eq!(cli, http);

    let cold = module();
    write(cold.path(), A_FILE, &format!("// touched\n{A_GO}"));
    logos_json(cold.path(), &["index"]);
    assert_eq!(http, figures(&http_status(cold.path()).await));
}
