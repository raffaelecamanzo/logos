//! **S-377 / [CR-120] at the MCP boundary: the intake split, on the shipped
//! tool** ([FR-WS-05], [NFR-CC-04]).
//!
//! The read-model is unit-tested in `logos_core::federation::coverage` and driven
//! end-to-end through the `logos` binary in `cli/tests/xservice_surface.rs`. What
//! neither covers is the **MCP surface's own two contracts**, and the
//! surface-parity rule ([FR-WS-05]) makes both of them this story's business:
//!
//! 1. **the payload** — `workspace_status` must carry `intake` on every row and
//!    the `by_intake` split, asserted through a live `call_tool` rather than
//!    inferred from the fact that the adapter delegates to `query::workspace_status`.
//!    A `#[serde(skip)]` or a projection introduced in the adapter would satisfy
//!    every core test and still leave this surface blind, which is the exact class
//!    of divergence a parity test exists to catch;
//! 2. **the tool description** — on this surface the description *is* the
//!    documentation. An agent reads no requirement file, so a field the payload
//!    carries and the description does not explain is the advertised-but-unexplained
//!    capability [NFR-CC-04] disfavours. This is the same discipline
//!    `federation::coverage`'s `readable_reason_surfaces` applies to the unbound
//!    reason vocabulary, applied to the field the reasons now sit beside.
//!
//! Both are read from the **shipped artifact**: the description from
//! [`LogosMcp::list_tools`], the payload from a real `call_tool` over a real
//! two-member workspace — never from a copy of either, following [S-372]'s parity
//! precedent.
//!
//! The harness (fixture writes, member indexing, registry construction,
//! in-process rmcp client) is the shared `support/federated.rs`, extracted at this
//! story's review from `reachability_bound.rs` — the other federated
//! MCP-boundary test in this crate, and until now the only copy. This crate
//! already records why a copied guard is the wrong answer (`support/roster.rs`),
//! and a six-item harness twin has a worse failure mode than the one number that
//! precedent is about: a copy that drifts in *how* it indexes passes for reasons
//! the original does not share.
//!
//! Gated on `lang-all` (this crate's default), because the fixture needs **two**
//! grammars: the Rust one for the client calls that make the invocation
//! population, and the OpenAPI one for the operations that make the
//! contract-surface population. The same gate `cli/tests/xservice_surface.rs`
//! carries, for the same reason. Note that this crate's `lang-all` does *not*
//! imply `lang-rust` — they are two independent forwards to `logos-core` — so
//! gating on `lang-rust` would silently filter every test out under the default
//! feature set, and a filtered-out test reads exactly like a passing one.
//!
//! [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! # Extended by S-376: the same two contracts, for the resolved-edge headline
//!
//! S-376 retires `bound_ratio` and makes `resolved_cross_service_edges` /
//! `egress_resolution` the headline, and [BR-51] requires the count and the rate
//! to travel together on **every** surface — so this boundary owes exactly the
//! two guarantees above for a second vocabulary.
//!
//! Those assertions live **here rather than in a file of their own**, and the
//! reason is this crate's own recorded one: a second parity file would need its
//! own copy of the two-population fixture below (an OpenAPI spec, a client with
//! one static and one runtime-composed call, an axum route), and a fixture twin
//! is the failure this crate already documents at `support/roster.rs` and that
//! sprint 66's risk register names for `coverage.rs`. The two stories' assertions
//! are about the *same payload from the same call*, so they share the fixture and
//! are separated by section instead. The file's name predates S-376; its subject
//! is the `workspace_status` coverage payload at this boundary.
//!
//! [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
//!
//! [S-372]: ../../docs/planning/journal.md#s-372-coverage-rows-name-the-provider-they-bound-and-the-candidates-they-tied-between
#![cfg(feature = "lang-all")]

use mcp::LogosMcp;
use serde_json::{Map, Value};

/// The federated MCP-boundary harness, shared with `reachability_bound.rs`.
#[path = "support/federated.rs"]
mod federated;

use federated::{boot, call, index_member, member, registry, write, Client};

/// The `api` member's OpenAPI spec — the **contract-surface** population: `get`
/// matches `web`'s axum route across the `{user_id}`/`{id}` param drift (bound),
/// `delete` has no provider anywhere (no-provider-in-workspace).
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
    delete:
      summary: Delete a user
";

/// The `api` member's client — the **invocation** population: one **static** call
/// (binds `web`'s route, an `invocation`-intake bound row) and one
/// **runtime-composed** call (refused by the arm, which records one keyless
/// ledger row since S-374 — an `invocation`-intake unbound row).
const API_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}

pub async fn fetch_dynamic(client: Client, url: String) {
    let _ = client.get(url).await;
}
"#;

