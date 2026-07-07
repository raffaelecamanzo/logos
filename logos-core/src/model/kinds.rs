//! The node- and edge-kind ontology — the canonical Logos taxonomy
//! ([FR-EX-05](../../../../docs/specs/requirements/FR-EX-05.md), DV-01/DV-02).
//!
//! # Discriminant stability contract
//!
//! Both enums are `#[repr(i32)]` with **explicit** discriminants so the integer
//! value of each variant is a frozen part of the on-disk contract: the SQLite
//! `graph-store` (S-005) writes these integers into `nodes.kind` / `edges.kind`
//! columns guarded by `CHECK (kind IN (1, 2, …))`. Reordering a variant or
//! reusing a retired discriminant would silently corrupt every existing graph.
//!
//! Rules for evolving this file:
//! - **Never** reorder variants or change an existing discriminant.
//! - Discriminants start at `1`; `0` is intentionally never a valid kind, so a
//!   zeroed/defaulted column can never masquerade as a real kind.
//! - New kinds append with the next free integer; the CHECK constraint in
//!   `graph-store` widens to match.
//!
//! # Two representations, two purposes
//!
//! - [`NodeKind::as_i32`] / [`EdgeKind::as_i32`] (+ the `TryFrom<i32>` impls)
//!   are the **database** representation used by the CHECK constraints.
//! - `serde` serialises these enums as their lower-case [`NodeKind::as_str`]
//!   name — the **wire** representation used by JSON read-models and by the
//!   `search(query, kind?)` filter ([FR-NV-01](../../../../docs/specs/requirements/FR-NV-01.md)).

use serde::{Deserialize, Serialize};

/// An ontology kind value (`i32`) that does not correspond to any known
/// [`NodeKind`] or [`EdgeKind`] variant.
///
/// Surfaced when reading a discriminant back from the database that falls
/// outside the frozen contract — an integrity error, never expected in a
/// store this binary wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownKind(pub i32);

impl std::fmt::Display for UnknownKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown ontology kind discriminant: {}", self.0)
    }
}

impl std::error::Error for UnknownKind {}

/// The 34 node kinds of the Logos data model.
///
/// Variants span the union of the five supported languages (Rust, Python,
/// TypeScript/JS, Go, Java) plus the two framework-promoted kinds, [`Route`]
/// and [`Component`] ([FR-FW-01](../../../../docs/specs/requirements/FR-FW-01.md),
/// [FR-FW-02](../../../../docs/specs/requirements/FR-FW-02.md)), the two
/// derived governance/policy kinds, [`Layer`] and [`Boundary`], materialised by
/// the annotation engine from `rules.toml`
/// ([FR-AN-03](../../../../docs/specs/requirements/FR-AN-03.md)), the five
/// documentation kinds added by [CR-003](../../../../docs/requests/CR-003-documentation-graph-layer.md):
/// [`DocFile`]/[`DocSection`] for the generic markdown layer ([FR-DG-02](../../../../docs/specs/requirements/FR-DG-02.md))
/// and the typed [`Requirement`]/[`Adr`]/[`Story`] enrichment for swe-skills
/// repositories ([FR-DG-07](../../../../docs/specs/requirements/FR-DG-07.md)),
/// and the twelve config & artifact kinds added by
/// [CR-010](../../../../docs/requests/CR-010-config-artifact-graph-layer.md):
/// the generic [`ConfigFile`]/[`ConfigSection`] layer ([FR-CG-02](../../../../docs/specs/requirements/FR-CG-02.md))
/// and the per-format typed anchors ([FR-CG-03](../../../../docs/specs/requirements/FR-CG-03.md))
/// [`ShellFunction`], [`DockerfileStage`], [`MakeTarget`], [`ProtoMessage`],
/// [`ProtoService`], [`GqlType`], [`SqlObject`], [`TfBlock`], [`ApiPath`], and
/// [`ApiOperation`]. Discriminants are frozen — see the module docs.
///
/// Documentation **and** config kinds are excluded from the code subgraph at
/// hydration ([ADR-19](../../../../docs/specs/architecture/decisions/ADR-19.md),
/// [ADR-25](../../../../docs/specs/architecture/decisions/ADR-25.md),
/// [FR-DG-06](../../../../docs/specs/requirements/FR-DG-06.md),
/// [FR-CG-05](../../../../docs/specs/requirements/FR-CG-05.md)) — the non-code-layer
/// scope ([`is_non_code`](NodeKind::is_non_code)) — so admitting either never
/// moves the quality signal.
///
/// [`Route`]: NodeKind::Route
/// [`Component`]: NodeKind::Component
/// [`Layer`]: NodeKind::Layer
/// [`Boundary`]: NodeKind::Boundary
/// [`DocFile`]: NodeKind::DocFile
/// [`DocSection`]: NodeKind::DocSection
/// [`Requirement`]: NodeKind::Requirement
/// [`Adr`]: NodeKind::Adr
/// [`Story`]: NodeKind::Story
/// [`ConfigFile`]: NodeKind::ConfigFile
/// [`ConfigSection`]: NodeKind::ConfigSection
/// [`ShellFunction`]: NodeKind::ShellFunction
/// [`DockerfileStage`]: NodeKind::DockerfileStage
/// [`MakeTarget`]: NodeKind::MakeTarget
/// [`ProtoMessage`]: NodeKind::ProtoMessage
/// [`ProtoService`]: NodeKind::ProtoService
/// [`GqlType`]: NodeKind::GqlType
/// [`SqlObject`]: NodeKind::SqlObject
/// [`TfBlock`]: NodeKind::TfBlock
/// [`ApiPath`]: NodeKind::ApiPath
/// [`ApiOperation`]: NodeKind::ApiOperation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum NodeKind {
    /// A namespace-like container: a module, package, or namespace.
    Module = 1,
    /// A class (Python/TS/Java) or any nominal record with methods.
    Class = 2,
    /// An interface (TS/Java) — a method contract without state.
    Interface = 3,
    /// A Rust trait — a method/associated-item contract.
    Trait = 4,
    /// A struct / record (Rust/Go) — a named aggregate of fields.
    Struct = 5,
    /// An enum / sum type.
    Enum = 6,
    /// A free function (not bound to a receiver).
    Function = 7,
    /// A method — a function bound to a receiver type.
    Method = 8,
    /// A field / property / struct member.
    Field = 9,
    /// A named compile-time constant.
    Constant = 10,
    /// A module- or function-scoped variable.
    Variable = 11,
    /// A type alias / `typedef`.
    TypeAlias = 12,
    /// A macro (Rust `macro_rules!`/proc-macro, C-style preprocessor macro).
    Macro = 13,
    /// A framework route node linking a URL/path to its handler symbol.
    Route = 14,
    /// A framework UI component declaration (e.g. a Next.js component).
    Component = 15,
    /// A derived architecture-layer policy node materialised from a
    /// `rules.toml` `[[layers]]` declaration (S-014, annotation engine).
    Layer = 16,
    /// A derived boundary policy node materialised from a `rules.toml`
    /// `[[boundaries]]` declaration (S-014, annotation engine).
    Boundary = 17,
    /// A markdown documentation file — one per indexed `.md`/`.markdown` file
    /// (CR-003, FR-DG-02). The root of a doc's [`Contains`](EdgeKind::Contains)
    /// hierarchy.
    DocFile = 18,
    /// A markdown heading and the content beneath it, nested under its parent
    /// [`DocFile`]/[`DocSection`] by [`Contains`](EdgeKind::Contains) (FR-DG-02).
    /// Its identity is the `path#heading-slug` scheme — sibling-ordinal
    /// disambiguated, reusing the ADR-07 construction — not SCIP.
    ///
    /// [`DocFile`]: NodeKind::DocFile
    /// [`DocSection`]: NodeKind::DocSection
    DocSection = 19,
    /// A typed requirement node (`FR-*`/`NFR-*`) promoted from a swe-skills
    /// convention file (FR-DG-07, S-039). Additive enrichment — only emitted
    /// when the conventions are detected; a plain markdown repo never produces it.
    Requirement = 20,
    /// A typed architecture-decision-record node (`ADR-NN`) promoted from a
    /// swe-skills convention file (FR-DG-07, S-039).
    Adr = 21,
    /// A typed story node (`S-NNN`) promoted from a swe-skills convention file
    /// (FR-DG-07, S-039).
    Story = 22,
    /// A config/artifact file — one per admitted artifact-class file (CR-010,
    /// FR-CG-02). The root of a config artifact's [`Contains`](EdgeKind::Contains)
    /// hierarchy, identity in the `path#anchor` scheme of the documentation layer
    /// ([ADR-07]). Excluded from the code subgraph at hydration so it never moves
    /// the signal ([FR-CG-05], [ADR-25]).
    ///
    /// [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
    /// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
    /// [FR-CG-02]: ../../../../docs/specs/requirements/FR-CG-02.md
    /// [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
    ConfigFile = 23,
    /// A nested mapping section of a data-format config file (YAML/JSON/TOML),
    /// nested under its parent [`ConfigFile`]/[`ConfigSection`] by
    /// [`Contains`](EdgeKind::Contains) to a fixed depth bound of 2 (CR-010,
    /// FR-CG-02, BR-30). Sibling-ordinal-disambiguated `path#anchor` identity.
    ///
    /// [`ConfigFile`]: NodeKind::ConfigFile
    /// [`ConfigSection`]: NodeKind::ConfigSection
    ConfigSection = 24,
    /// A shell function definition (`.sh`/`.bash` → typed anchor, CR-010,
    /// FR-CG-03). Deliberately metric-neutral: a structural node only, never a
    /// complexity or signal contribution ([ADR-25]).
    ShellFunction = 25,
    /// A Dockerfile build stage (one per `FROM`, CR-010, FR-CG-03).
    DockerfileStage = 26,
    /// A Makefile target (CR-010, FR-CG-03).
    MakeTarget = 27,
    /// A Protobuf message definition (CR-010, FR-CG-03).
    ProtoMessage = 28,
    /// A Protobuf service definition (CR-010, FR-CG-03).
    ProtoService = 29,
    /// A GraphQL type definition; the `object`/`interface`/`enum`/`input`/
    /// `union`/`scalar` subtype is carried in the node name/payload to cap
    /// frozen-discriminant growth (CR-010, FR-CG-03).
    GqlType = 30,
    /// A SQL DDL object (table/view/index/function/procedure), extracted
    /// conservatively (top-level `CREATE` names only; never fabricated, CR-010,
    /// FR-CG-03, [NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    SqlObject = 31,
    /// A Terraform/HCL block (resource/data/module/variable/output/provider
    /// subtype in the payload, CR-010, FR-CG-03).
    TfBlock = 32,
    /// An OpenAPI path template, promoted from a content-sniffed YAML/JSON
    /// document (CR-010, FR-CG-03). A binding target for the cross-artifact
    /// resolution follow-up ([CR-011]).
    ///
    /// [CR-011]: ../../../../docs/requests/CR-011-cross-artifact-resolution.md
    ApiPath = 33,
    /// An OpenAPI operation (one per HTTP method under an [`ApiPath`]), promoted
    /// from a content-sniffed OpenAPI document (CR-010, FR-CG-03).
    ///
    /// [`ApiPath`]: NodeKind::ApiPath
    ApiOperation = 34,
}

