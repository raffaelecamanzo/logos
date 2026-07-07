//! Behavioural integration test for the S-070 schema-format reference binding
//! (Protobuf + GraphQL), driving capture → persist → resolve end-to-end through
//! the [`Engine`] façade over the real `tree-sitter-proto`/`tree-sitter-graphql`
//! grammars and a Rust code graph (CR-011, [FR-CG-08], [FR-CG-10], [UAT-CG-03],
//! [UAT-CG-04], [NFR-RA-05]).
//!
//! Covers the acceptance criteria that need the full engine:
//! - a proto `import` binds to its sibling `ConfigFile`; a cross-file message
//!   field binds to its `ProtoMessage` within the import closure; a vendored
//!   `google/protobuf/*` import is a non-candidate with no ledger entry;
//! - a GraphQL field type binds to its type in the same schema scope;
//! - a declared schema name binds to exactly one type-like code symbol; a
//!   duplicate code type name stays unresolved — never fabricated;
//! - the new `ArtifactRef`/`ArtifactBinding` edges are metric-neutral
//!   ([UAT-CG-04]): the aggregate signal is byte-identical with them present.
//!
//! Gated on the two schema grammars plus Rust (the code-binding target).
#![cfg(all(
    feature = "lang-protobuf",
    feature = "lang-graphql",
    feature = "lang-rust"
))]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeKind};
use logos_core::models::pipeline::RelationCoverage;
use logos_core::{metrics, resolve, Engine, Granularity, Runtime};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Every edge of `kind` rendered as `(source-name, source-kind, target-name,
/// target-kind)` — node ids resolved back to names/kinds so assertions read in
/// domain terms.
fn artifact_edges(rt: &Runtime, kind: EdgeKind) -> Vec<(String, NodeKind, String, NodeKind)> {
    rt.submit_read(move |store| {
        let by_id: BTreeMap<_, _> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, (n.name, n.kind)))
            .collect();
        let mut out: Vec<_> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .filter_map(|e| {
                let (sn, sk) = by_id.get(&e.source)?.clone();
                let (tn, tk) = by_id.get(&e.target)?.clone();
                Some((sn, sk, tn, tk))
            })
            .collect();
        out.sort_by(|a, b| {
            (&a.0, a.1.as_str(), &a.2, a.3.as_str()).cmp(&(&b.0, b.1.as_str(), &b.2, b.3.as_str()))
        });
        Ok(out)
    })
    .expect("read runs")
}

/// The per-relation coverage read-model straight from the ledger.
fn coverage_by_relation(rt: &Runtime) -> BTreeMap<String, RelationCoverage> {
    rt.submit_read(|store| Ok(resolve::coverage(store)?.by_relation))
        .expect("read runs")
}

// ── Protobuf ─────────────────────────────────────────────────────────────────

const COMMON_PROTO: &str = r#"syntax = "proto3";
package common;

message Common {
  string id = 1;
}
"#;

/// `user.proto` imports a workspace sibling and a vendored well-known type,
/// references `Common` cross-file, and declares `Profile`.
const USER_PROTO: &str = r#"syntax = "proto3";
package app;

import "common.proto";
import "google/protobuf/timestamp.proto";

message Profile {
  Common data = 1;
  google.protobuf.Timestamp created = 2;
}
"#;

/// A Rust code symbol the schema's declared `Common` name binds to.
const MODELS_RS: &str = "pub struct Common { pub id: String }\n";

