//! **S-401 / [CR-125]: the unresolved egress residue is advisory — it moves no
//! gate verdict and no baseline** ([FR-WS-05], [NFR-CC-04], [ADR-53]).
//!
//! The residue's *content* is unit-tested in
//! `logos_core::federation::residue`, its rendering end-to-end in
//! `cli/tests/xservice_surface.rs`, and its two-surface parity in
//! `mcp/tests/xservice_residue_parity.rs`. What none of those can assert is the
//! criterion that says what the residue must **not** do: the advisory-tier rule
//! every workspace read-model is held to, and which [CR-125] restates because a
//! figure this alarming is exactly the kind that gets wired into a gate later.
//!
//! Asserted the way the sibling advisory guard does it
//! (`xservice_reachability_broker_promotion.rs`): take the member's gated verdict
//! and its store bytes **before**, assemble the residue, take them again, and
//! require byte-identity. A gate that read the residue, or a residue walk that
//! wrote to a member store, fails here.
//!
//! [CR-125]: ../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-53]: ../../docs/specs/architecture/decisions/ADR-53.md
#![cfg(feature = "lang-all")]

use std::path::{Path, PathBuf};

use logos_core::federation::{
    ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::Engine;
use tempfile::TempDir;

#[path = "support/bridge_reads.rs"]
mod bridge_reads;

/// The `api` member's client: one runtime-composed call the HTTP client-call arm
/// captures and refuses (S-374) — a guaranteed non-zero residue.
const API_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_dynamic(client: Client, url: String) {
    let _ = client.get(url).await;
}
"#;

const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

fn index(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// The gated facts a change must not move, as one comparable string.
fn gated_verdict(engine: &Engine) -> String {
    let g = engine.gate(None, false, true).expect("gate");
    serde_json::to_string(&serde_json::json!({
        "passed": g.passed,
        "signal": g.signal,
        "baseline_signal": g.baseline_signal,
        "regressions": g.regressions,
        "test_function_count": g.test_function_count,
    }))
    .expect("verdict serializes")
}

fn db_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(".logos/logos.db")).expect("member store is readable")
}

/// Assembling the residue leaves every member's gate verdict, baseline and store
/// byte-for-byte unchanged ([FR-WS-05] — the tier is advisory, never a gate input).
#[test]
fn assembling_the_residue_moves_no_gate_verdict_and_writes_no_member_store() {
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path();
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "src/client.rs", API_CLIENT);
    write(&web, "src/main.rs", AXUM_MAIN);
    index(&api);
    index(&web);

    // Baseline each member, then read the verdict that baseline produces.
    let before: Vec<String> = [&api, &web]
        .iter()
        .map(|root| {
            let engine = Engine::start(root).expect("engine starts");
            engine.gate(None, true, true).expect("gate --save");
            gated_verdict(&engine)
        })
        .collect();
    let stores_before = [db_bytes(&api), db_bytes(&web)];

    let federation = Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members: vec![
            Member { name: "api".to_string(), root: api.clone() },
            Member { name: "web".to_string(), root: web.clone() },
        ],
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
    };
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let bridge = ContractBridge::new();
    let (_edges, residue, _) = bridge.reachability_read(&registry);

    // Guard the guard: a residue of zero would make every assertion below pass
    // over a read that did nothing.
    let unresolved: u64 = residue.members.iter().map(|m| m.unresolved_sites).sum();
    assert_eq!(
        unresolved, 1,
        "the fixture must carry a real residue: {:?}",
        residue.members
    );

    bridge_reads::assert_narrowed_read_changes_no_answer(&registry);

    // Drop the registry so every member engine is closed before re-opening.
    drop(residue);
    drop(_edges);
    drop(registry);

    let after: Vec<String> = [&api, &web]
        .iter()
        .map(|root| gated_verdict(&Engine::start(root).expect("engine starts")))
        .collect();
    assert_eq!(
        before, after,
        "assembling the advisory residue must leave every member's gate verdict \
         and baseline byte-for-byte unchanged (FR-WS-05, ADR-53)"
    );
    assert_eq!(
        stores_before,
        [db_bytes(&api), db_bytes(&web)],
        "the residue is a pure read: no member store is written (ADR-52)"
    );
}
