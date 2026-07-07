//! Protobuf schema-format plugin tests (S-065, CR-010, FR-CG-03).
//!
//! Drives the **real** `tree-sitter-proto` grammar through the artifact substrate
//! using the embedded `plugins/protobuf/plugin.toml` descriptor — the typed-anchor
//! walk (`[[config.anchors]]`: `message` → `ProtoMessage`, `service` →
//! `ProtoService`) the substrate's generic anchor walk drives from pure plugin
//! data. Also the empirical half of the FR-CG-06 verification preflight: the
//! grammar loads at the workspace ABI and the fixture corpus parses.

use std::collections::BTreeMap;

use crate::extract::{extract, FileInput, SymbolContext};
use crate::model::{EdgeKind, NodeKind};
use crate::plugin::{CompiledPlugin, LanguagePlugin, PluginManifest};

/// Build the real Protobuf artifact plugin from its embedded descriptor + grammar.
fn proto_plugin() -> CompiledPlugin {
    let toml = include_str!("../../../plugins/protobuf/plugin.toml");
    let manifest = PluginManifest::parse("protobuf/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_proto::LANGUAGE.into();
    CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
}

/// A proto3 fixture: two top-level messages (one with a nested message), one
/// service, and an `import` — the `import` must produce no edge.
const FIXTURE: &str = r#"syntax = "proto3";
package example.v1;

import "google/protobuf/timestamp.proto";

message User {
  string id = 1;
  message Address {
    string city = 1;
  }
}

message Account {
  string owner = 1;
}

service UserService {
  rpc GetUser (User) returns (User);
}
"#;

fn names_of(facts: &crate::extract::Facts, kind: NodeKind) -> Vec<String> {
    let mut v: Vec<String> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == kind)
        .map(|n| n.name.clone())
        .collect();
    v.sort();
    v
}

/// The preflight's empirical leg: the grammar binds and the fixture parses with
/// no syntax error.
#[test]
fn preflight_grammar_loads_and_fixture_parses() {
    let plugin = proto_plugin();
    assert_eq!(
        plugin.semantics().abi_version,
        15,
        "descriptor ABI is recorded"
    );
    let facts = extract(
        &FileInput::new("api/user.proto", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );
    assert!(
        !facts.partial,
        "the fixture parses cleanly: {:?}",
        facts.warnings
    );
}

/// A `.proto` fixture yields `ProtoMessage` per message definition (including the
/// nested one) and `ProtoService` per service ([FR-CG-03]).
#[test]
fn messages_and_services_become_typed_anchors() {
    let plugin = proto_plugin();
    let facts = extract(
        &FileInput::new("api/user.proto", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );

    assert_eq!(
        names_of(&facts, NodeKind::ProtoMessage),
        vec!["Account", "Address", "User"],
        "one ProtoMessage per message, nested included"
    );
    assert_eq!(
        names_of(&facts, NodeKind::ProtoService),
        vec!["UserService"],
        "one ProtoService per service"
    );
    // Exactly one ConfigFile root.
    assert_eq!(
        facts
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::ConfigFile)
            .count(),
        1
    );
}

/// Extraction emits only `Contains` **edges**; cross-artifact references are
/// captured as `facts.refs` (bound into `ArtifactRef`/`ArtifactBinding` edges by
/// the resolution pass, CR-011), never as extraction-time edges. Anchors hang
/// off the `ConfigFile` root.
#[test]
fn extraction_edges_are_contains_only_references_are_facts() {
    let plugin = proto_plugin();
    let facts = extract(
        &FileInput::new("api/user.proto", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );

    for edge in &facts.edges {
        assert_eq!(
            edge.kind,
            EdgeKind::Contains,
            "extraction emits only Contains edges; reference edges are resolution-time"
        );
    }
    // CR-011 (S-070): references ARE now captured — every one carries a relation
    // class and an artifact edge kind (no code/doc reference leaks in).
    assert!(
        !facts.refs.is_empty(),
        "the schema's references are captured into facts.refs"
    );
    for r in &facts.refs {
        let relation = r.relation.expect("a config reference carries its relation");
        assert_eq!(r.kind, relation.edge_kind());
        assert!(matches!(
            r.kind,
            EdgeKind::ArtifactRef | EdgeKind::ArtifactBinding
        ));
    }

    let config_file = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::ConfigFile)
        .unwrap();
    // Every anchor is Contains-nested under the ConfigFile root.
    let anchors: Vec<_> = facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::ProtoMessage | NodeKind::ProtoService))
        .collect();
    assert_eq!(anchors.len(), 4);
    for a in anchors {
        assert!(
            facts
                .edges
                .iter()
                .any(|e| e.source == config_file.symbol && e.target == a.symbol),
            "anchor {} hangs off the ConfigFile root",
            a.name
        );
    }
}

