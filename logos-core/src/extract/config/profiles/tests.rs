//! OpenAPI content-sniffed promotion tests (S-067, [CR-010], [FR-CG-03],
//! [NFR-RA-06]).
//!
//! Drives the **real** `tree-sitter-yaml` and `tree-sitter-json` grammars through
//! the artifact substrate using the embedded `plugins/{yaml,json}/plugin.toml`
//! descriptors — exactly the trees the merged S-063 data-format plugins produce —
//! and asserts the additive promotion pass over them: a version-bearing
//! `openapi:`/`swagger:` document is promoted (`ApiPath` per template,
//! `ApiOperation` per method, `ConfigFile` profile-tagged) while a non-OpenAPI
//! document beside it is not, and a JSON-bodied spec promotes identically to its
//! YAML twin.

use std::collections::BTreeMap;

use crate::extract::{extract, Facts, FileInput, SymbolContext};
use crate::model::{EdgeKind, NodeKind};
use crate::plugin::{CompiledPlugin, PluginManifest};

/// The real YAML artifact plugin, built from the embedded descriptor + grammar.
fn yaml_plugin() -> CompiledPlugin {
    let toml = include_str!("../../../../plugins/yaml/plugin.toml");
    let manifest = PluginManifest::parse("yaml/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_yaml::LANGUAGE.into();
    CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
}

/// The real JSON artifact plugin, built from the embedded descriptor + grammar.
fn json_plugin() -> CompiledPlugin {
    let toml = include_str!("../../../../plugins/json/plugin.toml");
    let manifest = PluginManifest::parse("json/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_json::LANGUAGE.into();
    CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
}

/// A version-bearing OpenAPI 3.x document, in block YAML — note the file is named
/// `api.yaml` with **no** naming convention (the sniff is by content, FR-CG-03).
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: Pet API
  version: 1.0.0
paths:
  /pets:
    get:
      summary: List pets
    post:
      summary: Create a pet
  /pets/{id}:
    get:
      summary: Get a pet
    delete:
      summary: Delete a pet
";

/// The JSON twin of [`OPENAPI_YAML`] — the same document, JSON-bodied.
const OPENAPI_JSON: &str = r#"{
  "openapi": "3.0.3",
  "info": { "title": "Pet API", "version": "1.0.0" },
  "paths": {
    "/pets": {
      "get": { "summary": "List pets" },
      "post": { "summary": "Create a pet" }
    },
    "/pets/{id}": {
      "get": { "summary": "Get a pet" },
      "delete": { "summary": "Delete a pet" }
    }
  }
}"#;

/// A non-OpenAPI document *also* named `api.yaml` — it even has a top-level
/// `paths:` key, but **no** version-bearing `openapi:`/`swagger:` key, so the
/// sniff must reject it (the negative guard, UAT-CG-02).
const NOT_OPENAPI_YAML: &str = "\
service: gateway
paths:
  enabled: true
  prefix: /api
routes:
  upstream: localhost
";

fn names_of(facts: &Facts, kind: NodeKind) -> Vec<String> {
    let mut v: Vec<String> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == kind)
        .map(|n| n.name.clone())
        .collect();
    v.sort();
    v
}

fn config_file(facts: &Facts) -> &crate::extract::NodeFact {
    facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::ConfigFile)
        .expect("exactly one ConfigFile root")
}

// ── FR-CG-03: positive promotion ──────────────────────────────────────────────

