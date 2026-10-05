//! Pass 1 of the pipeline — the data-parallel extraction engine
//! ([extraction-engine], S-007).
//!
//! [`extract`] parses one file with a single grammar, runs the plugin's tagging
//! queries, and emits [`NodeFact`]s and [`EdgeFact`]s carrying canonical-ordinal
//! SCIP symbol IDs ([ADR-07]), cyclomatic complexity, and per-function line
//! counts. [`extract_files`] is the rayon driver: it parallelises the per-file
//! parse across cores, giving **each rayon worker its own
//! [`tree_sitter::Parser`]** (the Parser is not thread-shareable, [AR-05]) via
//! `map_init` ([FR-IX-03], [NFR-PE-08]).
//!
//! # Error tolerance ([FR-IX-04])
//!
//! tree-sitter recovers from syntax errors and still returns a parse tree with
//! the well-formed declarations around the break intact. A file that does not
//! parse cleanly is *partially* extracted — its [`Facts::partial`] flag is set
//! and a warning recorded — and the run is **never** aborted. No declaration is
//! lifted into an ERROR region, so none claims a span or body recovery tore off;
//! the warning counts the declarations the damage truncated or skipped
//! ([FR-EX-30]).
//!
//! # Determinism ([NFR-RA-06])
//!
//! Two facts make the output independent of how many rayon threads run:
//! `extract_files` collects results in input order, and within a file the
//! per-parent-scope **canonical sort** (`(start_byte, kind, name)`, [ADR-07])
//! assigns ordinals before they are folded into symbol IDs. Emitted nodes and
//! edges are themselves sorted, so the byte-for-byte output is fixed.
//!
//! [extraction-engine]: ../../../docs/specs/architecture/components/extraction-engine.md
//! [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
//! [AR-05]: ../../../docs/specs/architecture.md
//! [FR-IX-03]: ../../../docs/specs/requirements/FR-IX-03.md
//! [FR-IX-04]: ../../../docs/specs/requirements/FR-IX-04.md
//! [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
//! [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

mod complexity;
// Per-function max nesting depth (S-042, CR-005, FR-EX-07): a declarative
// block-kind walk, the structural sibling of `complexity`.
mod nesting;
// Winnowed near-clone shingle fingerprints (S-042, CR-005, FR-EX-09): a
// rename-invariant set fingerprint over the normalized token stream. `pub(crate)`
// so the near-clone clustering pass (`annotate::clone`, S-043) reads the fixed
// winnowing constants (K_GRAM/WINDOW) as the single source for its floor.
pub(crate) mod shingle;
// Structural documentation extraction (S-033, CR-003, ADR-19): a file whose
// plugin is a *documentation* grammar is parsed into a DocFile + nested
// DocSection tree here instead of via the code `symbols` query.
pub mod doc;
// Structural config & artifact extraction (S-062, CR-010, ADR-25): a file whose
// plugin is an *artifact* grammar is parsed into a ConfigFile + depth-bounded
// ConfigSection tree here (the third plugin class beside code and docs), instead
// of via the code `symbols` query.
pub mod config;
// Build manifests → member-local artifact facts (S-462, CR-148, ADR-69 point 1):
// Maven `pom.xml` and Gradle `build.gradle(.kts)` read into the artifacts a
// member produces and references. Not a grammar plugin — a manifest yields no
// node — so the pipeline drives it beside extraction rather than through it.
// PUBLIC so the reference-workspace census reads the product's own reader.
pub mod build_manifest;
// Declared types → member-local facts (S-472, CR-152, ADR-70 point 1): each
// top-level Java type under its package-aware name and each top-level type of a
// declared-namespace language (PHP, C#, Kotlin, Scala; S-518) under its declared
// namespace, and each `.avsc` record/enum under its namespace. The source half
// rides extraction (it needs the file's nodes and its `package` or namespace
// declaration); the schema half, like a build
// manifest, yields no node and is driven by the pipeline beside extraction.
// PUBLIC so the reference-workspace report reads the product's own reader.
pub mod declared_types;
// `pub(crate)`: the framework pass (resolve::framework, S-015) canonicalises
// captured handler paths and unquotes captured route-path literals with the
// same helpers extraction uses, so the two passes can never disagree on what
// a path's segments are.
pub(crate) mod refs;
// The message-broker publish/subscribe invocation arm's capture side (S-254,
// FR-WS-10): runs a grammar's optional `brokers` query and funnels topic-keyed
// sites through the generic `capture_invocation_refs` interpreter.
//
// PUBLIC since S-417, for the same reason `composer` is: the
// reference-workspace measurement must read the two-frame hop's own verdict
// (`Facts::forwarding`) rather than re-deriving a second one beside it. Only the
// forwarding carrier is reachable — the capture entry point stays `pub(super)`.
pub mod broker;
// The path-neutral composer contract test (S-405, CR-129, FR-WS-08 AC2): the
// byte-range reconciliation that lets a plugin query declare an arbitrary-length
// fluent URI-composer chain, which a fixed-nesting tree-sitter pattern cannot.
// PUBLIC because the reference-workspace operand-resolvability harness must
// count the arm's corpus with the arm's own rule rather than a second copy of it.
pub mod composer;
// Java receiver typing (S-467, CR-150 §3.2 A): retypes a receiver call's row to
// a type-qualified Path-form `T::name` where the file proves `T`.
mod receiver;
mod shape;
// Extraction-time test-marker evidence (S-027, FR-EX-06): the per-function
// `test_evidence` flag captured while the AST is in hand — the input the
// unified `is_test` annotation (S-028, FR-AN-05) needs to catch what path
// conventions miss.
pub(crate) mod testmarker;
// `pub(crate)`: the framework pass (resolve::framework, S-012) builds the
// canonical symbols of its promoted route/component nodes with the same
// builder extraction uses, so promoted identities follow ADR-07 like every
// other node's.
pub(crate) mod symbol;

/// SCIP descriptor-name escaping, shared with the annotation engine's
/// synthetic policy-node symbols (S-014, [FR-AN-03]) so layer names from
/// `rules.toml` always assemble into a valid symbol.
///
/// [FR-AN-03]: ../../../docs/specs/requirements/FR-AN-03.md
pub(crate) use symbol::escape_name;
pub use symbol::SymbolContext;

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use rayon::prelude::*;
use tree_sitter::{Node, Parser, Query, QueryCapture, QueryCursor, StreamingIterator};

use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeKind, ReceiverShape, RefForm};
use crate::plugin::{ImportSpecifier, LanguagePlugin, LanguageRegistry, ModuleModelKind, Semantics};
use crate::resolve::http_client_call::ClientCallRefusal;
use crate::resolve::package_key::PackageLayout;

use config::accessor::{BindingView, DeclaredTypes};
use config::binding::{PropertiesIndex, MEMBER_SCOPE};

use refs::{
    flatten_dotted_import, flatten_use_tree, from_module_segments, import_segments,
    is_relative_head, macro_call_refs, specifier_segments, split_path_text,
};
use symbol::{build_symbol, descriptor_family, descriptor_for, path_segments, DescriptorFamily};

/// The capture-name group prefix the `symbols` query uses (`@symbol.<kind>`).
/// The segment after it names a [`NodeKind`] by its [`NodeKind::as_str`] form.
const SYMBOL_CAPTURE_GROUP: &str = "symbol";

/// The `symbols`-query capture naming the **self type** of the declaration the
/// same match captures (S-493, [FR-RS-11]): the captured node's text is the base
/// type name, generics and path already left outside the capture by the query's
/// own pattern (`impl<M> crate::a::A<M>` → `A`; Go's `(s *A[T])` → `A`, S-509). A plugin opts in by adding the capture
/// to its query; no language is named here. Not a [`NodeKind`], so
/// [`kind_for_capture`] never mistakes it for a declaration.
///
/// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
const SELF_TYPE_CAPTURE: &str = "symbol.self_type";

/// The `references`-query capture naming a method call whose receiver is
/// **exactly the caller's own instance** — Rust's `self.m()` (S-493,
/// [FR-RS-11]). The captured node is the method name, as for `@ref.method`; a
/// query declaring it keeps its plain `@ref.method` pattern off such calls, so
/// one call is captured once. It is a `@ref.method` the receiver-shape pass
/// reads as marked `self` (S-514, `receiver`).
///
/// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
const SELF_RECEIVER_METHOD_CAPTURE: &str = "ref.method.self";

/// The `references`-query marker naming the module of a `from m import a`
/// (S-519, [FR-RS-14]): it records no row of its own, and each `@ref.import`
/// in its match is recorded under it — one row per imported name, a relative
/// module keeping its level ([`from_module_segments`]).
///
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
const FROM_MODULE_CAPTURE: &str = "ref.import.from";

/// The `references`-query capture naming the **local name** an import binds
/// (S-520): the `t` of Python's `import typing as t`, the `C` of PHP's
/// `use A\B as C`, the `Test` of C#'s `using Test = Xunit.FactAttribute`, the
/// `internalcloud` of Go's `internalcloud "…/internal/cloud"`. Its text is the
/// row's alias, replacing the path's last segment — the name a file can call
/// is the one it bound, never the one it imported. Like [`FROM_MODULE_CAPTURE`]
/// it records no row of its own, and it never reaches a wildcard row (whose
/// alias is a scope marker).
const IMPORT_ALIAS_CAPTURE: &str = "ref.import.alias";

/// One source file handed to the extractor.
#[derive(Debug, Clone)]
pub struct FileInput {
    /// Path relative to the project root, used both as the `files.path` key and
    /// as the leading namespace segments of every symbol from this file.
    pub path: String,
    /// The file's full source text.
    pub source: String,
}

impl FileInput {
    /// Construct a [`FileInput`] from a relative path and its source.
    pub fn new(path: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
        }
    }
}

/// Per-function quality metrics attached to a [`NodeFact`] ([FR-EX-03],
/// [FR-EX-04]), and the has-body fact with its body's token count (S-500,
/// [FR-EX-11]).
///
/// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionMetrics {
    /// Cyclomatic complexity: `1 + decision points` (see [`complexity`]).
    pub cyclomatic_complexity: u32,
    /// Physical line span of the definition (`end_line - start_line + 1`).
    pub line_count: u32,
    /// `true` when the declaration carries an implementation body (S-500,
    /// [FR-EX-11]), from the language's declared `body_node_kinds` (see
    /// [`shape::callable_body`]). `false` for an abstract method, an interface
    /// method with no default, a C++ pure-virtual or prototype; always `true`
    /// for a language declaring no body kind. Read by duplicate eligibility,
    /// LCOM4 and the Focus method count.
    ///
    /// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
    pub has_body: bool,
    /// The normalized token count of that body ([`shingle::token_count`] over
    /// [`shape::callable_body`]). Where the declaration names a `body` field
    /// this is the very stream the near-clone shingles k-gram, so the
    /// exact-duplicate floor (`duplicate_min_tokens`, S-501) and
    /// `clone_min_tokens` are comparable; a Kotlin `function_body` or a TS
    /// declarator's arrow body is counted too, though shingles read neither.
    /// `0` when [`has_body`](Self::has_body) is `false`.
    pub body_tokens: u32,
}

/// A graph vertex produced by extraction, keyed by its canonical SCIP symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFact {
    /// The canonical-ordinal SCIP identity ([ADR-07]).
    pub symbol: LogosSymbol,
    /// The ontology kind.
    pub kind: NodeKind,
    /// The human-facing declared name (the FTS-indexed value).
    pub name: String,
    /// 1-based first line of the declaration.
    pub start_line: u32,
    /// 1-based last line of the declaration.
    pub end_line: u32,
    /// Complexity, line count and the has-body fact (S-500), present for
    /// `Function`/`Method` nodes only.
    pub metrics: Option<FunctionMetrics>,
    /// `true` when the declaration carries a visibility modifier — the
    /// exported-is-live dead-code root set (S-014, [FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub exported: bool,
    /// The normalised AST-shape fingerprint duplicate detection groups by
    /// (S-014, [FR-AN-02]); `Function`/`Method` nodes only.
    ///
    /// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
    pub fingerprint: Option<String>,
    /// `true` when this function carries language-native test-marker evidence
    /// captured at extraction (S-027, [FR-EX-06]) — a Rust `#[test]`/`#[cfg(test)]`
    /// function, a Python `test_*`/`unittest` method, a TS/JS `it`/`describe`
    /// callee, a Go `TestXxx` in `*_test.go`, a Java `@Test` method. `false` for
    /// non-callables and for any plugin without test detection (absence ≠ error,
    /// [NFR-MA-01]). One input to the unified `is_test` annotation (S-028,
    /// [FR-AN-05]); never inferred from call relationships ([ADR-18]).
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    /// [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
    pub test_evidence: bool,
    /// The FTS-indexed body prose, for `DocSection` nodes only ([FR-DG-05],
    /// S-037): the section's own content beneath its heading, excluding nested
    /// sub-sections. `None` for code nodes, the synthetic file module, and the
    /// `DocFile` root — those carry no searchable body.
    ///
    /// [FR-DG-05]: ../../../docs/specs/requirements/FR-DG-05.md
    pub body: Option<String>,
    /// The per-function maximum block-structure nesting depth (CR-005,
    /// [FR-EX-07]); `Function`/`Method` nodes only, `None` otherwise. Depth 0 is
    /// a flat body. Computed from the language's declarative `nesting_block_kinds`
    /// (see [`nesting`]); the input to the Nesting ([FR-QM-09]) and Conciseness
    /// ([FR-QM-10]) dimensions.
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    pub max_nesting_depth: Option<u32>,
    /// The winnowed near-clone shingle fingerprint set (CR-005, [FR-EX-09]);
    /// `Function`/`Method` nodes only, empty otherwise and for a body below the
    /// token floor. A rename-invariant set the near-clone clustering pass
    /// ([FR-AN-06], S-043) reads for Jaccard similarity (see [`shingle`]).
    /// Distinct from the exact AST-shape [`fingerprint`](Self::fingerprint).
    ///
    /// [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
    pub shingles: Vec<u64>,
    /// The base type name of the type this declaration is a method of, when its
    /// plugin's `symbols` query declares one with a `@symbol.self_type` capture
    /// in the same match (S-493, [FR-RS-11]) — a Rust impl method's
    /// `impl<..> T<..>` / `impl Trait for T` → `T`, a Go method's receiver
    /// `func (s *T[K]) M()` → `T` (S-509). `None` for every other node.
    /// Recorded beside the symbol, never in it ([ADR-07]): the binder reads it
    /// to bind a `self.m()` / `Self::m()` call through the caller's own type.
    ///
    /// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
    /// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
    pub self_type: Option<String>,
}

/// A graph relationship produced by extraction.
///
/// Pass 1 emits only the bound, intra-file [`EdgeKind::Contains`] edge (lexical
/// nesting: a scope to the declarations it encloses). Call/import edges — whose
/// targets need cross-file resolution — are the resolution engine's concern
/// (S-011) and are not produced here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeFact {
    /// The enclosing scope's symbol.
    pub source: LogosSymbol,
    /// The enclosed declaration's symbol.
    pub target: LogosSymbol,
    /// The relationship kind.
    pub kind: EdgeKind,
}

/// An *outgoing reference* produced by extraction (S-011) — a call path, a
/// receiver-method call, or a `use` import whose target is **not** resolved
/// here.
///
/// Pass 1 records what a file points at, verbatim; the pipeline persists these
/// into the `unresolved_refs` ledger and the resolution engine (Pass 2) binds
/// each one by the scope-hierarchy rules — or leaves it honestly unresolved
/// ([NFR-RA-05], never fabricate).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefFact {
    /// The referencing declaration's symbol: the innermost enclosing captured
    /// declaration, or the file-module symbol for file-scope references.
    pub source: LogosSymbol,
    /// The reference target text, interpreted per `form` (a `::`-joined path,
    /// a method name).
    pub target: String,
    /// The in-scope name an import binds (`use a::b as c` → `c`); `None` for
    /// calls and globs.
    pub alias: Option<String>,
    /// The reference shape ([`RefForm`]).
    pub form: RefForm,
    /// The edge kind a successful binding produces.
    pub kind: EdgeKind,
    /// 1-based source line of the reference.
    pub line: u32,
    /// The cross-artifact relation class (CR-011, [FR-CG-07]) when this is an
    /// `ArtifactRef`/`ArtifactBinding` reference captured by the config extraction
    /// walk; `None` for every code/doc/access reference. Its
    /// [`as_str`](ArtifactRelation::as_str) token is persisted as the ledger row's
    /// payload, labels the bound edge, and keys per-relation-class coverage.
    ///
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    pub relation: Option<ArtifactRelation>,
    /// The receiver shape of a Method-form call (S-514, [FR-EX-13]) — set by
    /// the receiver pass (`extract::receiver`) from the query's `@ref.receiver.*`
    /// markers; `None` for every other row, and for a call no marker names. Part
    /// of the ledger identity: `this.m()` and `x.m()` from one caller are two
    /// rows that bind differently.
    ///
    /// [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
    pub receiver: Option<ReceiverShape>,
}

