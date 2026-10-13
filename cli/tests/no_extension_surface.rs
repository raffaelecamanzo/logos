//! A build with no extension serves no chat or wiki surface, and still starts on a
//! chat- or wiki-configured project (S-645, S-651, [FR-DS-05], CR-213, CR-214).
//!
//! The build is the default one, the build this repository ships: the composition
//! (`cli/src/composition.rs`) registers nothing. It needs the web surface, so this
//! file compiles to nothing in the slim build. `scripts/gate.sh` and CI run it with
//! every other `logos` suite.
//!
//! Every surface is read the way `surface_snapshots.rs` reads it — through the same
//! readers (`support/surface.rs`) over the host this build's `main` composes — and
//! held to two things:
//!
//! - **nothing on it names chat or the wiki** ([`names_chat_or_wiki`]), except an
//!   entry of [`KNOWN_EXCEPTIONS`], each listed with its reason and the decision that
//!   made it; and
//! - **it is the committed default-build surface less exactly its chat and wiki
//!   entries** — read from the `surface_snapshots/default/` files, never written
//!   here — so nothing else went missing with them. When chat and the wiki left this
//!   repository those files were regenerated to exactly that (S-651).
//!
//! The SPA's half is `web/ui/src/nav.snapshot.test.ts`, which boots the shipped
//! bundle's registrations over the body this build serves for `GET
//! /api/v1/extensions` — the fixture [`it_registers_no_extension_and_says_so`]
//! generates and guards.
//!
//! [FR-DS-05]: ../../docs/specs/requirements/FR-DS-05.md

#![cfg(feature = "ui")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use logos_core::federation::Backing;
use logos_core::Engine;
use logos_ext::ExtensionHost;
use serde_json::Value;
use tower::ServiceExt;

#[path = "support/surface.rs"]
mod surface;
use surface::{
    composed_web_surface, configured_project, get_json, help, mcp_rosters, pretty, render_census,
    RouteEntry,
};

/// The binary's composition, compiled from the file `main` composes from.
#[path = "../src/composition.rs"]
mod composition;

/// The host this build's `main` composes: empty.
fn public_host() -> ExtensionHost {
    composition::host()
}

// ── The matcher and its known exceptions ───────────────────────────────────

/// Does `text` name chat or the wiki? A case-insensitive substring test — every
/// spelling the extensions use (`wiki_read`, `/api/v1/chat/threads`, `Workspace
/// Chat`, `[wiki]`) contains one of the two words, and so does anything a later
/// change could add under either name. Deliberately wide: a false positive costs a
/// listed exception, a false negative costs the guarantee.
fn names_chat_or_wiki(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("chat") || text.contains("wiki")
}

/// A surface of the no-extension build that [`KNOWN_EXCEPTIONS`] may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// A route census line: `"<METHODS> <path>"`.
    Routes,
    /// A key path of the config read-model (`GET /api/v1/config`), dot-joined.
    ConfigReadModel,
    /// A question `logos init -i` asks on a TTY.
    InitPrompts,
    /// The body of the web method guard's `405`.
    MethodGuard,
}

/// A chat- or wiki-adjacent entry the no-extension build keeps, by a recorded
/// decision. Each one must be OBSERVED on its surface — an exception that no longer
/// holds fails [`assert_only_known_exceptions`] and has to be retired here, so this
/// list cannot outlive the decisions behind it.
struct KnownException {
    surface: Surface,
    item: &'static str,
    reason: &'static str,
    decision: &'static str,
}

/// The coordinator's decision for the chat-adjacent core: Sprint 97 Autonomous
/// Decision 4 ([S-643] Limitation 1).
///
/// [S-643]: ../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
const CHAT_CORE_DECISION: &str = "docs/planning/sprints/sprint-impl-97.md#autonomous-decisions (row 4; S-643 Limitation 1)";

/// [S-644] T2's Limitations, accepted by the coordinator for S-645.
///
/// [S-644]: ../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension
const WIKI_T2_LIMITATIONS: &str = "docs/planning/sprints/sprint-impl-97.md#s-644--t2-register-the-wiki-as-an-extension (Limitations)";