impl NodeKind {
    /// Every node kind, in declaration (discriminant) order.
    ///
    /// Used by `graph-store` to emit the `CHECK (kind IN (…))` clause and by
    /// tests asserting the taxonomy is complete (exactly 34).
    pub const ALL: [NodeKind; 34] = [
        NodeKind::Module,
        NodeKind::Class,
        NodeKind::Interface,
        NodeKind::Trait,
        NodeKind::Struct,
        NodeKind::Enum,
        NodeKind::Function,
        NodeKind::Method,
        NodeKind::Field,
        NodeKind::Constant,
        NodeKind::Variable,
        NodeKind::TypeAlias,
        NodeKind::Macro,
        NodeKind::Route,
        NodeKind::Component,
        NodeKind::Layer,
        NodeKind::Boundary,
        NodeKind::DocFile,
        NodeKind::DocSection,
        NodeKind::Requirement,
        NodeKind::Adr,
        NodeKind::Story,
        NodeKind::ConfigFile,
        NodeKind::ConfigSection,
        NodeKind::ShellFunction,
        NodeKind::DockerfileStage,
        NodeKind::MakeTarget,
        NodeKind::ProtoMessage,
        NodeKind::ProtoService,
        NodeKind::GqlType,
        NodeKind::SqlObject,
        NodeKind::TfBlock,
        NodeKind::ApiPath,
        NodeKind::ApiOperation,
    ];

    /// The stable integer discriminant written to the `nodes.kind` column.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// `true` for the five documentation kinds (CR-003, [ADR-19]): the generic
    /// [`DocFile`](NodeKind::DocFile)/[`DocSection`](NodeKind::DocSection) layer
    /// and the typed [`Requirement`](NodeKind::Requirement)/[`Adr`](NodeKind::Adr)/[`Story`](NodeKind::Story)
    /// enrichment. The single source of truth the doc→code matcher
    /// ([FR-DG-04](../../../../docs/specs/requirements/FR-DG-04.md)) and the
    /// traceability queries ([FR-NV-10](../../../../docs/specs/requirements/FR-NV-10.md),
    /// S-037) use to separate documentation from code.
    pub const fn is_doc(self) -> bool {
        self.is_documentation()
    }

