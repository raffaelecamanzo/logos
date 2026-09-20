// CR-078/ADR-60: the wiki-**generation** trigger is the LLM egress carve-out, so
// `POST /wiki/generate` is mounted only under `--features agents`. A listen-only
// `--features ui` build answers it `405` (`carve_out.rs` pins that), so this whole
// test crate is empty there.
#![cfg(feature = "agents")]
//! Logos's own wiki generator is not a developer browsing the dashboard
//! ([FR-OB-13], [CR-139], [FR-OB-09], [BR-42]).
//!
//! Drives the **real** router in-process over a **real** telemetry install —
//! `observability::init(ProcessSurface::Web, …)` → `POST /wiki/generate` +
//! `GET /api/v1/search` → `telemetry.db` → `Engine::stats` — because that whole
//! chain is the claim. The unit tests prove the pieces (the variant is declared,
//! rostered and classified); only this proves the shipped endpoint's calls
//! actually land under the generator's own surface.
//!
//! # The pair, in one process, is the test
//!
//! It asserts **both** halves and never one alone: the generator's
//! `wiki_materialize` is stored as `wikigen`, **and** a human SPA graph query
//! issued over the same surface in the same process is still stored as `web`.
//! Either half on its own is satisfied by a blanket rule — stamping the whole
//! process `wikigen`, or attributing nothing at all — and [CR-139]'s CRA-01/02
//! name both for exactly that reason.
//!
//! The assertions run against the **emission path**, never against a filter: the
//! rows are read out of `events` as the adapter wrote them, and the read-model
//! is then asked to keep both calls *and* tell them apart. A filter that
//! excluded either surface wholesale would fail this test rather than pass it.
//!
//! Everything lives in **one** test function, for the reason
//! `telemetry_classification.rs` records: `init` installs the *global*
//! subscriber, so a second parallel test in this binary would record its own
//! calls into the same store and perturb the counts.
//!
//! [CR-139]: ../../docs/requests/CR-139-the-wiki-generation-pass-names-its-own-surface.md
//! [FR-OB-09]: ../../docs/specs/requirements/FR-OB-09.md
//! [FR-OB-13]: ../../docs/specs/requirements/FR-OB-13.md

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;
use tower::ServiceExt;

use logos_core::observability::{self, ProcessSurface, Surface};
use logos_core::Engine;
use web::{router_with_intent, IntentToken, INTENT_HEADER, WIKI_GENERATE_ROUTE};

const ORIGIN: &str = "http://127.0.0.1:4983";
const HOST: &str = "127.0.0.1:4983";

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap()
}

