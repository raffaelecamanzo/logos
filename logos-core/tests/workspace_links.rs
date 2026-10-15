//! End-to-end test for `[[links]]` resolution on `workspace status` (S-708,
//! [CR-224], [FR-WS-41]).
//!
//! The unit tests in `federation::links` drive every shape over fake member
//! engines. This drives the **real** lookup: four member repositories indexed by
//! their own [`Engine`] over the real `tree-sitter-rust` grammar, behind an
//! [`EngineRegistry`], each link endpoint resolved by its member's own store —
//! a canonical symbol, or a name. It proves:
//!
//! - every declared link is reported on `workspace status`, in manifest order,
//!   with its verdict: `valid` (one `asserted` edge), `redundant` (naming the
//!   evidence edge the bridge already binds), and each invalid reason —
//!   `no-match`, `ambiguous` (with its candidates), `same-member`,
//!   `unknown-member` (an undeclared and an excluded member), `member-degraded`;
//! - the links change **nothing else** on the payload: beyond their own rows,
//!   the `asserted_edges` tally beside the coverage headline and the
//!   `asserted_by` pointer on a residue row a valid link covers (S-709), every
//!   key — coverage, its resolved-edge count, egress resolution and spec
//!   conformance included — serializes byte-identically with and without them
//!   ([BR-75]);
//! - a workspace without `[[links]]` carries no `links` key at all.
//!
//! [CR-224]: ../../docs/requests/CR-224-asserted-cross-service-links.md
//! [FR-WS-41]: ../../docs/specs/requirements/FR-WS-41.md
//! [BR-75]: ../../docs/specs/software-spec.md#327-workspace-federation
//!
//! Gated on the Rust grammar so a build excluding it does not run it.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    workspace_status, ContractBridge, EngineRegistry, Exclusion, Federation, Link, LinkRelation,
    Member, RegistryMode,
};
use logos_core::Engine;
use serde_json::Value;

/// `web`: a static client call `GET /users/{id}` from `fetch_user` (the bridge
/// binds it to `api`'s route — the evidence edge), and `fetch_cart`, whose call
/// is runtime-composed and binds nothing (the coupling a link declares).
const WEB_LIB: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}

pub async fn fetch_cart(client: Client, url: String) {
    let _ = client.get(url).await;
}
"#;

/// `api`: the axum route `fetch_user` binds, and the `get_cart` handler a link
/// names.
const API_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

pub async fn get_cart() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

/// `api`'s README: a heading named like the handler, and one named like nothing
/// in the code. Neither is a declaration a coupling can reach, so neither may
/// answer for — or make ambiguous — a link endpoint.
const API_README: &str = "# get_cart\n\nThe cart handler.\n\n# Checkout\n\nThe checkout flow.\n";

/// `other`: two functions named `get_cart` — a name that is not exactly one.
const OTHER_LIB: &str = r#"
pub fn get_cart() {}

pub mod nested {
    pub fn get_cart() {}
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own `.logos/logos.db`, then drop the engine so
/// the store is closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn http(from: &str, to: &str) -> Link {
    Link {
        relation: LinkRelation::HttpCall,
        from: from.to_string(),
        to: to.to_string(),
    }
}

/// The indexed fixture: `web`, `api`, `other`, and `broken`, whose store is not
/// a database.
fn fixture(tmp: &Path) -> Vec<Member> {
    let web = tmp.join("web");
    let api = tmp.join("api");
    let other = tmp.join("other");
    let broken = tmp.join("broken");
    write(&web, "src/lib.rs", WEB_LIB);
    write(&api, "src/main.rs", API_MAIN);
    write(&api, "README.md", API_README);
    write(&other, "src/lib.rs", OTHER_LIB);
    for root in [&web, &api, &other] {
        index_member(root);
    }
    write(&broken, ".logos/logos.db", "this is not a sqlite database");
    vec![
        member("web", &web),
        member("api", &api),
        member("other", &other),
        member("broken", &broken),
    ]
}

fn federation(root: &Path, members: Vec<Member>, links: Vec<Link>) -> Federation {
    Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links,
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
        exclusion: Exclusion {
            globs: vec!["legacy*".to_string()],
            excluded: vec!["legacy".to_string()],
            conflicts: Vec::new(),
        },
    }
}

fn status(federation: Federation) -> Value {
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    serde_json::to_value(workspace_status(&registry)).expect("status serializes")
}

