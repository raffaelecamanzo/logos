//! The cross-artifact edge **payload** vocabulary and its deterministic
//! external-target classifier (CR-011, [ADR-26], [FR-CG-07]).
//!
//! The two cross-artifact edge kinds — [`EdgeKind::ArtifactRef`] (artifact→
//! artifact) and [`EdgeKind::ArtifactBinding`] (artifact→code) — are
//! **payload-subtyped**: instead of burning a frozen edge discriminant per
//! relation, every relation class (`proto-import`, `tf-module-call`, `route`,
//! `type-name`, …) is named in the edge's payload string, exactly as the
//! [ADR-25] node anchors subtype their kind via the `body` payload. This module
//! is the typed vocabulary of those relation classes plus the per-format rule
//! that classifies an extracted reference target as **workspace-relative**
//! (a binding candidate, captured into the ledger) or **external** (never a
//! candidate — no edge, no ledger entry, no retry noise).
//!
//! The vocabulary is the **stable contract** the three consumer stories build
//! on (S-069 OpenAPI routes, S-070 Protobuf/GraphQL, S-071 SQL/Terraform/shell):
//! each consumer's extraction walk captures references tagged with the relevant
//! [`ArtifactRelation`], and the resolution pass binds them under the same
//! exactly-one-candidate discipline as code and documentation references
//! ([NFR-RA-05]).
//!
//! # External classification is deterministic and per-format ([FR-CG-07])
//!
//! [`ArtifactRelation::classify_target`] is a pure function of the relation
//! class and the target text — no I/O, no graph lookup. An *absolute URL* is
//! external for every relation; beyond that each format contributes its own
//! rule: a Terraform registry source, a vendored proto import prefix
//! (`google/protobuf/…`), an interpolated shell path. A reference the classifier
//! cannot prove external is treated as workspace-relative and captured; if its
//! target is never indexed it stays an honest unresolved-ref miss rather than a
//! fabricated edge.
//!
//! [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
//! [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
//! [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use serde::{Deserialize, Serialize};

use super::{EdgeKind, NodeKind};

/// Whether an extracted reference target is a binding **candidate**.
///
/// The gate that keeps the `unresolved_refs` ledger an honest work list rather
/// than a noise archive ([FR-CG-07], [ADR-26]): only [`Workspace`] targets are
/// captured and retried; an [`External`] target produces no edge *and* no ledger
/// entry.
///
/// [`Workspace`]: TargetClass::Workspace
/// [`External`]: TargetClass::External
/// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
/// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    /// A workspace-relative target: a binding candidate. Captured into the
    /// ledger and retried each sync until its target is indexed or it is proven
    /// ambiguous — never fabricated ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    Workspace,
    /// A deterministically-classifiable external target (registry source,
    /// absolute URL, vendored import prefix, interpolated path): **never** a
    /// candidate. No edge, no `unresolved_refs` entry, no retry.
    External,
}

/// A cross-artifact reference's **relation class** — the payload that subtypes
/// an [`EdgeKind::ArtifactRef`]/[`EdgeKind::ArtifactBinding`] edge (CR-011,
/// [ADR-26], [FR-CG-07]).
///
/// Each variant fixes two things: the [`edge_kind`](ArtifactRelation::edge_kind)
/// the relation produces (artifact→artifact vs artifact→code) and the
/// [`classify_target`](ArtifactRelation::classify_target) rule that decides
/// candidacy. The [`as_str`](ArtifactRelation::as_str) token is the on-disk
/// payload string written to `unresolved_refs.payload` and `edges.payload`, and
/// the key the per-relation-class coverage counts group by ([FR-CG-11],
/// [FR-RS-04]).
///
/// Wire tokens are kebab-case to match the relation names in the specification
/// prose (`proto-import`, `tf-module-call`, `route`, `type-name`).
///
/// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
/// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
/// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
/// [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactRelation {
    /// A Protobuf `import "path"` → the sibling `ConfigFile` at that
    /// workspace-relative path (S-070). External: well-known vendored prefixes
    /// (`google/protobuf/…`, …).
    ProtoImport,
    /// A Protobuf field/RPC type reference → the declaring `ProtoMessage` within
    /// the package-honoring import closure (S-070).
    ProtoType,
    /// A GraphQL type→type field reference within the same schema scope (S-070).
    GraphqlType,
    /// A schema-declared type name (Protobuf/GraphQL) → the one type-like **code**
    /// symbol that implements it (S-070). Artifact→code; literal name only, no
    /// synthesized candidates. Its wire token is `type-name` (the relation the
    /// specification prose names), not the variant's kebab default.
    #[serde(rename = "type-name")]
    SchemaType,
    /// An OpenAPI `ApiOperation` → the framework `route` handler it specifies
    /// (S-069). Artifact→code; positional-template + method match.
    Route,
    /// A Terraform `module` call's `source` → each admitted `.tf` `ConfigFile` in
    /// the local source directory (S-071). External: registry sources
    /// (`hashicorp/aws`), remote sources (`git::…`, `github.com/…`).
    TfModuleCall,
    /// A Terraform `var.x`/`local.x`/`module.x.output` reference → its declaring
    /// `TfBlock` (S-071).
    TfVarRef,
    /// A SQL view/foreign-key clause → the `SqlObject` table it reads (S-071).
    SqlObjectRef,
    /// A shell `source ./path` with a **literal** workspace-relative path → the
    /// target `ConfigFile` (S-071). External: interpolated paths (`$DIR/x.sh`).
    ShellSource,
}

