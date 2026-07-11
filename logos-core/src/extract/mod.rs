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
//! and a warning recorded — and the run is **never** aborted.
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
// `pub(crate)`: the framework pass (resolve::framework, S-015) canonicalises
// captured handler paths and unquotes captured route-path literals with the
// same helpers extraction uses, so the two passes can never disagree on what
// a path's segments are.
pub(crate) mod refs;
// The message-broker publish/subscribe invocation arm's capture side (S-254,
// FR-WS-10): runs a grammar's optional `brokers` query and funnels topic-keyed
// sites through the generic `capture_invocation_refs` interpreter.
mod broker;
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

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rayon::prelude::*;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeKind, RefForm};
use crate::plugin::{LanguagePlugin, LanguageRegistry};

use refs::{flatten_use_tree, import_segments, macro_call_refs, split_path_text};
use symbol::{build_symbol, descriptor_for, path_segments};

/// The capture-name group prefix the `symbols` query uses (`@symbol.<kind>`).
/// The segment after it names a [`NodeKind`] by its [`NodeKind::as_str`] form.
const SYMBOL_CAPTURE_GROUP: &str = "symbol";

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
/// [FR-EX-04]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionMetrics {
    /// Cyclomatic complexity: `1 + decision points` (see [`complexity`]).
    pub cyclomatic_complexity: u32,
    /// Physical line span of the definition (`end_line - start_line + 1`).
    pub line_count: u32,
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
    /// Complexity + line count, present for `Function`/`Method` nodes only.
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
    /// Ordinal among same-`(kind, name)` siblings, in canonical sort order.
    /// Assigned in [`assign_ordinals`].
    ordinal: u32,
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
    extract_one(&mut parser, input, plugin, ctx)
}

