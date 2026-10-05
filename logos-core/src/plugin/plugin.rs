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
    CallTargets, ConfigDescriptor, ExportConvention, ImplicitReceiver, ImportSpecifier,
    ModuleModelKind, PackageModules, PluginManifest, PropertiesDescriptor, Reach, TestConvention,
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
    /// What an unqualified call inside a class body means (S-514; see
    /// [`PluginManifest::implicit_receiver`]) — consumed by reference
    /// extraction's receiver-shape pass.
    pub implicit_receiver: ImplicitReceiver,
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
    /// Basename claims for extensionless artifact formats (S-062, [FR-CG-01]):
    /// exact + `Name.*` prefix. Empty for code/doc plugins.
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

    /// Basename claims for this plugin (S-062, [FR-CG-01]); exact + `Name.*`
    /// prefix at lookup. Empty for code/doc plugins. Defaults to none.
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
        let (package_stems, import_roots) = manifest
            .module_model
            .as_ref()
            .map(|m| (m.package_stems.clone(), m.import_roots.clone()))
            .unwrap_or_default();
        let semantics = Semantics {
            module_separator: manifest.module_separator,
            import_specifier: manifest.import_specifier,
            specifier_extensions: manifest.specifier_extensions,
            implicit_receiver: manifest.implicit_receiver,
            call_targets,
            supertype_kind_follows_target: manifest.supertype_kind_follows_target,
            package_modules: manifest.package_modules,
            module_model,
            package_stems,
            import_roots,
            family,
            complexity_keywords: manifest.complexity_keywords,
            nesting_block_kinds: manifest.nesting_block_kinds,
            body_node_kinds: manifest.body_node_kinds,
            abi_version: manifest.abi_version,
            framework_detectors: manifest.framework_detectors,
            http_client_detectors: manifest.http_client_detectors,
            framework_methods: manifest.framework_methods,
            invocation_methods: manifest.invocation_methods,
            export_convention: manifest.export_convention,
            test_convention: manifest.test_convention,
            reachability: manifest.reachability,
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
}
