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

// A callable's parameter range and a call's argument count (S-591, CR-190,
// FR-EX-32), and whether a Rust impl function takes `self` (CR-200): read off
// the `@arity.*` captures a plugin's queries declare.
mod arity;
// The facts one associated-item lookup reads (S-606, CR-202, FR-EX-34): impl
// block headers, receiver modes, enum variant names and required signatures,
// read off the `@item.*` captures a plugin's `symbols` query declares.
mod assoc;
mod complexity;
// How one declaration completes another (S-577, FR-EX-29): out-of-line members
// merge into their declaration, prototypes pair with definitions, class-body
// callables are methods — read off the `@decl.*` captures a plugin's `symbols`
// query declares.
mod definition;
// The names a callable's body binds locally (S-593, CR-191, FR-EX-33): each
// with its kind, scope and position, read off the `@local.*` captures a
// plugin's `locals` query declares. Facts only: `local_shadow` reads them.
mod locals;
// A local binding shadows the receiver and the callee (S-594, CR-191,
// FR-RS-44): the bare-call and receiver rules read off the `locals` facts.
mod local_shadow;
// Which aggregate a C-family member belongs to (S-582, FR-EX-31): a member no
// declaration contains is dropped, and a struct head error recovery tore from
// its body claims the members after it — read off the `@member.*` captures.
mod member;
// Per-function max nesting depth (S-042, CR-005, FR-EX-07): a declarative
// block-kind walk, the structural sibling of `complexity`.
mod nesting;
// Declarations placed by name rather than nesting (S-539, CR-174, FR-EX-18):
// JavaScript's `X.y = function …`, `Foo.prototype.y = …` and `{ y: … }`, read
// off the `@owner.*` captures a plugin's `symbols` query declares.
mod owner;
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
// A bare name that reads a field of the enclosing class (S-556, CR-177 — C#):
// the `Accesses` row of `_count`, `Name`, unless a local of the method hides it.
mod bare_access;
// A bare call no member of its class answers (S-573, CR-183 — Scala): the
// free call the imports and the package bind, where the file shows it.
mod free_call;
// A language's `#if` regions reduced to their first branch before parsing
// (S-555, CR-177 — C#), every position kept.
mod preprocess;
// Java receiver typing (S-467, CR-150 §3.2 A): retypes a receiver call's row to
// a type-qualified Path-form `T::name` where the file proves `T`.
mod receiver;
mod shape;
// The name a test macro declares by its arguments (S-580, CR-184 — C++):
// `TEST(Suite, Name)` is `Suite.Name`, `TEST_CASE("name")` is `name`.
mod test_name;
// Extraction-time test-marker evidence (S-027, FR-EX-06): the per-function
// `test_evidence` flag captured while the AST is in hand — the input the
// unified `is_test` annotation (S-028, FR-AN-05) needs to catch what path
// conventions miss.
pub(crate) mod testmarker;
// `pub(crate)`: the framework pass (resolve::framework, S-012), the topic
// pass (resolve::topics) and the annotation engine's policy nodes (S-014)
// build their symbols with the same builder extraction uses, so every node's
// identity follows ADR-07 and carries the repository coordinate (ADR-85).
pub(crate) mod symbol;

pub use assoc::ImplBlockFact;
pub use locals::LocalBindingFact;
pub use symbol::SymbolContext;

use std::cell::OnceCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use rayon::prelude::*;
use tree_sitter::{Node, Parser, Query, QueryCapture, QueryCursor, StreamingIterator};

use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeKind, ParamRange, ReceiverMode, ReceiverShape, RefForm};
use crate::plugin::{BareSpecifiers, ImportSpecifier, LanguagePlugin, LanguageRegistry, ModuleModelKind, Semantics};
use crate::resolve::http_client_call::ClientCallRefusal;
use crate::resolve::package_key::PackageLayout;
use crate::resolve::{COMMONJS_EXPORTS, DEFAULT_EXPORT};

use config::accessor::{BindingView, DeclaredTypes};
use config::binding::{PropertiesIndex, MEMBER_SCOPE};

use refs::{
    bracketed_call_target, flatten_dotted_import, flatten_use_tree, from_module_segments, import_segments, include_file_segments,
    include_specifier_segments, load_path_specifier_segments, macro_call_refs, qualified_call_target,
    relative_specifier_segments,
    specifier_segments, split_path_text, MacroReceiver,
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

/// A `symbols`-query marker, in the same match as a `@symbol.<kind>` capture,
/// that one declaration node declares several names and this capture is one of
/// them (S-525): Go's `var a, b = 1, 2` and `x, y int`. Without it a declaration
/// node is declared once, by the first name captured — what every other plugin's
/// multi-name form (C's `int a, b;`) has always recorded. Not a [`NodeKind`], so
/// [`kind_for_capture`] never mistakes it for a declaration.
const EACH_NAME_CAPTURE: &str = "symbol.names";

/// A `symbols`-query marker, in the same match as a `@symbol.<kind>` capture,
/// that the declaration is an **attribute an assignment declares** (S-543,
/// [FR-EX-20]): Python's class-body `x = …` / `x: int` and a method's
/// `self.x = …`, and JavaScript's `this.x = …` (S-542). Such a declaration is
/// taken at its name node, never lifted to the assignment, so what the
/// assignment holds — an initialiser's calls, the method's own `self.x` access —
/// stays its scope's. It is owned by its class:
/// the declaration it sits in when that is class-like, else the class of the
/// method it sits in; one sitting anywhere else declares nothing. Each name is
/// declared once per class — a class-body one ahead of a method write, then the
/// first in source order ([`settle_attributes`]). Not a [`NodeKind`], so
/// [`kind_for_capture`] never mistakes it for a declaration.
///
/// [FR-EX-20]: ../../../docs/specs/requirements/FR-EX-20.md
const ATTRIBUTE_CAPTURE: &str = "symbol.attribute";

/// A `symbols`-query marker, in the same match as a `@symbol.<kind>` capture,
/// that the declaration is **state of the class or module around it** (S-564,
/// Ruby's `attr_accessor :x` and the first `@x = …`): see [`settle_state`].
/// Not a [`NodeKind`], so [`kind_for_capture`] never mistakes it for a
/// declaration.
const STATE_CAPTURE: &str = "symbol.state";

/// A `symbols`-query setting, `(#set! symbol.name "<name>")`, on a pattern
/// whose `@symbol.<kind>` capture is not the declaration's name (S-559): a
/// Kotlin `companion object` written without a name is named `Companion`, and
/// the node a pattern can capture is the `companion` keyword. The declaration
/// takes the setting's value as its name, the captured node still deciding
/// which declaration it is. A pattern without the setting takes the captured
/// node's own text, as every pattern did before it. A pattern that sets it
/// captures one declaration: every `@symbol.<kind>` capture of its match takes
/// the name.
const NAME_SETTING: &str = "symbol.name";

/// A `symbols`-query capture on a node the grammar read as something other
/// than the declaration it is (S-559, [FR-EX-24]): Kotlin's top-level
/// annotated expression, which `tree-sitter-kotlin-ng` 1.1 reads a multiply
/// annotated type declaration as without reporting an error. Counted into the
/// file's partial-extraction warning ([`ParseDamage::misread`]); no
/// declaration is taken from it. Not a [`NodeKind`], so [`kind_for_capture`]
/// never mistakes it for one.
///
/// [FR-EX-24]: ../../../docs/specs/requirements/FR-EX-24.md
const MISREAD_CAPTURE: &str = "damage.misread";

/// The `references`-query capture naming a method call whose receiver is
/// **exactly the caller's own instance** — Rust's `self.m()` (S-493,
/// [FR-RS-11]). The captured node is the method name, as for `@ref.method`; a
/// query declaring it keeps its plain `@ref.method` pattern off such calls, so
/// one call is captured once. It is a `@ref.method` the receiver-shape pass
/// reads as marked `self` (S-514, `receiver`).
///
/// [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
const SELF_RECEIVER_METHOD_CAPTURE: &str = "ref.method.self";

/// A name a `@ref.call` path may not be headed by in this file (S-553): a
/// property, field, parameter, local or type parameter the file declares. A C#
/// call `Logger.Log()` reads `Logger` as a type name, which a value of that name
/// obscures; the query marks every such declaration, and a row of two or more
/// segments whose head is among them is dropped after the walk — read
/// file-wide, so it can lose a binding and never invent one ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
const CALL_SHADOW_CAPTURE: &str = "ref.call.shadow";

/// The names a file declares as values ([`CALL_SHADOW_CAPTURE`]) and the head of
/// each multi-segment `@ref.call` row, so [`apply`](CallShadows::apply) can drop
/// the rows a declared value shadows (S-553).
#[derive(Default)]
struct CallShadows {
    names: HashSet<String>,
    heads: Vec<(usize, String)>,
}

impl CallShadows {
    /// Whether `capture` is a shadow marker, which records no row of its own.
    fn note(&mut self, capture: &str, node: Node<'_>, source: &[u8]) -> bool {
        if capture != CALL_SHADOW_CAPTURE {
            return false;
        }
        if let Ok(name) = node.utf8_text(source) {
            self.names.insert(name.trim().to_string());
        }
        true
    }

    /// The call row `row` is headed by `segments[0]`, when it has a head.
    fn head(&mut self, row: usize, segments: &[String]) {
        if segments.len() > 1 {
            self.heads.push((row, segments[0].clone()));
        }
    }

    /// Drop the rows headed by a declared value's name, after the walk and the
    /// receiver pass, which both address rows by index.
    fn apply(self, rows: &mut Vec<RefFact>) {
        if self.names.is_empty() {
            return;
        }
        let dropped: HashSet<usize> = self
            .heads
            .into_iter()
            .filter(|(_, head)| self.names.contains(head))
            .map(|(row, _)| row)
            .collect();
        let mut row = 0;
        rows.retain(|_| {
            row += 1;
            !dropped.contains(&(row - 1))
        });
    }
}

/// The `references`-query companions of a `@ref.call` naming the **type** and
/// the **trait** of a fully qualified call `<T as Tr>::m()` (S-606, [CR-202]):
/// the row records both ([`qualified_call_target`]) where the plain path rules
/// would strip the bracket and record the bare `m`.
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
const QUALIFIED_TYPE_CAPTURE: &str = "ref.call.qualified.type";
/// See [`QUALIFIED_TYPE_CAPTURE`].
const QUALIFIED_TRAIT_CAPTURE: &str = "ref.call.qualified.trait";
/// The `references`-query companion of a `@ref.call` naming the **type** of a
/// qualified self that names no trait, `<T>::m()` (S-627, [CR-205]): the row
/// records the path the call writes ([`bracketed_call_target`]) — `T::m` — where
/// the plain path rules would strip the bracket and record the bare `m`. The
/// capture is the bracket's one child, so it also matches the `qualified_type`
/// of `<T as Tr>::m()`, which [`qualified_call`] leaves to its own captures.
///
/// [CR-205]: ../../../docs/requests/CR-205-qualified-self-calls-and-keyword-named-macro-calls-bind-as-rust-reads-them.md
const QUALIFIED_BARE_CAPTURE: &str = "ref.call.qualified.bare";

/// The `references`-query companion of a `@ref.call` naming the **constant**
/// a call is made on (S-565, [FR-RS-33]): Ruby's `Foo.bar` captures `bar` as
/// the call and `Foo` here, and the row records the Path `Foo::bar` — the
/// constant's path as [`type_path`] reads it (`A::B.bar` → `A::B::bar`, `::A.bar`
/// headed [`FULLY_QUALIFIED_HEAD`](crate::resolve::FULLY_QUALIFIED_HEAD)), then
/// the call's own segments. A companion naming no constant path leaves the
/// call's segments as they are.
///
/// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
const CALL_SCOPE_CAPTURE: &str = "ref.call.scope";

/// The `references`-query companion of a `@ref.call` / `@ref.method` naming
/// the expression a call is written as when no argument list of its own
/// follows it (S-573, [FR-RS-36]): Scala's parameterless member selection
/// `xs.size` and its infix call `a max b`. Its row records no argument count —
/// uniform access writes `def size` and `def size()` alike — and a selection
/// an argument list applies (`xs.m(1)`, `xs.m[T](1)`) or an assignment writes
/// (`a.x = v`) is no such call: the query matches every selection, and those
/// are dropped ([`is_unapplied_selection`]).
///
/// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
const SELECTION_CAPTURE: &str = "ref.selection";

/// The `references`-query companion of a `@ref.access` naming the selector's
/// operand (S-529, [FR-RS-18]), for a language whose method names its own
/// instance by a receiver parameter (Go's `c` in `func (c *ioCore) …`) and so
/// cannot anchor the access to it in the query. The row is kept only when the
/// operand's text is the enclosing declaration's `self_name` (the
/// [`receiver`] marker) and no argument list applies the selection
/// ([`is_unapplied_selection`]): `c.enc` is the receiver's field, `c.clone()`
/// a call, and `other.enc` another value's.
///
/// [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
const ACCESS_OPERAND_CAPTURE: &str = "ref.access.operand";

/// The `references`-query companion of a `@ref.access` naming the selection it
/// reads (S-580, C++'s `this->x`), for a grammar that spells a call on a member
/// and a read of it alike (`this->f()`, `this->f`): the row is dropped when an
/// argument list applies the selection ([`is_applied`]). Unlike
/// [`ACCESS_OPERAND_CAPTURE`] it keeps an assignment's target — C++ has no
/// setter — and asks no receiver name. TypeScript, TSX and JavaScript use it for
/// `this.f()` too (S-542).
const ACCESS_SELECTION_CAPTURE: &str = "ref.access.selection";

/// The `references`-query marker, in a `@ref.use` match, of a `use` that is
/// **exported** — a Rust `pub use`, any `pub(…)` included (S-606, [CR-202]):
/// each row of the declaration records [`RefFact::exported`].
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
const USE_EXPORTED_CAPTURE: &str = "ref.use.exported";

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
/// it records no row of its own. A wildcard row's alias is a scope marker, so
/// [`import_row`] never lets it take this one — a guard against a droppable
/// query that captures it on a wildcard match; no shipped query does.
const IMPORT_ALIAS_CAPTURE: &str = "ref.import.alias";

/// The `references`-query marker of an import whose specifier is relative to
/// the importing file's directory whatever it spells (S-566, [FR-RS-34]):
/// Ruby's `require_relative 'abstract/handler'`. Under a path grammar the row
/// records the `.`-headed target `./abstract/handler` would
/// ([`relative_specifier_segments`]). Like [`FROM_MODULE_CAPTURE`] it records
/// no row of its own.
///
/// [FR-RS-34]: ../../../docs/specs/requirements/FR-RS-34.md
const IMPORT_RELATIVE_CAPTURE: &str = "ref.import.relative";

/// The `references`-query marker of an import that names declarations rather
/// than a file, in a language whose other imports are paths (S-579,
/// [FR-RS-38]): C++'s `using ns::f;` and `using namespace ns;`, beside its
/// `#include` paths. Its text splits as a member path ([`import_segments`]); a
/// directive carries `ref.import.asterisk` too and is a `Glob` row. Like
/// [`FROM_MODULE_CAPTURE`] it records no row of its own.
///
/// [FR-RS-38]: ../../../docs/specs/requirements/FR-RS-38.md
const IMPORT_USING_CAPTURE: &str = "ref.import.using";

/// The `references`-query marker of an import that names a file rather than
/// declarations, in a language whose other imports are names (S-551,
/// [FR-RS-26]) — the converse of [`IMPORT_USING_CAPTURE`]: PHP's `require`,
/// `require_once`, `include` and `include_once` of a literal path. Its text is
/// read as a file path beside the including file ([`include_file_segments`]),
/// never split as a `\` member path, and its row carries no alias: an include
/// binds no name. Like [`FROM_MODULE_CAPTURE`] it records no row of its own.
///
/// [FR-RS-26]: ../../../docs/specs/requirements/FR-RS-26.md
const IMPORT_FILE_CAPTURE: &str = "ref.import.file";

/// The marker of an [`IMPORT_FILE_CAPTURE`] include whose path is appended to
/// the including file's directory (S-551): PHP's `__DIR__ . '/lib/a.php'`.
const IMPORT_FILE_UNDER_DIR_CAPTURE: &str = "ref.import.file.dir";

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
    /// The argument counts this callable admits (S-591, [FR-EX-32]), from its
    /// plugin's `@arity.*` captures ([`arity`]): a receiver parameter is not
    /// counted, a defaulted one raises only the maximum, a variadic one makes it
    /// unbounded. `None` — unknown — for every non-callable node, for a callable
    /// of a plugin that declares no captures, and for a parameter list holding a
    /// form the plugin cannot count. Recorded beside the symbol, never in it.
    ///
    /// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
    pub params: Option<ParamRange>,
    /// Whether a Rust `impl` function takes `self` ([CR-200]): `true` when its
    /// first parameter is a receiver (`self`, `&self`, `&'a mut self`,
    /// `self: Box<Self>`, …), `false` for an associated function such as
    /// `fn new() -> Self`. `false` too for a callable its plugin marks
    /// `@item.receiver.none` (S-565: Ruby's `def self.x`, `class << self`
    /// methods, `module_function`s). `None` for every other node — a free
    /// function, a trait's method, every other node of another language.
    ///
    /// [CR-200]: ../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
    pub takes_self: Option<bool>,
    /// How the callable writes its receiver (S-606, [FR-EX-34]), recorded
    /// wherever [`takes_self`](Self::takes_self) is: [`ReceiverMode::None`]
    /// exactly when it is `false`, the mode its plugin's `@item.receiver.*`
    /// capture names when `true` — `None` (unknown) when no capture names one.
    ///
    /// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
    pub receiver_mode: Option<ReceiverMode>,
    /// An enum's variant names in declaration order, space-joined (S-606,
    /// [FR-EX-34]); `Some("")` for an enum with none, `None` for every other
    /// node and for an enum whose plugin captures no `@item.variants`.
    ///
    /// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
    pub variants: Option<String>,
    /// `true` for a **required signature** (S-606, [CR-202] F3): a member its
    /// container's implementors supply, with no body of its own — a Rust
    /// trait's `fn m(&self);` or a Go interface's method spec (S-525), marked by
    /// an `@item.signature` capture. Its
    /// node records `has_body = false`, its range and whether it takes `self`.
    /// Also `true` for a C-family prototype paired with a definition in its own
    /// file (S-577, `definition`): a bodyless declaration that, like a required
    /// signature, binds nothing while the body it declares is a node.
    ///
    /// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
    pub signature: bool,
    /// `true` for an interface member a type implementing the interface does
    /// not inherit (S-609, [FR-RS-48]) — a Java `static` or `private` interface
    /// method, a Kotlin `private` interface `fun` — marked by an
    /// `@item.uninherited` capture. A supertype walk never binds one through
    /// an implementing type; the interface's own members still include it.
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    pub uninherited: bool,
    /// `true` for a Go type declared with `=` (S-525, [FR-EX-15]) — `type A = B`
    /// is another name for `B`, where `type L int8` defines a new type. Marked by
    /// an `@item.alias` capture; every other node, and every plugin that
    /// captures none, is `false`. An extraction fact only: no `nodes` column
    /// holds it yet, so it does not survive into the store.
    ///
    /// [FR-EX-15]: ../../../docs/specs/requirements/FR-EX-15.md
    pub alias: bool,
    /// A type declaration's generic arity (S-602, [CR-199]): the parameters of
    /// the type-parameter list its plugin captures with `@item.type_parameters`
    /// (`assoc`), 0 when none is captured — `Result<T>` is 1, `Result` 0.
    /// Recorded for every type-like node of a plugin that declares
    /// `[module_model] type_arity`, and `None` for every other node and every
    /// other plugin. Recorded beside the symbol, never in it.
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    pub type_arity: Option<u32>,
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
    /// The in-scope name an import binds (`use a::b as c` → `c`); a glob's or a
    /// relation's scope marker (`STATIC_WILDCARD_ALIAS`, `TRAIT_USE_ALIAS`, …);
    /// the import binding a qualified call went through (S-536, S-544); a bare
    /// call's local-binding mark (`LOCAL_VALUE_ALIAS`, `LOCAL_IMPORT_ALIAS`,
    /// S-594). `None` for every other row.
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
    /// The wrappers a proven receiver's declared type was peeled of to reach
    /// the type its Path-form `T::m` row names (S-587, [FR-RS-42]), outermost
    /// first and space-joined (`&`, `&mut`, `Box`, `Arc`, `Rc`: `"& Arc"` for
    /// `&Arc<T>`); `None` for every other row, and for a receiver declared `T`
    /// itself. Part of the ledger identity: a call through `Arc<T>` and one
    /// through `T` bind differently.
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub peeled: Option<String>,
    /// How many arguments a `Calls` row's call passes (S-591, [FR-EX-32]), from
    /// its plugin's `@arity.*` captures ([`arity`]): a trailing lambda counts as
    /// one, a Scala call counts its first argument list only. `None` — unknown —
    /// for a spread argument, a form the plugin cannot count (a call in a
    /// macro's token tree whose tokens may hide a comma, `refs::macro_call_refs`),
    /// and every non-call row but two: a partial type part's own `Extends` row
    /// (S-557) records the declaration's type-parameter count here, its arity,
    /// and a supertype's or a `new`'s row of a language that declares
    /// `[module_model] type_arity` the type-argument count it writes (S-602,
    /// [`written_arity`]). Part of the ledger identity:
    /// `f(a)` and `f(a, b)` from one caller are two rows that may bind
    /// differently, and so are `: Result` and `: Result<int>`.
    ///
    /// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
    pub arg_count: Option<u32>,
    /// Whether an `Imports` row's declaration is exported — a Rust `pub use`,
    /// any `pub(…)` visibility included (S-606, [FR-EX-34]): the re-export a
    /// path through this module may follow. `Some(false)` for a private one,
    /// `None` for every other row and for an import whose plugin marks no
    /// `@ref.use.exported`. A TypeScript-family `export` statement's rows
    /// ([`ts_export_rows`], S-534) are `Some(true)`, set by the engine from the
    /// statement's shape rather than by a marker: a `Glob` row is `export *
    /// from`, a multi-segment `Path` row a named re-export, a single-segment
    /// `Path` row a local export alias. Not part of the ledger identity: one declaration
    /// is either exported or not.
    ///
    /// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
    pub exported: Option<bool>,
}

