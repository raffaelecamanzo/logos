//! The cross-artifact reference-capture seam of the config extraction walk
//! (S-068, CR-011, [ADR-26], [FR-CG-07]) and its Protobuf/GraphQL arms (S-070).
//!
//! Sprint 10 ([ADR-25]) shipped the config & artifact layer `Contains`-only: its
//! typed anchors (`ProtoMessage`, `TfBlock`, `SqlObject`, `ApiOperation`,
//! `GqlType`, shell functions) exist but reference nothing. This module is the
//! **substrate** that lets each format's walk capture the *references* between
//! those anchors and to code, mirroring how [`super::anchors::extract_typed_anchors`]
//! dispatches the typed-anchor walks: a single [`capture_artifact_refs`]
//! dispatch point invoked from [`super::extract_one_config`], plus the shared
//! [`push_artifact_ref`] helper every per-format arm calls.
//!
//! The captured facts flow into the `unresolved_refs` ledger and are bound by
//! the resolution pass (pass 2) under the same exactly-one-candidate discipline
//! as code and documentation references ([NFR-RA-05]) — see the resolution
//! engine's artifact clients.
//!
//! # Externals never enter the ledger ([FR-CG-07])
//!
//! [`push_artifact_ref`] is the choke point that enforces the external-target
//! rule: before a reference fact is recorded, the relation's
//! [`ArtifactRelation::classify_target`] decides candidacy. A deterministically
//! external target (a registry source, an absolute URL, a vendored proto import,
//! a GraphQL built-in scalar, an interpolated shell path) produces **no fact at
//! all** — no edge, and no ledger entry to retry — so the ledger stays an honest
//! work list of genuine workspace-relative misses, never a noise archive of
//! permanently-unbindable externals ([ADR-26]).
//!
//! # The per-format dispatch is the consumers' extension point
//!
//! [`capture_artifact_refs`] dispatches on the plugin name. S-070 adds the
//! Protobuf ([`proto`]) and GraphQL ([`graphql`]) arms; S-069 (OpenAPI
//! operation→route) and S-071 (SQL/Terraform/shell) add the remaining arms. Each
//! arm walks its parsed tree and calls [`push_artifact_ref`] for every reference
//! it finds, sourcing artifact→artifact and artifact→code references from the
//! typed anchor the reference lives in (a message field → its `ProtoMessage`, a
//! GraphQL field → its `GqlType`). Anchor source symbols are reconstructed via
//! [`super::resolve_anchor_identities`], the single source of truth shared with
//! the anchor-emitting walk, so a reference's source is byte-identical to the
//! emitted anchor node ([NFR-RA-06]).
//!
//! [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
//! [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
//! [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, HashMap};

use tree_sitter::Node;

use crate::extract::{Facts, RefFact, SymbolContext};
use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeKind, RefForm};
use crate::plugin::LanguagePlugin;

/// Record one cross-artifact reference fact, applying the external-target gate
/// (S-068, CR-011, [FR-CG-07]).
///
/// The single helper every per-format capture arm calls. It is the substrate's
/// enforcement of two invariants:
///
/// 1. **Externals are never candidates** — if `relation` classifies `target` as
///    [`TargetClass::External`](crate::model::TargetClass::External), the
///    reference is dropped: no [`RefFact`] is pushed, so the resolution pass never
///    sees it (no edge, no ledger entry, [ADR-26]). Returns `false`.
/// 2. **The edge kind follows the relation** — a captured fact's
///    [`EdgeKind`](crate::model::EdgeKind) is always
///    [`relation.edge_kind()`](ArtifactRelation::edge_kind), so artifact→artifact
///    vs artifact→code is decided in one place, never by the caller.
///
/// Returns `true` when the reference was captured (a workspace-relative
/// candidate), `false` when it was classified external and dropped.
///
/// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
/// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
pub(super) fn push_artifact_ref(
    facts: &mut Facts,
    source: &LogosSymbol,
    target: &str,
    relation: ArtifactRelation,
    form: RefForm,
    line: u32,
) -> bool {
    // Externals are never candidates: no edge, no ledger entry (FR-CG-07).
    if relation.is_external(target) {
        return false;
    }
    facts.refs.push(RefFact {
        source: source.clone(),
        target: target.to_string(),
        alias: None,
        form,
        // The edge kind is a function of the relation — never the caller's call.
        kind: relation.edge_kind(),
        line,
        relation: Some(relation),
    });
    true
}

/// One captured cross-service **invocation site** — the language-neutral slots a
/// per-language capture query (`.scm`) pulled from a single call / publish /
/// subscribe expression, plus the enclosing anchor it is attributed to
/// ([FR-WS-07], [ADR-54]).
///
/// The `slots` map is opaque to the [`capture_invocation_refs`] interpreter (an
/// arm names its own capture slots — `method`/`path` for HTTP, `package`/
/// `service`/`method` for gRPC, `topic` for a broker); the interpreter passes
/// them verbatim to the arm's normalizer.
///
/// Consumed by the S-252/S-253/S-254 invocation arms; the contract ships the
/// carrier and interpreter so an arm is "one relation variant + one capture
/// file", never new plumbing here.
///
/// [FR-WS-07]: ../../../../docs/specs/requirements/FR-WS-07.md
/// [ADR-54]: ../../../../docs/specs/architecture/decisions/ADR-54.md
#[allow(dead_code)] // exercised by tests now; consumed by the S-252/253/254 arms.
pub(crate) struct InvocationSite {
    /// The enclosing anchor/function symbol the invocation is attributed to.
    pub(crate) source: LogosSymbol,
    /// The arm's named captures for this site (`capture-name → text`).
    pub(crate) slots: BTreeMap<String, String>,
    /// The 1-based source line the invocation is reported at.
    pub(crate) line: u32,
}

/// The generic, **arm-agnostic** consumer-side capture interpreter ([FR-WS-07],
/// [ADR-54]).
///
/// Drives the per-language captured `sites` through the arm's `render_target`
/// normalizer and funnels each rendered target through the existing
/// [`push_artifact_ref`] emission point under the arm's `relation`. This is the
/// reuse foundation the three invocation arms share — the loop, the refusal
/// discipline, and the emission choke-point are written once; an arm brings only
/// its per-language `.scm` (→ `sites`), its normalizer (`render_target`, e.g.
/// `route_key`/`grpc_key`/topic), and its [`ArtifactRelation`] variant.
///
/// Three invariants fall out for free, so every arm inherits them:
///
/// 1. **A language lacking the capture contributes nothing** — a language with
///    no capture file yields no `sites`, so the loop emits nothing.
/// 2. **Never fabricate** ([NFR-RA-05]) — a site the normalizer cannot reduce to
///    a static portable key (a runtime-composed path, a dynamic topic) is
///    refused by returning `None`, contributing no reference and no ledger entry.
/// 3. **Externals never enter the ledger** ([FR-CG-07]) — a rendered target the
///    relation classifies external is dropped by [`push_artifact_ref`]'s gate.
///
/// Returns the number of references captured. The routine is deliberately
/// relation-agnostic (it does not require `relation.is_invocation_arm()`), so
/// the contract's own tests exercise it with a stand-in relation before any real
/// arm variant exists.
///
/// [FR-WS-07]: ../../../../docs/specs/requirements/FR-WS-07.md
/// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-54]: ../../../../docs/specs/architecture/decisions/ADR-54.md
#[allow(dead_code)] // the reuse foundation; the S-252/253/254 arms are its callers.
pub(crate) fn capture_invocation_refs<F>(
    facts: &mut Facts,
    relation: ArtifactRelation,
    form: RefForm,
    sites: impl IntoIterator<Item = InvocationSite>,
    render_target: F,
) -> usize
where
    F: Fn(&BTreeMap<String, String>) -> Option<String>,
{
    let mut emitted = 0;
    for site in sites {
        // The arm's normalizer refuses a runtime-composed / non-normalizable site
        // by returning None: it contributes nothing (never fabricate).
        let Some(target) = render_target(&site.slots) else {
            continue;
        };
        // push_artifact_ref applies the external-target gate; an external target
        // produces no fact and no ledger entry.
        if push_artifact_ref(facts, &site.source, &target, relation, form, site.line) {
            emitted += 1;
        }
    }
    emitted
}

