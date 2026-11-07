//! S-508 ([FR-UI-38] and [FR-UI-12] as modified by [CR-165]): the workspace rules
//! contract, `<workspace-root>/.logos/rules.toml`, on the web surface.
//!
//! - `GET /api/v1/workspace/rules` and `POST /api/v1/workspace/rules/save` — the
//!   app-level editor's Rules section: validate before writing, refuse a
//!   member-only table with its reason, refuse a stale fingerprint, write only
//!   at the workspace root.
//! - `GET /api/v1/config?repo=<member>` — the member editor's read-model carries
//!   what the member inherits (origin + root value per key) beside its own
//!   literal document, and nothing at all without a tier.
//!
//! Every `rules.toml` here is written at runtime into a temp dir.
//!
//! [FR-UI-38]: ../../docs/specs/requirements/FR-UI-38.md
//! [FR-UI-12]: ../../docs/specs/requirements/FR-UI-12.md
//! [CR-165]: ../../docs/requests/CR-165-workspace-rules-inherit-and-ratchet.md

use std::path::Path;
use std::process::Command;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use logos_core::federation::{discover, EngineRegistry};
use logos_core::Engine;
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

/// The load fingerprint of an absent file — the empty document's.
const EMPTY_DOCUMENT_FINGERPRINT: &str =
    "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@logos", "-c", "user.name=t", "-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A two-member workspace (`api`, `web`), each an indexed git repo.
fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    for member in ["api", "web"] {
        let dir = tmp.path().join(member);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn entry() -> u32 { 1 }\n").unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        Engine::start(&dir).expect("engine").pipeline().index();
    }
    std::fs::write(
        tmp.path().join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    tmp
}

fn router(tmp: &TempDir) -> (axum::Router, IntentToken) {
    let federation = discover(tmp.path()).expect("discovery").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new_serve_default(federation);
    let intent = IntentToken::generate();
    let router = web::workspace_router_with_intent(registry, intent.clone()).expect("router builds");
    (router, intent)
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .body(Body::empty())
        .unwrap()
}

fn save(content: &str, fingerprint: &str, intent: &IntentToken) -> Request<Body> {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect::<String>()
    };
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/workspace/rules/save")
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ORIGIN, "http://127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::from(format!("content={}&fingerprint={}", enc(content), enc(fingerprint))))
        .unwrap()
}

async fn call(router: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = router.clone().oneshot(req).await.expect("route responds");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let json = serde_json::from_str(&text).unwrap_or_else(|e| panic!("JSON ({e}): {status} {text}"));
    (status, json)
}

/// The root contract's path, and its bytes (`None` when absent).
fn root_rules(tmp: &TempDir) -> (std::path::PathBuf, Option<Vec<u8>>) {
    let path = tmp.path().join(".logos").join("rules.toml");
    let bytes = std::fs::read(&path).ok();
    (path, bytes)
}

fn write_root_rules(tmp: &TempDir, text: &str) {
    let dir = tmp.path().join(".logos");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rules.toml"), text).unwrap();
}

