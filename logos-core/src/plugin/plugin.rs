//! The [`LanguagePlugin`] trait and its compiled-grammar implementation
//! ([plugin-registry], [ADR-09]).
//!
//! [`LanguagePlugin`] is the contract the [extraction-engine] (S-007) consumes:
//! a loaded grammar exposes its built `Language`, its declarative semantics, the
//! capabilities it supports, and the compiled `Query` backing each capability.
//! [`CompiledPlugin`] is the one concrete implementation in v1 — a grammar
//! compiled in via a cargo feature whose queries have already been resolved
//! (override-or-embedded), and compile on the language's first use (CR-197).
//!
//! [plugin-registry]: ../../../docs/specs/architecture/components/plugin-registry.md
//! [extraction-engine]: ../../../docs/specs/architecture/components/extraction-engine.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md

use std::sync::Arc;

use tree_sitter::{Language, Query};

use super::error::PluginError;
use super::manifest::{
    BareSpecifiers, CallTargets, ConfigDescriptor, ExportConvention, ImplicitReceiver, ImportSpecifier,
    ModuleModelKind, PackageModules, PluginManifest, Preprocessor, PropertiesDescriptor, Reach, TestConvention,
};
use super::queries::{CompiledQueries, LanguageQueries};

/// The declarative, on-disk-tunable semantics of a language ([NFR-MA-05]).
///
/// [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Semantics {
    /// The **member-path** separator joining symbol segments (`::`, `.`, `/`);
    /// see [`PluginManifest::module_separator`].
    pub module_separator: String,
    /// The grammar this language's import specifiers are written in (S-439;
    /// see [`PluginManifest::import_specifier`]) — consumed by reference
    /// extraction to choose between the name and the path canonicaliser.
    pub import_specifier: ImportSpecifier,
    /// Extensions a relative path specifier may spell for the imported file
    /// itself (S-439; see [`PluginManifest::specifier_extensions`]).
    pub specifier_extensions: Vec<String>,
    /// What a bare path specifier binds through (S-531; see
    /// [`PluginManifest::bare_specifiers`]) — consumed by the resolution pass
    /// through [`crate::plugin::LanguageRegistry::tsconfig_extensions`] and
    /// [`crate::plugin::LanguageRegistry::load_path_extensions`], and by
    /// extraction, which reads a load-path language's bare specifier apart
    /// (S-566, `extract::refs::load_path_specifier_segments`).
    pub bare_specifiers: BareSpecifiers,
    /// What an unqualified call inside a class body means (S-514; see
    /// [`PluginManifest::implicit_receiver`]) — consumed by reference
    /// extraction's receiver-shape pass.
    pub implicit_receiver: ImplicitReceiver,
    /// Whether a bare call binds free callables only (S-590, [FR-RS-07]; see
    /// [`PluginManifest::bare_calls_free_only`]): the descriptor declares
    /// `implicit_receiver = "none"` explicitly — consumed by the binder's
    /// bare-path rung through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    pub bare_calls_free_only: bool,
    /// Whether this language overloads callables by name (S-592, [FR-RS-43];
    /// see [`PluginManifest::overloaded_calls`]) — consumed by the binder's
    /// bare-call rung through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub overloaded_calls: bool,
    /// Whether a call to a type's method binds through the one associated-item
    /// lookup over recorded `impl` blocks (S-607, [FR-RS-47]; see
    /// [`PluginManifest::impl_block_lookup`]) — consumed by the binder through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    pub impl_block_lookup: bool,
    /// Whether a bare call never names a local value (S-594, [FR-RS-44]; see
    /// [`PluginManifest::separate_call_namespace`]) — read by extraction's
    /// local-binding rule (`extract::local_shadow`).
    ///
    /// [FR-RS-44]: ../../../docs/specs/requirements/FR-RS-44.md
    pub separate_call_namespace: bool,
    /// Whether a type implements an interface by its method set (S-529,
    /// [FR-RS-18]; see [`PluginManifest::method_set_implements`]) — consumed by
    /// the resolution pass's method-set reconcile ([`crate::resolve`]).
    ///
    /// [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
    pub method_set_implements: bool,
    /// The extensions of this plugin whose language enforces no arity (S-592,
    /// [FR-RS-43]; see [`PluginManifest::arity_unchecked_extensions`]) —
    /// consumed by the binder's arity filter through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub arity_unchecked_extensions: Vec<String>,
    /// Whether an unqualified in-class call no member admits goes on to the
    /// free functions and imports in scope (S-592, [FR-RS-43]; see
    /// [`PluginManifest::implicit_call_falls_through`]) — consumed by the
    /// binder's `self` arm through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub implicit_call_falls_through: bool,
    /// The members a root the graph never holds gives every class (S-592; see
    /// [`PluginManifest::implicit_root_members`]): names that never fall
    /// through.
    pub implicit_root_members: Vec<String>,
    /// The methods each peeled receiver wrapper provides itself (S-588,
    /// [FR-RS-42]; see [`PluginManifest::wrapper_methods`]) — consumed by the
    /// binder's proven-receiver arm through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub wrapper_methods: std::collections::BTreeMap<String, Vec<String>>,
    /// What a call may bind besides a callable (S-521, [FR-RS-16]; see
    /// [`PluginManifest::call_targets`]) — consumed by the binder's call
    /// admission through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    pub call_targets: CallTargets,
    /// Whether a supertype's edge kind follows the type it binds (S-522,
    /// [FR-RS-15]; see [`PluginManifest::supertype_kind_follows_target`]) —
    /// consumed by the binder's type-relation admission through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    pub supertype_kind_follows_target: bool,
    /// Whether this language's classes inherit the bodies of the interface
    /// members they implement (S-609, [FR-RS-48]; see
    /// [`PluginManifest::inherits_interface_bodies`]) — consumed by the
    /// binder's supertype walk through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    pub inherits_interface_bodies: bool,
    /// Whether this language's types are structural and every type relation
    /// it captures relates its declaration (S-535, [FR-EX-17]; see
    /// [`PluginManifest::structural_types`]) — consumed by the binder's
    /// type-relation admission and proven-receiver rung through
    /// [`crate::resolve::package_key`], and by the receiver-typing pass
    /// (S-537, `extract::receiver`).
    ///
    /// [FR-EX-17]: ../../../docs/specs/requirements/FR-EX-17.md
    pub structural_types: bool,
    /// Whether this language's modules are CommonJS as well as ECMAScript
    /// (S-540, [FR-RS-22]; see [`PluginManifest::commonjs`]) — consumed by
    /// reference extraction's import reader and `module.exports` rows.
    ///
    /// [FR-RS-22]: ../../../docs/specs/requirements/FR-RS-22.md
    pub commonjs: bool,
    /// Whether, and under which source roots, this language's module path is
    /// package-shaped (CR-149; see [`PluginManifest::package_modules`]) —
    /// consumed by the binder's module key through
    /// [`crate::resolve::package_key`].
    pub package_modules: Option<PackageModules>,
    /// Which module model keys this language's files (S-518, [FR-RS-13]; see
    /// [`PluginManifest::module_model_kind`]), the omitted table already
    /// resolved — consumed by [`crate::resolve::package_key`] and by extraction,
    /// which reads a declared namespace only under
    /// [`ModuleModelKind::Namespace`].
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    pub module_model: ModuleModelKind,
    /// The `path` model's package-file stems (S-519, [FR-RS-14]; see
    /// [`ModuleModel::package_stems`](super::ModuleModel::package_stems)) — a
    /// file with one of them names its directory. Read by
    /// [`crate::resolve::package_key`] and by extraction's file-module name.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    pub package_stems: Vec<String>,
    /// The `path` model's candidate import roots (S-519; see
    /// [`ModuleModel::import_roots`](super::ModuleModel::import_roots)), `None`
    /// when the language keeps the default model's `src/` crate rule.
    pub import_roots: Option<Vec<String>>,
    /// Whether a namespace of this language sees the types of its enclosing
    /// namespaces (S-595, [FR-RS-45]; see
    /// [`ModuleModel::enclosing_namespaces`](super::ModuleModel::enclosing_namespaces))
    /// — consumed by the binder's package rungs through
    /// [`crate::resolve::package_key`], and by reference extraction, which
    /// records the namespace block a wildcard is written in (S-603).
    ///
    /// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
    pub enclosing_namespaces: bool,
    /// Whether a type of this language is told apart from a same-named one by
    /// its generic arity (S-602, [CR-199]; see
    /// [`ModuleModel::type_arity`](super::ModuleModel::type_arity)) —
    /// consumed by extraction, which records each type declaration's and each
    /// written type's arity, and by the binder's package rungs through
    /// [`crate::resolve::package_key`].
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    pub type_arity: bool,
    /// Whether this language names its classes and modules by lexical
    /// constants (S-565, [FR-RS-33]; see
    /// [`ModuleModel::lexical_constants`](super::ModuleModel::lexical_constants))
    /// — consumed by the binder's constant index through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
    pub lexical_constants: bool,
    /// Whether the files of one directory that declare one `package` clause
    /// share one package scope (S-528, [FR-RS-17]; see
    /// [`ModuleModel::directory_packages`](super::ModuleModel::directory_packages))
    /// — consumed by extraction, which records the clause, and by the binder's
    /// same-package rung through [`crate::resolve::package_key`].
    ///
    /// [FR-RS-17]: ../../../docs/specs/requirements/FR-RS-17.md
    pub directory_packages: bool,
    /// Whether a bare call no included declaration supplies binds the one
    /// non-`static` definition of its name among the language's files (S-584,
    /// [FR-RS-40]; see [`ModuleModel::link_unit`](super::ModuleModel::link_unit))
    /// — consumed by the binder's link-unit rung through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-40]: ../../../docs/specs/requirements/FR-RS-40.md
    pub link_unit: bool,
    /// Whether a type and the same-named singleton beside it are one name
    /// (S-573, [FR-RS-36]; see
    /// [`ModuleModel::companion_objects`](super::ModuleModel::companion_objects))
    /// — consumed by the binder's path walk and wildcard lookup through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub companion_objects: bool,
    /// Whether a file's top-level functions are members of its declared
    /// namespace (S-573, [FR-RS-36]; see
    /// [`ModuleModel::top_level_callables`](super::ModuleModel::top_level_callables))
    /// — consumed by the binder's fully-qualified index through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub top_level_callables: bool,
    /// Whether a wildcard import outranks the namespace's other files (S-573,
    /// [FR-RS-36]; see
    /// [`ModuleModel::wildcards_outrank_package`](super::ModuleModel::wildcards_outrank_package))
    /// — consumed by the binder's package rungs through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub wildcards_outrank_package: bool,
    /// Whether a top-level function with a receiver type is callable on a
    /// value (S-560, [FR-RS-31]; see
    /// [`ModuleModel::extension_functions`](super::ModuleModel::extension_functions))
    /// — consumed by the binder's receiver-call arm through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub extension_functions: bool,
    /// The suffix of the class another language reaches a file's top-level
    /// functions through (S-560, [FR-RS-31]; see
    /// [`ModuleModel::file_facade`](super::ModuleModel::file_facade)) —
    /// consumed by the binder's fully-qualified index through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub file_facade: Option<String>,
    /// The name a type's anonymous companion object takes (I4-INT,
    /// [FR-RS-31]; see
    /// [`ModuleModel::companion_name`](super::ModuleModel::companion_name)) —
    /// consumed by the binder's call rungs through
    /// [`crate::resolve::package_key`], and by reference extraction's receiver
    /// pass, which records a typed receiver's call as a proven row (S-563).
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub companion_name: Option<String>,
    /// Whether an unqualified call its namespace does not supply falls back to
    /// the global function (S-551, [FR-RS-26]; see
    /// [`ModuleModel::global_function_fallback`](super::ModuleModel::global_function_fallback))
    /// — consumed by the binder's package rungs through
    /// [`crate::resolve::package_key`].
    ///
    /// [FR-RS-26]: ../../../docs/specs/requirements/FR-RS-26.md
    pub global_function_fallback: bool,
    /// The interop family this language binds within, resolved (S-519; see
    /// [`PluginManifest::module_family`]): the declared one, else the plugin's
    /// own name.
    pub family: String,
    /// Keywords that increment cyclomatic complexity for this language.
    /// Carried declaratively now; consumed by the complexity metric (S-011+).
    pub complexity_keywords: Vec<String>,
    /// Tree-sitter node kinds that introduce one block-structure nesting level
    /// (CR-005, [FR-EX-07]); consumed by `extract::nesting` to compute each
    /// function's maximum nesting depth.
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    pub nesting_block_kinds: Vec<String>,
    /// Tree-sitter node kinds that mark a callable as implemented (S-500,
    /// [FR-EX-11]; see [`PluginManifest::body_node_kinds`]); consumed by
    /// `extract::shape` to record each callable's has-body fact. Empty = every
    /// callable is bodied.
    ///
    /// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
    pub body_node_kinds: Vec<String>,
    /// The tree-sitter ABI version declared by the descriptor and asserted
    /// against the compiled grammar at load.
    pub abi_version: usize,
    /// Canonical reference-path prefixes that gate framework candidacy
    /// (S-015, [FR-FW-04]; see [`PluginManifest::framework_detectors`]).
    ///
    /// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
    pub framework_detectors: Vec<String>,
    /// Whether [`Self::framework_detectors`] match only import references
    /// (S-541; see [`FrameworkCandidacy::detectors_import_only`]).
    ///
    /// [`FrameworkCandidacy::detectors_import_only`]: crate::plugin::FrameworkCandidacy::detectors_import_only
    pub framework_detectors_import_only: bool,
    /// Parameter names that make a file a framework candidate by themselves
    /// (S-541; see [`FrameworkCandidacy::receiver_parameters`]).
    ///
    /// [`FrameworkCandidacy::receiver_parameters`]: crate::plugin::FrameworkCandidacy::receiver_parameters
    pub framework_receiver_parameters: Vec<String>,
    /// Canonical reference-path prefixes that gate **HTTP client-call**
    /// candidacy (S-341, [FR-WS-08]; see
    /// [`PluginManifest::http_client_detectors`]) — the consumer-side twin of
    /// [`Self::framework_detectors`].
    ///
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    pub http_client_detectors: Vec<String>,
    /// Captured `@fw.route.method` text → HTTP method for the declarative
    /// framework contract (S-015; see [`PluginManifest::framework_methods`]).
    pub framework_methods: std::collections::BTreeMap<String, String>,
    /// Captured `@invoke.http.method` text → HTTP method for the **client-call**
    /// arm (S-346; see [`PluginManifest::invocation_methods`]).
    pub invocation_methods: std::collections::BTreeMap<String, String>,
    /// How this language marks a declaration exported (S-015, [FR-AN-01]).
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub export_convention: ExportConvention,
    /// How this language marks a function as a test ([CR-001], [FR-EX-06],
    /// [ADR-18]); consumed by `extract::testmarker`.
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
    /// [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md
    pub test_convention: TestConvention,
    /// The source pre-pass run before parsing (S-555, [FR-EX-23]; see
    /// [`Preprocessor`]); consumed by `extract::preprocess`.
    ///
    /// [FR-EX-23]: ../../../docs/specs/requirements/FR-EX-23.md
    pub preprocessor: Preprocessor,
    /// Whether this language declares the optional reachability capability
    /// (S-159, [CR-043], [ADR-39]); gates Pass-3 dead-code reachability in the
    /// annotation engine ([FR-AN-01]). `false` (the default) makes the
    /// language's callables render `is_dead = NULL` rather than a fabricated
    /// verdict ([NFR-CC-04]).
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub reachability: bool,
    /// The functions this language's runtime calls by name (S-530, [FR-AN-01];
    /// see [`PluginManifest::entry_points`]) — dead-code live roots in this
    /// language's files only.
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub entry_points: Vec<String>,
    /// Whether this language declares the optional member-access capability
    /// (S-523, [CR-171]; see [`PluginManifest::member_access`]); gates Cohesion
    /// in the metrics engine ([FR-QM-11]) through
    /// [`LanguageRegistry::member_access_extensions`](crate::plugin::LanguageRegistry::member_access_extensions).
    ///
    /// [CR-171]: ../../../docs/requests/CR-171-the-quality-signal-and-test-classification-stay-honest-on-a-thin-graph.md
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    pub member_access: bool,
    /// The extensions of this plugin its `member_access` declaration does not
    /// cover (S-523; see [`PluginManifest::member_access_excluded_extensions`]).
    pub member_access_excluded_extensions: Vec<String>,
    /// Whether this is a documentation grammar (S-033, [CR-003], [ADR-19]):
    /// when `true`, extraction routes the file structurally through
    /// `extract::doc` into `DocFile`/`DocSection` nodes instead of the code
    /// `symbols`/`references` tagging path.
    ///
    /// [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
    /// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
    pub documentation: bool,
    /// Whether this is a config/artifact grammar (S-062, [CR-010], [ADR-25]):
    /// when `true`, extraction routes the file structurally through
    /// `extract::config` into `ConfigFile`/`ConfigSection` (+ typed-anchor) nodes
    /// instead of the code path. The third plugin class beside code and docs.
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
    pub artifact: bool,
    /// Basename claims for extensionless formats (S-062, [FR-CG-01]): exact,
    /// plus `Name.*` prefix for an artifact plugin. A code plugin claims exact
    /// names only (Ruby's `Gemfile`, S-564); a doc plugin claims none.
    ///
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    pub filenames: Vec<String>,
    /// The structural config-extraction descriptor (S-062, [FR-CG-02]) driving the
    /// generic depth-bounded `ConfigSection` walk; `None` for code/doc plugins and
    /// for typed-anchor-only artifact formats.
    ///
    /// [FR-CG-02]: ../../../docs/specs/requirements/FR-CG-02.md
    pub config: Option<ConfigDescriptor>,
    /// The configuration-**binding** descriptor (S-381, [FR-WS-19]) driving the
    /// generic properties-class interpreter in
    /// [`crate::extract::config::binding`]; `None` for every language that binds
    /// no configuration. See [`PropertiesDescriptor`].
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    pub properties: Option<PropertiesDescriptor>,
}