/// Every chat- or wiki-adjacent entry a build with no extension still serves. Matched
/// EXACTLY, entry by entry: nothing here widens [`names_chat_or_wiki`].
const KNOWN_EXCEPTIONS: &[KnownException] = &[
    // Three core routes stay in every build. Their paths name neither word, so the
    // matcher would never flag them; they are listed so their presence is a recorded
    // decision rather than an oversight, and so retiring them retires this entry.
    KnownException {
        surface: Surface::Routes,
        item: "POST /config/secret",
        reason: "the member chat-key write; ChatConfig, resolve_chat, the secret writers and \
                 read-roots move together, with CR-214",
        decision: CHAT_CORE_DECISION,
    },
    KnownException {
        surface: Surface::Routes,
        item: "POST /api/v1/workspace/config/secret",
        reason: "the workspace chat-key write; moves with the other chat config seams in CR-214",
        decision: CHAT_CORE_DECISION,
    },
    KnownException {
        surface: Surface::Routes,
        item: "GET /api/v1/workspace/config/read-roots",
        reason: "the Workspace Chat consent banner's read; moves with the other chat config \
                 seams in CR-214",
        decision: CHAT_CORE_DECISION,
    },
    // The read-model type is the core's until CR-214 moves ChatConfig and WikiConfig:
    // `[chat]` is still read by the core, `[wiki]` is ignored but its fields still
    // serialise as defaults.
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "chat_key",
        reason: "the core still reads and masks chat.api_key without the chat extension",
        decision: CHAT_CORE_DECISION,
    },
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "effective_chat",
        reason: "the core still resolves [chat] without the chat extension",
        decision: CHAT_CORE_DECISION,
    },
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "config.parsed.chat",
        reason: "Config.chat is skip_deserializing only, so it still serialises (the read-model \
                 is unchanged in every build)",
        decision: CHAT_CORE_DECISION,
    },
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "defaults.config.chat",
        reason: "the defaults half of the same unchanged read-model type",
        decision: CHAT_CORE_DECISION,
    },
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "config.parsed.wiki",
        reason: "WikiConfig stays in logos-core until CR-214; [wiki] is ignored, its fields \
                 serialise as defaults",
        decision: WIKI_T2_LIMITATIONS,
    },
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "defaults.config.wiki",
        reason: "the defaults half of the same core read-model type",
        decision: WIKI_T2_LIMITATIONS,
    },
    KnownException {
        surface: Surface::ConfigReadModel,
        item: "effective_wiki",
        reason: "EffectiveWikiModel is a core read-model until CR-214",
        decision: WIKI_T2_LIMITATIONS,
    },
    KnownException {
        surface: Surface::MethodGuard,
        item: "the Logos web UI is read-only except the enumerated config-write/apply, chat, \
               wiki-generation, and verify routes",
        reason: "kept byte-identical in every build: user-visible text no snapshot covers \
                 (Limitation 4). The one sentence names chat as well as wiki-generation",
        decision: WIKI_T2_LIMITATIONS,
    },
];