/// A fixture exercising every S-070 proto reference family: a workspace import,
/// a vendored import, an unqualified cross-message field reference, a
/// package-qualified (vendored) field reference, and an RPC signature.
const REF_FIXTURE: &str = r#"syntax = "proto3";
package example.v1;

import "common/types.proto";
import "google/protobuf/timestamp.proto";

message User {
  string id = 1;
  Account account = 2;
  google.protobuf.Timestamp created = 3;
}

message Account {
  string owner = 1;
}

service UserService {
  rpc GetUser (User) returns (Account);
}
"#;

/// The `(relation, source-anchor-name, target)` of every captured reference,
/// sorted — the source name resolves a ref's source symbol back to its node so
/// references from different anchors (a field vs an RPC) are distinguished.
/// (Intentionally identical to `graphql_tests::full_refs`; the two `#[cfg(test)]`
/// sibling modules cannot share a definition without a shared test-util module —
/// a worthwhile follow-up, kept duplicated here to keep this story isolated.)
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

/// The symbol of the unique anchor of `kind` named `name`.
fn anchor_symbol(facts: &crate::extract::Facts, name: &str, kind: NodeKind) -> String {
    facts
        .nodes
        .iter()
        .find(|n| n.kind == kind && n.name == name)
        .unwrap_or_else(|| panic!("no {kind:?} named {name}"))
        .symbol
        .as_str()
        .to_string()
}

/// FR-CG-08: a workspace-relative `import` is captured as a `ProtoImport`
/// reference; a vendored `google/protobuf/*` import is classified out — no fact,
/// no ledger entry ([UAT-CG-04]).
#[test]
fn proto_imports_capture_workspace_paths_and_drop_vendored() {
    let facts = extract(
        &FileInput::new("api/user.proto", REF_FIXTURE),
        &proto_plugin(),
        &SymbolContext::default(),
    );
    use crate::model::ArtifactRelation::ProtoImport;
    let imports: Vec<&str> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ProtoImport))
        .map(|r| r.target.as_str())
        .collect();
    assert_eq!(
        imports,
        ["common/types.proto"],
        "only the workspace import is a candidate; the vendored one is dropped"
    );
    // The import reference is sourced from the ConfigFile root (file-level).
    let config_file = anchor_symbol(&facts, "user.proto", NodeKind::ConfigFile);
    assert!(facts
        .refs
        .iter()
        .any(|r| r.relation == Some(ProtoImport) && r.source.as_str() == config_file));
}