    /// The lower-case wire/CLI name (matches the `serde` representation and the
    /// `search(query, kind?)` filter token).
    pub const fn as_str(self) -> &'static str {
        match self {
            NodeKind::Module => "module",
            NodeKind::Class => "class",
            NodeKind::Interface => "interface",
            NodeKind::Trait => "trait",
            NodeKind::Struct => "struct",
            NodeKind::Enum => "enum",
            NodeKind::Function => "function",
            NodeKind::Method => "method",
            NodeKind::Field => "field",
            NodeKind::Constant => "constant",
            NodeKind::Variable => "variable",
            NodeKind::TypeAlias => "type_alias",
            NodeKind::Macro => "macro",
            NodeKind::Route => "route",
            NodeKind::Component => "component",
            NodeKind::Layer => "layer",
            NodeKind::Boundary => "boundary",
            NodeKind::DocFile => "doc_file",
            NodeKind::DocSection => "doc_section",
            NodeKind::Requirement => "requirement",
            NodeKind::Adr => "adr",
            NodeKind::Story => "story",
            NodeKind::ConfigFile => "config_file",
            NodeKind::ConfigSection => "config_section",
            NodeKind::ShellFunction => "shell_function",
            NodeKind::DockerfileStage => "dockerfile_stage",
            NodeKind::MakeTarget => "make_target",
            NodeKind::ProtoMessage => "proto_message",
            NodeKind::ProtoService => "proto_service",
            NodeKind::GqlType => "gql_type",
            NodeKind::SqlObject => "sql_object",
            NodeKind::TfBlock => "tf_block",
            NodeKind::ApiPath => "api_path",
            NodeKind::ApiOperation => "api_operation",
        }
    }

    /// The inverse of [`as_str`](NodeKind::as_str): resolve a lower-case wire
    /// name back to its kind, or `None` for an unrecognised name. The lookup
    /// scans [`ALL`](NodeKind::ALL) so it can never drift from `as_str`. Used to
    /// validate and resolve the declarative `[[config.anchors]]` `kind` field of
    /// an artifact descriptor (S-065, [CR-010], [FR-CG-03]) — a typo there is a
    /// loud descriptor parse error, not a silently dropped anchor.
    ///
    /// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-03]: ../../../../docs/specs/requirements/FR-CG-03.md
    pub fn from_wire(wire: &str) -> Option<NodeKind> {
        NodeKind::ALL.iter().copied().find(|k| k.as_str() == wire)
    }

    /// Whether this is a documentation-layer kind — the generic markdown nodes
    /// ([`DocFile`](NodeKind::DocFile)/[`DocSection`](NodeKind::DocSection)) and
    /// the typed swe-skills enrichment
    /// ([`Requirement`](NodeKind::Requirement)/[`Adr`](NodeKind::Adr)/[`Story`](NodeKind::Story)).
    ///
    /// The single source of truth for the doc-kind set that graph hydration
    /// excludes from the code subgraph so metrics, cycle detection, DSM, and
    /// dead-code see code only — the same filter shape [ADR-18] uses for
    /// `is_test` ([FR-DG-06], [FR-QM-08], [ADR-19]). Adding or removing
    /// documentation therefore leaves the aggregate signal byte-identical.
    ///
    /// [ADR-18]: ../../../../docs/specs/architecture/decisions/ADR-18.md
    /// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
    /// [FR-DG-06]: ../../../../docs/specs/requirements/FR-DG-06.md
    /// [FR-QM-08]: ../../../../docs/specs/requirements/FR-QM-08.md
    pub const fn is_documentation(self) -> bool {
        matches!(
            self,
            NodeKind::DocFile
                | NodeKind::DocSection
                | NodeKind::Requirement
                | NodeKind::Adr
                | NodeKind::Story
        )
    }

    /// Whether this is a config/artifact-layer kind — the generic
    /// [`ConfigFile`](NodeKind::ConfigFile)/[`ConfigSection`](NodeKind::ConfigSection)
    /// layer and the per-format typed anchors (CR-010, [ADR-25], [FR-CG-02],
    /// [FR-CG-03]). The config sibling of [`is_documentation`](NodeKind::is_documentation),
    /// folded into [`is_non_code`](NodeKind::is_non_code) — the single non-code
    /// scope graph hydration excludes from the code subgraph so adding or removing
    /// any config artifact leaves the aggregate signal byte-identical ([FR-CG-05],
    /// [BR-31]). Shell is deliberately in this set: a `ShellFunction` is a
    /// structural node only, never a metric contribution.
    ///
    /// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
    /// [FR-CG-02]: ../../../../docs/specs/requirements/FR-CG-02.md
    /// [FR-CG-03]: ../../../../docs/specs/requirements/FR-CG-03.md
    /// [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
    pub const fn is_config(self) -> bool {
        matches!(
            self,
            NodeKind::ConfigFile
                | NodeKind::ConfigSection
                | NodeKind::ShellFunction
                | NodeKind::DockerfileStage
                | NodeKind::MakeTarget
                | NodeKind::ProtoMessage
                | NodeKind::ProtoService
                | NodeKind::GqlType
                | NodeKind::SqlObject
                | NodeKind::TfBlock
                | NodeKind::ApiPath
                | NodeKind::ApiOperation
        )
    }

    /// Whether this kind belongs to a **non-code layer** — documentation
    /// ([`is_documentation`](NodeKind::is_documentation)) or config/artifact
    /// ([`is_config`](NodeKind::is_config)). The single predicate graph hydration
    /// uses to keep the code subgraph code-only, generalizing the doc-kinds
    /// filter of [ADR-19] to "documentation **and** config artifacts" per
    /// [ADR-25]/[FR-CG-05]/[BR-31]: a `Contains`-only config node is dropped at the
    /// hydration boundary, and its incident edges fall away with it, so metrics,
    /// cycle detection, DSM, and dead-code never see it.
    ///
    /// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
    /// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
    /// [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
    pub const fn is_non_code(self) -> bool {
        self.is_documentation() || self.is_config()
    }
}

impl TryFrom<i32> for NodeKind {
    type Error = UnknownKind;

    /// Recover a [`NodeKind`] from its stored discriminant.
    ///
    /// # Errors
    /// Returns [`UnknownKind`] if `value` is not a known node-kind discriminant.
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(NodeKind::Module),
            2 => Ok(NodeKind::Class),
            3 => Ok(NodeKind::Interface),
            4 => Ok(NodeKind::Trait),
            5 => Ok(NodeKind::Struct),
            6 => Ok(NodeKind::Enum),
            7 => Ok(NodeKind::Function),
            8 => Ok(NodeKind::Method),
            9 => Ok(NodeKind::Field),
            10 => Ok(NodeKind::Constant),
            11 => Ok(NodeKind::Variable),
            12 => Ok(NodeKind::TypeAlias),
            13 => Ok(NodeKind::Macro),
            14 => Ok(NodeKind::Route),
            15 => Ok(NodeKind::Component),
            16 => Ok(NodeKind::Layer),
            17 => Ok(NodeKind::Boundary),
            18 => Ok(NodeKind::DocFile),
            19 => Ok(NodeKind::DocSection),
            20 => Ok(NodeKind::Requirement),
            21 => Ok(NodeKind::Adr),
            22 => Ok(NodeKind::Story),
            23 => Ok(NodeKind::ConfigFile),
            24 => Ok(NodeKind::ConfigSection),
            25 => Ok(NodeKind::ShellFunction),
            26 => Ok(NodeKind::DockerfileStage),
            27 => Ok(NodeKind::MakeTarget),
            28 => Ok(NodeKind::ProtoMessage),
            29 => Ok(NodeKind::ProtoService),
            30 => Ok(NodeKind::GqlType),
            31 => Ok(NodeKind::SqlObject),
            32 => Ok(NodeKind::TfBlock),
            33 => Ok(NodeKind::ApiPath),
            34 => Ok(NodeKind::ApiOperation),
            other => Err(UnknownKind(other)),
        }
    }
}

