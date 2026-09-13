//! **S-401 / [CR-125] at the MCP boundary: a reachability answer carries its
//! unresolved egress residue, and both surfaces report the same one**
//! ([FR-WS-05], [BR-53], [NFR-CC-04]).
//!
//! [CR-125]'s load-bearing criterion is structural rather than behavioural: the
//! residue is **assembled once** behind [api-facade] so the MCP and CLI
//! renderings *cannot* disagree. A test can only prove that by reading both, so
//! this file asserts payload equality between
//!
//! 1. the **MCP tool**, over a live `call_tool` against a real two-member
//!    workspace, and
//! 2. the **thick-core read-model assembled exactly as the CLI arm assembles it**
//!    (`query::edges` + `query::residue` + `query::xservice_callers`), serialized
//!    the way `cli::Output::print` serializes it.
//!
//! That second half is this crate's established shape for a CLI-vs-MCP parity
//! test (`coverage_parity.rs`), and it is what the dependency direction permits:
//! the mcp crate must not depend on the cli crate, which would invert the adapter
//! direction [ADR-01] fixes (the same reason this crate ships its own harness
//! binary). The CLI *adapter's own* arm is driven end-to-end through the shipped
//! `logos` binary in `cli/tests/xservice_surface.rs`, where the empty-answer and
//! zero-residue fixtures live; between the two files every link in the chain is
//! read from a shipped artifact.
//!
//! The fixture deliberately carries **both** halves of the question — one resolved
//! cross-service edge *and* one unresolved outbound site — so neither "the answer
//! is non-empty" nor "the residue is non-zero" can pass vacuously.
//!
//! Gated on `lang-all` for the same two-grammar reason
//! `workspace_status_intake_parity.rs` gives.
//!
//! [api-facade]: ../../docs/specs/architecture/components/api-facade.md
//! [BR-53]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [CR-125]: ../../docs/requests/CR-125-an-unresolved-egress-must-not-read-as-an-absence.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
#![cfg(feature = "lang-all")]

use logos_core::federation::{query, ContractBridge};
use mcp::LogosMcp;
use serde_json::{Map, Value};

/// The federated MCP-boundary harness, shared with the other federated tests.
#[path = "support/federated.rs"]
mod federated;

use federated::{boot, call, index_member, member, registry, write, Client};

/// The `api` member's OpenAPI spec: `get` binds `web`'s axum route, so the answer
/// under test is **not** empty.
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
";

/// The `api` member's client: one **runtime-composed** call, which the HTTP
/// client-call arm captures and records one keyless `base-url-runtime` refusal
/// for (S-374) — the residue under test.
const API_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_dynamic(client: Client, url: String) {
    let _ = client.get(url).await;
}
"#;

/// The `web` member: the axum route the OpenAPI operation binds.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

/// The queried symbol — `web`'s route, the provider endpoint of the one resolved
/// cross-service edge.
const ROUTE_SYMBOL: &str = "logos . . . src/`main.rs`/route/`GET /users/{user_id}`#";

/// Both surfaces' `xservice_callers` payloads for the same query, over one
/// workspace: `(cli, mcp)`.
async fn both_surfaces(
    repo: Option<&str>,
) -> (tempfile::TempDir, Value, Value, Client, tokio::task::JoinHandle<()>) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&api, "src/client.rs", API_CLIENT);
    write(&web, "src/main.rs", AXUM_MAIN);
    index_member(&api);
    index_member(&web);

    let members = vec![member("api", &api), member("web", &web)];

    // The CLI arm's body, verbatim minus the print: discover → registry → bridge →
    // `query::*` read-model → `serde_json` (cli/src/xservice.rs, cli/src/main.rs).
    let cli = {
        let reg = registry("shop", root, members.clone());
        let bridge = ContractBridge::new();
        let edges = query::edges(&bridge, &reg);
        let residue = query::residue(&bridge, &reg);
        serde_json::to_value(query::xservice_callers(
            &reg,
            &edges,
            &residue,
            ROUTE_SYMBOL,
            None,
            repo,
        ))
        .expect("the read-model serializes")
    };

    let (client, server) = boot(registry("shop", root, members)).await;
    let mut args = Map::from_iter([("symbol".to_string(), Value::from(ROUTE_SYMBOL))]);
    if let Some(repo) = repo {
        args.insert("repo".to_string(), Value::from(repo));
    }
    let mcp = call(&client, "xservice_callers", args).await;

    (tmp, cli, mcp, client, server)
}