/// The extraction result for a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The file's project-relative path (echoes [`FileInput::path`]).
    pub path: String,
    /// The grammar/plugin name that parsed the file (e.g. `rust`).
    pub language: String,
    /// `true` when the parse tree contained a syntax error and extraction was
    /// therefore partial ([FR-IX-04]).
    pub partial: bool,
    /// Extracted graph vertices, sorted by `(start_line, symbol)`.
    pub nodes: Vec<NodeFact>,
    /// Extracted graph relationships, sorted by `(source, target, kind)`.
    pub edges: Vec<EdgeFact>,
    /// Outgoing references for the resolution pass (S-011), sorted by
    /// `(source, target, form, kind)` and deduplicated.
    pub refs: Vec<RefFact>,
    /// Non-fatal diagnostics (incompatible grammar, symbol-build failure, …).
    pub warnings: Vec<String>,
    /// The committed-configuration facts this file proves (S-380, [FR-WS-19]):
    /// its profile and its canonical key → value pairs, flattened at full
    /// nesting depth from the source text this pass already read.
    ///
    /// `None` for every file that is not a configuration source — which is every
    /// file in a member with no configuration corpus, so such a member writes no
    /// corpus row and is byte-for-byte unaffected. Produced by
    /// [`config::corpus::source_facts`] on the artifact-extraction path only.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    pub config_source: Option<config::corpus::ConfigSourceFact>,
    /// The broker publish/subscribe sites of this file whose topic operand is a
    /// bare parameter of the method enclosing it, and what the two-frame wrapper
    /// hop decided about each (S-417, [FR-WS-26]).
    ///
    /// Every entry names a site this pass has **already reported refused**
    /// (`topic-not-literal`); the hop can only retract that refusal, never add
    /// one. Each carries [`broker::ForwardingCandidate::outcome`], which is
    /// [`None`] on the single-file [`extract`] entry point — one file cannot
    /// answer a cross-file question, and saying so beats reporting a refusal
    /// that was never tested.
    ///
    /// [FR-WS-26]: ../../../docs/specs/requirements/FR-WS-26.md
    pub forwarding: Vec<broker::ForwardingCandidate>,
    /// The top-level types this file declares under their package-aware
    /// fully-qualified names, or refused (S-472, [CR-152] §3.2 B).
    ///
    /// Empty for every file whose language is neither package-shaped nor keyed
    /// by its declared namespace (S-518) — which is every file of a member with
    /// no Java, Kotlin, PHP, C# or Scala source, so such a member writes no
    /// declared-type row and is byte-for-byte unaffected. Produced by
    /// [`declared_types::source_types`] on the code-extraction path only.
    ///
    /// [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
    pub declared_types: Vec<declared_types::SourceType>,
    /// The namespace or package this file declares, for a language keyed by it
    /// (S-518, [FR-RS-13]): in
    /// [`namespace_text`](crate::resolve::package_key::namespace_text) form,
    /// `Some("")` for the global namespace. `None` for every file of any other
    /// module model, and for one whose top-level declarations sit in two
    /// different namespaces ([`declared_types::file_namespace`]) — such a file
    /// keeps the default module key.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    pub namespace: Option<String>,
}

/// One captured declaration, retained with its tree-sitter node for the metrics
/// pass. Lives only for the duration of one [`extract_one`] call (it borrows the
/// parse tree).
struct Decl<'tree> {
    node: Node<'tree>,
    kind: NodeKind,
    name: String,
    start_byte: usize,
    start_line: u32,
    end_line: u32,
    /// Index of the nearest enclosing captured declaration, or `None` at file
    /// scope. Resolved in [`assign_parents`].
    parent: Option<usize>,
    /// Ordinal among same-name siblings of one [`DescriptorFamily`], in
    /// canonical sort order. Assigned in [`assign_ordinals`].
    ordinal: u32,
    /// The self type a `@symbol.self_type` capture in a match naming this
    /// declaration gives it (S-493). Never part of the symbol or the ordinal.
    self_type: Option<String>,
}

/// Extract one file with an explicit plugin, allocating a fresh parser.
///
/// This is the [extraction-engine]'s `extract(file, plugin) -> Facts` interface
/// ([extraction-engine]). [`extract_files`] is the parallel driver that reuses a
/// per-worker parser instead of allocating one per call.
///
/// [extraction-engine]: ../../../docs/specs/architecture/components/extraction-engine.md
pub fn extract(input: &FileInput, plugin: &dyn LanguagePlugin, ctx: &SymbolContext) -> Facts {
    let mut parser = Parser::new();
    // One file cannot answer a cross-file question, so the single-file interface
    // is given an index that knows no class and therefore binds no accessor
    // (S-397). That is not a degradation of this entry point: an accessor's
    // owning class is declared in another file by construction, so a per-file
    // caller never had the evidence. [`extract_files`] builds the real index
    // once, over the set the pass is about to read.
    extract_one(
        &mut parser,
        input,
        plugin,
        ctx,
        &PropertiesIndex::default(),
        &PackageLayout::from_plugin(plugin),
    )
}

/// Extract many files in parallel, one [`tree_sitter::Parser`] per rayon worker.
///
/// Files whose extension resolves to no loaded grammar are skipped (the
/// discovery layer, S-010, is responsible for filtering); the returned vector
/// preserves the input order of the files that *were* extracted, so the output
/// is deterministic regardless of the thread count ([NFR-RA-06], [NFR-PE-08]).
///
/// # The configuration-binding pre-pass (S-397, [CR-122], [FR-WS-19])
///
/// A `@ConfigurationProperties` accessor names a class declared in **another**
/// file, so the invocation arm cannot resolve one from the file it is looking
/// at. The member's properties index is therefore built here, once, before the
/// parallel map — over `inputs`, the text this pass is about to read anyway, so
/// ingestion opens **no file of its own** ([FR-WS-19] AC7). A member declaring
/// no bound class produces an empty index, which every consumer short-circuits
/// on, leaving such a member byte-for-byte unaffected.
///
/// It is a *pre-pass* rather than a second phase over the extracted facts
/// because the resolution needs the parse tree of the **use site**, which exists
/// only inside `extract_one`. What it costs is one extra parse per file whose
/// text mentions a binding annotation — 69 files of 7,895 on the reference
/// estate — and nothing at all for the rest, which are rejected on a substring
/// test.
///
/// ## The index spans `inputs`, which on an incremental sync is the dirty set
///
/// Stated rather than left to be discovered. A full walk passes every admitted
/// file, so every bound class is present and every accessor that can resolve
/// does. An **incremental** sync passes only the files it re-extracts, so a
/// caller re-extracted without its properties class sees an index that does not
/// hold it, and its site records no key — reverting to the keyless
/// runtime-composed row it carried before this hop existed, until the next full
/// walk restores it.
///
/// That is the conservative direction and not a correctness hole: the failure
/// mode is *losing* evidence, never inventing it, which is the side of
/// [NFR-RA-05] this whole chain sits on. Closing it needs the index to outlive
/// one pass — a stored artefact, or a re-read of the unchanged files — and a
/// re-read is exactly the file IO [FR-WS-19] AC7 forbids, so it is not done here.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
///
/// [CR-122]: ../../../docs/requests/CR-122-the-configuration-substrate-reaches-the-product.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
pub fn extract_files(
    inputs: &[FileInput],
    registry: &LanguageRegistry,
    ctx: &SymbolContext,
) -> Vec<Facts> {
    let properties = PropertiesIndex::from_sources(
        registry,
        inputs.iter().map(|i| (i.path.as_str(), i.source.as_str())),
    );
    // The one package-aware layout (S-465) the binder keys files by, so a
    // declared type is named exactly as an import of it binds (S-472).
    let layout = PackageLayout::from_registry(registry);
    let mut facts: Vec<Facts> = inputs
        .par_iter()
        // `map_init` runs the init closure once per rayon worker thread, so each
        // worker owns exactly one Parser — the AR-05 mitigation — and reuses it
        // across the files that worker handles.
        .map_init(Parser::new, |parser, input| {
            let plugin = plugin_for(registry, &input.path)?;
            Some(extract_one(parser, input, plugin, ctx, &properties, &layout))
        })
        // `rayon`'s `collect` preserves input order even through this
        // `Option`-flattening, so the result is deterministic (NFR-RA-06).
        .flatten()
        .collect();
    // The two-frame wrapper hop (S-417, [FR-WS-26]) is a **post**-pass, not a
    // pre-pass, and that is the one structural difference from the binding index
    // above: it needs the `Calls` ledger the map has just produced to know which
    // files to re-read, where the binding index needs only the text. It is
    // sequential and it reads a handful of files, so it is not inside the
    // parallel map; a workspace whose broker sites carry no parameter operand
    // returns from it on one `is_empty` test.
    broker::resolve_forwarded_topics(&mut facts, inputs, registry, &properties);
    facts
}

/// Resolve the plugin for a file by its **extension or claimed basename**, or
/// `None` if unsupported. Basename claiming (S-062, [CR-010], [FR-CG-01]) is what
/// lets an extensionless artifact (`Dockerfile`, `Makefile`) reach extraction; a
/// code/doc file still resolves by extension exactly as before.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
fn plugin_for<'r>(registry: &'r LanguageRegistry, path: &str) -> Option<&'r dyn LanguagePlugin> {
    registry.for_path(path)
}

/// The core single-file extraction, reusing the caller's parser.
fn extract_one(
    parser: &mut Parser,
    input: &FileInput,
    plugin: &dyn LanguagePlugin,
    ctx: &SymbolContext,
    properties: &PropertiesIndex,
    layout: &PackageLayout,
) -> Facts {
    // A documentation grammar (S-033, CR-003) is extracted structurally into a
    // DocFile + nested DocSection tree, not via the code `symbols` query. This
    // is the single dispatch point; discovery/config decide *which* files reach
    // here (S-034) — until then no `.md` file is in the default discovery globs,
    // so this branch is exercised only by direct callers and tests.
    if plugin.is_documentation() {
        return doc::extract_one_doc(parser, input, plugin, ctx);
    }

    // An artifact grammar (S-062, CR-010, ADR-25) is extracted structurally into
    // a ConfigFile + depth-bounded ConfigSection tree (+ per-format typed anchors
    // layered on by the format stories), not via the code `symbols` query — the
    // third plugin class beside code and documentation. Discovery/config decide
    // *which* files reach here (the config-layer toggle + globs).
    if plugin.is_artifact() {
        return config::extract_one_config(parser, input, plugin, ctx);
    }

    let mut facts = Facts {
        path: input.path.clone(),
        language: plugin.name().to_string(),
        partial: false,
        nodes: Vec::new(),
        edges: Vec::new(),
        refs: Vec::new(),
        warnings: Vec::new(),
        config_source: None,
        forwarding: Vec::new(),
        declared_types: Vec::new(),
        namespace: None,
    };

    // A grammar that fails to bind (ABI skew) is skipped-and-warned, never fatal.
    if parser.set_language(plugin.language()).is_err() {
        facts.warnings.push(format!(
            "grammar '{}' failed to bind; file skipped",
            plugin.name()
        ));
        return facts;
    }

    let Some(tree) = parser.parse(&input.source, None) else {
        facts
            .warnings
            .push("parser returned no tree; file skipped".to_string());
        return facts;
    };

    // Error-tolerant: a syntax error localises to ERROR nodes; the well-formed
    // declarations around it are still extracted (FR-IX-04). Its index is kept
    // so step 1 can count the declarations the damage cost into it (FR-EX-30).
    let partial_warning = tree.root_node().has_error().then(|| {
        facts.partial = true;
        facts
            .warnings
            .push("syntax error(s) present; partial extraction".to_string());
        facts.warnings.len() - 1
    });

    let Some(query) = plugin.query("symbols") else {
        // No symbols capability → nothing to extract, but not an error.
        return facts;
    };

    let source = input.source.as_bytes();

    // 1) Collect declarations from the query matches.
    let (mut decls, package_statement, namespaces, damage) = collect_decls(query, tree.root_node(), source);
    if let Some(i) = partial_warning {
        damage.annotate(&mut facts.warnings[i]);
    }

    // 2) Resolve parent scopes and 3) assign canonical-sort ordinals.
    assign_parents(&mut decls);
    assign_ordinals(&mut decls);

    // The namespace the file declares (S-518, FR-RS-13), for a language keyed by
    // it: the one its top-level declarations share. That namespace IS such a
    // file's package, so its declared types are named by it and never refused
    // as disagreeing with the directory.
    let package: Option<String>;
    let file_layout: PackageLayout;
    let layout = if plugin.semantics().module_model == ModuleModelKind::Namespace {
        let top_level: Vec<usize> = decls
            .iter()
            .filter(|d| d.parent.is_none())
            .map(|d| d.start_byte)
            .collect();
        facts.namespace = declared_types::file_namespace(&namespaces, &top_level, source.len());
        package = facts.namespace.clone();
        file_layout = layout
            .clone()
            .with_declared_namespaces(facts.namespace.clone().map(|ns| (input.path.clone(), ns)));
        &file_layout
    } else {
        package = package_statement;
        layout
    };

    // 4) Build a symbol per declaration; a build failure skips that node only.
    // Empty and `.` components (a `./`-prefixed or doubled-slash path) are
    // dropped so they cannot become junk namespace segments.
    let path_segments: Vec<&str> = path_segments(&input.path);
    let symbols: Vec<Option<LogosSymbol>> = (0..decls.len())
        .map(|i| {
            let chain = scope_chain(&decls, i);
            match build_symbol(ctx, &path_segments, &chain) {
                Ok(sym) => Some(sym),
                Err(err) => {
                    facts.warnings.push(format!(
                        "could not build symbol for '{}' ({}): {err}",
                        decls[i].name,
                        decls[i].kind.as_str()
                    ));
                    None
                }
            }
        })
        .collect();

    // 5) Synthesize the per-file Module node (S-011). It gives file-scope
    // references a source endpoint and gives module imports a bindable target
    // ([FR-RS-01] — "a cross-module import binds to the target module node").
    // Deliberately built OUTSIDE the decl/ordinal machinery: it never joins a
    // scope chain, so every pre-existing symbol ID is byte-for-byte unchanged
    // ([ADR-07] stability).
    //
    // [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
    // [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
    let file_module: Option<LogosSymbol> = match build_symbol(ctx, &path_segments, &[]) {
        Ok(sym) => {
            facts.nodes.push(NodeFact {
                symbol: sym.clone(),
                kind: NodeKind::Module,
                name: file_module_name(&path_segments, &plugin.semantics().package_stems),
                start_line: 1,
                end_line: input.source.lines().count().max(1) as u32,
                metrics: None,
                // The synthetic file module is bookkeeping, not a declaration:
                // it is never a dead-code candidate nor an exported root, and
                // carries no test-marker evidence (S-027 — evidence is per
                // function only).
                exported: false,
                fingerprint: None,
                test_evidence: false,
                // Code/module nodes carry no FTS body — only DocSection prose is
                // body-indexed (FR-DG-05).
                body: None,
                // The synthetic file module is not a function: no nesting depth,
                // no shingles (CR-005).
                max_nesting_depth: None,
                shingles: Vec::new(),
                self_type: None,
            });
            Some(sym)
        }
        Err(err) => {
            facts
                .warnings
                .push(format!("could not build the file-module symbol: {err}"));
            None
        }
    };

    // 6) Emit node facts (with per-function metrics) and Contains edges.
    let keywords = &plugin.semantics().complexity_keywords;
    let block_kinds = &plugin.semantics().nesting_block_kinds;
    let body_kinds = &plugin.semantics().body_node_kinds;
    let export_convention = plugin.semantics().export_convention;
    let test_convention = plugin.semantics().test_convention;
    // Trait-implementation reference rows (S-281, CR-073, FR-RS-08): one per
    // `impl T for X` method, linking the impl method to its trait so the binder
    // can enumerate a trait method's impls for `dyn T` fan-out. Collected here
    // (the AST is in hand) and appended to the reference set after the query walk.
    let mut impl_refs: Vec<RefFact> = Vec::new();
    for (i, decl) in decls.iter().enumerate() {
        let Some(symbol) = &symbols[i] else {
            continue;
        };
        // Re-kind a Rust `impl`-nested associated function as `Method` (CR-068
        // Part B, FR-EX-05): emission-only, so `decl.kind` — and thus every
        // symbol ID and ordinal — is byte-identical (NFR-RA-06). Free functions
        // and every other kind pass through unchanged.
        let is_rust_method = decl.kind == NodeKind::Function && is_rust_associated_method(decl.node);
        let node_kind = if is_rust_method {
            NodeKind::Method
        } else {
            decl.kind
        };
        // An `impl T for X` (trait-impl) method emits an Implements ref to its
        // trait `T`; an inherent `impl X` method carries no trait and emits none
        // (S-281, CR-073). Never fabricated — resolution binds it only to the one
        // workspace Trait of that name, or leaves it an honest miss (NFR-RA-05).
        if is_rust_method {
            if let Some(trait_name) = rust_impl_trait_name(decl.node, source) {
                impl_refs.push(RefFact {
                    source: symbol.clone(),
                    target: trait_name,
                    alias: None,
                    form: RefForm::Path,
                    kind: EdgeKind::Implements,
                    line: decl.start_line,
                    relation: None,
                    receiver: None,
                });
            }
        }
        let is_callable = matches!(node_kind, NodeKind::Function | NodeKind::Method);
        let metrics = is_callable.then(|| function_metrics(decl, keywords, body_kinds));
        facts.nodes.push(NodeFact {
            symbol: symbol.clone(),
            kind: node_kind,
            name: decl.name.clone(),
            start_line: decl.start_line,
            end_line: decl.end_line,
            metrics,
            // The S-014 annotation inputs, captured while the AST is in hand.
            exported: shape::is_exported(decl.node, &decl.name, export_convention),
            fingerprint: is_callable.then(|| shape::shape_fingerprint(decl.node, &facts.language)),
            // S-027 / FR-EX-06: language-native test-marker evidence, captured
            // in the same AST-in-hand pass. `test_evidence` itself gates on a
            // callable kind (`Function`/`Method` alike), so it is `false` for
            // every non-function node.
            test_evidence: testmarker::test_evidence(
                decl.node,
                &decl.name,
                node_kind,
                &input.path,
                test_convention,
                source,
            ),
            // Code declarations carry no FTS body (FR-DG-05 indexes DocSection
            // prose only); the code itself is read on demand, not searched here.
            body: None,
            // CR-005 structural facts, captured in the same AST-in-hand pass and
            // gated on a callable kind: the max block-nesting depth (FR-EX-07)
            // and the winnowed near-clone shingle set (FR-EX-09).
            max_nesting_depth: is_callable
                .then(|| nesting::max_nesting_depth(decl.node, block_kinds)),
            shingles: if is_callable {
                shingle::shingles(decl.node)
            } else {
                Vec::new()
            },
            self_type: decl.self_type.clone(),
        });

        // A Contains edge links the enclosing scope to this declaration; both
        // endpoints must have a built symbol (the current `symbol` does). A
        // top-level declaration is contained by the file-module node (S-011).
        if let Some(parent_idx) = decl.parent {
            if let Some(parent_symbol) = &symbols[parent_idx] {
                facts.edges.push(EdgeFact {
                    source: parent_symbol.clone(),
                    target: symbol.clone(),
                    kind: EdgeKind::Contains,
                });
            }
        } else if let Some(file_module) = &file_module {
            facts.edges.push(EdgeFact {
                source: file_module.clone(),
                target: symbol.clone(),
                kind: EdgeKind::Contains,
            });
        }
    }

    // The file's top-level types, read off the nodes and edges just emitted (S-472).
    facts.declared_types = declared_types::source_types(&facts, package.as_deref(), layout);

    // 7) Collect outgoing references (S-011) — calls, method calls, imports.
    // A grammar without the `references` capability simply produces none.
    if let Some(ref_query) = plugin.query("references") {
        facts.refs = collect_refs(
            ref_query,
            tree.root_node(),
            source,
            &decls,
            &symbols,
            file_module.as_ref(),
            plugin.semantics(),
        );
    }

    // Fold in the trait-implementation refs (S-281) collected above, then restore
    // the canonical `(source, target, form, kind)` ledger order and dedup — the
    // same key `collect_refs` sorts on, so the merged set stays byte-identical
    // regardless of collection order ([NFR-RA-06]).
    if !impl_refs.is_empty() {
        facts.refs.append(&mut impl_refs);
        dedup_sort_refs(&mut facts.refs);
    }

    // 8) HTTP client-call arm (S-252, CR-061, FR-WS-08).
    capture_http_client_call_arm(
        plugin,
        tree.root_node(),
        source,
        &decls,
        &symbols,
        file_module.as_ref(),
        properties,
        &mut facts,
    );

    // 9) Message-broker publish/subscribe invocation arm (S-254, FR-WS-10).
    capture_broker_invocation_arm(
        plugin,
        tree.root_node(),
        source,
        &decls,
        &symbols,
        file_module.as_ref(),
        properties,
        &mut facts,
    );

    #[cfg(debug_assertions)]
    debug_assert_unique_symbols(&facts);

    sort_facts(&mut facts);
    facts
}