/// The 15 edge kinds of the Logos data model.
///
/// [`Contains`] is the lexical-containment edge that graph hydration excludes
/// from the dependency graph ([FR-DB-06](../../../../docs/specs/requirements/FR-DB-06.md));
/// [`ForbiddenDependency`] is the derived governance edge materialised by
/// `check_rules` ([FR-GV-02](../../../../docs/specs/requirements/FR-GV-02.md));
/// [`RoutesTo`] links a [`NodeKind::Route`] to its handler
/// ([FR-FW-01](../../../../docs/specs/requirements/FR-FW-01.md)). The two
/// documentation edges added by [CR-003](../../../../docs/requests/CR-003-documentation-graph-layer.md)
/// — [`DocReference`] (a resolved doc→doc link or doc→code reference,
/// [FR-DG-03](../../../../docs/specs/requirements/FR-DG-03.md)/[FR-DG-04](../../../../docs/specs/requirements/FR-DG-04.md))
/// and [`TracesTo`] (a typed `Requirement`/`ADR`/`Story` trace,
/// [FR-DG-07](../../../../docs/specs/requirements/FR-DG-07.md)) — are distinct
/// kinds precisely so the hydration scope filter can exclude them from the code
/// subgraph ([ADR-19](../../../../docs/specs/architecture/decisions/ADR-19.md),
/// [FR-DG-06](../../../../docs/specs/requirements/FR-DG-06.md)). Discriminants
/// are frozen — see the module docs.
///
/// [`Contains`]: EdgeKind::Contains
/// [`ForbiddenDependency`]: EdgeKind::ForbiddenDependency
/// [`RoutesTo`]: EdgeKind::RoutesTo
/// [`DocReference`]: EdgeKind::DocReference
/// [`TracesTo`]: EdgeKind::TracesTo
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum EdgeKind {
    /// Lexical containment (module → function, class → method). Excluded from
    /// the dependency graph; drives module rollup instead.
    Contains = 1,
    /// A call from one symbol to another.
    Calls = 2,
    /// An import / use / require of a module or symbol.
    Imports = 3,
    /// A non-call reference (a type mention, a name read).
    References = 4,
    /// A type implements an interface or trait.
    Implements = 5,
    /// Inheritance — a subclass extends a superclass, a subtrait its supertrait.
    Extends = 6,
    /// A symbol constructs / instantiates a type.
    Instantiates = 7,
    /// A symbol uses a type in its signature or fields (a typed-by relation).
    TypeUses = 8,
    /// A framework route node to the handler symbol it dispatches to.
    RoutesTo = 9,
    /// A derived governance edge marking a dependency that violates `rules.toml`.
    ForbiddenDependency = 10,
    /// A resolved documentation reference: a doc→doc markdown link to a
    /// [`DocFile`](NodeKind::DocFile)/[`DocSection`](NodeKind::DocSection), or a
    /// doc→code reference bound to a code symbol (FR-DG-03/FR-DG-04, S-035).
    /// Bound only on an exactly-one-candidate match — never fabricated.
    DocReference = 11,
    /// A typed traceability edge between the swe-skills typed doc nodes
    /// ([`Requirement`](NodeKind::Requirement)/[`Adr`](NodeKind::Adr)/[`Story`](NodeKind::Story))
    /// — e.g. a story `traces-to` a requirement (FR-DG-07, S-039).
    TracesTo = 12,
    /// A member-access fact: a method reads a field of its own class-like
    /// container ([`NodeKind::Method`] → [`NodeKind::Field`], CR-005, [FR-EX-08]).
    /// Bound only on an exactly-one-candidate match — never fabricated
    /// ([NFR-RA-05]); an ambiguous or unmatched access stays in the
    /// `unresolved_refs` ledger and is retried on sync. The input to the LCOM4
    /// Cohesion dimension ([FR-QM-11]); also navigable as a field-usage fact.
    ///
    /// Excluded from the canonical dependency view the five original metrics run
    /// on ([ADR-21]) — it is a structural fact, not a code-coupling edge — so
    /// admitting it leaves the original quality signal byte-identical.
    ///
    /// [FR-EX-08]: ../../../../docs/specs/requirements/FR-EX-08.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
    Accesses = 13,
    /// A resolved **artifact→artifact** cross-reference (CR-011, [FR-CG-07]): a
    /// proto import binding to its sibling `ConfigFile`, a Terraform `var.x`
    /// reference to its declaring `TfBlock`, a GraphQL type referencing another
    /// type, … The relation class (`proto-import`, `tf-module-call`, …) is carried
    /// in the edge's **payload** rather than burning a discriminant per relation —
    /// payload subtyping caps frozen-discriminant growth exactly as the [ADR-25]
    /// node anchors do. Bound only on an exactly-one-candidate match — never
    /// fabricated ([NFR-RA-05]); externals are classified non-candidates before
    /// the ledger, and an ambiguous/unindexed workspace-relative reference stays
    /// in `unresolved_refs` and retries on sync.
    ///
    /// Excluded from the code subgraph at the hydration audit point
    /// ([`is_config_reference`](EdgeKind::is_config_reference)): an artifact
    /// reference is never a code-coupling dependency, so admitting it leaves the
    /// gated signal, cycles, DSM, and dead-code byte-identical ([ADR-26],
    /// [FR-CG-05]).
    ///
    /// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
    /// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
    ArtifactRef = 14,
    /// A resolved **artifact→code** binding (CR-011, [FR-CG-07]): an
    /// `ApiOperation` to the framework `route` handler it specifies, a proto/
    /// GraphQL declared type-name to the code symbol that implements it, … The
    /// relation class (`route`, `type-name`, …) is carried in the edge's
    /// **payload**, as for [`ArtifactRef`](EdgeKind::ArtifactRef). Bound only on
    /// an exactly-one-candidate match with no synthesized candidates — never
    /// fabricated ([NFR-RA-05]).
    ///
    /// These are exactly the **cross-layer** edges the metric scope must fence:
    /// excluded from the code subgraph at the hydration audit point
    /// ([`is_config_reference`](EdgeKind::is_config_reference)) so the gated
    /// signal stays byte-identical with the wiring present ([ADR-26],
    /// [FR-CG-05]).
    ///
    /// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
    ArtifactBinding = 15,
}