impl ArtifactRelation {
    /// Every relation class, in declaration order — the iteration universe tests
    /// and coverage surfaces drive off so a newly added relation cannot silently
    /// skip a contract assertion.
    pub const ALL: [ArtifactRelation; 9] = [
        ArtifactRelation::ProtoImport,
        ArtifactRelation::ProtoType,
        ArtifactRelation::GraphqlType,
        ArtifactRelation::SchemaType,
        ArtifactRelation::Route,
        ArtifactRelation::TfModuleCall,
        ArtifactRelation::TfVarRef,
        ArtifactRelation::SqlObjectRef,
        ArtifactRelation::ShellSource,
    ];

    /// The on-disk payload token (kebab-case, matching the `serde` form).
    pub const fn as_str(self) -> &'static str {
        match self {
            ArtifactRelation::ProtoImport => "proto-import",
            ArtifactRelation::ProtoType => "proto-type",
            ArtifactRelation::GraphqlType => "graphql-type",
            ArtifactRelation::SchemaType => "type-name",
            ArtifactRelation::Route => "route",
            ArtifactRelation::TfModuleCall => "tf-module-call",
            ArtifactRelation::TfVarRef => "tf-var-ref",
            ArtifactRelation::SqlObjectRef => "sql-object-ref",
            ArtifactRelation::ShellSource => "shell-source",
        }
    }

    /// Recover a relation class from its payload token, or `None` for an
    /// unknown string (a payload written by a newer schema, say).
    pub fn from_wire(token: &str) -> Option<ArtifactRelation> {
        ArtifactRelation::ALL
            .into_iter()
            .find(|r| r.as_str() == token)
    }

    /// The edge kind this relation produces: [`EdgeKind::ArtifactBinding`] for an
    /// artifact→**code** relation ([`SchemaType`], [`Route`]),
    /// [`EdgeKind::ArtifactRef`] for an artifact→**artifact** relation (everything
    /// else). The split is structural: `ArtifactBinding` edges are exactly the
    /// cross-layer edges the metric scope fences ([ADR-26]).
    ///
    /// [`SchemaType`]: ArtifactRelation::SchemaType
    /// [`Route`]: ArtifactRelation::Route
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    pub const fn edge_kind(self) -> EdgeKind {
        match self {
            ArtifactRelation::SchemaType | ArtifactRelation::Route => EdgeKind::ArtifactBinding,
            ArtifactRelation::ProtoImport
            | ArtifactRelation::ProtoType
            | ArtifactRelation::GraphqlType
            | ArtifactRelation::TfModuleCall
            | ArtifactRelation::TfVarRef
            | ArtifactRelation::SqlObjectRef
            | ArtifactRelation::ShellSource => EdgeKind::ArtifactRef,
        }
    }

    /// The specific artifact [`NodeKind`] a **literal-name** relation resolves to,
    /// or `None` for a path-based relation (resolved by file path, not name) or a
    /// code binding (resolved against code symbol kinds).
    ///
    /// Every name-matched artifact→artifact relation points at exactly one
    /// artifact node kind — a proto type reference at a [`ProtoMessage`], a GraphQL
    /// type reference at a [`GqlType`], a Terraform `var`/`local`/`module`
    /// reference at a [`TfBlock`], a SQL view/FK clause at a [`SqlObject`]. The
    /// binder filters name candidates to this kind so a reference can **never**
    /// bind across formats (a proto type name matching a same-named `TfBlock`) —
    /// the never-fabricate guarantee at the relation grain ([NFR-RA-05],
    /// [ADR-26]). Path relations ([`ProtoImport`], [`TfModuleCall`],
    /// [`ShellSource`]) resolve to a [`ConfigFile`] by path and code relations
    /// ([`SchemaType`], [`Route`]) resolve against code symbols, so both return
    /// `None` here.
    ///
    /// [`ProtoMessage`]: NodeKind::ProtoMessage
    /// [`GqlType`]: NodeKind::GqlType
    /// [`TfBlock`]: NodeKind::TfBlock
    /// [`SqlObject`]: NodeKind::SqlObject
    /// [`ConfigFile`]: NodeKind::ConfigFile
    /// [`ProtoImport`]: ArtifactRelation::ProtoImport
    /// [`TfModuleCall`]: ArtifactRelation::TfModuleCall
    /// [`ShellSource`]: ArtifactRelation::ShellSource
    /// [`SchemaType`]: ArtifactRelation::SchemaType
    /// [`Route`]: ArtifactRelation::Route
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    pub const fn target_kind(self) -> Option<NodeKind> {
        match self {
            ArtifactRelation::ProtoType => Some(NodeKind::ProtoMessage),
            ArtifactRelation::GraphqlType => Some(NodeKind::GqlType),
            ArtifactRelation::TfVarRef => Some(NodeKind::TfBlock),
            ArtifactRelation::SqlObjectRef => Some(NodeKind::SqlObject),
            ArtifactRelation::ProtoImport
            | ArtifactRelation::TfModuleCall
            | ArtifactRelation::ShellSource
            | ArtifactRelation::SchemaType
            | ArtifactRelation::Route => None,
        }
    }

    /// Classify `target` as a binding candidate ([`TargetClass::Workspace`]) or a
    /// non-candidate ([`TargetClass::External`]) — the deterministic, per-format
    /// rule ([FR-CG-07], [ADR-26]).
    ///
    /// An **absolute URL** (`scheme://…`) is external for every relation. Beyond
    /// that, each format contributes its own rule; a target that proves none of
    /// them is workspace-relative and captured. The rules are intentionally
    /// conservative — a missed external form degrades to honest ledger noise, not
    /// a fabricated edge.
    ///
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    pub fn classify_target(self, target: &str) -> TargetClass {
        // Universal: an absolute URL is never a workspace path (covers an
        // OpenAPI `$ref` to an absolute URL, a remote schema, …).
        if is_absolute_url(target) {
            return TargetClass::External;
        }
        match self {
            ArtifactRelation::ProtoImport => {
                if has_vendored_proto_prefix(target) {
                    TargetClass::External
                } else {
                    TargetClass::Workspace
                }
            }
            ArtifactRelation::TfModuleCall => {
                // A local Terraform module source is a relative or absolute
                // filesystem path; anything else (a registry address, a
                // `git::`/`github.com/` remote) is external (HCL semantics).
                if is_local_path(target) {
                    TargetClass::Workspace
                } else {
                    TargetClass::External
                }
            }
            ArtifactRelation::ShellSource => {
                // An interpolated path (`$DIR/x.sh`, `${BASE}/x.sh`) is not a
                // literal workspace target — never a candidate.
                if target.contains('$') {
                    TargetClass::External
                } else {
                    TargetClass::Workspace
                }
            }
            ArtifactRelation::GraphqlType => {
                // A GraphQL field whose type is a built-in scalar (`ID`, `String`,
                // `Int`, `Float`, `Boolean`) references no workspace-declared type
                // — it is never a candidate (no edge, no ledger entry), the
                // schema-format twin of the vendored proto-import rule (S-070).
                if is_graphql_builtin_scalar(target) {
                    TargetClass::External
                } else {
                    TargetClass::Workspace
                }
            }
            // The remaining relations carry no external form beyond the
            // universal absolute-URL rule: a declared type name, a SQL object
            // name, a `var.x` reference, a proto type reference (the proto walk
            // classifies vendored package-qualified references out structurally,
            // before the ledger — S-070).
            ArtifactRelation::ProtoType
            | ArtifactRelation::SchemaType
            | ArtifactRelation::Route
            | ArtifactRelation::TfVarRef
            | ArtifactRelation::SqlObjectRef => TargetClass::Workspace,
        }
    }

    /// Convenience: `true` iff [`classify_target`](ArtifactRelation::classify_target)
    /// classifies `target` as [`TargetClass::External`].
    pub fn is_external(self, target: &str) -> bool {
        self.classify_target(target) == TargetClass::External
    }
}