/// An OpenAPI `api.yaml` is promoted: one `ApiPath` per template, one
/// `ApiOperation` per HTTP method nested under its path, and the `ConfigFile` is
/// tagged with the `openapi` profile ([FR-CG-03]).
#[test]
fn openapi_yaml_is_promoted_with_paths_operations_and_profile_tag() {
    let facts = extract(
        &FileInput::new("api.yaml", OPENAPI_YAML),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert!(
        !facts.partial,
        "the spec parses cleanly: {:?}",
        facts.warnings
    );

    // One ApiPath per path template (by content, not filename).
    assert_eq!(
        names_of(&facts, NodeKind::ApiPath),
        vec!["/pets", "/pets/{id}"],
        "one ApiPath per path template"
    );
    // One ApiOperation per method; /pets has get+post, /pets/{id} has get+delete.
    assert_eq!(
        names_of(&facts, NodeKind::ApiOperation),
        vec!["delete", "get", "get", "post"],
        "one ApiOperation per HTTP method"
    );

    // The ConfigFile is tagged with the profile in its FTS-indexed body.
    assert_eq!(
        config_file(&facts).body.as_deref(),
        Some("openapi"),
        "the ConfigFile carries the openapi profile tag"
    );
}

/// Promotion is **additive**: the document keeps every generic `ConfigSection`
/// the S-063 walk produced (`openapi`, `info`, `paths`, …) — promotion only adds
/// the typed nodes, never altering generic extraction ([FR-CG-03]).
#[test]
fn promotion_only_adds_nodes_generic_sections_survive() {
    let promoted = extract(
        &FileInput::new("api.yaml", OPENAPI_YAML),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    let sections = names_of(&promoted, NodeKind::ConfigSection);
    // The top-level keys are still generic sections (depth-bounded walk), untouched.
    for key in ["openapi", "info", "paths"] {
        assert!(
            sections.iter().any(|s| s == key),
            "generic ConfigSection '{key}' survives promotion: {sections:?}"
        );
    }
}

/// Every promoted anchor hangs off the document by `Contains`: `ApiPath` under the
/// `ConfigFile` root, `ApiOperation` under its `ApiPath`. The promotion layer's
/// *edges* stay `Contains`-only ([CR-010] scope rule); CR-011/S-069 adds the
/// cross-artifact wiring as `refs`-ledger rows (not `edges`), so each operation
/// now also captures one `route` reference the resolution pass binds ([FR-CG-09]).
#[test]
fn promoted_nodes_form_a_contains_tree_and_capture_route_references() {
    let facts = extract(
        &FileInput::new("api.yaml", OPENAPI_YAML),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    for e in &facts.edges {
        assert_eq!(e.kind, EdgeKind::Contains, "the layer is Contains-only");
    }

    // CR-011/S-069: each promoted ApiOperation captures a `route` reference —
    // an `ArtifactBinding` + `Path` under the `route` relation, rendered
    // `"METHOD /template"` — into the refs ledger (never into `edges`).
    for r in &facts.refs {
        assert_eq!(r.kind, EdgeKind::ArtifactBinding);
        assert_eq!(r.form, crate::model::RefForm::Path);
        assert_eq!(r.relation, Some(crate::model::ArtifactRelation::Route));
    }
    let mut routes: Vec<&str> = facts.refs.iter().map(|r| r.target.as_str()).collect();
    routes.sort();
    assert_eq!(
        routes,
        vec![
            "DELETE /pets/{id}",
            "GET /pets",
            "GET /pets/{id}",
            "POST /pets",
        ],
        "one `METHOD /template` route reference per promoted operation"
    );

    let sym = |kind: NodeKind, name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("missing {kind:?} {name}"))
            .symbol
            .clone()
    };
    let file = config_file(&facts).symbol.clone();
    let pets = sym(NodeKind::ApiPath, "/pets");
    // /pets is Contains-nested under the ConfigFile root.
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.source == file && e.target == pets),
        "ApiPath hangs off the ConfigFile root"
    );
    // Both /pets operations are Contains-nested under the /pets ApiPath.
    let ops_under_pets = facts
        .edges
        .iter()
        .filter(|e| e.source == pets)
        .filter(|e| {
            facts
                .nodes
                .iter()
                .any(|n| n.symbol == e.target && n.kind == NodeKind::ApiOperation)
        })
        .count();
    assert_eq!(ops_under_pets, 2, "get+post nested under /pets");
}

// ── UAT-CG-02: the negative guard ─────────────────────────────────────────────