impl EdgeKind {
    /// Every edge kind, in declaration (discriminant) order.
    pub const ALL: [EdgeKind; 15] = [
        EdgeKind::Contains,
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::References,
        EdgeKind::Implements,
        EdgeKind::Extends,
        EdgeKind::Instantiates,
        EdgeKind::TypeUses,
        EdgeKind::RoutesTo,
        EdgeKind::ForbiddenDependency,
        EdgeKind::DocReference,
        EdgeKind::TracesTo,
        EdgeKind::Accesses,
        EdgeKind::ArtifactRef,
        EdgeKind::ArtifactBinding,
    ];

    /// The stable integer discriminant written to the `edges.kind` column.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// The lower-case wire/CLI name (matches the `serde` representation).
    pub const fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::References => "references",
            EdgeKind::Implements => "implements",
            EdgeKind::Extends => "extends",
            EdgeKind::Instantiates => "instantiates",
            EdgeKind::TypeUses => "type_uses",
            EdgeKind::RoutesTo => "routes_to",
            EdgeKind::ForbiddenDependency => "forbidden_dependency",
            EdgeKind::DocReference => "doc_reference",
            EdgeKind::TracesTo => "traces_to",
            EdgeKind::Accesses => "accesses",
            EdgeKind::ArtifactRef => "artifact_ref",
            EdgeKind::ArtifactBinding => "artifact_binding",
        }
    }

    /// The inverse of [`as_str`](EdgeKind::as_str): resolve a lower-case wire name
    /// back to its kind, or `None` for an unrecognised name. The lookup scans
    /// [`ALL`](EdgeKind::ALL) so it can never drift from `as_str` — the sibling of
    /// [`NodeKind::from_wire`]. Used by the web canvas's server-side `edge_types`
    /// re-budgeting filter (S-122, [FR-UI-15]) to validate the comma-separated wire
    /// tokens before they narrow the graph-elements query, dropping any
    /// unrecognised token rather than erroring.
    ///
    /// [FR-UI-15]: ../../../../docs/specs/requirements/FR-UI-15.md
    pub fn from_wire(wire: &str) -> Option<EdgeKind> {
        EdgeKind::ALL.iter().copied().find(|k| k.as_str() == wire)
    }

    /// Whether this is a documentation-layer edge — a resolved doc→doc / doc→code
    /// [`DocReference`](EdgeKind::DocReference) ([FR-DG-03]/[FR-DG-04]) or a typed
    /// [`TracesTo`](EdgeKind::TracesTo) trace ([FR-DG-07]).
    ///
    /// The companion of [`NodeKind::is_documentation`]: graph hydration drops
    /// these edges from the code subgraph so the dependency graph the metrics,
    /// cycle detection, and DSM run on carries no documentation coupling
    /// ([FR-DG-06], [ADR-19]).
    ///
    /// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
    /// [FR-DG-03]: ../../../../docs/specs/requirements/FR-DG-03.md
    /// [FR-DG-04]: ../../../../docs/specs/requirements/FR-DG-04.md
    /// [FR-DG-06]: ../../../../docs/specs/requirements/FR-DG-06.md
    /// [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
    pub const fn is_documentation(self) -> bool {
        matches!(self, EdgeKind::DocReference | EdgeKind::TracesTo)
    }

    /// Whether this is a cross-artifact reference edge — an
    /// [`ArtifactRef`](EdgeKind::ArtifactRef) (artifact→artifact) or an
    /// [`ArtifactBinding`](EdgeKind::ArtifactBinding) (artifact→code), added by
    /// [CR-011]/[ADR-26].
    ///
    /// The edge-layer companion of [`NodeKind::is_config`]: graph hydration drops
    /// these edges from the code subgraph **at the same audit point** as the
    /// non-code node predicate ([`NodeKind::is_non_code`]), so the dependency
    /// graph the metrics, cycle detection, DSM, and dead-code run on carries no
    /// artifact wiring ([FR-CG-05], BR-32). An `ArtifactBinding` is precisely the
    /// cross-layer edge that would otherwise leak a non-code endpoint's coupling
    /// into the signal; this predicate is the explicit fence guaranteeing the
    /// gated number stays byte-identical with the wiring present ([UAT-CG-04]).
    ///
    /// [CR-011]: ../../../../docs/requests/CR-011-cross-artifact-resolution.md
    /// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
    /// [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
    /// [UAT-CG-04]: ../../../../docs/specs/requirements/UAT-CG-04.md
    pub const fn is_config_reference(self) -> bool {
        matches!(self, EdgeKind::ArtifactRef | EdgeKind::ArtifactBinding)
    }
}

impl TryFrom<i32> for EdgeKind {
    type Error = UnknownKind;

    /// Recover an [`EdgeKind`] from its stored discriminant.
    ///
    /// # Errors
    /// Returns [`UnknownKind`] if `value` is not a known edge-kind discriminant.
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(EdgeKind::Contains),
            2 => Ok(EdgeKind::Calls),
            3 => Ok(EdgeKind::Imports),
            4 => Ok(EdgeKind::References),
            5 => Ok(EdgeKind::Implements),
            6 => Ok(EdgeKind::Extends),
            7 => Ok(EdgeKind::Instantiates),
            8 => Ok(EdgeKind::TypeUses),
            9 => Ok(EdgeKind::RoutesTo),
            10 => Ok(EdgeKind::ForbiddenDependency),
            11 => Ok(EdgeKind::DocReference),
            12 => Ok(EdgeKind::TracesTo),
            13 => Ok(EdgeKind::Accesses),
            14 => Ok(EdgeKind::ArtifactRef),
            15 => Ok(EdgeKind::ArtifactBinding),
            other => Err(UnknownKind(other)),
        }
    }
}

/// The 4 *target forms* an extracted reference can take — how the
/// `unresolved_refs.target` text is to be interpreted by the resolution pass
/// ([FR-RS-03](../../../../docs/specs/requirements/FR-RS-03.md),
/// [ADR-10](../../../../docs/specs/architecture/decisions/ADR-10.md)).
///
/// Like [`NodeKind`]/[`EdgeKind`], the discriminants are a frozen part of the
/// on-disk contract: the `unresolved_refs.form` column is guarded by
/// `CHECK (form IN (1,2,3,4))` (migration 2). The evolution rules from the
/// module docs apply — never reorder, never reuse, append with the next free
/// integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum RefForm {
    /// A textual language path (`crate::extract::extract_files`, `helpers::run`,
    /// or a bare `run`) to be bound by the scope-hierarchy rules.
    Path = 1,
    /// An exact canonical [`LogosSymbol`](crate::model::LogosSymbol) string —
    /// produced by capture-before-delete ([ADR-10]) — bound by exact symbol
    /// lookup, no scope inference.
    ///
    /// [ADR-10]: ../../../../docs/specs/architecture/decisions/ADR-10.md
    Symbol = 2,
    /// A receiver-method name (`x.foo()` → `foo`): the receiver type is
    /// unknown to extraction, so only a policy-gated unique-name match can
    /// bind it.
    Method = 3,
    /// A glob import (`use m::*` → target `m`): binds to the module node and
    /// additionally brings that module's members into the importing file's
    /// name scope.
    Glob = 4,
}