/// Capture every cross-artifact reference fact for one config/artifact file
/// (S-068, CR-011, [FR-CG-07]).
///
/// The reference-capture sibling of [`super::anchors::extract_typed_anchors`],
/// invoked from [`super::extract_one_config`] after the structural anchors are
/// emitted and before the canonical sort. The substrate ships the dispatch seam
/// and the shared [`push_artifact_ref`] helper, and each consumer story
/// (S-069/070/071) adds its format arm here.
///
/// The first arm (S-069, [FR-CG-09]) is the OpenAPI operation→route capture
/// ([`capture_openapi_routes`]). It is **content-sniffed, not plugin-keyed**: the
/// OpenAPI profile promotion ([`super::profiles`], S-067) runs *before* this seam
/// and emits `ApiPath`/`ApiOperation` nodes into `facts` for any YAML/JSON
/// document that sniffs as a spec, so the presence of those nodes — not
/// `plugin.name()` — is the signal. A document that did not sniff has no
/// `ApiOperation` nodes, so the arm is a no-op. The remaining arms (S-070
/// Protobuf/GraphQL, S-071 SQL/Terraform/shell) dispatch on `plugin.name()` to a
/// per-format capture walk over `root`, built from `segments`/`source`/`ctx`; a
/// format with no arm is a no-op.
///
/// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
/// [FR-CG-09]: ../../../../docs/specs/requirements/FR-CG-09.md
pub(super) fn capture_artifact_refs(
    plugin: &dyn LanguagePlugin,
    root: Node<'_>,
    segments: &[&str],
    parent_symbol: &LogosSymbol,
    source: &[u8],
    ctx: &SymbolContext,
    facts: &mut Facts,
) {
    // S-069: OpenAPI operation→route references, keyed on the promoted nodes the
    // content-sniffed profile already emitted into `facts` (no re-parse). A
    // document with no `ApiOperation` nodes is a no-op.
    capture_openapi_routes(facts);

    // S-071: SQL / Terraform / Shell arms — sourced directly from the parse tree
    // and the anchors already emitted into `facts`; these formats carry no config
    // descriptor, so they run before the descriptor gate below.
    match plugin.name() {
        "sql" => sql::capture(root, source, facts),
        "terraform" => terraform::capture(root, source, facts),
        "shell" => shell::capture(root, parent_symbol, source, facts),
        _ => {}
    }

    // S-070: Protobuf / GraphQL arms — keyed on the config descriptor (the anchor
    // table the reference sources are reconstructed from); without one there is
    // nothing to source references from.
    let Some(cfg) = plugin.config_extraction() else {
        return;
    };
    match plugin.name() {
        "protobuf" => proto::capture(cfg, root, segments, parent_symbol, source, ctx, facts),
        "graphql" => graphql::capture(cfg, root, segments, parent_symbol, source, ctx, facts),
        _ => {}
    }
}

/// Capture one `Route` reference per OpenAPI `ApiOperation` node already emitted
/// into `facts` (S-069, CR-011, [FR-CG-09]).
///
/// The OpenAPI profile promotion ([`super::profiles`]) shapes a spec into an
/// `ApiPath` per path template (its `name` is the template, e.g. `/users/{id}`)
/// with one `ApiOperation` child per HTTP method (its `name` is the lower-cased
/// method, e.g. `get`) hung off it by [`EdgeKind::Contains`]. For each operation
/// we recover its path template (the parent `ApiPath`'s name) and method (its own
/// name) and push a single reference rendered `"METHOD /template"` — the exact
/// shape a framework [`NodeKind::Route`] node's `name` carries — under the
/// [`ArtifactRelation::Route`] class. The resolution pass binds it to the one
/// route whose method and positionally-normalized template match, via the shared
/// normalizer; externals (an absolute-URL template) are dropped by
/// [`push_artifact_ref`]'s gate, and an operation with no matching route stays in
/// the ledger for the next sync ([NFR-RA-05]).
///
/// Reading the already-promoted nodes (rather than re-walking the tree) keeps
/// this capture in lockstep with the profile promotion — the same templates,
/// methods, and identities — and makes it inherently format-agnostic across the
/// YAML and JSON spec dialects ([NFR-RA-06]).
///
/// [FR-CG-09]: ../../../../docs/specs/requirements/FR-CG-09.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn capture_openapi_routes(facts: &mut Facts) {
    // Each `ApiPath` symbol → its path-template name (`/users/{id}`).
    let path_template: HashMap<&str, &str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ApiPath)
        .map(|n| (n.symbol.as_str(), n.name.as_str()))
        .collect();
    if path_template.is_empty() {
        return; // not an OpenAPI document — nothing to capture
    }

    // Each `ApiOperation` symbol → its containing `ApiPath` symbol, from the
    // `Contains` tree the promotion built (ApiPath --Contains--> ApiOperation).
    let operations: std::collections::HashSet<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ApiOperation)
        .map(|n| n.symbol.as_str())
        .collect();
    let parent_of: HashMap<&str, &str> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains && operations.contains(e.target.as_str()))
        .map(|e| (e.target.as_str(), e.source.as_str()))
        .collect();

    // Collect each operation's `(symbol, "METHOD /template", line)` first so the
    // immutable read of `facts.nodes`/`facts.edges` is finished before the
    // mutable push below.
    let to_capture: Vec<(LogosSymbol, String, u32)> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ApiOperation)
        .filter_map(|op| {
            let template = parent_of
                .get(op.symbol.as_str())
                .and_then(|parent| path_template.get(parent))?;
            let target = format!("{} {}", op.name.to_ascii_uppercase(), template);
            Some((op.symbol.clone(), target, op.start_line))
        })
        .collect();

    for (source, target, line) in to_capture {
        push_artifact_ref(
            facts,
            &source,
            &target,
            ArtifactRelation::Route,
            RefForm::Path,
            line,
        );
    }
}

