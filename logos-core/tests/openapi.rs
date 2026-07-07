//! Behavioural integration test for the OpenAPI content-sniffed promotion
//! (S-067, [CR-010], [FR-CG-03], [FR-CG-04], [NFR-RA-06]), driving the promotion
//! through the public discover → extract → persist → search pipeline over the
//! real `tree-sitter-yaml`/`tree-sitter-json` grammars the S-063 data-format
//! plugins ship.
//!
//! Covers the S-067 acceptance criteria that need the full engine:
//! - an `api.yaml` with no naming convention is promoted (ApiPath per template,
//!   ApiOperation per method, ConfigFile profile-tagged), while a non-OpenAPI
//!   `api.yaml` beside it is not ([FR-CG-03], [UAT-CG-02]);
//! - a path-template search returns its `ApiPath` node, kind-filterable
//!   ([FR-CG-04]);
//! - a JSON-bodied spec promotes to the same anchors as its YAML twin
//!   ([NFR-RA-06]).
//!
//! Gated on the two data features so a build that excludes them does not run
//! these assertions.
#![cfg(all(feature = "lang-yaml", feature = "lang-json"))]

use std::fs;
use std::path::Path;

use logos_core::model::NodeKind;
use logos_core::{Engine, Runtime};

/// A version-bearing OpenAPI 3.x document — named `api.yaml`, promoted by content.
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

/// The JSON twin of [`OPENAPI_YAML`].
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

/// A non-OpenAPI document, also named `api.yaml` — a top-level `paths:` key but no
/// version-bearing `openapi:`/`swagger:` key, so the sniff must reject it.
const NOT_OPENAPI_YAML: &str = "\
service: gateway
paths:
  enabled: true
  prefix: /api
";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

fn node_names(rt: &Runtime, kind: NodeKind) -> Vec<String> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| n.name)
            .collect())
    })
    .expect("read runs")
}

/// FR-CG-03 / UAT-CG-02: a content-sniffed `api.yaml` is promoted end-to-end; a
/// non-OpenAPI `api.yaml` directory neighbour is not.
#[test]
fn openapi_document_is_promoted_and_non_openapi_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Two files, both named `api.yaml` (different directories) — promotion is by
    // content, not filename.
    write(root, "spec/api.yaml", OPENAPI_YAML);
    write(root, "gateway/api.yaml", NOT_OPENAPI_YAML);

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    // The OpenAPI spec promoted: a path-template ApiPath per `paths` entry.
    let mut paths = node_names(rt, NodeKind::ApiPath);
    paths.sort();
    assert_eq!(
        paths,
        vec!["/pets", "/pets/{id}"],
        "the OpenAPI spec is promoted to one ApiPath per template"
    );
    // One ApiOperation per HTTP method (get+post under /pets, get+delete under /pets/{id}).
    assert_eq!(
        node_names(rt, NodeKind::ApiOperation).len(),
        4,
        "one ApiOperation per HTTP method"
    );

    // The non-OpenAPI neighbour produced no API anchors — only the OpenAPI spec's.
    // (Both files are still indexed as generic ConfigFiles.)
    let config_files = node_names(rt, NodeKind::ConfigFile);
    assert_eq!(
        config_files.iter().filter(|n| *n == "api.yaml").count(),
        2,
        "both api.yaml files are indexed as ConfigFiles: {config_files:?}"
    );
}

/// FR-CG-04: a path-template search returns its `ApiPath` node, kind-filterable.
#[test]
fn path_template_search_returns_the_apipath_node_kind_filterable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "openapi/api.yaml", OPENAPI_YAML);

    let engine = Engine::start(root).expect("engine starts");
    engine.index();

    // Searching a known path template, filtered to ApiPath, returns the node.
    let hits = engine.search("pets", Some(NodeKind::ApiPath), None);
    assert!(hits.warnings.is_empty(), "{:?}", hits.warnings);
    assert!(
        hits.hits.iter().any(|h| h.name == "/pets"),
        "the path template is found as an ApiPath: {:?}",
        hits.hits
    );
    assert!(
        hits.hits.iter().all(|h| h.kind == NodeKind::ApiPath),
        "the kind filter restricts to ApiPath: {:?}",
        hits.hits
    );

    // The kind filter genuinely excludes: the same query as a Function returns nothing.
    let none = engine.search("pets", Some(NodeKind::Function), None);
    assert!(
        none.hits.is_empty(),
        "a path template is not a Function: {:?}",
        none.hits
    );

    // The ConfigFile is profile-tagged in its FTS-indexed body: an `openapi`
    // search filtered to ConfigFile finds the spec via its tag.
    let tagged = engine.search("openapi", Some(NodeKind::ConfigFile), None);
    assert!(
        tagged.hits.iter().any(|h| h.name == "api.yaml"),
        "the profile-tagged ConfigFile is found via its body tag: {:?}",
        tagged.hits
    );
}

/// NFR-RA-06: a JSON-bodied spec promotes to the same path/operation anchors as
/// its YAML twin, indexed through the full pipeline.
#[test]
fn json_spec_promotes_to_the_same_anchors_as_its_yaml_twin() {
    let index_paths = |rel: &str, src: &str| {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), rel, src);
        let engine = Engine::start(tmp.path()).expect("engine starts");
        engine.index();
        let rt = engine.runtime().expect("runtime present");
        let mut paths = node_names(rt, NodeKind::ApiPath);
        paths.sort();
        let mut ops = node_names(rt, NodeKind::ApiOperation);
        ops.sort();
        (paths, ops)
    };

    let yaml = index_paths("api.yaml", OPENAPI_YAML);
    let json = index_paths("api.json", OPENAPI_JSON);
    assert_eq!(
        yaml, json,
        "the JSON spec promotes to the identical path templates and operations as its YAML twin"
    );
    assert_eq!(yaml.0, vec!["/pets", "/pets/{id}"]);
    assert_eq!(yaml.1, vec!["delete", "get", "get", "post"]);
}