/// Debug-only per-file symbol-uniqueness assertion (S-512, [ADR-07]): every node
/// one file emits must carry its own symbol. Two that share one are folded into
/// a single node by the store's `symbol_id` upsert, and the second `Contains`
/// edge to it then fails `UNIQUE(source, target, kind)` and aborts the whole
/// index, so a debug build (tests, dev) panics here — naming the file and the
/// symbol — rather than at persistence.
/// A release build never pays for it, mirroring the pipeline's
/// `debug_assert_structural_integrity`.
///
/// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
#[cfg(debug_assertions)]
fn debug_assert_unique_symbols(facts: &Facts) {
    let mut seen: HashSet<&str> = HashSet::with_capacity(facts.nodes.len());
    let duplicates: Vec<&str> = facts
        .nodes
        .iter()
        .map(|n| n.symbol.as_str())
        .filter(|s| !seen.insert(s))
        .collect();
    debug_assert!(
        duplicates.is_empty(),
        "{} emitted one symbol for two declarations (S-512, ADR-07): {duplicates:?}",
        facts.path
    );
}

/// HTTP client-call arm (S-252, CR-061, FR-WS-08): capture outbound calls via
/// the optional `invocations` capability and funnel them through the shared
/// S-251 interpreter with the `route_key`-based normalizer. A grammar without
/// the capability produces none; the interpreter's `push_artifact_ref`
/// choke-point applies the external gate and `HttpClientCall.edge_kind()`.
///
/// Ledger-gated candidacy (the consumer-side mirror of the framework pass's
/// FR-FW-04): the `.scm` anchor is a broad `<receiver>.<method>(<arg>)` shape,
/// so it is captured ONLY in a file whose reference ledger names one of the
/// client packages that language's own descriptor declares
/// (`http_client_detectors`, matched by [`crate::resolve::matches_detector`] —
/// the same prefix rule the provider side applies to `framework_detectors`).
/// Without this gate an incidental collection/registry call whose key looks
/// like a route (`perms.get("/admin/users")`, a route-table `.get("/health")`)
/// would be captured, normalize, and fabricate a cross-service edge — exactly
/// what never-fabricate forbids ([NFR-RA-05]).
///
/// The gate is **independent evidence only when every detector row is a name the
/// call shape itself cannot supply.** A detector is matched against a canonical
/// reference *target* (at a `::` segment boundary, per
/// [`crate::resolve::matches_detector`]), not against an import specifier
/// specifically — and for a **single-segment** row the boundary rule and a plain
/// name equality coincide. So a **bare-identifier** row also matches a plain-call
/// and — in a name-only reference dialect such as TypeScript's — a member-call
/// name. A row like `fetch` (S-343: a global, imported from nothing) therefore
/// makes the gate **non-independent**: the very call the query is about is the
/// ledger evidence that opens it, and a file declaring its own local `fetch`
/// self-satisfies it.
///
/// A language adding such a row owes the compensating scope in its own
/// `invocations.scm`: **every pattern anchored to a named client**, and it must
/// **not** ship the broad `<receiver>.<method>(<arg>)` anchor with nothing but
/// this gate behind it — with a tautological gate behind it, that anchor reopens the
/// CR-110 fabrication class (`formGroup.get("year")`, `cache.get("/cache/key")`)
/// with nothing standing behind it but the leading-`/` requirement and
/// `route_key` ([NFR-RA-05]).
///
/// **This paragraph is the single statement of that rule** — a descriptor row
/// records only which of *its own* entries sit in this position, and points
/// here rather than restating the argument (see `plugins/typescript/plugin.toml`).
///
/// This gate is **file-grained**, and that is all it can be: it reads the file's
/// reference ledger, so it answers "is this file plausibly a client file?" and
/// cannot answer [FR-WS-08] AC5's question about a single *call*. **The
/// per-call half belongs to each language's own `invocations.scm`**, and a
/// language shipping the broad `<receiver>.<method>(<arg>)` anchor owes a
/// narrowing there. Two mechanisms exist, both pure descriptor/query data:
///
/// 1. A **receiver-name boundary rule** in the query. Receiver *typing* is not
///    required — that impossibility claim is what S-375 ([CR-120]) retired when
///    it closed Java's copy of this residual; a name rule suffices.
/// 2. The `[invocation_methods]` table's **filter half**, which drops any method
///    name absent from the table, so a bare-verb collection call is refused
///    without any receiver rule at all.
///
/// Where an arm has neither, the file gate is the only thing behind it and the
/// same incidental call *inside* a genuine client file still captures. That
/// residual is the documented ADR-54 accuracy ceiling (see `HTTP_METHODS`).
/// Under-capture is safe; over-capture is not, so a language whose client
/// wrapper is undetected simply stays unbound.
///
/// **Which arm is in which state is deliberately not listed here.** A roster in
/// this position goes stale on every arm story — the one this paragraph replaced
/// omitted two languages and pointed at a per-language test name that three of
/// the arms it named do not have. Each `invocations.scm` header states its own
/// position and names its own pin; that is the single place to read it.
///
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
///
/// A descriptor declaring `invocations` with an empty detector set is refused at
/// parse time ([`crate::plugin::PluginManifest::validate`]) — otherwise the
/// capability would read present while this function returned early for ever.
///
/// See [FR-WS-08]'s "Shared negative-case fixture contract" section (S-340)
/// for the three negative cases every per-language `invocations.scm` story
/// (S-341..S-348) must fixture — defined there once, not restated here, so
/// the two can't drift. Cases 2-3 (base-url-runtime / path-not-composed) are
/// already generic in [`resolve::http_client_call::classify_client_call`](crate::resolve::http_client_call::classify_client_call); a
/// language story's own fixtures only need to prove its query populates the
/// `invoke.http.method` / `invoke.http.arg` capture names correctly
/// ([`collect_invocation_sites`]) — never re-implement the classification.
/// Case 1 (a same-shaped non-HTTP receiver call) is per-language, gated by this
/// function's ledger check above against the descriptor's
/// `http_client_detectors` rows **and**, for a language that scopes its
/// receivers, by that query's own receiver rule — which is what makes the case
/// hold *within* a client file rather than only across files.
///
/// # A declined call is recorded, not silent (S-374, [CR-120], [FR-WS-08] AC2)
///
/// [FR-WS-08] AC2 requires a refused path to emit no reference **and** appear
/// under a runtime-composition coverage reason. The first half shipped with
/// S-252; the second did not, because the normalizer's refusal reason was
/// discarded by `render_client_call_target`'s `.ok()` and the shared interpreter
/// then skipped the site — so a declined call left no reference, no ledger row
/// and no coverage entry, and an estate whose client paths are all composed at
/// runtime read exactly like one with no outbound calls at all. That is the
/// sparsity-indistinguishable-from-absence dishonesty [NFR-CC-04] forbids, and
/// it is a conformance failure rather than a gap.
///
/// This function now judges each captured call once and hands the two halves to
/// the two arm-agnostic passes: the bound sites to
/// [`capture_invocation_refs`](crate::extract::config::capture_invocation_refs),
/// and the declined ones to
/// [`record_refusals`](crate::extract::config::record_refusals) — the **same**
/// recorder the broker arm has used since S-370, not a copy of it. A recorded
/// refusal is a keyless row: its target is empty, so it promotes no node, keys
/// no topic, creates no edge, and the [FR-WS-05] tier reports it under
/// `base-url-runtime` ([`UnboundReason::BaseUrlRuntime`](crate::federation::UnboundReason::BaseUrlRuntime),
/// which this path is the production producer of).
///
/// **The refusal site is the path OPERAND, not the whole call.** It is the
/// tightest range available — the broker arm's site is deliberately *wider* than
/// its operand so a sibling literal in the same annotation cancels the refusal,
/// but this arm's query yields one operand per match and decides bind-vs-refuse
/// from that operand alone, so a wider range would only reach further. Two
/// patterns matching one call still cancel correctly, because the admitted
/// literal's range lies inside the wider match's operand range — and the
/// cancellation is keyed on *the arm resolved a literal here*, not on *the
/// literal keyed*, so an inner `path-not-composed` literal cancels the outer
/// refusal too. Withholding that range would report a static absolute literal
/// as a runtime-composed path. The residual is
/// a client call nested *inside* another call's path argument
/// (`client.get(other.get("/x"))`): the inner literal would cancel the outer
/// refusal. No such shape exists in the reference workspace and none is
/// idiomatic; it is recorded here rather than guarded, because the guard would
/// be an exact-range rule that loses the two-pattern cancellation above.
///
/// ## Which refusals this records, and which stay invisible
///
/// Stated explicitly, because a coverage denominator that silently covers a
/// narrower population than its reason word suggests is the same dishonesty in a
/// new place ([NFR-CC-04]):
///
/// - **Recorded** — a call the arm's query matched whose path is not a static
///   absolute literal ([`ClientCallRefusal::BaseUrlRuntime`]): a bare variable,
///   a base-URL join, an interpolated/`format!` string, a builder lambda, a
///   helper-method return, a relative or absolute-URL literal. This is the
///   population [FR-WS-08] AC2 names and the one the reference-workspace figure
///   is measured over.
/// - **Not recorded — refused at QUERY-MATCH time.** A call the arm's `.scm`
///   never matched leaves no site for any pass to judge, so it emits no
///   reference *and* surfaces no reason. Every stated capture ceiling is in this
///   class: Java's verb-suffixed `RestTemplate` methods and `exchange`, OpenFeign
///   interfaces, a receiver a language's own receiver rule declines, a chained receiver,
///   a language shipping no `invocations` query at all. These are invisible by
///   construction and cannot be made visible by a refusal ledger — only by a
///   query that matches them. The narrowing is deliberate: recording a refusal
///   for a site the arm never recognised as a client call would manufacture a
///   coverage denominator out of ordinary code, the hazard [CR-120] §7 names.
/// - **Not recorded — [`ClientCallRefusal::PathNotComposed`].** A static absolute
///   literal that does not positionally normalize (a catch-all/regex/mixed
///   template) is still dropped without a row. A keyless row could not carry
///   this reason: the coverage tier distinguishes the two HTTP refusals by
///   whether the stored target is empty, so an empty row is `base-url-runtime`
///   by construction and a `path-not-composed` row would need a **non**-keyless
///   target — a different mechanism, with its own inertness proof and its own
///   measurement, and outside this story's acceptance criteria. On the language
///   the [CR-120] figure is measured over it costs nothing: S-355 recorded that
///   **0 of the estate's Java client-call arguments is a string literal of any
///   kind**, and this reason requires one. Workspace-wide the population is
///   **not separately measured** — it is bounded above by the gated sites that
///   produced no row (63 of 195), a gap dominated by the per-declaration dedup
///   rather than by this reason (Go alone collapsed 98 sites into 36 rows). The
///   bound is stated rather than the value, because the value is not known.
///   Both figures are S-374's reading, taken 2026-09-09 and **not** re-derived
///   since: S-402 made Go's candidacy receiver-grained and removed 26 of its 37
///   captured sites on the reference estate, so Go's share of the gated
///   population — and therefore the 63-of-195 bound built on it — is now an
///   upper bound on an upper bound. Anchored rather than re-measured, because
///   re-deriving it is a workspace measurement rather than a doc edit.
///
/// [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// The reading file's configuration-binding view, or [`None`] for a member that
/// declares no `@ConfigurationProperties` class (S-397, extended to the broker arm
/// by S-409).
///
/// One constructor rather than one per arm. The two arms had a byte-identical
/// five-field literal 140 lines apart in this file, which is exactly the
/// hand-mirrored twin `config::refs::record_refusals` was written to avoid — and
/// the arms differ in *when* they build it, never in *what* they build, so the
/// difference belongs at the call sites and the construction belongs here.
///
/// The `!properties.is_empty()` test is a **cost** guard: an index over no class
/// resolves nothing, so the answer is the same either way and what it saves is the
/// per-file [`DeclaredTypes::build`] AST walk.
fn accessor_binding<'a>(
    plugin: &'a dyn LanguagePlugin,
    root: Node<'_>,
    source: &[u8],
    properties: &'a PropertiesIndex,
) -> Option<BindingView<'a>> {
    (!properties.is_empty()).then(|| BindingView {
        index: properties,
        types: DeclaredTypes::build(root, source),
        language: plugin.name(),
        module: MEMBER_SCOPE,
    })
}

#[allow(clippy::too_many_arguments)]
fn capture_http_client_call_arm(
    plugin: &dyn LanguagePlugin,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
    properties: &PropertiesIndex,
    facts: &mut Facts,
) {
    let Some(inv_query) = plugin.query("invocations") else {
        return;
    };
    let detectors = &plugin.semantics().http_client_detectors;
    let is_http_client_file = !detectors.is_empty()
        && facts.refs.iter().any(|r| {
            detectors
                .iter()
                .any(|d| crate::resolve::matches_detector(&r.target, d))
        });
    if !is_http_client_file {
        return;
    }
    // The configuration-binding view of this file, built ONLY when the member
    // declares at least one bound class (S-397). An index over no class can
    // resolve nothing, so skipping the per-file declared-type walk there is the
    // whole of what keeps a member with no configuration corpus unaffected —
    // and it is the common case for every language and repository that ships no
    // `properties` capability at all.
    let binding = accessor_binding(plugin, root, source, properties);
    let calls = collect_invocation_sites(
        inv_query,
        root,
        source,
        decls,
        symbols,
        file_module,
        &plugin.semantics().invocation_methods,
        binding.as_ref(),
    );
    if calls.is_empty() {
        return;
    }

    // Partition the captured calls by the arm's own judgement, made ONCE here so
    // the bound half and the refused half cannot disagree about a site — the
    // reconcile below needs the operand range of everything that bound, and
    // deriving that from anything other than the judgement itself is how the two
    // would drift.
    //
    // The interpreter then re-renders the `Ok` sites through
    // `render_client_call_target` (`classify_client_call(..).ok()`), so each bound
    // site is classified twice. That is deliberate: the alternative is to inline a
    // second copy of the emission loop here, and `capture_invocation_refs` exists
    // precisely so every arm shares one. Passing only the `Ok` sites emits exactly
    // what passing all of them would — the interpreter skips a `None` render — and
    // the repeated work is three map lookups, two small string allocations and one
    // `route_key`, per **bound** call.
    //
    // `judged_operands` holds the operand range of every call whose path the arm
    // RESOLVED to a static literal — whether or not that literal went on to key.
    // Both outcomes must cancel a wider match's refusal, and the distinction is
    // load-bearing: a `path-not-composed` literal records nothing itself, but if
    // its range were withheld here, the enclosing match's `base-url-runtime`
    // candidate would survive and the site would be reported under the WRONG
    // reason — a static absolute literal filed as a runtime-composed path, the
    // classifier drift [NFR-CC-04] forbids. That shape is real: Java's and
    // Kotlin's `.uri(URI.create("/files/**"))` and Ruby's `get(URI("/files/**"))`
    // / `get(path: "/files/**")` are matched by two patterns each, the outer
    // seeing an expression and the inner unwrapping the literal.
    let mut bound_sites = Vec::with_capacity(calls.len());
    let mut judged_operands: Vec<(ArtifactRelation, std::ops::Range<usize>)> =
        Vec::with_capacity(calls.len());
    let mut refusals: Vec<crate::extract::config::RefusalCandidate> =
        Vec::with_capacity(calls.len());
    for call in calls {
        match crate::resolve::http_client_call::classify_client_call(&call.site.slots) {
            // Both admissions take this arm. A config-bound path (S-382) is
            // stored verbatim, placeholders and all, so the coverage tier
            // resolves it against the bytes the repository commits; it creates
            // no edge here, exactly as a literal that fails to key creates none.
            Ok(_) => {
                judged_operands.push((ArtifactRelation::HttpClientCall, call.operand));
                bound_sites.push(call.site);
            }
            Err(ClientCallRefusal::BaseUrlRuntime) => {
                refusals.push(crate::extract::config::RefusalCandidate {
                    relation: ArtifactRelation::HttpClientCall,
                    site: call.operand,
                    source: call.site.source,
                    line: call.site.line,
                });
            }
            // A static absolute literal that does not positionally normalize
            // records NOTHING, and that is a scoped decision rather than an
            // oversight — see this function's doc comment. It still contributes
            // its operand range, because the arm DID resolve a literal there and
            // a wider match's refusal must not outlive that judgement.
            Err(ClientCallRefusal::PathNotComposed) => {
                judged_operands.push((ArtifactRelation::HttpClientCall, call.operand));
            }
        }
    }

    crate::extract::config::capture_invocation_refs(
        facts,
        ArtifactRelation::HttpClientCall,
        RefForm::Path,
        bound_sites,
        crate::resolve::http_client_call::render_client_call_target,
    );
    // The refused half, through the SAME recorder the broker arm uses (S-370) —
    // one keyless row per declining declaration, reconciled against what the arm
    // resolved and
    // deduped per `(relation, declaration, line)`. The mechanism is documented
    // once, on `record_refusals`.
    crate::extract::config::record_refusals(
        facts,
        RefForm::Path,
        refusals,
        &judged_operands,
    );
    // Re-canonicalize: both passes appended to `facts.refs`.
    dedup_sort_refs(&mut facts.refs);
}