/// The extraction result for a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The file's project-relative path (echoes [`FileInput::path`]).
    pub path: String,
    /// The grammar/plugin name that parsed the file (e.g. `rust`).
    pub language: String,
    /// `true` when extraction was partial ([FR-IX-04]): the parse tree
    /// contained a syntax error, or the grammar misread a declaration without
    /// reporting one ([`MISREAD_CAPTURE`], S-559).
    pub partial: bool,
    /// `true` when the parse tree contained an ERROR or MISSING node —
    /// tree-sitter's `has_error` — the file's **parse damage** ([FR-IX-14]),
    /// persisted as `files.parse_damaged`. Narrower than
    /// [`partial`](Self::partial): a misread with no parse error is not one.
    ///
    /// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
    pub parse_damaged: bool,
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
    /// `Some("")` for the global namespace. For a file of a directory-package
    /// language (S-528, [FR-RS-17]: Go), the `package` clause it declares,
    /// which places it among its directory's files and keys nothing. `None`
    /// for every file of any other module model, for one whose top-level
    /// declarations sit in two different namespaces
    /// ([`declared_types::file_namespace`]) — such a file keeps the default
    /// module key — and for a directory-package file whose clause was not
    /// captured.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    /// [FR-RS-17]: ../../../docs/specs/requirements/FR-RS-17.md
    pub namespace: Option<String>,
    /// Every impl block this file declares, an empty one included, in source
    /// order (S-606, [CR-202] F1): its self type, reference flag, trait and
    /// `Deref` target ([`ImplBlockFact`]). Empty for a file whose plugin
    /// captures no `@item.impl`.
    ///
    /// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
    pub impl_blocks: Vec<ImplBlockFact>,
    /// Every name a callable body of this file binds locally (S-593,
    /// [FR-EX-33]): its kind, the scope it binds in and its position, sorted
    /// by position ([`LocalBindingFact`]). Empty for a file whose plugin
    /// declares no `locals` capability. A bare call or a receiver a binding in
    /// force shadows is decided by it (S-594, `local_shadow`).
    ///
    /// [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md
    pub local_bindings: Vec<LocalBindingFact>,
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
    /// The parameter list the `@arity.*` captures give this declaration
    /// (S-591), `None` when they give it none. Never part of the symbol.
    params: Option<arity::Params>,
    /// The receiver mode, variant names and signature marker the `@item.*`
    /// captures give this declaration (S-606). Never part of the symbol.
    item: assoc::DeclItemFacts,
    /// What the `@decl.*` captures say about this declaration (S-577).
    def: definition::DeclDefinitionFacts,
    /// The scope path an out-of-line definition renders into its own symbol
    /// when the file declares no container it names (S-577,
    /// [`definition::complete`]). Empty for every other declaration.
    qualifier: Vec<String>,
    /// The declaration whose symbol and node this one completes: an
    /// out-of-line body merged into its in-class prototype (S-577). It emits no
    /// node of its own; its body is the one the prototype's node carries.
    completes: Option<usize>,
    /// An attribute an assignment declares ([`ATTRIBUTE_CAPTURE`], S-543):
    /// taken at its name, owned by its class, once per class.
    attribute: bool,
    /// The declaration is state of the nearest enclosing class or module
    /// ([`STATE_CAPTURE`], S-564): its node is its name alone, so it owns no
    /// scope and no reference, and [`settle_state`] keeps one per name.
    state: bool,
    /// Where the `@owner.*` captures place this declaration (S-539): by the
    /// name its assignment gives, not by nesting ([`owner::place`]). A placed
    /// declaration owns no scope. `None` for every other declaration.
    owner: Option<owner::Owner>,
    /// What the `@member.*` captures say about this declaration (S-582).
    member: member::DeclMemberFacts,
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
        parse_damaged: false,
        nodes: Vec::new(),
        edges: Vec::new(),
        refs: Vec::new(),
        warnings: Vec::new(),
        config_source: None,
        forwarding: Vec::new(),
        declared_types: Vec::new(),
        namespace: None,
        impl_blocks: Vec::new(),
        local_bindings: Vec::new(),
    };

    // A grammar that fails to bind (ABI skew) is skipped-and-warned, never fatal.
    if parser.set_language(plugin.language()).is_err() {
        facts.warnings.push(format!(
            "grammar '{}' failed to bind; file skipped",
            plugin.name()
        ));
        return facts;
    }

    let text = preprocess::reduce(plugin.semantics().preprocessor, &input.source);
    let Some(tree) = parser.parse(text.as_ref(), None) else {
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
        facts.parse_damaged = true;
        facts
            .warnings
            .push("syntax error(s) present; partial extraction".to_string());
        facts.warnings.len() - 1
    });

    let Some(query) = plugin.query("symbols") else {
        // No symbols capability → nothing to extract, but not an error.
        return facts;
    };

    let source = text.as_bytes();

    // 1) Collect declarations from the query matches.
    let (mut decls, package_statement, namespaces, damage, impl_blocks) =
        collect_decls(query, tree.root_node(), source, &plugin.semantics().body_node_kinds);
    facts.impl_blocks = impl_blocks;
    if let Some(i) = partial_warning {
        damage.annotate(&mut facts.warnings[i]);
    } else if let Some(warning) = damage.silent_warning() {
        facts.partial = true;
        facts.warnings.push(warning);
    }

    // 2) Resolve parent scopes, complete the declarations one another defines
    // (S-577) and 3) assign canonical-sort ordinals.
    assign_parents(&mut decls);
    settle_attributes(&mut decls);
    let completion = definition::complete(&mut decls, source, &plugin.semantics().body_node_kinds);
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
        // A directory-package language (S-528, FR-RS-17) records its `package`
        // clause in the same column: what places the file among its
        // directory's files, never a key.
        if plugin.semantics().directory_packages {
            facts.namespace = package_statement.clone();
        }
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
            facts.nodes.push(file_module_node(
                sym.clone(),
                file_module_name(&path_segments, &plugin.semantics().package_stems),
                input.source.lines().count().max(1) as u32,
            ));
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
    // What the whole file says about its tests, read once for every declaration.
    let mut test_facts = testmarker::FileFacts::default();
    let type_arity = plugin.semantics().type_arity;
    // Trait-implementation reference rows (S-281, CR-073, FR-RS-08): one per
    // `impl T for X` method, linking the impl method to its trait so the binder
    // can enumerate a trait method's impls for `dyn T` fan-out. Collected here
    // (the AST is in hand) and appended to the reference set after the query walk.
    let mut impl_refs: Vec<RefFact> = Vec::new();
    for (i, decl) in decls.iter().enumerate() {
        // A merged out-of-line definition emits as the declaration it completes,
        // whose node carries its body: span, metrics and evidence (S-577).
        let Some(symbol) = symbols[i].as_ref().filter(|_| decl.completes.is_none()) else {
            continue;
        };
        let body = completion.bodies.get(&i).map_or(decl, |&d| &decls[d]);
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
        // trait `T` ([`trait_impl_ref`], S-281, CR-073).
        if is_rust_method {
            impl_refs.extend(trait_impl_ref(decl.node, source, symbol, decl.start_line));
        }
        let is_callable = matches!(node_kind, NodeKind::Function | NodeKind::Method);
        let metrics = is_callable.then(|| function_metrics(body, keywords, body_kinds));
        let (takes_self, receiver_mode) = receiver_facts(decl, is_rust_method, is_callable);
        facts.nodes.push(NodeFact {
            symbol: symbol.clone(),
            kind: node_kind,
            name: decl.name.clone(),
            start_line: body.start_line,
            end_line: body.end_line,
            metrics,
            // The S-014 annotation inputs, captured while the AST is in hand.
            exported: shape::is_exported(decl.node, &decl.name, export_convention, source),
            fingerprint: is_callable.then(|| shape::shape_fingerprint(body.node, &facts.language)),
            // S-027 / FR-EX-06: language-native test-marker evidence, captured
            // in the same AST-in-hand pass. `test_evidence` itself gates on a
            // callable kind (`Function`/`Method` alike), so it is `false` for
            // every non-function node.
            test_evidence: testmarker::test_evidence(
                body.node,
                &decl.name,
                node_kind,
                &input.path,
                test_convention,
                source,
                &mut test_facts,
            ),
            // Code declarations carry no FTS body (FR-DG-05 indexes DocSection
            // prose only); the code itself is read on demand, not searched here.
            body: None,
            // CR-005 structural facts, captured in the same AST-in-hand pass and
            // gated on a callable kind: the max block-nesting depth (FR-EX-07)
            // and the winnowed near-clone shingle set (FR-EX-09).
            max_nesting_depth: is_callable
                .then(|| nesting::max_nesting_depth(body.node, block_kinds)),
            shingles: if is_callable {
                shingle::shingles(body.node)
            } else {
                Vec::new()
            },
            self_type: decl.self_type.clone(),
            // S-591 / FR-EX-32: the range a callable admits; CR-200 / S-606:
            // whether it takes `self`, and how ([`receiver_facts`]).
            params: decl.params.filter(|_| is_callable).and_then(|p| p.range),
            takes_self,
            receiver_mode,
            variants: decl.item.variants.clone(),
            signature: decl.item.signature || decl.def.paired,
            uninherited: decl.item.uninherited,
            alias: decl.item.alias,
            // S-602: a type's generic arity, where its plugin tells types apart by it.
            type_arity: (type_arity && crate::resolve::is_type_like(node_kind)).then(|| decl.item.type_parameters.unwrap_or(0)),
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

    // Each prototype's link to the definition it pairs with (S-577).
    let paired = completion.link_pairs(&symbols, &mut facts.edges);

    // The file's top-level types, read off the nodes and edges just emitted (S-472).
    facts.declared_types = declared_types::source_types(&facts, package.as_deref(), layout);

    // The names each callable body binds locally (S-593): facts only, none without `locals`.
    facts.local_bindings = plugin.query("locals").map_or_else(Vec::new, |q| locals::collect(q, tree.root_node(), source));
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
            &namespaces,
            plugin.semantics(),
            &local_shadow::LocalShadows::new(&facts.local_bindings, &decls, plugin.semantics().separate_call_namespace),
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

    sort_facts(&mut facts);
    definition::hoist_definitions(&mut facts.nodes, &paired);
    facts
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
    let id_to_idx = decl_index(decls);
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
/// `(source, target, form, kind, relation, receiver, alias, peeled, args)` — the same reference
/// on two lines is one ref, first wins — then sort into that canonical order
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
/// The import alias (S-597, [FR-DB-07]) is part of it too: `from m import X
/// as A` and `… as B` share every other component and bind through different
/// local names, so both must reach the ledger. A row with no alias keys exactly
/// as before.
///
/// The peeled wrappers (S-587) complete the identity: a caller's `a.m()` with
/// `a: Arc<T>` and `b.m()` with `b: T` both record `T::m`, and a method the
/// wrapper itself provides binds through one and not the other. A row that
/// peeled nothing keys exactly as before.
///
/// The argument count (S-591, [FR-EX-32]) completes it: a caller's `f(a)` and
/// `f(a, b)` share every other component, and a binder that admits a candidate
/// by its parameter range binds them differently. An unknown count keys apart
/// from every known one. The key, the writer's `ON CONFLICT` target
/// (`insert_unresolved_ref`) and the unique index of migration 33 name the same
/// nine components.
///
/// The export mark of an import (S-606) is outside the identity, so two rows
/// that differ only by it are one row — exported when either is: a module that
/// writes both `use m::*;` and `pub use m::*;` (or a `pub use` and a `use` of
/// one path under two `#[cfg]`s) re-exports it, whichever comes first.
///
/// Shared by the code [`collect_refs`] and the documentation extractor
/// ([`doc`], S-035) so both passes produce byte-identical, order-independent
/// ledger input.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
/// [FR-DB-07]: ../../../docs/specs/requirements/FR-DB-07.md
/// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
pub(super) fn dedup_sort_refs(refs: &mut Vec<RefFact>) {
    // The relation token (`None` for a plain code/doc reference) completes the
    // ledger identity; a `&'static str` keeps the key allocation-free.
    let relation_token =
        |r: &RefFact| -> Option<&'static str> { r.relation.map(crate::model::ArtifactRelation::as_str) };
    // The alias is normalised as the index's `COALESCE(alias, '')` does, so a
    // missing alias and an empty one are one identity here exactly as in the
    // ledger.
    let alias_token = |r: &RefFact| -> String { r.alias.clone().unwrap_or_default() };
    // The peeled wrappers likewise (`COALESCE(peeled, '')`, S-587).
    let peeled_token = |r: &RefFact| -> String { r.peeled.clone().unwrap_or_default() };
    // `(source, target, form, kind, relation, receiver, alias, peeled, args)`.
    type LedgerKey = (String, String, i32, i32, Option<&'static str>, Option<i32>, String, String, Option<u32>);
    let mut kept: HashMap<LedgerKey, usize> = HashMap::new();
    let mut deduped: Vec<RefFact> = Vec::with_capacity(refs.len());
    for r in refs.drain(..) {
        let key = (
            r.source.as_str().to_string(),
            r.target.clone(),
            r.form.as_i32(),
            r.kind.as_i32(),
            relation_token(&r),
            r.receiver.map(ReceiverShape::as_i32),
            alias_token(&r),
            peeled_token(&r),
            r.arg_count,
        );
        match kept.get(&key) {
            Some(&at) => {
                let first = &mut deduped[at].exported;
                *first = match (*first, r.exported) {
                    (None, mark) | (mark, None) => mark,
                    (Some(a), Some(b)) => Some(a || b),
                };
            }
            None => {
                kept.insert(key, deduped.len());
                deduped.push(r);
            }
        }
    }
    *refs = deduped;
    refs.sort_by(|a, b| {
        (
            a.source.as_str(),
            &a.target,
            a.form.as_i32(),
            a.kind.as_i32(),
            relation_token(a),
            a.receiver.map(ReceiverShape::as_i32),
            a.alias.as_deref().unwrap_or_default(),
            a.peeled.as_deref().unwrap_or_default(),
            a.arg_count,
        )
            .cmp(&(
                b.source.as_str(),
                &b.target,
                b.form.as_i32(),
                b.kind.as_i32(),
                relation_token(b),
                b.receiver.map(ReceiverShape::as_i32),
                b.alias.as_deref().unwrap_or_default(),
                b.peeled.as_deref().unwrap_or_default(),
                b.arg_count,
            ))
    });
}

/// The synthetic file-module node `symbol` names, spanning lines 1 to
/// `end_line`. It is bookkeeping, not a declaration: never a dead-code
/// candidate nor an exported root, with no test-marker evidence (S-027 —
/// evidence is per function only), no FTS body (only DocSection prose is
/// body-indexed, FR-DG-05), and — not being a function — no nesting depth,
/// shingles (CR-005) or callable fact.
fn file_module_node(symbol: LogosSymbol, name: String, end_line: u32) -> NodeFact {
    NodeFact {
        symbol,
        kind: NodeKind::Module,
        name,
        start_line: 1,
        end_line,
        metrics: None,
        exported: false,
        fingerprint: None,
        test_evidence: false,
        body: None,
        max_nesting_depth: None,
        shingles: Vec::new(),
        self_type: None,
        params: None,
        takes_self: None,
        receiver_mode: None,
        variants: None,
        signature: false,
        uninherited: false,
        alias: false,
        type_arity: None,
    }
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
/// Path form, in place of its bare row (S-467, Java; S-587, Rust, which keeps
/// the `other` shape and records the wrappers it peeled), and every Method-form
/// row left bare records its receiver's shape (S-514, [FR-EX-13]).
///
/// A method call the query captures as [`SELF_RECEIVER_METHOD_CAPTURE`] is a
/// `self`-marked `@ref.method`: recorded as the Path-form `Self::m` of shape
/// `self` when its enclosing declaration has a recorded self type
/// ([`Decl::self_type`], S-493) — the target a written `Self::m()` records,
/// which the binder resolves through that self type, the shape keeping the
/// method syntax a written one lacks (S-606) — and otherwise as a Method-form
/// row of shape `self`.
///
/// A call on a constant whose query marks the constant beside its `@ref.call`
/// ([`CALL_SCOPE_CAPTURE`], S-565) is recorded qualified by it — Ruby's
/// `Foo.bar` as the Path `Foo::bar`, never the bare `bar`. A call written with
/// no argument list of its own ([`SELECTION_CAPTURE`], S-573) records no
/// argument count, and a bare call on the instance that the file shows no
/// member answers is turned into the free call ([`free_call`], S-573).
///
/// A fully qualified call `<T as Tr>::m()` whose query marks its type and trait
/// (`@ref.call.qualified.type` / `.trait` beside its `@ref.call`) is recorded
/// with both (S-606, [`qualified_call_target`]), never as the bare `m`; one
/// whose bracket names no trait, `<T>::m()` (`@ref.call.qualified.bare`), is
/// recorded as the path it writes (S-627, [`bracketed_call_target`]).
///
/// [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
#[allow(clippy::too_many_arguments)]
fn collect_refs(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
    namespaces: &[declared_types::NamespaceScope],
    semantics: &Semantics,
    local_shadows: &local_shadow::LocalShadows<'_>,
) -> Vec<RefFact> {
    let id_to_idx = decl_index(decls);
    // The innermost enclosing captured declaration whose symbol built, if any.
    // A declaration whose own symbol failed to build defers to the next
    // enclosing scope, and so does a value binding (`is_value_binding`, S-532):
    // `const util = require('./util')` is the file's import.
    let enclosing_decl = |node: Node<'_>| -> Option<usize> {
        let mut ancestor = node.parent();
        while let Some(n) = ancestor {
            if let Some(&idx) = id_to_idx.get(&n.id()) {
                if owns_references(&decls[idx], &symbols[idx]) {
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
    let imports = ImportBindings::collect(query, root, source, decls, semantics);
    // Receiver typing (S-467) and shapes (S-514) — only for a query that marks
    // receivers.
    let mut receivers = receiver::Receivers::for_query(capture_names, semantics);
    let mut out: Vec<RefFact> = Vec::new();
    // The argument captures (S-591) and each call row's captured callee, read
    // once the walk ends: a call's arguments may be matched after its callee.
    let mut arguments = arity::ArgCaptures::default();
    let mut callees: Vec<(usize, Node<'_>)> = Vec::new();
    // Each `@ref.call` node's row: a call two patterns capture — a fully
    // qualified `<T as Tr>::m()` (S-606) is also a plain path call — is one
    // row, whichever match comes first.
    let mut call_rows: HashMap<usize, usize> = HashMap::new();
    let mut shadows = CallShadows::default();
    let mut bare_accesses = bare_access::BareAccesses::default();
    let mut free_calls = free_call::FreeCalls::default();
    // A query that never marks an exported `use` says nothing of any (S-606).
    let marks_exports = capture_names.contains(&USE_EXPORTED_CAPTURE);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let node = cap.node;
            let capture = capture_names[cap.index as usize];
            // A `_`-prefixed capture is a predicate operand (`@_receiver`),
            // never a reference.
            // An `@arity.*` capture records no row of its own either (S-591).
            if capture.starts_with('_') || arguments.note(capture, node) {
                continue;
            }
            if shadows.note(capture, node, source) {
                continue;
            }
            // A receiver marker records no row of its own: it is read before
            // the per-row source lookup below (`self_name` asks only for its
            // enclosing declaration).
            if receiver::is_marker(capture) {
                if let Some(receivers) = receivers.as_mut() {
                    // An anchor marker reads its companions from its own match
                    // (S-587).
                    let companion = |name: &str| match_capture(m.captures, capture_names, name);
                    receivers.mark(capture, node, source, || enclosing_decl(node), companion);
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
                    let segments = rooted_call(node, text, call_segments(m.captures, capture_names, source, text));
                    let Some(counted) = counted_call(m.captures, capture_names).filter(|_| !segments.is_empty()) else {
                        continue;
                    };
                    // A fully qualified call records its type and trait (S-606).
                    let qualified = qualified_call(m.captures, capture_names, source, &segments);
                    if let Some(&row) = call_rows.get(&node.id()) {
                        if let Some(qualified) = qualified {
                            out[row].target = qualified;
                        }
                        continue;
                    }
                    call_rows.insert(node.id(), out.len());
                    shadows.head(out.len(), &segments);
                    let local = local_shadows.bare(node, &segments, qualified.is_some()); // S-594
                    let target = qualified.unwrap_or_else(|| segments.join("::"));
                    // A JSX tag naming a local value (`const Icon = icons[k];
                    // <Icon />`) renders that value: it calls no component
                    // (S-440), so it records no reference at all.
                    let jsx = node.parent().is_some_and(|p| p.kind().starts_with("jsx_"));
                    if jsx && imports.locally_bound(node, &target) {
                        continue;
                    }
                    // A call through a named import is recorded qualified by the
                    // module it was imported from (S-440, `ImportBindings`).
                    let target = imports.named_target(node, &target).filter(|_| local.is_none()).map_or(target.clone(), str::to_string);
                    if let (Some(receivers), None) = (receivers.as_mut(), local) {
                        receivers.site(out.len(), node.parent(), enclosing_decl(node));
                    }
                    callees.extend(counted.then_some((out.len(), node)));
                    out.push(RefFact {
                        source: source_symbol,
                        target,
                        alias: local.map(local_shadow::Shadow::alias),
                        form: RefForm::Path,
                        kind: EdgeKind::Calls,
                        line,
                        relation: None,
                        receiver: None,
                        peeled: None,
                        arg_count: None,
                        exported: None,
                    });
                }
                "ref.method" | SELF_RECEIVER_METHOD_CAPTURE => {
                    let name = generic_callee_name(node, source).unwrap_or(text).trim();
                    let Some(counted) = counted_call(m.captures, capture_names).filter(|_| !name.is_empty()) else {
                        continue;
                    };
                    // `@ref.method.self` is a `self`-marked `@ref.method` (S-514).
                    let self_marked = capture == SELF_RECEIVER_METHOD_CAPTURE;
                    if let (Some(receivers), true) = (receivers.as_mut(), self_marked) {
                        receivers.mark_self(node.parent());
                    }
                    // A member call whose receiver is an import — a Go package, a TS
                    // namespace or named import (S-536) — is a qualified path, not a
                    // receiver-method call (S-440, `ImportBindings`).
                    callees.extend(counted.then_some((out.len(), node)));
                    if let Some((module, alias)) = imports.qualifier_of(node, source) {
                        out.push(RefFact {
                            source: source_symbol,
                            target: format!("{module}::{name}"),
                            // The import binding it went through: Python's (S-544), a TS named import's (S-536).
                            alias: alias.map(str::to_string),
                            form: RefForm::Path,
                            kind: EdgeKind::Calls,
                            line,
                            relation: None,
                            receiver: None,
                            peeled: None,
                            arg_count: None,
                            exported: None,
                        });
                        continue;
                    }
                    // Trait-object dynamic dispatch (S-281, CR-073, FR-RS-08): when
                    // the receiver is a *provable* `&dyn T` (an explicit parameter
                    // or `let` type in the enclosing fn) — or `impl T`, or a
                    // generic parameter bounded by `T` (S-608, FR-RS-47) —
                    // qualify the target as `T::f` (`A+B::f` for several bounds)
                    // so the binder fans out to the trait method's impls. A
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
                        peeled: None,
                        arg_count: None,
                        exported: None,
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
                // binds no name — `b` is not a name the file can now use; its
                // alias, if any, marks its scope (static, global, or the
                // namespace block it is written in, [`block_alias`], S-603).
                // `ref.import.global` marks one whose scope is every file of
                // the declaring file's directory (a C# `global using`). A match
                // carrying `ref.import.from` names its module apart from the
                // imported name (Python's `from m import a`, S-519), and one
                // carrying `ref.import.alias` names the local binding, which
                // [`import_row`] records as the row's alias (S-520).
                "ref.import" => {
                    let Some((segments, form, alias)) =
                        import_row(m.captures, capture_names, source, text, semantics)
                    else {
                        continue;
                    };
                    let is_static = match_capture(m.captures, capture_names, "ref.import.static").is_some();
                    if let (Some(receivers), RefForm::Path, Some(name), false) =
                        (receivers.as_mut(), form, alias.as_deref(), is_static)
                    {
                        receivers.type_import(name);
                    }
                    out.push(RefFact {
                        source: source_symbol,
                        alias: alias.or_else(|| block_alias(form, node, namespaces, decls, semantics)),
                        target: segments.join("::"),
                        form,
                        kind: EdgeKind::Imports,
                        line,
                        relation: None,
                        receiver: None,
                        peeled: None,
                        arg_count: None,
                        exported: None,
                    });
                }
                // A TypeScript `export` statement (S-534, [`ts_export_rows`]):
                // a re-export is an import marked exported, and a local export
                // under another name records the module's export alias.
                "ref.export" => {
                    out.extend(ts_export_rows(node, source, &imports, &source_symbol, line, semantics));
                }
                // A member-access fact (S-042, CR-005, FR-EX-08): a method body
                // reads a field of its own class-like container (`self.x`,
                // `this.x`). The captured node is the field-name token; the
                // receiver-anchored capture pattern in `references.scm` already
                // restricted it to an own-field access, or [`access_rows`] does
                // (S-529, a named receiver). Resolution binds it to an
                // `Accesses` edge only when exactly one Field candidate matches in
                // the enclosing container — ambiguous/unmatched stays in the
                // ledger (NFR-RA-05). The `Method` form carries the bare-name
                // semantics the binder's member-access path expects.
                "ref.access" => {
                    let row = access_row(source_symbol, text.trim().to_string(), line);
                    access_rows(&mut out, receivers.as_mut(), row, (m.captures, capture_names), source, || enclosing_decl(node));
                }
                // S-556, S-568: rows follow the walk
                bare_access::ACCESS_CAPTURE | bare_access::CALL_CAPTURE | bare_access::SHADOW_CAPTURE => {
                    bare_accesses.note(capture, node, source, decls, symbols, || enclosing_decl(node));
                }
                // Calls nested inside a macro invocation's token tree (S-162,
                // CR-043), attributed to the macro's enclosing declaration —
                // see [`macro_rows`].
                "ref.macro" => {
                    push_macro_rows(&mut out, receivers.as_mut(), (node, local_shadows), source, enclosing_decl(node), decls, &source_symbol);
                }
                // A type relation (S-466, CR-149 §3.2 B, FR-EX-10): the captured
                // node is a TYPE, recorded as a Path-form row of the capture's
                // kind — never Method form, whose `::` target the binder
                // reserves for trait-object dispatch (S-281). Its type
                // arguments are type uses of the same declaration(s). A used
                // trait (S-522, `ref.implements.trait`) and a partial type part
                // (S-557, `ref.extends.partial`) are marked on their rows. A
                // type named through a named import is recorded qualified by
                // its module, as a call through one is (S-535).
                name @ ("ref.extends" | "ref.implements" | "ref.instantiates" | "ref.type_use") => {
                    let owners =
                        relation_owners(name, declarator_symbols(node), enclosing_decl(node), decls, symbols, source_symbol);
                    let marker = relation_marker(m.captures, capture_names, node, semantics.type_arity);
                    let mut rows = type_relation_rows(name, &owners, node, source, line, marker);
                    imports.qualify_relations(node, &mut rows);
                    out.extend(rows);
                }
                // An import whose paths no single node spans (S-518; Scala's
                // `import_declaration`): the declaration is walked like a Rust
                // use-tree, one row per imported path.
                // A Rust `use` records whether it is a re-export (S-606): its
                // match carries `ref.use.exported` for a `pub use`.
                name @ ("ref.use" | "ref.import.dotted") => {
                    let exported = (name == "ref.use" && marks_exports)
                        .then(|| match_capture(m.captures, capture_names, USE_EXPORTED_CAPTURE).is_some());
                    out.extend(tree_import_rows(name, node, source, &source_symbol, line, exported));
                }
                // A free-call mark (S-573) records no row of its own; any
                // other capture this pass does not consume.
                _ => {
                    free_calls.note(capture, node, source);
                }
            }
        }
    }

    // Each call row's argument count (S-591). Rows the receiver pass retypes
    // keep it: the call is the same.
    for &(row, callee) in &callees {
        out[row].arg_count = arguments.count(callee, |n| id_to_idx.contains_key(&n.id()));
    }

    bare_accesses.finish(&mut out, receivers.as_mut()); // appended: no row the receiver pass indexes moves
    finish_receivers(receivers, &mut out, (root, source), decls, symbols, &imports, local_shadows);
    free_calls.finish(&mut out, &callees, decls, &id_to_idx); // reads the shapes just recorded
    shadows.apply(&mut out);
    // A CommonJS module's `module.exports` statements (S-540).
    out.extend(commonjs_export_rows(root, source, &imports, file_module, semantics));

    // Dedup on the ledger's uniqueness key, then canonical sort (NFR-RA-06).
    dedup_sort_refs(&mut out);
    out
}

/// Record one `@ref.access` row (S-042): pushed as it stands, unless its match
/// names the selector's operand ([`ACCESS_OPERAND_CAPTURE`], S-529). Such a row
/// is handed to the receiver pass, which keeps it only when the operand is the
/// `caller`'s `self_name` and no local binding rebinds that name at the operand
/// (S-594, [`receiver::Receivers::defer_access`]); one an
/// argument list applies (`c.clone()`) is a call, and a query naming operands
/// without marking receivers keeps none — never a guess ([NFR-RA-05]). A row
/// naming no field records nothing, and neither does one whose selection an
/// argument list applies ([`ACCESS_SELECTION_CAPTURE`], [`is_applied`]).
///
/// `matched` is the match's captures and their names, where the operand and the
/// selection are read.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn access_rows<'tree>(
    out: &mut Vec<RefFact>,
    receivers: Option<&mut receiver::Receivers<'tree>>,
    row: RefFact,
    matched: (&[QueryCapture<'tree>], &[&str]),
    source: &[u8],
    caller: impl FnOnce() -> Option<usize>,
) {
    let (captures, capture_names) = matched;
    if row.target.is_empty() || match_capture(captures, capture_names, ACCESS_SELECTION_CAPTURE).is_some_and(is_applied) {
        return;
    }
    let Some(operand) = match_capture(captures, capture_names, ACCESS_OPERAND_CAPTURE) else {
        out.push(row);
        return;
    };
    let unapplied = operand.parent().is_some_and(is_unapplied_selection);
    if let (true, Some(receivers), Ok(text)) = (unapplied, receivers, operand.utf8_text(source)) {
        receivers.defer_access(caller(), operand, text.trim(), row);
    }
}

/// The `Method`-form `Accesses` row of a member read: `self.x` / `this.x`
/// (S-042) and a bare `x` that names the class's own field (S-556,
/// [`bare_access`]) record the same row, which the binder's member-access path
/// binds to exactly one `Field` of the enclosing container.
fn access_row(source: LogosSymbol, name: String, line: u32) -> RefFact {
    RefFact {
        source,
        target: name,
        alias: None,
        form: RefForm::Method,
        kind: EdgeKind::Accesses,
        line,
        relation: None,
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: None,
    }
}

/// Whether a call row of the match `captures` is recorded, and whether its
/// argument count is (S-573): `None` for a [`SELECTION_CAPTURE`] an argument
/// list applies or an assignment writes — no call of its own —
/// `Some(false)` for one that is a call, whose count is unknown, and
/// `Some(true)` for every other call row, whose count is read.
fn counted_call(captures: &[QueryCapture<'_>], capture_names: &[&str]) -> Option<bool> {
    match match_capture(captures, capture_names, SELECTION_CAPTURE) {
        Some(selection) => is_unapplied_selection(selection).then_some(false),
        None => Some(true),
    }
}

/// The node kinds of a JavaScript function that binds its own `this` (S-542): a
/// `this.x = …` written inside one (`setTimeout(function () { this.t = 1; })`)
/// is not the instance's. An arrow function keeps the lexical `this`.
const THIS_REBINDING_KINDS: &[&str] = &["function_expression", "generator_function", "generator_function_declaration"];

/// Whether `node` sits inside a node of one of `kinds` that is itself inside
/// `scope` — the ancestors strictly between the two.
fn sits_within_any(node: Node<'_>, scope: Node<'_>, kinds: &[&str]) -> bool {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor.filter(|n| n.id() != scope.id()) {
        if kinds.contains(&n.kind()) {
            return true;
        }
        ancestor = n.parent();
    }
    false
}

/// Whether an argument list applies `selection` (S-580): it is the `function` of
/// a call, which names the callee and reads no value. A `@ref.access` whose
/// [`ACCESS_SELECTION_CAPTURE`] is so applied (`this->f()`) is a call, which the
/// method rows record, and records no access.
fn is_applied(selection: Node<'_>) -> bool {
    selection.parent().is_some_and(|call| {
        call.child_by_field_name("arguments").is_some()
            && call.child_by_field_name("function").is_some_and(|f| f.id() == selection.id())
    })
}

/// Whether the expression a [`SELECTION_CAPTURE`] names is a call of its own
/// (S-573): no argument list applies it — it is not the `function` of a call,
/// directly or through the type arguments of a generic one (`xs.m[T](1)`) —
/// and it is not the target an assignment writes (`a.x = v`, a setter).
fn is_unapplied_selection(selection: Node<'_>) -> bool {
    let mut callee = selection;
    while let Some(parent) = callee.parent() {
        if parent.kind() == "assignment_expression" {
            return parent.child_by_field_name("left").is_none_or(|left| left.id() != callee.id());
        }
        if parent.child_by_field_name("function").is_none_or(|f| f.id() != callee.id()) {
            return true;
        }
        if parent.child_by_field_name("arguments").is_some() {
            return false;
        }
        callee = parent;
    }
    true
}

/// The receiver pass over the finished rows ([`receiver`]): every site the walk
/// registered is typed or shaped, in place. A query that declares no receiver
/// marker has none, and nothing happens. A type a path-import language's typed
/// row names is qualified through the file's named imports (S-537), as its
/// type relations are.
fn finish_receivers<'tree>(
    receivers: Option<receiver::Receivers<'tree>>,
    out: &mut Vec<RefFact>,
    (root, source): (Node<'tree>, &[u8]),
    decls: &[Decl<'tree>],
    symbols: &[Option<LogosSymbol>],
    imports: &ImportBindings,
    shadows: &local_shadow::LocalShadows<'_>,
) {
    if let Some(receivers) = receivers {
        let id_to_idx = &decl_index(decls);
        let imported = |node: Node<'tree>, name: &str| imports.named_target(node, name).map(str::to_string);
        let file = receiver::FileDecls { root, source, decls, symbols, id_to_idx, imported: &imported, shadows };
        receivers.finish(out, &file);
    }
}

/// Push one macro invocation's rows onto `out`, each row whose receiver the
/// walk read registered with `receivers` to be typed after the walk, like a
/// query-captured one (S-610). `caller` is the index of the enclosing
/// declaration in `decls`. A bare call a local binding shadows is marked as
/// one outside a macro is (S-594, [`local_shadow`]): a token tree holds no
/// scope boundary, so the macro sees the bindings its invocation sees.
fn push_macro_rows<'tree>(
    out: &mut Vec<RefFact>,
    mut receivers: Option<&mut receiver::Receivers<'tree>>,
    (macro_node, shadows): (Node<'tree>, &local_shadow::LocalShadows<'_>),
    source: &[u8],
    caller: Option<usize>,
    decls: &[Decl<'_>],
    source_symbol: &LogosSymbol,
) {
    for (mut fact, read) in macro_rows(macro_node, source, caller.map(|i| &decls[i]), source_symbol) {
        if let (Some(receivers), Some(read)) = (receivers.as_deref_mut(), read) {
            receivers.macro_site(out.len(), macro_node, caller, read);
        }
        if fact.kind == EdgeKind::Calls && fact.form == RefForm::Path && fact.alias.is_none() {
            if let Some(local) = shadows.bare(macro_node, std::slice::from_ref(&fact.target), false) {
                local.mark(&mut fact);
            }
        }
        out.push(fact);
    }
}

/// The `Calls` rows of the calls nested inside one macro invocation's token
/// tree (S-162, CR-043): tree-sitter does not parse a macro body as
/// expressions, so the call/method-call query patterns cannot match inside it.
/// The token tree is walked in code ([`macro_call_refs`]) and each call emits
/// the same `Calls` path/method row it would outside the macro, attributed to
/// the macro's enclosing declaration `caller` — so a callee whose only call
/// site is a macro argument (`format!("{x}", x = activity_card(s))`,
/// `self.state.chip_class()`) is bound, or stays honestly unresolved, exactly
/// like any other call ([NFR-RA-05]).
///
/// Each row comes with the receiver the walk read for it, if any, for
/// [`push_macro_rows`] to hand to [`receiver::Receivers::macro_site`] (S-610).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn macro_rows(
    macro_node: Node<'_>,
    source: &[u8],
    caller: Option<&Decl<'_>>,
    source_symbol: &LogosSymbol,
) -> Vec<(RefFact, Option<MacroReceiver>)> {
    let mut out = Vec::new();
    for call in macro_call_refs(macro_node, source) {
        if call.target.is_empty() {
            continue;
        }
        // Each call records the row it records outside a macro. A trait-typed
        // receiver (`&dyn Tr`, `impl Tr`, a bounded generic) is
        // trait-qualified (S-281, S-608) and hands no receiver on to typing.
        // `self.f()` in a macro argument is the self row (S-493, S-514). Any
        // other method call is `other` (S-517): a shapeless row would no
        // longer merge with the query's `other` row of the same call, which
        // the shape-keyed dedup keeps apart. Its argument count (S-591) is the
        // token tree's, for the same reason.
        let trait_head = match &call.receiver {
            Some(MacroReceiver::Name(name)) if call.form == RefForm::Method => {
                receiver_trait_head(macro_node, name, source)
            }
            _ => None,
        };
        let (target, form, receiver, read) = if let Some(head) = trait_head {
            (format!("{head}::{}", call.target), RefForm::Method, None, None)
        } else if call.self_receiver {
            let (target, form, receiver) = receiver::self_call(caller, &call.target, true);
            (target, form, receiver, call.receiver)
        } else {
            let shape = (call.form == RefForm::Method).then_some(ReceiverShape::Other);
            (call.target, call.form, shape, call.receiver)
        };
        let fact = RefFact {
            source: source_symbol.clone(),
            target,
            alias: None,
            form,
            kind: EdgeKind::Calls,
            line: call.line,
            relation: None,
            receiver,
            peeled: None,
            arg_count: call.arg_count,
            exported: None,
        };
        out.push((fact, read));
    }
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
/// - A path specifier marked relative (`ref.import.relative`, S-566) is read
///   against the importing file's directory, `./` prepended; an unmarked one
///   of a load-path language is read against its load roots
///   ([`load_path_specifier_segments`]).
/// - An include-roots language's `#include` (S-579) names its header whole
///   ([`include_specifier_segments`]) and binds no name, so its row carries no
///   alias; a `using` beside it (`ref.import.using`) is a member path.
/// - A name-grammar language's file include (`ref.import.file`, S-551: PHP's
///   `require`) names its file whole beside the includer
///   ([`include_file_segments`]) and binds no name either.
fn import_row(
    captures: &[QueryCapture<'_>],
    capture_names: &[&str],
    source: &[u8],
    text: &str,
    semantics: &Semantics,
) -> Option<(Vec<String>, RefForm, Option<String>)> {
    let marked = |name: &str| captures.iter().any(|c| capture_names[c.index as usize] == name);
    let capture_text =
        |name: &str| match_capture(captures, capture_names, name).and_then(|n| n.utf8_text(source).ok());
    let segments = match (capture_text(FROM_MODULE_CAPTURE), semantics.import_specifier) {
        (Some(module), _) => {
            let mut segments = from_module_segments(module);
            if !marked("ref.import.asterisk") {
                segments.extend(import_segments(text));
            }
            segments
        }
        (None, _) if marked(IMPORT_USING_CAPTURE) => import_segments(text),
        (None, _) if marked(IMPORT_FILE_CAPTURE) => include_file_segments(text, marked(IMPORT_FILE_UNDER_DIR_CAPTURE)),
        (None, ImportSpecifier::Path) if semantics.bare_specifiers == BareSpecifiers::IncludeRoots => {
            include_specifier_segments(text)
        }
        (None, ImportSpecifier::Path) if marked(IMPORT_RELATIVE_CAPTURE) => {
            relative_specifier_segments(text, &semantics.specifier_extensions)
        }
        (None, ImportSpecifier::Path) if semantics.bare_specifiers == BareSpecifiers::LoadPath => {
            load_path_specifier_segments(text, &semantics.specifier_extensions)
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
        _ if semantics.bare_specifiers == BareSpecifiers::IncludeRoots && !marked(IMPORT_USING_CAPTURE) => None,
        _ if marked(IMPORT_FILE_CAPTURE) => None,
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

/// The alias of a namespace wildcard written inside a namespace block, in a
/// language that declares `enclosing_namespaces` (S-603, [CR-199]): the block's
/// namespace ([`block_wildcard_alias`](crate::resolve::block_wildcard_alias)),
/// read at the import's own position ([`declared_types::using_block`]) —
/// C#'s `namespace A { using X; … }` records `{A}` when that block encloses
/// every declaration of the file, one in a block enclosing none of them the
/// empty block `{}`, which the binder reads nowhere, and one in a block
/// enclosing only some of them nothing: it reads as before. `None` for any other row, and for a wildcard outside
/// every block, which reads as before. The binder reads the block's wildcards
/// at that namespace's level, before the next level out.
///
/// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
fn block_alias(
    form: RefForm,
    node: Node<'_>,
    namespaces: &[declared_types::NamespaceScope],
    decls: &[Decl<'_>],
    semantics: &Semantics,
) -> Option<String> {
    if form != RefForm::Glob || !semantics.enclosing_namespaces {
        return None;
    }
    let top_level: Vec<usize> = decls.iter().filter(|d| d.parent.is_none()).map(|d| d.start_byte).collect();
    match declared_types::using_block(namespaces, node.start_byte(), &top_level) {
        Some(block) if block.is_empty() => None,
        block => Some(crate::resolve::block_wildcard_alias(&block.unwrap_or_default())),
    }
}

/// The `Imports` rows of an import whose paths a structural walk recovers —
/// Rust's use-tree (`capture` = `ref.use`) or Scala's `import_declaration`
/// (`ref.import.dotted`, S-518): one row per imported path, a glob a `Glob`
/// row, each recording the declaration's `exported` mark (S-606; `None` where
/// the query marks none). A Rust glob has no alias; a Scala one brings every
/// member of what it names into view — a package's types and top-level
/// definitions, an object's methods and values — so it carries the static
/// wildcard's alias (S-573, [`STATIC_WILDCARD_ALIAS`](crate::resolve::STATIC_WILDCARD_ALIAS)).
fn tree_import_rows(
    capture: &str,
    node: Node<'_>,
    source: &[u8],
    source_symbol: &LogosSymbol,
    line: u32,
    exported: Option<bool>,
) -> Vec<RefFact> {
    let mut items = Vec::new();
    if capture == "ref.use" {
        flatten_use_tree(node, source, &mut items);
    } else {
        flatten_dotted_import(node, source, &mut items);
    }
    let glob_alias = (capture == "ref.import.dotted").then(|| crate::resolve::STATIC_WILDCARD_ALIAS.to_string());
    items
        .into_iter()
        .map(|item| {
            let (form, alias) = if item.glob {
                (RefForm::Glob, glob_alias.clone())
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
                peeled: None,
                arg_count: None,
                exported,
            }
        })
        .collect()
}

/// The `Imports` rows of one file-level TypeScript `export` statement (S-534,
/// [FR-RS-19]) — every row marked [`RefFact::exported`] is one the binder
/// follows a name imported from this module through:
///
/// - `export * from 's'`: a `Glob` row of `s`, exported.
/// - `export { a as b } from 's'` (`{ default }`, `{ default as X }`
///   included): one `Path` row per name, `<s>::a` aliased `b`, exported — the
///   shape of a Rust `pub use s::a as b`.
/// - `export * as ns from 's'`: the module bound under one name, as `import *
///   as ns from 's'` binds it — an ordinary import row, not a name re-export.
/// - A local export under the name `e` of what the file calls `l` —
///   `export default l`, `export default function l`, `export { l as e }` —
///   is the row `l` aliased `e`, exported: a single-segment target, which no
///   re-export row has, naming the file's own declaration. When the file
///   imported `l` by name, it is a re-export of what `l` names
///   (`<s>::<name>`). `export { l }` of a local declaration records nothing:
///   the declaration is found by its own name.
///
/// `export default <expression>` records nothing here: its `default` symbol
/// (S-532) is found by name. A specifier that canonicalises to nothing records
/// no row.
///
/// [FR-RS-19]: ../../../docs/specs/requirements/FR-RS-19.md
fn ts_export_rows(
    statement: Node<'_>,
    source: &[u8],
    imports: &ImportBindings,
    source_symbol: &LogosSymbol,
    line: u32,
    semantics: &Semantics,
) -> Vec<RefFact> {
    let text = |n: Node<'_>| n.utf8_text(source).ok().map(str::to_string);
    let row = |target: String, alias: Option<String>, form: RefForm, exported: Option<bool>| RefFact {
        source: source_symbol.clone(),
        target,
        alias,
        form,
        kind: EdgeKind::Imports,
        line,
        relation: None,
        receiver: None,
        peeled: None,
        arg_count: None,
        exported,
    };
    let mut cursor = statement.walk();
    let children: Vec<Node<'_>> = statement.children(&mut cursor).collect();
    let child = |kind: &str| children.iter().copied().find(|c| c.kind() == kind);
    // Each `export_specifier` of a clause: the name it exports, and the name
    // it is exported under (the same when it is not renamed).
    let specifiers = |clause: Node<'_>| -> Vec<(String, String)> {
        let mut cursor = clause.walk();
        clause
            .named_children(&mut cursor)
            .filter(|s| s.kind() == "export_specifier")
            .filter_map(|s| ts_specifier_names(s, source))
            .collect()
    };

    if let Some(spec) = statement.child_by_field_name("source").and_then(text) {
        let segments = specifier_segments(&spec, &semantics.specifier_extensions);
        if segments.is_empty() {
            return Vec::new();
        }
        let module = segments.join("::");
        if let Some(clause) = child("export_clause") {
            return specifiers(clause)
                .into_iter()
                .map(|(name, alias)| row(format!("{module}::{name}"), Some(alias), RefForm::Path, Some(true)))
                .collect();
        }
        if let Some(namespace) = child("namespace_export") {
            let mut cursor = namespace.walk();
            let local = namespace.named_children(&mut cursor).find(|n| n.kind() == "identifier").and_then(text);
            return vec![row(module, local, RefForm::Path, None)];
        }
        if child("*").is_some() {
            return vec![row(module, None, RefForm::Glob, Some(true))];
        }
        return Vec::new();
    }

    let local_export =
        |local: String, exported: String| local_export_row(imports, statement, source_symbol, line, local, exported);
    if child(DEFAULT_EXPORT).is_some() {
        let local = match (statement.child_by_field_name("value"), statement.child_by_field_name("declaration")) {
            (Some(value), _) if value.kind() == "identifier" => text(value),
            (None, Some(declaration)) => declaration.child_by_field_name("name").and_then(text),
            _ => None,
        };
        return local
            .and_then(|local| local_export(local, DEFAULT_EXPORT.to_string()))
            .into_iter()
            .collect();
    }
    child("export_clause")
        .map(|clause| {
            specifiers(clause)
                .into_iter()
                .filter_map(|(local, exported)| local_export(local, exported))
                .collect()
        })
        .unwrap_or_default()
}

/// The exported `Imports` row of a local export of `local` under `exported`
/// (S-534, S-540) — [`ts_export_rows`]' `export default l` and `export { l as
/// e }`, and [`commonjs_export_rows`]' `module.exports` forms alike: a
/// re-export of what `local` names when the file imported it by name, else
/// `local` aliased `exported`, and nothing when the two are one name (the
/// declaration is found by its own).
fn local_export_row(
    imports: &ImportBindings,
    statement: Node<'_>,
    source_symbol: &LogosSymbol,
    line: u32,
    local: String,
    exported: String,
) -> Option<RefFact> {
    let target = match imports.named_target(statement, &local) {
        Some(imported) => imported.to_string(),
        None if local != exported => local,
        None => return None,
    };
    Some(RefFact {
        source: source_symbol.clone(),
        target,
        alias: Some(exported),
        form: RefForm::Path,
        kind: EdgeKind::Imports,
        line,
        relation: None,
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: Some(true),
    })
}

/// The `Imports` rows of a CommonJS module's top-level `module.exports`
/// statements (S-540, [FR-RS-22]), each marked exported as a TypeScript
/// `export` statement's are ([`ts_export_rows`]), and attributed to `file`:
///
/// - `module.exports = X`, and `const X = module.exports` (`let`, `var`): `X`
///   aliased [`COMMONJS_EXPORTS`] — the module's export object. The binder
///   reads its members as the module's names (`User.exists` of `const User =
///   module.exports` is the module's `exists`), and the object itself as the
///   module's `default`.
/// - `module.exports = { a, b: c }`: each property exported under its key, as
///   `export { c as b }` is — `c` aliased `b`. A shorthand `a` records nothing:
///   the declaration is found by its own name.
///
/// A value the file imported by name re-exports what that import names, as in
/// [`ts_export_rows`]. Anything else assigned — a function, a call, a
/// `require` — records nothing, and so does a statement inside a block or a
/// function. A language that does not declare `commonjs` records none.
///
/// [FR-RS-22]: ../../../docs/specs/requirements/FR-RS-22.md
fn commonjs_export_rows(
    root: Node<'_>,
    source: &[u8],
    imports: &ImportBindings,
    file_module: Option<&LogosSymbol>,
    semantics: &Semantics,
) -> Vec<RefFact> {
    let Some(file) = file_module.filter(|_| semantics.commonjs) else {
        return Vec::new();
    };
    let mut cursor = root.walk();
    let statements: Vec<Node<'_>> = root.named_children(&mut cursor).collect();
    statements
        .into_iter()
        .flat_map(|statement| {
            let line = statement.start_position().row as u32 + 1;
            commonjs_exports_of(statement, source)
                .into_iter()
                .filter_map(move |(local, exported)| local_export_row(imports, statement, file, line, local, exported))
        })
        .collect()
}

/// What one top-level statement exports the CommonJS way
/// ([`commonjs_export_rows`]), as `(local, exported)` pairs.
fn commonjs_exports_of(statement: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let text = |n: Node<'_>| n.utf8_text(source).ok().map(str::to_string);
    let object = |local: Option<Node<'_>>| local.and_then(text).map(|x| (x, COMMONJS_EXPORTS.to_string()));
    let mut cursor = statement.walk();
    let children: Vec<Node<'_>> = statement.named_children(&mut cursor).collect();
    match statement.kind() {
        "expression_statement" => {
            let value = children
                .first()
                .filter(|a| a.kind() == "assignment_expression")
                .filter(|a| a.child_by_field_name("left").is_some_and(|l| is_module_exports(l, source)))
                .and_then(|a| a.child_by_field_name("right"));
            match value {
                Some(value) if value.kind() == "identifier" => object(Some(value)).into_iter().collect(),
                Some(value) if value.kind() == "object" => object_exports(value, source),
                _ => Vec::new(),
            }
        }
        "lexical_declaration" | "variable_declaration" => children
            .iter()
            .filter(|d| d.kind() == "variable_declarator")
            .filter(|d| d.child_by_field_name("value").is_some_and(|v| is_module_exports(v, source)))
            .filter_map(|d| object(d.child_by_field_name("name").filter(|n| n.kind() == "identifier")))
            .collect(),
        _ => Vec::new(),
    }
}

/// The `(local, exported)` pairs of a `module.exports = { a, b: c }` object
/// ([`commonjs_export_rows`]): `(a, a)` and `(c, b)`. A quoted, computed or
/// numeric key, a non-identifier value and a method record none.
fn object_exports(object: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let text = |n: Node<'_>| n.utf8_text(source).ok().map(str::to_string);
    let mut cursor = object.walk();
    let properties: Vec<Node<'_>> = object.named_children(&mut cursor).collect();
    properties
        .into_iter()
        .filter_map(|property| {
            let (key, local) = match property.kind() {
                "shorthand_property_identifier" => (Some(property), Some(property)),
                "pair" => (
                    property.child_by_field_name("key").filter(|k| k.kind() == "property_identifier"),
                    property.child_by_field_name("value").filter(|v| v.kind() == "identifier"),
                ),
                _ => return None,
            };
            Some((key.and_then(text)?, local.and_then(text)?)).map(|(key, local)| (local, key))
        })
        .collect()
}

/// Whether `n` is `module.exports` itself — not `module.parent`, not
/// `module.exports.y` (S-540).
fn is_module_exports(n: Node<'_>, source: &[u8]) -> bool {
    let reads = |n: Option<Node<'_>>, expected: &str| n.is_some_and(|n| n.utf8_text(source) == Ok(expected));
    n.kind() == "member_expression"
        && reads(n.child_by_field_name("object").filter(|o| o.kind() == "identifier"), "module")
        && reads(n.child_by_field_name("property"), "exports")
}

/// The marker capture of a PHP trait `use` (S-522): present in the same match
/// as the `ref.implements` naming the trait, it records the row's alias as
/// [`TRAIT_USE_ALIAS`](crate::resolve::TRAIT_USE_ALIAS) — a used trait's
/// method outranks every inherited one, which an `implements` never does.
const TRAIT_USE_CAPTURE: &str = "ref.implements.trait";

/// The marker captures of a Ruby `prepend M` and `extend M` (S-568): present in
/// the same match as the `ref.implements` naming the module, each records the
/// row's alias — [`MIXIN_PREPEND_ALIAS`](crate::resolve::MIXIN_PREPEND_ALIAS),
/// [`MIXIN_EXTEND_ALIAS`](crate::resolve::MIXIN_EXTEND_ALIAS) — where an
/// `include` records none: a prepended module precedes its class among the
/// ancestors, and an extended one's methods are the object's singleton methods.
const MIXIN_PREPEND_CAPTURE: &str = "ref.implements.prepend";
/// See [`MIXIN_PREPEND_CAPTURE`].
const MIXIN_EXTEND_CAPTURE: &str = "ref.implements.extend";

/// The marker capture of a C# `partial` type declaration (S-557): present in
/// the same match as the `ref.extends` naming the declaration itself, it records
/// the row's alias as [`PARTIAL_TYPE_ALIAS`](crate::resolve::PARTIAL_TYPE_ALIAS)
/// — the declaration is one part of a type others may declare too.
const PARTIAL_TYPE_CAPTURE: &str = "ref.extends.partial";

/// The type parameter list of a C# `partial` type declaration, in the
/// [`PARTIAL_TYPE_CAPTURE`] match (S-557): its type parameters are the part's
/// arity, which its row records, since `Box` and `Box<T>` are two types.
const PARTIAL_ARITY_CAPTURE: &str = "ref.extends.partial.arity";

/// What a type-relation match's marker capture records on its head row
/// ([`type_relation_rows`]).
#[derive(Debug, Clone, Copy, Default)]
struct RelationMarker {
    /// [`TRAIT_USE_ALIAS`](crate::resolve::TRAIT_USE_ALIAS) for a used trait
    /// ([`TRAIT_USE_CAPTURE`], S-522),
    /// [`PARTIAL_TYPE_ALIAS`](crate::resolve::PARTIAL_TYPE_ALIAS) for a partial
    /// type part ([`PARTIAL_TYPE_CAPTURE`], S-557), a Ruby mixin's kind for a
    /// `prepend` or an `extend` ([`MIXIN_PREPEND_CAPTURE`], S-568), none for an
    /// unmarked match.
    alias: Option<&'static str>,
    /// The arity of the type the head row names, recorded as the row's
    /// `arg_count`: a partial type part's type-parameter count
    /// ([`PARTIAL_ARITY_CAPTURE`], 0 without a list), or — for a language that
    /// tells types apart by arity — the written type's ([`written_arity`],
    /// S-602); `None` otherwise.
    arity: Option<u32>,
}

/// The [`RelationMarker`] of a type-relation match whose captured type is
/// `node`. A language that tells types apart by arity (`type_arity`, S-602)
/// records the written type's ([`written_arity`]) on an unmarked match; a
/// partial part's own row holds its declaration's.
fn relation_marker(
    captures: &[QueryCapture<'_>],
    capture_names: &[&str],
    node: Node<'_>,
    type_arity: bool,
) -> RelationMarker {
    let marked = |marker: &str| captures.iter().any(|c| capture_names[c.index as usize] == marker);
    // The markers that record an alias alone.
    let aliased = [
        (TRAIT_USE_CAPTURE, crate::resolve::TRAIT_USE_ALIAS),
        (MIXIN_PREPEND_CAPTURE, crate::resolve::MIXIN_PREPEND_ALIAS),
        (MIXIN_EXTEND_CAPTURE, crate::resolve::MIXIN_EXTEND_ALIAS),
    ];
    if let Some(&(_, alias)) = aliased.iter().find(|(capture, _)| marked(capture)) {
        return RelationMarker {
            alias: Some(alias),
            arity: None,
        };
    }
    if !marked(PARTIAL_TYPE_CAPTURE) {
        return RelationMarker {
            alias: None,
            arity: type_arity.then(|| written_arity(node)).flatten(),
        };
    }
    let arity = match_capture(captures, capture_names, PARTIAL_ARITY_CAPTURE).map_or(0, list_arity);
    RelationMarker {
        alias: Some(crate::resolve::PARTIAL_TYPE_ALIAS),
        arity: Some(arity),
    }
}

/// The Path-form rows a type-relation `capture` (`ref.extends`,
/// `ref.implements`, `ref.instantiates`, `ref.type_use`) records for its type
/// `node`, once per declaration in `owners` ([`type_relation_targets`], S-466).
/// A marked match's head row carries the marker ([`RelationMarker`]): a used
/// trait's [`TRAIT_USE_ALIAS`](crate::resolve::TRAIT_USE_ALIAS) (S-522), a
/// partial part's [`PARTIAL_TYPE_ALIAS`](crate::resolve::PARTIAL_TYPE_ALIAS)
/// and its arity as `arg_count` (S-557).
fn type_relation_rows(
    capture: &str,
    owners: &[LogosSymbol],
    node: Node<'_>,
    source: &[u8],
    line: u32,
    marker: RelationMarker,
) -> Vec<RefFact> {
    let head = match capture {
        "ref.extends" => EdgeKind::Extends,
        "ref.implements" => EdgeKind::Implements,
        "ref.instantiates" => EdgeKind::Instantiates,
        _ => EdgeKind::TypeUses,
    };
    let mut rows = Vec::new();
    for (kind, target) in type_relation_targets(node, source, head) {
        let head_row = kind == head;
        let alias = marker.alias.filter(|_| head_row).map(str::to_string);
        let arg_count = marker.arity.filter(|_| head_row);
        for owner in owners {
            rows.push(RefFact {
                source: owner.clone(),
                target: target.clone(),
                alias: alias.clone(),
                form: RefForm::Path,
                kind,
                line,
                relation: None,
                receiver: None,
                peeled: None,
                arg_count,
                exported: None,
            });
        }
    }
    rows
}

/// The declarations a type-relation row (`ref.extends`, …) belongs to
/// ([`collect_refs`]): the `declarators` a Java field's type sits beside, else
/// the `enclosing` declaration's symbol (`source`).
///
/// One exception (S-525): a Go `struct { Core }` embeds `Core` through a
/// [`NodeKind::Field`] of its own that encloses the type node, but it is the
/// struct that extends `Core`, so a `ref.extends` inside a Field belongs to the
/// field's container. Only `ref.extends`, and no other language's supertype list
/// sits in a Field.
fn relation_owners(
    capture: &str,
    declarators: Option<Vec<LogosSymbol>>,
    enclosing: Option<usize>,
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    source: LogosSymbol,
) -> Vec<LogosSymbol> {
    let container = enclosing
        .map(|i| &decls[i])
        .filter(|d| capture == "ref.extends" && d.kind == NodeKind::Field)
        .and_then(|d| symbols[d.parent?].clone());
    declarators.or_else(|| container.map(|c| vec![c])).unwrap_or_else(|| vec![source])
}

/// The TypeScript type nodes whose named children are the types they are built
/// from (S-535, [`type_relation_targets`]): a union, an intersection, a
/// parenthesised or `readonly` type, an indexed access (`User['id']`, whose
/// index is a literal and names nothing), `keyof T` and a tuple.
const TS_TYPE_OPERANDS: &[&str] = &[
    "union_type",
    "intersection_type",
    "parenthesized_type",
    "readonly_type",
    "lookup_type",
    "index_type_query",
    "tuple_type",
];

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
/// name either. Every shape is read from the grammar's node kinds
/// ([`type_path`]): Java's, since S-522 the supertype shapes of Python, PHP,
/// C# and Kotlin, since S-525 Go's embedded field types, and since S-535
/// TypeScript's. A type node that is none of them records nothing rather than
/// a guess. Only a Java or TypeScript `type_arguments` holds type nodes, so
/// only their type arguments record a `TypeUses`; C#'s `type_argument_list`,
/// Kotlin's type projections and Go's `type_arguments` record none.
///
/// A TypeScript type built from other types ([`TS_TYPE_OPERANDS`]) records
/// each operand as a `TypeUses`, at any depth: `Dto | null` → `Dto`,
/// `User['id']` → `User`. A nested annotation (`{ filter?: Dto }`) is a capture
/// of its own and is not walked into, so it records its type once.
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
        let children: Vec<Node<'_>> = n.named_children(&mut cursor)
            // A nested annotation (`{ a: Dto }`) is a capture of its own.
            .filter(|c| c.kind() != "type_annotation")
            .collect();
        if TS_TYPE_OPERANDS.contains(&n.kind()) {
            for operand in &children {
                record(EdgeKind::TypeUses, *operand);
            }
        }
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

/// The type-argument count a written type node spells (S-602, [CR-199]): 0 for
/// a plain name (C#'s `identifier`), the items of a `generic_name`'s
/// `type_argument_list` ([`list_arity`]: `Result<int>` → 1), and a `qualified_name`'s or
/// `alias_qualified_name`'s last segment's (`X.Result<int>`, `global::Result`).
/// `None` — unknown — for any other shape, and for an unbound generic whose
/// list holds no type (`Result<>`, `Map<,>`), whose arity its commas spell and
/// no named child does. Read only for a language that declares
/// `[module_model] type_arity`.
///
/// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
fn written_arity(node: Node<'_>) -> Option<u32> {
    match node.kind() {
        "identifier" => Some(0),
        "generic_name" => {
            let mut cursor = node.walk();
            let list = node.named_children(&mut cursor).find(|c| c.kind() == "type_argument_list")?;
            let count = list_arity(list);
            (count > 0).then_some(count)
        }
        "qualified_name" | "alias_qualified_name" => written_arity(node.child_by_field_name("name")?),
        _ => None,
    }
}

/// How many items a type-parameter or type-argument list holds (S-602): its
/// named children less the extras among them, so a comment written inside
/// `Result<int /* the value */>` or `class Box<T /* the item */>` is no type.
pub(super) fn list_arity(list: Node<'_>) -> u32 {
    let mut cursor = list.walk();
    let items = list.named_children(&mut cursor).filter(|c| !c.is_extra()).count();
    u32::try_from(items).unwrap_or(u32::MAX)
}

/// The `::`-joined path a type node names, generics and array dimensions
/// stripped, or `None` for a node that names no class-like type (a primitive,
/// `void`, an annotation, a call). See [`type_relation_targets`].
///
/// Java's type nodes, and the supertype shapes S-522 captures ([FR-RS-15]):
///
/// - a single name — Python's and C#'s `identifier`, PHP's `name`;
/// - a dotted one — Python's `attribute` (`models.Model`), C#'s
///   `qualified_name` (`System.Exception`), Kotlin's `user_type` (`a.b.C<T>`);
/// - a PHP `qualified_name` (`Monolog\Handler\HandlerInterface`) — never a
///   `relative_name` (`namespace\Handler`), whose current-namespace prefix the
///   import rungs would misread;
/// - a C# `generic_name` (`JsonConverter<T>`) — its name;
/// - a Go `qualified_type` (`sync.Locker`) — its package, then its name, and
///   a Go `generic_type` (`Gen[int]`) — its head type (S-525);
/// - a Ruby constant (S-565): a `constant` (`Foo`), or a `scope_resolution`
///   (`A::B`) — its scope's path, then the name — whose scope is itself a
///   constant path; any other scope (`obj::B`) names no type;
/// - a Scala `stable_type_identifier` (`a.b.C`, S-573) — its identifiers, in
///   order;
/// - TypeScript's (S-535): a `type_identifier`, a `generic_type` (its name), a
///   `nested_type_identifier` (`ns.Dto`), a bracketed `array_type` (`Dto[]`),
///   and the `identifier` or `member_expression` an `extends` or `new` names
///   (`Base`, `React.Component`). A `predefined_type` (`string`, `void`) and a
///   call (`extends Mixin(Base)`) name none.
///
/// A name written **fully qualified** — PHP's leading `\`, C#'s `global::`,
/// Ruby's leading `::` —
/// is headed [`FULLY_QUALIFIED_HEAD`], so the binder reads it from the global
/// namespace alone ([NFR-RA-05]).
///
/// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn type_path(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let is_annotation = |n: &Node<'_>| matches!(n.kind(), "annotation" | "marker_annotation");
    let text = |n: Node<'_>| n.utf8_text(source).ok().map(|t| t.trim().to_string());
    let mut cursor = node.walk();
    let named: Vec<Node<'_>> = node
        .named_children(&mut cursor)
        .filter(|n| !is_annotation(n))
        .collect();
    match node.kind() {
        "type_identifier" | "identifier" | "name" => Some(vec![text(node)?]),
        // `a.b.C`, `Outer<A>.Inner`: the qualifier, then the last identifier.
        "scoped_type_identifier" => {
            let (last, qualifier) = named.split_last()?;
            if last.kind() != "type_identifier" {
                return None;
            }
            let mut path = type_path(*qualifier.first()?, source)?;
            path.push(text(*last)?);
            Some(path)
        }
        "generic_type" | "annotated_type" => named
            .iter()
            .find(|n| n.kind() != "type_arguments")
            .and_then(|n| type_path(*n, source)),
        // Java's `element`; TypeScript's `Dto[]` names no field (S-535).
        "array_type" => type_path(node.child_by_field_name("element").or_else(|| named.first().copied())?, source),
        // TypeScript `ns.Dto`: the module's path, then the name (S-535).
        "nested_type_identifier" => member_path(node, source, "module", "name"),
        // TypeScript `a.b` in a type's module, and an `extends` or `new` value
        // spelled as a member (`React.Component`, `new models.User()`): the
        // object's path, then the property. A `this.x` object names no type.
        "nested_identifier" | "member_expression" => member_path(node, source, "object", "property"),
        // Python `a.b.C`: the object's path, then the attribute.
        "attribute" => member_path(node, source, "object", "attribute"),
        // C# `A.B<T>`: the qualifier's path, then the name.
        "qualified_name" if node.child_by_field_name("qualifier").is_some() => {
            let mut path = type_path(node.child_by_field_name("qualifier")?, source)?;
            path.extend(type_path(node.child_by_field_name("name")?, source)?);
            Some(path)
        }
        // PHP `\A\B\C`, `A\B\C`: the namespace's names, then the name; a
        // leading `\` makes it fully qualified. A `relative_name`
        // (`namespace\A\C`, the file's own namespace) names nothing here: its
        // prefix is not a name the binder may read through imports.
        "qualified_name" => {
            let rooted = node.child(0).is_some_and(|c| c.kind() == "\\");
            let mut path: Vec<String> = rooted
                .then(|| crate::resolve::FULLY_QUALIFIED_HEAD.to_string())
                .into_iter()
                .collect();
            for part in &named {
                match part.kind() {
                    "namespace_name" => {
                        let mut c = part.walk();
                        for seg in part.named_children(&mut c) {
                            path.push(text(seg)?);
                        }
                    }
                    "name" => path.push(text(*part)?),
                    _ => return None,
                }
            }
            Some(path)
        }
        // Go `pkg.T`: the package, then the type (S-525).
        "qualified_type" => {
            let package = text(node.child_by_field_name("package")?)?;
            Some(vec![package, text(node.child_by_field_name("name")?)?])
        }
        // C# `global::A.B`: the global namespace, then the name.
        "alias_qualified_name" => {
            let alias = text(node.child_by_field_name("alias")?)?;
            let mut path = (alias == "global")
                .then(|| vec![crate::resolve::FULLY_QUALIFIED_HEAD.to_string()])?;
            path.extend(type_path(node.child_by_field_name("name")?, source)?);
            Some(path)
        }
        "generic_name" => named
            .iter()
            .find(|n| n.kind() == "identifier")
            .and_then(|n| type_path(*n, source)),
        // Ruby `Foo`.
        "constant" => Some(vec![text(node)?]),
        // Ruby `A::B`: the scope's path, then the name; `::B` is read from the
        // top level.
        "scope_resolution" => {
            let name = text(node.child_by_field_name("name")?)?;
            let mut path = match node.child_by_field_name("scope") {
                Some(scope) if matches!(scope.kind(), "constant" | "scope_resolution") => type_path(scope, source)?,
                Some(_) => return None,
                None => vec![crate::resolve::FULLY_QUALIFIED_HEAD.to_string()],
            };
            path.push(name);
            Some(path)
        }
        // Scala `a.b.C` (S-573): the stable path's identifiers, then the name.
        "stable_type_identifier" | "stable_identifier" => named.iter().try_fold(Vec::new(), |mut path, part| {
            match part.kind() {
                "identifier" | "type_identifier" => path.push(text(*part)?),
                "stable_identifier" => path.extend(type_path(*part, source)?),
                _ => return None,
            }
            Some(path)
        }),
        // Kotlin `a.b.C<T>`: its identifiers, in order.
        "user_type" => {
            let path: Vec<String> = named
                .iter()
                .filter(|n| n.kind() == "identifier")
                .map(|n| text(*n))
                .collect::<Option<_>>()?;
            (!path.is_empty()).then_some(path)
        }
        // C++ `ns::Base`, `Outer<T>::Inner` (S-579): the scope's path, then the
        // name. A global `::Base` is headed `FULLY_QUALIFIED_HEAD`: it names the global `Base`
        // alone, never one an enclosing namespace declares.
        "qualified_identifier" => {
            let mut path = match node.child_by_field_name("scope") {
                Some(scope) => type_path(scope, source)?,
                None => vec![crate::resolve::FULLY_QUALIFIED_HEAD.to_string()],
            };
            path.extend(type_path(node.child_by_field_name("name")?, source)?);
            Some(path)
        }
        "namespace_identifier" => Some(vec![text(node)?]),
        // C++ `Base<T>` (S-579): the template's name.
        "template_type" => type_path(node.child_by_field_name("name")?, source),
        _ => None,
    }
}

/// The [`type_path`] of `node`'s `qualifier` field, then the text of its `name`
/// field — a member spelled on a path (`ns.Dto`, `React.Component`, `a.b.C`).
fn member_path(node: Node<'_>, source: &[u8], qualifier: &str, name: &str) -> Option<Vec<String>> {
    let mut path = type_path(node.child_by_field_name(qualifier)?, source)?;
    path.push(node.child_by_field_name(name)?.utf8_text(source).ok()?.trim().to_string());
    Some(path)
}

/// The key a TypeScript mapped type's `index_signature` binds (`P` of
/// `[P in keyof T]`, S-535), or `None` for any other node.
fn mapped_type_key(node: Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "index_signature" {
        return None;
    }
    let mut cursor = node.walk();
    let clause = node.named_children(&mut cursor).find(|c| c.kind() == "mapped_type_clause")?;
    let name = clause.child_by_field_name("name")?.utf8_text(source).ok()?;
    Some(name.trim().to_string())
}

/// A C++ template's parameter list (S-581): its `parameters` field, whose
/// `typename T`, `class U = V` and `typename... Ts` each spell their name as a
/// `type_identifier`. Read by kind: other grammars' callables field-name their
/// value parameters `parameters`.
fn template_parameter_list(declaration: Node<'_>) -> Option<Node<'_>> {
    let list = declaration.child_by_field_name("parameters")?;
    (list.kind() == "template_parameter_list").then_some(list)
}

/// A C# class, struct or record's type-parameter list (S-553): an unnamed child
/// of the declaration, where a method or an interface names a field. Kotlin's
/// grammar names no field at all, so a Kotlin class's or function's
/// `type_parameters` is read here too (S-563) — only as an unnamed child: a
/// Scala `function_declaration` field-names its `type_parameters`
/// `parameters`, whose bound would read as a parameter name.
fn unnamed_type_parameter_list(declaration: Node<'_>) -> Option<Node<'_>> {
    if !matches!(
        declaration.kind(),
        "class_declaration" | "struct_declaration" | "record_declaration" | "function_declaration"
    ) {
        return None;
    }
    let mut cursor = declaration.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        let child = cursor.node();
        if cursor.field_name().is_none() && matches!(child.kind(), "type_parameter_list" | "type_parameters") {
            return Some(child);
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

/// Every type-parameter name the declarations enclosing `node` (itself
/// included) declare — `T` of `class Box<T>`, `E` of `<E> E get(E e)` — and
/// the key a TypeScript mapped type binds over its member type (`P` of
/// `{ [P in keyof T]: P }`, S-535).
fn type_parameters_in_scope(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut at = Some(node);
    while let Some(n) = at {
        if let Some(key) = mapped_type_key(n, source) {
            names.insert(key);
        }
        let parameters = n
            .child_by_field_name("type_parameters")
            .or_else(|| unnamed_type_parameter_list(n))
            .or_else(|| template_parameter_list(n));
        if let Some(parameters) = parameters {
            let mut cursor = parameters.walk();
            for parameter in parameters.named_children(&mut cursor) {
                let mut inner = parameter.walk();
                // C#'s and Kotlin's `type_parameter` spell their name as a
                // plain `identifier` (S-553, S-563); of the grammars that name
                // a `type_parameters` field, no other's `type_parameter` holds
                // one.
                let name = parameter.named_children(&mut inner).find(|c| {
                    c.kind() == "type_identifier"
                        || (parameter.kind() == "type_parameter" && c.kind() == "identifier")
                });
                if let Some(text) = name.and_then(|c| c.utf8_text(source).ok()) {
                    names.insert(text.trim().to_string());
                }
            }
        }
        names.extend(named_type_parameters(n, source));
        at = n.parent();
    }
    names
}

/// The type parameters a declaration's list names by its own `name` field
/// (S-576, Scala): `[T, +U, F[_]]` field-names `T` and `F` on the list and `U`
/// on its `covariant_type_parameter`, never as a `type_identifier`. A `def`'s
/// list sits in its `parameters` field, beside its value parameters. No other
/// vendored grammar's `type_parameters` carries a `name` field, or sits in a
/// `parameters` one.
fn named_type_parameters(declaration: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut cursor = declaration.walk();
    let lists: Vec<Node<'_>> = declaration
        .children_by_field_name("parameters", &mut cursor)
        .filter(|c| c.kind() == "type_parameters")
        .chain(declaration.child_by_field_name("type_parameters"))
        .collect();
    let mut out = Vec::new();
    for list in lists {
        let mut cursor = list.walk();
        let variant = list
            .named_children(&mut cursor)
            .filter(|c| matches!(c.kind(), "covariant_type_parameter" | "contravariant_type_parameter"))
            .filter_map(|c| c.child_by_field_name("name"));
        let mut names = list.walk();
        let own: Vec<Node<'_>> = list.children_by_field_name("name", &mut names).collect();
        let texts = own.into_iter().chain(variant).filter_map(|n| n.utf8_text(source).ok());
        out.extend(texts.map(|t| t.trim().to_string()));
    }
    out
}

/// The superclass a class declares, as its `Extends` row records it (`Base`,
/// `a::b::Base`), read from the file's own reference facts — the per-file
/// accessor S-467's `super.m()` receiver shape consumes, so the type it
/// qualifies a `super` call with is the one this capture recorded and never a
/// second reading of the source (S-466, [CR-150]).
///
/// `None` when `class` records no `Extends` row, and when it records several
/// (an interface's super-interfaces) — a `super` receiver names one type or
/// none ([NFR-RA-05]). A partial type part's own row (S-557,
/// [`PARTIAL_TYPE_ALIAS`](crate::resolve::PARTIAL_TYPE_ALIAS)) names the class
/// itself, never a superclass, so it is not one of them.
///
/// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn declared_superclass<'a>(refs: &'a [RefFact], class: &LogosSymbol) -> Option<&'a str> {
    let mut supers = refs.iter().filter(|r| {
        r.kind == EdgeKind::Extends
            && r.form == RefForm::Path
            && &r.source == class
            && r.alias.as_deref() != Some(crate::resolve::PARTIAL_TYPE_ALIAS)
    });
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
/// argument once for the language that owns it. The mapped value is **still**
/// passed through [`is_http_method`] by the caller, so a mistyped row value
/// (`"GTE"`) captures nothing rather than inventing a method ([NFR-RA-05]).
///
/// [CR-108]: ../../../docs/requests/CR-108-per-language-http-client-call-capture.md
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
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

    let id_to_idx = decl_index(decls);
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

/// Whether a declaration takes `self` and how (CR-200, S-606 / [FR-EX-34]):
/// recorded for a Rust impl function (`is_rust_method`) and a callable required
/// signature, from its parameter list — the mode [`ReceiverMode::None`]
/// exactly when it takes none, the `@item.receiver.*` capture's when it does.
/// A callable other than a Rust impl function that its plugin marks
/// `@item.receiver.none` takes none (S-565): Ruby's `def self.x`, a
/// `class << self` method and a `module_function`, called on the class or
/// module object itself.
/// `(None, None)` for every other declaration.
///
/// [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
fn receiver_facts(decl: &Decl<'_>, is_rust_method: bool, is_callable: bool) -> (Option<bool>, Option<ReceiverMode>) {
    if is_callable && !is_rust_method && decl.item.receiver == Some(ReceiverMode::None) {
        return (Some(false), Some(ReceiverMode::None));
    }
    let takes_self = decl
        .params
        .filter(|_| is_rust_method || (is_callable && decl.item.signature))
        .map(|p| p.receiver);
    let mode = takes_self.and_then(|takes| if takes { decl.item.receiver } else { Some(ReceiverMode::None) });
    (takes_self, mode)
}

/// The segments a `@ref.call` records ([`split_path_text`] of its `text`),
/// qualified by the constant its match names beside it, when one does
/// ([`CALL_SCOPE_CAPTURE`], S-565): Ruby's `Foo.bar` is `[Foo, bar]`. Empty when
/// the call's own text names nothing.
fn call_segments(captures: &[QueryCapture<'_>], capture_names: &[&str], source: &[u8], text: &str) -> Vec<String> {
    let segments = split_path_text(text);
    let scope = match_capture(captures, capture_names, CALL_SCOPE_CAPTURE).and_then(|n| type_path(n, source));
    match scope {
        Some(mut scope) if !segments.is_empty() => {
            scope.extend(segments);
            scope
        }
        _ => segments,
    }
}

/// A call's `segments`, headed [`FULLY_QUALIFIED_HEAD`](crate::resolve::FULLY_QUALIFIED_HEAD)
/// when it is a rooted C++ `::f()` (S-579): it names the global `f` alone, as a
/// global `::Base` does ([`type_path`]). Every other call's segments as given.
fn rooted_call(node: Node<'_>, text: &str, mut segments: Vec<String>) -> Vec<String> {
    if !segments.is_empty() && node.kind() == "qualified_identifier" && text.trim_start().starts_with("::") {
        segments.insert(0, crate::resolve::FULLY_QUALIFIED_HEAD.to_string());
    }
    segments
}

/// The node of the capture named `name` in one match, if the match has one.
fn match_capture<'t>(captures: &[QueryCapture<'t>], capture_names: &[&str], name: &str) -> Option<Node<'t>> {
    captures.iter().find(|c| capture_names[c.index as usize] == name).map(|c| c.node)
}

/// The target a fully qualified call `<T as Tr>::m()` records (S-606), when
/// its match carries the type and trait companions of its `@ref.call`; the one
/// a qualified self naming no trait, `<T>::m()`, records (S-627), when its match
/// carries the bracket's type.
fn qualified_call(captures: &[QueryCapture<'_>], capture_names: &[&str], source: &[u8], segments: &[String]) -> Option<String> {
    let text = |name| match_capture(captures, capture_names, name).and_then(|n| n.utf8_text(source).ok());
    if let Some((ty, tr)) = text(QUALIFIED_TYPE_CAPTURE).zip(text(QUALIFIED_TRAIT_CAPTURE)) {
        return Some(qualified_call_target(ty, tr, segments));
    }
    let bare = match_capture(captures, capture_names, QUALIFIED_BARE_CAPTURE).filter(|n| n.kind() != "qualified_type")?;
    Some(bracketed_call_target(bare.utf8_text(source).ok()?, segments))
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
/// the declarations a parse-error region cost the file ([FR-EX-30]), and the
/// file's impl blocks (S-606, the [`assoc`] `@item.*` vocabulary, which also
/// gives each declaration its receiver mode, variants and signature marker).
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
    body_kinds: &[String],
) -> (
    Vec<Decl<'t>>,
    Option<String>,
    Vec<declared_types::NamespaceScope>,
    ParseDamage,
    Vec<ImplBlockFact>,
) {
    let capture_names = query.capture_names();
    let mut decls: Vec<Decl<'t>> = Vec::new();
    // Guard against a declaration being captured more than once (a query with
    // overlapping patterns): a duplicate would corrupt the parent map and
    // inflate ordinals, churning the symbol ID. It keeps the ID-stability
    // invariant (ADR-07) robust against future query authors. The key is the
    // declaration node, plus the name node for a match that marks
    // [`EACH_NAME_CAPTURE`]: one node declares several names where a grammar
    // lists them (Go's `var a, b = 1, 2`, `x, y int`, S-525), each a declaration
    // of its own, while two patterns naming the same name still collapse to the
    // first, and every other query keeps one declaration per node. A later
    // pattern may deliberately name a node an earlier one did (TypeScript's
    // top-level binding patterns, S-532 — see `value_bindings`).
    let mut seen_decls: HashSet<(usize, usize)> = HashSet::new();
    // The Constant/Variable declarations taken so far, by node id → index into
    // `decls` (S-532): a pattern naming a value binding is a fallback another
    // pattern may refine to a Function, whichever matched first.
    let mut value_bindings: HashMap<usize, usize> = HashMap::new();
    // Declarations a capture named nothing (S-512). One that another pattern
    // names properly is taken after all, so only the remainder counts as skipped.
    let mut nameless: HashSet<usize> = HashSet::new();
    let mut damage = ParseDamage::default();
    let mut misread: HashSet<usize> = HashSet::new();
    let mut package: Option<String> = None;
    let mut namespaces: Vec<declared_types::NamespaceScope> = Vec::new();
    // declaration node id → the self type its match declares (S-493).
    let mut self_types: HashMap<usize, String> = HashMap::new();
    // The parameter-list captures (S-591), resolved once every declaration is in.
    let mut params = arity::ParamCaptures::default();
    // The impl-block, receiver-mode, variant and signature captures (S-606).
    let mut items = assoc::AssocCaptures::default();
    // The class-body, prototype and out-of-line scope captures (S-577).
    let mut definitions = definition::DefinitionCaptures::default();
    // The test macros' names, applied once the walk is done (S-580).
    let mut test_names = test_name::TestNames::default();
    // The owner names and placement markers (S-539).
    let mut owners = owner::OwnerCaptures::default();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        let self_type = match_capture(m.captures, capture_names, SELF_TYPE_CAPTURE)
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let chained = m
            .captures
            .iter()
            .any(|c| capture_names[c.index as usize] == declared_types::NAMESPACE_CHAINED_CAPTURE);
        for cap in m.captures {
            let capture = capture_names[cap.index as usize];
            let companion = |name: &str| match_capture(m.captures, capture_names, name);
            if declared_types::note_package(&mut package, capture, cap.node, source)
                || declared_types::note_namespace(&mut namespaces, capture, cap.node, source, chained)
                || params.note(capture, cap.node)
                || items.note(capture, cap.node, companion)
                || test_names.note(capture, cap.node, m.captures, capture_names, source)
                || definitions.note(capture, cap.node)
                || owners.note(capture, cap.node)
            {
                continue;
            }
            if capture == MISREAD_CAPTURE {
                // One expression is counted once, whichever pattern names it.
                misread.insert(cap.node.id());
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
            // An attribute (S-543) and state (S-564) are declared at their name
            // ([`ATTRIBUTE_CAPTURE`], [`STATE_CAPTURE`]): the assignment or call
            // they sit in is the method's, not the field's.
            let attribute = m.captures.iter().any(|c| capture_names[c.index as usize] == ATTRIBUTE_CAPTURE);
            let state = m.captures.iter().any(|c| capture_names[c.index as usize] == STATE_CAPTURE);
            let lifted = if attribute || state {
                Lifted {
                    node: name_node,
                    at_error: false,
                }
            } else {
                lift_to_declaration(name_node)
            };
            let decl_node = lifted.node;
            // Checked before `seen_decls`, so another pattern naming the same
            // declaration properly is still taken.
            let each_name = m.captures.iter().any(|c| capture_names[c.index as usize] == EACH_NAME_CAPTURE);
            let declared = (decl_node.id(), if each_name { name_node.start_byte() } else { 0 });
            if names_nothing(name_node) {
                nameless.insert(decl_node.id());
                continue;
            }
            if let Some(self_type) = self_type {
                self_types
                    .entry(decl_node.id())
                    .or_insert_with(|| self_type.to_string());
            }
            if !seen_decls.insert(declared) {
                // Already captured by another pattern — keep the first, except
                // that a binding captured as a value is a function when any
                // pattern says so.
                refine_value_binding(&mut decls, &value_bindings, decl_node.id(), kind);
                continue;
            }
            let Ok(text) = name_node.utf8_text(source) else {
                continue; // non-UTF-8 identifier slice — skip defensively
            };
            let name = crate::plugin::queries::pattern_setting(query, m.pattern_index, NAME_SETTING).unwrap_or(text);
            let name = if state { state_name(name) } else { name.to_string() };
            damage.truncated += usize::from(lifted.at_error);
            note_value_binding(&mut value_bindings, decls.len(), decl_node.id(), kind);
            decls.push(Decl {
                node: decl_node,
                kind,
                name,
                start_byte: decl_node.start_byte(),
                start_line: decl_node.start_position().row as u32 + 1,
                end_line: decl_node.end_position().row as u32 + 1,
                parent: None,
                ordinal: 0,
                self_type: None,
                params: None,
                item: assoc::DeclItemFacts::default(),
                def: definition::DeclDefinitionFacts::default(),
                qualifier: Vec::new(),
                completes: None,
                attribute,
                state,
                owner: None,
                member: member::DeclMemberFacts::of_match(m.captures, capture_names),
            });
        }
    }
    for decl in &mut decls {
        decl.self_type = self_types.get(&decl.node.id()).cloned();
    }
    test_names.apply(&mut decls);
    let own_params = params.per_decl(&decls, body_kinds);
    for (decl, own) in decls.iter_mut().zip(own_params) {
        decl.params = own;
    }
    let own_items = items.per_decl(&decls, source);
    for (decl, own) in decls.iter_mut().zip(own_items) {
        decl.item = own;
    }
    let own_definitions = definitions.per_decl(&decls, source);
    for (decl, own) in decls.iter_mut().zip(own_definitions) {
        decl.def = own;
    }
    let own_owners = owners.per_decl(&decls, source);
    for (decl, own) in decls.iter_mut().zip(own_owners) {
        decl.owner = own;
    }
    // An out-of-line definition error recovery invented is not a declaration
    // (S-577, `definition`). Dropped before parents exist, so no index moves.
    decls.retain(|d| !d.def.artifact);
    settle_state(&mut decls);
    damage.truncated -= member::settle(&mut decls, source);
    let named: HashSet<usize> = seen_decls.iter().map(|&(decl, _)| decl).collect();
    damage.skipped = nameless.difference(&named).count();
    damage.misread = misread.len();
    (decls, package, namespaces, damage, items.impl_blocks(source))
}

/// The name of a state declaration (S-564): the instance variable it is, `@x`
/// — whether the capture is the variable itself (`@x`), a symbol literal
/// (`:x`) or a string's content (`x`).
fn state_name(captured: &str) -> String {
    format!("@{}", captured.trim_start_matches(':').trim_start_matches('@'))
}

/// The class or module a state declaration is state of (S-564): the nearest
/// enclosing declaration that is one, whatever method, block or call lies
/// between. `None` for state no class or module holds.
fn state_owner(decl: &Decl<'_>, decls: &[Decl<'_>], index: &HashMap<usize, usize>) -> Option<usize> {
    let mut ancestor = decl.node.parent();
    while let Some(node) = ancestor {
        if let Some(&j) = index.get(&node.id()) {
            if matches!(decls[j].kind, NodeKind::Class | NodeKind::Module) {
                return Some(j);
            }
        }
        ancestor = node.parent();
    }
    None
}

/// Settle the state declarations ([`STATE_CAPTURE`], S-564): one per name in
/// its class or module, at the first place the file writes it — an `attr_*`
/// symbol and the `@x = …` it stands for are one Field — and none for state
/// outside any class or module. Run before [`assign_parents`], which places
/// each at its owner, because it removes declarations.
fn settle_state(decls: &mut Vec<Decl<'_>>) {
    if !decls.iter().any(|d| d.state) {
        return;
    }
    let index = decl_index(decls);
    let mut first: HashMap<(usize, &str), usize> = HashMap::new();
    let mut drop = vec![false; decls.len()];
    for (i, decl) in decls.iter().enumerate().filter(|(_, d)| d.state) {
        let Some(owner) = state_owner(decl, decls, &index) else {
            drop[i] = true;
            continue;
        };
        match first.entry((owner, decl.name.as_str())) {
            Entry::Vacant(slot) => {
                slot.insert(i);
            }
            Entry::Occupied(mut slot) => {
                let earlier = *slot.get();
                if decls[earlier].start_byte <= decl.start_byte {
                    drop[i] = true;
                } else {
                    drop[earlier] = true;
                    slot.insert(i);
                }
            }
        }
    }
    let mut kept = drop.iter().map(|d| !d);
    decls.retain(|_| kept.next().unwrap_or(true));
}

/// Remember a Constant/Variable declaration (`index` into `decls`) so a later
/// capture of the same node can refine it ([`refine_value_binding`], S-532).
fn note_value_binding(value_bindings: &mut HashMap<usize, usize>, index: usize, node_id: usize, kind: NodeKind) {
    if matches!(kind, NodeKind::Constant | NodeKind::Variable) {
        value_bindings.insert(node_id, index);
    }
}

/// A second pattern naming the node of a Constant/Variable already taken, as a
/// Function (a TypeScript `const f = () => …`, which the top-level binding
/// patterns name too), makes it that Function, whichever matched first (S-532).
fn refine_value_binding(decls: &mut [Decl<'_>], value_bindings: &HashMap<usize, usize>, node_id: usize, kind: NodeKind) {
    if kind == NodeKind::Function {
        if let Some(&i) = value_bindings.get(&node_id) {
            decls[i].kind = kind;
        }
    }
}

/// The declarations a parse-error region cost one file ([FR-EX-30]), counted
/// into its partial-extraction warning ([CR-168] §3.2(2)) — and the annotated
/// declarations a grammar misread without any parse error ([`Self::misread`],
/// S-559), which that warning also counts.
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
    /// Annotated declarations the grammar read as an expression
    /// ([`MISREAD_CAPTURE`]): the declaration, or the annotations written before
    /// it, are lost to the extraction with **no** parse error to say so (S-559).
    misread: usize,
}

impl ParseDamage {
    /// Append this damage to the file's partial-extraction warning. A file that
    /// lost no declaration keeps the warning byte-identical.
    ///
    /// `truncated` and `skipped` imply the parse has an error, so the warning
    /// always exists when either is non-zero: an ERROR ancestor and a MISSING
    /// name each set tree-sitter's `has_error`, and a zero-width name is only
    /// ever recovered. `misread` does not: with no parse error there is no
    /// warning to append to, and [`silent_warning`](Self::silent_warning) makes
    /// one.
    fn annotate(self, warning: &mut String) {
        if self.truncated + self.skipped > 0 {
            warning.push_str(&format!(
                "; {} declaration(s) truncated and {} skipped at a parse error",
                self.truncated, self.skipped
            ));
        }
        if self.misread > 0 {
            warning.push_str(&self.misread_clause());
        }
    }

    /// The warning of a file whose parse reports no error but lost
    /// declarations ([`MISREAD_CAPTURE`]) — `None` when it lost none. It keeps
    /// the "partial extraction" wording of the parse-error warning, which is
    /// what a reader of the index warnings looks for.
    fn silent_warning(self) -> Option<String> {
        (self.misread > 0).then(|| format!("partial extraction (no syntax error reported){}", self.misread_clause()))
    }

    fn misread_clause(self) -> String {
        format!(
            "; {} annotated declaration(s) read as an expression (a declaration or its annotations are lost)",
            self.misread
        )
    }
}

/// Declaration node id → the index of the **first** declaration on that node.
///
/// A query that opts into [`EACH_NAME_CAPTURE`] declares each name of
/// `var a, b = …` as a declaration of its own, all on one node (S-525). The
/// enclosing-declaration lookups key on the node, so the first name — the one
/// every pre-S-525 graph recorded — owns what the node holds: the calls of its
/// initialiser and the types nested in it. Every other declaration sits on a
/// node of its own, where this is the plain index.
fn decl_index(decls: &[Decl<'_>]) -> HashMap<usize, usize> {
    let mut index = HashMap::with_capacity(decls.len());
    for (i, decl) in decls.iter().enumerate() {
        index.entry(decl.node.id()).or_insert(i);
    }
    index
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
/// The walk climbs only through the C/C++ declarator node kinds — the
/// `qualified_identifier` an out-of-line C++ definition is named by (S-577) and
/// the `scope_resolution` a Ruby compact `class A::B` is named by (S-564) —
/// which no other supported grammar puts above a captured name (Kotlin has a
/// `qualified_identifier`, but only as a package header's path, never a
/// declaration's name) — so it is a provable no-op for every other language
/// (`name.parent()` is never one of these kinds), keeping their decl
/// nodes byte-identical ([NFR-RA-06]). The captured *name* is unchanged: it is
/// still read from the original leaf node, never from the lifted declaration.
///
/// A JavaScript function defined by assignment is the other exception (S-539):
/// its name is the property of the `X.y` it assigns, and the declaration is
/// the `assignment_expression` that holds the function ([`assigned_by`]).
///
/// One name is its own declaration, by kind: a `compound_statement`, which only
/// the C++ query captures — a Catch2 test is its block, the call that names it a
/// statement before it (S-580, `extract::test_name`). No other plugin's query
/// captures a block as a name, so this too is a no-op for every other language;
/// a plugin that did would get the block as its declaration.
///
/// **It never enters an ERROR node ([FR-EX-30]).** Error recovery can strand a
/// declarator directly in an ERROR region that swallows the rest of the file
/// (ccache's `parse_umask`, whose climb used to land on a 627-line ERROR and
/// report CC 117). A climb that would enter an ERROR is abandoned, and the
/// declaration keeps the name's own declarator — never an outer declarator
/// wrapper that recovery stretched over torn body tokens. A name that sits
/// loose in an ERROR region (a C++ class head whose body recovery tore apart)
/// is its own node. Either way [`Lifted::at_error`] is set and the declaration
/// counts as truncated. Only C, C++ and Ruby climb, and only the C++ query
/// captures a name loose in an ERROR region, so the guard is a no-op for every
/// other language.
///
/// [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
fn lift_to_declaration(name: Node<'_>) -> Lifted<'_> {
    const CFAMILY_DECLARATORS: [&str; 8] = [
        "function_declarator",
        "pointer_declarator",
        "reference_declarator",
        "array_declarator",
        "parenthesized_declarator",
        "init_declarator",
        // The name of an out-of-line C++ definition, `Cls::m` (S-577).
        "qualified_identifier",
        // The name of a Ruby compact `class A::B` / `module A::B` (S-564): the
        // declaration is the `class`, which holds the body, not its scope.
        "scope_resolution",
    ];
    let cut = |node| Lifted { node, at_error: true };
    // A Catch2 test is its body block (S-580): the `TEST_CASE("…")` call that
    // names it is a statement before the block, never its parent, so the block
    // is the declaration — the one node that holds the body's references.
    if name.kind() == "compound_statement" {
        return Lifted {
            node: name,
            at_error: name.parent().is_some_and(|parent| parent.is_error()),
        };
    }
    let own = match name.parent() {
        Some(parent) if parent.is_error() => return cut(name),
        Some(parent) => parent,
        None => name,
    };
    // A JavaScript function defined by assignment (S-539): `X.y` names it, and
    // the assignment, which holds the function, is the declaration.
    if let Some(assignment) = assigned_by(own) {
        return Lifted {
            node: assignment,
            at_error: false,
        };
    }
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

/// The `assignment_expression` whose left side is `target`, when `target` is
/// the `member_expression` it assigns (`X.y` in `X.y = function …`, S-539).
/// Only the JavaScript layer captures a name in a `member_expression`, and no
/// other grammar has one, so this is a no-op for every other language.
fn assigned_by(target: Node<'_>) -> Option<Node<'_>> {
    if target.kind() != "member_expression" {
        return None;
    }
    target
        .parent()
        .filter(|p| p.kind() == "assignment_expression")
        .filter(|p| p.child_by_field_name("left").is_some_and(|left| left.id() == target.id()))
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

/// The simple names of the traits a bound list names (S-608) — the `A + B` of
/// `impl A + B` (a `bounded_type`) or of `T: A + B` (a `trait_bounds`), or the
/// one trait of a single bound — in written order, each once. A lifetime,
/// `?Sized`, a higher-ranked or `Fn(…)` bound names none ([`trait_simple_name`]).
fn bound_names(bounds: Node<'_>, source: &[u8]) -> Vec<String> {
    let parts: Vec<Node<'_>> = match bounds.kind() {
        "bounded_type" | "trait_bounds" => (0..bounds.named_child_count()).filter_map(|i| bounds.named_child(i)).collect(),
        _ => vec![bounds],
    };
    let mut names: Vec<String> = Vec::new();
    for name in parts.into_iter().filter_map(|b| trait_simple_name(b, source)) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The trait bounds of every generic type parameter in scope in the function
/// `fn_node` (S-608, [FR-RS-47] rule 5): its own `<T: A>` and `where T: B`,
/// and those of the `impl` or `trait` block it is an item of — the function's
/// own parameter shadowing a block's of the same name. A parameter bounded by
/// no trait maps to an empty list: a generic that is no trait-typed receiver.
///
/// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
fn generic_trait_bounds(fn_node: Node<'_>, source: &[u8]) -> HashMap<String, Vec<String>> {
    let block = fn_node
        .parent()
        .filter(|list| list.kind() == "declaration_list")
        .and_then(|list| list.parent())
        .filter(|owner| matches!(owner.kind(), "impl_item" | "trait_item"));
    let mut bounds: HashMap<String, Vec<String>> = HashMap::new();
    for scope in block.into_iter().chain([fn_node]) {
        if let Some(params) = scope.child_by_field_name("type_parameters") {
            for param in (0..params.named_child_count()).filter_map(|i| params.named_child(i)) {
                if param.kind() != "type_parameter" {
                    continue;
                }
                let Some(name) = param.child_by_field_name("name").and_then(|n| n.utf8_text(source).ok()) else {
                    continue;
                };
                let own = param
                    .child_by_field_name("bounds")
                    .map(|b| bound_names(b, source))
                    .unwrap_or_default();
                bounds.insert(name.to_string(), own);
            }
        }
        let clauses = (0..scope.named_child_count())
            .filter_map(|i| scope.named_child(i))
            .filter(|c| c.kind() == "where_clause");
        for predicate in clauses.flat_map(|c| (0..c.named_child_count()).filter_map(move |i| c.named_child(i))) {
            let left = predicate.child_by_field_name("left").filter(|l| l.kind() == "type_identifier");
            let Some(name) = left.and_then(|l| l.utf8_text(source).ok()) else {
                continue;
            };
            let Some(known) = bounds.get_mut(name) else {
                continue;
            };
            for tr in predicate
                .child_by_field_name("bounds")
                .map(|b| bound_names(b, source))
                .unwrap_or_default()
            {
                if !known.contains(&tr) {
                    known.push(tr);
                }
            }
        }
    }
    bounds
}

/// The traits a receiver of the **trait-typed** type `ty` is known by, or
/// `None` when `ty` is typed by no trait — the receivers that fan out to a
/// trait method's implementations ([FR-RS-08]):
///
/// - a trait object `dyn T` (S-281, [CR-073]): its principal `trait:` alone;
/// - `impl A + B` (S-608, [FR-RS-47] rule 5): each bound it names;
/// - a generic parameter `T` with trait bounds (`<T: A + B>`, `where T: A`),
///   as `generics` records them for the enclosing function
///   ([`generic_trait_bounds`]): each bound.
///
/// Peels the transparent layers such a type can wear — `&`/`&mut`
/// (`reference_type`), a `dyn T + Send` bound list (`bounded_type`), and the
/// smart pointers `Box`/`Rc`/`Arc<…>` (`generic_type`). Depth-bounded (4
/// covers `Arc<Box<dyn T>>` and beyond). A bound is read by its simple name;
/// a lifetime, `?Sized`, a higher-ranked or `Fn(…)` bound names no trait and
/// is skipped. Anything else — a concrete type, a generic parameter bounded
/// by no trait — yields `None`, so the receiver stays a bare method name and
/// never fans out (the CR-066 guard, [FR-RS-06]).
///
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
/// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
/// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
fn receiver_traits_of_type(
    ty: Node<'_>,
    source: &[u8],
    generics: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    let mut cur = ty;
    for _ in 0..4 {
        match cur.kind() {
            "dynamic_type" => {
                return trait_simple_name(cur.child_by_field_name("trait")?, source).map(|t| vec![t]);
            }
            "abstract_type" => {
                let traits = bound_names(cur.child_by_field_name("trait")?, source);
                return (!traits.is_empty()).then_some(traits);
            }
            "type_identifier" => {
                let traits = generics.get(cur.utf8_text(source).ok()?)?;
                return (!traits.is_empty()).then(|| traits.clone());
            }
            "reference_type" => cur = cur.child_by_field_name("type")?,
            // `dyn A + Send`: the object trait sits in the reference/dynamic
            // part. `impl A + B` parses as `(impl A) + B`: every bound counts.
            "bounded_type" => {
                let parts: Vec<Node<'_>> = (0..cur.named_child_count()).filter_map(|i| cur.named_child(i)).collect();
                if let Some(opaque) = parts.iter().find(|c| c.kind() == "abstract_type") {
                    let mut traits = bound_names(opaque.child_by_field_name("trait")?, source);
                    for name in parts.iter().filter(|c| c.kind() != "abstract_type").filter_map(|c| trait_simple_name(*c, source)) {
                        if !traits.contains(&name) {
                            traits.push(name);
                        }
                    }
                    return (!traits.is_empty()).then_some(traits);
                }
                cur = (0..cur.named_child_count())
                    .filter_map(|i| cur.named_child(i))
                    .find(|c| {
                        matches!(c.kind(), "reference_type" | "dynamic_type" | "generic_type")
                    })?;
            }
            // Only the transparent smart-pointer wrappers carry a trait-typed
            // receiver; `Vec<dyn T>` and the like are not one.
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
                        // the reference path that already peels `&dyn T + Send`;
                        // `Box<T>` / `Box<impl T>` box a bounded parameter.
                        matches!(
                            c.kind(),
                            "dynamic_type"
                                | "reference_type"
                                | "generic_type"
                                | "bounded_type"
                                | "type_identifier"
                                | "abstract_type"
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

/// The Implements row a Rust impl method `symbol` records to the trait of its
/// `impl T for X` block (S-281, [CR-073]); an inherent `impl X` method carries
/// no trait and records none ([`rust_impl_trait_name`]). Never fabricated —
/// resolution binds it only to the one workspace Trait of that name, or leaves
/// it an honest miss ([NFR-RA-05]).
///
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn trait_impl_ref(method: Node<'_>, source: &[u8], symbol: &LogosSymbol, line: u32) -> Option<RefFact> {
    Some(RefFact {
        source: symbol.clone(),
        target: rust_impl_trait_name(method, source)?,
        alias: None,
        form: RefForm::Path,
        kind: EdgeKind::Implements,
        line,
        relation: None,
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: None,
    })
}

/// The in-scope names a path-grammar file's imports bind (S-440, [CR-142] D2,
/// [FR-RS-03]) — the per-file syntactic evidence that a call goes **through an
/// import**, so it can be recorded qualified by the module it names.
///
/// The imported rung of the binder needs to know which module a call's name
/// came from; the call's own text does not say. This pre-pass reads it off the
/// file's import statements and rewrites exactly three call shapes into the
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
///   namespace import (`import * as nav` → `nav.f()` reads `.::nav::f`);
/// - a member call whose receiver is a TypeScript **named import** (S-536,
///   [FR-RS-20]) — `import { UserMapper } from './m'` makes
///   `UserMapper.toDomain()` read `.::m::UserMapper::toDomain`, which binds
///   only when the import reaches a class.
///
/// Everything else is left exactly as before. **Every** TypeScript specifier
/// contributes, relative or bare (S-531): extraction cannot tell a `tsconfig`
/// alias (`@/lib/api`) or a workspace package (`@acme/ui`) from an external
/// package (`react`), so it records each call through any of them qualified,
/// and the binder decides — through the import's own binding, which an
/// external package never has, so `react::useState` stays unbound rather than
/// reaching a workspace function that shares its name ([NFR-RA-05]). A default
/// import reads as its module's `default` export (S-534); a Go dot or blank
/// import binds no qualifier. A local name the file also **declares** is
/// dropped — a member of a class-like container excepted, which a bare name
/// never reaches in these languages (S-590, [FR-RS-07]): a method `f` beside
/// `import { f }` leaves the import in force — and so is one a function scope
/// enclosing the call **binds** — a
/// parameter, a local, a destructured prop, a Go `:=` ([`local_bindings`]):
/// `func handle(admin *admin.Server) { admin.Reload() }` calls a method on the
/// parameter, not the package. The shadowed call keeps its bare form — a
/// missed edge, never a fabricated one ([NFR-RA-05]). The receiver of a Go
/// method call on a value (`s.Start()`) is no import and stays a
/// receiver-unqualified method name, so the [FR-RS-06] discipline is
/// untouched.
///
/// Built for a language declaring [`ImportSpecifier::Path`], and for an
/// import-root language — Python (S-544, [FR-RS-23]), whose dotted imports bind
/// **qualifiers** only: `curl.post()` after `from hc.lib import curl` reads
/// `hc::lib::curl::post`, and `Check.m()` after `from .models import Check`
/// reads `.::models::Check::m`, which the binder decides among `Check`'s
/// methods. A bare call of a Python named import keeps its bare form: the
/// binder's alias rung already reaches it. Python may rebind an imported name
/// at file scope, so such a binding shadows the import too. Every other
/// language (Rust included) takes [`ImportBindings::default`], which rewrites
/// nothing.
///
/// [FR-RS-23]: ../../../docs/specs/requirements/FR-RS-23.md
/// [FR-RS-20]: ../../../docs/specs/requirements/FR-RS-20.md
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
/// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
/// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
/// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Default)]
struct ImportBindings {
    /// A named import's local name → `<specifier target>::<exported name>`;
    /// a default import's exported name is `default` (S-534). It qualifies a
    /// bare call of that name, and a member call on it (S-536).
    named: HashMap<String, String>,
    /// A module qualifier's local name → the specifier target it names.
    qualifiers: HashMap<String, String>,
    /// A CommonJS `const m = require('./x')` binding's local name →
    /// `<specifier target>::default` (S-540): `m` is a qualifier too, and a
    /// bare call `m()` calls what the module exports as its default — its
    /// export object. Consulted after [`named`](ImportBindings::named).
    defaults: HashMap<String, String>,
    /// Function-scope node id → the names bound locally in it: parameters,
    /// variable declarators, destructured patterns, Go `:=` left-hand sides
    /// ([`local_bindings`]); for Python a class body's id and the root's too,
    /// for the names a class body or the file rebinds. A name bound here
    /// shadows any import of that name for every reference inside the scope (a
    /// class body's, for none inside its methods).
    locals: HashMap<usize, HashSet<String>>,
    /// Whether a call through a qualifier names the import binding it went
    /// through, as its row's alias: a dotted-name language's (Python, S-544).
    /// A call through a TypeScript named import always does (S-536).
    names_qualifier: bool,
}

impl ImportBindings {
    /// Read the file's import statements off the `@ref.import` captures: a
    /// path-grammar language's imports, and an import-root language's dotted
    /// ones (Python, S-544). Any other language binds none.
    fn collect(
        query: &Query,
        root: Node<'_>,
        source: &[u8],
        decls: &[Decl<'_>],
        semantics: &Semantics,
    ) -> ImportBindings {
        if semantics.import_specifier == ImportSpecifier::Name && semantics.import_roots.is_none() {
            return ImportBindings::default();
        }
        let capture_names = query.capture_names();
        let mut reader = ImportReader {
            source,
            named: HashMap::new(),
            qualifiers: HashMap::new(),
            defaults: HashMap::new(),
            requires: HashSet::new(),
            require_locals: HashSet::new(),
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            for cap in m.captures {
                if capture_names[cap.index as usize] != "ref.import" {
                    continue;
                }
                match semantics.import_specifier {
                    ImportSpecifier::Path => reader.read(cap.node, semantics),
                    ImportSpecifier::Name => reader.read_dotted(m.captures, capture_names, cap.node, semantics),
                }
            }
        }
        let member = |d: &Decl<'_>| d.parent.is_some_and(|p| crate::resolve::is_class_like(decls[p].kind));
        // The `const m` a CommonJS binding declares is the import itself
        // (S-540), never a declaration shadowing it.
        let declared: HashSet<&str> = decls
            .iter()
            .filter(|d| !member(d) && !reader.requires.contains(&d.node.id()))
            .map(|d| d.name.as_str())
            .collect();
        let keep = |map: HashMap<String, Option<String>>| -> HashMap<String, String> {
            map.into_iter()
                .filter(|(local, _)| !declared.contains(local.as_str()))
                .filter_map(|(local, v)| v.map(|v| (local, v)))
                .collect()
        };
        let mut bindings = ImportBindings {
            named: keep(reader.named),
            qualifiers: keep(reader.qualifiers),
            defaults: keep(reader.defaults),
            locals: local_bindings(root, source, semantics.import_roots.is_some()),
            names_qualifier: semantics.import_specifier == ImportSpecifier::Name,
        };
        bindings.forget_rebound_requires(root, source, &reader.requires, &reader.require_locals);
        bindings
    }

    /// Forget every CommonJS binding (S-540) the file binds again outside a
    /// function, or assigns where no enclosing function binds the name
    /// ([`rebound_names`]): `let store = require('./memory'); if (…) { store =
    /// require('./redis'); }`, a `var` redeclared in a block, or `for (const
    /// batch of …)` beside `const batch = require('./batch')`. Which module the
    /// name holds where it is called is then not the file's syntax to say, so
    /// it decides nothing — a missed edge, never a fabricated one. A binding
    /// inside a function is the function's own, and
    /// [`locally_bound`](ImportBindings::locally_bound) shadows the import
    /// there; so is an assignment to it (`notifications = …` after `let
    /// notifications` in a function).
    fn forget_rebound_requires(
        &mut self,
        root: Node<'_>,
        source: &[u8],
        requires: &HashSet<usize>,
        require_locals: &HashSet<String>,
    ) {
        if require_locals.is_empty() {
            return;
        }
        let rebound = rebound_names(root, source, requires, |node, name| self.locally_bound(node, name));
        for name in rebound.intersection(require_locals) {
            self.named.remove(name);
            self.qualifiers.remove(name);
            self.defaults.remove(name);
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
        // A Python class body's names are seen by the body itself, never by a
        // function inside it (S-544).
        let mut in_function = false;
        let mut ancestor = node.parent();
        while let Some(n) = ancestor {
            let unseen = in_function && n.kind() == "class_definition";
            if !unseen && self.locals.get(&n.id()).is_some_and(|names| names.contains(name)) {
                return true;
            }
            in_function |= FUNCTION_SCOPES.contains(&n.kind());
            ancestor = n.parent();
        }
        false
    }

    /// The qualified target a bare call of `name` at `node` is recorded under,
    /// when `name` is a named import — or a CommonJS `require` binding, read
    /// as its module's default export (S-540) — not shadowed by a local
    /// binding.
    fn named_target(&self, node: Node<'_>, name: &str) -> Option<&str> {
        let target = self.named.get(name).or_else(|| self.defaults.get(name))?;
        (!self.locally_bound(node, name)).then_some(target.as_str())
    }

    /// Record each type-relation row of the type at `node` that names a named
    /// import by the module it was imported from (S-535), as
    /// [`named_target`](ImportBindings::named_target) records a call: `User`
    /// after `import { User } from './user'` reads `.::user::User`. Only a
    /// TypeScript import fills the named map, so no other language's rows move.
    fn qualify_relations(&self, node: Node<'_>, rows: &mut [RefFact]) {
        for row in rows {
            if let Some(imported) = self.named_target(node, &row.target) {
                row.target = imported.to_string();
            }
        }
    }

    /// The specifier target of `method_name`'s receiver, and the receiver's
    /// name when the row names the binding it went through (Python's, S-544),
    /// when that receiver is a plain identifier naming an imported module — a
    /// Go `selector_expression` operand, a TypeScript `member_expression`
    /// object, or a Python `attribute` object, which may name an imported class
    /// too (S-544) — else `None`.
    ///
    /// A TypeScript receiver naming a named, aliased or default import
    /// qualifies with that import's `<specifier target>::<exported name>`
    /// (S-536, [FR-RS-20]): `UserMapper.toDomain(x)` after `import {
    /// UserMapper } from './user.mapper'` reads
    /// `.::user.mapper::UserMapper::toDomain`. That is also how a bare call of
    /// `toDomain` imported from `./user.mapper/UserMapper` reads, so the row
    /// names its receiver as its alias: the binder decides it among the
    /// methods of the one class the import reaches, and never as a module's
    /// function — and leaves it unbound when the name is a function or a value,
    /// as it was before. A local name both maps bind (invalid code) qualifies
    /// with neither.
    ///
    /// [FR-RS-20]: ../../../docs/specs/requirements/FR-RS-20.md
    fn qualifier_of<'s>(&self, method_name: Node<'_>, source: &'s [u8]) -> Option<(&str, Option<&'s str>)> {
        if self.qualifiers.is_empty() && self.named.is_empty() {
            return None;
        }
        let access = method_name.parent()?;
        let receiver = match access.kind() {
            "selector_expression" => access.child_by_field_name("operand")?,
            "member_expression" | "attribute" => access.child_by_field_name("object")?,
            _ => return None,
        };
        if receiver.kind() != "identifier" {
            return None;
        }
        let name = receiver.utf8_text(source).ok()?;
        let (module, named) = match (self.qualifiers.get(name), self.named.get(name)) {
            (Some(module), None) => (module, false),
            (None, Some(module)) => (module, true),
            _ => return None,
        };
        let alias = (named || self.names_qualifier).then_some(name);
        (!self.locally_bound(receiver, name)).then_some((module.as_str(), alias))
    }
}

/// The node kinds that open a function scope in the path-grammar languages —
/// TypeScript/JavaScript functions, arrows and methods, Go functions, methods
/// and function literals — and in Python, its functions and lambdas (S-544).
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
    "function_definition",
    "lambda",
];

/// Every name bound inside a function scope of the file, keyed by the scope's
/// node id (S-440) — the evidence that a reference names a local value.
///
/// Conservative by construction: a name bound anywhere in a scope (in any
/// block of it, before or after the reference) counts for the whole scope, so
/// an over-approximation can only turn a call through an import back into a
/// bare, unresolved one — a missed edge, never a fabricated one ([NFR-RA-05]).
/// A binding at file scope is collected only when `file_scope` says the
/// language allows one (Python, S-544), keyed by the root, so it shadows the
/// import for the whole file; TypeScript and Go both reject a top-level
/// redeclaration of an imported name. A Python class body's bindings are keyed
/// by the class: they shadow the import for the body itself, comprehensions
/// included, and never inside its methods, which do not see them
/// ([`ImportBindings::locally_bound`]).
///
/// Not the recorded local-binding facts ([`Facts::local_bindings`], S-593):
/// those are kinded, block-scoped and positioned, read off each plugin's
/// `locals` query, and decide a bare call a binding in force shadows (S-594,
/// [`local_shadow`]) before this set is asked. This set stays the whole-scope
/// over-approximation the import rewrite has always shadowed by: replacing it
/// would put an import back in force wherever a binding sits after the call
/// or in a sibling block, and lose the file and class-body rebinds of Python
/// that the facts, recorded inside callables only, never hold.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn local_bindings(root: Node<'_>, source: &[u8], file_scope: bool) -> HashMap<usize, HashSet<String>> {
    let mut out: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        if !binds_a_name(node) {
            continue;
        }
        let scope = match binding_scope(node) {
            Some(scope) => scope,
            None if file_scope => root,
            None => continue,
        };
        if let Ok(name) = node.utf8_text(source) {
            out.entry(scope.id()).or_default().insert(name.to_string());
        }
    }
    out
}

/// Every name the file binds outside every function scope, or assigns where
/// `shadowed` says no enclosing function binds it (S-540) — the evidence that
/// a CommonJS `require` binding may not name its module where it is called
/// ([`ImportBindings::forget_rebound_requires`]). The declarators in `own`,
/// the `require` bindings themselves, are skipped.
fn rebound_names(
    root: Node<'_>,
    source: &[u8],
    own: &HashSet<usize>,
    shadowed: impl Fn(Node<'_>, &str) -> bool,
) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if own.contains(&node.id()) {
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        let (name, assigned) = match node.kind() {
            "assignment_expression" | "augmented_assignment_expression" => {
                (node.child_by_field_name("left").filter(|left| left.kind() == "identifier"), true)
            }
            _ => ((binds_a_name(node) && binding_scope(node).is_none()).then_some(node), false),
        };
        let Some((target, text)) = name.and_then(|n| Some((n, n.utf8_text(source).ok()?))) else {
            continue;
        };
        if !(assigned && shadowed(target, text)) {
            out.insert(text.to_string());
        }
    }
    out
}

/// The scope `node` binds its name in ([`local_bindings`]): the nearest
/// function scope or Python class body around it, or `None` for the file —
/// which a Python `global a` also names.
fn binding_scope(node: Node<'_>) -> Option<Node<'_>> {
    if node.parent().is_some_and(|p| p.kind() == "global_statement") {
        return None;
    }
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if FUNCTION_SCOPES.contains(&n.kind()) || n.kind() == "class_definition" {
            return Some(n);
        }
        ancestor = n.parent();
    }
    None
}

/// Whether `node` is an identifier in a **binding** position: a parameter, a
/// declarator name, a destructuring pattern element, a `catch` parameter, a
/// `for … of` variable, a Go parameter / `:=` / `range` left-hand side, or a
/// Python parameter, assignment target, loop or comprehension variable, `with`
/// / `except` target, walrus name, `match` capture, type parameter or `global`
/// name (S-544).
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
        // Python: `def f(a, b: T, c=1, d: T = 1, *e, **g)`, `lambda a: …`, the
        // targets of `a = …`, `a += …`, `a, b = …`, `for a in …`, `[… for a in
        // …]`, `with … as a`, `except E as a` and `(a := …)`, a `case [a, *b]:`
        // splat, `def f[T]()` and `global a`.
        "parameters" | "lambda_parameters" | "list_splat_pattern" | "dictionary_splat_pattern"
        | "pattern_list" | "tuple_pattern" | "list_pattern" | "as_pattern_target" | "splat_pattern"
        | "global_statement" => true,
        "type" => parent.parent().is_some_and(|gp| gp.kind() == "type_parameter"),
        // A `case a:` or `case P(x=a):` capture: a one-name pattern.
        "dotted_name" => {
            parent.named_child_count() == 1
                && parent.parent().is_some_and(|gp| matches!(gp.kind(), "case_pattern" | "keyword_pattern"))
        }
        // `with … as (a, b)`: the elements of a target tuple or list.
        "tuple" | "list" => {
            let mut target = Some(parent);
            while let Some(t) = target.filter(|t| matches!(t.kind(), "tuple" | "list" | "parenthesized_expression")) {
                target = t.parent();
            }
            target.is_some_and(|t| t.kind() == "as_pattern_target")
        }
        "typed_parameter" => !is_field("type"),
        "default_parameter" | "typed_default_parameter" | "named_expression" => is_field("name"),
        "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => is_field("left"),
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
    defaults: HashMap<String, Option<String>>,
    /// The `variable_declarator` of each CommonJS binding read (S-540): the
    /// declaration it makes is the import, never a shadow of it.
    requires: HashSet<usize>,
    /// The local names those bindings bind (S-540).
    require_locals: HashSet<String>,
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
    /// `import … from '<spec>'`, `require('<spec>')` or `import('<spec>')`, or
    /// a Go `import [alias] "<path>"`.
    fn read(&mut self, spec_node: Node<'_>, semantics: &Semantics) {
        let Some(spec) = self.text(spec_node) else { return };
        let segments = specifier_segments(&spec, &semantics.specifier_extensions);
        let (false, Some(statement)) = (segments.is_empty(), spec_node.parent()) else {
            return;
        };
        let target = segments.join("::");
        match statement.kind() {
            "import_statement" => {
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
            "arguments" if semantics.commonjs => self.read_require(statement, &target),
            // A dynamic `import()` argument, or a `require()` one of a language
            // whose modules are not CommonJS: binds no name.
            _ => {}
        }
    }

    /// The argument list of a CommonJS `require('<spec>')` (S-540,
    /// [FR-RS-22]), read only when the call is the whole initialiser of a
    /// declarator in a declaration at the top of the file:
    ///
    /// - `const m = require('./x')` binds `m` as a qualifier of `x`, so
    ///   `m.f()` reads `x::f`, and as `x`'s default export, so `m()` reads
    ///   `x::default` — the module's export object;
    /// - `const { f, g: h } = require('./x')` binds `f` and `h` as named
    ///   imports of `x`'s `f` and `g`, a default value (`{ f = d }`) included.
    ///
    /// A `require` whose value is used any other way binds no name: called
    /// (`require('./x')(User)`), read for a property (`require('./x').y`),
    /// assigned later, or bound inside a block or a function, where the name is
    /// not the file's. A rest element or a nested pattern binds none either.
    ///
    /// [FR-RS-22]: ../../../docs/specs/requirements/FR-RS-22.md
    fn read_require(&mut self, arguments: Node<'_>, target: &str) {
        let Some(call) = arguments.parent().filter(|c| c.kind() == "call_expression") else {
            return;
        };
        let Some(declarator) = call
            .parent()
            .filter(|d| d.kind() == "variable_declarator" && d.child_by_field_name("value") == Some(call))
        else {
            return;
        };
        let top_level = declarator
            .parent()
            .and_then(|declaration| declaration.parent())
            .is_some_and(|scope| scope.kind() == "program");
        let Some(pattern) = declarator.child_by_field_name("name").filter(|_| top_level) else {
            return;
        };
        match pattern.kind() {
            "identifier" => {
                let Some(local) = self.text(pattern) else { return };
                Self::bind(&mut self.qualifiers, local.clone(), target.to_string());
                Self::bind(&mut self.defaults, local.clone(), format!("{target}::{DEFAULT_EXPORT}"));
                self.require_locals.insert(local);
            }
            "object_pattern" => {
                let mut cursor = pattern.walk();
                let properties: Vec<Node<'_>> = pattern.named_children(&mut cursor).collect();
                for property in properties {
                    if let Some((exported, local)) = self.require_property(property) {
                        Self::bind(&mut self.named, local.clone(), format!("{target}::{exported}"));
                        self.require_locals.insert(local);
                    }
                }
            }
            _ => return,
        }
        self.requires.insert(declarator.id());
    }

    /// One property of a destructured `require` (S-540): the name the module
    /// exports, and the local name it binds — `f` (`f`, `f`), `g: h` (`g`,
    /// `h`), and either with a default value. `None` for a rest element, a
    /// computed or quoted key, or a nested pattern.
    fn require_property(&self, property: Node<'_>) -> Option<(String, String)> {
        // `{ f = d }` and `{ g: h = d }` bind the name left of the default.
        fn defaulted<'t>(n: Node<'t>, kind: &str) -> Option<Node<'t>> {
            match n.kind() == kind {
                true => n.child_by_field_name("left"),
                false => Some(n),
            }
        }
        match property.kind() {
            "shorthand_property_identifier_pattern" | "object_assignment_pattern" => {
                let name = defaulted(property, "object_assignment_pattern")?;
                let name = self.text(name).filter(|_| name.kind() == "shorthand_property_identifier_pattern")?;
                Some((name.clone(), name))
            }
            "pair_pattern" => {
                let key = property.child_by_field_name("key").filter(|k| k.kind() == "property_identifier")?;
                let local = defaulted(property.child_by_field_name("value")?, "assignment_pattern")?;
                let local = self.text(local).filter(|_| local.kind() == "identifier")?;
                Some((self.text(key)?, local))
            }
            _ => None,
        }
    }

    /// A TypeScript `import_clause`: its default import, named imports and
    /// namespace import. A default import `import App from './app'` binds the
    /// module's `default` export (S-534), exactly as `{ default as App }` does,
    /// so a call through it is recorded as `<module>::default` and the binder
    /// follows the module's default-export alias.
    fn read_ts_clause(&mut self, clause: Node<'_>, target: &str) {
        let mut cursor = clause.walk();
        let parts: Vec<Node<'_>> = clause.named_children(&mut cursor).collect();
        for part in parts {
            match part.kind() {
                "identifier" => {
                    if let Some(local) = self.text(part) {
                        Self::bind(&mut self.named, local, format!("{target}::{DEFAULT_EXPORT}"));
                    }
                }
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
        if let Some((exported, local)) = ts_specifier_names(specifier, self.source) {
            Self::bind(&mut self.named, local, format!("{target}::{exported}"));
        }
    }

    /// One `@ref.import` capture of a dotted-name language — Python's `import
    /// a.b [as x]` or `from m import a [as x]` (S-544, [FR-RS-23]): the local
    /// name it binds qualifies a call through it by the path its `Imports` row
    /// records ([`import_row`], so the two are spelled alike). `from m import a`
    /// binds `a`, which may be a module or a class — the binder decides which.
    /// `import a.b` binds the head `a` to the package `a`, as Python does; with
    /// `as x` it binds `x` to `a.b`. A wildcard binds no name.
    ///
    /// [FR-RS-23]: ../../../docs/specs/requirements/FR-RS-23.md
    fn read_dotted(
        &mut self,
        captures: &[QueryCapture<'_>],
        capture_names: &[&str],
        node: Node<'_>,
        semantics: &Semantics,
    ) {
        let Some(text) = self.text(node) else { return };
        let Some((mut segments, RefForm::Path, Some(mut local))) =
            import_row(captures, capture_names, self.source, &text, semantics)
        else {
            return;
        };
        let marked = |name: &str| match_capture(captures, capture_names, name).is_some();
        if !marked(FROM_MODULE_CAPTURE) && !marked(IMPORT_ALIAS_CAPTURE) {
            segments.truncate(1);
            local = segments[0].clone();
        }
        Self::bind(&mut self.qualifiers, local, segments.join("::"));
    }
}

/// The two names of a TypeScript `import_specifier` or `export_specifier` —
/// `a`, or `a as b`: the name on the module's side (`a`), and the name on the
/// other (`b`, else `a`). `None` for a specifier whose `name` is not an
/// identifier (a string name, `export { "a-b" as c }`).
fn ts_specifier_names(specifier: Node<'_>, source: &[u8]) -> Option<(String, String)> {
    let text = |n: Node<'_>| n.utf8_text(source).ok().map(str::to_string);
    let name = specifier
        .child_by_field_name("name")
        .filter(|n| n.kind() == "identifier")
        .and_then(text)?;
    let other = specifier.child_by_field_name("alias").and_then(text).unwrap_or_else(|| name.clone());
    Some((name, other))
}

/// The name of a generic callee `M<T>` — its identifier, the type arguments
/// dropped (S-553, C#'s `generic_name`). The query captures the whole name so
/// it shares its parent with the receiver markers; `None` for any other node.
fn generic_callee_name<'s>(node: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    if node.kind() != "generic_name" {
        return None;
    }
    let mut cursor = node.walk();
    let name = node.named_children(&mut cursor).find(|n| n.kind() == "identifier")?;
    std::str::from_utf8(&source[name.byte_range()]).ok()
}

/// The trait head when a receiver-method call's receiver is **provably**
/// trait-typed: a trait object (S-281, [CR-073], [FR-RS-08]), an `impl Tr`, or
/// a generic parameter bounded by traits (S-608, [FR-RS-47] rule 5). One trait
/// is its simple name (`Run`); several bounds are joined by `+` (`Run+Walk`),
/// in written order, and the binder decides which of them provides the method.
///
/// `field_ident` is the `@ref.method` capture — the method-name `field_identifier`
/// of a `field_expression`. The receiver must be a plain named `identifier` whose
/// trait-typed type ([`receiver_traits_of_type`]) is provable **from this file's
/// own syntax**: an explicit parameter type on the enclosing `function_item`, or
/// a `let recv: &dyn T` binding in its body, a generic parameter read against
/// the bounds in scope there ([`generic_trait_bounds`]). This is a superset-free, per-file-pure gate — a receiver
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
/// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
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
    receiver_trait_head(field_expr, receiver.utf8_text(source).ok()?, source)
}

/// The trait head of the receiver named `recv_name` at `site` — a method
/// call, or a macro invocation whose token tree holds one (S-610 parity,
/// [`macro_rows`]): read in the function enclosing `site`, whose parameters and
/// `let` bindings are the only per-file-provable sources of a receiver's
/// trait-typed type ([`function_dyn_binding`]).
fn receiver_trait_head(site: Node<'_>, recv_name: &str, source: &[u8]) -> Option<String> {
    let mut anc = site.parent();
    while let Some(n) = anc {
        if n.kind() == "function_item" {
            return function_dyn_binding(n, recv_name, source);
        }
        anc = n.parent();
    }
    None
}

/// The trait head of a name `recv` inside `fn_node` ([`rust_dyn_receiver_trait`])
/// — an explicit parameter type (preferred), else a `let recv: &dyn T` binding
/// in the body.
/// A nested `function_item` is not descended (its bindings belong to its own
/// scope, and it cannot capture `recv`), so a shadowing inner binding cannot
/// mis-type an outer receiver.
fn function_dyn_binding(fn_node: Node<'_>, recv_name: &str, source: &[u8]) -> Option<String> {
    // Collect EVERY binding of `recv_name` in this function's own scope — each
    // parameter and each body `let` whose pattern is exactly the receiver name,
    // as its optional type-annotation node, and every other pattern that binds
    // it (S-608): a destructuring `let`, a `for`, an `if let` / `while let`, a
    // `match` arm or a closure's parameter, each untyped. A closure's body is
    // walked: it shares the function's scope for every name it does not bind.
    //
    // The proof must be a *single* per-file type: a name bound more than once is
    // shadowed, and this walk is scope-blind (it cannot tell which binding is live
    // at the call site), so guessing one would fabricate a dispatch edge — e.g. a
    // `c: &Concrete` parameter shadowed by a later `let c: &dyn T` would wrongly
    // qualify the *first* `c.method()` as `T::method`, and a `t: T` parameter
    // shadowed by `for t in xs` would type the loop's `t.go()` by `T`'s bound.
    // Bail on anything but exactly one binding, and require that binding to be
    // annotated: an unambiguous, annotated binding qualifies; zero, several, or
    // an un-annotated binding is an honest miss ([NFR-RA-05]).
    let name_of = |n: Node<'_>| {
        n.child_by_field_name("pattern")
            .and_then(|p| p.utf8_text(source).ok())
    };
    let binds = |pattern: Option<Node<'_>>| {
        let mut stack: Vec<Node<'_>> = pattern.into_iter().collect();
        while let Some(n) = stack.pop() {
            if n.kind() == "identifier" && n.utf8_text(source).ok() == Some(recv_name) {
                return true;
            }
            stack.extend((0..n.named_child_count()).filter_map(|i| n.named_child(i)));
        }
        false
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
            match n.kind() {
                "let_declaration" if name_of(n) == Some(recv_name) => bindings.push(n.child_by_field_name("type")),
                "let_declaration" | "for_expression" | "let_condition" | "match_arm"
                    if binds(n.child_by_field_name("pattern")) =>
                {
                    bindings.push(None);
                }
                "closure_expression" if binds(n.child_by_field_name("parameters")) => bindings.push(None),
                _ => {}
            }
            for i in 0..n.child_count() {
                if let Some(ch) = n.child(i) {
                    if ch.kind() != "function_item" {
                        stack.push(ch);
                    }
                }
            }
        }
    }
    match bindings.as_slice() {
        [Some(ty)] => receiver_traits_of_type(*ty, source, &generic_trait_bounds(fn_node, source))
            .map(|traits| traits.join("+")),
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
/// A state declaration ([`STATE_CAPTURE`], [`settle_state`]) is placed at its
/// class or module whatever lies between ([`state_owner`]). Otherwise one
/// exception: a [`NodeKind::Field`] declared **in a method's parameter list**
/// — a TypeScript parameter property, `constructor(private readonly http: Client)`,
/// or a PHP promoted property, `__construct(private Logger $log)` (S-548, pinned by
/// `php_members.rs`) — is owned by the class the method belongs to, not by the
/// method. Lexically it nests in the constructor, but it is a member of the
/// class: the own-field access binder looks a field up among the class's members
/// ([FR-EX-08]), and the class-cohesion metric counts the class's fields
/// ([FR-QM-11]). The test is
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
    let id_to_idx = decl_index(decls);

    let scopeless: Vec<bool> = decls.iter().map(owns_no_scope).collect();
    // State is placed at its class or module, never the method it sits in (S-564).
    let owners: Vec<Option<usize>> = decls
        .iter()
        .map(|d| if d.state { state_owner(d, decls, &id_to_idx) } else { None })
        .collect();
    for (i, decl) in decls.iter_mut().enumerate() {
        if decl.state {
            decl.parent = owners[i];
            continue;
        }
        // A member a torn head claims sits at that head (S-582, `member`).
        if let Some(head) = decl.member.claimed_by() {
            decl.parent = id_to_idx.get(&head).copied();
            continue;
        }
        // `node` is `Copy`, so walking the ancestry borrows nothing from `decl`.
        let mut ancestor = decl.node.parent();
        while let Some(node) = ancestor {
            if let Some(&j) = id_to_idx.get(&node.id()).filter(|j| !scopeless[**j]) {
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

    // A declaration placed by name is owned by what its assignment names, or
    // by the object or function it is written in (S-539).
    owner::place(decls, &id_to_idx);
}

/// Give each attribute declaration ([`ATTRIBUTE_CAPTURE`], S-543) its class,
/// and keep one per name per class. Runs on the parents [`assign_parents`]
/// resolved.
///
/// An attribute whose parent is class-like ([`crate::resolve::is_class_like`]) —
/// a class-body `x = …` — is that class's. One whose parent is a callable whose
/// own parent is class-like — a method's `self.x = …` — is lifted to that class.
/// Any other — a `self.x = …` in a function nested in a method, or in a free
/// function — has no class to belong to and is dropped. Of the attributes one
/// class declares under one name, the class-body one wins over a method write,
/// then the first in source order; the rest are dropped, so the class holds one
/// Field per name. A method's attribute written inside a function of its own
/// that rebinds `this` ([`THIS_REBINDING_KINDS`]) is no class's and is dropped. A Field the class declares in its own right — JavaScript's
/// `x = 0;` beside a constructor's `this.x = …` (S-542) — is that name's
/// declaration already: every attribute of it is dropped.
///
/// A dropped attribute is a name node, so no declaration sits inside it: the
/// indices are compacted and every parent is remapped without orphaning one.
fn settle_attributes(decls: &mut Vec<Decl<'_>>) {
    if !decls.iter().any(|d| d.attribute) {
        return;
    }
    let class_like = |i: usize| crate::resolve::is_class_like(decls[i].kind);
    let callable = |i: usize| matches!(decls[i].kind, NodeKind::Function | NodeKind::Method);
    let mut attributes: Vec<(bool, usize, usize, usize)> = decls
        .iter()
        .enumerate()
        .filter(|(_, d)| d.attribute)
        .filter_map(|(i, d)| {
            let scope = d.parent?;
            if class_like(scope) {
                return Some((false, d.start_byte, i, scope));
            }
            let class = decls[scope].parent.filter(|&c| callable(scope) && class_like(c))?;
            (!sits_within_any(d.node, decls[scope].node, THIS_REBINDING_KINDS)).then_some((true, d.start_byte, i, class))
        })
        .collect();
    attributes.sort_unstable();
    let mut keep: Vec<bool> = decls.iter().map(|d| !d.attribute).collect();
    let mut declared: HashSet<(usize, String)> = decls
        .iter()
        .filter(|d| d.kind == NodeKind::Field && !d.attribute)
        .filter_map(|d| Some((d.parent.filter(|&c| class_like(c))?, d.name.clone())))
        .collect();
    for (_, _, i, class) in attributes {
        if declared.insert((class, decls[i].name.clone())) {
            keep[i] = true;
            decls[i].parent = Some(class);
        }
    }
    let mut remap: Vec<Option<usize>> = Vec::with_capacity(decls.len());
    let mut next = 0;
    for &kept in &keep {
        remap.push(kept.then_some(next));
        next += usize::from(kept);
    }
    let mut index = 0;
    decls.retain(|_| {
        index += 1;
        keep[index - 1]
    });
    for decl in decls.iter_mut() {
        decl.parent = decl.parent.and_then(|p| remap[p]);
    }
}

/// `true` for a TypeScript/JavaScript value binding — a top-level `const x = …`
/// / `let` / `var` ([`NodeKind::Constant`]/[`NodeKind::Variable`] on a
/// `variable_declarator`) or an `export default <expression>` (the Constant
/// named `default` on its `export_statement`) — which owns no scope and no
/// reference (S-532).
///
/// Such a binding is a declaration, but what lexically sits inside its value
/// was the file's before the binding was captured, and stays so
/// ([`assign_parents`], [`collect_refs`]): a method of an object literal or
/// class expression it holds (`const registry = { handle() {} }`,
/// `export default { start() {} }`) would be renamed by nesting it, and a
/// `const util = require('./util')` import or a `const cfg = load()` call would
/// leave the file module its resolution reads them from ([NFR-RA-06]). An
/// arrow-function binding is a Function, not a value binding, and owns what it
/// holds as it always has.
///
/// Keyed on both the kind and the node kind, so a Rust `const` block's nested
/// item (a `const_item`) and every other grammar keep their parents.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn is_value_binding(decl: &Decl<'_>) -> bool {
    matches!(decl.kind, NodeKind::Constant | NodeKind::Variable)
        && matches!(decl.node.kind(), "variable_declarator" | "export_statement")
}

/// Whether a declaration is a scope its nested declarations are placed under
/// ([`assign_parents`]): not a value binding or a wrapped component (S-532),
/// nor a declaration placed by name (S-539, [`owner`]), whose body's
/// declarations were the enclosing scope's before it was captured.
fn owns_no_scope(decl: &Decl<'_>) -> bool {
    is_value_binding(decl) || is_wrapped_component(decl) || decl.owner.is_some()
}

/// Whether a reference inside `decl` is the declaration's own ([`collect_refs`]):
/// its symbol built and it is not a value binding (S-532).
fn owns_references(decl: &Decl<'_>, symbol: &Option<LogosSymbol>) -> bool {
    symbol.is_some() && !is_value_binding(decl)
}

/// `true` for a TypeScript/JavaScript component its binding wraps — a
/// `const Button = forwardRef(…)` / `memo(…)` / `styled.div`…``, a Function on a
/// `variable_declarator` whose value is a call (S-532). An arrow or function
/// expression binding has a function value, so it is not one.
///
/// It owns the references of its body, but no scope: the functions declared
/// inside the wrapped callback (`const onClick = () => …`) were file-scope
/// before the component was captured, and nesting them under it would rename
/// every one ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn is_wrapped_component(decl: &Decl<'_>) -> bool {
    decl.kind == NodeKind::Function
        && decl.node.kind() == "variable_declarator"
        && decl.node.child_by_field_name("value").is_some_and(|value| value.kind() == "call_expression")
}

/// Whether a declaration is numbered after the others of its name and
/// [`DescriptorFamily`] (S-532): an abstract class, captured since S-532 beside
/// an `interface` of its name (a declaration merge). The interface was already
/// `X#` and keeps that symbol wherever the class sits; the class takes the next
/// ordinal. So is a PHP class constant (S-548), captured since then beside a
/// property of its name (`const name` and `$name` both render `name.`): the
/// property keeps its symbol wherever the constant sits. An enum case needs no
/// such rule: an enum declares no property, and a case and a constant of one name
/// do not compile. So is a declaration placed by name (S-539, [`owner`]): a
/// JavaScript `exports.exists = …` written before a `function exists` keeps
/// that function's symbol.
fn numbered_after_existing(decl: &Decl<'_>) -> bool {
    decl.node.kind() == "abstract_class_declaration"
        || (decl.kind == NodeKind::Constant && decl.node.kind() == "const_element")
        || decl.owner.is_some()
}

/// Assign each declaration its ordinal among same-name siblings of one
/// [`DescriptorFamily`], in the canonical sort order `(start_byte, kind, name)`
/// ([ADR-07]) — declarations a later story began capturing
/// ([`numbered_after_existing`]) after the ones the graph already held.
///
/// Numbered per family, not per kind (S-512): kinds that share a descriptor
/// suffix — a Go function and method (`name().`), a TS interface and class
/// (`name#`) — would otherwise both take ordinal 0 and render one symbol. Where
/// no scope holds two kinds of one family under one name, per-family numbering
/// equals per-kind numbering and every ID is unchanged.
///
/// An out-of-line definition merged into its declaration takes no ordinal — it
/// renders that declaration's symbol. Every other out-of-line definition is
/// numbered after all its siblings that are not, whatever its position, and
/// one rendering its own scope path ([`Decl::qualifier`]) only among the
/// definitions of that path: none moves a sibling's ordinal (S-577).
fn assign_ordinals(decls: &mut [Decl<'_>]) {
    // Group declaration indices by parent scope.
    let mut by_parent: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, d) in decls.iter().enumerate() {
        if d.completes.is_none() {
            by_parent.entry(d.parent).or_default().push(i);
        }
    }

    for (_, mut siblings) in by_parent {
        siblings.sort_by(|&a, &b| {
            (
                numbered_after_existing(&decls[a]),
                decls[a].def.out_of_line(),
                decls[a].start_byte,
                decls[a].kind.as_i32(),
                decls[a].name.as_str(),
            )
                .cmp(&(
                    numbered_after_existing(&decls[b]),
                    decls[b].def.out_of_line(),
                    decls[b].start_byte,
                    decls[b].kind.as_i32(),
                    decls[b].name.as_str(),
                ))
        });
        // Owned-String keys so the counter does not borrow `decls` while we
        // write back `ordinal`.
        let mut seen: HashMap<(DescriptorFamily, String, Vec<String>), u32> = HashMap::new();
        for idx in siblings {
            let key = (
                descriptor_family(decls[idx].kind),
                decls[idx].name.clone(),
                decls[idx].qualifier.clone(),
            );
            let ordinal = *seen.get(&key).unwrap_or(&0);
            decls[idx].ordinal = ordinal;
            seen.insert(key, ordinal + 1);
        }
    }
}

/// Build the descriptor chain (outermost scope down to the leaf) for one
/// declaration, each rendered with its own ordinal.
///
/// A merged out-of-line definition renders the chain of the declaration it
/// completes, and one with a [`Decl::qualifier`] renders that scope path as
/// type descriptors between its parent's chain and its own (S-577).
fn scope_chain(decls: &[Decl<'_>], i: usize) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = Some(i);
    while let Some(at) = current {
        let idx = decls[at].completes.unwrap_or(at);
        chain.push(descriptor_for(
            decls[idx].kind,
            &decls[idx].name,
            decls[idx].ordinal,
        ));
        for segment in decls[idx].qualifier.iter().rev() {
            chain.push(descriptor_for(NodeKind::Class, segment, 0));
        }
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