/// Extract many files in parallel, one [`tree_sitter::Parser`] per rayon worker.
///
/// Files whose extension resolves to no loaded grammar are skipped (the
/// discovery layer, S-010, is responsible for filtering); the returned vector
/// preserves the input order of the files that *were* extracted, so the output
/// is deterministic regardless of the thread count ([NFR-RA-06], [NFR-PE-08]).
pub fn extract_files(
    inputs: &[FileInput],
    registry: &LanguageRegistry,
    ctx: &SymbolContext,
) -> Vec<Facts> {
    inputs
        .par_iter()
        // `map_init` runs the init closure once per rayon worker thread, so each
        // worker owns exactly one Parser — the AR-05 mitigation — and reuses it
        // across the files that worker handles.
        .map_init(Parser::new, |parser, input| {
            let plugin = plugin_for(registry, &input.path)?;
            Some(extract_one(parser, input, plugin, ctx))
        })
        // `rayon`'s `collect` preserves input order even through this
        // `Option`-flattening, so the result is deterministic (NFR-RA-06).
        .flatten()
        .collect()
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
    // declarations around it are still extracted (FR-IX-04).
    if tree.root_node().has_error() {
        facts.partial = true;
        facts
            .warnings
            .push("syntax error(s) present; partial extraction".to_string());
    }

    let Some(query) = plugin.query("symbols") else {
        // No symbols capability → nothing to extract, but not an error.
        return facts;
    };

    let source = input.source.as_bytes();
    let capture_names = query.capture_names();

    // 1) Collect declarations from the query matches.
    let mut decls: Vec<Decl<'_>> = Vec::new();
    // Guard against a declaration node being captured more than once (a query
    // with overlapping patterns): a duplicate would corrupt the parent map and
    // inflate ordinals, churning the symbol ID. The current `symbols.scm` has
    // one pattern per node kind so this never fires today, but it keeps the
    // ID-stability invariant (ADR-07) robust against future query authors.
    let mut seen_decls: HashSet<usize> = HashSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let Some(kind) = kind_for_capture(capture_names[cap.index as usize]) else {
                continue; // a capture we do not map to a NodeKind
            };
            // The query is expected to capture the *name* node; its parent is the
            // declaration. If a query instead captures the declaration node, the
            // parent walk simply starts one level higher — the contract is that
            // a capture identifies one declaration.
            let name_node = cap.node;
            let decl_node = lift_to_declaration(name_node.parent().unwrap_or(name_node));
            if !seen_decls.insert(decl_node.id()) {
                continue; // already captured by another pattern — keep the first
            }
            let Ok(name) = name_node.utf8_text(source) else {
                continue; // non-UTF-8 identifier slice — skip defensively
            };
            decls.push(Decl {
                node: decl_node,
                kind,
                name: name.to_string(),
                start_byte: decl_node.start_byte(),
                start_line: decl_node.start_position().row as u32 + 1,
                end_line: decl_node.end_position().row as u32 + 1,
                parent: None,
                ordinal: 0,
            });
        }
    }

    // 2) Resolve parent scopes and 3) assign canonical-sort ordinals.
    assign_parents(&mut decls);
    assign_ordinals(&mut decls);

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
                name: file_module_name(&path_segments),
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
                });
            }
        }
        let is_callable = matches!(node_kind, NodeKind::Function | NodeKind::Method);
        let metrics = is_callable.then(|| FunctionMetrics {
            cyclomatic_complexity: complexity::cyclomatic_complexity(decl.node, keywords),
            // `end_line >= start_line` always holds for a tree-sitter node;
            // `saturating_sub` is belt-and-braces against any future change.
            line_count: decl.end_line.saturating_sub(decl.start_line) + 1,
        });
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

    // 8) HTTP client-call arm (S-252, CR-061, FR-WS-08): capture outbound calls
    // via the optional `invocations` capability and funnel them through the
    // shared S-251 interpreter with the `route_key`-based normalizer. A grammar
    // without the capability produces none; the interpreter's `push_artifact_ref`
    // choke-point applies the external gate and `HttpClientCall.edge_kind()`.
    //
    // Ledger-gated candidacy (the consumer-side mirror of the framework pass's
    // FR-FW-04): the `.scm` anchor is a broad `<receiver>.<method>(<arg>)` shape,
    // so it is captured ONLY in a file that references a known HTTP-client crate.
    // Without this gate an incidental collection/registry call whose key looks
    // like a route (`perms.get("/admin/users")`, a route-table `.get("/health")`)
    // would be captured, normalize, and fabricate a cross-service edge — exactly
    // what never-fabricate forbids ([NFR-RA-05]).
    if let Some(inv_query) = plugin.query("invocations") {
        let detectors = crate::resolve::http_client_call::http_client_crates(plugin.name());
        let is_http_client_file = !detectors.is_empty()
            && facts.refs.iter().any(|r| {
                let head = r.target.split("::").next().unwrap_or_default();
                detectors.contains(&head)
            });
        if is_http_client_file {
            let sites = collect_invocation_sites(
                inv_query,
                tree.root_node(),
                source,
                &decls,
                &symbols,
                file_module.as_ref(),
            );
            if !sites.is_empty() {
                crate::extract::config::capture_invocation_refs(
                    &mut facts,
                    ArtifactRelation::HttpClientCall,
                    RefForm::Path,
                    sites,
                    crate::resolve::http_client_call::render_client_call_target,
                );
                // Re-canonicalize: the interpreter appended to `facts.refs`.
                dedup_sort_refs(&mut facts.refs);
            }
        }
    }

    // 7b) Cross-service invocation arms (S-254, [FR-WS-10]): a code arm captures
    // its publish/subscribe sites through its own optional `brokers` query and
    // funnels them through the generic invocation interpreter. A grammar without
    // the capability contributes nothing. The site's source symbol is its
    // innermost enclosing declaration — the same attribution `collect_refs` uses.
    if let Some(broker_query) = plugin.query("brokers") {
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
            file_module.clone()
        };
        if broker::capture_broker_invocations(
            broker_query,
            tree.root_node(),
            source,
            enclosing,
            &mut facts,
        ) > 0
        {
            // Broker refs are appended after the code-reference sort; restore the
            // canonical ledger order + dedup so the output stays byte-stable.
            dedup_sort_refs(&mut facts.refs);
        }
    }

    sort_facts(&mut facts);
    facts
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
/// `(source, target, form, kind, relation)` — the same reference on two lines is
/// one ref, first wins — then sort into that canonical order ([NFR-RA-06]).
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
    let mut seen: HashSet<(String, String, i32, i32, Option<&'static str>)> = HashSet::new();
    refs.retain(|r| {
        seen.insert((
            r.source.as_str().to_string(),
            r.target.clone(),
            r.form.as_i32(),
            r.kind.as_i32(),
            relation_token(r),
        ))
    });
    refs.sort_by(|a, b| {
        (
            a.source.as_str(),
            &a.target,
            a.form.as_i32(),
            a.kind.as_i32(),
            relation_token(a),
        )
            .cmp(&(
                b.source.as_str(),
                &b.target,
                b.form.as_i32(),
                b.kind.as_i32(),
                relation_token(b),
            ))
    });
}