/// Message-broker publish/subscribe invocation arm (S-254, [FR-WS-10]): a code
/// arm captures its publish/subscribe sites through its own optional `brokers`
/// query and funnels them through the generic invocation interpreter. A
/// grammar without the capability contributes nothing. The site's source
/// symbol is its innermost enclosing declaration — the same attribution
/// `collect_refs` uses.
///
/// # The accessor hop, built once and now reaching both arms (S-409, [FR-WS-19])
///
/// The [`BindingView`] below is constructed by the **same function** as
/// [`capture_http_client_call_arm`]'s ([`accessor_binding`]), deliberately and not
/// by coincidence: a
/// topic operand that reads a `@ConfigurationProperties` getter is the identical
/// shape a request-path operand takes, and the HTTP arm resolved 81 of its 96
/// such sites while this arm refused every one of them `topic-not-literal`
/// ([CR-131] §3.2 A2). Nothing about the mechanism is broker-specific, so nothing
/// about it is restated here — read [`config::accessor::BindingView::placeholder_for`]
/// for the chain and [`broker::capture_broker_invocations`] for where the answer
/// is put.
///
/// The `!properties.is_empty()` guard is a **cost** guard, not a correctness one,
/// and the distinction is worth stating because the first draft of this comment
/// got it wrong. Correctness does not need it: an index over no class resolves
/// nothing, so `placeholder_for` returns [`None`] and the output is byte-identical
/// with the guard removed — a mutation confirmed that, which is why no test can
/// pin this line. What the guard buys is that a member declaring no
/// configuration-bound class does not pay the per-file `DeclaredTypes` walk, and
/// that is the common case for every repository shipping no `properties`
/// capability. "A member with no `@ConfigurationProperties` class is unaffected"
/// is pinned instead by the negative controls in
/// `extract::tests::an_accessor_topic_reaches_the_ledger_in_every_captured_broker_form`.
///
/// **What this arm does NOT share with its HTTP twin, stated because the two look
/// alike.** `capture_http_client_call_arm` builds its view *after* the
/// `is_http_client_file` ledger-detector gate, so only detector-matching files pay
/// the walk. This arm has no such gate — `brokers.scm` is not detector-gated — so
/// the view is built lazily instead, on the first refusal slot that needs it (see
/// [`broker::capture_broker_invocations`]). Without that laziness every file of a
/// `brokers`-capable language in a properties-bearing member would walk its own
/// AST to serve the handful that carry a broker site: on the reference estate, the
/// whole ~2,450-file Java corpus for 86 broker rows.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[allow(clippy::too_many_arguments)]
fn capture_broker_invocation_arm(
    plugin: &dyn LanguagePlugin,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
    properties: &PropertiesIndex,
    facts: &mut Facts,
) {
    let Some(broker_query) = plugin.query("brokers") else {
        return;
    };
    // Built at most ONCE, and only if a refusal slot actually asks — see this
    // function's doc comment for why this arm cannot afford the eager
    // construction its HTTP twin makes after a detector gate.
    let binding_cell: OnceCell<Option<BindingView<'_>>> = OnceCell::new();
    let binding = || -> Option<&BindingView<'_>> {
        binding_cell
            .get_or_init(|| accessor_binding(plugin, root, source, properties))
            .as_ref()
    };
    let id_to_idx: HashMap<usize, usize> =
        decls.iter().enumerate().map(|(i, d)| (d.node.id(), i)).collect();
    let enclosing = |node: Node<'_>| -> Option<LogosSymbol> {
        let mut ancestor = node.parent();
        while let Some(n) = ancestor {
            if let Some(&idx) = id_to_idx.get(&n.id()) {
                if let Some(sym) = &symbols[idx] {
                    return Some(sym.clone());
                }
            }
            ancestor = n.parent();
        }
        file_module.cloned()
    };
    if broker::capture_broker_invocations(
        broker_query,
        root,
        source,
        enclosing,
        binding,
        facts,
    ) > 0
    {
        // Broker refs are appended after the code-reference sort; restore the
        // canonical ledger order + dedup so the output stays byte-stable.
        dedup_sort_refs(&mut facts.refs);
    }
}

/// Sort a [`Facts`]'s nodes and edges into canonical order ([NFR-RA-06]): node
/// facts by `(start_line, symbol)` and edges by `(source, target, kind)`. The
/// symbol string is the tiebreaker so the order never depends on query-match or
/// traversal order. Shared by the code path ([`extract_one`]) and the
/// documentation path ([`doc::extract_one_doc`]) so the two can never disagree
/// on the byte-stable output ordering.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn sort_facts(facts: &mut Facts) {
    facts
        .nodes
        .sort_by(|a, b| (a.start_line, a.symbol.as_str()).cmp(&(b.start_line, b.symbol.as_str())));
    facts.edges.sort_by(|a, b| {
        (a.source.as_str(), a.target.as_str(), a.kind.as_i32()).cmp(&(
            b.source.as_str(),
            b.target.as_str(),
            b.kind.as_i32(),
        ))
    });
}

/// Deduplicate references on the ledger's uniqueness key
/// `(source, target, form, kind, relation, receiver)` — the same reference on
/// two lines is one ref, first wins — then sort into that canonical order
/// ([NFR-RA-06]).
///
/// The `relation` is part of the identity: two facts that share source, target,
/// form, and edge kind but carry **different** [`ArtifactRelation`]s are distinct
/// (they file distinct ledger rows and bind under distinct relation classes), so
/// collapsing them would silently drop one. This is load-bearing for the broker
/// invocation arm (S-254, [FR-WS-10]): a relay method that both *subscribes to*
/// and *publishes on* one topic emits a `BrokerSubscribe` and a `BrokerPublish`
/// that coincide on `(source, target=topic, Method, ArtifactRef)` and differ
/// only in relation — both must survive to the ledger or a real cross-service
/// fan-out edge is never produced. For every earlier relation (each capture site
/// files at most one relation per `(source, target, form, kind)`) the extra key
/// component is a no-op, so the byte-stable output is unchanged.
///
/// The receiver shape (S-514) is part of the identity for the same reason: a
/// caller's `this.m()` and `x.m()` share every other component and bind
/// differently. A row with no shape keys exactly as before.
///
/// Shared by the code [`collect_refs`] and the documentation extractor
/// ([`doc`], S-035) so both passes produce byte-identical, order-independent
/// ledger input.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
pub(super) fn dedup_sort_refs(refs: &mut Vec<RefFact>) {
    // The relation token (`None` for a plain code/doc reference) completes the
    // ledger identity; a `&'static str` keeps the key allocation-free.
    let relation_token =
        |r: &RefFact| -> Option<&'static str> { r.relation.map(crate::model::ArtifactRelation::as_str) };
    // `(source, target, form, kind, relation, receiver)`.
    type LedgerKey = (String, String, i32, i32, Option<&'static str>, Option<i32>);
    let mut seen: HashSet<LedgerKey> = HashSet::new();
    refs.retain(|r| {
        seen.insert((
            r.source.as_str().to_string(),
            r.target.clone(),
            r.form.as_i32(),
            r.kind.as_i32(),
            relation_token(r),
            r.receiver.map(ReceiverShape::as_i32),
        ))
    });
    refs.sort_by(|a, b| {
        (
            a.source.as_str(),
            &a.target,
            a.form.as_i32(),
            a.kind.as_i32(),
            relation_token(a),
            a.receiver.map(ReceiverShape::as_i32),
        )
            .cmp(&(
                b.source.as_str(),
                &b.target,
                b.form.as_i32(),
                b.kind.as_i32(),
                relation_token(b),
                b.receiver.map(ReceiverShape::as_i32),
            ))
    });
}

/// The human-facing name of a file's module node: the file stem, or — for a
/// package-file stem the file's plugin declares, which names its *enclosing*
/// module (Rust's `mod`/`lib`/`main`, Python's `__init__`; S-519) — the nearest
/// preceding path segment that is not `src`, falling back to `crate`. A stem no
/// plugin declares is the name: a JavaScript `main.js` is `main`.
///
/// Display-only: resolution computes real module paths independently, so this
/// name carries no binding semantics (it is what FTS search shows).
fn file_module_name(path_segments: &[&str], package_stems: &[String]) -> String {
    let stem = path_segments
        .last()
        .map(|s| {
            Path::new(s)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(s)
                .to_string()
        })
        .unwrap_or_default();
    if !stem.is_empty() && !package_stems.contains(&stem) {
        return stem;
    }
    path_segments
        .iter()
        .rev()
        .skip(1)
        .find(|s| **s != "src")
        .map_or_else(|| "crate".to_string(), |s| (*s).to_string())
}

/// Collect the file's outgoing references from the `references` query matches.
///
/// Each capture is attributed to its innermost enclosing captured declaration
/// (falling back to the file module for file-scope references), normalised via
/// [`split_path_text`] / [`flatten_use_tree`] — or, for an import in a language
/// whose `semantics` declare path specifiers, [`specifier_segments`] (S-439) —
/// deduplicated, and sorted into the canonical `(source, target, form, kind)`
/// order ([NFR-RA-06]). In such a language a call **through** an import is
/// recorded qualified by the module it names, `<import target>::<name>`, and a
/// JSX tag naming a local value records nothing ([`ImportBindings`], S-440). In
/// a language whose query marks its receivers (`@ref.receiver.*`), the receiver
/// pass ([`receiver`]) then rewrites what the markers prove: a call whose
/// receiver's type the file proves is recorded type-qualified, `T::<name>` in
/// Path form, in place of its bare row (S-467, Java), and every Method-form row
/// left bare records its receiver's shape (S-514, [FR-EX-13]).
///
/// A method call the query captures as [`SELF_RECEIVER_METHOD_CAPTURE`] is a
/// `self`-marked `@ref.method`: recorded as the Path-form `Self::m` when its
/// enclosing declaration has a recorded self type ([`Decl::self_type`], S-493)
/// — the same row a written `Self::m()` records, which the binder resolves
/// through that self type — and otherwise as a Method-form row of shape `self`.
///
/// [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
fn collect_refs(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
    semantics: &Semantics,
) -> Vec<RefFact> {
    let id_to_idx: HashMap<usize, usize> = decls
        .iter()
        .enumerate()
        .map(|(i, d)| (d.node.id(), i))
        .collect();
    // The innermost enclosing captured declaration whose symbol built, if any.
    // A declaration whose own symbol failed to build defers to the next
    // enclosing scope.
    let enclosing_decl = |node: Node<'_>| -> Option<usize> {
        let mut ancestor = node.parent();
        while let Some(n) = ancestor {
            if let Some(&idx) = id_to_idx.get(&n.id()) {
                if symbols[idx].is_some() {
                    return Some(idx);
                }
            }
            ancestor = n.parent();
        }
        None
    };
    // The symbol of that declaration, or the file module at file scope.
    let enclosing_symbol = |node: Node<'_>| -> Option<LogosSymbol> {
        match enclosing_decl(node) {
            Some(idx) => symbols[idx].clone(),
            None => file_module.cloned(),
        }
    };
    // The declarations a declared type belongs to, when its node sits beside
    // captured declarators rather than inside one — a Java field's type is a
    // sibling of its `variable_declarator`s (`private Dto a, b;`), so the
    // enclosing declaration would be the class, not the fields (S-466). `None`
    // for every other shape: the enclosing declaration is the owner.
    let declarator_symbols = |node: Node<'_>| -> Option<Vec<LogosSymbol>> {
        let parent = node.parent()?;
        let mut cursor = parent.walk();
        let owners: Vec<LogosSymbol> = parent
            .children_by_field_name("declarator", &mut cursor)
            .filter_map(|d| id_to_idx.get(&d.id()))
            .filter_map(|&idx| symbols[idx].clone())
            .collect();
        (!owners.is_empty()).then_some(owners)
    };

    let capture_names = query.capture_names();
    let imports = match semantics.import_specifier {
        ImportSpecifier::Path => ImportBindings::collect(query, root, source, decls, semantics),
        ImportSpecifier::Name => ImportBindings::default(),
    };
    // Receiver typing (S-467) and shapes (S-514) — only for a query that marks
    // receivers.
    let mut receivers = receiver::Receivers::for_query(capture_names, semantics.implicit_receiver);
    let mut out: Vec<RefFact> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let node = cap.node;
            let capture = capture_names[cap.index as usize];
            // A `_`-prefixed capture is a predicate operand (`@_receiver`),
            // never a reference.
            if capture.starts_with('_') {
                continue;
            }
            // A receiver marker records no row of its own: it is read before
            // the per-row source lookup below (`self_name` asks only for its
            // enclosing declaration).
            if receiver::is_marker(capture) {
                if let Some(receivers) = receivers.as_mut() {
                    receivers.mark(capture, node, source, || enclosing_decl(node));
                }
                continue;
            }
            let Some(source_symbol) = enclosing_symbol(node) else {
                continue; // no attributable scope (file-module symbol failed)
            };
            let line = node.start_position().row as u32 + 1;
            let Ok(text) = node.utf8_text(source) else {
                continue; // non-UTF-8 slice — skip defensively
            };
            match capture {
                "ref.call" => {
                    let segments = split_path_text(text);
                    if segments.is_empty() {
                        continue;
                    }
                    let target = segments.join("::");
                    // A JSX tag naming a local value (`const Icon = icons[k];
                    // <Icon />`) renders that value: it calls no component
                    // (S-440), so it records no reference at all.
                    let jsx = node.parent().is_some_and(|p| p.kind().starts_with("jsx_"));
                    if jsx && imports.locally_bound(node, &target) {
                        continue;
                    }
                    // A call through a named import is recorded qualified by the
                    // module it was imported from (S-440, `ImportBindings`).
                    let target = imports
                        .named_target(node, &target)
                        .map_or(target.clone(), str::to_string);
                    if let Some(receivers) = receivers.as_mut() {
                        receivers.site(out.len(), node.parent(), enclosing_decl(node));
                    }
                    out.push(RefFact {
                        source: source_symbol,
                        target,
                        alias: None,
                        form: RefForm::Path,
                        kind: EdgeKind::Calls,
                        line,
                        relation: None,
                        receiver: None,
                    });
                }
                "ref.method" | SELF_RECEIVER_METHOD_CAPTURE => {
                    let name = text.trim();
                    if name.is_empty() {
                        continue;
                    }
                    // `@ref.method.self` is a `self`-marked `@ref.method` (S-514).
                    let self_marked = capture == SELF_RECEIVER_METHOD_CAPTURE;
                    if let (Some(receivers), true) = (receivers.as_mut(), self_marked) {
                        receivers.mark_self(node.parent());
                    }
                    // A member call whose receiver is an imported module — a Go
                    // package, a TS namespace import — is a qualified path, not a
                    // receiver-method call (S-440, `ImportBindings`).
                    if let Some(module) = imports.qualifier_of(node, source) {
                        out.push(RefFact {
                            source: source_symbol,
                            target: format!("{module}::{name}"),
                            alias: None,
                            form: RefForm::Path,
                            kind: EdgeKind::Calls,
                            line,
                            relation: None,
                            receiver: None,
                        });
                        continue;
                    }
                    // Trait-object dynamic dispatch (S-281, CR-073, FR-RS-08): when
                    // the receiver is a *provable* `&dyn T` (an explicit parameter
                    // or `let` type in the enclosing fn), qualify the target as
                    // `T::f` so the binder fans out to the trait method's impls. A
                    // receiver of unknown type stays the bare `f`, bound by its
                    // receiver's shape (S-514, FR-RS-12).
                    let target = match rust_dyn_receiver_trait(node, source) {
                        Some(trait_name) => format!("{trait_name}::{name}"),
                        None => {
                            // Typed or shaped by its receiver after the walk
                            // (S-467, S-514, `receiver`) — never a Method-form `::`.
                            if let Some(receivers) = receivers.as_mut() {
                                receivers.site(out.len(), node.parent(), enclosing_decl(node));
                            }
                            name.to_string()
                        }
                    };
                    out.push(RefFact {
                        source: source_symbol,
                        target,
                        alias: None,
                        form: RefForm::Method,
                        kind: EdgeKind::Calls,
                        line,
                        relation: None,
                        receiver: None,
                    });
                }
                // The language-agnostic import capture (S-015): the captured
                // node's *text* is one import specifier — a Python dotted name,
                // a Go/TS quoted module string, a Java scoped identifier. Which
                // grammar that text is written in is the descriptor's
                // declaration, not a guess from the text (S-439): a path
                // specifier canonicalises by path rules, a name by the
                // member-path rules. The Rust grammar keeps `ref.use` below
                // because its use-trees (groups, renames, globs) need a
                // structural walk no text split can express.
                //
                // A match that also carries `ref.import.asterisk` wrote the
                // specifier before a wildcard (a Java `import a.b.*`, CR-149),
                // or names a namespace whose types all come into view (a C#
                // `using A.B;`, S-518): it names a scope whose members come
                // into view, not one declaration, so it is a `Glob` row and
                // introduces no alias — `b` is not a name the file can now use.
                // `ref.import.global` marks one whose scope is every file of
                // the declaring file's directory (a C# `global using`). A match
                // carrying `ref.import.from` names its module apart from the
                // imported name (Python's `from m import a`, S-519).
                "ref.import" => {
                    let marked = |name: &str| {
                        m.captures
                            .iter()
                            .any(|c| capture_names[c.index as usize] == name)
                    };
                    let Some((segments, form, alias)) =
                        import_row(m.captures, capture_names, source, text, semantics)
                    else {
                        continue;
                    };
                    if let (Some(receivers), RefForm::Path, Some(name)) =
                        (receivers.as_mut(), form, alias.as_deref())
                    {
                        if !marked("ref.import.static") {
                            receivers.type_import(name);
                        }
                    }
                    out.push(RefFact {
                        source: source_symbol,
                        alias,
                        target: segments.join("::"),
                        form,
                        kind: EdgeKind::Imports,
                        line,
                        relation: None,
                        receiver: None,
                    });
                }
                // A member-access fact (S-042, CR-005, FR-EX-08): a method body
                // reads a field of its own class-like container (`self.x`,
                // `this.x`). The captured node is the field-name token; the
                // receiver-anchored capture pattern in `references.scm` already
                // restricted it to an own-field access. Resolution binds it to an
                // `Accesses` edge only when exactly one Field candidate matches in
                // the enclosing container — ambiguous/unmatched stays in the
                // ledger (NFR-RA-05). The `Method` form carries the bare-name
                // semantics the binder's member-access path expects.
                "ref.access" => {
                    let name = text.trim();
                    if name.is_empty() {
                        continue;
                    }
                    out.push(RefFact {
                        source: source_symbol,
                        target: name.to_string(),
                        alias: None,
                        form: RefForm::Method,
                        kind: EdgeKind::Accesses,
                        line,
                        relation: None,
                        receiver: None,
                    });
                }
                // Calls nested inside a macro invocation's token tree (S-162,
                // CR-043): tree-sitter does not parse a macro body as
                // expressions, so the call/method-call query patterns cannot
                // match inside it. Walk the token tree in code and emit the same
                // `Calls` path/method RefFacts, attributed to the macro's
                // enclosing declaration — so a callee whose only call site is a
                // macro argument (`format!("{x}", x = activity_card(s))`,
                // `self.state.chip_class()`) is bound, or stays honestly
                // unresolved, exactly like any other call ([NFR-RA-05]).
                "ref.macro" => {
                    let caller = enclosing_decl(node).map(|i| &decls[i]);
                    for call in macro_call_refs(node, source) {
                        if call.target.is_empty() {
                            continue;
                        }
                        // `self.f()` in a macro argument: the row the same call
                        // records outside one (S-493, S-514). Any other method
                        // call is `other`, as outside one (S-517): a shapeless
                        // row would no longer merge with the query's `other`
                        // row of the same call, which the shape-keyed dedup
                        // keeps apart.
                        let (target, form, receiver) = if call.self_receiver {
                            receiver::self_call(caller, &call.target)
                        } else {
                            let shape = (call.form == RefForm::Method).then_some(ReceiverShape::Other);
                            (call.target, call.form, shape)
                        };
                        out.push(RefFact {
                            source: source_symbol.clone(),
                            target,
                            alias: None,
                            form,
                            kind: EdgeKind::Calls,
                            line: call.line,
                            relation: None,
                            receiver,
                        });
                    }
                }
                // A type relation (S-466, CR-149 §3.2 B, FR-EX-10): the captured
                // node is a TYPE, recorded as a Path-form row of the capture's
                // kind — never Method form, whose `::` target the binder
                // reserves for trait-object dispatch (S-281). Its type
                // arguments are type uses of the same declaration(s).
                name @ ("ref.extends" | "ref.implements" | "ref.instantiates" | "ref.type_use") => {
                    let owners = declarator_symbols(node).unwrap_or_else(|| vec![source_symbol]);
                    out.extend(type_relation_rows(name, &owners, node, source, line));
                }
                // An import whose paths no single node spans (S-518; Scala's
                // `import_declaration`): the declaration is walked like a Rust
                // use-tree, one row per imported path.
                name @ ("ref.use" | "ref.import.dotted") => {
                    out.extend(tree_import_rows(name, node, source, &source_symbol, line));
                }
                _ => {} // a capture this pass does not consume
            }
        }
    }

    if let Some(receivers) = receivers {
        let file = receiver::FileDecls {
            root,
            source,
            decls,
            symbols,
            id_to_idx: &id_to_idx,
        };
        receivers.finish(&mut out, &file);
    }

    // Dedup on the ledger's uniqueness key, then canonical sort (NFR-RA-06).
    dedup_sort_refs(&mut out);
    out
}