/// A loaded language grammar — the unit the registry indexes by extension.
///
/// Implemented by [`CompiledPlugin`]; a trait so the extraction engine depends
/// on the capability surface, not the concrete loading strategy.
pub trait LanguagePlugin {
    /// The grammar's lookup/display name (the descriptor `name`).
    fn name(&self) -> &str;
    /// File extensions this grammar claims (without the leading dot).
    fn extensions(&self) -> &[String];
    /// The built tree-sitter `Language` for parsing.
    fn language(&self) -> &Language;
    /// The declarative semantics (separator, complexity keywords, ABI).
    fn semantics(&self) -> &Semantics;
    /// Extraction capabilities this grammar supports (e.g. `["symbols"]`).
    fn capabilities(&self) -> &[String];
    /// The compiled query backing `capability`, if any. The first call on a
    /// language may compile all of that language's queries (CR-197).
    fn query(&self, capability: &str) -> Option<&Query>;
    /// Capabilities whose active query is an on-disk override ([FR-PL-04]).
    /// Defaults to none.
    ///
    /// [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
    fn overridden_capabilities(&self) -> &[String] {
        &[]
    }

    /// Whether this language declares the optional **reachability capability**
    /// (S-159, [CR-043], [ADR-39]) — the gate on the [annotation-engine] Pass-3
    /// dead-code detector ([FR-AN-01]). Defaults to `false`: a plugin that does
    /// not declare it renders `is_dead = NULL` ("not computed", [NFR-CC-04]) for
    /// its callables rather than a fabricated verdict, and still indexes
    /// normally ([NFR-MA-01]). The same optional-capability pattern as the
    /// [CR-001] test-marker query.
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    fn supports_reachability(&self) -> bool {
        false
    }

