use std::sync::Arc;

use logos_core::federation::{Backing, EngineRegistry, Federation, Member, RegistryMode};
use logos_core::Engine;
use logos_ext::web::WebContext;
use logos_ext::Extension;
use tempfile::TempDir;

use super::routes::{chat_for, ChatState};
use super::service::ConfiguredChatService;
use super::*;

/// The member chat resolves its service against the request's member (S-250).
///
/// The rule has two halves and both matter: the **default** engine must keep the
/// injected service (the S-170 mock seam the route tests drive), while a **scoped**
/// engine must get a service bound to THAT member — otherwise a `?repo=`-scoped turn
/// would be answered from the default member's graph under another member's name
/// ([NFR-RA-05]). Moved from `web` with the routes (S-643).
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn the_member_chat_follows_the_member_scope() {
    let default_dir = TempDir::new().unwrap();
    let other_dir = TempDir::new().unwrap();
    let default = Arc::new(Engine::start(default_dir.path()).expect("default engine"));
    let other = Arc::new(Engine::start(other_dir.path()).expect("member engine"));
    let injected: Arc<dyn ChatService> = Arc::new(ConfiguredChatService::new(Arc::clone(&default), None));
    let single = Arc::new(Backing::Single(Arc::clone(&default)));

    // Unscoped (or single-root): the injected service is used verbatim — the mock
    // seam the route tests inject is never bypassed.
    assert!(Arc::ptr_eq(&chat_for(&injected, &default, Arc::clone(&default), &single), &injected));
    // Scoped to another member: a DIFFERENT service, bound to that member's engine.
    assert!(!Arc::ptr_eq(&chat_for(&injected, &default, other, &single), &injected));
}

fn federated_workspace(tmp: &TempDir) -> Arc<Backing<Engine>> {
    let members: Vec<Member> = ["api", "web"]
        .into_iter()
        .map(|name| {
            std::fs::create_dir_all(tmp.path().join(name)).unwrap();
            Member { name: name.to_string(), root: tmp.path().join(name) }
        })
        .collect();
    let federation = Federation {
        name: "shop".to_string(),
        root: tmp.path().to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
    };
    Arc::new(Backing::Federated(Box::new(EngineRegistry::new(federation, RegistryMode::Lazy))))
}

/// [S-481]: under a federated backing the member chat is still the member chat —
/// the default member's (built with the routes) and a `?repo=`-scoped member's
/// (built by `chat_for`, bound to that member) — and neither is handed any
/// cross-service reach; the reach is the workspace chat's alone, minted once
/// beside it ([S-482]). Moved from `web` with the routes (S-643).
///
/// [S-481]: ../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-482]: ../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
#[test]
fn a_federated_backing_still_binds_each_member_chat_to_its_member() {
    let tmp = TempDir::new().unwrap();
    let federated = federated_workspace(&tmp);
    let default = Arc::new(Engine::start(tmp.path().join("api")).expect("default engine"));
    let other = Arc::new(Engine::start(tmp.path().join("web")).expect("member engine"));

    let context = WebContext { engine: Arc::clone(&default), backing: Arc::clone(&federated) };
    let state = ChatState::new(&context, None, None);
    let member = state.chat();
    assert!(Arc::ptr_eq(&chat_for(member, &default, Arc::clone(&default), &federated), member));
    let scoped = chat_for(member, &default, other, &federated);
    assert!(!Arc::ptr_eq(&scoped, member));

    let workspace = state.workspace_chat().expect("a federated serve has a workspace chat");
    for chat in [member, &scoped] {
        assert!(!Arc::ptr_eq(chat, workspace), "a member turn never gets the workspace chat");
    }

    let single = WebContext {
        engine: Arc::clone(&default),
        backing: Arc::new(Backing::Single(Arc::clone(&default))),
    };
    assert!(ChatState::new(&single, None, None).workspace_chat().is_none(), "a single root has no workspace chat");
    // An injected workspace chat does not give a single root one either: its
    // routes answer the not-a-workspace `404` whatever a test injects.
    let injected: Arc<dyn ChatService> = Arc::new(ConfiguredChatService::new(Arc::clone(&default), None));
    assert!(ChatState::new(&single, None, Some(injected)).workspace_chat().is_none());
}