/// `true` if `target` is an absolute URL — a `scheme://` authority form. Cheap,
/// allocation-free, and deterministic.
fn is_absolute_url(target: &str) -> bool {
    target.contains("://")
}

/// `true` if `target` is a local filesystem path in Terraform source syntax: a
/// `./`/`../` relative path or an absolute `/` path. Everything else is a
/// registry or remote address (a non-candidate).
fn is_local_path(target: &str) -> bool {
    target.starts_with("./") || target.starts_with("../") || target.starts_with('/')
}

/// The well-known vendored Protobuf import prefixes that are never workspace
/// files — the standard library and common ecosystem protos a repo imports but
/// does not vendor. A target under one of these is external ([FR-CG-07]).
///
/// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
const VENDORED_PROTO_PREFIXES: &[&str] = &[
    "google/protobuf/",
    "google/api/",
    "google/rpc/",
    "google/type/",
    "google/longrunning/",
    "gogoproto/",
    "validate/",
    "protoc-gen-openapiv2/",
];

/// `true` if `target` begins with a [`VENDORED_PROTO_PREFIXES`] entry.
fn has_vendored_proto_prefix(target: &str) -> bool {
    VENDORED_PROTO_PREFIXES
        .iter()
        .any(|p| target.starts_with(p))
}

/// The five built-in GraphQL scalar types every schema may reference without
/// declaring. A field typed by one names no workspace artifact, so it is a
/// non-candidate ([FR-CG-07]) — the GraphQL twin of the vendored proto prefix.
///
/// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
const GRAPHQL_BUILTIN_SCALARS: &[&str] = &["ID", "String", "Int", "Float", "Boolean"];