    /// Whether this grammar is a documentation plugin (S-033, [CR-003]) — the
    /// signal the extraction engine routes on to parse a file into
    /// `DocFile`/`DocSection` nodes (`extract::doc`) rather than code symbols.
    /// Defaults to `false` so every existing code plugin is unaffected.
    ///
    /// [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
    fn is_documentation(&self) -> bool {
        false
    }

    /// Whether this grammar is a config/artifact plugin (S-062, [CR-010]) — the
    /// signal the extraction engine routes on to parse a file structurally into
    /// `ConfigFile`/`ConfigSection` (+ typed-anchor) nodes (`extract::config`)
    /// rather than code symbols. Defaults to `false`.
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    fn is_artifact(&self) -> bool {
        false
    }

    /// Basename claims for this plugin (S-062, [FR-CG-01]); exact at lookup, plus
    /// the `Name.*` prefix for an artifact plugin's claim (`Dockerfile.dev`).
    /// A code plugin may claim exact names too (Ruby's `Gemfile`, S-564); a
    /// doc plugin claims none. Defaults to none.
    ///
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    fn filenames(&self) -> &[String] {
        &[]
    }

    /// The structural config-extraction descriptor driving the generic
    /// `ConfigSection` walk (S-062, [FR-CG-02]); `None` for code/doc plugins and
    /// typed-anchor-only artifact formats. Defaults to none.
    ///
    /// [FR-CG-02]: ../../../docs/specs/requirements/FR-CG-02.md
    fn config_extraction(&self) -> Option<&ConfigDescriptor> {
        None
    }