fn wiki_post(intent: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(WIKI_GENERATE_ROUTE)
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(INTENT_HEADER, intent)
        .header(header::ACCEPT, "text/event-stream")
        .body(Body::empty())
        .unwrap()
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Every `(surface, tool)` pair the store holds for `tool`, as the emission path
/// wrote it — read straight out of `events`, with no read-model predicate
/// between the assertion and the row.
fn surfaces_for_tool(db: &Path, tool: &str) -> Vec<String> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open telemetry.db read-only");
    let mut stmt = conn
        .prepare("SELECT surface FROM events WHERE tool = ?1 ORDER BY surface")
        .expect("prepare");
    let rows = stmt
        .query_map([tool], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    rows
}

/// [FR-OB-13] end to end through the emission path: the generation pass's engine
/// call is attributed to [`Surface::WikiGen`], while a human's graph query in the
/// same `serve --ui` process is still [`Surface::Web`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_generators_pass_and_a_humans_query_are_stored_under_different_surfaces() {
    // ── The vocabulary the pair is asserted in is four distinct wire names ───
    //
    // CRA-01 asks for separability from `web`, `mcp` AND `chat`. Only `web` can
    // be produced in this process, so the other two are pinned here, on the one
    // thing that makes a stored row separable at all: its wire value.
    let generator = Surface::WikiGen.as_str();
    assert_eq!(generator, "wikigen", "the FR-OB-13 wire value");
    for other in [Surface::Web, Surface::Mcp, Surface::Chat] {
        assert_ne!(
            generator,
            other.as_str(),
            "the generator's surface is separable from {:?}; a shared wire value \
             would sum two different claims into one bucket (FR-OB-13, NFR-CC-04)",
            other
        );
    }

    let dir = TempDir::new().expect("temp root");
    let repo = dir.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    // SRS mode, so `wiki_materialize` does real work rather than short-circuiting
    // — the event is emitted either way, but a no-op pass would make the fixture
    // prove less than it looks like it proves.
    commit(
        repo,
        "docs/specs/architecture.md",
        "# Architecture\n\nThe system design.\n",
        "add architecture doc",
    );
    commit(
        repo,
        "docs/specs/requirements/FR-X-01.md",
        "# FR-X-01\n\nA requirement.\n",
        "add requirement",
    );
    commit(repo, "src/lib.rs", "pub fn f() {}\n", "add code");
    std::fs::create_dir_all(repo.join(".logos")).expect("pre-create .logos");

    // Index BEFORE telemetry is installed: the fixture's own indexing is not
    // part of the claim, and counting it would put `index` rows in the store
    // that neither half of the pair is about.
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();

    // The adapter wiring, exactly as `logos serve --ui` installs it — including
    // the production `ConfiguredWikiRunService` that `router_with_intent`
    // assembles. No mock service: the call under test is the one production
    // spawns.
    let guard = observability::init(ProcessSurface::Web, repo);
    let intent = IntentToken::generate();
    let router = router_with_intent(Arc::clone(&engine), intent.clone());

    // ── Logos's own generator ────────────────────────────────────────────────
    //
    // Unconfigured, so the LLM half never runs ([FR-UI-18]) — but the
    // deterministic presented tier runs first and unconditionally ([FR-WK-20],
    // [CR-062]), which is the `wiki_materialize` call this story is about.
    let resp = router
        .clone()
        .oneshot(wiki_post(intent.as_str()))
        .await
        .expect("route responds");
    assert_eq!(resp.status(), StatusCode::OK, "the generation trigger answers");
    let body = body_string(resp).await;
    assert!(
        body.contains("event: configure-first"),
        "no model/key configured — the LLM half never ran: {body}"
    );
    assert!(
        !body.contains("event: error"),
        "materialize succeeded, so the pass reached the configure-first state \
         rather than faulting before it: {body}"
    );

    // ── A developer browsing the dashboard, same process, same surface ───────
    let resp = router
        .clone()
        .oneshot(get("/api/v1/search?q=f"))
        .await
        .expect("route responds");
    assert_eq!(resp.status(), StatusCode::OK, "the search endpoint answers");

    // Flush the last telemetry batch exactly as a process exit would.
    drop(guard);

    let telemetry_db = repo.join(".logos").join("telemetry.db");
    assert!(telemetry_db.is_file(), "telemetry.db created ([FR-OB-03])");

    // ── The pair, as the emission path wrote it ─────────────────────────────
    assert_eq!(
        surfaces_for_tool(&telemetry_db, "wiki_materialize"),
        vec!["wikigen".to_string()],
        "the generation pass's engine call is attributed to Logos's own \
         generator, not to the `web` process surface it runs inside \
         (FR-OB-13 AC 1)"
    );
    assert_eq!(
        surfaces_for_tool(&telemetry_db, "search"),
        vec!["web".to_string()],
        "and a human's SPA graph query in the SAME process is still `web` — \
         the half that distinguishes a correct attribution from a blanket one \
         (FR-OB-13 AC 2)"
    );

    // ── Separable in the read-model, and neither one excluded ───────────────
    //
    // `Surface::WikiGen` answers `None` to `event_class`, so the generator's
    // work stays counted; what changed is only whose bucket it lands in. A
    // variant that had quietly forced the self-referential class would drop
    // `wiki_materialize` out of `calls_by_tool` and fail here.
    let stats = Engine::open(repo).stats(None);
    let counted: Vec<(&str, &str)> = stats
        .calls_by_tool
        .iter()
        .map(|u| (u.surface.as_str(), u.tool.as_str()))
        .collect();
    assert!(
        counted.contains(&("wikigen", "wiki_materialize")),
        "the generator's pass is real engine work and still appears, under its \
         own surface: {counted:?}"
    );
    assert!(
        counted.contains(&("web", "search")),
        "so does the developer's query, under theirs: {counted:?}"
    );
    assert!(
        !counted.contains(&("web", "wiki_materialize")),
        "and nothing books the generator's pass against a developer browsing \
         the dashboard — the defect CR-139 was filed about: {counted:?}"
    );
}