/// FR-CG-08: an unqualified field type reference is captured as a `ProtoType`
/// reference; a package-qualified (`google.protobuf.Timestamp`) reference is
/// **not** — the never-fabricate guard for cross-package/vendored types.
#[test]
fn proto_field_types_capture_unqualified_and_skip_qualified() {
    let facts = extract(
        &FileInput::new("api/user.proto", REF_FIXTURE),
        &proto_plugin(),
        &SymbolContext::default(),
    );
    use crate::model::ArtifactRelation::ProtoType;
    let type_targets: Vec<&str> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ProtoType))
        .map(|r| r.target.as_str())
        .collect();
    // `Account` (User field) and the RPC's `User`/`Account` are unqualified.
    assert!(type_targets.contains(&"Account"), "{type_targets:?}");
    assert!(type_targets.contains(&"User"), "{type_targets:?}");
    // The qualified `google.protobuf.Timestamp` is never captured (no `Timestamp`,
    // nothing containing `google`).
    assert!(
        !type_targets
            .iter()
            .any(|t| t.contains("Timestamp") || t.contains("google")),
        "package-qualified references are not candidates: {type_targets:?}"
    );
    // The User field reference is sourced from the User message anchor.
    let user = anchor_symbol(&facts, "User", NodeKind::ProtoMessage);
    assert!(
        facts.refs.iter().any(|r| r.relation == Some(ProtoType)
            && r.target == "Account"
            && r.source.as_str() == user),
        "the field reference is sourced from its enclosing message anchor"
    );
}

/// FR-CG-10: each declared message name becomes a `SchemaType` `ArtifactBinding`
/// reference (artifact→code), sourced from the message anchor.
#[test]
fn proto_declared_message_names_capture_schema_type_bindings() {
    let facts = extract(
        &FileInput::new("api/user.proto", REF_FIXTURE),
        &proto_plugin(),
        &SymbolContext::default(),
    );
    use crate::model::ArtifactRelation::SchemaType;
    let mut names: Vec<&str> = facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(SchemaType))
        .map(|r| r.target.as_str())
        .collect();
    names.sort();
    names.dedup();
    // Only message names bind to code ([FR-CG-10]): a `ProtoService` is an RPC
    // grouping, not a data type, so `UserService` is deliberately NOT a
    // SchemaType target — its RPC signatures contribute `ProtoType` references.
    assert_eq!(
        names,
        ["Account", "User"],
        "every declared message binds its name; the service does not"
    );
    assert!(
        !names.contains(&"UserService"),
        "a service declaration is not a type-like code binding"
    );
    for r in facts.refs.iter().filter(|r| r.relation == Some(SchemaType)) {
        assert_eq!(
            r.kind,
            EdgeKind::ArtifactBinding,
            "a declared-name binding is artifact→code"
        );
    }
    // Sourced from the message anchor it declares.
    let user = anchor_symbol(&facts, "User", NodeKind::ProtoMessage);
    assert!(facts.refs.iter().any(|r| r.relation == Some(SchemaType)
        && r.target == "User"
        && r.source.as_str() == user));
}

/// The complete captured reference set for [`REF_FIXTURE`] — pins the exact,
/// deduplicated facts so an over- or under-capture regression is caught.
#[test]
fn proto_reference_capture_is_exact() {
    use crate::model::ArtifactRelation::*;
    let facts = extract(
        &FileInput::new("api/user.proto", REF_FIXTURE),
        &proto_plugin(),
        &SymbolContext::default(),
    );
    let r = |rel, src: &str, tgt: &str| (rel, src.to_string(), tgt.to_string());
    assert_eq!(
        full_refs(&facts),
        vec![
            r(ProtoImport, "user.proto", "common/types.proto"),
            r(ProtoType, "User", "Account"), // the User.account field
            r(ProtoType, "UserService", "Account"), // the RPC return type
            r(ProtoType, "UserService", "User"), // the RPC request type
            r(SchemaType, "Account", "Account"), // declared name → code
            r(SchemaType, "User", "User"),
        ]
    );
}

/// Every emitted node is a config / non-code kind — metric-neutral by construction
/// ([FR-CG-05]).
#[test]
fn every_node_is_a_config_kind() {
    let plugin = proto_plugin();
    let facts = extract(
        &FileInput::new("api/user.proto", FIXTURE),
        &plugin,
        &SymbolContext::default(),
    );
    assert!(!facts.nodes.is_empty());
    for n in &facts.nodes {
        assert!(
            n.kind.is_config() && n.kind.is_non_code(),
            "{}",
            n.kind.as_str()
        );
    }
}

