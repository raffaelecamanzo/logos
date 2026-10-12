//! The `chat` extension as a binary composes it ([S-643]): the manifest the server
//! serves for it (the bytes the SPA mounts), and its config claims registered with
//! the process exactly as `logos_cli::run` registers them.
//!
//! This binary registers the claims process-wide (`ExtensionHost::register_with_process`),
//! so every test here loads config with chat **active** — the release build's case.
//! The other suites in this crate leave them unregistered: the core reads `[chat]`
//! itself there, as it does in a build without the extension.
//!
//! [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use logos_chat::ChatExtension;
use logos_core::config::{load_config_from_root, load_secrets_from_root, resolve_chat, ConfigError};
use logos_core::federation::Backing;
use logos_core::Engine;
use logos_ext::ExtensionHost;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// The body `GET /api/v1/extensions` serves for a host composing chat, committed
/// where the SPA reads it (`web/ui/src/test/chat.ts`, which the navigation snapshot
/// and the composed-entry suites mount chat from).
const SERVED_FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../web/ui/src/extensions/fixtures/chat.real.json");

/// The `agents` build's committed config read-model (S-639) — read, never written.
const AGENTS_CONFIG_SNAPSHOT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../cli/tests/surface_snapshots/agents/config-read-model.json");

/// The host the `logos` binary composes under `agents`, its claims registered with
/// this process as `logos_cli::run` registers them (idempotent).
fn chat_host() -> ExtensionHost {
    let host = ExtensionHost::new().register(ChatExtension::new()).expect("registers");
    host.register_with_process().expect("the chat claims are acceptable");
    host
}

async fn get_json(host: &ExtensionHost, root: &Path, uri: &str) -> Value {
    let engine = Arc::new(Engine::start(root).expect("the engine starts"));
    let router = web::hosted_router(Arc::new(Backing::Single(engine)), web::IntentToken::generate(), host)
        .expect("the router builds");
    let response = router
        .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK, "GET {uri}");
    let bytes = response.into_body().collect().await.expect("a body").to_bytes();
    serde_json::from_slice(&bytes).expect("JSON")
}

/// `value` with every object's keys sorted — key order is a `serde_json` feature
/// detail, not part of any contract here (the snapshot suite reads it the same way).
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canonical(&map[k]))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The served manifest is byte-for-byte (as values) the fixture the SPA mounts, so
/// the chat registration in the bundle is proven against what the server says.
#[tokio::test]
async fn the_served_manifest_is_the_fixture_the_spa_mounts() {
    let dir = TempDir::new().unwrap();
    let served = get_json(&chat_host(), dir.path(), "/api/v1/extensions").await;
    if std::env::var_os("LOGOS_UPDATE_FIXTURES").is_some() {
        let rendered = format!("{}\n", serde_json::to_string_pretty(&served).expect("serialize"));
        std::fs::write(SERVED_FIXTURE, rendered).expect("write the fixture");
    }
    let committed: Value = std::fs::read_to_string(SERVED_FIXTURE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        committed, served,
        "the SPA's chat manifest fixture drifted from GET /api/v1/extensions — regenerate with \
         LOGOS_UPDATE_FIXTURES=1"
    );
}

/// The project the surface snapshots' config read-model is taken over
/// (`cli/tests/surface_snapshots.rs`, `configured_project`) — copied, not shared: a
/// change there fails the comparison below rather than passing silently.
fn snapshot_project() -> TempDir {
    let dir = TempDir::new().expect("a temp project");
    let logos = dir.path().join(".logos");
    std::fs::create_dir_all(&logos).unwrap();
    std::fs::write(
        logos.join("config.toml"),
        "# surface-snapshot fixture\n\n[chat]\nmodel = \"snapshot-chat-model\"\n\n\
         [wiki]\nmodel = \"snapshot-wiki-model\"\n",
    )
    .unwrap();
    std::fs::write(logos.join("rules.toml"), "[constraints]\nmax_cycles = 0\n").unwrap();
    std::fs::write(logos.join("secrets.toml"), "[chat]\napi_key = \"sk-snapshot-key-0123\"\n").unwrap();
    dir
}

/// With chat active — its claims registered, `[chat]` validated by the extension —
/// the config read-model is the `agents` build's committed snapshot: the table and
/// the key moved owner, and nothing a client reads changed (S-643 AC1, FR-CF-09 AC1).
#[tokio::test]
async fn with_chat_active_the_config_read_model_equals_the_agents_snapshot() {
    // The `agents` build composes the wiki too, whose extension claims `[wiki]`
    // (S-644). This crate cannot name it, so its claim is registered here by the
    // core validator the extension delegates to — the agents build's claims.
    logos_core::config::register_extension_config(
        &[logos_core::config::TableClaim::new("wiki", "wiki", logos_core::config::WikiConfig::validate_table)],
        &[],
    )
    .expect("the wiki claim registers");
    let dir = snapshot_project();
    let mut text = serde_json::to_string(&get_json(&chat_host(), dir.path(), "/api/v1/config").await).unwrap();
    for root in [dir.path().to_path_buf(), dir.path().canonicalize().unwrap()] {
        text = text.replace(&root.display().to_string(), "<ROOT>");
    }
    let model: Value = serde_json::from_str(&text).unwrap();
    let snapshot: Value =
        serde_json::from_str(&std::fs::read_to_string(AGENTS_CONFIG_SNAPSHOT).expect("the committed snapshot"))
            .expect("JSON");
    assert_eq!(canonical(&model), canonical(&snapshot));
}