    /// The cross-file reach this code language declares in its `[reach]` table
    /// ([FR-PL-09], [CR-180]); `None` for documentation and artifact plugins,
    /// which bind no code reference. Defaults to none.
    ///
    /// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
    /// [CR-180]: ../../../docs/requests/CR-180-scala-is-declared-as-limited-support-and-every-language-declares-its-reach.md
    fn reach(&self) -> Option<&Reach> {
        None
    }

    /// The one-sentence disclosure this plugin's descriptor carries (`note`,
    /// S-572, [FR-PL-07]), surfaced beside the language by `logos languages`.
    /// Defaults to none.
    ///
    /// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
    fn note(&self) -> Option<&str> {
        None
    }
}

/// A grammar compiled in via a cargo feature, fully loaded and ready to parse.
///
/// ABI assertion, query resolution and every override's compile happen *before*
/// a `CompiledPlugin` exists (in [`crate::plugin::LanguageRegistry`]), so
/// holding one is proof the grammar is safe to use. Its embedded queries
/// compile on the first [`query`](LanguagePlugin::query) call (CR-197); every
/// one of them is compiled by a test, so that compile cannot fail in a shipped
/// build.
#[derive(Debug)]
pub struct CompiledPlugin {
    name: String,
    extensions: Vec<String>,
    language: Language,
    semantics: Semantics,
    capabilities: Vec<String>,
    /// Capability → compiled query, compiled at load or on first use.
    queries: LanguageQueries,
    /// Capabilities whose query came from an on-disk override (observability).
    overridden: Vec<String>,
    /// The declared `[reach]` ([FR-PL-09]); `None` for a non-code plugin.
    ///
    /// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
    reach: Option<Reach>,
    /// The descriptor's `note` (S-572); `None` when it declares none.
    note: Option<String>,
    /// The extensions this plugin shares with another that claims them too
    /// (S-582, [`PluginManifest::shared_extensions`]).
    shared_extensions: Vec<String>,
}