/// Re-extraction is byte-identical — the `path#anchor` identity determinism
/// ([NFR-RA-06]).
#[test]
fn re_extraction_is_byte_identical() {
    let plugin = proto_plugin();
    let input = FileInput::new("api/user.proto", FIXTURE);
    let a = extract(&input, &plugin, &SymbolContext::default());
    let b = extract(&input, &plugin, &SymbolContext::default());
    let syms = |f: &crate::extract::Facts| {
        f.nodes
            .iter()
            .map(|n| n.symbol.as_str().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(syms(&a), syms(&b));
}

/// Build a Protobuf artifact plugin whose `message` anchor declares **no**
/// `name_child`, to drive the `anchor_name` first-source-line fallback (the
/// path the embedded descriptor, which always sets `name_child`, never reaches).
fn proto_plugin_without_name_child() -> CompiledPlugin {
    let toml = r#"
        name = "protobuf"
        extensions = ["proto"]
        module_separator = "/"
        abi_version = 15
        capabilities = []
        artifact = true
        [[config.anchors]]
        node_kind = "message"
        kind = "proto_message"
    "#;
    let manifest = PluginManifest::parse("protobuf/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_proto::LANGUAGE.into();
    CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
}

/// With no `name_child`, the anchor name falls back to the node's first source
/// line — derived, never fabricated ([NFR-RA-05]). Proves the fallback branch of
/// `anchor_name`.
#[test]
fn anchor_name_falls_back_to_first_source_line() {
    let plugin = proto_plugin_without_name_child();
    let facts = extract(
        &FileInput::new("a.proto", "message Solo {\n  string id = 1;\n}\n"),
        &plugin,
        &SymbolContext::default(),
    );
    let msg = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::ProtoMessage)
        .expect("one ProtoMessage");
    // The embedded descriptor would name it "Solo" via `message_name`; with the
    // child lookup disabled the name is the node's first line instead.
    assert_eq!(msg.name, "message Solo {");
}

/// Two messages slugifying to the same value are ordinal-disambiguated, so their
/// symbols never collide ([NFR-RA-06]) — the anchor-walk analogue of the section
/// walk's sibling-collision test. (Duplicate message names are not valid proto3
/// but the grammar parses them; the walk must still produce distinct identities.)
#[test]
fn same_name_anchors_are_ordinal_disambiguated() {
    let plugin = proto_plugin();
    let facts = extract(
        &FileInput::new("dup.proto", "message Dup {}\nmessage Dup {}\n"),
        &plugin,
        &SymbolContext::default(),
    );
    let dups: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ProtoMessage && n.name == "Dup")
        .map(|n| n.symbol.as_str())
        .collect();
    assert_eq!(dups.len(), 2, "both same-named messages are emitted");
    assert_ne!(
        dups[0], dups[1],
        "their identities are disambiguated, not collided"
    );
}

/// A syntactically broken `.proto` is partially extracted, never aborted
/// ([FR-IX-04]): the parse error sets `partial`, a warning is recorded, and the
/// well-formed message before the break is still emitted.
#[test]
fn a_malformed_proto_is_partially_extracted() {
    let plugin = proto_plugin();
    // `Ok` is well-formed; `Broken` is left unclosed.
    let facts = extract(
        &FileInput::new("broken.proto", "message Ok {}\nmessage Broken {\n"),
        &plugin,
        &SymbolContext::default(),
    );
    assert!(facts.partial, "a syntax error flags partial extraction");
    assert!(
        facts
            .warnings
            .iter()
            .any(|w| w.contains("partial extraction")),
        "a partial-extraction warning is recorded: {:?}",
        facts.warnings
    );
    assert!(
        facts
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::ProtoMessage && n.name == "Ok"),
        "the well-formed message before the break is still extracted"
    );
}