/// The read names the document, its fingerprint and parse verdict, and the
/// member-only tables it declares, each with the reason the tier refuses it.
#[tokio::test]
async fn the_rules_read_states_the_document_its_fingerprint_and_its_refused_tables() {
    let tmp = workspace();
    let (router, _intent) = router(&tmp);

    let (status, absent) = call(&router, get("/api/v1/workspace/rules")).await;
    assert_eq!(status, StatusCode::OK, "{absent}");
    assert_eq!(absent["exists"], false);
    assert_eq!(absent["content"], "");
    assert_eq!(absent["fingerprint"], EMPTY_DOCUMENT_FINGERPRINT);
    assert_eq!(absent["refused"], serde_json::json!([]));

    write_root_rules(
        &tmp,
        "[constraints]\nmax_cc = 15\n\n[[layers]]\nname = \"core\"\npaths = [\"src/**\"]\norder = 0\n",
    );
    let (status, declared) = call(&router, get("/api/v1/workspace/rules")).await;
    assert_eq!(status, StatusCode::OK, "{declared}");
    assert_eq!(declared["exists"], true);
    assert_eq!(declared["parsed"]["constraints"]["max_cc"], 15);
    assert_eq!(declared["refused"][0]["table"], "[[layers]]", "{declared}");
    assert!(
        declared["refused"][0]["reason"].as_str().unwrap().contains("member paths"),
        "the refusal states its reason: {declared}"
    );

    write_root_rules(&tmp, "[constraints\n");
    let (status, broken) = call(&router, get("/api/v1/workspace/rules")).await;
    assert_eq!(status, StatusCode::OK, "a broken contract still loads for repair: {broken}");
    assert!(broken["parsed"].is_null() && broken["error"].as_str().is_some(), "{broken}");
}

/// Validate before writing: a candidate the parser rejects is a `422`, and
/// nothing is written — not even the root's `.logos/` for an absent file.
#[tokio::test]
async fn an_invalid_rules_save_is_refused_before_anything_is_written() {
    let tmp = workspace();
    let (router, intent) = router(&tmp);
    for bad in ["[constraints\n", "[constraints]\nmax_cc = \"high\"\n", "[constraints]\nno_such_key = 1\n"] {
        let (status, body) = call(&router, save(bad, EMPTY_DOCUMENT_FINGERPRINT, &intent)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad:?}: {body}");
        assert!(!tmp.path().join(".logos").exists(), "{bad:?} wrote nothing at the root");
    }
}