/// FR-CG-08 / UAT-CG-04: a proto import binds to its sibling `ConfigFile`; a
/// cross-file field binds to its `ProtoMessage`; a vendored import is a
/// non-candidate with no ledger entry.
#[test]
fn proto_imports_and_cross_file_types_bind_vendored_is_no_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "proto/common.proto", COMMON_PROTO);
    write(root, "proto/user.proto", USER_PROTO);
    write(root, "src/models.rs", MODELS_RS);

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    // The workspace import bound ConfigFile → ConfigFile; the cross-file field
    // bound Profile → Common (both ProtoMessage).
    let refs = artifact_edges(rt, EdgeKind::ArtifactRef);
    assert!(
        refs.contains(&(
            "user.proto".into(),
            NodeKind::ConfigFile,
            "common.proto".into(),
            NodeKind::ConfigFile
        )),
        "the workspace import binds to the sibling ConfigFile: {refs:?}"
    );
    assert!(
        refs.contains(&(
            "Profile".into(),
            NodeKind::ProtoMessage,
            "Common".into(),
            NodeKind::ProtoMessage
        )),
        "the cross-file message field binds within the import closure: {refs:?}"
    );

    // The vendored import never entered the ledger: exactly one proto-import
    // candidate (the workspace sibling), fully bound, none unresolved.
    let cov = coverage_by_relation(rt);
    let proto_import = cov.get("proto-import").expect("proto-import coverage");
    assert_eq!(
        (proto_import.bound, proto_import.unresolved),
        (1, 0),
        "the vendored google/protobuf import is no candidate — no ledger entry"
    );

    // The qualified `google.protobuf.Timestamp` field reference was never a
    // candidate either: the only proto-type bind is the unqualified `Common`.
    let proto_type = cov.get("proto-type").expect("proto-type coverage");
    assert_eq!((proto_type.bound, proto_type.unresolved), (1, 0));
}

/// FR-CG-10 / NFR-RA-05: a declared schema name binds to exactly one type-like
/// code symbol; a name matching two code symbols stays unresolved — never
/// fabricated, no synthesized candidate.
#[test]
fn declared_name_binds_to_one_code_symbol_duplicate_stays_unresolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // `Common` matches exactly one code struct → binds.
    write(root, "proto/common.proto", COMMON_PROTO);
    write(root, "src/models.rs", MODELS_RS);
    // `Dup` is declared in proto and matches TWO code structs → never binds.
    write(
        root,
        "proto/dup.proto",
        "syntax = \"proto3\";\nmessage Dup {\n  string x = 1;\n}\n",
    );
    write(root, "src/dup_a.rs", "pub struct Dup { pub a: String }\n");
    write(root, "src/dup_b.rs", "pub struct Dup { pub b: String }\n");

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let bindings = artifact_edges(rt, EdgeKind::ArtifactBinding);
    // `Common` (ProtoMessage) → `Common` (Struct): the one type-like code symbol.
    assert!(
        bindings.contains(&(
            "Common".into(),
            NodeKind::ProtoMessage,
            "Common".into(),
            NodeKind::Struct
        )),
        "a declared name with one code type binds: {bindings:?}"
    );
    // `Dup` matches two structs → no binding edge is ever created.
    assert!(
        !bindings.iter().any(|(s, _, _, _)| s == "Dup"),
        "an ambiguous declared name is never fabricated: {bindings:?}"
    );

    // Exactly one bound (`Common` → its one struct) and one unresolved (the
    // ambiguous `Dup`) — the duplicate stays an honest ledger entry, no fabrication.
    let cov = coverage_by_relation(rt);
    let type_name = cov.get("type-name").expect("type-name coverage");
    assert_eq!(
        (type_name.bound, type_name.unresolved),
        (1, 1),
        "one declared name binds, the duplicate stays unresolved: {type_name:?}"
    );
}

// ── GraphQL ──────────────────────────────────────────────────────────────────

const SCHEMA_GRAPHQL: &str = r#"
type User {
  id: ID!
  account: Account
}

type Account {
  owner: User
}
"#;

/// FR-CG-08 / UAT-CG-03: a GraphQL field type binds to its declared type in the
/// same schema scope.
#[test]
fn graphql_field_types_bind_in_schema_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "schema.graphql", SCHEMA_GRAPHQL);

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let refs = artifact_edges(rt, EdgeKind::ArtifactRef);
    assert!(
        refs.contains(&(
            "User".into(),
            NodeKind::GqlType,
            "Account".into(),
            NodeKind::GqlType
        )),
        "User.account binds to the Account type: {refs:?}"
    );
    assert!(
        refs.contains(&(
            "Account".into(),
            NodeKind::GqlType,
            "User".into(),
            NodeKind::GqlType
        )),
        "Account.owner binds to the User type: {refs:?}"
    );

    // The built-in `ID` scalar produced no graphql-type candidate.
    let cov = coverage_by_relation(rt);
    let graphql_type = cov.get("graphql-type").expect("graphql-type coverage");
    assert_eq!(
        (graphql_type.bound, graphql_type.unresolved),
        (2, 0),
        "exactly the two in-scope field references bound; ID is no candidate"
    );
}