/// A non-OpenAPI `api.yaml` beside it is **not** promoted: no `ApiPath`/
/// `ApiOperation`, no profile tag — generic sections only. The version-key sniff
/// (not the mere presence of a `paths:` key) is what gates ([UAT-CG-02]).
#[test]
fn non_openapi_yaml_is_not_promoted() {
    let facts = extract(
        &FileInput::new("api.yaml", NOT_OPENAPI_YAML),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert!(
        names_of(&facts, NodeKind::ApiPath).is_empty(),
        "no ApiPath emitted for a non-OpenAPI document"
    );
    assert!(
        names_of(&facts, NodeKind::ApiOperation).is_empty(),
        "no ApiOperation emitted for a non-OpenAPI document"
    );
    assert_eq!(
        config_file(&facts).body,
        None,
        "the ConfigFile is not profile-tagged"
    );
    // ...but the generic layer still produced sections (it stands alone).
    assert!(
        !names_of(&facts, NodeKind::ConfigSection).is_empty(),
        "generic sections are still extracted"
    );
}

/// A document with an `openapi` key whose value is **empty** is not version-bearing
/// — so it is not promoted. Guards the "version-bearing" half of the sniff.
#[test]
fn openapi_key_without_a_version_value_is_not_promoted() {
    let src = "openapi:\npaths:\n  /a:\n    get:\n      summary: x\n";
    let facts = extract(
        &FileInput::new("api.yaml", src),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert!(
        names_of(&facts, NodeKind::ApiPath).is_empty(),
        "a bare `openapi:` with no version does not promote"
    );
    assert_eq!(config_file(&facts).body, None);
}

/// A `openapi:`/`swagger:` key whose value is an empty string or a YAML null is
/// **not** version-bearing — so it does not promote. Guards the "version-bearing"
/// half of the sniff over a degenerate value ([NFR-RA-05]).
#[test]
fn openapi_with_empty_or_null_version_value_is_not_promoted() {
    for src in [
        "openapi: \"\"\npaths:\n  /a:\n    get:\n      summary: x\n",
        "swagger: null\npaths:\n  /a:\n    get:\n      summary: x\n",
        "openapi: ~\npaths:\n  /a:\n    get:\n      summary: x\n",
    ] {
        let facts = extract(
            &FileInput::new("api.yaml", src),
            &yaml_plugin(),
            &SymbolContext::default(),
        );
        assert!(
            names_of(&facts, NodeKind::ApiPath).is_empty(),
            "a non-version-bearing value must not promote: {src:?}"
        );
        assert_eq!(config_file(&facts).body, None, "not tagged: {src:?}");
    }
}

/// Swagger 2.0's `swagger:` version key sniffs to the same OpenAPI profile.
#[test]
fn swagger_2_0_document_is_promoted() {
    let src = "\
swagger: \"2.0\"
paths:
  /things:
    get:
      summary: list
";
    let facts = extract(
        &FileInput::new("api.yaml", src),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert_eq!(names_of(&facts, NodeKind::ApiPath), vec!["/things"]);
    assert_eq!(config_file(&facts).body.as_deref(), Some("openapi"));
}

// ── FR-CG-03 / NFR-RA-05: conservative, never-fabricate promotion ─────────────

/// A version-bearing spec with **no** `paths` map is still recognised — its
/// `ConfigFile` is profile-tagged — but it emits no `ApiPath`/`ApiOperation`
/// (the tag-only promotion branch).
#[test]
fn openapi_spec_with_no_paths_map_is_tagged_but_emits_no_api_nodes() {
    let src = "openapi: 3.0.3\ninfo:\n  title: No paths here\n  version: 1.0.0\n";
    let facts = extract(
        &FileInput::new("api.yaml", src),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert_eq!(
        config_file(&facts).body.as_deref(),
        Some("openapi"),
        "a spec with no paths is still profile-tagged"
    );
    assert!(
        names_of(&facts, NodeKind::ApiPath).is_empty(),
        "no ApiPath without a paths map"
    );
    assert!(
        names_of(&facts, NodeKind::ApiOperation).is_empty(),
        "no ApiOperation without a paths map"
    );
}

/// A non-`/`-prefixed key under `paths` (a `$ref`, an `x-` extension) is **not**
/// a path template and is skipped — never fabricated as an `ApiPath` ([NFR-RA-05]).
#[test]
fn non_path_keys_under_paths_are_not_emitted_as_api_paths() {
    let src = "\
openapi: 3.0.3
paths:
  x-internal:
    note: ignored
  $ref: '#/components/pathItems/foo'
  /real:
    get:
      summary: ok
";
    let facts = extract(
        &FileInput::new("api.yaml", src),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert_eq!(
        names_of(&facts, NodeKind::ApiPath),
        vec!["/real"],
        "only the `/`-prefixed key is an ApiPath; x-/$ref keys are skipped"
    );
}

/// A non-HTTP-method key under a path item (`summary`, `parameters`, an `x-`
/// extension) is **not** an operation and is skipped — never fabricated as an
/// `ApiOperation` ([NFR-RA-05]).
#[test]
fn non_method_keys_under_a_path_item_are_not_emitted_as_operations() {
    let src = "\
openapi: 3.0.3
paths:
  /pets:
    summary: Pets collection
    parameters:
      - name: limit
    get:
      summary: list
    x-internal: true
";
    let facts = extract(
        &FileInput::new("api.yaml", src),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    assert_eq!(names_of(&facts, NodeKind::ApiPath), vec!["/pets"]);
    assert_eq!(
        names_of(&facts, NodeKind::ApiOperation),
        vec!["get"],
        "only the HTTP-method key is an ApiOperation; summary/parameters/x- are skipped"
    );
}

// ── NFR-RA-06: determinism & the JSON/YAML twin ───────────────────────────────

/// Re-indexing the same spec yields byte-identical nodes and edges ([NFR-RA-06]).
#[test]
fn re_index_is_byte_identical() {
    let input = FileInput::new("api.yaml", OPENAPI_YAML);
    let a = extract(&input, &yaml_plugin(), &SymbolContext::default());
    let b = extract(&input, &yaml_plugin(), &SymbolContext::default());
    assert_eq!(a.nodes, b.nodes, "re-extract nodes are byte-identical");
    assert_eq!(a.edges, b.edges, "re-extract edges are byte-identical");
}

/// A JSON-bodied spec promotes **identically** to its YAML twin ([NFR-RA-06]): the
/// promoted `ApiPath`/`ApiOperation` symbols, names, and `Contains` structure match
/// exactly. Extracted under the same path so the only thing that *could* differ —
/// the file-path segment of the symbol — is held equal, isolating the promotion.
#[test]
fn json_spec_promotes_identically_to_its_yaml_twin() {
    let ctx = SymbolContext::default();
    let y = extract(
        &FileInput::new("api.yaml", OPENAPI_YAML),
        &yaml_plugin(),
        &ctx,
    );
    let j = extract(
        &FileInput::new("api.yaml", OPENAPI_JSON),
        &json_plugin(),
        &ctx,
    );

    // The promoted typed nodes (symbol + name) are identical across the twins.
    let promoted = |f: &Facts| {
        let mut v: Vec<(String, String)> = f
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::ApiPath | NodeKind::ApiOperation))
            .map(|n| (n.symbol.as_str().to_string(), n.name.clone()))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        promoted(&y),
        promoted(&j),
        "the JSON spec promotes to the identical ApiPath/ApiOperation nodes"
    );

    // The Contains edges among the promoted subtree are identical too.
    let promoted_syms: std::collections::HashSet<String> = y
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n.kind,
                NodeKind::ConfigFile | NodeKind::ApiPath | NodeKind::ApiOperation
            )
        })
        .map(|n| n.symbol.as_str().to_string())
        .collect();
    let promoted_edges = |f: &Facts| {
        let mut v: Vec<(String, String)> = f
            .edges
            .iter()
            .filter(|e| {
                promoted_syms.contains(e.source.as_str())
                    && promoted_syms.contains(e.target.as_str())
            })
            .map(|e| (e.source.as_str().to_string(), e.target.as_str().to_string()))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        promoted_edges(&y),
        promoted_edges(&j),
        "the promoted Contains structure is identical across the twins"
    );
    // And both are genuinely tagged.
    assert_eq!(config_file(&y).body.as_deref(), Some("openapi"));
    assert_eq!(config_file(&j).body.as_deref(), Some("openapi"));
}

/// Same-slug path templates are ordinal-disambiguated so their identities never
/// collide ([NFR-RA-06]) — the promotion analogue of the anchor-walk sibling test.
#[test]
fn same_slug_path_templates_are_ordinal_disambiguated() {
    // `/a` and `/A` slugify to the same `a`; both must be emitted with distinct ids.
    let src = "\
openapi: 3.0.0
paths:
  /a:
    get:
      summary: lower
  /A:
    get:
      summary: upper
";
    let facts = extract(
        &FileInput::new("api.yaml", src),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    let paths: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ApiPath)
        .map(|n| n.symbol.as_str())
        .collect();
    assert_eq!(paths.len(), 2, "both same-slug templates are emitted");
    assert_ne!(paths[0], paths[1], "their identities are disambiguated");
}

/// Every promoted node is a config / non-code kind — metric-neutral by
/// construction ([FR-CG-05]); `ApiPath`/`ApiOperation` never enter the code subgraph.
#[test]
fn promoted_nodes_are_metric_neutral_config_kinds() {
    let facts = extract(
        &FileInput::new("api.yaml", OPENAPI_YAML),
        &yaml_plugin(),
        &SymbolContext::default(),
    );
    for n in &facts.nodes {
        assert!(
            n.kind.is_config() && n.kind.is_non_code(),
            "{} must be a non-code config kind",
            n.kind.as_str()
        );
    }
}