/// `true` if `target` is one of the [`GRAPHQL_BUILTIN_SCALARS`].
fn is_graphql_builtin_scalar(target: &str) -> bool {
    GRAPHQL_BUILTIN_SCALARS.contains(&target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_relation_round_trips_through_its_wire_token() {
        for rel in ArtifactRelation::ALL {
            assert_eq!(ArtifactRelation::from_wire(rel.as_str()), Some(rel));
            // serde uses the same kebab token as `as_str`.
            let json = serde_json::to_string(&rel).unwrap();
            assert_eq!(json, format!("\"{}\"", rel.as_str()));
            assert_eq!(
                serde_json::from_str::<ArtifactRelation>(&json).unwrap(),
                rel
            );
        }
        assert_eq!(ArtifactRelation::from_wire("not-a-relation"), None);
    }

    #[test]
    fn edge_kind_splits_artifact_to_code_from_artifact_to_artifact() {
        // Artifact→code bindings.
        assert_eq!(
            ArtifactRelation::SchemaType.edge_kind(),
            EdgeKind::ArtifactBinding
        );
        assert_eq!(
            ArtifactRelation::Route.edge_kind(),
            EdgeKind::ArtifactBinding
        );
        // Artifact→artifact references.
        for rel in [
            ArtifactRelation::ProtoImport,
            ArtifactRelation::ProtoType,
            ArtifactRelation::GraphqlType,
            ArtifactRelation::TfModuleCall,
            ArtifactRelation::TfVarRef,
            ArtifactRelation::SqlObjectRef,
            ArtifactRelation::ShellSource,
        ] {
            assert_eq!(
                rel.edge_kind(),
                EdgeKind::ArtifactRef,
                "{} should be an artifact→artifact reference",
                rel.as_str()
            );
        }
    }

    #[test]
    fn name_relations_target_their_own_artifact_kind_path_and_code_relations_target_none() {
        // Each literal-name artifact→artifact relation points at exactly one
        // artifact node kind — the cross-format never-fabricate filter.
        assert_eq!(
            ArtifactRelation::ProtoType.target_kind(),
            Some(NodeKind::ProtoMessage)
        );
        assert_eq!(
            ArtifactRelation::GraphqlType.target_kind(),
            Some(NodeKind::GqlType)
        );
        assert_eq!(
            ArtifactRelation::TfVarRef.target_kind(),
            Some(NodeKind::TfBlock)
        );
        assert_eq!(
            ArtifactRelation::SqlObjectRef.target_kind(),
            Some(NodeKind::SqlObject)
        );
        // Path relations (resolved by file path) and code bindings carry no
        // artifact target kind.
        for rel in [
            ArtifactRelation::ProtoImport,
            ArtifactRelation::TfModuleCall,
            ArtifactRelation::ShellSource,
            ArtifactRelation::SchemaType,
            ArtifactRelation::Route,
        ] {
            assert_eq!(
                rel.target_kind(),
                None,
                "{} targets no artifact kind",
                rel.as_str()
            );
        }
        // A name relation's target kind is always an artifact (non-code) kind.
        for rel in ArtifactRelation::ALL {
            if let Some(kind) = rel.target_kind() {
                assert!(
                    kind.is_config(),
                    "{} target must be an artifact kind",
                    rel.as_str()
                );
            }
        }
    }

    #[test]
    fn absolute_urls_are_external_for_every_relation() {
        for rel in ArtifactRelation::ALL {
            assert!(
                rel.is_external("https://example.com/schema.json"),
                "{} must treat an absolute URL as external",
                rel.as_str()
            );
            assert!(rel.is_external("git://host/repo.git"));
        }
    }

    #[test]
    fn proto_import_vendored_prefixes_are_external_workspace_paths_are_not() {
        let rel = ArtifactRelation::ProtoImport;
        assert!(rel.is_external("google/protobuf/timestamp.proto"));
        assert!(rel.is_external("google/api/annotations.proto"));
        assert!(rel.is_external("gogoproto/gogo.proto"));
        // A workspace-relative sibling import is a candidate.
        assert_eq!(
            rel.classify_target("common/types.proto"),
            TargetClass::Workspace
        );
        assert_eq!(rel.classify_target("./types.proto"), TargetClass::Workspace);
    }

    #[test]
    fn terraform_registry_sources_are_external_local_paths_are_not() {
        let rel = ArtifactRelation::TfModuleCall;
        // Registry / remote sources: non-candidates.
        assert!(rel.is_external("hashicorp/aws"));
        assert!(rel.is_external("terraform-aws-modules/vpc/aws"));
        assert!(rel.is_external("github.com/org/repo//modules/x"));
        assert!(rel.is_external("git::https://example.com/vpc.git"));
        assert!(rel.is_external("app.terraform.io/example/vpc/aws"));
        // Local module sources: candidates.
        assert_eq!(rel.classify_target("./modules/vpc"), TargetClass::Workspace);
        assert_eq!(rel.classify_target("../shared"), TargetClass::Workspace);
        assert_eq!(
            rel.classify_target("/abs/modules/x"),
            TargetClass::Workspace
        );
    }

    #[test]
    fn shell_source_interpolated_paths_are_external_literals_are_not() {
        let rel = ArtifactRelation::ShellSource;
        assert!(rel.is_external("$DIR/common.sh"));
        assert!(rel.is_external("${BASE}/lib.sh"));
        assert_eq!(
            rel.classify_target("./lib/common.sh"),
            TargetClass::Workspace
        );
        assert_eq!(rel.classify_target("lib/common.sh"), TargetClass::Workspace);
    }

    #[test]
    fn graphql_builtin_scalars_are_external_declared_types_are_not() {
        let rel = ArtifactRelation::GraphqlType;
        for builtin in ["ID", "String", "Int", "Float", "Boolean"] {
            assert!(
                rel.is_external(builtin),
                "the built-in scalar {builtin} is not a workspace type reference"
            );
        }
        // A schema-declared type is a candidate (case-sensitive: `string` is not
        // the built-in `String`).
        assert_eq!(rel.classify_target("Account"), TargetClass::Workspace);
        assert_eq!(rel.classify_target("DateTime"), TargetClass::Workspace);
        assert_eq!(rel.classify_target("string"), TargetClass::Workspace);
    }

    #[test]
    fn name_relations_have_no_external_form_beyond_urls() {
        // `GraphqlType` is excluded: it now carries a format-specific external
        // form (built-in scalars), covered by
        // `graphql_builtin_scalars_are_external_declared_types_are_not`.
        for rel in [
            ArtifactRelation::ProtoType,
            ArtifactRelation::SchemaType,
            ArtifactRelation::Route,
            ArtifactRelation::TfVarRef,
            ArtifactRelation::SqlObjectRef,
        ] {
            assert_eq!(
                rel.classify_target("SomeName"),
                TargetClass::Workspace,
                "{} should treat a bare name as a candidate",
                rel.as_str()
            );
        }
    }
}