impl CompiledPlugin {
    /// Assemble a plugin from its parsed descriptor, built language, and the
    /// queries the registry resolved for it.
    ///
    /// `overridden` lists the capabilities whose query was sourced from an
    /// on-disk override rather than the embedded default.
    pub(crate) fn new(
        manifest: PluginManifest,
        language: Language,
        queries: LanguageQueries,
        overridden: Vec<String>,
    ) -> Self {
        let module_model = manifest.module_model_kind();
        let family = manifest.module_family();
        let call_targets = manifest.call_targets();
        let candidacy = manifest.framework_candidacy.clone().unwrap_or_default();
        let implicit_receiver = manifest.implicit_receiver();
        let bare_calls_free_only = manifest.bare_calls_free_only();
        let enclosing_namespaces = manifest.enclosing_namespaces();
        let type_arity = manifest.type_arity();
        let lexical_constants = manifest.lexical_constants();
        let directory_packages = manifest.directory_packages();
        let link_unit = manifest.link_unit();
        let companion_objects = manifest.companion_objects();
        let top_level_callables = manifest.top_level_callables();
        let wildcards_outrank_package = manifest.wildcards_outrank_package();
        let extension_functions = manifest.extension_functions();
        let file_facade = manifest.file_facade().map(str::to_string);
        let companion_name = manifest.companion_name().map(str::to_string);
        let global_function_fallback = manifest.global_function_fallback();
        let (package_stems, import_roots) = manifest
            .module_model
            .as_ref()
            .map(|m| (m.package_stems.clone(), m.import_roots.clone()))
            .unwrap_or_default();
        let semantics = Semantics {
            module_separator: manifest.module_separator,
            import_specifier: manifest.import_specifier,
            specifier_extensions: manifest.specifier_extensions,
            bare_specifiers: manifest.bare_specifiers,
            implicit_receiver,
            bare_calls_free_only,
            overloaded_calls: manifest.overloaded_calls,
            impl_block_lookup: manifest.impl_block_lookup,
            separate_call_namespace: manifest.separate_call_namespace,
            method_set_implements: manifest.method_set_implements,
            arity_unchecked_extensions: manifest.arity_unchecked_extensions,
            implicit_call_falls_through: manifest.implicit_call_falls_through,
            implicit_root_members: manifest.implicit_root_members,
            wrapper_methods: manifest.wrapper_methods,
            call_targets,
            supertype_kind_follows_target: manifest.supertype_kind_follows_target,
            inherits_interface_bodies: manifest.inherits_interface_bodies,
            structural_types: manifest.structural_types,
            commonjs: manifest.commonjs,
            package_modules: manifest.package_modules,
            module_model,
            package_stems,
            import_roots,
            enclosing_namespaces,
            type_arity,
            lexical_constants,
            directory_packages,
            link_unit,
            companion_objects,
            top_level_callables,
            wildcards_outrank_package,
            extension_functions,
            file_facade,
            companion_name,
            global_function_fallback,
            family,
            complexity_keywords: manifest.complexity_keywords,
            nesting_block_kinds: manifest.nesting_block_kinds,
            body_node_kinds: manifest.body_node_kinds,
            abi_version: manifest.abi_version,
            framework_detectors: manifest.framework_detectors,
            framework_detectors_import_only: candidacy.detectors_import_only,
            framework_receiver_parameters: candidacy.receiver_parameters,
            http_client_detectors: manifest.http_client_detectors,
            framework_methods: manifest.framework_methods,
            invocation_methods: manifest.invocation_methods,
            export_convention: manifest.export_convention,
            test_convention: manifest.test_convention,
            preprocessor: manifest.preprocessor,
            reachability: manifest.reachability,
            entry_points: manifest.entry_points,
            member_access: manifest.member_access,
            member_access_excluded_extensions: manifest.member_access_excluded_extensions,
            documentation: manifest.documentation,
            artifact: manifest.artifact,
            filenames: manifest.filenames,
            config: manifest.config,
            properties: manifest.properties,
        };
        Self {
            name: manifest.name,
            extensions: manifest.extensions,
            language,
            semantics,
            capabilities: manifest.capabilities,
            queries,
            overridden,
            reach: manifest.reach,
            note: manifest.note,
            shared_extensions: manifest.shared_extensions,
        }
    }
}