/// Hold `observed` — every entry `surface` serves — to the rule: an entry that names
/// chat or the wiki must be a [`KNOWN_EXCEPTIONS`] entry for `surface`, and every
/// such exception must be observed.
fn assert_only_known_exceptions(surface: Surface, observed: &[String]) {
    let known: Vec<&KnownException> = KNOWN_EXCEPTIONS.iter().filter(|e| e.surface == surface).collect();
    let unexpected: Vec<&String> = observed
        .iter()
        .filter(|item| names_chat_or_wiki(item) && !known.iter().any(|e| e.item == item.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "the no-extension build's {surface:?} surface names chat or the wiki in {unexpected:?}. \
         A build without extensions serves no chat or wiki surface (FR-DS-05): remove the entry, \
         or — only by a recorded decision — list it in KNOWN_EXCEPTIONS with its reason"
    );
    for exception in known {
        assert!(
            observed.iter().any(|item| item == exception.item),
            "KNOWN_EXCEPTIONS lists {:?} on the {surface:?} surface ({}; {}), but this build no \
             longer serves it — retire the exception",
            exception.item,
            exception.reason,
            exception.decision,
        );
    }
}

/// The matcher, probed with what it must catch and its near misses.
#[test]
fn the_matcher_catches_every_spelling_and_nothing_else() {
    for named in ["wiki_read", "/api/v1/chat/threads", "Workspace Chat", "[wiki]", "WIKI", "/workspace-chat"] {
        assert!(names_chat_or_wiki(named), "{named} names chat or the wiki");
    }
    for clean in ["/api/v1/workspace/config/secret", "hook", "wik", "cha t", "/config/apply", "w-iki"] {
        assert!(!names_chat_or_wiki(clean), "{clean} names neither");
    }
    // Every exception is an exact entry on a surface this suite reads, and none is
    // listed twice.
    for (i, e) in KNOWN_EXCEPTIONS.iter().enumerate() {
        assert!(!e.reason.is_empty() && e.decision.starts_with("docs/planning/sprints/"), "{}", e.item);
        assert!(
            !KNOWN_EXCEPTIONS[i + 1..].iter().any(|o| o.surface == e.surface && o.item == e.item),
            "{} is listed twice",
            e.item
        );
    }
}

// ── The committed default-build surface ────────────────────────────────────

/// A committed default-build snapshot (`surface_snapshots/default/`). Read, never
/// written here: `surface_snapshots.rs` owns them.
fn default_snapshot(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/surface_snapshots/default").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The lines of `text` that name neither chat nor the wiki.
fn less_chat_and_wiki_lines(text: &str) -> String {
    text.lines().filter(|l| !names_chat_or_wiki(l)).map(|l| format!("{l}\n")).collect()
}

// ── GET /api/v1/extensions ──────────────────────────────────────────────────

/// The body this build serves for `GET /api/v1/extensions`, which the SPA's
/// no-extension tests boot from (`web/ui/src/nav.snapshot.test.ts`). This test is its
/// generator and its guard; regenerate with `LOGOS_UPDATE_FIXTURES=1 cargo test -p
/// logos --test no_extension_surface`.
const SERVED_FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../web/ui/src/extensions/fixtures/no-extension.real.json");

#[tokio::test]
async fn it_registers_no_extension_and_says_so() {
    assert!(public_host().active().is_empty(), "the composition registers nothing in this build");

    let project = configured_project();
    let (status, served) = get_json(&public_host(), project.path(), "/api/v1/extensions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(served["extensions"], Value::Array(Vec::new()), "no extension is listed: {served}");

    if std::env::var_os("LOGOS_UPDATE_FIXTURES").is_some() {
        std::fs::write(SERVED_FIXTURE, pretty(&served)).expect("write the fixture");
    }
    let committed: Value = std::fs::read_to_string(SERVED_FIXTURE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        committed, served,
        "the SPA's no-extension fixture drifted from this build's GET /api/v1/extensions — \
         regenerate with LOGOS_UPDATE_FIXTURES=1"
    );
}

// ── The web route census ────────────────────────────────────────────────────

fn census_items(census: &[RouteEntry]) -> Vec<String> {
    census.iter().map(|r| format!("{} {}", r.methods.join(" "), r.path)).collect()
}

#[tokio::test]
async fn the_route_census_names_no_chat_or_wiki_route() {
    let (census, admitted) = composed_web_surface(&public_host()).await;
    assert_only_known_exceptions(Surface::Routes, &census_items(&census));

    // The committed census less exactly its chat and wiki rows: nothing else is
    // missing.
    assert_eq!(render_census(&census), less_chat_and_wiki_lines(&default_snapshot("web-routes.txt")));
    assert_eq!(
        admitted.join("\n") + "\n",
        less_chat_and_wiki_lines(&default_snapshot("web-post-allow-list.txt")),
        "the POST allow-list is the default build's, with no chat or wiki write"
    );
}

/// Every chat and wiki route a build composing both extensions serves, as its route
/// census renders them: 12 chat rows (five member and five workspace thread routes,
/// `/chat`, `/workspace/chat`), five wiki reads and `/wiki/generate`. Copied from that
/// build's committed census when the extensions left this repository (S-651), so the
/// router below is asked about every one of them.
const COMPOSED_CHAT_AND_WIKI_ROUTES: &[&str] = &[
    "GET      /api/v1/chat/threads",
    "GET      /api/v1/chat/threads/:id",
    "POST     /api/v1/chat/threads/:id/delete",
    "POST     /api/v1/chat/threads/:id/rename",
    "POST     /api/v1/chat/threads/:id/truncate",
    "GET      /api/v1/wiki",
    "GET      /api/v1/wiki/asset/*path",
    "GET      /api/v1/wiki/nav",
    "GET      /api/v1/wiki/page/*slug",
    "GET      /api/v1/wiki/search",
    "GET      /api/v1/workspace/chat/threads",
    "GET      /api/v1/workspace/chat/threads/:id",
    "POST     /api/v1/workspace/chat/threads/:id/delete",
    "POST     /api/v1/workspace/chat/threads/:id/rename",
    "POST     /api/v1/workspace/chat/threads/:id/truncate",
    "GET POST /chat",
    "GET POST /wiki/generate",
    "POST     /workspace/chat",
];

/// The census is read from source plus the composed host; this asks the router
/// itself. Every chat and wiki route a composing build serves is unrouted here: a
/// `GET` finds nothing (`404`), a `POST` is refused by the method guard (`405`) —
/// whose text is the one [`KNOWN_EXCEPTIONS`] entry on that surface.
#[tokio::test]
async fn the_router_answers_no_chat_or_wiki_route() {
    let routes: Vec<(Vec<&str>, &str)> = COMPOSED_CHAT_AND_WIKI_ROUTES
        .iter()
        .map(|l| {
            let path = l.rsplit(' ').next().expect("a path");
            (l[..l.len() - path.len()].split_whitespace().collect(), path)
        })
        .collect();
    assert!(routes.iter().all(|(_, path)| names_chat_or_wiki(path)), "each row is a chat or wiki route");
    assert_eq!(routes.len(), 18, "the composed build's chat and wiki rows");

    let project = configured_project();
    let engine = std::sync::Arc::new(Engine::start(project.path()).expect("the engine starts"));
    let router = web::hosted_router(
        std::sync::Arc::new(Backing::Single(engine)),
        web::IntentToken::generate(),
        &public_host(),
    )
    .expect("the router builds");
    let mut guard_texts = Vec::new();
    for (methods, pattern) in routes {
        let path = surface::probe_path(pattern);
        for method in methods {
            let request = Request::builder()
                .method(Method::from_bytes(method.as_bytes()).expect("a method"))
                .uri(&path)
                .body(Body::empty())
                .expect("a request");
            let response = router.clone().oneshot(request).await.expect("the router answers");
            let expected = if method == "GET" { StatusCode::NOT_FOUND } else { StatusCode::METHOD_NOT_ALLOWED };
            assert_eq!(response.status(), expected, "{method} {path}");
            if method == "POST" {
                let body = response.into_body().collect().await.expect("a body").to_bytes();
                guard_texts.push(String::from_utf8(body.to_vec()).expect("UTF-8"));
            }
        }
    }
    guard_texts.sort();
    guard_texts.dedup();
    assert_only_known_exceptions(Surface::MethodGuard, &guard_texts);
}

// ── MCP tools/list ──────────────────────────────────────────────────────────

/// `snapshot` (a rendered roster) less the tools whose name names chat or the wiki.
fn less_chat_and_wiki_tools(snapshot: &str) -> String {
    let tools: Vec<Value> = serde_json::from_str(snapshot).expect("a committed roster");
    let kept = tools
        .into_iter()
        .filter(|t| !names_chat_or_wiki(t["name"].as_str().expect("a tool name")))
        .collect();
    pretty(&Value::Array(kept))
}

#[test]
fn the_mcp_tool_roster_names_no_chat_or_wiki_tool() {
    let (single, federated) = mcp_rosters(&public_host());
    for (roster, snapshot) in [(&single, "mcp-tools-single-root.json"), (&federated, "mcp-tools-federated.json")] {
        let tools: Vec<Value> = serde_json::from_str(roster).expect("a rendered roster");
        assert!(tools.len() > 20, "{snapshot}: the core roster is served ({} tools)", tools.len());
        let named: Vec<&Value> = tools.iter().filter(|t| names_chat_or_wiki(&t.to_string())).collect();
        assert!(named.is_empty(), "{snapshot}: a tool names chat or the wiki: {named:?}");
        // Every core tool exactly as the default build serves it: names, descriptions
        // and schemas.
        assert_eq!(roster, &less_chat_and_wiki_tools(&default_snapshot(snapshot)), "{snapshot}");
    }
}

// ── logos --help ────────────────────────────────────────────────────────────

/// The subcommand names `logos --help` lists, in order.
fn subcommands(help: &str) -> Vec<String> {
    help.split_once("Commands:\n")
        .expect("a Commands section")
        .1
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .filter_map(|l| l.strip_prefix("  ").and_then(|l| l.split_whitespace().next()))
        .map(str::to_string)
        .collect()
}

#[test]
fn logos_help_names_no_chat_or_wiki_command() {
    let help = help(&["--help"]);
    let named: Vec<&str> = help.lines().filter(|l| names_chat_or_wiki(l)).collect();
    assert!(named.is_empty(), "`logos --help` names chat or the wiki: {named:?}");

    // The committed default-build commands less any chat or wiki one, in the same
    // order.
    let expected: Vec<String> = subcommands(&default_snapshot("logos-help.txt"))
        .into_iter()
        .filter(|c| !names_chat_or_wiki(c))
        .collect();
    assert_eq!(subcommands(&help), expected);
    assert!(expected.len() > 20, "the core commands are listed: {expected:?}");

    for command in ["wiki", "chat"] {
        let out = Command::new(env!("CARGO_BIN_EXE_logos")).arg(command).output().expect("logos runs");
        assert_eq!(out.status.code(), Some(2), "`logos {command}` is not a command in this build");
    }
}

// ── The config read-model and init ──────────────────────────────────────────

/// Every key path of `value`, dot-joined, whose own key names chat or the wiki.
fn named_key_paths(value: &Value, prefix: &str, out: &mut Vec<String>) {
    if let Value::Object(map) = value {
        for (key, child) in map {
            let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
            if names_chat_or_wiki(key) {
                out.push(path.clone());
            }
            named_key_paths(child, &path, out);
        }
    }
}

#[tokio::test]
async fn the_config_read_model_carries_only_the_known_chat_and_wiki_keys() {
    public_host().register_with_process().expect("no claim to register");
    let project = configured_project();
    let (status, model) = get_json(&public_host(), project.path(), "/api/v1/config").await;
    assert_eq!(status, StatusCode::OK, "a [chat]/[wiki] project with a chat key loads: {model}");
    let mut keys = Vec::new();
    named_key_paths(&model, "", &mut keys);
    assert_only_known_exceptions(Surface::ConfigReadModel, &keys);
}

/// The string literals in `text`, unescaped (`\"` → `"`).
fn string_literals(text: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = text.chars();
    while chars.by_ref().any(|c| c == '"') {
        let mut literal = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => literal.extend(chars.next()),
                '"' => break,
                c => literal.push(c),
            }
        }
        literals.push(literal);
    }
    literals
}

/// The argument list of the call whose `(` ends `text`'s prefix, string literals
/// skipped over so a `)` inside one does not close it.
fn call_argument(text: &str) -> &str {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for (i, c) in text.char_indices() {
        match (in_string, c) {
            (true, _) if escaped => escaped = false,
            (true, '\\') => escaped = true,
            (true, '"') | (false, '"') => in_string = !in_string,
            (false, '(') => depth += 1,
            (false, ')') if depth == 0 => return &text[..i],
            (false, ')') => depth -= 1,
            _ => {}
        }
    }
    panic!("an `ask(` call closes")
}

/// `ask(` arguments that build their question outside the call: the argument as
/// written, the source that builds it, and the function there whose string literals
/// are the question's parts. An argument with no literal that is not listed here
/// fails the test, so no question can go unread.
const BUILT_ELSEWHERE: &[(&str, &str, &str)] = &[(
    "&shape.question(host_setup)",
    include_str!("../../logos-core/src/federation/enable.rs"),
    "pub fn question(",
)];

/// The questions `logos init` can ask: every string literal in the argument of each
/// `ask(` in the CLI library's init paths — a `format!` template included — and, for
/// an argument built elsewhere, the literals of the function that builds it. A TTY
/// prompt cannot be driven from a test, so its text is read where it is written.
#[test]
fn init_asks_no_chat_or_wiki_question() {
    let mut questions = Vec::new();
    for (file, source) in [("lib.rs", include_str!("../src/lib.rs")), ("workspace_init.rs", include_str!("../src/workspace_init.rs"))] {
        let before = questions.len();
        for (at, marker) in source.match_indices("ask(") {
            // Not a call: a longer name (`task(`), the definition (`fn ask(`), or a
            // mention in a comment.
            let line = &source[source[..at].rfind('\n').map_or(0, |n| n + 1)..at];
            if source[..at].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_')
                || line.trim_end().ends_with("fn")
                || line.contains("//")
            {
                continue;
            }
            let argument = call_argument(&source[at + marker.len()..]);
            let literals = string_literals(argument);
            if !literals.is_empty() {
                questions.extend(literals);
                continue;
            }
            let written = argument.split(',').next().expect("a first argument").trim();
            let (_, builder, function) = BUILT_ELSEWHERE
                .iter()
                .find(|(arg, _, _)| *arg == written)
                .unwrap_or_else(|| panic!("{file}: `ask({written}, …)` builds its question elsewhere — list where in BUILT_ELSEWHERE"));
            let body = &builder[builder.find(function).expect("the builder is declared")..];
            questions.extend(string_literals(&body[..body.find("\n    }\n").expect("the builder closes")]));
        }
        assert!(questions.len() > before, "{file}: no question read — the scan would check nothing there");
    }
    assert_only_known_exceptions(Surface::InitPrompts, &questions);
}

/// The two readers above, probed with the shapes a question can take.
#[test]
fn the_question_readers_see_templates_and_escaped_quotes() {
    assert_eq!(string_literals(r#"&format!("regenerate the {} wiki?", n), true"#), ["regenerate the {} wiki?"]);
    assert_eq!(string_literals(r#""use the \"wiki\" skill?", true"#), [r#"use the "wiki" skill?"#]);
    assert_eq!(call_argument(r#""a (b)?", f(x)) + 1"#), r#""a (b)?", f(x)"#);
    assert_eq!(call_argument(r#""say \")\"", true) rest"#), r#""say \")\"", true"#);
}

// ── status and serve --ui on a chat- and wiki-configured project ───────────

/// `logos --project <project> <args…>` from this build, its root pinned to the
/// fixture by a git ceiling (the fixture is not a repository).
fn logos(project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_logos"));
    command
        .arg("--project")
        .arg(project)
        .env("GIT_CEILING_DIRECTORIES", project.parent().expect("a temp parent"))
        .stdin(Stdio::null());
    command
}

/// The config notices a run printed: one per ignored table, naming it.
fn ignored_table_notices(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|l| l.contains("which no active extension claims: ignored"))
        .map(str::to_string)
        .collect()
}

/// `[wiki]` is ignored and reported, once; `[chat]` is still read by the core, so it
/// is not reported (`CHAT_CORE_DECISION`).
fn assert_reports_wiki_ignored_once(what: &str, stderr: &[u8]) {
    let notices = ignored_table_notices(stderr);
    assert_eq!(notices.len(), 1, "{what} reports the ignored table once: {notices:?}");
    assert!(notices[0].contains("declares [wiki]"), "{what}: {}", notices[0]);
}

#[test]
fn status_succeeds_on_a_chat_and_wiki_configured_project() {
    let project = configured_project();
    std::fs::write(project.path().join("lib.rs"), "pub fn f() {}\n").expect("a source file");
    let index = logos(project.path()).arg("index").output().expect("logos index runs");
    assert!(index.status.success(), "index: {}", String::from_utf8_lossy(&index.stderr));

    let status = logos(project.path()).args(["status", "--json"]).output().expect("logos status runs");
    assert!(status.status.success(), "status: {}", String::from_utf8_lossy(&status.stderr));
    let report: Value = serde_json::from_slice(&status.stdout).expect("status --json is JSON");
    // Carries what the index run said and what status read, so a failure seen
    // once in an exported tree and never reproduced explains itself next time.
    assert_eq!(
        report["indexed"],
        Value::Bool(true),
        "status after index on {}:\n{report:#}\nindex stdout: {}\nindex stderr: {}",
        project.path().display(),
        String::from_utf8_lossy(&index.stdout),
        String::from_utf8_lossy(&index.stderr)
    );
    assert_reports_wiki_ignored_once("status", &status.stderr);
}

/// A `logos serve --ui` child, killed when dropped so a failing assertion never
/// leaves it running.
struct Served(Option<Child>);

impl Served {
    fn stop(mut self) -> Output {
        let mut child = self.0.take().expect("running");
        let _ = child.kill();
        child.wait_with_output().expect("the server is reaped")
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A loopback port free at the time of asking.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").expect("a free port").local_addr().expect("an address").port()
}

/// `GET <path>` over a real socket, bounded: the status code and the body.
fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).expect("connects");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("a read timeout");
    // HTTP/1.0: the server closes after the response, and never chunks it.
    write!(stream, "GET {path} HTTP/1.0\r\nHost: {addr}\r\n\r\n").expect("the request is sent");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("the response is read");
    let response = String::from_utf8_lossy(&response).into_owned();
    let (head, body) = response.split_once("\r\n\r\n").expect("a head and a body");
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("a status code");
    (code, body.to_string())
}

#[test]
fn serve_ui_starts_on_a_chat_and_wiki_configured_project() {
    let project = configured_project();
    let port = free_port();
    let server = Served(Some(
        logos(project.path())
            .args(["serve", "--ui", "--port", &port.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("logos serve --ui starts"),
    ));
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();

    // Bounded wait for the listener; a server that exits first fails with its stderr.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut server = server;
    loop {
        if let Some(code) = server.0.as_mut().expect("running").try_wait().expect("polls") {
            let out = server.stop();
            panic!("serve --ui exited ({code}): {}", String::from_utf8_lossy(&out.stderr));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "serve --ui did not listen on {addr} within 60s");
        std::thread::sleep(Duration::from_millis(100));
    }

    let (code, _) = http_get(addr, "/");
    assert_eq!(code, 200, "the dashboard shell is served");
    let (code, body) = http_get(addr, "/api/v1/extensions");
    assert_eq!(code, 200);
    let listed: Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(listed["extensions"], Value::Array(Vec::new()), "no extension is listed: {body}");
    let (code, _) = http_get(addr, "/api/v1/status");
    assert_eq!(code, 200, "the core API answers");

    let out = server.stop();
    assert_reports_wiki_ignored_once("serve --ui", &out.stderr);
}

/// The fixture never sits under this checkout, so neither child above can resolve
/// its root to the enclosing repository.
#[test]
fn the_fixture_lives_outside_the_checkout() {
    let project = configured_project();
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("the workspace").to_path_buf();
    let fixture = project.path().canonicalize().expect("canonical");
    assert!(!fixture.starts_with(checkout.canonicalize().expect("canonical")), "{}", fixture.display());
}