impl RefForm {
    /// Every ref form, in declaration (discriminant) order.
    ///
    /// Used by `graph-store` tests to assert the migration-2 `CHECK (form IN
    /// (…))` clause never drifts from this enum.
    pub const ALL: [RefForm; 4] = [
        RefForm::Path,
        RefForm::Symbol,
        RefForm::Method,
        RefForm::Glob,
    ];

    /// The stable integer discriminant written to the `unresolved_refs.form`
    /// column.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// The lower-case wire/CLI name (matches the `serde` representation).
    pub const fn as_str(self) -> &'static str {
        match self {
            RefForm::Path => "path",
            RefForm::Symbol => "symbol",
            RefForm::Method => "method",
            RefForm::Glob => "glob",
        }
    }
}

impl TryFrom<i32> for RefForm {
    type Error = UnknownKind;

    /// Recover a [`RefForm`] from its stored discriminant.
    ///
    /// # Errors
    /// Returns [`UnknownKind`] if `value` is not a known ref-form discriminant.
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(RefForm::Path),
            2 => Ok(RefForm::Symbol),
            3 => Ok(RefForm::Method),
            4 => Ok(RefForm::Glob),
            other => Err(UnknownKind(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_taxonomy_has_exactly_34() {
        assert_eq!(NodeKind::ALL.len(), 34);
    }

    #[test]
    fn edge_kind_taxonomy_has_exactly_15() {
        assert_eq!(EdgeKind::ALL.len(), 15);
    }

    #[test]
    fn ref_form_discriminants_are_dense_1_through_4_in_order() {
        for (idx, form) in RefForm::ALL.iter().enumerate() {
            assert_eq!(
                form.as_i32(),
                idx as i32 + 1,
                "{form:?} discriminant drifted — the on-disk contract is frozen"
            );
        }
    }

    #[test]
    fn ref_form_roundtrips_through_i32_and_rejects_out_of_range() {
        for form in RefForm::ALL {
            assert_eq!(RefForm::try_from(form.as_i32()), Ok(form));
        }
        assert_eq!(RefForm::try_from(0), Err(UnknownKind(0)));
        assert_eq!(RefForm::try_from(5), Err(UnknownKind(5)));
    }

    #[test]
    fn ref_form_serialises_as_snake_case_name() {
        for form in RefForm::ALL {
            let json = serde_json::to_string(&form).unwrap();
            assert_eq!(json, format!("\"{}\"", form.as_str()));
            assert_eq!(serde_json::from_str::<RefForm>(&json).unwrap(), form);
        }
    }

    #[test]
    fn node_discriminants_are_dense_1_through_34_in_order() {
        for (idx, kind) in NodeKind::ALL.iter().enumerate() {
            assert_eq!(
                kind.as_i32(),
                idx as i32 + 1,
                "{kind:?} discriminant drifted — the on-disk contract is frozen"
            );
        }
    }

    #[test]
    fn edge_discriminants_are_dense_1_through_15_in_order() {
        for (idx, kind) in EdgeKind::ALL.iter().enumerate() {
            assert_eq!(
                kind.as_i32(),
                idx as i32 + 1,
                "{kind:?} discriminant drifted — the on-disk contract is frozen"
            );
        }
    }

    #[test]
    fn zero_is_never_a_valid_node_kind() {
        assert_eq!(NodeKind::try_from(0), Err(UnknownKind(0)));
    }

    #[test]
    fn zero_is_never_a_valid_edge_kind() {
        assert_eq!(EdgeKind::try_from(0), Err(UnknownKind(0)));
    }

    #[test]
    fn node_kind_roundtrips_through_i32() {
        for kind in NodeKind::ALL {
            assert_eq!(NodeKind::try_from(kind.as_i32()), Ok(kind));
        }
    }

    #[test]
    fn node_kind_roundtrips_through_wire_name() {
        // `from_wire` is the exact inverse of `as_str` for every kind, and an
        // unknown name is `None` — the contract the artifact descriptor's
        // `[[config.anchors]]` `kind` validation rests on (S-065, FR-CG-03).
        for kind in NodeKind::ALL {
            assert_eq!(NodeKind::from_wire(kind.as_str()), Some(kind));
        }
        assert_eq!(NodeKind::from_wire("gql_type"), Some(NodeKind::GqlType));
        assert_eq!(
            NodeKind::from_wire("proto_message"),
            Some(NodeKind::ProtoMessage)
        );
        assert_eq!(NodeKind::from_wire("not_a_kind"), None);
        assert_eq!(NodeKind::from_wire(""), None);
    }

    #[test]
    fn edge_kind_roundtrips_through_i32() {
        for kind in EdgeKind::ALL {
            assert_eq!(EdgeKind::try_from(kind.as_i32()), Ok(kind));
        }
    }

    #[test]
    fn edge_kind_roundtrips_through_wire_name() {
        // `from_wire` is the exact inverse of `as_str` for every edge kind, and an
        // unknown token is `None` — the contract the canvas's server-side
        // `edge_types` re-budgeting filter rests on (S-122, FR-UI-15): a malformed
        // token is dropped, never an error.
        for kind in EdgeKind::ALL {
            assert_eq!(EdgeKind::from_wire(kind.as_str()), Some(kind));
        }
        assert_eq!(EdgeKind::from_wire("calls"), Some(EdgeKind::Calls));
        assert_eq!(EdgeKind::from_wire("doc_reference"), Some(EdgeKind::DocReference));
        assert_eq!(EdgeKind::from_wire("not_an_edge"), None);
        assert_eq!(EdgeKind::from_wire(""), None);
    }

    #[test]
    fn out_of_range_discriminants_are_rejected() {
        assert_eq!(NodeKind::try_from(35), Err(UnknownKind(35)));
        assert_eq!(NodeKind::try_from(-1), Err(UnknownKind(-1)));
        assert_eq!(EdgeKind::try_from(16), Err(UnknownKind(16)));
    }

    /// The twelve config & artifact kinds added by CR-010/ADR-25 ride the
    /// documented evolution path: appended with the next free integers (23..=34),
    /// never reordered, with their `config_*`/typed-anchor wire names
    /// ([FR-CG-02], [FR-CG-03]). CR-010 is a `Contains`-only layer, so it appends
    /// node kinds **only** — it added no edge kind (the two artifact edge kinds
    /// arrive later with CR-011, discriminants 14..=15).
    #[test]
    fn config_node_kinds_carry_the_appended_discriminants() {
        let expected = [
            (NodeKind::ConfigFile, 23, "config_file"),
            (NodeKind::ConfigSection, 24, "config_section"),
            (NodeKind::ShellFunction, 25, "shell_function"),
            (NodeKind::DockerfileStage, 26, "dockerfile_stage"),
            (NodeKind::MakeTarget, 27, "make_target"),
            (NodeKind::ProtoMessage, 28, "proto_message"),
            (NodeKind::ProtoService, 29, "proto_service"),
            (NodeKind::GqlType, 30, "gql_type"),
            (NodeKind::SqlObject, 31, "sql_object"),
            (NodeKind::TfBlock, 32, "tf_block"),
            (NodeKind::ApiPath, 33, "api_path"),
            (NodeKind::ApiOperation, 34, "api_operation"),
        ];
        for (kind, disc, name) in expected {
            assert_eq!(kind.as_i32(), disc, "{name} discriminant");
            assert_eq!(kind.as_str(), name, "{name} wire name");
            assert_eq!(NodeKind::try_from(disc), Ok(kind), "{name} round-trip");
        }
        // CR-010 burned no edge kinds — the layer was Contains-only (FR-EX-05
        // note). The two artifact edge kinds (14..=15) are the CR-011 addition,
        // lifting the total to 15.
        assert_eq!(EdgeKind::ALL.len(), 15, "edge kinds total after CR-011");
        assert!(
            !EdgeKind::ArtifactRef.is_documentation()
                && !EdgeKind::ArtifactBinding.is_documentation(),
            "artifact edges are not documentation edges"
        );
    }

    /// `is_config` partitions the taxonomy into exactly the twelve config kinds
    /// (23..=34) and nothing else, and `is_non_code = is_documentation ∪ is_config`
    /// (17 kinds) — the single non-code scope graph hydration excludes from the
    /// code subgraph so adding or removing config artifacts leaves the signal
    /// byte-identical ([FR-CG-05], [ADR-25], [BR-31]).
    #[test]
    fn is_config_and_is_non_code_partition_the_taxonomy() {
        let config_count = NodeKind::ALL.iter().filter(|k| k.is_config()).count();
        assert_eq!(config_count, 12, "exactly twelve config node kinds");

        for kind in NodeKind::ALL {
            // The three layers are disjoint: a kind is code, doc, or config.
            assert!(
                !(kind.is_documentation() && kind.is_config()),
                "{} cannot be both documentation and config",
                kind.as_str()
            );
            assert_eq!(
                kind.is_non_code(),
                kind.is_documentation() || kind.is_config(),
                "{} non-code = doc ∪ config",
                kind.as_str()
            );
        }
        // The boundary: a code kind is neither; a config kind is config + non-code
        // but not documentation; a doc kind is documentation + non-code but not config.
        assert!(!NodeKind::Function.is_non_code());
        assert!(NodeKind::ConfigSection.is_config() && NodeKind::ConfigSection.is_non_code());
        assert!(!NodeKind::ConfigSection.is_documentation());
        assert!(NodeKind::DocSection.is_documentation() && NodeKind::DocSection.is_non_code());
        assert!(!NodeKind::DocSection.is_config());
        // 17 non-code kinds = 5 doc + 12 config.
        assert_eq!(
            NodeKind::ALL.iter().filter(|k| k.is_non_code()).count(),
            17,
            "five documentation + twelve config kinds"
        );
    }

    /// The member-access `Accesses` edge added by CR-005/ADR-21 rides the
    /// documented evolution path: appended with the next free integer (13),
    /// never reordered, with its `accesses` wire name ([FR-EX-08]).
    #[test]
    fn accesses_edge_kind_carries_the_appended_discriminant() {
        assert_eq!(EdgeKind::Accesses.as_i32(), 13);
        assert_eq!(EdgeKind::Accesses.as_str(), "accesses");
        assert_eq!(EdgeKind::try_from(13), Ok(EdgeKind::Accesses));
        // It is a code-structural fact, never a documentation edge.
        assert!(!EdgeKind::Accesses.is_documentation());
    }

    /// The two cross-artifact edges (CR-011/ADR-26) ride the documented evolution
    /// path: appended with the next free integers (14..=15), never reordered, with
    /// their `artifact_*` wire names ([FR-CG-07]). They are payload-subtyped, so
    /// the relation class lives in the edge payload, not in a new discriminant.
    #[test]
    fn artifact_edge_kinds_carry_the_appended_discriminants() {
        assert_eq!(EdgeKind::ArtifactRef.as_i32(), 14);
        assert_eq!(EdgeKind::ArtifactBinding.as_i32(), 15);
        assert_eq!(EdgeKind::ArtifactRef.as_str(), "artifact_ref");
        assert_eq!(EdgeKind::ArtifactBinding.as_str(), "artifact_binding");
        assert_eq!(EdgeKind::try_from(14), Ok(EdgeKind::ArtifactRef));
        assert_eq!(EdgeKind::try_from(15), Ok(EdgeKind::ArtifactBinding));
    }

    /// `EdgeKind::is_config_reference` marks exactly `ArtifactRef`/`ArtifactBinding`
    /// — the cross-artifact edges hydration drops at the same audit point as the
    /// non-code node predicate, so the gated signal stays byte-identical with the
    /// wiring present ([FR-CG-05], [ADR-26], BR-32). Driving it off `ALL` means a
    /// newly appended edge kind cannot silently join or skip the artifact fence.
    #[test]
    fn is_config_reference_marks_exactly_the_two_artifact_edge_kinds() {
        for kind in EdgeKind::ALL {
            let expected = matches!(kind, EdgeKind::ArtifactRef | EdgeKind::ArtifactBinding);
            assert_eq!(
                kind.is_config_reference(),
                expected,
                "{} artifact-classification drifted",
                kind.as_str()
            );
            // The artifact fence and the documentation fence are disjoint.
            assert!(
                !(kind.is_config_reference() && kind.is_documentation()),
                "{} cannot be both an artifact and a documentation edge",
                kind.as_str()
            );
        }
        assert!(!EdgeKind::Calls.is_config_reference());
        assert!(!EdgeKind::Contains.is_config_reference());
        assert_eq!(
            EdgeKind::ALL
                .iter()
                .filter(|k| k.is_config_reference())
                .count(),
            2,
            "exactly two cross-artifact edge kinds"
        );
    }

    #[test]
    fn artifact_edge_kinds_serialise_as_snake_case_name() {
        for kind in [EdgeKind::ArtifactRef, EdgeKind::ArtifactBinding] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(serde_json::from_str::<EdgeKind>(&json).unwrap(), kind);
        }
    }

    /// The five documentation kinds added by CR-003/ADR-19 ride the documented
    /// evolution path: appended with the next free integers (18..=22), never
    /// reordered, with their `doc_*`/typed wire names ([FR-DG-02], [FR-DG-07]).
    #[test]
    fn documentation_node_kinds_carry_the_appended_discriminants() {
        assert_eq!(NodeKind::DocFile.as_i32(), 18);
        assert_eq!(NodeKind::DocSection.as_i32(), 19);
        assert_eq!(NodeKind::Requirement.as_i32(), 20);
        assert_eq!(NodeKind::Adr.as_i32(), 21);
        assert_eq!(NodeKind::Story.as_i32(), 22);
        assert_eq!(NodeKind::DocFile.as_str(), "doc_file");
        assert_eq!(NodeKind::DocSection.as_str(), "doc_section");
        assert_eq!(NodeKind::Requirement.as_str(), "requirement");
        assert_eq!(NodeKind::Adr.as_str(), "adr");
        assert_eq!(NodeKind::Story.as_str(), "story");
    }

    /// The two documentation edges (CR-003/ADR-19) are appended at 11/12 — kept
    /// distinct from the code edges so the hydration scope filter (FR-DG-06) can
    /// exclude them from the code subgraph.
    #[test]
    fn documentation_edge_kinds_carry_the_appended_discriminants() {
        assert_eq!(EdgeKind::DocReference.as_i32(), 11);
        assert_eq!(EdgeKind::TracesTo.as_i32(), 12);
        assert_eq!(EdgeKind::DocReference.as_str(), "doc_reference");
        assert_eq!(EdgeKind::TracesTo.as_str(), "traces_to");
    }

    /// `NodeKind::is_documentation` partitions the taxonomy into exactly the
    /// five doc kinds (18..=22) and nothing else — the doc-kind set graph
    /// hydration excludes from the code subgraph ([FR-DG-06], [ADR-19]). Driving
    /// it off `ALL` means a newly appended kind cannot silently join or skip the
    /// documentation partition without this test moving.
    #[test]
    fn is_documentation_marks_exactly_the_five_doc_node_kinds() {
        for kind in NodeKind::ALL {
            let expected = matches!(
                kind,
                NodeKind::DocFile
                    | NodeKind::DocSection
                    | NodeKind::Requirement
                    | NodeKind::Adr
                    | NodeKind::Story
            );
            assert_eq!(
                kind.is_documentation(),
                expected,
                "{} doc-classification drifted",
                kind.as_str()
            );
        }
        // The code kinds the metrics, cycle, DSM, and dead-code scopes keep.
        assert!(!NodeKind::Function.is_documentation());
        assert!(!NodeKind::Module.is_documentation());
        assert!(!NodeKind::Layer.is_documentation());
        assert_eq!(
            NodeKind::ALL
                .iter()
                .filter(|k| k.is_documentation())
                .count(),
            5,
            "exactly five documentation node kinds"
        );
    }

    /// `EdgeKind::is_documentation` marks exactly `DocReference`/`TracesTo` — the
    /// doc edges hydration drops so the dependency graph carries no doc coupling
    /// ([FR-DG-06], [ADR-19]).
    #[test]
    fn is_documentation_marks_exactly_the_two_doc_edge_kinds() {
        for kind in EdgeKind::ALL {
            let expected = matches!(kind, EdgeKind::DocReference | EdgeKind::TracesTo);
            assert_eq!(
                kind.is_documentation(),
                expected,
                "{} doc-classification drifted",
                kind.as_str()
            );
        }
        assert!(!EdgeKind::Calls.is_documentation());
        assert!(!EdgeKind::Contains.is_documentation());
        assert_eq!(
            EdgeKind::ALL
                .iter()
                .filter(|k| k.is_documentation())
                .count(),
            2,
            "exactly two documentation edge kinds"
        );
    }

    /// `is_doc` delegates to `is_documentation` — both are exactly the five
    /// documentation kinds (CR-003/ADR-19). Single predicate body, two public
    /// names kept for call-site compatibility.
    #[test]
    fn is_doc_is_exactly_the_documentation_kinds() {
        for kind in NodeKind::ALL {
            let expected = matches!(
                kind,
                NodeKind::DocFile
                    | NodeKind::DocSection
                    | NodeKind::Requirement
                    | NodeKind::Adr
                    | NodeKind::Story
            );
            assert_eq!(kind.is_doc(), expected, "{kind:?} doc-classification");
        }
        // Spot-check the boundary: a code kind is not doc, a doc kind is.
        assert!(!NodeKind::Function.is_doc());
        assert!(NodeKind::DocSection.is_doc());
    }

    /// The two policy kinds appended by the annotation engine (S-014,
    /// [FR-AN-03]) ride the documented evolution path: appended with the next
    /// free integers, never reordered.
    #[test]
    fn policy_kinds_carry_the_appended_discriminants() {
        assert_eq!(NodeKind::Layer.as_i32(), 16);
        assert_eq!(NodeKind::Boundary.as_i32(), 17);
        assert_eq!(NodeKind::Layer.as_str(), "layer");
        assert_eq!(NodeKind::Boundary.as_str(), "boundary");
    }

    #[test]
    fn node_kind_serialises_as_snake_case_name() {
        let json = serde_json::to_string(&NodeKind::TypeAlias).unwrap();
        assert_eq!(json, "\"type_alias\"");
        assert_eq!(NodeKind::TypeAlias.as_str(), "type_alias");
    }

    #[test]
    fn edge_kind_serialises_as_snake_case_name() {
        let json = serde_json::to_string(&EdgeKind::ForbiddenDependency).unwrap();
        assert_eq!(json, "\"forbidden_dependency\"");
        assert_eq!(
            EdgeKind::ForbiddenDependency.as_str(),
            "forbidden_dependency"
        );
    }

    #[test]
    fn every_node_kind_name_matches_serde() {
        for kind in NodeKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
        }
    }

    #[test]
    fn every_edge_kind_name_matches_serde() {
        for kind in EdgeKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
        }
    }

    #[test]
    fn node_kind_deserialises_from_snake_case_name() {
        // The read path of the `search(query, kind?)` wire form (FR-NV-01) —
        // a `rename_all`/`as_str` drift on deserialize would otherwise be
        // uncaught, since only the serialize path was previously tested.
        assert_eq!(
            serde_json::from_str::<NodeKind>("\"type_alias\"").unwrap(),
            NodeKind::TypeAlias
        );
        for kind in NodeKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<NodeKind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn edge_kind_deserialises_from_snake_case_name() {
        assert_eq!(
            serde_json::from_str::<EdgeKind>("\"forbidden_dependency\"").unwrap(),
            EdgeKind::ForbiddenDependency
        );
        for kind in EdgeKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<EdgeKind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn unknown_kind_names_are_rejected_on_deserialize() {
        assert!(serde_json::from_str::<NodeKind>("\"not_a_kind\"").is_err());
        assert!(serde_json::from_str::<NodeKind>("\"typealias\"").is_err());
        assert!(serde_json::from_str::<EdgeKind>("\"calls_to\"").is_err());
    }
}