/// The `web` member: an axum route, the provider the static call binds.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

/// The `workspace_status` payload, as the tool actually returns it.
async fn workspace_status(client: &Client) -> Value {
    call(client, "workspace_status", Map::new()).await
}

/// **The MCP payload carries `intake` on every row and the split beside the
/// headline** — over a workspace that genuinely has both populations, so neither
/// half of the assertion can pass vacuously.
#[tokio::test]
async fn workspace_status_mcp_reports_intake_on_every_row_and_splits_the_counts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&api, "src/client.rs", API_CLIENT);
    write(&web, "src/main.rs", AXUM_MAIN);
    index_member(&api);
    index_member(&web);

    let (client, server) =
        boot(registry("shop", root, vec![member("api", &api), member("web", &web)])).await;
    let status = workspace_status(&client).await;
    let coverage = &status["coverage"];
    let references = coverage["references"].as_array().expect("classified references");

    // Guard the guard: the fixture must reach this surface with both populations,
    // or "every row carries it" is a claim about one of them.
    let mut intakes: Vec<&str> =
        references.iter().map(|r| r["intake"].as_str().unwrap_or("")).collect();
    intakes.sort_unstable();
    intakes.dedup();
    assert_eq!(
        intakes,
        ["contract-surface", "invocation"],
        "the fixture must reach this surface with both populations: {references:?}"
    );

    for row in references {
        assert!(
            row["intake"].is_string(),
            "every row carries `intake` at the MCP boundary, in every state: {row}"
        );
    }

    let split = &coverage["by_intake"];
    assert!(
        split.is_object(),
        "the coverage summary carries the intake split: {coverage}"
    );
    assert_eq!(
        split["invocation"]["bound"], 1,
        "the static client call binds web's route: {coverage}"
    );
    assert_eq!(
        split["invocation"]["unbound"], 1,
        "the runtime-composed call's recorded refusal (S-374): {coverage}"
    );
    assert_eq!(
        split["contract_surface"]["bound"], 1,
        "the OpenAPI `get` operation binds the same route — a CONTRACT-SURFACE \
         binding, counted apart from the invocation one beside it: {coverage}"
    );
    assert_eq!(
        split["contract_surface"]["no_provider_in_workspace"], 1,
        "the `delete` operation has no provider anywhere: {coverage}"
    );

    // The split sums to the headline on this surface too — the property that keeps
    // a split from ever reporting less than the counters it sits beside.
    for field in ["bound", "ambiguous", "unbound", "no_provider_in_workspace"] {
        let summed = split["contract_surface"][field].as_u64().expect(field)
            + split["invocation"][field].as_u64().expect(field);
        assert_eq!(
            Some(summed),
            coverage[field].as_u64(),
            "`{field}`: split must sum to the headline: {coverage}"
        );
    }

    client.cancel().await.ok();
    server.abort();
}

/// **The shipped tool description explains what the shipped payload carries.**
///
/// Read off [`LogosMcp::list_tools`] — the same static router the roster
/// byte-identity test reads — so this cannot pass against a description that was
/// edited somewhere else.
#[test]
fn the_workspace_status_tool_description_documents_the_intake_split() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tools = LogosMcp::federated(registry("shop", tmp.path(), Vec::new())).list_tools();
    let description = tools
        .iter()
        .find(|t| t.name == "workspace_status")
        .and_then(|t| t.description.clone())
        .expect("the federated roster registers `workspace_status` with a description");

    for token in [
        // The field itself, and both of its wire tokens — an agent that cannot
        // tell the two populations apart cannot use the split.
        "`intake`",
        "`contract-surface`",
        "`invocation`",
        // The split, and how to join it to the rows.
        "`coverage.by_intake`",
        "by_intake.contract_surface",
        "by_intake.invocation",
    ] {
        assert!(
            description.contains(token),
            "the `workspace_status` description must explain {token} — the MCP \
             surface's description IS its documentation (NFR-CC-04)"
        );
    }

    // And it must not still describe `intake` as optional: an agent told the field
    // may be absent will keep reading its presence as "this row bound something",
    // which is precisely the misreading CR-120 was filed against.
    assert!(
        !description.contains("All three fields are OPTIONAL"),
        "`intake` is no longer one of the CR-118 optional trio (S-377): {description}"
    );
}

// ── S-376 / CR-120 / BR-51: the headline is a resolved-edge count ────────────