/// Every link shape, through the real lookup, on the real `workspace status`.
#[test]
fn every_link_shape_is_reported_on_workspace_status_and_nothing_else_moves() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let members = fixture(&root);

    // The evidence edge a redundant link duplicates, read off the real bridge so
    // the link names it by its canonical symbols.
    let evidence = {
        let registry = EngineRegistry::<Engine>::new(
            federation(&root, members.clone(), Vec::new()),
            RegistryMode::Lazy,
        );
        let edges = ContractBridge::new().edges(&registry);
        assert_eq!(edges.len(), 1, "the fixture binds one evidence edge: {edges:?}");
        edges[0].clone()
    };
    let redundant = http(
        &format!("web::{}", evidence.from.symbol.as_str()),
        &format!("api::{}", evidence.to.symbol.as_str()),
    );

    let links = vec![
        http("web::fetch_cart", "api::get_cart"),
        redundant,
        http("web::fetch_cart", "api::get_kart"),
        http("web::fetch_cart", "other::get_cart"),
        http("web::fetch_cart", "web::fetch_user"),
        http("web::fetch_cart", "ghost::get_cart"),
        http("web::fetch_cart", "legacy::get_cart"),
        http("web::fetch_cart", "broken::get_cart"),
        // A documentation heading and a whole file are never endpoints.
        http("web::fetch_cart", "api::Checkout"),
        http("web::fetch_cart", "api::README.md"),
    ];
    let without = status(federation(&root, members.clone(), Vec::new()));
    let with = status(federation(&root, members, links));

    let rows = with["links"].as_array().expect("declared links are reported");
    let verdicts: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|row| (row["status"].as_str().unwrap(), row["reason"].as_str()))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("valid", None),
            ("redundant", None),
            ("invalid", Some("no-match")),
            ("invalid", Some("ambiguous")),
            ("invalid", Some("same-member")),
            ("invalid", Some("unknown-member")),
            ("invalid", Some("unknown-member")),
            ("invalid", Some("member-degraded")),
            ("invalid", Some("no-match")),
            ("invalid", Some("no-match")),
        ],
        "{rows:#?}"
    );

    // valid: one asserted edge, `route` class, both ends resolved by name — the
    // README heading also named `get_cart` neither answers for the handler nor
    // makes it ambiguous.
    let edge = &rows[0]["edge"];
    assert_eq!(edge["intake"], "asserted");
    assert_eq!(edge["relation"], "route");
    assert_eq!(edge["from"]["member"], "web");
    assert!(edge["from"]["symbol"].as_str().unwrap().contains("fetch_cart"), "{edge}");
    assert_eq!(edge["to"]["member"], "api");
    assert!(edge["to"]["symbol"].as_str().unwrap().contains("get_cart"), "{edge}");

    // redundant: names the evidence edge, which says it is evidence.
    assert_eq!(rows[1]["duplicates"], serde_json::to_value(&evidence).unwrap());
    assert_eq!(rows[1]["duplicates"]["intake"], "invocation");

    // ambiguous: both `get_cart`s listed, none omitted.
    let candidates = &rows[3]["candidates"];
    assert_eq!(candidates["total"], 2, "{candidates}");
    assert_eq!(candidates["omitted"], 0);
    assert_eq!(candidates["symbols"].as_array().unwrap().len(), 2);

    // unknown-member: an undeclared name, and an excluded one said to be so.
    assert!(rows[5]["detail"].as_str().unwrap().contains("not a member"), "{}", rows[5]);
    assert!(rows[6]["detail"].as_str().unwrap().contains("`[workspace] exclude` removed"), "{}", rows[6]);
    // member-degraded: the endpoint is about the far side.
    assert_eq!(rows[7]["endpoint"], "to");

    // Nothing else moves: every other key is byte-identical ([BR-75]) — the
    // coverage headline, its resolved-edge count and both rates among them.
    // The links' own additive keys are taken out (in place, keeping key order):
    // their rows, the tally beside the headline, and the residue pointers.
    let mut rest = with.clone();
    rest.as_object_mut().unwrap().shift_remove("links");
    let coverage = rest["coverage"].as_object_mut().unwrap();
    assert!(coverage.shift_remove("asserted_edges").is_some(), "the tally is reported");
    let pointed = coverage["references"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter_map(|row| row.as_object_mut().unwrap().shift_remove("asserted_by"))
        .count();
    assert_eq!(pointed, 1, "the one valid link points from `fetch_cart`'s residue row");
    assert_eq!(
        serde_json::to_string(&rest).unwrap(),
        serde_json::to_string(&without).unwrap(),
        "links change nothing on the payload but their own rows"
    );
    assert!(without.get("links").is_none(), "no `[[links]]`, no `links` key");
    assert_eq!(without["coverage"]["resolved_cross_service_edges"], 1);
}