/// The 1-based start line of `node` (the line a captured reference is reported
/// at).
fn line_of(node: Node<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

/// The first direct named child of `node` whose kind is `kind`, if any. Index
/// iteration keeps the returned node's lifetime tied to the tree, not a local
/// `walk()` cursor borrow. (Mirrors `anchors::sql::first_child_of_kind`; kept
/// local to this isolated arm — a shared `config/` tree-walk utility is a
/// worthwhile follow-up consolidation but out of this story's scope.)
fn first_named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    for i in 0..node.named_child_count() {
        let child = node.named_child(i)?;
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

/// The trimmed UTF-8 text of `node`, or `None` when non-UTF-8 or empty.
fn non_empty_text<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let text = node.utf8_text(source).ok()?.trim();
    (!text.is_empty()).then_some(text)
}

/// Protobuf reference capture (S-070, CR-011, [FR-CG-08], [FR-CG-10]).
///
/// Three reference families over the `ProtoMessage`/`ProtoService` anchors the
/// substrate already emits:
///
/// - **`import "path"`** → the sibling `ConfigFile` at that workspace-relative
///   path ([`ArtifactRelation::ProtoImport`]); a vendored prefix
///   (`google/protobuf/…`) is classified out and produces no ledger entry.
/// - **field / RPC type references** → the declaring `ProtoMessage`
///   ([`ArtifactRelation::ProtoType`]). Only **unqualified** references are
///   candidates: a package-qualified reference (`google.protobuf.Timestamp`, a
///   cross-package `pkg.Foo`) is not captured, because the substrate binder
///   resolves by simple name with no package model, so honoring the qualifier is
///   impossible — skipping it avoids both vendored ledger noise and cross-package
///   mis-binds ([NFR-RA-05], never fabricate).
/// - **declared message names** → the one type-like **code** symbol of that name
///   ([`ArtifactRelation::SchemaType`], an `ArtifactBinding`), with no synthesized
///   candidates ([FR-CG-10]).
///
/// [FR-CG-08]: ../../../../docs/specs/requirements/FR-CG-08.md
/// [FR-CG-10]: ../../../../docs/specs/requirements/FR-CG-10.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
mod proto {
    use super::*;

    pub(super) fn capture(
        cfg: &crate::plugin::ConfigDescriptor,
        root: Node<'_>,
        segments: &[&str],
        config_file_symbol: &LogosSymbol,
        source: &[u8],
        ctx: &SymbolContext,
        facts: &mut Facts,
    ) {
        // File-level imports are sourced from the ConfigFile root.
        capture_imports(root, config_file_symbol, source, facts);

        // Per-anchor references: type references and declared-name code bindings,
        // each sourced from its enclosing typed anchor.
        for anchor in super::super::resolve_anchor_identities(cfg, ctx, segments, source, root) {
            match anchor.kind {
                NodeKind::ProtoMessage => {
                    // The declared message name → its one type-like code symbol.
                    push_artifact_ref(
                        facts,
                        &anchor.symbol,
                        &anchor.name,
                        ArtifactRelation::SchemaType,
                        RefForm::Method,
                        line_of(anchor.node),
                    );
                    // The message's own field types (excluding nested messages,
                    // whose field types belong to their own anchor).
                    capture_type_refs(anchor.node, &anchor.symbol, source, facts);
                }
                NodeKind::ProtoService => {
                    // The RPC request/response types referenced by the service.
                    capture_type_refs(anchor.node, &anchor.symbol, source, facts);
                }
                _ => {}
            }
        }
    }

    /// Push a [`ProtoImport`](ArtifactRelation::ProtoImport) reference per
    /// top-level `import` statement, targeting the imported path. The external
    /// gate in [`push_artifact_ref`] drops vendored imports.
    fn capture_imports(
        root: Node<'_>,
        config_file_symbol: &LogosSymbol,
        source: &[u8],
        facts: &mut Facts,
    ) {
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() != "import" {
                continue;
            }
            let Some(string_node) = first_named_child(child, "string") else {
                continue;
            };
            let Some(raw) = non_empty_text(string_node, source) else {
                continue;
            };
            // Proto string literals are quoted; the path is the unquoted text.
            let path = raw.trim_matches(|c| c == '"' || c == '\'');
            // A residual quote means a concatenated string literal (`"a" "b"`),
            // which is not a clean import path — skip rather than emit a garbled
            // target (never fabricate; the case does not occur for real imports).
            if path.is_empty() || path.contains('"') || path.contains('\'') {
                continue;
            }
            push_artifact_ref(
                facts,
                config_file_symbol,
                path,
                ArtifactRelation::ProtoImport,
                RefForm::Path,
                line_of(child),
            );
        }
    }

    /// Push a [`ProtoType`](ArtifactRelation::ProtoType) reference per
    /// **unqualified** `message_or_enum_type` within `node`, sourced from
    /// `source_symbol`. Descent stops at nested `message`/`service` anchors so a
    /// reference is attributed to the anchor it textually lives in.
    fn capture_type_refs(
        node: Node<'_>,
        source_symbol: &LogosSymbol,
        source: &[u8],
        facts: &mut Facts,
    ) {
        let mut refs = Vec::new();
        collect_type_ref_nodes(node, &mut refs);
        for met in refs {
            // Only a single-identifier (unqualified) reference is a candidate:
            // a package-qualified reference cannot be honored by a name-based
            // binder and is skipped (never fabricated).
            if let Some(name) = unqualified_type_name(met, source) {
                push_artifact_ref(
                    facts,
                    source_symbol,
                    name,
                    ArtifactRelation::ProtoType,
                    RefForm::Method,
                    line_of(met),
                );
            }
        }
    }

    /// Collect every `message_or_enum_type` under `node`, not descending into a
    /// nested `message`/`service` (those carry their own anchor and capture their
    /// own references). A `message_or_enum_type` is recorded and not descended
    /// into — its identifiers are read directly.
    fn collect_type_ref_nodes<'tree>(node: Node<'tree>, out: &mut Vec<Node<'tree>>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "message" | "service" => {}
                "message_or_enum_type" => out.push(child),
                _ => collect_type_ref_nodes(child, out),
            }
        }
    }

    /// The single type name of an **unqualified** `message_or_enum_type`, or
    /// `None` when it is package-qualified (more than one `identifier`) — the
    /// never-fabricate guard for cross-package references.
    fn unqualified_type_name<'a>(met: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
        let mut cursor = met.walk();
        let idents: Vec<Node<'_>> = met
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "identifier")
            .collect();
        match idents.as_slice() {
            [only] => non_empty_text(*only, source),
            _ => None,
        }
    }
}

/// GraphQL reference capture (S-070, CR-011, [FR-CG-08], [FR-CG-10]).
///
/// Over the `GqlType` anchors the substrate emits:
///
/// - **field type references** → the referenced `GqlType` in the same schema
///   scope ([`ArtifactRelation::GraphqlType`]). Only object/interface/input field
///   types are references; `implements`, union members, enum values, and field
///   **argument** input types are not field references and are out of scope.
///   Built-in scalars (`ID`, `String`, `Int`, `Float`, `Boolean`) are classified
///   out by the relation's external rule, so they produce no ledger entry.
/// - **declared type names** → the one type-like **code** symbol of that name
///   ([`ArtifactRelation::SchemaType`], an `ArtifactBinding`), no synthesized
///   candidates ([FR-CG-10]).
///
/// [FR-CG-08]: ../../../../docs/specs/requirements/FR-CG-08.md
/// [FR-CG-10]: ../../../../docs/specs/requirements/FR-CG-10.md
mod graphql {
    use super::*;

    pub(super) fn capture(
        cfg: &crate::plugin::ConfigDescriptor,
        root: Node<'_>,
        segments: &[&str],
        // GraphQL has no file-level reference family (every reference is sourced
        // from a typed `GqlType` anchor), so the `ConfigFile` root symbol is
        // unused here — but the arm keeps the substrate's symmetric intake shape
        // so a future file-level GraphQL reference can source from it without a
        // signature change.
        _config_file_symbol: &LogosSymbol,
        source: &[u8],
        ctx: &SymbolContext,
        facts: &mut Facts,
    ) {
        for anchor in super::super::resolve_anchor_identities(cfg, ctx, segments, source, root) {
            if anchor.kind != NodeKind::GqlType {
                continue;
            }
            // The declared type name → its one type-like code symbol.
            push_artifact_ref(
                facts,
                &anchor.symbol,
                &anchor.name,
                ArtifactRelation::SchemaType,
                RefForm::Method,
                line_of(anchor.node),
            );
            // Field type references within this type definition's scope.
            for named in field_named_types(anchor.node) {
                if let Some(name) = named_type_name(named, source) {
                    push_artifact_ref(
                        facts,
                        &anchor.symbol,
                        name,
                        ArtifactRelation::GraphqlType,
                        RefForm::Method,
                        line_of(named),
                    );
                }
            }
        }
    }

    /// Every `named_type` that is a **field type** within a field-definition
    /// container of `node` — the object/interface `fields_definition` and the
    /// input `input_fields_definition`. Restricting to these containers excludes
    /// `implements_interfaces` and `union_member_types`; descent additionally
    /// stops at `arguments_definition`, so a field *argument*'s input type
    /// (`posts(filter: PostFilter): Post` → `PostFilter`) is **not** a field type
    /// reference — only the field's own type (`Post`) is. All are type→type
    /// relations, but S-070's scope is the field's declared type.
    fn field_named_types<'tree>(node: Node<'tree>) -> Vec<Node<'tree>> {
        let mut containers = Vec::new();
        collect_kind(
            node,
            &["fields_definition", "input_fields_definition"],
            &mut containers,
        );
        let mut out = Vec::new();
        for container in containers {
            collect_field_type_named_types(container, &mut out);
        }
        out
    }

    /// Collect every `named_type` under `node` that is a field's declared type,
    /// not descending into an `arguments_definition` (a field argument's input
    /// type is not a field type reference). A wrapped type (`[Role!]!`) is still
    /// reached because the wrappers are descended into.
    fn collect_field_type_named_types<'tree>(node: Node<'tree>, out: &mut Vec<Node<'tree>>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "arguments_definition" => {}
                "named_type" => out.push(child),
                _ => collect_field_type_named_types(child, out),
            }
        }
    }

    /// Collect every descendant of `node` (and `node` itself is not matched) whose
    /// kind is in `kinds`; a matched node is recorded and still descended into so
    /// a wrapped type (`[Role!]!` → the inner `named_type`) is found.
    fn collect_kind<'tree>(node: Node<'tree>, kinds: &[&str], out: &mut Vec<Node<'tree>>) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if kinds.contains(&child.kind()) {
                out.push(child);
            }
            collect_kind(child, kinds, out);
        }
    }

    /// The type name of a `named_type` node — the text of its `name` child.
    fn named_type_name<'a>(named: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
        first_named_child(named, "name").and_then(|n| non_empty_text(n, source))
    }
}