/// The human-facing name of a file's module node: the file stem, or — for the
/// `mod`/`lib`/`main` stems that name their *enclosing* module — the nearest
/// preceding path segment that is not `src`, falling back to `crate`.
///
/// Display-only: resolution computes real module paths independently, so this
/// name carries no binding semantics (it is what FTS search shows).
fn file_module_name(path_segments: &[&str]) -> String {
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
    if !stem.is_empty() && !matches!(stem.as_str(), "mod" | "lib" | "main") {
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
/// [`split_path_text`] / [`flatten_use_tree`], deduplicated, and sorted into
/// the canonical `(source, target, form, kind)` order ([NFR-RA-06]).
fn collect_refs(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
) -> Vec<RefFact> {
    let id_to_idx: HashMap<usize, usize> = decls
        .iter()
        .enumerate()
        .map(|(i, d)| (d.node.id(), i))
        .collect();
    // The symbol of the innermost enclosing captured declaration, or the file
    // module at file scope. A declaration whose own symbol failed to build
    // defers to the next enclosing scope.
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

    let capture_names = query.capture_names();
    let mut out: Vec<RefFact> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let node = cap.node;
            let Some(source_symbol) = enclosing_symbol(node) else {
                continue; // no attributable scope (file-module symbol failed)
            };
            let line = node.start_position().row as u32 + 1;
            let Ok(text) = node.utf8_text(source) else {
                continue; // non-UTF-8 slice — skip defensively
            };
            match capture_names[cap.index as usize] {
                "ref.call" => {
                    let segments = split_path_text(text);
                    if segments.is_empty() {
                        continue;
                    }
                    out.push(RefFact {
                        source: source_symbol,
                        target: segments.join("::"),
                        alias: None,
                        form: RefForm::Path,
                        kind: EdgeKind::Calls,
                        line,
                        relation: None,
                    });
                }
                "ref.method" => {
                    let name = text.trim();
                    if name.is_empty() {
                        continue;
                    }
                    // Trait-object dynamic dispatch (S-281, CR-073, FR-RS-08): when
                    // the receiver is a *provable* `&dyn T` (an explicit parameter
                    // or `let` type in the enclosing fn), qualify the target as
                    // `T::f` so the binder fans out to the trait method's impls. A
                    // receiver of unknown type stays the bare `f`, taking the
                    // ordinary receiver-method path — the CR-066 guard (FR-RS-06)
                    // is not loosened.
                    let target = match rust_dyn_receiver_trait(node, source) {
                        Some(trait_name) => format!("{trait_name}::{name}"),
                        None => name.to_string(),
                    };
                    out.push(RefFact {
                        source: source_symbol,
                        target,
                        alias: None,
                        form: RefForm::Method,
                        kind: EdgeKind::Calls,
                        line,
                        relation: None,
                    });
                }
                // The language-agnostic import capture (S-015): the captured
                // node's *text* is one import path — a Python dotted name, a
                // Go/TS quoted module string, a Java scoped identifier. The
                // Rust grammar keeps `ref.use` below because its use-trees
                // (groups, renames, globs) need a structural walk no text
                // split can express.
                "ref.import" => {
                    let segments = import_segments(text);
                    if segments.is_empty() {
                        continue;
                    }
                    out.push(RefFact {
                        source: source_symbol,
                        alias: segments.last().cloned(),
                        target: segments.join("::"),
                        form: RefForm::Path,
                        kind: EdgeKind::Imports,
                        line,
                        relation: None,
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
                    for call in macro_call_refs(node, source) {
                        if call.target.is_empty() {
                            continue;
                        }
                        out.push(RefFact {
                            source: source_symbol.clone(),
                            target: call.target,
                            alias: None,
                            form: call.form,
                            kind: EdgeKind::Calls,
                            line: call.line,
                            relation: None,
                        });
                    }
                }
                "ref.use" => {
                    let mut items = Vec::new();
                    flatten_use_tree(node, source, &mut items);
                    for item in items {
                        let (form, alias) = if item.glob {
                            (RefForm::Glob, None)
                        } else {
                            (RefForm::Path, item.alias)
                        };
                        out.push(RefFact {
                            source: source_symbol.clone(),
                            target: item.path.join("::"),
                            alias,
                            form,
                            kind: EdgeKind::Imports,
                            line,
                            relation: None,
                        });
                    }
                }
                _ => {} // a capture this pass does not consume
            }
        }
    }

    // Dedup on the ledger's uniqueness key, then canonical sort (NFR-RA-06).
    dedup_sort_refs(&mut out);
    out
}

/// The HTTP verbs the client-call arm (S-252, [FR-WS-08]) captures. A method call
/// whose name is not one of these is not an outbound HTTP call.
///
/// This name filter is **necessary but not sufficient**: a collection/registry
/// method that shares a verb name (`HashMap::get`) still passes it, and a
/// `/`-prefixed string key (`map.get("/health")`) would normalize and fabricate a
/// cross-service edge. Two further guards make the capture honest ([NFR-RA-05]):
/// the file-level HTTP-client-crate gate in `extract_one` (only a file that uses
/// a client crate is a candidate at all), and the arm's normalizer (which refuses
/// any non-static / non-absolute / non-normalizable path). The residual ceiling —
/// a genuine HTTP-client file that also does an incidental `/`-keyed collection
/// `.get` — is a documented accuracy ceiling ([ADR-54]), reported unbound-or-not
/// at worst, never silently guessed.
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
/// content children (e.g. a raw-string form) falls back to trimming its quote and
/// prefix characters.
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
            "string_content" | "string_fragment" | "escape_sequence" => {
                content.push_str(child.utf8_text(source).ok()?);
            }
            // An interpolation / template substitution / expansion → dynamic.
            _ => return None,
        }
    }
    if !saw_child {
        // A literal whose grammar exposes no content node (e.g. a raw string):
        // strip the surrounding quote and literal-prefix characters.
        let raw = node.utf8_text(source).ok()?;
        content.push_str(
            raw.trim_start_matches(['r', 'b', '#'])
                .trim_matches(['"', '\'', '`', '#']),
        );
    }
    let content = content.trim().to_string();
    (!content.is_empty()).then_some(content)
}