fn project(config: &str, secrets: Option<&str>) -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
    std::fs::write(dir.path().join(".logos/config.toml"), config).unwrap();
    if let Some(secrets) = secrets {
        std::fs::write(dir.path().join(".logos/secrets.toml"), secrets).unwrap();
    }
    dir
}

/// With chat active, the extension validates `[chat]` and its key: a bad value or an
/// unknown key fails the load (exit 2) as its error — naming the key and the file,
/// never the value — and a valid table resolves to the policy and key as written.
#[test]
fn with_chat_active_its_owner_validates_the_table_and_the_key() {
    chat_host();

    let bounds = project("[chat]\nmax_tool_calls = 0\n", None);
    let err = load_config_from_root(bounds.path()).expect_err("out of bounds");
    let file = bounds.path().join(".logos/config.toml");
    assert!(
        matches!(&err, ConfigError::InvalidValue { key, message }
            if key == "chat"
                && message.starts_with("chat.max_tool_calls: must be at least 1")
                && message.ends_with(&format!("in {})", file.display()))),
        "{err:?}"
    );
    assert_eq!(err.exit_code(), 2);

    // A key pasted into a numeric field is a schema error that never echoes it.
    let pasted = project("[chat]\nmax_tokens = \"sk-live-PASTED-9f3a\"\n", None);
    let err = load_config_from_root(pasted.path()).expect_err("not a number").to_string();
    assert!(err.contains("invalid type") && !err.contains("PASTED"), "{err}");
    // Nor where serde backticks it — a rejected provider, a key name — through the
    // resolution the chat turn renders its fault from (S-643 review, NFR-SE-07).
    for text in ["[chat]\nprovider = \"sk-live-LEAK-ENUM\"\n", "[chat]\nsk-live-LEAK-KEYNAME = 1\n"] {
        let leaky = project(text, None);
        let err = resolve_chat(leaky.path(), None).expect_err("invalid").to_string();
        assert!(err.starts_with("invalid value for chat:") && !err.contains("LEAK"), "{err}");
    }

    let typo = project("[chat]\nmodel = \"m\"\n", Some("[chat]\napi_key = \"sk-1\"\napi_kye = \"sk-SECRET-77\"\n"));
    let err = load_secrets_from_root(typo.path()).expect_err("an unknown secret key").to_string();
    assert!(err.contains("holds a key no active extension owns") && !err.contains("SECRET"), "{err}");

    let valid = project(
        "[chat]\nprovider = \"anthropic\"\nmodel = \"m\"\nmax_tool_calls = 30\n",
        Some("[chat]\napi_key = \"sk-valid-key-0042\"\n"),
    );
    let resolution = resolve_chat(valid.path(), None).expect("resolves");
    assert_eq!(resolution.policy.model.as_deref(), Some("m"));
    assert_eq!(resolution.policy.max_tool_calls, 30);
    assert_eq!(resolution.api_key(), Some("sk-valid-key-0042"));
    assert_eq!(resolution.credential.last4.as_deref(), Some("0042"));
}

/// `GET /chat` — a browser navigation to the Chat view, at the path the turn's
/// `POST` is mounted on — is answered with the SPA shell, the session's intent token
/// injected, whatever the request accepts: the extension declares it a
/// [`logos_ext::web::WebRoute::shell`] route, as the web adapter mounted it before
/// S-643. Without the extension the path is unrouted, so only an HTML navigation
/// falls back to the shell and anything else is the honest `404`.
#[tokio::test]
async fn a_navigation_to_chat_is_answered_with_the_spa_shell() {
    let dir = TempDir::new().unwrap();
    let engine = Arc::new(Engine::start(dir.path()).expect("the engine starts"));
    let request = || {
        Request::get(logos_chat::CHAT_POST_ROUTE)
            .header(axum::http::header::ACCEPT, "application/json")
            .body(Body::empty())
            .expect("a request")
    };
    let intent = web::IntentToken::generate();
    let composed = web::hosted_router(Arc::new(Backing::Single(Arc::clone(&engine))), intent.clone(), &chat_host())
        .expect("the router builds");
    let response = composed.oneshot(request()).await.expect("answers");
    assert_eq!(response.status(), StatusCode::OK);
    let html = response.into_body().collect().await.expect("a body").to_bytes();
    assert!(String::from_utf8_lossy(&html).contains(intent.as_str()), "the shell carries the session's token");

    let bare = web::hosted_router(Arc::new(Backing::Single(engine)), web::IntentToken::generate(), &ExtensionHost::new())
        .expect("the router builds");
    assert_eq!(bare.oneshot(request()).await.expect("answers").status(), StatusCode::NOT_FOUND);
}