/// **The MCP payload publishes the resolved-edge headline with its rate, and no
/// longer publishes the retired bound-ratio** ([CR-120] AC1/AC3, [BR-51]).
///
/// Over the same two-population fixture, because that is the shape where the
/// pooled `bound` and the resolved-edge count genuinely differ: the OpenAPI
/// operation binds AND the static client call binds, so `bound: 2` while only
/// **one** of them is a resolved cross-service edge. An adapter that republished
/// the pooled count under the new name would pass a single-population fixture and
/// fail here, which is the whole reason the fixture is shared.
///
/// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[tokio::test]
async fn workspace_status_mcp_publishes_the_resolved_edge_headline_with_its_rate() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let api = root.join("api");
    let web = root.join("web");
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&api, "src/client.rs", API_CLIENT);
    write(&web, "src/main.rs", AXUM_MAIN);
    index_member(&api);
    index_member(&web);

    let (client, server) =
        boot(registry("shop", root, vec![member("api", &api), member("web", &web)])).await;
    let status = workspace_status(&client).await;
    let coverage = &status["coverage"];

    // AC1 at this boundary: the retired key is gone under all three spellings.
    for retired in ["bound_ratio", "bound_ratio_measured", "bound_ratio_summary"] {
        assert!(
            coverage.get(retired).is_none(),
            "`{retired}` must be absent from the MCP payload (CR-120 AC1): {coverage}"
        );
    }

    // The headline: one resolved edge, not the pooled `bound: 2`.
    assert_eq!(coverage["bound"], 2, "{coverage}");
    assert_eq!(
        coverage["resolved_cross_service_edges"], 1,
        "only the captured call site is a resolved cross-service edge — the OpenAPI \
         operation's match is documentation conformance: {coverage}"
    );
    // …and its rate, over the invocation population's own denominator (the bound
    // call plus the runtime-composed call's recorded refusal).
    assert_eq!(coverage["egress_resolution"], 0.5, "{coverage}");
    assert_eq!(coverage["egress_resolution_measured"], 2, "{coverage}");
    assert_eq!(
        coverage["resolved_edges_summary"],
        "1 resolved cross-service edges; egress resolution 0.500 (1 of 2 egress sites resolved)",
        "the count and the rate arrive as one composed line (BR-51): {coverage}"
    );
    // The renamed ratio is still published, still never bare.
    assert_eq!(coverage["spec_conformance_measured"], 3, "{coverage}");
    assert!(
        coverage["spec_conformance_summary"].is_string(),
        "the composed spec-conformance line rides too: {coverage}"
    );

    client.cancel().await.ok();
    server.abort();
}

/// **The shipped tool description explains the headline it now carries** — the
/// second half of the same duty, on the surface where the description *is* the
/// documentation ([NFR-CC-04]).
///
/// The negative half matters as much as the positive one: a description that still
/// told an agent to read `coverage.bound_ratio` would send it to a key the payload
/// no longer has, and an agent has no requirement file to correct it with.
#[test]
fn the_workspace_status_tool_description_documents_the_resolved_edge_headline() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let tools = LogosMcp::federated(registry("shop", tmp.path(), Vec::new())).list_tools();
    let description = tools
        .iter()
        .find(|t| t.name == "workspace_status")
        .and_then(|t| t.description.clone())
        .expect("the federated roster registers `workspace_status` with a description");

    for token in [
        // The headline and its rate — named together, because BR-51's duty is the
        // pairing and a description that explained only one would licence exactly
        // the reading the pairing exists to prevent.
        "`coverage.resolved_cross_service_edges`",
        "`coverage.egress_resolution`",
        "`coverage.egress_resolution_measured`",
        "`coverage.resolved_edges_summary`",
        // The renamed ratio and its two disclosure fields.
        "`coverage.spec_conformance_ratio`",
        "`coverage.spec_conformance_measured`",
        "`coverage.spec_conformance_summary`",
    ] {
        assert!(
            description.contains(token),
            "the `workspace_status` description must explain {token} — on this \
             surface the description IS the documentation (NFR-CC-04)"
        );
    }

    // And it must state the retirement rather than leave the old key described.
    assert!(
        description.contains("`bound_ratio` IS NO LONGER SENT"),
        "the description must say the retired key is gone, so an agent does not \
         keep reaching for it: {description}"
    );

    // The reachability rider's description owes the same pairing — it is the
    // fourth rendering CR-111 found missing the denominator, and it now carries
    // the successor headline too.
    let reachability = tools
        .iter()
        .find(|t| t.name == "workspace_reachability")
        .and_then(|t| t.description.clone())
        .expect("the federated roster registers `workspace_reachability`");
    for token in [
        "`coverage.resolved_cross_service_edges`",
        "`coverage.egress_resolution`",
        "`coverage.spec_conformance_ratio`",
    ] {
        assert!(
            reachability.contains(token),
            "the `workspace_reachability` rider description must explain {token}"
        );
    }
}