impl CompiledPlugin {
    /// Every capability's compiled query, compiling the language now if this
    /// is its first use.
    ///
    /// # Errors
    /// The [`PluginError`] that refused the language's compile, naming its file.
    pub(crate) fn compiled_queries(&self) -> Result<&CompiledQueries, &PluginError> {
        self.queries.get(&self.language)
    }

    /// The extensions this plugin shares with another plugin claiming them
    /// (S-582) — the registry routes each by the repository's sources.
    pub(crate) fn shared_extensions(&self) -> &[String] {
        &self.shared_extensions
    }

    /// Whether this language's queries are compiled yet — at load for an
    /// override, else by a first use.
    #[cfg(test)]
    pub(crate) fn queries_compiled(&self) -> bool {
        self.queries.is_compiled()
    }
}

impl LanguagePlugin for CompiledPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn extensions(&self) -> &[String] {
        &self.extensions
    }

    fn language(&self) -> &Language {
        &self.language
    }

    fn semantics(&self) -> &Semantics {
        &self.semantics
    }

    fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    fn query(&self, capability: &str) -> Option<&Query> {
        self.compiled_queries()
            .ok()?
            .get(capability)
            .map(Arc::as_ref)
    }

    fn overridden_capabilities(&self) -> &[String] {
        &self.overridden
    }

    fn supports_reachability(&self) -> bool {
        self.semantics.reachability
    }

    fn is_documentation(&self) -> bool {
        self.semantics.documentation
    }

    fn is_artifact(&self) -> bool {
        self.semantics.artifact
    }

    fn filenames(&self) -> &[String] {
        &self.semantics.filenames
    }

    fn config_extraction(&self) -> Option<&ConfigDescriptor> {
        self.semantics.config.as_ref()
    }

    fn reach(&self) -> Option<&Reach> {
        self.reach.as_ref()
    }

    fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}