/// The canonical `segments`, form and alias of one `@ref.import` match, or
/// `None` when its specifier canonicalises to nothing.
///
/// - `from m import a` (`ref.import.from`, S-519): the name is recorded under
///   its module, one row per imported name; a wildcard's row is the module
///   itself.
/// - An import that names its local binding (`ref.import.alias`, S-520) is
///   aliased by it, not by the last segment of the path it imports; a
///   wildcard's alias is a scope marker and never takes it.
fn import_row(
    captures: &[QueryCapture<'_>],
    capture_names: &[&str],
    source: &[u8],
    text: &str,
    semantics: &Semantics,
) -> Option<(Vec<String>, RefForm, Option<String>)> {
    let marked = |name: &str| captures.iter().any(|c| capture_names[c.index as usize] == name);
    let capture_text = |name: &str| {
        captures
            .iter()
            .find(|c| capture_names[c.index as usize] == name)
            .and_then(|c| c.node.utf8_text(source).ok())
    };
    let segments = match (capture_text(FROM_MODULE_CAPTURE), semantics.import_specifier) {
        (Some(module), _) => {
            let mut segments = from_module_segments(module);
            if !marked("ref.import.asterisk") {
                segments.extend(import_segments(text));
            }
            segments
        }
        (None, ImportSpecifier::Path) => specifier_segments(text, &semantics.specifier_extensions),
        (None, ImportSpecifier::Name) => import_segments(text),
    };
    if segments.is_empty() {
        return None;
    }
    let (form, alias) = import_form(&marked, &segments);
    let alias = match (form, capture_text(IMPORT_ALIAS_CAPTURE)) {
        (RefForm::Path, Some(local)) => Some(local.to_string()),
        _ => alias,
    };
    Some((segments, form, alias))
}

/// The form and alias of one `@ref.import` row, from the markers its match
/// carries (`marked`) and its canonical `segments`: a wildcard
/// (`ref.import.asterisk`) is a `Glob` row whose alias says what it brings in
/// — every static member (`ref.import.static`, the `*` alias
/// `STATIC_WILDCARD_ALIAS`) or, for a global one (`ref.import.global`, S-518),
/// the global marker — since the ledger has no other column for it; any other
/// import is a `Path` row aliased by its last segment (`import_row` replaces that
/// with the local name an `@ref.import.alias` capture names, S-520).
fn import_form(marked: &impl Fn(&str) -> bool, segments: &[String]) -> (RefForm, Option<String>) {
    if !marked("ref.import.asterisk") {
        return (RefForm::Path, segments.last().cloned());
    }
    let scope = if marked("ref.import.global") {
        Some(crate::resolve::GLOBAL_WILDCARD_ALIAS.to_string())
    } else {
        marked("ref.import.static").then(|| crate::resolve::STATIC_WILDCARD_ALIAS.to_string())
    };
    (RefForm::Glob, scope)
}

/// The `Imports` rows of an import whose paths a structural walk recovers —
/// Rust's use-tree (`capture` = `ref.use`) or Scala's `import_declaration`
/// (`ref.import.dotted`, S-518): one row per imported path, a glob a `Glob`
/// row with no alias.
fn tree_import_rows(
    capture: &str,
    node: Node<'_>,
    source: &[u8],
    source_symbol: &LogosSymbol,
    line: u32,
) -> Vec<RefFact> {
    let mut items = Vec::new();
    if capture == "ref.use" {
        flatten_use_tree(node, source, &mut items);
    } else {
        flatten_dotted_import(node, source, &mut items);
    }
    items
        .into_iter()
        .map(|item| {
            let (form, alias) = if item.glob {
                (RefForm::Glob, None)
            } else {
                (RefForm::Path, item.alias)
            };
            RefFact {
                source: source_symbol.clone(),
                target: item.path.join("::"),
                alias,
                form,
                kind: EdgeKind::Imports,
                line,
                relation: None,
                receiver: None,
            }
        })
        .collect()
}

/// The Path-form rows a type-relation `capture` (`ref.extends`,
/// `ref.implements`, `ref.instantiates`, `ref.type_use`) records for its type
/// `node`, once per declaration in `owners` ([`type_relation_targets`], S-466).
fn type_relation_rows(
    capture: &str,
    owners: &[LogosSymbol],
    node: Node<'_>,
    source: &[u8],
    line: u32,
) -> Vec<RefFact> {
    let head = match capture {
        "ref.extends" => EdgeKind::Extends,
        "ref.implements" => EdgeKind::Implements,
        "ref.instantiates" => EdgeKind::Instantiates,
        _ => EdgeKind::TypeUses,
    };
    let mut rows = Vec::new();
    for (kind, target) in type_relation_targets(node, source, head) {
        for owner in owners {
            rows.push(RefFact {
                source: owner.clone(),
                target: target.clone(),
                alias: None,
                form: RefForm::Path,
                kind,
                line,
                relation: None,
                receiver: None,
            });
        }
    }
    rows
}

/// The rows one captured **type** node records (S-466, [CR-149] §3.2 B): its
/// head type under `head` — `Base<T>` → `Base`, `a.b.C` → `a::b::C`, `Dto[]` →
/// `Dto` — then every type argument inside it, at any depth, as a
/// [`EdgeKind::TypeUses`] (`List<Map<K, Dto>>` → `Map`, `K`, `Dto`; a wildcard's
/// bound included).
///
/// A primitive or `void` names no type and records nothing, and neither does a
/// single name the enclosing declarations declare as a **type parameter** — `T`
/// in `class Box<T>` is a type variable, and binding it to a same-package class
/// `T` would fabricate the edge [NFR-RA-05] forbids. Java's `var` is not a type
/// name either. Every shape is read from the grammar's node kinds, which only
/// the Java query captures today; another grammar's type nodes that are not
/// among them record nothing rather than a guess.
///
/// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn type_relation_targets(node: Node<'_>, source: &[u8], head: EdgeKind) -> Vec<(EdgeKind, String)> {
    let parameters = type_parameters_in_scope(node, source);
    let mut out: Vec<(EdgeKind, String)> = Vec::new();
    let mut record = |kind: EdgeKind, ty: Node<'_>| {
        let Some(path) = type_path(ty, source) else {
            return;
        };
        if let [only] = path.as_slice() {
            if only == "var" || parameters.contains(only) {
                return;
            }
        }
        out.push((kind, path.join("::")));
    };
    record(head, node);
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        let mut cursor = n.walk();
        let children: Vec<Node<'_>> = n.named_children(&mut cursor).collect();
        if n.kind() == "type_arguments" {
            for arg in &children {
                // `? extends Dto` — the bound is the type the argument names.
                let ty = if arg.kind() == "wildcard" {
                    let mut c = arg.walk();
                    arg.named_children(&mut c).last()
                } else {
                    Some(*arg)
                };
                if let Some(ty) = ty {
                    record(EdgeKind::TypeUses, ty);
                }
            }
        }
        stack.extend(children);
    }
    out
}

/// The `::`-joined path a Java type node names, generics and array dimensions
/// stripped, or `None` for a node that names no class-like type (a primitive,
/// `void`, an annotation). See [`type_relation_targets`].
fn type_path(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let is_annotation = |n: &Node<'_>| matches!(n.kind(), "annotation" | "marker_annotation");
    let mut cursor = node.walk();
    let named: Vec<Node<'_>> = node
        .named_children(&mut cursor)
        .filter(|n| !is_annotation(n))
        .collect();
    match node.kind() {
        "type_identifier" => Some(vec![node.utf8_text(source).ok()?.trim().to_string()]),
        // `a.b.C`, `Outer<A>.Inner`: the qualifier, then the last identifier.
        "scoped_type_identifier" => {
            let (last, qualifier) = named.split_last()?;
            if last.kind() != "type_identifier" {
                return None;
            }
            let mut path = type_path(*qualifier.first()?, source)?;
            path.push(last.utf8_text(source).ok()?.trim().to_string());
            Some(path)
        }
        "generic_type" | "annotated_type" => named
            .iter()
            .find(|n| n.kind() != "type_arguments")
            .and_then(|n| type_path(*n, source)),
        "array_type" => type_path(node.child_by_field_name("element")?, source),
        _ => None,
    }
}

/// Every type-parameter name the declarations enclosing `node` (itself
/// included) declare — `T` of `class Box<T>`, `E` of `<E> E get(E e)`.
fn type_parameters_in_scope(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut at = Some(node);
    while let Some(n) = at {
        if let Some(parameters) = n.child_by_field_name("type_parameters") {
            let mut cursor = parameters.walk();
            for parameter in parameters.named_children(&mut cursor) {
                let mut inner = parameter.walk();
                let name = parameter
                    .named_children(&mut inner)
                    .find(|c| c.kind() == "type_identifier");
                if let Some(text) = name.and_then(|c| c.utf8_text(source).ok()) {
                    names.insert(text.trim().to_string());
                }
            }
        }
        at = n.parent();
    }
    names
}

/// The superclass a class declares, as its `Extends` row records it (`Base`,
/// `a::b::Base`), read from the file's own reference facts — the per-file
/// accessor S-467's `super.m()` receiver shape consumes, so the type it
/// qualifies a `super` call with is the one this capture recorded and never a
/// second reading of the source (S-466, [CR-150]).
///
/// `None` when `class` records no `Extends` row, and when it records several
/// (an interface's super-interfaces) — a `super` receiver names one type or
/// none ([NFR-RA-05]).
///
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn declared_superclass<'a>(refs: &'a [RefFact], class: &LogosSymbol) -> Option<&'a str> {
    let mut supers = refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Extends && r.form == RefForm::Path && &r.source == class);
    let first = supers.next()?;
    supers.next().is_none().then_some(first.target.as_str())
}

/// The HTTP verbs the client-call arm (S-252, [FR-WS-08]) captures. A method call
/// whose name is not one of these is not an outbound HTTP call.
///
/// This name filter is **necessary but not sufficient**: a collection/registry
/// method that shares a verb name (`HashMap::get`) still passes it, and a
/// `/`-prefixed string key (`map.get("/health")`) would normalize and fabricate a
/// cross-service edge. Three further guards make the capture honest
/// ([NFR-RA-05]): the file-level HTTP-client-crate gate in `extract_one` (only a
/// file that uses a client crate is a candidate at all), **the receiver rule in
/// the language's own `invocations.scm`** where it ships one, and the arm's
/// normalizer (which refuses any non-static / non-absolute / non-normalizable
/// path).
///
/// The residual ceiling — a genuine HTTP-client file that also does an
/// incidental `/`-keyed collection `.get` — survives for any arm whose query
/// narrows neither its receiver nor its verb vocabulary. There it is a
/// documented accuracy ceiling ([ADR-54]), reported unbound-or-not at worst,
/// never silently guessed. An arm that closes it does so in query or descriptor
/// data and says so in its own header; see the per-call half of the rule on
/// [`capture_http_client_call_arm`] for the two mechanisms.
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
const HTTP_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// `true` if `name` (case-insensitively) is one of the [`HTTP_METHODS`].
fn is_http_method(name: &str) -> bool {
    HTTP_METHODS
        .iter()
        .any(|m| name.eq_ignore_ascii_case(m))
}

/// Resolve a captured `@invoke.http.method` text through the plugin descriptor's
/// `[invocation_methods]` table (S-346, [CR-108], [FR-WS-08]).
///
/// The table plays the **same two roles** as `[framework_methods]` on the
/// provider side — normalizer and recognised-token filter — and the lookup is
/// the same plain, case-sensitive exact match (`resolve::framework`'s
/// `methods.get(text.trim())`). One deliberate difference: an **empty** table
/// here is a pass-through, whereas an empty `framework_methods` promotes
/// nothing. The pass-through is what every language whose verbs are already
/// bare relies on (Rust, Go, Java, TypeScript), so their arms are untouched by
/// this mechanism.
///
/// The filter half is what lets C# ship a `<receiver>.<method>(<arg>)` anchor
/// safely — see `plugins/c-sharp/queries/invocations.scm`, which states that
/// argument once for the language that owns it.
///
/// [CR-108]: ../../../docs/requests/CR-108-per-language-http-client-call-capture.md
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
fn normalize_invocation_method<'a>(
    table: &'a std::collections::BTreeMap<String, String>,
    text: &'a str,
) -> Option<&'a str> {
    if table.is_empty() {
        return Some(text);
    }
    table.get(text).map(String::as_str)
}