/// FR-CG-10 / NFR-RA-05 (GraphQL): a declared GraphQL type name binds to its one
/// type-like code symbol end-to-end; a duplicate code type name stays unresolved
/// — the artifact→code path proven for GraphQL, not only Protobuf.
#[test]
fn graphql_declared_names_bind_to_code_and_duplicates_stay_unresolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "schema.graphql", SCHEMA_GRAPHQL); // declares `User` and `Account`
                                                   // `Account` matches exactly one code struct → binds; `User` matches none.
    write(
        root,
        "src/models.rs",
        "pub struct Account { pub owner: String }\n",
    );
    // `Dup` is a GraphQL type matching TWO code structs → never binds.
    write(root, "schema_dup.graphql", "type Dup { id: ID! }\n");
    write(root, "src/dup_a.rs", "pub struct Dup { pub a: String }\n");
    write(root, "src/dup_b.rs", "pub struct Dup { pub b: String }\n");

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let bindings = artifact_edges(rt, EdgeKind::ArtifactBinding);
    // `Account` (GqlType) → `Account` (Struct): the one type-like code symbol.
    assert!(
        bindings.contains(&(
            "Account".into(),
            NodeKind::GqlType,
            "Account".into(),
            NodeKind::Struct
        )),
        "a declared GraphQL type with one code type binds: {bindings:?}"
    );
    // `Dup` matches two structs → no binding edge is fabricated.
    assert!(
        !bindings
            .iter()
            .any(|(s, sk, _, _)| s == "Dup" && *sk == NodeKind::GqlType),
        "an ambiguous GraphQL declared name is never fabricated: {bindings:?}"
    );
}

// ── UAT-CG-04: metric-neutrality with schema reference edges present ──────────

/// A small but non-trivial Rust graph (functions + a call edge) so the aggregate
/// signal is a real, stable value.
const APP_RS: &str = "\
pub fn run() -> i64 { helper() }
pub fn helper() -> i64 { 0 }
pub fn unused() -> i64 { 1 }
";

fn aggregate_signal(root: &Path) -> Option<u32> {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("view hydrates");
    let rt = engine.runtime().expect("runtime present");
    let (_, model) = metrics::snapshot(rt, &view, Some("sha"), metrics::Thresholds::default())
        .expect("snapshot runs");
    model.aggregate_signal
}

/// UAT-CG-04: adding the schema files — which create `ArtifactRef` and
/// `ArtifactBinding` edges, including an `ArtifactBinding` from `Common` to the
/// code struct — leaves the code graph's aggregate signal byte-identical. The
/// hydration edge predicate fences these edges at the same audit point as the
/// node predicate, so no cross-layer edge enters metrics.
#[test]
fn schema_reference_edges_are_metric_neutral() {
    // Code only.
    let bare = tempfile::tempdir().unwrap();
    write(bare.path(), "src/app.rs", APP_RS);
    write(bare.path(), "src/models.rs", MODELS_RS);
    let baseline = aggregate_signal(bare.path());

    // The same code plus proto + graphql schemas with bound reference edges.
    let withschema = tempfile::tempdir().unwrap();
    write(withschema.path(), "src/app.rs", APP_RS);
    write(withschema.path(), "src/models.rs", MODELS_RS);
    write(withschema.path(), "proto/common.proto", COMMON_PROTO);
    write(withschema.path(), "proto/user.proto", USER_PROTO);
    write(withschema.path(), "schema.graphql", SCHEMA_GRAPHQL);
    let with_edges = aggregate_signal(withschema.path());

    // Sanity: the schema project really did create artifact edges.
    let engine = Engine::start(withschema.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");
    assert!(
        !artifact_edges(rt, EdgeKind::ArtifactRef).is_empty()
            && !artifact_edges(rt, EdgeKind::ArtifactBinding).is_empty(),
        "the fixture must exercise both artifact edge kinds"
    );

    assert_eq!(
        baseline, with_edges,
        "the aggregate signal is byte-identical with artifact edges present (UAT-CG-04)"
    );
    assert!(baseline.is_some(), "the code graph has a real signal");
}