/// Collect the file's outbound **HTTP client-call** invocation sites from the
/// `invocations` query matches (S-252, [FR-WS-08], [ADR-54]).
///
/// The code-side twin of [`collect_refs`] for the pluggable invocation-arm
/// contract: each match anchors a `<receiver>.<method>(<first-arg>)` call; a site
/// is emitted only when the method is an HTTP verb ([`is_http_method`]). The first
/// argument becomes the arm's slots — a static string literal fills the `path`
/// slot ([`PATH_SLOT`](crate::resolve::http_client_call::PATH_SLOT)); any other
/// shape sets the dynamic-path marker
/// ([`DYNAMIC_PATH_SLOT`](crate::resolve::http_client_call::DYNAMIC_PATH_SLOT)) so
/// the arm's normalizer refuses it as base-url-runtime. The sites are funnelled
/// through the shared [`capture_invocation_refs`](crate::extract::config::capture_invocation_refs)
/// interpreter by the caller; no reference is emitted here, and no judgment of
/// bind-ability is made — that is the normalizer's job ([NFR-RA-05]).
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn collect_invocation_sites(
    query: &Query,
    root: Node<'_>,
    source: &[u8],
    decls: &[Decl<'_>],
    symbols: &[Option<LogosSymbol>],
    file_module: Option<&LogosSymbol>,
) -> Vec<crate::extract::config::InvocationSite> {
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

    let capture_names = query.capture_names();
    let mut sites = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    while let Some(m) = matches.next() {
        let mut method_node = None;
        let mut arg_node = None;
        for cap in m.captures {
            match capture_names[cap.index as usize] {
                "invoke.http.method" => method_node = Some(cap.node),
                "invoke.http.arg" => arg_node = Some(cap.node),
                _ => {}
            }
        }
        let (Some(method_node), Some(arg_node)) = (method_node, arg_node) else {
            continue;
        };
        let Ok(method) = method_node.utf8_text(source) else {
            continue;
        };
        let method = method.trim();
        // Narrow the broad method-call anchor to HTTP verbs (a map/collection
        // `.get(...)` is not an outbound call).
        if !is_http_method(method) {
            continue;
        }
        let Some(source_symbol) = enclosing_symbol(method_node) else {
            continue; // no attributable scope
        };
        let line = method_node.start_position().row as u32 + 1;

        let mut slots = std::collections::BTreeMap::new();
        slots.insert(METHOD_SLOT.to_string(), method.to_string());
        match static_string_literal(arg_node, source) {
            // A static string literal → the `"METHOD /template"` path candidate.
            Some(path) => {
                slots.insert(PATH_SLOT.to_string(), path);
            }
            // Anything else → a runtime-composed path; the marker's presence makes
            // the normalizer refuse it (base-url-runtime), no target guessed.
            None => {
                let raw = arg_node
                    .utf8_text(source)
                    .ok()
                    .map(|t| t.trim().to_string())
                    .unwrap_or_default();
                slots.insert(DYNAMIC_PATH_SLOT.to_string(), raw);
            }
        }
        sites.push(crate::extract::config::InvocationSite {
            source: source_symbol,
            slots,
            line,
        });
    }
    sites
}

/// Lift a captured name's parent past any C-family *declarator* wrapper to the
/// body-bearing declaration/definition that owns it (S-058).
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
fn lift_to_declaration(node: Node<'_>) -> Node<'_> {
    const CFAMILY_DECLARATORS: [&str; 6] = [
        "function_declarator",
        "pointer_declarator",
        "reference_declarator",
        "array_declarator",
        "parenthesized_declarator",
        "init_declarator",
    ];
    let mut decl = node;
    while CFAMILY_DECLARATORS.contains(&decl.kind()) {
        match decl.parent() {
            Some(parent) => decl = parent,
            None => break,
        }
    }
    decl
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
/// `decl.kind` stays `Function` through the symbol and ordinal machinery, so
/// `Function`/`Method` share the SCIP method-descriptor slot ([`descriptor_for`])
/// and the `(kind, name)` ordinal grouping — every pre-existing symbol ID is
/// byte-identical, only the emitted `nodes.kind` discriminant changes
/// ([NFR-RA-06]).
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
}

/// Assign each declaration its ordinal among same-`(kind, name)` siblings, in
/// the canonical sort order `(start_byte, kind, name)` ([ADR-07]).
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
        let mut seen: HashMap<(i32, String), u32> = HashMap::new();
        for idx in siblings {
            let key = (decls[idx].kind.as_i32(), decls[idx].name.clone());
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

#[cfg(all(test, feature = "lang-rust"))]
mod tests;