/// The reference-capture walks re-traverse the same parse tree the anchor walk
/// did, so a referencing node's `SqlObject`/`TfBlock` anchor is found by its line
/// span rather than by reconstructing its `path#anchor` symbol (which would
/// duplicate the anchor walk's name/ordinal logic and risk drift). Returns `None`
/// when zero or **several** anchors share the span, so an ambiguous source line
/// captures no reference rather than attributing an edge to the wrong anchor
/// ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
fn anchor_symbol_at(
    facts: &Facts,
    kind: NodeKind,
    start_line: u32,
    end_line: u32,
) -> Option<LogosSymbol> {
    let mut matches = facts
        .nodes
        .iter()
        .filter(|n| n.kind == kind && n.start_line == start_line && n.end_line == end_line);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None; // ambiguous span — never fabricate a wrong-source edge
    }
    Some(first.symbol.clone())
}

/// The qualified name of a SQL `object_reference` node (`schema.name`, or `name`
/// when unqualified) — the same construction [`super::anchors`] uses for a
/// `SqlObject` anchor's object name, so a captured table reference's target text
/// matches the table anchor's `<type> <name>`.
fn sql_object_reference_name(obj: Node<'_>, source: &[u8]) -> Option<String> {
    let name = obj
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())?;
    match obj
        .child_by_field_name("schema")
        .and_then(|n| n.utf8_text(source).ok())
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
    {
        Some(schema) => Some(format!("{schema}.{name}")),
        None => Some(name.to_string()),
    }
}

/// SQL view/foreign-key → table references ([FR-CG-08], `sql-object-ref`).
mod sql {
    use super::*;

    /// Capture every SQL table reference: a `CREATE VIEW`'s `FROM`/`JOIN` tables
    /// and a `CREATE TABLE`'s foreign-key targets, each bound (by name) to the
    /// `SqlObject` table it reads. Parsed DDL only — a statement carrying a syntax
    /// error is skipped (its anchor was never emitted either), so unparseable SQL
    /// yields no reference candidates ([NFR-RA-05]).
    pub(super) fn capture(root: Node<'_>, source: &[u8], facts: &mut Facts) {
        // (source SqlObject symbol, target table name, line) — collected against
        // the immutable `facts.nodes`, then pushed (which borrows `facts` mutably).
        let mut captured: Vec<(LogosSymbol, String, u32)> = Vec::new();
        let mut cursor = root.walk();
        for stmt in root.named_children(&mut cursor) {
            let Some(ddl) = ddl_node(stmt) else {
                continue;
            };
            if ddl.has_error() {
                continue; // misparse → no anchor, no reference (NFR-RA-05)
            }
            let start_line = ddl.start_position().row as u32 + 1;
            let end_line = ddl.end_position().row as u32 + 1;
            let Some(src_sym) = anchor_symbol_at(facts, NodeKind::SqlObject, start_line, end_line)
            else {
                continue;
            };
            match ddl.kind() {
                "create_view" | "create_materialized_view" => {
                    collect_view_table_refs(ddl, source, &src_sym, &mut captured);
                }
                "create_table" => collect_fk_refs(ddl, source, &src_sym, &mut captured),
                _ => {}
            }
        }
        for (src, table, line) in dedup(captured) {
            // The table anchor's name leads with its object-type payload
            // (`table app.users`), so the reference target must too.
            let target = format!("table {table}");
            push_artifact_ref(
                facts,
                &src,
                &target,
                ArtifactRelation::SqlObjectRef,
                RefForm::Method,
                line,
            );
        }
    }

    /// The `create_*` definition node for a top-level item: the item itself, or its
    /// first `create_*` child (the common `statement → create_*` wrapping). Mirrors
    /// the anchor walk's `ddl_node` so reference capture sees the same nodes.
    fn ddl_node(item: Node<'_>) -> Option<Node<'_>> {
        if is_create(item.kind()) {
            return Some(item);
        }
        for i in 0..item.named_child_count() {
            let child = item.named_child(i)?;
            if is_create(child.kind()) {
                return Some(child);
            }
        }
        None
    }

    fn is_create(kind: &str) -> bool {
        kind.starts_with("create_")
    }

    /// A view's referenced tables: the `object_reference` inside each `relation`
    /// (FROM/JOIN) within the view's query. The view's own name is a direct child
    /// of the `create_view`, never inside a `relation`, so it is not captured.
    fn collect_view_table_refs(
        ddl: Node<'_>,
        source: &[u8],
        src_sym: &LogosSymbol,
        out: &mut Vec<(LogosSymbol, String, u32)>,
    ) {
        for relation in descendants_of_kind(ddl, "relation") {
            for i in 0..relation.named_child_count() {
                let Some(child) = relation.named_child(i) else {
                    continue;
                };
                if child.kind() == "object_reference" {
                    if let Some(name) = sql_object_reference_name(child, source) {
                        out.push((src_sym.clone(), name, child.start_position().row as u32 + 1));
                    }
                }
            }
        }
    }

    /// A table's foreign-key targets: the `object_reference` that follows a
    /// `keyword_references` (in a table-level `constraint` or an inline column
    /// definition) names the referenced table.
    fn collect_fk_refs(
        ddl: Node<'_>,
        source: &[u8],
        src_sym: &LogosSymbol,
        out: &mut Vec<(LogosSymbol, String, u32)>,
    ) {
        for kr in descendants_of_kind(ddl, "keyword_references") {
            let Some(parent) = kr.parent() else {
                continue;
            };
            // The first `object_reference` after the REFERENCES keyword is the
            // referenced table.
            for i in 0..parent.named_child_count() {
                let Some(child) = parent.named_child(i) else {
                    continue;
                };
                if child.kind() == "object_reference" && child.start_byte() > kr.start_byte() {
                    if let Some(name) = sql_object_reference_name(child, source) {
                        out.push((src_sym.clone(), name, kr.start_position().row as u32 + 1));
                    }
                    break;
                }
            }
        }
    }

    /// Every descendant of `node` (excluding `node` itself) whose kind is `kind`,
    /// in document order.
    fn descendants_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Vec<Node<'tree>> {
        let mut out = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect(child, kind, &mut out);
        }
        out
    }

    fn collect<'tree>(node: Node<'tree>, kind: &str, out: &mut Vec<Node<'tree>>) {
        if node.kind() == kind {
            out.push(node);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect(child, kind, out);
        }
    }
}

/// Terraform module calls and `var`/`local`/`module` references ([FR-CG-08],
/// `tf-module-call` / `tf-var-ref`).
mod terraform {
    use super::*;

    /// Capture, for each top-level HCL `block`: its module-`source` call (when it is
    /// a `module` block) and every `var.x`/`local.x`/`module.x.*` reference in its
    /// subtree, each attributed to that block's `TfBlock` anchor. Terraform
    /// resource-attribute references (`aws_instance.web.id`, `data.x.y`,
    /// `count`/`each`/`path`/`terraform`) are out of scope and never captured
    /// ([FR-CG-08], CR-011 §3.3).
    pub(super) fn capture(root: Node<'_>, source: &[u8], facts: &mut Facts) {
        // (source TfBlock symbol, target, relation, form, line).
        let mut captured: Vec<(LogosSymbol, String, ArtifactRelation, RefForm, u32)> = Vec::new();
        let mut rc = root.walk();
        for body in root.named_children(&mut rc) {
            if body.kind() != "body" {
                continue;
            }
            let mut bc = body.walk();
            for block in body.named_children(&mut bc) {
                if block.kind() != "block" {
                    continue;
                }
                let start_line = block.start_position().row as u32 + 1;
                let end_line = block.end_position().row as u32 + 1;
                let Some(src_sym) =
                    anchor_symbol_at(facts, NodeKind::TfBlock, start_line, end_line)
                else {
                    continue;
                };
                // A `module "x" { source = "..." }` call → the source directory.
                if block_type(block, source).as_deref() == Some("module") {
                    if let Some((path, line)) = module_source(block, source) {
                        captured.push((
                            src_sym.clone(),
                            path,
                            ArtifactRelation::TfModuleCall,
                            RefForm::Path,
                            line,
                        ));
                    }
                }
                // Every var/local/module reference anywhere under this top-level
                // block resolves to its declaring block.
                collect_var_refs(block, source, &src_sym, &mut captured);
            }
        }
        for (src, target, relation, form, line) in dedup_tf(captured) {
            push_artifact_ref(facts, &src, &target, relation, form, line);
        }
    }