/// **The parity criterion ([CR-125] §4.4): the two surfaces report the same
/// residue for the same query.**
///
/// Asserted as whole-payload equality rather than field-by-field, so a residue
/// that diverged *and* a neighbouring field that diverged both fail here.
#[tokio::test]
async fn cli_and_mcp_report_the_same_residue_for_the_same_query() {
    let (_tmp, cli, mcp, client, server) = both_surfaces(None).await;

    // Guard the guard: a parity assertion over two absent blocks proves nothing,
    // and over an empty answer it would not be testing the interesting case.
    assert_eq!(
        mcp["cross_service"].as_array().map(Vec::len),
        Some(1),
        "the fixture must resolve one cross-service caller: {mcp}"
    );
    assert_eq!(
        mcp["unresolved_egress"]["unresolved_sites"], 1,
        "and must leave one outbound site unresolved: {mcp}"
    );

    assert_eq!(
        cli, mcp,
        "CLI and MCP `xservice_callers` payloads — residue included — must be \
         identical for the same query (CR-125 §4.4)"
    );

    client.cancel().await.ok();
    server.abort();
}

/// Parity holds under the scope rule too: `--repo`/`repo` narrows the residue the
/// same way on both surfaces ([FR-WS-05]).
#[tokio::test]
async fn the_two_surfaces_scope_the_residue_identically() {
    let (_tmp, cli, mcp, client, server) = both_surfaces(Some("api")).await;

    assert_eq!(
        mcp["unresolved_egress"]["scope"], "api",
        "the residue names the scope it covers: {mcp}"
    );
    assert_eq!(mcp["unresolved_egress"]["members_in_scope"], 1);
    assert_eq!(cli, mcp, "and both surfaces scope it identically");

    client.cancel().await.ok();
    server.abort();
}

/// **The shipped tool descriptions explain what the shipped payload carries.**
///
/// On this surface the description *is* the documentation — an agent reads no
/// requirement file — so a field the payload carries and the description does not
/// explain is the advertised-but-unexplained capability [NFR-CC-04] disfavours.
/// The same discipline `workspace_status_intake_parity.rs` applies to the intake
/// split, applied to the field [CR-125] adds beside it.
///
/// The **misreading** clause is the load-bearing one: an empty `cross_service`
/// list beside a residue block is exactly the payload this CR exists to stop
/// being read as "nothing reaches this".
#[test]
fn both_reachability_tool_descriptions_document_the_residue() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tools = LogosMcp::federated(registry("shop", tmp.path(), Vec::new())).list_tools();

    for tool in ["xservice_callers", "xservice_impact"] {
        let description = tools
            .iter()
            .find(|t| t.name == tool)
            .and_then(|t| t.description.clone())
            .unwrap_or_else(|| panic!("the federated roster registers `{tool}` with a description"));

        for token in [
            "`unresolved_egress`",
            "`unresolved_sites`",
            "`measured_sites`",
            "`by_reason`",
            "`no_provider_in_workspace`",
            "ABSENT EXACTLY WHEN THE RESIDUE IS ZERO",
            "Do not read it as an absence",
        ] {
            assert!(
                description.contains(token),
                "the `{tool}` description must explain {token} — the MCP surface's \
                 description IS its documentation (NFR-CC-04)"
            );
        }
    }
}