/// Path-based (member-only) tables are refused inline with the stated reason,
/// and the contract on disk stays byte-identical.
#[tokio::test]
async fn a_member_only_table_is_refused_with_its_reason_and_nothing_is_written() {
    let tmp = workspace();
    write_root_rules(&tmp, "[constraints]\nmax_cc = 20\n");
    let (router, intent) = router(&tmp);
    let (_, loaded) = call(&router, get("/api/v1/workspace/rules")).await;
    let fingerprint = loaded["fingerprint"].as_str().unwrap();
    let (_, before) = root_rules(&tmp);

    for (candidate, table, reason) in [
        (
            "[constraints]\nmax_cc = 15\n\n[[forbidden_imports]]\nfrom = \"src/web/**\"\nto = \"src/db/**\"\nreason = \"x\"\n",
            "[[forbidden_imports]]",
            "member paths",
        ),
        ("[history]\nwindow_months = 6\n", "[history]", "one member's own evidence"),
        ("[review]\n", "[review]", "one member's own gate policy"),
    ] {
        let (status, body) = call(&router, save(candidate, fingerprint, &intent)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{table}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(error.contains(table) && error.contains(reason), "{table} refused with its reason: {error}");
        assert!(error.contains("nothing was written"), "{error}");
        assert_eq!(root_rules(&tmp).1, before, "{table}: the contract is byte-identical");
    }
}

/// The fingerprint conflict check: a contract changed on disk since the load is
/// not clobbered — `409` with what is on disk now, and nothing written. A save
/// with no fingerprint at all is a `400`.
#[tokio::test]
async fn a_rules_save_against_a_stale_fingerprint_is_a_conflict_that_writes_nothing() {
    let tmp = workspace();
    write_root_rules(&tmp, "[constraints]\nmax_cc = 20\n");
    let (router, intent) = router(&tmp);
    let (_, loaded) = call(&router, get("/api/v1/workspace/rules")).await;
    let fingerprint = loaded["fingerprint"].as_str().unwrap().to_string();

    let by_hand = "[constraints]\nmax_cc = 25\n";
    write_root_rules(&tmp, by_hand);
    let (status, body) = call(&router, save("[constraints]\nmax_cc = 15\n", &fingerprint, &intent)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["outcome"], "conflict");
    assert_eq!(body["disk_content"], by_hand);
    assert_eq!(root_rules(&tmp).1.as_deref(), Some(by_hand.as_bytes()), "not clobbered");

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/workspace/rules/save")
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ORIGIN, "http://127.0.0.1:4983")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(INTENT_HEADER, intent.as_str())
        .body(Body::from("content=%5Bconstraints%5D%0A"))
        .unwrap();
    let (status, body) = call(&router, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(root_rules(&tmp).1.as_deref(), Some(by_hand.as_bytes()));
}

/// A valid save writes the root contract (stamped like every browser-written
/// contract), reports the fingerprint of the bytes now on disk, writes nothing
/// into any member, and every member's editor now shows the key as inherited
/// with the root's value — while its own literal document is unchanged.
#[tokio::test]
async fn a_valid_rules_save_writes_the_root_and_members_see_the_key_inherited() {
    let tmp = workspace();
    let (router, intent) = router(&tmp);
    let member_rules = |m: &str| std::fs::read(tmp.path().join(m).join(".logos").join("rules.toml")).ok();
    let members_before = (member_rules("api"), member_rules("web"));

    let candidate = "[constraints]\nmax_cc = 15\n\n[metric_thresholds]\ngod_methods = 30\n";
    let (status, body) = call(&router, save(candidate, EMPTY_DOCUMENT_FINGERPRINT, &intent)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "written", "{body}");
    let (path, on_disk) = root_rules(&tmp);
    let on_disk = String::from_utf8(on_disk.expect("written")).unwrap();
    assert!(
        on_disk.starts_with(logos_core::config::RULES_PROVENANCE_STAMP) && on_disk.ends_with(candidate),
        "the candidate, after the provenance stamp (BR-35): {on_disk}"
    );
    assert_eq!(on_disk.len(), logos_core::config::RULES_PROVENANCE_STAMP.len() + candidate.len(), "{on_disk}");
    let (_, reread) = call(&router, get("/api/v1/workspace/rules")).await;
    assert_eq!(body["fingerprint"], reread["fingerprint"], "the fingerprint is of the bytes on disk");
    assert!(path.is_file());
    assert_eq!((member_rules("api"), member_rules("web")), members_before, "no member was written");

    let (status, config) = call(&router, get("/api/v1/config?repo=api")).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    let inherited = &config["workspace_rules"];
    assert_eq!(inherited["origins"]["constraints"]["max_cc"], "workspace", "{inherited}");
    assert_eq!(inherited["origins"]["metric_thresholds"]["god_methods"], "workspace", "{inherited}");
    assert_eq!(inherited["constraints"]["max_cc"], 15, "the root value rides along: {inherited}");
    assert_eq!(inherited["metric_thresholds"]["god_methods"], 30, "{inherited}");
    assert!(inherited["error"].is_null(), "{inherited}");
    assert!(
        config["rules"]["parsed"]["constraints"]["max_cc"].is_null(),
        "the member's own document never carries the inherited value: {config}"
    );
}

/// A member key wins over the root's: its origin reads `member`, while the
/// root's value is still delivered beside it.
#[tokio::test]
async fn a_member_override_reads_member_origin_beside_the_root_value() {
    let tmp = workspace();
    write_root_rules(&tmp, "[constraints]\nmax_cc = 15\nmax_fn_lines = 80\n");
    std::fs::write(tmp.path().join("api/.logos/rules.toml"), "[constraints]\nmax_cc = 25\n").unwrap();
    let (router, _intent) = router(&tmp);
    let (_, config) = call(&router, get("/api/v1/config?repo=api")).await;
    let inherited = &config["workspace_rules"];
    assert_eq!(inherited["origins"]["constraints"]["max_cc"], "member", "{inherited}");
    assert_eq!(inherited["origins"]["constraints"]["max_fn_lines"], "workspace", "{inherited}");
    assert_eq!(inherited["constraints"]["max_cc"], 15, "{inherited}");
    assert_eq!(config["rules"]["parsed"]["constraints"]["max_cc"], 25);
}

/// Without a root contract the member read-model carries no `workspace_rules`
/// key at all ([ADR-52]); a broken one is stated, not raised, so the member's
/// editor still opens over its own document.
///
/// [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
#[tokio::test]
async fn no_root_contract_means_no_inherited_slice_and_a_broken_one_is_stated() {
    let tmp = workspace();
    let (router, _intent) = router(&tmp);
    let (status, config) = call(&router, get("/api/v1/config?repo=web")).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert!(config.get("workspace_rules").is_none(), "no tier, no key: {config}");

    write_root_rules(&tmp, "[constraints\n");
    let (status, config) = call(&router, get("/api/v1/config?repo=web")).await;
    assert_eq!(status, StatusCode::OK, "the member editor still loads: {config}");
    let inherited = &config["workspace_rules"];
    assert!(inherited["origins"].is_null(), "{inherited}");
    assert!(inherited["error"].as_str().unwrap().contains("rules.toml"), "{inherited}");
}

/// A save that would leave the contract exactly as it is writes nothing and says
/// `unchanged` — the candidate saved again, and the stamped document on disk
/// posted back verbatim, even against a stale fingerprint, since identity is
/// decided before staleness (S-508 review).
#[tokio::test]
async fn re_saving_the_contract_as_it_is_is_unchanged_and_writes_nothing() {
    let tmp = workspace();
    let (router, intent) = router(&tmp);
    let candidate = "[constraints]\nmax_cc = 15\n";
    let (status, first) = call(&router, save(candidate, EMPTY_DOCUMENT_FINGERPRINT, &intent)).await;
    assert_eq!((status, first["outcome"].as_str()), (StatusCode::OK, Some("written")), "{first}");
    let (path, written) = root_rules(&tmp);
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();

    let held = first["fingerprint"].as_str().unwrap();
    let (status, again) = call(&router, save(candidate, held, &intent)).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["outcome"], "unchanged", "the same candidate rewrites nothing: {again}");
    assert_eq!(again["fingerprint"], held);

    let on_disk = String::from_utf8(written.clone().unwrap()).unwrap();
    let (status, verbatim) = call(&router, save(&on_disk, "stale-load", &intent)).await;
    assert_eq!(status, StatusCode::OK, "{verbatim}");
    assert_eq!(verbatim["outcome"], "unchanged", "identity is decided before staleness: {verbatim}");

    assert_eq!(root_rules(&tmp).1, written, "the contract is byte-identical");
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified, "and was not rewritten");
}

/// A broken root contract is stated by file and position only — never a
/// fragment of it — on both new read paths: the app-level read and the member
/// read-model's inherited slice (S-508 review). The sentinel sits on the very
/// line the parser rejects, which a parser rendering would quote.
#[tokio::test]
async fn a_broken_root_contract_is_stated_without_quoting_it() {
    const SENTINEL: &str = "sk-SENTINEL-4242";
    let tmp = workspace();
    write_root_rules(&tmp, &format!("[constraints]\nmax_cc = 15\nnote = \"{SENTINEL}\" trailing\n"));
    let (router, _intent) = router(&tmp);

    let (status, read) = call(&router, get("/api/v1/workspace/rules")).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let error = read["error"].as_str().expect("the fault is stated");
    assert!(error.contains("line 3"), "by position: {error}");
    assert!(!error.contains(SENTINEL), "never a fragment of the file: {error}");

    let (status, config) = call(&router, get("/api/v1/config?repo=api")).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    let error = config["workspace_rules"]["error"].as_str().expect("the fault is stated");
    assert!(error.contains("line 3"), "by position: {error}");
    assert!(!error.contains(SENTINEL), "never a fragment of the file: {error}");
}