    /// The block's type — the text of its first `identifier` child (`module`,
    /// `resource`, `variable`, …), or `None` for a malformed block.
    fn block_type(block: Node<'_>, source: &[u8]) -> Option<String> {
        let mut cursor = block.walk();
        for child in block.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                return child
                    .utf8_text(source)
                    .ok()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty());
            }
        }
        None
    }

    /// A `module` block's `source` directory and its line, when the `source`
    /// attribute is a **pure string literal** (an interpolated `"${var.x}/m"` is not
    /// a literal directory and is skipped; the classifier drops non-local literals).
    fn module_source(block: Node<'_>, source: &[u8]) -> Option<(String, u32)> {
        let body = block_body(block)?;
        let mut cursor = body.walk();
        for attr in body.named_children(&mut cursor) {
            if attr.kind() != "attribute" {
                continue;
            }
            if attribute_name(attr, source).as_deref() != Some("source") {
                continue;
            }
            let value = string_literal_value(attr, source)?;
            return Some((value, attr.start_position().row as u32 + 1));
        }
        None
    }

    /// The `body` child of an HCL block. Index iteration keeps the returned node's
    /// lifetime tied to the tree, not a local `walk()` cursor borrow.
    fn block_body(block: Node<'_>) -> Option<Node<'_>> {
        for i in 0..block.named_child_count() {
            let child = block.named_child(i)?;
            if child.kind() == "body" {
                return Some(child);
            }
        }
        None
    }

    /// An `attribute`'s name — its first `identifier` child's text.
    fn attribute_name(attr: Node<'_>, source: &[u8]) -> Option<String> {
        let mut cursor = attr.walk();
        for child in attr.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                return child.utf8_text(source).ok().map(|t| t.trim().to_string());
            }
        }
        None
    }

    /// The literal string value of an `attribute` whose expression is a single
    /// quoted string with no interpolation (`source = "./modules/net"` →
    /// `./modules/net`). `None` for a non-literal expression (an interpolated
    /// template, a number, a reference) so only a literal directory is captured.
    fn string_literal_value(attr: Node<'_>, source: &[u8]) -> Option<String> {
        let string_lit = find_first_kind(attr, "string_lit")?;
        // A pure literal contains only `template_literal` text between its quotes;
        // any `template_interpolation`/`template_directive` makes it non-literal.
        let mut parts = String::new();
        let mut cursor = string_lit.walk();
        for child in string_lit.named_children(&mut cursor) {
            match child.kind() {
                "template_literal" => parts.push_str(child.utf8_text(source).ok()?),
                "quoted_template_start" | "quoted_template_end" => {}
                _ => return None, // an interpolation or directive → not a literal
            }
        }
        (!parts.is_empty()).then_some(parts)
    }

    /// The first descendant of `node` (or `node`) of kind `kind`, depth-first.
    fn find_first_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if let Some(found) = find_first_kind(child, kind) {
                return Some(found);
            }
        }
        None
    }

    /// Every `var.x`/`local.x`/`module.x.*` reference under `block`, attributed to
    /// `src_sym`. A reference is a `variable_expr` headed by `var`/`local`/`module`,
    /// followed by `get_attr` segments naming the referenced symbol:
    /// - `var.NAME`   → the `variable "NAME"` block (`variable NAME`);
    /// - `local.NAME` → the `locals` block (`locals`);
    /// - `module.NAME.OUT` → the `module "NAME"` call block (`module NAME`).
    fn collect_var_refs(
        block: Node<'_>,
        source: &[u8],
        src_sym: &LogosSymbol,
        out: &mut Vec<(LogosSymbol, String, ArtifactRelation, RefForm, u32)>,
    ) {
        for ve in var_exprs(block) {
            let head = ve.utf8_text(source).ok().map(str::trim);
            let line = ve.start_position().row as u32 + 1;
            // The `get_attr` segments are the named siblings of `variable_expr`
            // within its parent `expression`.
            let target = match head {
                Some("var") => first_get_attr(ve, source).map(|n| format!("variable {n}")),
                Some("local") => Some("locals".to_string()),
                Some("module") => first_get_attr(ve, source).map(|n| format!("module {n}")),
                _ => None, // resource/data/count/each/path/terraform → out of scope
            };
            if let Some(target) = target {
                out.push((
                    src_sym.clone(),
                    target,
                    ArtifactRelation::TfVarRef,
                    RefForm::Method,
                    line,
                ));
            }
        }
    }

    /// Every `variable_expr` node under `node`, document order.
    fn var_exprs(node: Node<'_>) -> Vec<Node<'_>> {
        let mut out = Vec::new();
        collect_var_exprs(node, &mut out);
        out
    }

    fn collect_var_exprs<'tree>(node: Node<'tree>, out: &mut Vec<Node<'tree>>) {
        if node.kind() == "variable_expr" {
            out.push(node);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_var_exprs(child, out);
        }
    }

    /// The identifier of the first `get_attr` sibling following `variable_expr`
    /// within its parent `expression` (`var.region` → `region`).
    fn first_get_attr(ve: Node<'_>, source: &[u8]) -> Option<String> {
        let parent = ve.parent()?;
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            if child.kind() == "get_attr" && child.start_byte() > ve.start_byte() {
                // `get_attr` → identifier child.
                let mut gc = child.walk();
                for g in child.named_children(&mut gc) {
                    if g.kind() == "identifier" {
                        return g
                            .utf8_text(source)
                            .ok()
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty());
                    }
                }
            }
        }
        None
    }
}

/// Shell `source`/`.` of a literal path → the target script ([FR-CG-08],
/// `shell-source`).
mod shell {
    use super::*;

    /// Capture every `source PATH` / `. PATH` command whose argument is a
    /// **literal** workspace-relative path, attributed to the script's own
    /// `ConfigFile`. An interpolated argument (`"$DIR/x.sh"`) is dropped — the
    /// `string` node carries an expansion, so no literal is extracted and the
    /// classifier rejects any `$` (FR-CG-08, "interpolated paths never
    /// candidates").
    pub(super) fn capture(
        root: Node<'_>,
        config_file_symbol: &LogosSymbol,
        source: &[u8],
        facts: &mut Facts,
    ) {
        let mut captured: Vec<(String, u32)> = Vec::new();
        collect_source_commands(root, source, &mut captured);
        for (path, line) in captured {
            push_artifact_ref(
                facts,
                config_file_symbol,
                &path,
                ArtifactRelation::ShellSource,
                RefForm::Path,
                line,
            );
        }
    }

    fn collect_source_commands(node: Node<'_>, source: &[u8], out: &mut Vec<(String, u32)>) {
        if node.kind() == "command" {
            if let Some((path, line)) = source_command_path(node, source) {
                out.push((path, line));
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_source_commands(child, source, out);
        }
    }

    /// The literal path argument of a `command` node iff its name is `source` or
    /// `.` and its first argument is a literal: a bare `word`, a single-quoted
    /// `raw_string` (literal by construction — no expansion in `'…'`), or a
    /// double-quoted `string` with no interpolation. `None` otherwise — an
    /// interpolated or non-source command captures nothing.
    fn source_command_path(command: Node<'_>, source: &[u8]) -> Option<(String, u32)> {
        let name = command
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::trim)?;
        if name != "source" && name != "." {
            return None;
        }
        let arg = command.child_by_field_name("argument")?;
        let line = arg.start_position().row as u32 + 1;
        let path = match arg.kind() {
            // A bare word: a literal path (`source ./lib/common.sh`).
            "word" => arg.utf8_text(source).ok().map(|t| t.trim().to_string()),
            // A single-quoted string is literal by construction (Bash performs no
            // expansion inside `'…'`); strip the surrounding quotes.
            "raw_string" => arg
                .utf8_text(source)
                .ok()
                .map(|t| t.trim().trim_matches('\'').to_string()),
            // A double-quoted string: literal only if it carries no expansion.
            "string" => literal_string(arg, source),
            _ => None,
        };
        path.filter(|p| !p.is_empty()).map(|p| (p, line))
    }

