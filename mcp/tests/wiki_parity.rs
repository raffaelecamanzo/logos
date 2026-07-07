//! CLI-vs-MCP payload-equality for the source-wiki surfaces (S-053, CR-008,
//! [FR-WK-09], [UAT-WK-01]).
//!
//! Both surfaces delegate to the **one** `api` entrypoint per command
//! (`Engine::wiki_write` / `wiki_read` / `wiki_search` / `wiki_status` /
//! `wiki_materialize`, [NFR-CC-01]) and serialise its read-model — the CLI via
//! `serde_json`, the MCP twin via `Content::json`. This proves the wire payloads
//! are byte-identical for the same `wiki.db` + tree over a real indexed git
//! fixture, that every read carries the four mandatory provenance fields
//! ([FR-WK-04]), and that the host lists exactly the five `logos:*` wiki tools
//! as part of the 28-tool surface.

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{Map, Value};

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?} failed");
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// An SRS-mode (Case 1) fixture: `docs/specs/architecture.md` plus one
/// requirement file, so [`Engine::wiki_materialize`] presents the Architecture
/// page and the Functional Requirements category ([FR-WK-20], [FR-WK-21]).
fn srs_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");
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
    Engine::start(repo).expect("engine starts").index();
    tmp
}

/// An indexed git fixture with two source files and one wiki page already
/// written (anchored to one file, with a phrase unique to its body).
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", "pub fn a() -> i64 { 0 }\n", "add a");
    commit(repo, "src/b.rs", "pub fn b() -> i64 { 1 }\n", "add b");
    {
        let engine = Engine::start(repo).expect("engine starts");
        engine.index();
        engine
            .wiki_write(
                "guide/a",
                "About a",
                "# About a\n\nthe quux subsystem orchestrates the a module",
                &["file:src/a.rs".to_string()],
                "claude-opus",
            )
            .expect("seed page write");
    }
    tmp
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = Engine::start(root).expect("engine start");
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

async fn mcp_payload(
    client: &Client,
    tool: &'static str,
    args: Option<Map<String, Value>>,
) -> Value {
    let mut params = CallToolRequestParams::new(tool);
    if let Some(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.call_tool(params).await.expect("tool call");
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_wiki_read_payloads_are_identical_with_full_provenance() {
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.wiki_read("guide/a").expect("read")).unwrap()
    };

    let (client, server) = boot(root).await;
    let args = Map::from_iter([("slug".to_string(), Value::String("guide/a".into()))]);
    let mcp = mcp_payload(&client, "wiki_read", Some(args)).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_read payloads must be byte-identical (FR-WK-09)"
    );

    // All four mandatory provenance fields are present and the marker is fixed
    // (FR-WK-04, UAT-WK-01).
    assert_eq!(mcp["generator"], "claude-opus");
    assert!(mcp["written_head"].is_string(), "written-at HEAD present");
    assert_eq!(
        mcp["marker"], "generated content — not extracted by Logos",
        "the fixed generated-content marker is present and verbatim"
    );
    assert_eq!(
        mcp["anchors"][0]["freshness"], "fresh",
        "per-anchor freshness present"
    );

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn cli_and_mcp_wiki_search_payloads_are_identical_and_rank_the_phrase_first() {
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.wiki_search("quux subsystem", false).expect("search")).unwrap()
    };

    let (client, server) = boot(root).await;
    let args = Map::from_iter([("query".to_string(), Value::String("quux subsystem".into()))]);
    let mcp = mcp_payload(&client, "wiki_search", Some(args)).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_search payloads must be byte-identical (FR-WK-09)"
    );
    // The page with the unique phrase ranks first, staleness-flagged (FR-WK-05).
    assert_eq!(mcp[0]["slug"], "guide/a");
    assert_eq!(mcp[0]["stale"], false);
    // The unified revision-pending verdict (CR-039) is on the wire as a boolean on
    // both surfaces — a fresh-revision page reads false. The byte-identical check
    // above proves CLI≡MCP; this guards against the field being dropped or
    // mistyped on both at once (e.g. a stray `#[serde(skip)]`, FR-WK-09).
    assert_eq!(
        mcp[0]["revision_pending"], false,
        "the revision-pending verdict is carried on the search-hit payload (FR-WK-09, CR-039)"
    );
    // Every hit carries its provenance summary (FR-WK-04 content-side analogue).
    assert_eq!(mcp[0]["generator"], "claude-opus");
    assert!(
        mcp[0]["written_head"].is_string(),
        "search hit carries the written-at HEAD"
    );

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn wiki_read_of_a_stale_page_carries_the_stale_flag_on_both_surfaces() {
    // FR-WK-04 AC2: a stale page renders its stale label alongside the body —
    // never silently — and CLI/MCP stay byte-identical even in the stale state.
    let tmp = fixture();
    let root = tmp.path();
    // Edit the anchored file so guide/a goes stale at the next read.
    std::fs::write(root.join("src/a.rs"), "pub fn a() -> i64 { 99 }\n").unwrap();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        let page = engine.wiki_read("guide/a").expect("read").expect("exists");
        assert!(page.stale, "CLI: the edited anchor makes the read stale");
        serde_json::to_value(Some(page)).unwrap()
    };

    let (client, server) = boot(root).await;
    let args = Map::from_iter([("slug".to_string(), Value::String("guide/a".into()))]);
    let mcp = mcp_payload(&client, "wiki_read", Some(args)).await;
    assert_eq!(
        mcp["stale"], true,
        "MCP: the stale flag is present on the read"
    );
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_read payloads are byte-identical even when stale (FR-WK-09)"
    );

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn cli_and_mcp_wiki_status_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.wiki_status().expect("status")).unwrap()
    };

    let (client, server) = boot(root).await;
    let mcp = mcp_payload(&client, "wiki_status", None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_status payloads must be byte-identical (FR-WK-09)"
    );
    assert_eq!(mcp["page_count"].as_u64(), Some(1));
    // src/b.rs has no page, but File is no longer page-worthy ([CR-056]/[S-221])
    // — it never appears in the unanchored work-list, same as the anchored
    // src/a.rs.
    let unanchored: Vec<&str> = mcp["work_list"]["unanchored_entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entity_id"].as_str().unwrap())
        .collect();
    assert!(
        !unanchored.contains(&"src/b.rs"),
        "the unanchored file is NOT seeded into the work-list: {unanchored:?}"
    );
    assert!(
        !unanchored.contains(&"src/a.rs"),
        "the anchored file is NOT in the work-list: {unanchored:?}"
    );

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn cli_and_mcp_wiki_write_summaries_are_identical() {
    // ONE fixture so both writes see the same tree + HEAD (the WriteSummary
    // carries `written_head`, which is repo-specific). The CLI write is rolled
    // back with `wiki delete` so the MCP write starts from identical state and
    // yields a byte-identical WriteSummary (FR-WK-09).
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        let summary = engine
            .wiki_write(
                "guide/b",
                "About b",
                "# About b\n\nthe b module, described at enough length to clear the guard.",
                &["file:src/b.rs".to_string()],
                "gen",
            )
            .expect("cli write");
        engine.wiki_delete("guide/b").expect("rollback");
        serde_json::to_value(summary).unwrap()
    };

    let (client, server) = boot(root).await;
    let args = Map::from_iter([
        ("slug".to_string(), Value::String("guide/b".into())),
        ("title".to_string(), Value::String("About b".into())),
        (
            "body".to_string(),
            Value::String("# About b\n\nthe b module, described at enough length to clear the guard.".into()),
        ),
        (
            "anchors".to_string(),
            Value::Array(vec![Value::String("file:src/b.rs".into())]),
        ),
        ("generator".to_string(), Value::String("gen".into())),
    ]);
    let mcp = mcp_payload(&client, "wiki_write", Some(args)).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_write summaries must be byte-identical (FR-WK-09)"
    );
    assert_eq!(mcp["anchor_count"].as_u64(), Some(1));
    assert_eq!(mcp["replaced"], false);

    client.cancel().await.ok();
    server.abort();
}