/// The **static** content of a string-literal node, or `None` when the argument
/// is not a static literal (a bare variable, a `format!`, a concatenation) or is
/// an interpolated/templated string with runtime substitutions ([NFR-RA-05]).
///
/// Grammar-agnostic: a literal node's kind contains `"string"` across the
/// supported grammars (Rust `string_literal`/`raw_string_literal`, Python
/// `string`, JS/TS `string`/`template_string`, Go `*_string_literal`). Its static
/// text is the concatenation of its literal-content children; **any** other child
/// (an interpolation / substitution / expansion) makes the whole literal dynamic,
/// so the arm refuses it rather than guessing a target. A node that exposes no
/// content children (e.g. Rust's raw string, or C#'s `verbatim_string_literal`)
/// falls back to unwrapping its quotes.
///
/// The content-child kind names below are where a grammar that names them
/// differently gets added (S-345). Go 0.25 calls them
/// `interpreted_string_literal_content` / `raw_string_literal_content`; C# 0.23
/// calls them `string_literal_content` / `raw_string_content`, and additionally
/// spells a raw literal's `"""` fences as the named `raw_string_start` /
/// `raw_string_end` — carried in the skip arm, since a delimiter is neither
/// content nor evidence of dynamism ([S-346] added all four). A grammar whose
/// names are missing from the list does not merely lose the literal — its every
/// literal reads as *dynamic*, so the whole language's arm silently captures
/// nothing. Pass the **literal** node, never a content child: an
/// already-unquoted content child takes the no-children fallback below, whose
/// quote/`#` trimming would corrupt a path ending in one of those characters.
///
/// **Accepted [NFR-MA-01] debt, not the intended end state.** This union is a
/// flat, language-agnostic list — never a `match language {…}` — so it does not
/// reintroduce the per-language branch S-341 deleted. But it does mean each
/// remaining CR-108 language costs a `logos-core` edit, which the descriptor
/// contract exists to avoid: `nesting_block_kinds` already carries exactly this
/// data class declaratively, and `http_client_detectors` is the second such
/// field. Lifting these names into a descriptor field beside them is the right
/// end state; it is deliberately NOT done here, because it is an
/// every-plugin change that belongs to a CR rather than to an integration
/// session porting two arms.
///
/// **[S-346] restates the debt rather than paying it**, and the reason is that
/// paying it here would have made it *worse*: C# needed four kind names, one of
/// which (`raw_string_start`) is a *delimiter to skip*, not content. A single
/// descriptor list of "content kinds" cannot express that third state, so the
/// declarative field this paragraph proposes would need to be two fields — a
/// shape worth designing against every grammar at once in its own CR, not
/// inferred from the second language to need it. The budget line stays open, and
/// the flat union above is still a union, never a `match language {…}`.
///
/// One latent note for whoever pays it: `raw_string_content` is also a
/// tree-sitter-cpp node kind. That is inert today — C++ ships no `invocations`
/// capability, and this function has one caller — but a future C++ arm would
/// also meet `raw_string_delimiter`, which falls to the dynamic arm, so its
/// raw-string paths would read as composed until that name is added too.
///
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
/// [S-346]: ../../../docs/planning/journal.md#s-346-c-http-client-call-capture
fn static_string_literal(node: Node<'_>, source: &[u8]) -> Option<String> {
    if !node.kind().contains("string") {
        return None;
    }
    let mut content = String::new();
    let mut saw_child = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        saw_child = true;
        match child.kind() {
            // Static literal-content fragments and escapes across grammars.
            "string_content"
            | "string_fragment"
            | "escape_sequence"
            | "interpreted_string_literal_content"
            | "raw_string_literal_content"
            // C# 0.23 (S-346).
            | "string_literal_content"
            | "raw_string_content" => {
                content.push_str(child.utf8_text(source).ok()?);
            }
            // Python 0.21+'s PEP-701 grammar rewrite names the surrounding
            // quote/prefix tokens as their own NAMED children (`string_start`
            // `f"`/`r"`/`"""`, `string_end` `"`/`"""`) rather than leaving them
            // anonymous like every other supported grammar's `string` node
            // (S-344). They carry no literal content, so skip them without
            // disqualifying the literal — an f-string's `interpolation` child
            // is what disqualifies it, via the catch-all arm below.
            "string_start" | "string_end" => {}
            // Named delimiter tokens that carry no content: C# spells the
            // `"""` fences of a raw string literal as named nodes, so they must
            // be skipped rather than either concatenated (they would corrupt
            // the path) or treated as dynamic (S-346).
            "raw_string_start" | "raw_string_end" => {}
            // An interpolation / template substitution / expansion → dynamic.
            _ => return None,
        }
    }
    if !saw_child {
        // A literal whose grammar exposes no content node (e.g. Rust's raw
        // string, or C#'s `verbatim_string_literal` — `@"/users"`, S-346):
        // strip the literal-prefix characters, then the quotes.
        let raw = node.utf8_text(source).ok()?;
        let body = raw.trim_start_matches(['r', 'b', '#', '@']);
        // A body that opens and closes on the SAME quote is unwrapped by exactly
        // one character each side, so a path whose last character happens to be
        // a delimiter survives (`@"/tag/#"` is `/tag/#`, not `/tag/`). The
        // greedy trim below is kept only for the multi-character fences it was
        // written for (Rust's `r#"…"#`, where the two ends differ), which cannot
        // be unwrapped one-for-one (S-346).
        let first = body.chars().next();
        let unwrapped = match (first, body.chars().next_back()) {
            (Some(open @ ('"' | '\'' | '`')), Some(close))
                if open == close && body.chars().count() >= 2 =>
            {
                &body[open.len_utf8()..body.len() - close.len_utf8()]
            }
            _ => body.trim_matches(['"', '\'', '`', '#']),
        };
        content.push_str(unwrapped);
    }
    let content = content.trim().to_string();
    (!content.is_empty()).then_some(content)
}

/// The capture-name prefix whose **suffix is the HTTP verb** —
/// `@invoke.http.method.get` declares "this shape is a GET" (S-343).
///
/// The escape hatch for a call shape that spells no verb anywhere in its source
/// and therefore has no node for [`collect_invocation_sites`] to read one from:
/// a bare `fetch("/users")` is a `GET` by the WHATWG Fetch standard, not by
/// inference. Because the verb rides the capture name it is stated *in the
/// plugin's own `.scm`*, per pattern, visible in review — never a core-side
/// default silently applied to every verb-less match ([NFR-RA-05]). It is
/// gated by [`is_http_method`] like any other verb, and an explicit
/// `@invoke.http.method` node always outranks it.
///
/// This is **not** the mechanism for a verb whose text is merely non-canonical
/// (a C# `GetAsync`, S-346): there the verb *is* in the source and wants a
/// text normalizer, not a per-pattern constant.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
const DECLARED_METHOD_PREFIX: &str = "invoke.http.method.";

/// Collect the file's outbound **HTTP client-call** invocation sites from the
/// `invocations` query matches (S-252, [FR-WS-08], [ADR-54]).
///
/// The code-side twin of [`collect_refs`] for the pluggable invocation-arm
/// contract. The dispatch is **receiver-agnostic**: it reads only the capture
/// names, never the shape around them, so a *free-function* anchor
/// (`fetch("/users", {method: "POST"})`, S-343) is carried by exactly the same
/// code as the receiver-method anchor Rust's query uses (`client.get("/p")`) —
/// the finding S-344 (Python `requests.get` / `httpx.request`) consumes rather
/// than re-derives. A site is emitted only when the method is an HTTP verb
/// ([`is_http_method`]), read from the `@invoke.http.method` node or, for a
/// shape that spells none, declared by a [`DECLARED_METHOD_PREFIX`] capture
/// name. A verb read from a node first passes through the descriptor's
/// `[invocation_methods]` table ([`normalize_invocation_method`], S-346), which
/// is how a non-canonical spelling (`GetAsync`, `HttpMethod.Get`) reaches
/// [`is_http_method`] at all — and, for a language that declares one, how a
/// non-client spelling (`MapGet`) is filtered out before it can. The first
/// argument becomes the arm's slots — a static string literal
/// fills the `path` slot ([`PATH_SLOT`](crate::resolve::http_client_call::PATH_SLOT));
/// a `@ConfigurationProperties` accessor `binding` resolves fills the same slot
/// with the `${…}` placeholder naming its canonical key (S-397, [FR-WS-19]), so
/// it is admitted and resolved exactly as a source-written placeholder is; and
/// any other shape sets the dynamic-path marker
/// ([`DYNAMIC_PATH_SLOT`](crate::resolve::http_client_call::DYNAMIC_PATH_SLOT)) so
/// the arm's normalizer refuses it as base-url-runtime. A shape whose path is
/// composed in a fluent URI-**composer** chain instead of passed directly binds
/// no operand in its own match — the chain is arbitrarily long and a pattern's
/// nesting is not — and reaches the same three outcomes through
/// [`composer::Composers`], which reconciles the query's per-link matches by
/// byte range and hands back the composed operand only when every link the chain
/// carries beside the path one is provably path-neutral (S-405, [CR-129]).
/// The sites are funnelled
/// through the shared [`capture_invocation_refs`](crate::extract::config::capture_invocation_refs)
/// interpreter by the caller; no reference is emitted here, and no judgment of
/// bind-ability is made — that is the normalizer's job ([NFR-RA-05]).
///
/// [CR-129]: ../../../docs/requests/CR-129-path-neutral-composer-link-in-a-uribuilder-lambda.md
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[allow(clippy::too_many_arguments)]
fn collect_invocation_sites(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
    invocation_methods: &std::collections::BTreeMap<String, String>,
    binding: Option<&BindingView<'_>>,
) -> Vec<CapturedCall> {
    use crate::resolve::http_client_call::{DYNAMIC_PATH_SLOT, METHOD_SLOT, PATH_SLOT};

    let id_to_idx: HashMap<usize, usize> = decls
        .iter()
        .enumerate()
        .map(|(i, d)| (d.node.id(), i))
        .collect();
    // The innermost enclosing captured declaration (the call site's owner), or
    // the file module at file scope — the same attribution `collect_refs` uses.
    let enclosing_symbol = |node: Node<'_>| -> Option<LogosSymbol> {
        let mut ancestor = node.parent();
        while let Some(n) = ancestor {
            if let Some(&idx) = id_to_idx.get(&n.id()) {
                if let Some(sym) = &symbols[idx] {
                    return Some(sym.clone());
                }
            }
            ancestor = n.parent();
        }
        file_module.cloned()
    };

    // The composer vocabulary this file's query matched (S-405, CR-129) — empty,
    // and unwalked, for a query declaring none. Collected before the site loop
    // because a composer's links are reconciled across MATCHES: the chain is
    // arbitrarily long, so no single pattern can hold both the call's verb and
    // the operand buried in it.
    let composers = composer::Composers::collect(query, root, source);

    let capture_names = query.capture_names();
    let mut sites = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        let mut method_node = None;
        let mut declared_method = None;
        let mut arg_node = None;
        let mut composer_node = None;
        let mut composer_param = None;
        for cap in m.captures {
            let name = capture_names[cap.index as usize];
            match name {
                "invoke.http.method" => method_node = Some(cap.node),
                "invoke.http.arg" => arg_node = Some(cap.node),
                composer::COMPOSER => composer_node = Some(cap.node),
                composer::COMPOSER_PARAM => composer_param = Some(cap.node),
                // `@invoke.http.method.<verb>` — the verb declared by the
                // capture name for a shape that spells no verb in its source.
                // The node is kept alongside the verb purely for attribution
                // (see the `anchor` below); the verb itself comes from the name.
                // First declaration wins: a droppable on-disk query (FR-PL-04)
                // could bind two conflicting verbs to one match, and resolving
                // that by capture order would make the verb depend on node
                // position. First-wins is deterministic and inspectable.
                _ => {
                    if let Some(verb) = name.strip_prefix(DECLARED_METHOD_PREFIX) {
                        declared_method.get_or_insert((verb, cap.node));
                    }
                }
            }
        }
        // A site binds its operand directly, or — for a composer match — through
        // the reconciliation, which hands back the operand the chain composes
        // its path from ONLY when every other link in it is provably
        // path-neutral. A composer it declines yields no site here at all, so
        // the call keeps the wider match's `base-url-runtime` candidate and
        // records exactly the row it recorded before (S-405, CR-129).
        let Some(arg_node) = composers.site_operand(arg_node, composer_node, composer_param, source)
        else {
            continue;
        };
        // A verb read from the source always wins over a name-declared one, so a
        // query that binds both can never downgrade a spelled-out `POST` to the
        // shape's declared default ([NFR-RA-05]).
        let method = match method_node {
            Some(node) => {
                let Ok(text) = node.utf8_text(source) else {
                    continue;
                };
                // Resolve the source spelling through the descriptor's
                // `[invocation_methods]` table (S-346). Empty table = the text
                // itself; non-empty = normalizer AND filter, so an unlisted
                // spelling (`MapGet`, a bare `Get`) is dropped here.
                let Some(method) = normalize_invocation_method(invocation_methods, text.trim())
                else {
                    continue;
                };
                method
            }
            None => {
                let Some((verb, _)) = declared_method else {
                    continue;
                };
                verb
            }
        };
        // Narrow the broad method-call anchor to HTTP verbs (a map/collection
        // `.get(...)` is not an outbound call). A name-declared verb passes the
        // same gate, so a typo'd capture name captures nothing rather than
        // inventing a method.
        if !is_http_method(method) {
            continue;
        }
        // Attribute to the node the verb came from, so a site's reported line is
        // the CALL's line for every shape. A name-declared verb still has a node
        // — the capture that carried the name (typically the callee itself) —
        // and using it keeps a verb-less `fetch(\n  "/p"\n)` reported on the
        // `fetch` line rather than on the wrapped argument's, which is where
        // every method-bearing shape reports. The path argument is the last
        // resort, for a hypothetical query that binds a declared verb to nothing
        // but the argument itself.
        let anchor = method_node
            .or(declared_method.map(|(_, node)| node))
            .unwrap_or(arg_node);
        let Some(source_symbol) = enclosing_symbol(anchor) else {
            continue; // no attributable scope
        };
        let line = anchor.start_position().row as u32 + 1;

        let mut slots = std::collections::BTreeMap::new();
        slots.insert(METHOD_SLOT.to_string(), method.to_string());
        match static_string_literal(arg_node, source) {
            // A static string literal → the `"METHOD /template"` path candidate.
            Some(path) => {
                slots.insert(PATH_SLOT.to_string(), path);
            }
            // Not a literal — but a `@ConfigurationProperties` accessor names a
            // key the repository commits, so it is asked before the path is
            // called runtime-composed (S-397, FR-WS-19). The answer is already a
            // `${…}` placeholder, which is not a re-spelling of the operand but
            // the point of the story: a placeholder is the form S-382's
            // resolution already consumes, so the accessor reaches it by the
            // same path, through the same `ConfigBound` admission, the same
            // committed-corpus lookup, the same overlay handling and the same
            // `config-bound` provenance — no second rule anywhere.
            //
            // The key inside it is CANONICAL — relaxed binding applied, so
            // `mailserver.api.uriGetArchive` and a yaml's
            // `mailserver.api.uri-get-archive` store as one string. That is the
            // form the resolution matches on and the form the row's own
            // provenance already carries (`ConfigBound::key`), so the stored
            // target names the key the same way every surface downstream does.
            // The cost, stated: the target is not the source's spelling, so it
            // is not greppable in the yaml — the provenance's defining sources
            // are what name the file. `accessor::placeholder` owns both the
            // spelling and the check that the reader reads it back.
            None => match binding.and_then(|b| b.placeholder_for(arg_node, source)) {
                Some(placeholder) => {
                    slots.insert(PATH_SLOT.to_string(), placeholder);
                }
                // Anything else → a runtime-composed path; the marker's presence
                // makes the normalizer refuse it (base-url-runtime), no target
                // guessed. An accessor the source does not prove lands here too,
                // indistinguishable from a site that was never a candidate —
                // this arm gains no refusal of its own.
                None => {
                    let raw = arg_node
                        .utf8_text(source)
                        .ok()
                        .map(|t| t.trim().to_string())
                        .unwrap_or_default();
                    slots.insert(DYNAMIC_PATH_SLOT.to_string(), raw);
                }
            },
        }
        sites.push(CapturedCall {
            site: crate::extract::config::InvocationSite {
                source: source_symbol,
                slots,
                line,
            },
            operand: arg_node.byte_range(),
        });
    }
    sites
}

/// One captured client call: the site the shared interpreter normalizes, plus
/// the byte range of the **path operand** the arm's refusal reconcile keys on
/// (S-374, [CR-120]).
///
/// The range is carried alongside the site rather than inside
/// [`InvocationSite`](crate::extract::config::InvocationSite) deliberately: that
/// carrier is the arm-agnostic contract every invocation arm fills, and only
/// this arm derives its refusal site from the operand node. Widening the shared
/// struct for one arm's bookkeeping would put a field there that the broker arm
/// must fill with a value it never reads.
struct CapturedCall {
    /// The language-neutral slots the shared interpreter judges.
    site: crate::extract::config::InvocationSite,
    /// The `@invoke.http.arg` node's byte range — this arm's refusal **site**
    /// grain, and the range an admitted path literal occupies when the call
    /// binds. See [`capture_http_client_call_arm`] for why the operand rather
    /// than the whole call is the grain.
    operand: std::ops::Range<usize>,
}

/// The [`FunctionMetrics`] of one callable declaration, captured while its AST
/// is in hand: complexity and line count ([FR-EX-03], [FR-EX-04]), and the
/// has-body fact with its body's token count (S-500, [FR-EX-11]) from the
/// language's declared `body_kinds`.
///
/// [FR-EX-03]: ../../../docs/specs/requirements/FR-EX-03.md
/// [FR-EX-04]: ../../../docs/specs/requirements/FR-EX-04.md
/// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
fn function_metrics(decl: &Decl<'_>, keywords: &[String], body_kinds: &[String]) -> FunctionMetrics {
    let body = shape::callable_body(decl.node, body_kinds);
    FunctionMetrics {
        cyclomatic_complexity: complexity::cyclomatic_complexity(decl.node, keywords),
        // `end_line >= start_line` always holds for a tree-sitter node;
        // `saturating_sub` is belt-and-braces against any future change.
        line_count: decl.end_line.saturating_sub(decl.start_line) + 1,
        has_body: body.is_some(),
        body_tokens: body.map_or(0, shingle::token_count),
    }
}