    /// The literal content of a quoted `string` argument, or `None` if it contains
    /// any expansion (`simple_expansion`/`expansion`) — an interpolated path is
    /// never a candidate.
    fn literal_string(string: Node<'_>, source: &[u8]) -> Option<String> {
        let mut parts = String::new();
        let mut cursor = string.walk();
        for child in string.named_children(&mut cursor) {
            match child.kind() {
                "string_content" => parts.push_str(child.utf8_text(source).ok()?),
                _ => return None, // simple_expansion / expansion / … → interpolated
            }
        }
        (!parts.is_empty()).then_some(parts)
    }
}

/// Drop duplicate `(source, target)` SQL references within one file (a view
/// reading the same table twice), keeping the first — deterministic, and the
/// ledger row would be redundant.
fn dedup(mut refs: Vec<(LogosSymbol, String, u32)>) -> Vec<(LogosSymbol, String, u32)> {
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    refs.retain(|(src, target, _)| seen.insert((src.as_str().to_string(), target.clone())));
    refs
}

/// Drop duplicate `(source, target, relation)` Terraform references within one
/// file (a block referencing the same `var` twice), keeping the first.
#[allow(clippy::type_complexity)]
fn dedup_tf(
    mut refs: Vec<(LogosSymbol, String, ArtifactRelation, RefForm, u32)>,
) -> Vec<(LogosSymbol, String, ArtifactRelation, RefForm, u32)> {
    let mut seen: std::collections::HashSet<(String, String, ArtifactRelation)> =
        std::collections::HashSet::new();
    refs.retain(|(src, target, rel, _, _)| {
        seen.insert((src.as_str().to_string(), target.clone(), *rel))
    });
    refs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EdgeKind;

    fn empty_facts() -> Facts {
        Facts {
            path: "svc.proto".to_string(),
            language: "proto".to_string(),
            partial: false,
            nodes: Vec::new(),
            edges: Vec::new(),
            refs: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn sym() -> LogosSymbol {
        LogosSymbol::parse("local 0").unwrap()
    }

    #[test]
    fn a_workspace_relative_reference_is_captured_with_the_relations_edge_kind() {
        let mut facts = empty_facts();
        let captured = push_artifact_ref(
            &mut facts,
            &sym(),
            "common/types.proto",
            ArtifactRelation::ProtoImport,
            RefForm::Path,
            7,
        );
        assert!(captured, "a workspace-relative import is a candidate");
        assert_eq!(facts.refs.len(), 1);
        let r = &facts.refs[0];
        assert_eq!(r.target, "common/types.proto");
        assert_eq!(r.form, RefForm::Path);
        // The edge kind is derived from the relation — artifact→artifact here.
        assert_eq!(r.kind, ArtifactRelation::ProtoImport.edge_kind());
        assert_eq!(r.relation, Some(ArtifactRelation::ProtoImport));
    }

    #[test]
    fn an_external_reference_produces_no_fact_and_no_ledger_entry() {
        // The external forms the push gate drops: a vendored proto import, a
        // Terraform registry source, an absolute-URL target, an interpolated
        // shell path, and a GraphQL built-in scalar (S-070).
        let cases = [
            (
                ArtifactRelation::ProtoImport,
                "google/protobuf/timestamp.proto",
            ),
            (ArtifactRelation::TfModuleCall, "hashicorp/aws"),
            (ArtifactRelation::Route, "https://example.com/spec.yaml"),
            (ArtifactRelation::ShellSource, "$DIR/common.sh"),
            (ArtifactRelation::GraphqlType, "String"),
        ];
        for (relation, target) in cases {
            let mut facts = empty_facts();
            let captured =
                push_artifact_ref(&mut facts, &sym(), target, relation, RefForm::Path, 1);
            assert!(
                !captured,
                "{} target {target} must be classified external",
                relation.as_str()
            );
            assert!(
                facts.refs.is_empty(),
                "an external {} target must produce no ledger entry",
                relation.as_str()
            );
        }
    }

    #[test]
    fn an_artifact_binding_relation_captures_an_artifact_binding_edge() {
        let mut facts = empty_facts();
        push_artifact_ref(
            &mut facts,
            &sym(),
            "UserProfile",
            ArtifactRelation::SchemaType,
            RefForm::Method,
            3,
        );
        assert_eq!(
            facts.refs[0].kind,
            EdgeKind::ArtifactBinding,
            "a schema type-name reference binds artifact→code"
        );
    }

    // ── FR-WS-07 / ADR-54: the generic consumer-side capture interpreter ──────

    fn site(source: &str, line: u32, slots: &[(&str, &str)]) -> InvocationSite {
        InvocationSite {
            source: LogosSymbol::parse(source).unwrap(),
            slots: slots
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            line,
        }
    }

    /// The interpreter renders each captured site via the arm's (synthetic)
    /// normalizer and funnels it through the shared emission point: a rendered
    /// static site is captured; a site the normalizer refuses (a runtime-composed
    /// target) contributes nothing; an external rendered target is dropped by the
    /// push gate; and an empty site stream (a language with no capture file)
    /// contributes nothing. The stand-in relation is `Route` — no relation is an
    /// invocation arm yet, and the interpreter is relation-agnostic plumbing.
    #[test]
    fn the_interpreter_renders_refuses_and_drops_externals() {
        // A synthetic arm normalizer: join `method` + `path`, refusing a
        // runtime-composed path (one carrying a `$` marker).
        let render = |slots: &BTreeMap<String, String>| -> Option<String> {
            let method = slots.get("method")?;
            let path = slots.get("path")?;
            if path.contains('$') {
                return None; // runtime-composed → refuse (never fabricate)
            }
            Some(format!("{method} {path}"))
        };

        // A language lacking the capture file yields no sites → nothing captured.
        let mut facts = empty_facts();
        let n = capture_invocation_refs(
            &mut facts,
            ArtifactRelation::Route,
            RefForm::Path,
            Vec::new(),
            &render,
        );
        assert_eq!(n, 0, "no capture file → no sites → no refs");
        assert!(facts.refs.is_empty());

        // A static site is captured; a refused and an external one are dropped.
        let mut facts = empty_facts();
        let sites = vec![
            site("local 1", 5, &[("method", "GET"), ("path", "/users/{id}")]),
            site("local 2", 6, &[("method", "POST"), ("path", "$base/orders")]),
            site("local 3", 7, &[("method", "GET"), ("path", "https://ext/x")]),
        ];
        let n = capture_invocation_refs(
            &mut facts,
            ArtifactRelation::Route,
            RefForm::Path,
            sites,
            &render,
        );
        assert_eq!(n, 1, "only the static, workspace-relative site is captured");
        assert_eq!(facts.refs.len(), 1);
        let r = &facts.refs[0];
        assert_eq!(r.source.as_str(), "local 1");
        assert_eq!(r.target, "GET /users/{id}");
        assert_eq!(r.line, 5);
        assert_eq!(r.relation, Some(ArtifactRelation::Route));
    }

    // ── S-069: OpenAPI operation→route capture ───────────────────────────────

    use crate::extract::{EdgeFact, NodeFact};

    /// A promoted config node, as the OpenAPI profile promotion emits it.
    fn api_node(symbol: &str, name: &str, kind: NodeKind, line: u32) -> NodeFact {
        NodeFact {
            symbol: LogosSymbol::parse(symbol).unwrap(),
            kind,
            name: name.to_string(),
            start_line: line,
            end_line: line,
            metrics: None,
            exported: false,
            fingerprint: None,
            test_evidence: false,
            body: None,
            max_nesting_depth: None,
            shingles: Vec::new(),
        }
    }

    fn contains(parent: &str, child: &str) -> EdgeFact {
        EdgeFact {
            source: LogosSymbol::parse(parent).unwrap(),
            target: LogosSymbol::parse(child).unwrap(),
            kind: EdgeKind::Contains,
        }
    }

    /// A facts set shaped like a promoted OpenAPI spec: a `ConfigFile`, one
    /// `ApiPath` per template, one `ApiOperation` per method, wired by `Contains`.
    fn openapi_facts() -> Facts {
        let mut facts = empty_facts();
        facts.path = "openapi.yaml".to_string();
        facts.language = "yaml".to_string();
        facts.nodes = vec![
            api_node("local 0", "openapi.yaml", NodeKind::ConfigFile, 1),
            api_node("local 1", "/pets", NodeKind::ApiPath, 5),
            api_node("local 2", "get", NodeKind::ApiOperation, 6),
            api_node("local 3", "post", NodeKind::ApiOperation, 8),
            api_node("local 4", "/pets/{id}", NodeKind::ApiPath, 10),
            api_node("local 5", "delete", NodeKind::ApiOperation, 11),
        ];
        facts.edges = vec![
            contains("local 0", "local 1"),
            contains("local 1", "local 2"),
            contains("local 1", "local 3"),
            contains("local 0", "local 4"),
            contains("local 4", "local 5"),
        ];
        facts
    }

    #[test]
    fn each_api_operation_captures_a_method_and_template_route_reference() {
        let mut facts = openapi_facts();
        capture_openapi_routes(&mut facts);

        let mut captured: Vec<(&str, &str)> = facts
            .refs
            .iter()
            .map(|r| (r.source.as_str(), r.target.as_str()))
            .collect();
        captured.sort();
        assert_eq!(
            captured,
            vec![
                ("local 2", "GET /pets"),
                ("local 3", "POST /pets"),
                ("local 5", "DELETE /pets/{id}"),
            ],
            "one `METHOD /template` route ref per operation, method upper-cased"
        );
        // Every captured ref is an ArtifactBinding + Path under the `route` class.
        for r in &facts.refs {
            assert_eq!(r.kind, EdgeKind::ArtifactBinding);
            assert_eq!(r.form, RefForm::Path);
            assert_eq!(r.relation, Some(ArtifactRelation::Route));
        }
    }

    #[test]
    fn a_document_with_no_api_operations_captures_nothing() {
        // A plain config file (no OpenAPI promotion): the capture is a no-op, so
        // the seam costs nothing on the overwhelming majority of artifact files.
        let mut facts = empty_facts();
        facts.nodes = vec![
            api_node("local 0", "values.yaml", NodeKind::ConfigFile, 1),
            api_node("local 1", "service", NodeKind::ConfigSection, 2),
        ];
        capture_openapi_routes(&mut facts);
        assert!(
            facts.refs.is_empty(),
            "a non-OpenAPI document produces no route references"
        );
    }

    #[test]
    fn an_orphan_operation_without_a_path_parent_is_skipped() {
        // Defensive: an `ApiOperation` with no `Contains` parent (a shape the
        // promotion never emits) yields no reference rather than a fabricated one.
        let mut facts = empty_facts();
        facts.nodes = vec![
            api_node("local 0", "openapi.yaml", NodeKind::ConfigFile, 1),
            api_node("local 1", "/pets", NodeKind::ApiPath, 5),
            api_node("local 9", "get", NodeKind::ApiOperation, 6), // no Contains edge
        ];
        capture_openapi_routes(&mut facts);
        assert!(
            facts.refs.is_empty(),
            "an operation with no parent ApiPath is skipped, never guessed"
        );
    }
}
#[cfg(test)]
mod infra_tests {
    use super::*;

    fn empty_facts() -> Facts {
        Facts {
            path: "svc.proto".to_string(),
            language: "proto".to_string(),
            partial: false,
            nodes: Vec::new(),
            edges: Vec::new(),
            refs: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn sym() -> LogosSymbol {
        LogosSymbol::parse("local 0").unwrap()
    }

    #[test]
    fn a_workspace_relative_reference_is_captured_with_the_relations_edge_kind() {
        let mut facts = empty_facts();
        let captured = push_artifact_ref(
            &mut facts,
            &sym(),
            "common/types.proto",
            ArtifactRelation::ProtoImport,
            RefForm::Path,
            7,
        );
        assert!(captured, "a workspace-relative import is a candidate");
        assert_eq!(facts.refs.len(), 1);
        let r = &facts.refs[0];
        assert_eq!(r.target, "common/types.proto");
        assert_eq!(r.form, RefForm::Path);
        // The edge kind is derived from the relation — artifact→artifact here.
        assert_eq!(r.kind, ArtifactRelation::ProtoImport.edge_kind());
        assert_eq!(r.relation, Some(ArtifactRelation::ProtoImport));
    }

    #[test]
    fn an_external_reference_produces_no_fact_and_no_ledger_entry() {
        // The three external forms UAT-CG-04 step 3 exercises: a vendored proto
        // import, a Terraform registry source, and an absolute-URL target.
        let cases = [
            (
                ArtifactRelation::ProtoImport,
                "google/protobuf/timestamp.proto",
            ),
            (ArtifactRelation::TfModuleCall, "hashicorp/aws"),
            (ArtifactRelation::Route, "https://example.com/spec.yaml"),
            (ArtifactRelation::ShellSource, "$DIR/common.sh"),
        ];
        for (relation, target) in cases {
            let mut facts = empty_facts();
            let captured =
                push_artifact_ref(&mut facts, &sym(), target, relation, RefForm::Path, 1);
            assert!(
                !captured,
                "{} target {target} must be classified external",
                relation.as_str()
            );
            assert!(
                facts.refs.is_empty(),
                "an external {} target must produce no ledger entry",
                relation.as_str()
            );
        }
    }

    #[test]
    fn an_artifact_binding_relation_captures_an_artifact_binding_edge() {
        let mut facts = empty_facts();
        push_artifact_ref(
            &mut facts,
            &sym(),
            "UserProfile",
            ArtifactRelation::SchemaType,
            RefForm::Method,
            3,
        );
        assert_eq!(
            facts.refs[0].kind,
            crate::model::EdgeKind::ArtifactBinding,
            "a schema type-name reference binds artifact→code"
        );
    }
}

// The per-format capture walks need a linked grammar to exercise end-to-end; like
// the anchor-walk tests they run only when the matching grammar is compiled in.
// The walk *logic* compiles under the default set — these prove capture through
// the public `extract` entry point over the real grammars (S-071).
#[cfg(test)]
mod capture_support {
    use super::*;

    /// The symbol of the unique node of `kind` named `name` — the expected source
    /// endpoint of a captured reference.
    pub(super) fn node_symbol(facts: &Facts, kind: NodeKind, name: &str) -> LogosSymbol {
        facts
            .nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("no {kind:?} node named {name:?}"))
            .symbol
            .clone()
    }

    /// Every captured reference fact of the given relation class.
    pub(super) fn refs_for(facts: &Facts, relation: ArtifactRelation) -> Vec<&RefFact> {
        facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(relation))
            .collect()
    }
}

#[cfg(all(test, feature = "lang-sql"))]
mod sql_capture_tests {
    use super::capture_support::*;
    use super::*;
    use crate::extract::{extract, FileInput};
    use crate::model::EdgeKind;
    use crate::plugin::{CompiledPlugin, PluginManifest};
    use std::collections::BTreeMap;

    fn sql_plugin() -> CompiledPlugin {
        let toml = include_str!("../../../plugins/sql/plugin.toml");
        let manifest = PluginManifest::parse("sql/plugin.toml", toml).unwrap();
        let language: tree_sitter::Language = tree_sitter_sequel::LANGUAGE.into();
        CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
    }

    /// A view's `FROM` table and a table's foreign-key target both bind (by name)
    /// to the created table's `SqlObject`, sourced from the view / FK-owning table
    /// ([FR-CG-08]).
    #[test]
    fn view_and_fk_clauses_reference_their_tables() {
        let src = "CREATE TABLE app.users (id INT PRIMARY KEY);\n\
                   CREATE VIEW active AS SELECT * FROM app.users;\n\
                   CREATE TABLE orders (id INT, uid INT, FOREIGN KEY (uid) REFERENCES app.users(id));\n";
        let facts = extract(
            &FileInput::new("schema.sql", src),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        let refs = refs_for(&facts, ArtifactRelation::SqlObjectRef);
        assert_eq!(
            refs.len(),
            2,
            "the view FROM and the FK each reference a table: {refs:?}"
        );
        for r in &refs {
            assert_eq!(
                r.target, "table app.users",
                "the target leads with the table object-type payload"
            );
            assert_eq!(r.kind, EdgeKind::ArtifactRef);
            assert_eq!(r.form, RefForm::Method);
        }
        // The view reference is sourced from the view's own SqlObject anchor; the
        // FK reference from the FK-owning table's anchor.
        let view = node_symbol(&facts, NodeKind::SqlObject, "view active");
        let orders = node_symbol(&facts, NodeKind::SqlObject, "table orders");
        let sources: Vec<&str> = refs.iter().map(|r| r.source.as_str()).collect();
        assert!(
            sources.contains(&view.as_str()),
            "the view sources a table ref"
        );
        assert!(
            sources.contains(&orders.as_str()),
            "the FK-owning table sources a table ref"
        );
    }

    /// An unparseable dialect construct (a T-SQL stored procedure) yields **no**
    /// reference candidates — its statement carries a syntax error, so it is never
    /// anchored and its inner `FROM` is never captured ([NFR-RA-05]).
    #[test]
    fn unparseable_sql_yields_no_reference_candidates() {
        let src = "CREATE PROCEDURE dbo.GetUsers AS BEGIN SELECT * FROM app.users END;\n";
        let facts = extract(
            &FileInput::new("proc.sql", src),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        assert!(
            refs_for(&facts, ArtifactRelation::SqlObjectRef).is_empty(),
            "unparseable SQL produces no reference candidates: {:?}",
            facts.refs
        );
    }
}

#[cfg(all(test, feature = "lang-terraform"))]
mod terraform_capture_tests {
    use super::capture_support::*;
    use super::*;
    use crate::extract::{extract, FileInput};
    use crate::model::EdgeKind;
    use crate::plugin::{CompiledPlugin, PluginManifest};
    use std::collections::BTreeMap;

    fn terraform_plugin() -> CompiledPlugin {
        let toml = include_str!("../../../plugins/terraform/plugin.toml");
        let manifest = PluginManifest::parse("terraform/plugin.toml", toml).unwrap();
        let language: tree_sitter::Language = tree_sitter_hcl::LANGUAGE.into();
        CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
    }

    fn fixture() -> Facts {
        let src = r#"
variable "region" {
  default = "us-east-1"
}
locals {
  name = "web"
}
module "vpc" {
  source = "./modules/vpc"
}
module "registry" {
  source = "hashicorp/aws"
}
resource "aws_instance" "web" {
  ami    = var.region
  name   = local.name
  subnet = module.vpc.subnet_id
  self   = aws_instance.web.id
}
"#;
        extract(
            &FileInput::new("main.tf", src),
            &terraform_plugin(),
            &SymbolContext::default(),
        )
    }

    /// A local module call captures a path reference to its source dir, sourced
    /// from the `module` block; a registry source is classified external and never
    /// captured ([FR-CG-08], [UAT-CG-04]).
    #[test]
    fn local_module_call_captured_registry_source_is_not() {
        let facts = fixture();
        let calls = refs_for(&facts, ArtifactRelation::TfModuleCall);
        assert_eq!(
            calls.len(),
            1,
            "only the local module call is a candidate: {calls:?}"
        );
        let call = calls[0];
        assert_eq!(call.target, "./modules/vpc");
        assert_eq!(call.form, RefForm::Path);
        assert_eq!(call.kind, EdgeKind::ArtifactRef);
        assert_eq!(
            call.source.as_str(),
            node_symbol(&facts, NodeKind::TfBlock, "module vpc").as_str(),
            "the call is sourced from the module block"
        );
        assert!(
            !calls.iter().any(|r| r.target == "hashicorp/aws"),
            "a registry source must not be captured"
        );
    }

    /// `var`/`local`/`module` references bind (by name) to their declaring blocks,
    /// sourced from the referencing block; resource-attribute references stay out
    /// of scope ([FR-CG-08], CR-011 §3.3).
    #[test]
    fn var_local_module_refs_target_their_blocks_resource_attrs_excluded() {
        let facts = fixture();
        let refs = refs_for(&facts, ArtifactRelation::TfVarRef);
        let targets: Vec<&str> = refs.iter().map(|r| r.target.as_str()).collect();
        assert!(
            targets.contains(&"variable region"),
            "var.region → variable block: {targets:?}"
        );
        assert!(
            targets.contains(&"locals"),
            "local.name → locals block: {targets:?}"
        );
        assert!(
            targets.contains(&"module vpc"),
            "module.vpc.x → module block: {targets:?}"
        );
        // The resource-attribute reference `aws_instance.web.id` is out of scope.
        assert!(
            !targets.iter().any(|t| t.starts_with("aws_instance")),
            "a resource-attribute reference must not be captured: {targets:?}"
        );
        let resource = node_symbol(&facts, NodeKind::TfBlock, "resource aws_instance web");
        for r in &refs {
            assert_eq!(
                r.source.as_str(),
                resource.as_str(),
                "sourced from the referencing block"
            );
            assert_eq!(r.form, RefForm::Method);
        }
    }

    /// An interpolated module source (`"${var.x}/m"`) is not a literal directory
    /// and is never captured ([NFR-RA-05]).
    #[test]
    fn interpolated_module_source_is_not_captured() {
        let src = "module \"x\" {\n  source = \"${var.base}/mod\"\n}\n";
        let facts = extract(
            &FileInput::new("main.tf", src),
            &terraform_plugin(),
            &SymbolContext::default(),
        );
        assert!(
            refs_for(&facts, ArtifactRelation::TfModuleCall).is_empty(),
            "an interpolated module source is not a literal directory: {:?}",
            facts.refs
        );
    }
}

#[cfg(all(test, feature = "lang-shell"))]
mod shell_capture_tests {
    use super::capture_support::*;
    use super::*;
    use crate::extract::{extract, FileInput};
    use crate::model::EdgeKind;
    use crate::plugin::{CompiledPlugin, PluginManifest};
    use std::collections::BTreeMap;

    fn shell_plugin() -> CompiledPlugin {
        let toml = include_str!("../../../plugins/shell/plugin.toml");
        let manifest = PluginManifest::parse("shell/plugin.toml", toml).unwrap();
        let language: tree_sitter::Language = tree_sitter_bash::LANGUAGE.into();
        CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
    }

    /// `source`/`.` of a literal path (bare or quoted) is captured as a path
    /// reference sourced from the script's `ConfigFile`; an interpolated argument
    /// produces neither edge nor ledger entry ([FR-CG-08]).
    #[test]
    fn literal_source_captured_interpolated_is_not() {
        let src = "source ./lib/common.sh\n\
                   . ./lib/util.sh\n\
                   source \"./lib/quoted.sh\"\n\
                   source './lib/single.sh'\n\
                   source \"$DIR/x.sh\"\n\
                   source \"${BASE}/y.sh\"\n";
        let facts = extract(
            &FileInput::new("deploy.sh", src),
            &shell_plugin(),
            &SymbolContext::default(),
        );
        let refs = refs_for(&facts, ArtifactRelation::ShellSource);
        let mut targets: Vec<&str> = refs.iter().map(|r| r.target.as_str()).collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            [
                "./lib/common.sh",
                "./lib/quoted.sh",
                "./lib/single.sh",
                "./lib/util.sh"
            ],
            "every literal source path (bare, single- and double-quoted) is captured; \
             interpolated ones are not: {:?}",
            facts.refs
        );
        let config_file = node_symbol(&facts, NodeKind::ConfigFile, "deploy.sh");
        for r in &refs {
            assert_eq!(
                r.source.as_str(),
                config_file.as_str(),
                "sourced from the script ConfigFile"
            );
            assert_eq!(r.form, RefForm::Path);
            assert_eq!(r.kind, EdgeKind::ArtifactRef);
        }
    }
}