/// `[chat]` is validated by its owner exactly as the core validated it: the
/// schema (an unknown key is refused) and the bounds (`ChatConfig::validate`),
/// with the offending key named inside the table ([FR-CF-09] AC1).
///
/// [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
#[test]
fn the_chat_table_is_validated_by_its_schema_and_bounds() {
    let chat = ChatExtension::new();
    let table = toml_table;
    assert_eq!(chat.validate_config_table(CHAT_TABLE, &table("model = \"m\"\nprovider = \"anthropic\"")), Ok(()));
    assert_eq!(chat.validate_config_table(CHAT_TABLE, &table("")), Ok(()), "every key has a default");

    let unknown = chat.validate_config_table(CHAT_TABLE, &table("bogus = 1")).unwrap_err();
    assert!(unknown.starts_with("unknown field `…`, expected one of `provider`"), "{unknown}");
    let wrong_type = chat.validate_config_table(CHAT_TABLE, &table("max_tool_calls = \"many\"")).unwrap_err();
    assert!(wrong_type.contains("invalid type"), "{wrong_type}");
    let out_of_bounds = chat.validate_config_table(CHAT_TABLE, &table("max_tool_calls = 0")).unwrap_err();
    assert!(
        out_of_bounds.starts_with("chat.max_tool_calls: must be at least 1"),
        "the bound names its key: {out_of_bounds}"
    );
}

/// The `[chat]` table of a document holding `text` under it.
fn toml_table(text: &str) -> TomlValue {
    let document: TomlValue = format!("[chat]\n{text}\n").parse().expect("a TOML document");
    document[CHAT_TABLE].clone()
}

/// The extension owns `[chat]` and `chat.api_key` and nothing else, and declares
/// no telemetry tool: a turn's engine calls are the core's own tools under the
/// `chat` surface ([FR-OB-10]).
///
/// [FR-OB-10]: ../../docs/specs/requirements/FR-OB-10.md
#[test]
fn it_owns_the_chat_table_and_key_and_no_telemetry_tool() {
    let chat = ChatExtension::new();
    assert_eq!(chat.id(), "chat");
    assert_eq!(chat.config_tables(), ["chat"]);
    assert_eq!(chat.secret_keys(), ["chat.api_key"]);
    assert!(chat.telemetry_tools().is_empty());
}

/// The manifest's config panel edits the table the extension owns — the one
/// check `UiConfigPanel::table` has no host-side guard for yet (Sprint 96 note).
#[test]
fn every_config_panel_edits_a_table_the_extension_owns() {
    let chat = ChatExtension::new();
    let manifest = chat.ui_manifest().expect("chat has a UI");
    assert!(!manifest.config_panels.is_empty());
    for panel in &manifest.config_panels {
        assert!(chat.config_tables().contains(&panel.table), "{panel:?}");
    }
    // Every navigation entry's route is one of the views it mounts.
    for item in &manifest.nav {
        assert!(manifest.views.contains(&item.path), "{item:?}");
    }
}

/// Production code of `source`: everything before its unit-test module, `//`
/// comments stripped — prose about a marker is not a marker.
fn production_code(source: &str) -> String {
    source
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(source, |(before, _)| before)
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every boundary crossing the chat routes make names the web surface, and only it
/// ([BR-42], [FR-OB-09]): the ten thread handlers cross through `web::bridge` or
/// `web::run_blocking` and each names `Surface::Web`. The web adapter's own census
/// (`web/src/lib.rs`) reads that crate's sources only, so the sites it classified
/// before S-643 moved them here are pinned here — a moved handler naming another
/// surface would otherwise be classified by nothing.
///
/// The turn itself crosses nowhere here: its engine calls run inside `agent-core`'s
/// bridges, which attribute them to `Surface::Chat` ([FR-OB-10]); so the service
/// modules name no surface at all.
///
/// [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
/// [FR-OB-09]: ../../docs/specs/requirements/FR-OB-09.md
/// [FR-OB-10]: ../../docs/specs/requirements/FR-OB-10.md
#[test]
fn every_route_crossing_names_the_web_surface_and_the_service_names_none() {
    let routes = production_code(include_str!("routes.rs"));
    let named: Vec<&str> = routes
        .match_indices("Surface::")
        .map(|(at, marker)| {
            let rest = &routes[at + marker.len()..];
            &rest[..rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len())]
        })
        .collect();
    let crossings = routes.matches("bridge(").count() + routes.matches("run_blocking(").count();
    // Anti-vacuity, and the census itself: five member and five workspace thread
    // handlers, each one crossing.
    assert_eq!(crossings, 10, "the thread handlers' crossings");
    assert_eq!(named, ["Web"; 10], "each crossing names the web surface, and nothing else: {named:?}");

    for (file, source) in [
        ("service.rs", include_str!("service.rs")),
        ("service/configured.rs", include_str!("service/configured.rs")),
        ("service/workspace.rs", include_str!("service/workspace.rs")),
    ] {
        let code = production_code(source);
        for marker in ["Surface::", "bridge(", "run_blocking(", "in_surface("] {
            assert!(!code.contains(marker), "{file} carries `{marker}`, which nothing here classifies");
        }
    }
}
