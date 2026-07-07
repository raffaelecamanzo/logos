//! GraphQL schema-format plugin tests (S-065, CR-010, FR-CG-03).
//!
//! Drives the **real** `tree-sitter-graphql` grammar — the CR-010 highest-risk
//! grammar — through the artifact substrate using the embedded
//! `plugins/graphql/plugin.toml` descriptor. GraphQL emits a single `GqlType`
//! anchor kind whose `payload` distinguishes the six type-definition subtypes
//! (object / interface / enum / input / union / scalar). Also the empirical half
//! of the FR-CG-06 preflight for the highest-risk grammar.

use std::collections::BTreeMap;

use crate::extract::{extract, FileInput, SymbolContext};
use crate::model::{EdgeKind, NodeKind};
use crate::plugin::{CompiledPlugin, LanguagePlugin, PluginManifest};

fn graphql_plugin() -> CompiledPlugin {
    let toml = include_str!("../../../plugins/graphql/plugin.toml");
    let manifest = PluginManifest::parse("graphql/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_graphql::LANGUAGE.into();
    CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
}

/// A schema exercising all six type-definition subtypes.
const FIXTURE: &str = r#"
type User implements Node {
  id: ID!
  name: String
}

interface Node {
  id: ID!
}

enum Role {
  ADMIN
  USER
}

input UserFilter {
  name: String
}

union SearchResult = User | Account

scalar DateTime
"#;

/// The preflight's empirical leg for the highest-risk grammar: it binds and the
/// fixture parses cleanly.
#[test]
fn preflight_grammar_loads_and_fixture_parses() {
    let plugin = graphql_plugin();
    assert_eq!(plugin.semantics().abi_version, 15);
    let facts = extract(
        &FileInput::new("schema.graphql", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );
    assert!(
        !facts.partial,
        "fixture parses cleanly: {:?}",
        facts.warnings
    );
}

/// Every type definition becomes a `GqlType` whose `payload` (carried in `body`)
/// distinguishes object / interface / enum / input / union / scalar ([FR-CG-03]).
#[test]
fn type_definitions_become_gql_types_with_payload() {
    let plugin = graphql_plugin();
    let facts = extract(
        &FileInput::new("schema.graphql", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );

    // name → payload, for every GqlType node.
    let mut by_name: Vec<(String, String)> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::GqlType)
        .map(|n| (n.name.clone(), n.body.clone().unwrap_or_default()))
        .collect();
    by_name.sort();

    assert_eq!(
        by_name,
        vec![
            // `Account` is referenced in the union but never defined, so it yields
            // no anchor — only the six defined type definitions do.
            ("DateTime".to_string(), "scalar".to_string()),
            ("Node".to_string(), "interface".to_string()),
            ("Role".to_string(), "enum".to_string()),
            ("SearchResult".to_string(), "union".to_string()),
            ("User".to_string(), "object".to_string()),
            ("UserFilter".to_string(), "input".to_string()),
        ]
    );
}

/// Extraction emits only `Contains` **edges** (each `GqlType` hangs off the
/// `ConfigFile` root); cross-artifact references are captured as `facts.refs`,
/// bound into `ArtifactRef`/`ArtifactBinding` edges by the resolution pass
/// (CR-011, S-070). Every node is a metric-neutral config kind.
#[test]
fn extraction_edges_are_contains_only_references_are_facts() {
    let plugin = graphql_plugin();
    let facts = extract(
        &FileInput::new("schema.graphql", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );
    for edge in &facts.edges {
        assert_eq!(edge.kind, EdgeKind::Contains);
    }
    // The declared type names are bound to code; the FIXTURE's field types are
    // all built-in scalars, so they yield no GraphqlType references here.
    assert!(
        !facts.refs.is_empty(),
        "declared type names are captured as references"
    );
    for r in &facts.refs {
        let relation = r.relation.expect("a config reference carries its relation");
        assert_eq!(r.kind, relation.edge_kind());
        assert!(matches!(
            r.kind,
            EdgeKind::ArtifactRef | EdgeKind::ArtifactBinding
        ));
    }
    for n in &facts.nodes {
        assert!(
            n.kind.is_config() && n.kind.is_non_code(),
            "{}",
            n.kind.as_str()
        );
    }
}

/// A fixture exercising S-070 GraphQL field references: object/interface/input
/// field types that reference declared types, built-in scalar fields,
/// `implements`, and a union — the last two are type→type references but **not**
/// field references and must be out of scope.
const REF_FIXTURE: &str = r#"
interface Node {
  id: ID!
}

type User implements Node {
  id: ID!
  account: Account
  roles: [Role!]!
}

type Account {
  owner: User
}

enum Role {
  ADMIN
  USER
}

input UserFilter {
  account: Account
  name: String
}

union SearchResult = User | Account
"#;

/// The `(relation, source-type, target)` of every captured reference, sorted.
/// (Intentionally identical to `proto_tests::full_refs`; see the note there on
/// the deferred shared test-util consolidation.)
fn full_refs(
    facts: &crate::extract::Facts,
) -> Vec<(crate::model::ArtifactRelation, String, String)> {
    let mut v: Vec<_> = facts
        .refs
        .iter()
        .map(|r| {
            let src = facts
                .nodes
                .iter()
                .find(|n| n.symbol == r.source)
                .map(|n| n.name.clone())
                .unwrap_or_default();
            (r.relation.unwrap(), src, r.target.clone())
        })
        .collect();
    v.sort_by(|a, b| (a.0.as_str(), &a.1, &a.2).cmp(&(b.0.as_str(), &b.1, &b.2)));
    v
}

/// FR-CG-08 / UAT-CG-03: a field type references the declared type in the same
/// schema scope (`GraphqlType`); a built-in scalar field references nothing;
/// `implements` and union members are not field references.
#[test]
fn graphql_field_types_capture_in_scope_type_refs_only() {
    use crate::model::ArtifactRelation::GraphqlType;
    let facts = extract(
        &FileInput::new("schema.graphql", REF_FIXTURE),
        &graphql_plugin(),
        &SymbolContext::default(),
    );
    let type_refs: Vec<(String, String)> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(GraphqlType))
        .map(|r| {
            let src = facts
                .nodes
                .iter()
                .find(|n| n.symbol == r.source)
                .map(|n| n.name.clone())
                .unwrap_or_default();
            (src, r.target.clone())
        })
        .collect();

    // Object/interface/input field references to declared types are captured.
    assert!(type_refs.contains(&("User".into(), "Account".into())));
    assert!(type_refs.contains(&("User".into(), "Role".into()))); // through [Role!]!
    assert!(type_refs.contains(&("Account".into(), "User".into())));
    assert!(type_refs.contains(&("UserFilter".into(), "Account".into())));

    // Built-in scalars are never referenced (ID/String classified out).
    assert!(
        !type_refs
            .iter()
            .any(|(_, t)| matches!(t.as_str(), "ID" | "String")),
        "built-in scalars are not workspace type references: {type_refs:?}"
    );
    // `implements Node` is not a field reference.
    assert!(
        !type_refs.contains(&("User".into(), "Node".into())),
        "an implemented interface is not a field reference: {type_refs:?}"
    );
    // Union members are not field references.
    assert!(
        !type_refs.iter().any(|(src, _)| src == "SearchResult"),
        "union members are not field references: {type_refs:?}"
    );
}

/// A field **argument**'s input type is not a field type reference — only the
/// field's own declared type is ([FR-CG-08], "type→type field references").
#[test]
fn graphql_field_argument_types_are_not_field_references() {
    use crate::model::ArtifactRelation::GraphqlType;
    let src = "\
type Query {
  posts(filter: PostFilter): Post
}
type Post { id: ID! }
input PostFilter { tag: String }
";
    let facts = extract(
        &FileInput::new("schema.graphql", src),
        &graphql_plugin(),
        &SymbolContext::default(),
    );
    let targets: Vec<&str> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(GraphqlType))
        .map(|r| r.target.as_str())
        .collect();
    assert!(
        targets.contains(&"Post"),
        "the field's declared return type is a field reference: {targets:?}"
    );
    assert!(
        !targets.contains(&"PostFilter"),
        "a field argument's input type is not a field reference: {targets:?}"
    );
}