/// Step 1 of [`extract_one`]: the declarations the `symbols` query captures,
/// the file's `package` statement when its grammar's query names one (S-472),
/// the namespace declarations it names (S-518, [`declared_types::NAMESPACE_CAPTURE`]),
/// and the declarations a parse-error region cost the file ([FR-EX-30]).
///
/// A [`SELF_TYPE_CAPTURE`] gives its text to every declaration captured in the
/// **same match** (S-493). It is gathered by declaration node and applied after
/// the walk, so it holds regardless of which pattern captured the declaration
/// first — a
/// declaration another, self-type-less pattern also names (Rust's plain
/// `function_item` pattern) is still taken once, by the first-wins rule, and
/// still carries its self type.
///
/// [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
fn collect_decls<'t>(
    query: &Query,
    root: Node<'t>,
    source: &[u8],
) -> (
    Vec<Decl<'t>>,
    Option<String>,
    Vec<declared_types::NamespaceScope>,
    ParseDamage,
) {
    let capture_names = query.capture_names();
    let mut decls: Vec<Decl<'t>> = Vec::new();
    // Guard against a declaration node being captured more than once (a query
    // with overlapping patterns): a duplicate would corrupt the parent map and
    // inflate ordinals, churning the symbol ID. The current `symbols.scm` has
    // one pattern per node kind so this never fires today, but it keeps the
    // ID-stability invariant (ADR-07) robust against future query authors.
    let mut seen_decls: HashSet<usize> = HashSet::new();
    // Declarations a capture named nothing (S-512). One that another pattern
    // names properly is taken after all, so only the remainder counts as skipped.
    let mut nameless: HashSet<usize> = HashSet::new();
    let mut damage = ParseDamage::default();
    let mut package: Option<String> = None;
    let mut namespaces: Vec<declared_types::NamespaceScope> = Vec::new();
    // declaration node id → the self type its match declares (S-493).
    let mut self_types: HashMap<usize, String> = HashMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        let self_type = m
            .captures
            .iter()
            .find(|c| capture_names[c.index as usize] == SELF_TYPE_CAPTURE)
            .and_then(|c| c.node.utf8_text(source).ok())
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let chained = m
            .captures
            .iter()
            .any(|c| capture_names[c.index as usize] == declared_types::NAMESPACE_CHAINED_CAPTURE);
        for cap in m.captures {
            let capture = capture_names[cap.index as usize];
            if declared_types::note_package(&mut package, capture, cap.node, source)
                || declared_types::note_namespace(&mut namespaces, capture, cap.node, source, chained)
            {
                continue;
            }
            let Some(kind) = kind_for_capture(capture) else {
                continue; // a capture we do not map to a NodeKind
            };
            // The query is expected to capture the *name* node; its parent is the
            // declaration. If a query instead captures the declaration node, the
            // parent walk simply starts one level higher — the contract is that
            // a capture identifies one declaration.
            let name_node = cap.node;
            let lifted = lift_to_declaration(name_node);
            let decl_node = lifted.node;
            // Checked before `seen_decls`, so another pattern naming the same
            // declaration properly is still taken.
            if names_nothing(name_node) {
                nameless.insert(decl_node.id());
                continue;
            }
            if let Some(self_type) = self_type {
                self_types
                    .entry(decl_node.id())
                    .or_insert_with(|| self_type.to_string());
            }
            if !seen_decls.insert(decl_node.id()) {
                continue; // already captured by another pattern — keep the first
            }
            let Ok(name) = name_node.utf8_text(source) else {
                continue; // non-UTF-8 identifier slice — skip defensively
            };
            damage.truncated += usize::from(lifted.at_error);
            decls.push(Decl {
                node: decl_node,
                kind,
                name: name.to_string(),
                start_byte: decl_node.start_byte(),
                start_line: decl_node.start_position().row as u32 + 1,
                end_line: decl_node.end_position().row as u32 + 1,
                parent: None,
                ordinal: 0,
                self_type: None,
            });
        }
    }
    for decl in &mut decls {
        decl.self_type = self_types.remove(&decl.node.id());
    }
    damage.skipped = nameless.difference(&seen_decls).count();
    (decls, package, namespaces, damage)
}

/// The declarations a parse-error region cost one file ([FR-EX-30]), counted
/// into its partial-extraction warning ([CR-168] §3.2(2)).
///
/// [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
/// [CR-168]: ../../../docs/requests/CR-168-an-index-never-silently-empties.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ParseDamage {
    /// Declarations kept at their own node because their lift stopped at an
    /// ERROR region: the body error recovery tore off is not theirs to claim.
    truncated: usize,
    /// Declarations that emit nothing because their name is MISSING or
    /// zero-width ([`names_nothing`]).
    skipped: usize,
}

impl ParseDamage {
    /// Append this damage to the file's partial-extraction warning. A file that
    /// lost no declaration keeps the warning byte-identical.
    ///
    /// Both counts imply the parse has an error, so the warning always exists
    /// when either is non-zero: an ERROR ancestor and a MISSING name each set
    /// tree-sitter's `has_error`, and a zero-width name is only ever recovered.
    fn annotate(self, warning: &mut String) {
        if self.truncated + self.skipped > 0 {
            warning.push_str(&format!(
                "; {} declaration(s) truncated and {} skipped at a parse error",
                self.truncated, self.skipped
            ));
        }
    }
}

/// `true` for a captured name node that names nothing: a MISSING
/// (error-recovery) or zero-width node. Its descriptor would be the bare suffix,
/// which is not a valid SCIP symbol, and storing it made the graph unreadable
/// (S-512 — a C++ `enum : uint8_t {}`, a C `typedef struct {…} ;`). Such a
/// declaration emits nothing; the file's other declarations still do. A MISSING
/// node is always zero-width, so the second test is the one that decides; the
/// first names the case it exists for.
fn names_nothing(name_node: Node<'_>) -> bool {
    name_node.is_missing() || name_node.start_byte() == name_node.end_byte()
}

/// Lift a captured name to its parent, and past any C-family *declarator*
/// wrapper to the body-bearing declaration/definition that owns it (S-058).
///
/// Every grammar Logos supported before C++ puts a declaration's name as a
/// *direct* child of the node that also holds its body (Go/Python/Java
/// `… name: (identifier) body: …`, a TS `variable_declarator` whose value is the
/// arrow), so `name.parent()` is already the right declaration node. The C
/// family is the exception: `int add(int) { … }` nests the name inside a
/// `function_declarator`, and the body is that declarator's *sibling* under the
/// `function_definition`. Without this lift the per-function metrics
/// (complexity/nesting/shingles, [FR-EX-03]/[FR-EX-07]/[FR-EX-09]) and reference
/// attribution (the `this->field` access that feeds LCOM4, [FR-EX-08]) would see
/// the bodyless declarator and silently degrade.
///
/// The walk climbs only through the C/C++ declarator node kinds, which no other
/// supported grammar produces — so it is a provable no-op for every pre-C++
/// language (`name.parent()` is never one of these kinds), keeping their decl
/// nodes byte-identical ([NFR-RA-06]). The captured *name* is unchanged: it is
/// still read from the original leaf node, never from the lifted declaration.
///
/// **It never enters an ERROR node ([FR-EX-30]).** Error recovery can strand a
/// declarator directly in an ERROR region that swallows the rest of the file
/// (ccache's `parse_umask`, whose climb used to land on a 627-line ERROR and
/// report CC 117). A climb that would enter an ERROR is abandoned, and the
/// declaration keeps the name's own declarator — never an outer declarator
/// wrapper that recovery stretched over torn body tokens. A name that sits
/// loose in an ERROR region (a C++ class head whose body recovery tore apart)
/// is its own node. Either way [`Lifted::at_error`] is set and the declaration
/// counts as truncated. Only C
/// and C++ climb declarators, and only the C++ query captures a name loose in
/// an ERROR region, so the guard is a no-op for every other language.
///
/// [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
fn lift_to_declaration(name: Node<'_>) -> Lifted<'_> {
    const CFAMILY_DECLARATORS: [&str; 6] = [
        "function_declarator",
        "pointer_declarator",
        "reference_declarator",
        "array_declarator",
        "parenthesized_declarator",
        "init_declarator",
    ];
    let cut = |node| Lifted { node, at_error: true };
    let own = match name.parent() {
        Some(parent) if parent.is_error() => return cut(name),
        Some(parent) => parent,
        None => name,
    };
    let mut decl = own;
    while CFAMILY_DECLARATORS.contains(&decl.kind()) {
        match decl.parent() {
            // Recovery can nest declarators whose ERROR children hold the torn
            // body, so a cut climb keeps the name's own declarator, never the
            // outermost wrapper it reached.
            Some(parent) if parent.is_error() => return cut(own),
            Some(parent) => decl = parent,
            None => break,
        }
    }
    Lifted {
        node: decl,
        at_error: false,
    }
}

/// The declaration node a captured name lifts to ([`lift_to_declaration`]).
#[derive(Debug, Clone, Copy)]
struct Lifted<'tree> {
    node: Node<'tree>,
    /// The climb stopped at an ERROR region, so `node` is the declaration's
    /// own fragment rather than its body-bearing owner ([FR-EX-30]).
    ///
    /// [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
    at_error: bool,
}

/// `true` for a Rust `impl`-nested associated `function_item` — the declarations
/// re-kinded from free [`NodeKind::Function`] to [`NodeKind::Method`] at emission
/// (CR-068 Part B, [FR-EX-05], [ADR-39]).
///
/// The Rust `symbols.scm` captures *every* `function_item` as `@symbol.function`
/// — a tree-sitter query cannot express "`function_item` NOT inside an `impl`" —
/// so [`kind_for_capture`] maps them all to `Function`. This restores the
/// distinction structurally: an associated function is a `function_item` whose
/// immediate parent is the `declaration_list` body of an `impl_item`. A local
/// `fn` nested in a method body (its parent is a `block`) and a `trait_item`
/// default method (its grandparent is a `trait_item`, not an `impl_item`) are
/// deliberately **not** re-kinded — only `impl` associated functions are, matching
/// the [resolution-engine]'s `Type::func` model where associated items collapse to
/// module scope.
///
/// Rust-specific by construction and a proven no-op for every other grammar,
/// mirroring [`lift_to_declaration`]: no other supported grammar produces an
/// `impl_item`, and every other language already kinds its methods via a
/// `@symbol.method` capture in its own query. The re-kinding is **emission-only**:
/// `decl.kind` stays `Function` through the symbol and ordinal machinery, and
/// `Function`/`Method` share one [`DescriptorFamily`] — the method-descriptor
/// slot ([`descriptor_for`]) and the unit [`assign_ordinals`] numbers by — so
/// every pre-existing symbol ID is byte-identical, only the emitted `nodes.kind`
/// discriminant changes ([NFR-RA-06]).
///
/// [FR-EX-05]: ../../../docs/specs/requirements/FR-EX-05.md
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
/// [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
fn is_rust_associated_method(node: Node<'_>) -> bool {
    node.kind() == "function_item"
        && node.parent().is_some_and(|body| {
            body.kind() == "declaration_list"
                && body.parent().is_some_and(|owner| owner.kind() == "impl_item")
        })
}

/// The last `::`-path segment of a type path text (`LanguagePlugin` for
/// `crate::plugin::LanguagePlugin`), trimmed.
fn last_type_segment(text: &str) -> &str {
    text.rsplit("::").next().unwrap_or(text).trim()
}

/// The simple (last-segment) name of a trait bound node — a `type_identifier`
/// (`T`), a `scoped_type_identifier` (`a::b::T` → `T`), or a generic trait
/// (`Iterator<Item=X>` → `Iterator`, its base head). `None` for any other shape.
fn trait_simple_name(trait_node: Node<'_>, source: &[u8]) -> Option<String> {
    let named = match trait_node.kind() {
        "type_identifier" | "scoped_type_identifier" => trait_node,
        "generic_type" => trait_node.child_by_field_name("type")?,
        _ => return None,
    };
    let name = last_type_segment(named.utf8_text(source).ok()?);
    (!name.is_empty()).then(|| name.to_string())
}

/// The trait name of a **trait-object** type `ty`, or `None` when `ty` is not a
/// `dyn T` (S-281, [CR-073], [FR-RS-08]). Peels the transparent layers a `dyn T`
/// can wear — `&dyn`/`&mut dyn` (`reference_type`), a `T + Send` bound list
/// (`bounded_type`), and the smart pointers `Box`/`Rc`/`Arc<dyn T>`
/// (`generic_type`) — down to the `dynamic_type`, then reads its principal
/// `trait:`. Depth-bounded (4 covers `Arc<Box<dyn T>>` and beyond); anything
/// that is not a trait object yields `None`, so the receiver stays a bare
/// method name and never fans out (the CR-066 guard, [FR-RS-06]).
///
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
/// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
fn dyn_trait_of_type(ty: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cur = ty;
    for _ in 0..4 {
        match cur.kind() {
            "dynamic_type" => {
                return trait_simple_name(cur.child_by_field_name("trait")?, source);
            }
            "reference_type" => cur = cur.child_by_field_name("type")?,
            // `dyn A + Send`: the object trait sits in the reference/dynamic part.
            "bounded_type" => {
                cur = (0..cur.named_child_count())
                    .filter_map(|i| cur.named_child(i))
                    .find(|c| {
                        matches!(c.kind(), "reference_type" | "dynamic_type" | "generic_type")
                    })?;
            }
            // Only the transparent smart-pointer wrappers carry a `dyn T` object;
            // `Vec<dyn T>` and the like are not trait-object receivers.
            "generic_type" => {
                let head = cur.child_by_field_name("type")?;
                if !matches!(last_type_segment(head.utf8_text(source).ok()?), "Box" | "Rc" | "Arc") {
                    return None;
                }
                let args = cur.child_by_field_name("type_arguments")?;
                cur = (0..args.named_child_count())
                    .filter_map(|i| args.named_child(i))
                    .find(|c| {
                        // `bounded_type` covers `Arc<dyn T + Send>`, symmetric with
                        // the reference path that already peels `&dyn T + Send`.
                        matches!(
                            c.kind(),
                            "dynamic_type" | "reference_type" | "generic_type" | "bounded_type"
                        )
                    })?;
            }
            _ => return None,
        }
    }
    None
}

/// The trait name of the enclosing `impl T for X` block of a Rust impl method
/// (S-281, [CR-073]) — the enabling link the binder enumerates a trait method's
/// impls from ([FR-RS-08]). `method` is an impl-nested `function_item`
/// (`is_rust_associated_method` holds): `function_item → declaration_list →
/// impl_item`. A **trait** impl carries a `trait:` field; an inherent
/// `impl X { … }` does not, so it yields `None` and emits no Implements ref.
///
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
fn rust_impl_trait_name(method: Node<'_>, source: &[u8]) -> Option<String> {
    let impl_item = method.parent()?.parent()?;
    if impl_item.kind() != "impl_item" {
        return None;
    }
    trait_simple_name(impl_item.child_by_field_name("trait")?, source)
}

/// The in-scope names a path-grammar file's imports bind (S-440, [CR-142] D2,
/// [FR-RS-03]) — the per-file syntactic evidence that a call goes **through an
/// import**, so it can be recorded qualified by the module it names.
///
/// The imported rung of the binder needs to know which module a call's name
/// came from; the call's own text does not say. This pre-pass reads it off the
/// file's import statements and rewrites exactly two call shapes into the
/// `<specifier target>::<name>` form the binder resolves against that
/// import's **bound** target (the `Imports` edge, not the name hierarchy):
///
/// - a bare call of a **named import** — TypeScript/JavaScript
///   `import { a } from './m'` makes `a()` read `.::m::a`, and
///   `import { a as b }` makes `b()` read `.::m::a`, the name the module
///   exports it under, never the local spelling;
/// - a member call whose receiver is an **imported module** — a Go package
///   (`admin.Register()` → `…::internal::admin::Register`, its qualifier the
///   explicit import alias or the path's last segment), or a TypeScript
///   namespace import (`import * as nav` → `nav.f()` reads `.::nav::f`).
///
/// Everything else is left exactly as before. Only a **relative** TypeScript
/// specifier contributes (a package such as `react` names no workspace file,
/// and its calls keep their bare form); a default import is not read (the name
/// it was exported under is not in the importing file); a Go dot or blank
/// import binds no qualifier. A local name the file also **declares** is
/// dropped, and so is one a function scope enclosing the call **binds** — a
/// parameter, a local, a destructured prop, a Go `:=` ([`local_bindings`]):
/// `func handle(admin *admin.Server) { admin.Reload() }` calls a method on the
/// parameter, not the package. The shadowed call keeps its bare form — a
/// missed edge, never a fabricated one ([NFR-RA-05]). The receiver of a Go
/// method call on a value (`s.Start()`) is no import and stays a
/// receiver-unqualified method name, so the [FR-RS-06] discipline is
/// untouched.
///
/// Built only for a language declaring [`ImportSpecifier::Path`]; every other
/// language (Rust included) takes [`ImportBindings::default`], which rewrites
/// nothing.
///
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
/// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
/// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Default)]
struct ImportBindings {
    /// A named import's local name → `<specifier target>::<exported name>`.
    named: HashMap<String, String>,
    /// A module qualifier's local name → the specifier target it names.
    qualifiers: HashMap<String, String>,
    /// Function-scope node id → the names bound locally in it: parameters,
    /// variable declarators, destructured patterns, Go `:=` left-hand sides
    /// ([`local_bindings`]). A name bound here shadows any import of that name
    /// for every reference inside the scope.
    locals: HashMap<usize, HashSet<String>>,
}

