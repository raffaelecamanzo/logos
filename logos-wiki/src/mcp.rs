//! The wiki's MCP tools ([FR-WK-09], [FR-DS-04]): `wiki_write`, `wiki_read`,
//! `wiki_search`, `wiki_status` and `wiki_materialize`, served by the MCP adapter
//! as dynamic routes beside the core roster while the `wiki` extension is active.
//!
//! They were `#[tool]` methods on the MCP server until the wiki became an
//! extension (S-644). Each keeps its name, its description and its input schema
//! byte for byte — the schema is generated from the same parameter types by the
//! same rmcp function the `#[tool]` macro calls — and each is still one
//! [`WikiEngine`] call, the one its CLI twin makes, so the two payloads stay
//! byte-identical ([NFR-CC-01]). `wiki delete` and `wiki skill` stay CLI-only:
//! destructive and install operations stay off the agent surface.
//!
//! [FR-WK-09]: ../../docs/specs/requirements/FR-WK-09.md
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

use std::sync::Arc;

use logos_core::Engine;
use logos_ext::mcp::{parse_arguments, McpTool};
use rmcp::handler::server::common::{schema_for_empty_input, schema_for_type};
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars;
use serde::Deserialize;
use serde_json::Value;

use crate::WikiEngine;

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct WikiWriteParams {
    /// The page slug (path-like: lowercase/digit/`-`/`_` segments, FR-WK-02).
    pub slug: String,
    /// The page title.
    pub title: String,
    /// The markdown body, stored byte-verbatim (1 MiB cap, FR-WK-02).
    pub body: String,
    /// Anchor entity ids: `file:<path>` or `symbol:<symbol>` (default none).
    #[serde(default)]
    pub anchors: Vec<String>,
    /// The mandatory non-empty generator label (FR-WK-02).
    pub generator: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct WikiReadParams {
    /// The slug to read.
    pub slug: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub(crate) struct WikiSearchParams {
    /// The search query (omit with `list: true` to enumerate all pages).
    pub query: Option<String>,
    /// Enumerate all pages instead of searching (default false).
    pub list: Option<bool>,
}

/// The tool's `tools/list` definition, as the `#[tool]` macro built it.
fn definition(name: &'static str, description: &'static str, schema: Arc<JsonObject>) -> Tool {
    Tool::new_with_raw(name, Some(description.into()), schema)
}

/// Serialise a read-model as the tool's JSON content.
fn json<T: serde::Serialize>(value: T) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// The five wiki tools, in the order the MCP server declared them.
pub(crate) fn tools() -> Vec<McpTool> {
    vec![
        McpTool::new(
            definition("wiki_write", WIKI_WRITE, schema_for_type::<WikiWriteParams>()),
            |engine: &Engine, arguments| {
                let p: WikiWriteParams = parse_arguments(arguments)?;
                json(engine.wiki_write(&p.slug, &p.title, &p.body, &p.anchors, &p.generator)?)
            },
        ),
        McpTool::new(
            definition("wiki_read", WIKI_READ, schema_for_type::<WikiReadParams>()),
            |engine: &Engine, arguments| {
                let p: WikiReadParams = parse_arguments(arguments)?;
                json(engine.wiki_read(&p.slug)?)
            },
        ),
        McpTool::new(
            definition("wiki_search", WIKI_SEARCH, schema_for_type::<WikiSearchParams>()),
            |engine: &Engine, arguments| {
                let p: WikiSearchParams = parse_arguments(arguments)?;
                json(engine.wiki_search(p.query.as_deref().unwrap_or(""), p.list.unwrap_or(false))?)
            },
        ),
        McpTool::new(
            definition("wiki_status", WIKI_STATUS, schema_for_empty_input()),
            |engine: &Engine, _| json(engine.wiki_status()?),
        ),
        McpTool::new(
            definition("wiki_materialize", WIKI_MATERIALIZE, schema_for_empty_input()),
            |engine: &Engine, _| json(engine.wiki_materialize()?),
        ),
    ]
}

const WIKI_WRITE: &str = "Write (upsert) a source-wiki page by slug (FR-WK-02): byte-verbatim markdown body (1 MiB cap), write-time anchor resolution to content hashes, write-time HEAD tag, and a MANDATORY non-empty generator label. Anchors are `file:<repo-relative-path>` or `symbol:<canonical-symbol>`; an unknown anchor / empty generator / over-cap body is rejected loudly with the store left byte-identical. The gate-immune wiki store (BR-29) — never moves the quality signal.";
const WIKI_READ: &str = "Read a source-wiki page by slug (FR-WK-04) with MANDATORY provenance no surface may omit: the generator label, the written-at HEAD commit, per-anchor freshness (fresh/stale/missing, computed against the current tree — no sync needed), and the fixed 'generated content — not extracted by Logos' marker. A miss (or an all-anchors-gone auto-prune) returns null. Wiki prose is generated content, never extracted fact.";
const WIKI_SEARCH: &str = "FTS5 bm25 search over source-wiki page titles and bodies (FR-WK-05), indexed inside wiki.db so it survives `index`. Every hit carries its staleness flag and provenance summary (generator, HEAD). `list: true` enumerates all pages (slug-ordered) instead of searching. Offline; no vectors (NFR-SE-01). A pure read — never prunes.";
const WIKI_STATUS: &str = "Source-wiki store summary + regeneration work-list (FR-WK-06): page/stale/missing counts and the freshness fraction, the pruned-orphan log, and the work-list driving regeneration — stale pages, missing-anchor pages, and page-worthy entities lacking a page (modules, top-level files, and — only when the swe-skills doc graph is present — Requirement/Adr/Story nodes). Logos discovers deterministically; the agent writes the pages.";
const WIKI_MATERIALIZE: &str = "Deterministically materialize the presented tier (FR-WK-20, CR-062): in SRS mode, assembles each present Design/Specs category and the single-file Architecture page from the project's authored `docs/specs/**` sources into `wiki.db` with `generator = \"logos:doc-present\"`, then runs the reconciliation sweep. Pure local-FS reads + `wiki.db` writes — no LLM, no network (NFR-SE-01); byte-identical on re-run. Outside SRS mode (Case 2) this is a no-op returning the empty summary.";