/// FR-CG-10: every declared type name (all six subtypes) becomes a `SchemaType`
/// `ArtifactBinding` reference (artifact→code), sourced from its `GqlType` anchor.
#[test]
fn graphql_declared_type_names_capture_schema_type_bindings() {
    use crate::model::ArtifactRelation::SchemaType;
    let facts = extract(
        &FileInput::new("schema.graphql", REF_FIXTURE),
        &graphql_plugin(),
        &SymbolContext::default(),
    );
    let mut names: Vec<&str> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(SchemaType))
        .map(|r| r.target.as_str())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Account",
            "Node",
            "Role",
            "SearchResult",
            "User",
            "UserFilter"
        ],
        "every declared GraphQL type binds its name to code"
    );
    for r in facts.refs.iter().filter(|r| r.relation == Some(SchemaType)) {
        assert_eq!(r.kind, EdgeKind::ArtifactBinding);
    }
}

/// The complete captured reference set for [`REF_FIXTURE`] — pins the exact facts
/// so an over- or under-capture regression is caught.
#[test]
fn graphql_reference_capture_is_exact() {
    use crate::model::ArtifactRelation::*;
    let facts = extract(
        &FileInput::new("schema.graphql", REF_FIXTURE),
        &graphql_plugin(),
        &SymbolContext::default(),
    );
    let r = |rel, src: &str, tgt: &str| (rel, src.to_string(), tgt.to_string());
    assert_eq!(
        full_refs(&facts),
        vec![
            r(GraphqlType, "Account", "User"),
            r(GraphqlType, "User", "Account"),
            r(GraphqlType, "User", "Role"),
            r(GraphqlType, "UserFilter", "Account"),
            r(SchemaType, "Account", "Account"),
            r(SchemaType, "Node", "Node"),
            r(SchemaType, "Role", "Role"),
            r(SchemaType, "SearchResult", "SearchResult"),
            r(SchemaType, "User", "User"),
            r(SchemaType, "UserFilter", "UserFilter"),
        ]
    );
}

/// Re-extraction is byte-identical ([NFR-RA-06]).
#[test]
fn re_extraction_is_byte_identical() {
    let plugin = graphql_plugin();
    let input = FileInput::new("schema.graphql", FIXTURE);
    let a = extract(&input, &plugin, &SymbolContext::default());
    let b = extract(&input, &plugin, &SymbolContext::default());
    let syms = |f: &crate::extract::Facts| {
        f.nodes
            .iter()
            .map(|n| (n.symbol.as_str().to_string(), n.body.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(syms(&a), syms(&b));
}