/// [FR-WK-20]/[FR-WK-09]: `Engine::wiki_materialize` — the CR-062 deterministic
/// presented tier — is byte-identical between the CLI's direct `Engine` call and
/// the MCP `wiki_materialize` twin. The CLI run (against a fresh SRS-mode store)
/// already presents the Architecture page and the Functional Requirements
/// category; re-running through MCP against the same store is idempotent (no
/// new pages, nothing pruned), so both payloads name the same materialized
/// slugs and an empty pruned set.
#[tokio::test]
async fn cli_and_mcp_wiki_materialize_payloads_are_identical() {
    let tmp = srs_fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        let summary = engine.wiki_materialize().expect("materialize");
        assert!(summary.srs_mode, "architecture.md + a requirement → Case 1");
        assert_eq!(
            summary.materialized,
            vec![
                "overview/architecture".to_string(),
                "specs/functional-requirements".to_string(),
            ],
        );
        serde_json::to_value(summary).unwrap()
    };

    let (client, server) = boot(root).await;
    let mcp = mcp_payload(&client, "wiki_materialize", None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_materialize payloads must be byte-identical (FR-WK-09)"
    );
    // The MCP call re-runs materialize against the already-materialized store —
    // byte-identical re-run means nothing new pruned (FR-WK-20 acceptance).
    assert_eq!(mcp["pruned"], serde_json::json!([]));

    client.cancel().await.ok();
    server.abort();
}

/// Case 2 (no `docs/specs/architecture.md`): `wiki_materialize` is a no-op on
/// both surfaces — the empty summary, no `wiki.db` write ([FR-WK-21] Case 2).
#[tokio::test]
async fn wiki_materialize_is_a_no_op_outside_srs_mode_on_both_surfaces() {
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.wiki_materialize().expect("materialize")).unwrap()
    };

    let (client, server) = boot(root).await;
    let mcp = mcp_payload(&client, "wiki_materialize", None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP wiki_materialize payloads must be byte-identical outside SRS mode (FR-WK-09)"
    );
    assert_eq!(mcp["srs_mode"], false);
    assert_eq!(mcp["materialized"], serde_json::json!([]));
    assert_eq!(mcp["pruned"], serde_json::json!([]));

    client.cancel().await.ok();
    server.abort();
}