impl ImportBindings {
    /// Read the file's import statements off the `@ref.import` captures.
    fn collect(
        query: &Query,
        root: Node<'_>,
        source: &[u8],
        decls: &[Decl<'_>],
        semantics: &Semantics,
    ) -> ImportBindings {
        let capture_names = query.capture_names();
        let mut reader = ImportReader {
            source,
            named: HashMap::new(),
            qualifiers: HashMap::new(),
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            for cap in m.captures {
                if capture_names[cap.index as usize] == "ref.import" {
                    reader.read(cap.node, &semantics.specifier_extensions);
                }
            }
        }
        let declared: HashSet<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        let keep = |map: HashMap<String, Option<String>>| -> HashMap<String, String> {
            map.into_iter()
                .filter(|(local, _)| !declared.contains(local.as_str()))
                .filter_map(|(local, v)| v.map(|v| (local, v)))
                .collect()
        };
        ImportBindings {
            named: keep(reader.named),
            qualifiers: keep(reader.qualifiers),
            locals: local_bindings(root, source),
        }
    }

    /// Whether `name`, referenced at `node`, is bound by a function scope that
    /// encloses it — a parameter, a local, a destructured prop — and so names
    /// that local value rather than anything imported or declared at the top
    /// level of the file.
    fn locally_bound(&self, node: Node<'_>, name: &str) -> bool {
        if self.locals.is_empty() {
            return false;
        }
        let mut ancestor = node.parent();
        while let Some(n) = ancestor {
            if self.locals.get(&n.id()).is_some_and(|names| names.contains(name)) {
                return true;
            }
            ancestor = n.parent();
        }
        false
    }

    /// The qualified target a bare call of `name` at `node` is recorded under,
    /// when `name` is a named import not shadowed by a local binding.
    fn named_target(&self, node: Node<'_>, name: &str) -> Option<&str> {
        let target = self.named.get(name)?;
        (!self.locally_bound(node, name)).then_some(target.as_str())
    }

    /// The specifier target of `method_name`'s receiver when that receiver is a
    /// plain identifier naming an imported module — a Go `selector_expression`
    /// operand or a TypeScript `member_expression` object — else `None`.
    fn qualifier_of(&self, method_name: Node<'_>, source: &[u8]) -> Option<&str> {
        if self.qualifiers.is_empty() {
            return None;
        }
        let access = method_name.parent()?;
        let receiver = match access.kind() {
            "selector_expression" => access.child_by_field_name("operand")?,
            "member_expression" => access.child_by_field_name("object")?,
            _ => return None,
        };
        if receiver.kind() != "identifier" {
            return None;
        }
        let name = receiver.utf8_text(source).ok()?;
        let module = self.qualifiers.get(name)?;
        (!self.locally_bound(receiver, name)).then_some(module.as_str())
    }
}

/// The node kinds that open a function scope in the path-grammar languages —
/// TypeScript/JavaScript functions, arrows and methods, Go functions, methods
/// and function literals.
const FUNCTION_SCOPES: &[&str] = &[
    "function_declaration",
    "function_expression",
    "function",
    "generator_function",
    "generator_function_declaration",
    "arrow_function",
    "method_definition",
    "method_declaration",
    "func_literal",
];

/// Every name bound inside a function scope of the file, keyed by the scope's
/// node id (S-440) — the evidence that a reference names a local value.
///
/// Conservative by construction: a name bound anywhere in a scope (in any
/// block of it, before or after the reference) counts for the whole scope, so
/// an over-approximation can only turn a call through an import back into a
/// bare, unresolved one — a missed edge, never a fabricated one ([NFR-RA-05]).
/// A binding at file scope is not collected: TypeScript and Go both reject a
/// top-level redeclaration of an imported name.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn local_bindings(root: Node<'_>, source: &[u8]) -> HashMap<usize, HashSet<String>> {
    let mut out: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        if !binds_a_name(node) {
            continue;
        }
        let Some(scope) = enclosing_function_scope(node) else { continue };
        if let Ok(name) = node.utf8_text(source) {
            out.entry(scope.id()).or_default().insert(name.to_string());
        }
    }
    out
}

/// The nearest function-scope ancestor of `node`, if any.
fn enclosing_function_scope(node: Node<'_>) -> Option<Node<'_>> {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if FUNCTION_SCOPES.contains(&n.kind()) {
            return Some(n);
        }
        ancestor = n.parent();
    }
    None
}

/// Whether `node` is an identifier in a **binding** position: a parameter, a
/// declarator name, a destructuring pattern element, a `catch` parameter, a
/// `for … of` variable, or a Go parameter / `:=` / `range` left-hand side.
fn binds_a_name(node: Node<'_>) -> bool {
    if node.kind() == "shorthand_property_identifier_pattern" {
        return true;
    }
    if node.kind() != "identifier" {
        return false;
    }
    let Some(parent) = node.parent() else { return false };
    let is_field = |field: &str| parent.child_by_field_name(field) == Some(node);
    match parent.kind() {
        "formal_parameters" | "array_pattern" | "rest_pattern" | "object_pattern" => true,
        "required_parameter" | "optional_parameter" => is_field("pattern"),
        "arrow_function" => is_field("parameter"),
        "variable_declarator" => is_field("name"),
        // Go names every parameter of `a, b int` in one declaration, and its type
        // is never a plain `identifier`, so each identifier child is a name.
        "parameter_declaration" | "variadic_parameter_declaration" | "var_spec" | "const_spec" => {
            true
        }
        "pair_pattern" => is_field("value"),
        "assignment_pattern" | "object_assignment_pattern" => is_field("left"),
        "catch_clause" => is_field("parameter"),
        "for_in_statement" => is_field("left"),
        // Go `a, b := …` and `for k, v := range …`: the left expression list.
        "expression_list" => parent.parent().is_some_and(|gp| {
            matches!(gp.kind(), "short_var_declaration" | "range_clause")
                && gp.child_by_field_name("left") == Some(parent)
        }),
        _ => false,
    }
}

/// The accumulator [`ImportBindings::collect`] fills, one import statement at a
/// time. A local name bound twice with different values (invalid code, or a
/// merge artefact) maps to `None` — it decides nothing.
struct ImportReader<'s> {
    source: &'s [u8],
    named: HashMap<String, Option<String>>,
    qualifiers: HashMap<String, Option<String>>,
}

impl ImportReader<'_> {
    fn text(&self, n: Node<'_>) -> Option<String> {
        n.utf8_text(self.source).ok().map(str::to_string)
    }

    fn bind(map: &mut HashMap<String, Option<String>>, local: String, value: String) {
        map.entry(local)
            .and_modify(|v| {
                if v.as_ref() != Some(&value) {
                    *v = None;
                }
            })
            .or_insert(Some(value));
    }

    /// One `@ref.import` capture: the specifier string of a TypeScript
    /// `import … from '<spec>'` or a Go `import [alias] "<path>"`.
    fn read(&mut self, spec_node: Node<'_>, extensions: &[String]) {
        let Some(spec) = self.text(spec_node) else { return };
        let segments = specifier_segments(&spec, extensions);
        let (Some(first), Some(statement)) = (segments.first(), spec_node.parent()) else {
            return;
        };
        let target = segments.join("::");
        match statement.kind() {
            "import_statement" if is_relative_head(first) => {
                let mut cursor = statement.walk();
                let clauses: Vec<Node<'_>> = statement
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() == "import_clause")
                    .collect();
                for clause in clauses {
                    self.read_ts_clause(clause, &target);
                }
            }
            "import_spec" => {
                let local = match statement.child_by_field_name("name") {
                    Some(n) if n.kind() == "package_identifier" => self.text(n),
                    Some(_) => None, // a dot or blank import binds no qualifier
                    None => segments.last().cloned(),
                };
                if let Some(local) = local {
                    Self::bind(&mut self.qualifiers, local, target);
                }
            }
            _ => {} // a `require()` argument, or a package specifier
        }
    }

    /// A TypeScript `import_clause`: its named imports and namespace import. A
    /// default import is not read — the name it was exported under is unknown.
    fn read_ts_clause(&mut self, clause: Node<'_>, target: &str) {
        let mut cursor = clause.walk();
        let parts: Vec<Node<'_>> = clause.named_children(&mut cursor).collect();
        for part in parts {
            match part.kind() {
                "named_imports" => {
                    let mut spec_cursor = part.walk();
                    let specs: Vec<Node<'_>> = part.named_children(&mut spec_cursor).collect();
                    for s in specs {
                        self.read_ts_specifier(s, target);
                    }
                }
                "namespace_import" => {
                    let mut ns_cursor = part.walk();
                    let ns = part
                        .named_children(&mut ns_cursor)
                        .find(|n| n.kind() == "identifier")
                        .and_then(|n| self.text(n));
                    if let Some(ns) = ns {
                        Self::bind(&mut self.qualifiers, ns, target.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    /// One `import_specifier`: `a` or `a as b` — the local name maps to the
    /// name the module exports.
    fn read_ts_specifier(&mut self, specifier: Node<'_>, target: &str) {
        let exported = specifier
            .child_by_field_name("name")
            .filter(|n| n.kind() == "identifier")
            .and_then(|n| self.text(n));
        let local = specifier
            .child_by_field_name("alias")
            .and_then(|n| self.text(n))
            .or_else(|| exported.clone());
        if let (Some(local), Some(exported)) = (local, exported) {
            Self::bind(&mut self.named, local, format!("{target}::{exported}"));
        }
    }
}

/// The trait name when a receiver-method call's receiver is a **provable**
/// workspace trait object (S-281, [CR-073], [FR-RS-08]).
///
/// `field_ident` is the `@ref.method` capture — the method-name `field_identifier`
/// of a `field_expression`. The receiver must be a plain named `identifier` whose
/// `&dyn T` type is provable **from this file's own syntax**: an explicit
/// parameter type on the enclosing `function_item`, or a `let recv: &dyn T`
/// binding in its body. This is a superset-free, per-file-pure gate — a receiver
/// whose type comes from inference (a closure parameter, a method-chain result)
/// is *not* provable and stays a bare method name, so the fan-out never fires on
/// an unknown receiver ([FR-RS-06], the CR-066 guard is not loosened) and the
/// failure mode is a missed edge (false-live), never a fabricated one
/// ([NFR-RA-05]). Rust-specific by construction — the node kinds it matches
/// (`field_expression`, `function_item`, `dynamic_type`, …) exist only in the
/// Rust grammar, so it is a proven no-op for every other language.
///
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
/// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn rust_dyn_receiver_trait(field_ident: Node<'_>, source: &[u8]) -> Option<String> {
    let field_expr = field_ident.parent()?;
    if field_expr.kind() != "field_expression" {
        return None;
    }
    let receiver = field_expr.child_by_field_name("value")?;
    if receiver.kind() != "identifier" {
        return None; // only a simple named receiver is provable per-file
    }
    let recv_name = receiver.utf8_text(source).ok()?;
    // Climb to the enclosing function; its parameters and `let` bindings are the
    // only per-file-provable sources of a receiver's `&dyn T` type.
    let mut anc = field_expr.parent();
    while let Some(n) = anc {
        if n.kind() == "function_item" {
            return function_dyn_binding(n, recv_name, source);
        }
        anc = n.parent();
    }
    None
}

/// The `&dyn T` trait bound of a name `recv` inside `fn_node` — an explicit
/// parameter type (preferred), else a `let recv: &dyn T` binding in the body.
/// A nested `function_item` is not descended (its bindings belong to its own
/// scope), so a shadowing inner binding cannot mis-type an outer receiver.
fn function_dyn_binding(fn_node: Node<'_>, recv_name: &str, source: &[u8]) -> Option<String> {
    // Collect EVERY binding of `recv_name` in this function's own scope — each
    // parameter and each body `let` (not descending into a nested fn/closure,
    // which owns its own scope) whose pattern is exactly the receiver name — as
    // its optional type-annotation node.
    //
    // The proof must be a *single* per-file type: a name bound more than once is
    // shadowed, and this walk is scope-blind (it cannot tell which binding is live
    // at the call site), so guessing one would fabricate a dispatch edge — e.g. a
    // `c: &Concrete` parameter shadowed by a later `let c: &dyn T` would wrongly
    // qualify the *first* `c.method()` as `T::method`. Bail on anything but exactly
    // one binding, and require that binding to be annotated: an unambiguous,
    // annotated `&dyn T` binding qualifies; zero, several, or an un-annotated
    // binding is an honest miss ([NFR-RA-05]).
    let name_of = |n: Node<'_>| {
        n.child_by_field_name("pattern")
            .and_then(|p| p.utf8_text(source).ok())
    };
    let mut bindings: Vec<Option<Node<'_>>> = Vec::new();
    if let Some(params) = fn_node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            if p.kind() == "parameter" && name_of(p) == Some(recv_name) {
                bindings.push(p.child_by_field_name("type"));
            }
        }
    }
    if let Some(body) = fn_node.child_by_field_name("body") {
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "let_declaration" && name_of(n) == Some(recv_name) {
                bindings.push(n.child_by_field_name("type"));
            }
            for i in 0..n.child_count() {
                if let Some(ch) = n.child(i) {
                    if !matches!(ch.kind(), "function_item" | "closure_expression") {
                        stack.push(ch);
                    }
                }
            }
        }
    }
    match bindings.as_slice() {
        [Some(ty)] => dyn_trait_of_type(*ty, source),
        _ => None, // zero, several (shadowed), or an un-annotated single binding
    }
}

/// Map a `@symbol.<kind>` capture name to a [`NodeKind`], or `None` for a
/// capture this pass does not turn into a node.
///
/// The kind segment is matched against [`NodeKind::as_str`], so the mapping
/// never drifts from the ontology — adding a capture name that matches a node
/// kind's wire name is all a new query needs.
fn kind_for_capture(capture_name: &str) -> Option<NodeKind> {
    let (group, kind_name) = capture_name.split_once('.')?;
    if group != SYMBOL_CAPTURE_GROUP {
        return None;
    }
    NodeKind::ALL
        .iter()
        .copied()
        .find(|k| k.as_str() == kind_name)
}

/// Resolve each declaration's nearest enclosing captured declaration by walking
/// the tree-sitter ancestry. A declaration with no captured ancestor is at file
/// scope (`parent = None`).
///
/// One exception: a [`NodeKind::Field`] declared **in a method's parameter list**
/// — a TypeScript parameter property, `constructor(private readonly http: Client)`
/// — is owned by the class the method belongs to, not by the method. Lexically it
/// nests in the constructor, but it is a member of the class: the own-field access
/// binder looks a field up among the class's members ([FR-EX-08]), and the
/// class-cohesion metric counts the class's fields ([FR-QM-11]). The test is
/// structural (the field's node sits inside the method's `parameters` field), not a
/// `constructor` name match, so a field a plugin captures elsewhere in a method keeps
/// the method as its parent. Java reaches that guard today: its field query is not
/// anchored to a class body and an anonymous class is not a captured class, so a
/// field of `new Runnable() { int n; }` in a method body is a `Field` under the
/// method, and stays there — pinned by `structural_metrics.rs`
/// `a_java_anonymous_class_field_in_a_method_body_keeps_the_method_as_its_parent`
/// (S-477, CR-154).
///
/// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
/// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
fn assign_parents(decls: &mut [Decl<'_>]) {
    let id_to_idx: HashMap<usize, usize> = decls
        .iter()
        .enumerate()
        .map(|(i, d)| (d.node.id(), i))
        .collect();

    for decl in decls.iter_mut() {
        // `node` is `Copy`, so walking the ancestry borrows nothing from `decl`.
        let mut ancestor = decl.node.parent();
        while let Some(node) = ancestor {
            if let Some(&j) = id_to_idx.get(&node.id()) {
                decl.parent = Some(j);
                break;
            }
            ancestor = node.parent();
        }
    }

    // Lift a parameter-list field to the method's own parent (see above). Done in
    // a second pass so every method's parent is already resolved; the lift reads
    // the *original* parents and writes only `Field` decls, so it is order-free.
    let lifted: Vec<(usize, usize)> = decls
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == NodeKind::Field)
        .filter_map(|(i, d)| {
            let method = &decls[d.parent?];
            if method.kind != NodeKind::Method {
                return None;
            }
            let params = method.node.child_by_field_name("parameters")?;
            let (start, end) = (d.node.start_byte(), d.node.end_byte());
            let inside = params.start_byte() <= start && end <= params.end_byte();
            inside.then_some((i, method.parent?))
        })
        .collect();
    for (i, class) in lifted {
        decls[i].parent = Some(class);
    }
}

/// Assign each declaration its ordinal among same-name siblings of one
/// [`DescriptorFamily`], in the canonical sort order `(start_byte, kind, name)`
/// ([ADR-07]).
///
/// Numbered per family, not per kind (S-512): kinds that share a descriptor
/// suffix — a Go function and method (`name().`), a TS interface and class
/// (`name#`) — would otherwise both take ordinal 0 and render one symbol. Where
/// no scope holds two kinds of one family under one name, per-family numbering
/// equals per-kind numbering and every ID is unchanged.
fn assign_ordinals(decls: &mut [Decl<'_>]) {
    // Group declaration indices by parent scope.
    let mut by_parent: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, d) in decls.iter().enumerate() {
        by_parent.entry(d.parent).or_default().push(i);
    }

    for (_, mut siblings) in by_parent {
        siblings.sort_by(|&a, &b| {
            (
                decls[a].start_byte,
                decls[a].kind.as_i32(),
                decls[a].name.as_str(),
            )
                .cmp(&(
                    decls[b].start_byte,
                    decls[b].kind.as_i32(),
                    decls[b].name.as_str(),
                ))
        });
        // Owned-String keys so the counter does not borrow `decls` while we
        // write back `ordinal`.
        let mut seen: HashMap<(DescriptorFamily, String), u32> = HashMap::new();
        for idx in siblings {
            let key = (descriptor_family(decls[idx].kind), decls[idx].name.clone());
            let ordinal = *seen.get(&key).unwrap_or(&0);
            decls[idx].ordinal = ordinal;
            seen.insert(key, ordinal + 1);
        }
    }
}

/// Build the descriptor chain (outermost scope down to the leaf) for one
/// declaration, each rendered with its own ordinal.
fn scope_chain(decls: &[Decl<'_>], i: usize) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = Some(i);
    while let Some(idx) = current {
        chain.push(descriptor_for(
            decls[idx].kind,
            &decls[idx].name,
            decls[idx].ordinal,
        ));
        current = decls[idx].parent;
    }
    chain.reverse();
    chain
}

/// Unit tests for the extraction walk.
///
/// Gated on `lang-rust` because most fixtures here are Rust sources. Note the
/// consequence for the per-language arms: the S-343 TypeScript client-call
/// tests live in this module (they need the crate-private
/// [`collect_invocation_sites`] for their slot-level assertions), so they
/// inherit this gate — a `--no-default-features --features lang-typescript`
/// build compiles and passes without running them. The suite that gates this
/// project runs `--all-features`, where they always run. The sibling arms whose
/// tests need only the public surface live in `tests/` instead and carry their
/// own feature gate (`tests/go_invocations.rs`, `tests/java_http_client_call.rs`).
#[cfg(all(test, feature = "lang-rust"))]
mod tests;
