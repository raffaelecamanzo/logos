//! The [`LanguageRegistry`] — the plugin micro-kernel ([plugin-registry], [ADR-09]).
//!
//! At startup the registry walks the compiled-in grammar table
//! ([`super::grammars::compiled`]) and, for each grammar:
//!
//! 1. parses its embedded `plugin.toml` ([FR-PL-02]);
//! 2. builds the `Language` from its `LanguageFn` and asserts ABI — a mismatch
//!    is skipped-and-warned, never fatal ([FR-PL-03], [NFR-PC-03]);
//! 3. resolves each capability's query (on-disk override shadows embedded). A
//!    language with an override compiles now, failing fast and naming the file
//!    on error ([FR-PL-02], [FR-PL-04]); every other language compiles its
//!    queries on its first use instead (CR-197, [`queries::LanguageQueries`]),
//!    so a cold start pays only for the languages it uses. Either way each
//!    distinct source compiles once per process, so a later load that resolves
//!    the same text shares the compiled query (HF-3, [`queries::compile_shared`]);
//! 4. indexes the loaded grammar by extension for `for_extension` lookups.
//!
//! A broken *embedded* query no longer fails the load: shipped queries are
//! fixed per build, so the test that compiles every one of them
//! ([`LanguageRegistry::compile_all_queries`]) catches it before a user can.
//!
//! Built once per engine ([ADR-04]) — a workspace process builds one per member,
//! over queries compiled once per process — and thereafter every lookup is a
//! hash probe.
//!
//! [plugin-registry]: ../../../docs/specs/architecture/components/plugin-registry.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-03]: ../../../docs/specs/requirements/FR-PL-03.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [NFR-PC-03]: ../../../docs/specs/requirements/NFR-PC-03.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use tree_sitter::Language;

use super::abi::{assert_abi, AbiRange};
use super::error::{PluginError, SkippedGrammar};
use super::grammars::{self, GrammarEntry};
use super::manifest::{CallTargets, PluginManifest};
use super::plugin::{CompiledPlugin, LanguagePlugin, Semantics};
use super::queries::{self, LanguageQueries};

/// One language's path-model declaration (S-519, [FR-RS-14]), as
/// [`LanguageRegistry::path_models`] hands it to the module key: the package-file
/// stems, the candidate import roots, and the names the import-root override and
/// the family partition read.
///
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathModelDecl {
    /// The plugin's name — the key of `.logos/config.toml`'s
    /// `[resolution.import_roots]` override.
    pub language: String,
    /// The interop family the plugin binds within.
    pub family: String,
    /// The package-file stems (`__init__`; `mod`, `lib`, `main`).
    pub package_stems: Vec<String>,
    /// The candidate import roots; `None` keeps the default `src/` crate rule.
    pub import_roots: Option<Vec<String>>,
}

impl PathModelDecl {
    /// The path-model declaration `plugin` makes — `None` unless it declares
    /// the `path` model with package stems or import roots. The one reading of
    /// a plugin's path model, shared by [`LanguageRegistry::path_models`] and
    /// [`PackageLayout::from_plugin`](crate::resolve::package_key::PackageLayout::from_plugin),
    /// so a single-plugin layout is the registry's exactly.
    pub fn of(plugin: &dyn LanguagePlugin) -> Option<Self> {
        let s = plugin.semantics();
        let declares = !s.package_stems.is_empty() || s.import_roots.is_some();
        (s.module_model == super::ModuleModelKind::Path && declares).then(|| Self {
            language: plugin.name().to_string(),
            family: s.family.clone(),
            package_stems: s.package_stems.clone(),
            import_roots: s.import_roots.clone(),
        })
    }
}

/// The in-memory registry of loaded language grammars.
#[derive(Debug)]
pub struct LanguageRegistry {
    /// Loaded plugins in declaration order (the order `languages` lists them).
    plugins: Vec<CompiledPlugin>,
    /// Normalised extension → index into `plugins`.
    by_extension: HashMap<String, usize>,
    /// Basename claims in declaration order: `(claim, plugin index)` (S-062,
    /// [CR-010], [FR-CG-01]). Kept as an ordered list rather than a map so both
    /// the exact and the `Name.*` prefix lookup in [`for_path`](LanguageRegistry::for_path)
    /// are deterministic (first declaration wins), and so the prefix scan is a
    /// simple ordered walk — the claim set is tiny (≤ a few per artifact plugin).
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    filename_claims: Vec<(String, usize)>,
    /// Grammars skipped at load (ABI mismatch), recorded for `languages` and
    /// diagnostics ([FR-PL-03]).
    skipped: Vec<SkippedGrammar>,
}

impl LanguageRegistry {
    /// Load every compiled-in grammar, resolving on-disk overrides under
    /// `project_root` and asserting ABI against the linked tree-sitter runtime.
    ///
    /// ABI mismatches are skipped with a warning to stderr; a malformed
    /// descriptor, an override query that fails to compile, or an unreadable
    /// override is a hard error naming the file ([FR-PL-02]). Embedded queries
    /// are not compiled here — each language compiles on its first use
    /// (CR-197).
    ///
    /// # Errors
    /// Returns [`PluginError`] on a descriptor parse error, an override query
    /// compile error, or an unreadable override file.
    pub fn load(project_root: impl AsRef<Path>) -> Result<Self, PluginError> {
        Self::load_from(
            &grammars::compiled(),
            AbiRange::runtime(),
            Some(project_root.as_ref()),
            // Warnings route through the single tracing seam — never a direct
            // print (FR-OB-01, NFR-OO-01); stderr rendering is the fmt layer's.
            &mut |w| tracing::warn!("{w}"),
        )
    }

    /// The seam under [`load`](Self::load): explicit grammar table, ABI range,
    /// optional override root, and a warning sink.
    ///
    /// Tests drive this directly to force an ABI mismatch (narrow range or a
    /// disagreeing descriptor) and to capture warnings, without a second build
    /// artifact ([UAT-PL-02]).
    ///
    /// # Errors
    /// As [`load`](Self::load).
    pub(crate) fn load_from(
        entries: &[GrammarEntry],
        abi_range: AbiRange,
        project_root: Option<&Path>,
        warn: &mut dyn FnMut(&str),
    ) -> Result<Self, PluginError> {
        let mut plugins: Vec<CompiledPlugin> = Vec::new();
        let mut by_extension: HashMap<String, usize> = HashMap::new();
        let mut filename_claims: Vec<(String, usize)> = Vec::new();
        let mut skipped: Vec<SkippedGrammar> = Vec::new();

        for entry in entries {
            let manifest = PluginManifest::parse(entry.manifest_label, entry.manifest_toml)?;

            // Build the Language from its LanguageFn and assert ABI before use.
            let language: Language = entry.language.into();
            let compiled_abi = language.abi_version();
            if let Err(reason) = assert_abi(manifest.abi_version, compiled_abi, &abi_range) {
                let skip = SkippedGrammar {
                    name: manifest.name.clone(),
                    reason,
                };
                warn(&skip.to_string());
                skipped.push(skip);
                continue; // skip only this grammar — the run is not aborted
            }

            let override_dir = project_root.map(|root| override_dir_for(root, &manifest.name));

            let (queries, overridden) =
                compile_capabilities(entry, &manifest, &language, override_dir.as_deref())?;

            let plugin = CompiledPlugin::new(manifest, language, queries, overridden);

            let idx = plugins.len();
            for ext in plugin.extensions() {
                // Last declaration wins on an extension collision; warn so the
                // shadowing is visible rather than silent.
                if let Some(prev) = by_extension.insert(normalize_ext(ext), idx) {
                    warn(&format!(
                        "extension '{ext}' claimed by '{}' shadows '{}'",
                        plugin.name(),
                        plugins[prev].name()
                    ));
                }
            }
            // Basename claims (S-062, CR-010, FR-CG-01): recorded in declaration
            // order so `for_path`'s exact + `Name.*` prefix lookup is deterministic
            // (first declaration wins). A collision is surfaced as a warning, like
            // the extension case.
            for fname in plugin.filenames() {
                if let Some((_, prev)) = filename_claims.iter().find(|(c, _)| c == fname) {
                    warn(&format!(
                        "filename '{fname}' claimed by '{}' also claimed by '{}'",
                        plugin.name(),
                        plugins[*prev].name()
                    ));
                }
                filename_claims.push((fname.clone(), idx));
            }
            plugins.push(plugin);
        }

        Ok(Self {
            plugins,
            by_extension,
            filename_claims,
            skipped,
        })
    }

    /// The plugin that claims `ext` (with or without a leading dot, any case).
    pub fn for_extension(&self, ext: &str) -> Option<&dyn LanguagePlugin> {
        self.by_extension
            .get(&normalize_ext(ext))
            .map(|&i| &self.plugins[i] as &dyn LanguagePlugin)
    }

    /// The plugin that claims `rel` by its **extension or basename** (S-062,
    /// [CR-010], [FR-CG-01], [FR-IX-02] as modified).
    ///
    /// Resolution order, extension first so a code/doc file is unaffected:
    /// 1. the file's extension (`for_extension`);
    /// 2. else the **exact** basename among the `filenames` claims (so
    ///    `Dockerfile` binds the Dockerfile plugin);
    /// 3. else the documented **`Name.*` prefix** rule — a basename `Name.<rest>`
    ///    matches a claim `Name` (so `Dockerfile.dev` binds the same plugin).
    ///
    /// Returns the first match in declaration order, so the lookup is
    /// deterministic ([NFR-RA-06]).
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    /// [FR-IX-02]: ../../../docs/specs/requirements/FR-IX-02.md
    pub fn for_path(&self, rel: &str) -> Option<&dyn LanguagePlugin> {
        let path = Path::new(rel);
        if let Some(plugin) = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.for_extension(ext))
        {
            return Some(plugin);
        }
        let base = path.file_name().and_then(|b| b.to_str())?;
        // Exact basename claim first, then the `Name.*` prefix rule.
        let idx = self
            .filename_claims
            .iter()
            .find(|(claim, _)| claim == base)
            .or_else(|| {
                self.filename_claims.iter().find(|(claim, _)| {
                    base.len() > claim.len()
                        && base.starts_with(claim.as_str())
                        && base.as_bytes()[claim.len()] == b'.'
                })
            })
            .map(|&(_, i)| i)?;
        Some(&self.plugins[idx] as &dyn LanguagePlugin)
    }

    /// All loaded plugins in declaration order (the `languages` listing order).
    pub fn iter(&self) -> impl Iterator<Item = &dyn LanguagePlugin> {
        self.plugins.iter().map(|p| p as &dyn LanguagePlugin)
    }

    /// The set of file extensions (normalised: lower-case, no leading dot) whose
    /// loaded plugin declares the **reachability capability** (S-159, [CR-043],
    /// [ADR-39]). The [annotation-engine] gates Pass-3 dead-code reachability on
    /// it: a callable whose extension is absent renders `is_dead = NULL` ("not
    /// computed", [NFR-CC-04]) rather than a fabricated verdict. Empty when no
    /// loaded grammar declares it — every callable then renders NULL, the honest
    /// degraded state.
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn reachability_extensions(&self) -> std::collections::HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| p.supports_reachability())
            .flat_map(|p| p.extensions().iter().map(|e| normalize_ext(e)))
            .collect()
    }

    /// The file extensions (normalised: lower-case, no leading dot) whose loaded
    /// plugin declares its import specifiers **paths**
    /// ([`ImportSpecifier::Path`](super::ImportSpecifier::Path); S-439,
    /// [CR-142] D1), each mapped to the extensions a relative specifier written
    /// in that file may resolve to — its plugin's
    /// [`specifier_extensions`](super::PluginManifest::specifier_extensions).
    ///
    /// The binder's twin of [`reachability_extensions`](Self::reachability_extensions).
    /// The keys say which imports bind by path rules and never through the
    /// member-path scope hierarchy, so a bare package specifier (`react`) can
    /// never land on a workspace file that shares its name; the values keep a
    /// relative specifier inside its own language, so a TypeScript `./helper`
    /// never binds a `helper.py` beside it ([NFR-RA-05]). Go declares no
    /// specifier extensions, so its set is empty.
    ///
    /// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub fn specifier_target_extensions(
        &self,
    ) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
        self.plugins
            .iter()
            .filter(|p| p.semantics().import_specifier == super::ImportSpecifier::Path)
            .flat_map(|p| {
                let targets: std::collections::HashSet<String> = p
                    .semantics()
                    .specifier_extensions
                    .iter()
                    .map(|e| normalize_ext(e))
                    .collect();
                p.extensions()
                    .iter()
                    .map(move |e| (normalize_ext(e), targets.clone()))
            })
            .collect()
    }

    /// The file extensions (normalised: lower-case, no leading dot) whose loaded
    /// plugin declares a **package-shaped** module path
    /// ([`PackageModules`](super::PackageModules); [CR-149]), each mapped to its
    /// plugin's source roots in declaration order.
    ///
    /// The binder's twin of
    /// [`specifier_target_extensions`](Self::specifier_target_extensions),
    /// consumed through [`crate::resolve::package_key::PackageLayout`]. Every
    /// extension absent from the map keeps the default module model.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    pub fn package_source_roots(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.plugins
            .iter()
            .filter_map(|p| Some((p, p.semantics().package_modules.as_ref()?)))
            .flat_map(|(p, pm)| {
                p.extensions()
                    .iter()
                    .map(move |e| (normalize_ext(e), pm.source_roots.clone()))
            })
            .collect()
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose loaded plugin
    /// declares the **declared-namespace** module model
    /// ([`ModuleModelKind::Namespace`](super::ModuleModelKind::Namespace),
    /// S-518, [FR-RS-13]) — consumed through
    /// [`crate::resolve::package_key::PackageLayout`] beside the package roots.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    pub fn namespace_extensions(&self) -> std::collections::HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| p.semantics().module_model == super::ModuleModelKind::Namespace)
            .flat_map(|p| p.extensions().iter().map(|e| normalize_ext(e)))
            .collect()
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose loaded plugin
    /// declares **path-model data** — package-file stems or import roots (S-519,
    /// [FR-RS-14]) — each mapped to that declaration. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the map folds no stem and keeps the default model's `src/` crate rule.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    pub fn path_models(&self) -> HashMap<String, PathModelDecl> {
        self.plugins
            .iter()
            .filter_map(|p| Some((p, PathModelDecl::of(p)?)))
            .flat_map(|(p, decl)| {
                p.extensions()
                    .iter()
                    .map(move |e| (normalize_ext(e), decl.clone()))
            })
            .collect()
    }

    /// The normalised extensions of every code plugin whose semantics
    /// `declares` — the one shape of the per-key extension sets the binder's
    /// [`PackageLayout`](crate::resolve::package_key::PackageLayout) reads
    /// (S-592: written once, not once per key).
    fn code_extensions_where(&self, declares: impl Fn(&Semantics) -> bool) -> HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| !p.is_documentation() && !p.is_artifact())
            .filter(|p| declares(p.semantics()))
            .flat_map(|p| p.extensions().iter().map(|e| normalize_ext(e)))
            .collect()
    }

    /// Each normalised extension of every code plugin for which `value` gives
    /// a value, mapped to it — the per-key extension maps' one shape, as
    /// [`code_extensions_where`](Self::code_extensions_where) is the sets'.
    fn code_extension_map<V: Clone>(&self, value: impl Fn(&Semantics) -> Option<V>) -> HashMap<String, V> {
        self.plugins
            .iter()
            .filter(|p| !p.is_documentation() && !p.is_artifact())
            .filter_map(|p| Some((p, value(p.semantics())?)))
            .flat_map(|(p, v)| p.extensions().iter().map(move |e| (normalize_ext(e), v.clone())))
            .collect()
    }

    /// Every code plugin's file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)), each mapped to the
    /// interop family its plugin binds within (S-519, [NFR-RA-05]) — the
    /// declared `[module_model] family`, else the plugin's own name. Consumed
    /// through [`crate::resolve::package_key::PackageLayout`], which partitions
    /// the type and namespace indexes by it.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub fn families(&self) -> HashMap<String, String> {
        self.code_extension_map(|s| Some(s.family.clone()))
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares a call target beyond a callable (S-521, [FR-RS-16]), each mapped
    /// to its [`CallTargets`]. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the map admits a `Function` or `Method` only, as before.
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    pub fn call_targets(&self) -> HashMap<String, CallTargets> {
        self.code_extension_map(|s| s.call_targets.any().then_some(s.call_targets))
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a supertype's edge kind follows its target (S-522,
    /// [FR-RS-15]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds each supertype as the kind its clause spells.
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    pub fn supertype_kind_follows_target(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.supertype_kind_follows_target)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a namespace sees the types of its enclosing namespaces
    /// (S-595, [FR-RS-45]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
    pub fn enclosing_namespace_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.enclosing_namespaces)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// explicitly declares `implicit_receiver = "none"`, so a bare call binds
    /// free callables only (S-590, [FR-RS-07]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set — Java's, which declares nothing — binds exactly as before.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    pub fn free_only_bare_call_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.bare_calls_free_only)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose unqualified
    /// in-class call, when no member of its class admits its arguments, goes on
    /// to the free functions and imports in scope (S-592, [FR-RS-43]): the
    /// plugin declares `implicit_call_falls_through` — Kotlin, whose
    /// resolution tries each scope level for an applicable candidate. Read off
    /// the descriptor, never the queries, so deciding it compiles no language
    /// (CR-197); the language's compile checks its `references` query records
    /// the supertypes the fall-through's guard needs. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`].
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub fn free_call_fallthrough_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.implicit_call_falls_through)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose plugin
    /// declares `implicit_root_members` (S-592), each mapped to those names: a
    /// call of one never falls through, as the root every class inherits may
    /// hold it. Consumed through [`crate::resolve::package_key::PackageLayout`].
    pub fn implicit_root_members(&self) -> HashMap<String, Vec<String>> {
        self.code_extension_map(|s| {
            (!s.implicit_root_members.is_empty()).then(|| s.implicit_root_members.clone())
        })
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares `overloaded_calls = true` (S-592, [FR-RS-43]): a bare call
    /// there binds only a callable whose parameter range admits it. Consumed
    /// through [`crate::resolve::package_key::PackageLayout`]; an extension
    /// absent from the set binds its bare calls by name, as before.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub fn overloaded_call_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.overloaded_calls)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) a code plugin
    /// declares in `arity_unchecked_extensions` (S-592, [FR-RS-43]): files of a
    /// language that enforces no arity — JavaScript, parsed by the TypeScript
    /// grammars — whose calls are never filtered by a parameter range.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub fn arity_unchecked_extensions(&self) -> HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| !p.is_documentation() && !p.is_artifact())
            .flat_map(|p| p.semantics().arity_unchecked_extensions.iter().map(|e| normalize_ext(e)))
            .collect()
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares the methods a peeled receiver wrapper provides (S-588,
    /// [FR-RS-42]), each mapped to that declaration. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the map peels no wrapper that provides a method.
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub fn wrapper_methods(&self) -> HashMap<String, BTreeMap<String, Vec<String>>> {
        self.code_extension_map(|s| (!s.wrapper_methods.is_empty()).then(|| s.wrapper_methods.clone()))
    }

    /// Grammars skipped at load due to an ABI mismatch ([FR-PL-03]).
    pub fn skipped(&self) -> &[SkippedGrammar] {
        &self.skipped
    }

    /// Compile every loaded language's queries now, stopping at the first that
    /// fails (CR-197, [FR-PL-02]). A language already compiled — at load for
    /// an override, or by an earlier first use — is not compiled again.
    ///
    /// Production never needs it: each language compiles on its first use. It
    /// is the test-time fail-fast for the embedded queries, and the way a
    /// harness prices what a cold start no longer pays.
    ///
    /// # Errors
    /// The [`PluginError`] of the first language whose queries do not compile,
    /// naming the query file — or, for a namespace language whose `symbols`
    /// does not capture its namespace, the descriptor.
    pub fn compile_all_queries(&self) -> Result<(), PluginError> {
        for plugin in &self.plugins {
            plugin.compiled_queries().map_err(PluginError::clone)?;
        }
        Ok(())
    }

    /// Number of successfully loaded grammars.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// `true` when no grammar loaded successfully.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// [`load`](Self::load), timed per phase ([CR-116], [NFR-PE-05]).
    ///
    /// Mirrors [`load_from`](Self::load_from)'s loop step for step, splitting
    /// its wall-clock into three buckets: `plugin.toml` parse, query
    /// compilation, and everything else the loop does (ABI assertion, override
    /// resolution, extension/filename bookkeeping — the [NFR-PE-05]-enumerated
    /// "`LanguageRegistry` construction"). `load`/`load_from` are untouched, so
    /// this diagnostic path's own cost never lands on the production
    /// cold-start path — used only by
    /// [`Engine::start_with_phase_report`](crate::Engine::start_with_phase_report).
    ///
    /// # Errors
    /// As [`load`](Self::load).
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    pub(crate) fn load_with_timings(
        project_root: impl AsRef<Path>,
    ) -> Result<(Self, RegistryLoadTimings), PluginError> {
        let entries = grammars::compiled();
        let abi_range = AbiRange::runtime();
        let project_root = Some(project_root.as_ref());

        let mut plugins: Vec<CompiledPlugin> = Vec::new();
        let mut by_extension: HashMap<String, usize> = HashMap::new();
        let mut filename_claims: Vec<(String, usize)> = Vec::new();
        let mut skipped: Vec<SkippedGrammar> = Vec::new();
        let mut timings = RegistryLoadTimings::default();

        for entry in &entries {
            let t = Instant::now();
            let manifest = PluginManifest::parse(entry.manifest_label, entry.manifest_toml)?;
            timings.manifest_parse += t.elapsed();

            let t = Instant::now();
            let language: Language = entry.language.into();
            let compiled_abi = language.abi_version();
            if let Err(reason) = assert_abi(manifest.abi_version, compiled_abi, &abi_range) {
                let skip = SkippedGrammar {
                    name: manifest.name.clone(),
                    reason,
                };
                tracing::warn!("{skip}");
                skipped.push(skip);
                timings.construction += t.elapsed();
                continue; // skip only this grammar — the run is not aborted
            }
            let override_dir = project_root.map(|root| override_dir_for(root, &manifest.name));
            timings.construction += t.elapsed();

            let t = Instant::now();
            let (queries, overridden) =
                compile_capabilities(entry, &manifest, &language, override_dir.as_deref())?;
            timings.query_compile += t.elapsed();

            let t = Instant::now();
            let plugin = CompiledPlugin::new(manifest, language, queries, overridden);

            let idx = plugins.len();
            for ext in plugin.extensions() {
                if let Some(prev) = by_extension.insert(normalize_ext(ext), idx) {
                    tracing::warn!(
                        "extension '{ext}' claimed by '{}' shadows '{}'",
                        plugin.name(),
                        plugins[prev].name()
                    );
                }
            }
            for fname in plugin.filenames() {
                if let Some((_, prev)) = filename_claims.iter().find(|(c, _)| c == fname) {
                    tracing::warn!(
                        "filename '{fname}' claimed by '{}' also claimed by '{}'",
                        plugin.name(),
                        plugins[*prev].name()
                    );
                }
                filename_claims.push((fname.clone(), idx));
            }
            plugins.push(plugin);
            timings.construction += t.elapsed();
        }

        Ok((
            Self {
                plugins,
                by_extension,
                filename_claims,
                skipped,
            },
            timings,
        ))
    }
}

/// Per-phase timings for [`LanguageRegistry::load_with_timings`] ([CR-116],
/// [NFR-PE-05]).
///
/// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
/// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct RegistryLoadTimings {
    /// Parsing every grammar's embedded `plugin.toml`.
    pub manifest_parse: Duration,
    /// Resolving every capability's query and compiling the languages that
    /// carry an override — or, for a query this process already compiled,
    /// fetching it from the cache (HF-3). Embedded queries compile on first
    /// use (CR-197), outside this phase.
    pub query_compile: Duration,
    /// Everything else the load loop does: ABI assertion, override-dir
    /// resolution, plugin/extension/filename bookkeeping — the
    /// [NFR-PE-05]-enumerated "`LanguageRegistry` construction".
    ///
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    pub construction: Duration,
}

/// Fail fast when a declared `body_node_kinds` entry (S-500, [FR-EX-11]) names
/// no named node kind of the built grammar — the same posture as a query that
/// does not compile ([FR-PL-02]). A misspelt kind would otherwise match nothing,
/// silently recording every callable of the language as bodyless.
///
/// Checked here, beside the queries, because both registry load paths resolve
/// a descriptor's queries through [`compile_capabilities`]. It needs no
/// compiled query, so it stays at load for every language.
///
/// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
/// [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
fn check_body_node_kinds(
    entry: &GrammarEntry,
    manifest: &PluginManifest,
    language: &Language,
) -> Result<(), PluginError> {
    match manifest
        .body_node_kinds
        .iter()
        .find(|kind| language.id_for_node_kind(kind, true) == 0)
    {
        Some(unknown) => Err(PluginError::Manifest {
            file: entry.manifest_label.to_string(),
            detail: format!(
                "body_node_kinds entry '{unknown}' is not a named node kind of the '{}' grammar",
                manifest.name
            ),
        }),
        None => Ok(()),
    }
}

/// Resolve every capability's query for one grammar, compiling them now only
/// when one is an on-disk override (CR-197; see [`queries::LanguageQueries`]).
///
/// Returns the language's queries and the list of query keys whose source was
/// an on-disk override. Each compiled query comes from the process-wide cache
/// when this grammar's capability already compiled to the same text.
fn compile_capabilities(
    entry: &GrammarEntry,
    manifest: &PluginManifest,
    language: &Language,
    override_dir: Option<&Path>,
) -> Result<(LanguageQueries, Vec<String>), PluginError> {
    check_body_node_kinds(entry, manifest, language)?;

    let mut resolved_queries = Vec::with_capacity(manifest.capabilities.len());
    let mut overridden = Vec::new();

    // Each declared capability has a required, fail-fast query (`validate`
    // guarantees the `[queries]` entry exists).
    let query_keys = manifest.capabilities.iter().map(String::as_str);

    for key in query_keys {
        // `validate` guarantees the capability's `[queries]` entry exists, so
        // the index is safe.
        let relative_path = &manifest.queries[key];
        let embedded = entry
            .embedded_queries
            .iter()
            .find(|q| q.relative_path == relative_path.as_str())
            .ok_or_else(|| PluginError::Manifest {
                file: entry.manifest_label.to_string(),
                detail: format!("query '{key}' maps to '{relative_path}' with no embedded source"),
            })?;

        let resolved = queries::resolve_query(
            key,
            relative_path,
            embedded.label,
            embedded.source,
            override_dir,
        )?;
        if resolved.overridden {
            overridden.push(key.to_string());
        }
        resolved_queries.push(resolved);
    }
    let queries = LanguageQueries::new(
        &manifest.name,
        entry.manifest_label,
        manifest.module_model_kind() == super::ModuleModelKind::Namespace,
        manifest.implicit_call_falls_through,
        resolved_queries,
        language,
    )?;

    Ok((queries, overridden))
}

/// The override directory for a language: `<root>/.logos/plugins/<name>/`.
fn override_dir_for(root: &Path, name: &str) -> std::path::PathBuf {
    root.join(".logos").join("plugins").join(name)
}

/// Normalise an extension for indexing: lower-cased, leading dot stripped.
fn normalize_ext(ext: &str) -> String {
    ext.trim_start_matches('.').to_ascii_lowercase()
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;

    /// A synthetic second grammar that reuses the real Rust `LanguageFn` but
    /// declares a bogus `abi_version` (99). Its descriptor therefore disagrees
    /// with the compiled grammar (ABI 15) and the registry must skip *it* while
    /// the genuine Rust grammar still loads — proving selectivity ([UAT-PL-02]).
    const SKIP_MANIFEST: &str = r#"
        name = "rustskip"
        extensions = ["rsx"]
        module_separator = "::"
        abi_version = 99
        capabilities = ["symbols"]
        [queries]
        symbols = "queries/symbols.scm"
    "#;

    fn skip_entry() -> GrammarEntry {
        GrammarEntry {
            manifest_label: "rustskip/plugin.toml",
            manifest_toml: SKIP_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "rustskip/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        }
    }

    /// A second grammar that loads cleanly (ABI 15, valid query) but claims the
    /// same `rs` extension as the real Rust grammar — to exercise the
    /// extension-collision warn-and-last-writer-wins path.
    const COLLIDE_MANIFEST: &str = r#"
        name = "rustdup"
        extensions = ["rs"]
        module_separator = "::"
        abi_version = 15
        capabilities = ["symbols"]
        [queries]
        symbols = "queries/symbols.scm"
    "#;

    fn collide_entry() -> GrammarEntry {
        GrammarEntry {
            manifest_label: "rustdup/plugin.toml",
            manifest_toml: COLLIDE_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "rustdup/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        }
    }

    /// NFR-MA-01 / S-015 acceptance: adding a language is *pure data* — a
    /// descriptor (with framework/export semantics), query text, and a
    /// `LanguageFn` row. This test assembles such a grammar entirely from
    /// literals against the unchanged registry and gets a fully capable
    /// plugin back: nothing in `logos-core` knows the language exists.
    #[test]
    fn a_new_language_loads_from_pure_data_with_no_core_edit() {
        const TOY_MANIFEST: &str = r#"
            name = "toylang"
            extensions = ["toy"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            export_convention = "underscore-private"
            framework_detectors = ["toyweb"]
            [queries]
            symbols = "queries/symbols.scm"
            [framework_methods]
            get = "GET"
        "#;
        let entry = GrammarEntry {
            manifest_label: "toylang/plugin.toml",
            manifest_toml: TOY_MANIFEST,
            // Any compiled grammar works — the point is the *registry* needs
            // no new code, only a LanguageFn it has never seen named.
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "toylang/queries/symbols.scm",
                source: "(function_item name: (identifier) @symbol.function)",
            }],
        };

        let mut entries = grammars::compiled();
        entries.push(entry);
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("a data-only grammar loads");

        let toy = reg.for_extension("toy").expect("toylang claims .toy");
        assert_eq!(toy.name(), "toylang");
        assert!(toy.query("symbols").is_some(), "its query compiled");
        let semantics = toy.semantics();
        assert_eq!(semantics.framework_detectors, ["toyweb"]);
        assert_eq!(semantics.framework_methods.get("get").unwrap(), "GET");
        assert_eq!(
            semantics.export_convention,
            crate::plugin::ExportConvention::UnderscorePrivate
        );
    }

    /// S-518 / FR-RS-13: a declared-namespace grammar whose `symbols` query
    /// names no `@module.namespace` would read every file as the global
    /// namespace, so its compile fails naming the descriptor — at the
    /// compile-every-query check for an embedded query (CR-197), at load for an
    /// override; the same grammar capturing it compiles with the model in its
    /// semantics.
    #[test]
    fn a_namespace_model_without_the_namespace_capture_fails_its_compile() {
        fn entry(source: &'static str) -> GrammarEntry {
            toy_grammar("toyns", "", "[module_model]\nkind = \"namespace\"", "symbols", source)
        }
        let load = |source: &'static str, root: Option<&Path>| {
            let mut entries = grammars::compiled();
            entries.push(entry(source));
            LanguageRegistry::load_from(&entries, AbiRange::runtime(), root, &mut |_| {})
        };
        const UNCAPTURED: &str = "(function_item name: (identifier) @symbol.function)";

        let reg = load(UNCAPTURED, None).expect("an embedded query compiles on first use");
        let err = reg
            .compile_all_queries()
            .expect_err("no namespace capture fails the compile")
            .to_string();
        assert!(err.contains("toyns/plugin.toml") && err.contains("@module.namespace"), "{err}");

        let root = tempfile::tempdir().unwrap();
        let qdir = override_dir_for(root.path(), "toyns").join("queries");
        std::fs::create_dir_all(&qdir).unwrap();
        std::fs::write(qdir.join("symbols.scm"), format!("; override\n{UNCAPTURED}")).unwrap();
        let Err(err) = load(UNCAPTURED, Some(root.path())) else {
            panic!("an override without the namespace capture fails the load");
        };
        let err = err.to_string();
        assert!(err.contains("toyns/plugin.toml") && err.contains("@module.namespace"), "{err}");

        let reg = load(
            "(function_item name: (identifier) @symbol.function)\n\
             (mod_item name: (identifier) @module.namespace)",
            None,
        )
        .expect("the captured namespace loads");
        reg.compile_all_queries().expect("the captured namespace compiles");
        assert_eq!(
            reg.for_extension("toyns").unwrap().semantics().module_model,
            crate::plugin::ModuleModelKind::Namespace
        );
        assert!(reg.namespace_extensions().contains("toyns"));
    }

    /// S-592: a language declaring `implicit_call_falls_through` must record
    /// its classes' supertypes — its `references` query captures
    /// `@ref.extends` — or its compile is refused naming the descriptor, as a
    /// namespace model's missing capture is. Checked at compile, never when the
    /// key is read.
    #[test]
    fn a_fall_through_language_without_the_supertype_capture_fails_its_compile() {
        fn entry(source: &'static str) -> GrammarEntry {
            let keys = "implicit_receiver = \"self\"\nimplicit_call_falls_through = true";
            toy_grammar("toyft", keys, "", "references", source)
        }
        let load = |source: &'static str| {
            let mut entries = grammars::compiled();
            entries.push(entry(source));
            LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
                .expect("an embedded query compiles on first use")
        };
        let err = load("(call_expression function: (identifier) @ref.call)")
            .compile_all_queries()
            .expect_err("no supertype capture fails the compile")
            .to_string();
        assert!(err.contains("toyft/plugin.toml") && err.contains("@ref.extends"), "{err}");
        load("(call_expression function: (identifier) @ref.call)\n(type_identifier) @ref.extends")
            .compile_all_queries()
            .expect("the capture satisfies the check");
    }

    /// S-500 / FR-EX-11 / FR-PL-02: a `body_node_kinds` entry must name a node
    /// kind of the descriptor's own grammar. A misspelt one would match nothing
    /// and silently record every callable bodyless, so the load fails naming
    /// the descriptor instead; a real kind loads and reaches the semantics.
    #[test]
    fn an_unknown_body_node_kind_fails_the_load_naming_the_descriptor() {
        fn entry(manifest: &'static str) -> GrammarEntry {
            GrammarEntry {
                manifest_label: "toybody/plugin.toml",
                manifest_toml: manifest,
                language: tree_sitter_rust::LANGUAGE,
                embedded_queries: &[grammars::EmbeddedQuery {
                    relative_path: "queries/symbols.scm",
                    label: "toybody/queries/symbols.scm",
                    source: "(function_item name: (identifier) @symbol.function)",
                }],
            }
        }
        const MISSPELT: &str = r#"
            name = "toybody"
            extensions = ["toyb"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            body_node_kinds = ["block", "blok"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        const REAL: &str = r#"
            name = "toybody"
            extensions = ["toyb"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            body_node_kinds = ["block"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;

        let mut entries = grammars::compiled();
        entries.push(entry(MISSPELT));
        let err = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect_err("a misspelt body kind fails the load");
        let message = err.to_string();
        assert!(
            message.contains("toybody/plugin.toml") && message.contains("'blok'"),
            "the error names the descriptor and the kind: {message}"
        );

        let mut entries = grammars::compiled();
        entries.push(entry(REAL));
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("a real body kind loads");
        let toy = reg.for_extension("toyb").expect("toybody claims .toyb");
        assert_eq!(toy.semantics().body_node_kinds, ["block"]);
    }

    #[test]
    fn extension_collision_warns_and_last_writer_wins() {
        let mut warnings = Vec::new();
        let mut entries = grammars::compiled(); // rust claims "rs" first
        entries.push(collide_entry()); // rustdup also claims "rs"

        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("a valid colliding grammar still loads");

        // Every compiled-in grammar plus the collider loaded; the later
        // declaration wins the `rs` lookup.
        assert_eq!(reg.len(), grammars::compiled().len() + 1);
        assert_eq!(reg.for_extension("rs").unwrap().name(), "rustdup");
        // ...and the collision was surfaced as a warning, not silently.
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("rs") && w.contains("shadow")),
            "expected an extension-shadow warning, got {warnings:?}"
        );
    }

    /// S-159 / CR-043 / ADR-39: the reachability-capability set is built from the
    /// loaded descriptors — Rust declares the capability (so `rs` is present),
    /// while a synthetic grammar that omits the flag is absent. The
    /// [`crate::annotate`] dead-code pass gates on exactly this set.
    #[test]
    fn reachability_extensions_collects_only_capable_languages() {
        // A second grammar that loads cleanly but does NOT declare reachability.
        const PLAIN: &str = r#"
            name = "toyplain"
            extensions = ["TOY"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let entry = GrammarEntry {
            manifest_label: "toyplain/plugin.toml",
            manifest_toml: PLAIN,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "toyplain/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        };
        let mut entries = grammars::compiled();
        entries.push(entry);
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("the grammars load");

        let exts = reg.reachability_extensions();
        assert!(
            exts.contains("rs"),
            "Rust declares the reachability capability — `rs` is capable"
        );
        assert!(
            !exts.contains("toy"),
            "a grammar that omits the flag is not capable (normalised lower-case)"
        );
    }

    /// The path-grammar extension map covers exactly the grammars that declare
    /// `import_specifier = "path"` (S-439): both TypeScript grammars and Go, and
    /// no name-grammar language — Rust above all, whose imports must stay on the
    /// scope hierarchy byte for byte — and each maps to its own family only.
    #[test]
    fn specifier_target_extensions_collects_only_path_grammar_languages() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let exts = reg.specifier_target_extensions();
        #[cfg(feature = "lang-typescript")]
        for ext in ["ts", "js", "mjs", "cjs", "tsx", "jsx"] {
            let targets = exts
                .get(ext)
                .unwrap_or_else(|| panic!("`{ext}` specifiers are paths"));
            // A TS/JS relative specifier resolves within the TS family only.
            assert!(targets.contains("ts") && targets.contains("tsx"), "{ext}: {targets:?}");
            assert!(!targets.contains("py") && !targets.contains("go"), "{ext}: {targets:?}");
        }
        #[cfg(feature = "lang-go")]
        assert_eq!(
            exts.get("go").map(|t| t.len()),
            Some(0),
            "Go import paths are paths, and name no file extension"
        );
        for ext in ["rs", "py", "java", "kt", "cs", "php", "rb", "scala"] {
            assert!(!exts.contains_key(ext), "`{ext}` specifiers are names");
        }
    }

    /// Only Java (CR-149) declares source roots, under its two Maven roots.
    /// Kotlin's S-472 roots gave way to its declared package (S-518), the model
    /// PHP, C# and Scala declare too; every other grammar — Rust above all —
    /// keeps the default module model.
    #[test]
    fn package_source_roots_collects_only_the_opted_in_jvm_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let roots = reg.package_source_roots();
        #[cfg(feature = "lang-java")]
        assert_eq!(
            roots.get("java").map(Vec::as_slice),
            Some(["src/main/java".to_string(), "src/test/java".to_string()].as_slice())
        );
        for ext in ["rs", "py", "ts", "go", "cs", "php", "rb", "scala", "kt", "kts"] {
            assert!(!roots.contains_key(ext), "`{ext}` declares no source roots");
        }
    }

    /// The declared-namespace model (S-518, FR-RS-13) is opted into by PHP, C#,
    /// Kotlin and Scala — every extension of each — and by nothing else.
    #[test]
    fn namespace_extensions_collects_only_the_declared_namespace_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let exts = reg.namespace_extensions();
        #[cfg(all(
            feature = "lang-php",
            feature = "lang-c-sharp",
            feature = "lang-kotlin",
            feature = "lang-scala"
        ))]
        for ext in ["php", "cs", "kt", "kts", "scala", "sc"] {
            assert!(exts.contains(ext), "`{ext}` is keyed by its declared namespace");
        }
        for ext in ["rs", "py", "ts", "go", "rb", "java", "c", "cpp"] {
            assert!(!exts.contains(ext), "`{ext}` keeps its own module model");
        }
    }

    /// The call targets (S-521, FR-RS-16): Python, Kotlin and Scala declare that
    /// calling a class instantiates it — every extension of each — and C that a
    /// macro is callable. Every other grammar, Rust and Java above all, admits a
    /// callable only, so is absent from the map.
    #[test]
    fn call_targets_collects_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let targets = reg.call_targets();
        let classes = CallTargets {
            classes: true,
            macros: false,
        };
        #[cfg(all(feature = "lang-python", feature = "lang-kotlin", feature = "lang-scala"))]
        for ext in ["py", "pyi", "kt", "kts", "scala", "sc"] {
            assert_eq!(targets.get(ext), Some(&classes), "`{ext}` instantiates a called class");
        }
        #[cfg(feature = "lang-c")]
        assert_eq!(
            targets.get("c"),
            Some(&CallTargets {
                classes: false,
                macros: true
            })
        );
        for ext in ["rs", "java", "ts", "tsx", "go", "cs", "php", "rb", "cpp", "h", "md"] {
            assert!(!targets.contains_key(ext), "`{ext}` binds a callable only");
        }
    }

    /// The supertype key (S-522, FR-RS-15): C# and Kotlin write a base class
    /// and an interface in one list, so their every extension declares it.
    /// Every other grammar — Java, PHP and Python above all, whose clauses say
    /// which is which — is absent.
    #[test]
    fn supertype_kind_follows_target_collects_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.supertype_kind_follows_target();
        #[cfg(all(feature = "lang-c-sharp", feature = "lang-kotlin"))]
        for ext in ["cs", "kt", "kts"] {
            assert!(declaring.contains(ext), "`{ext}` follows the bound supertype's kind");
        }
        for ext in ["rs", "java", "php", "py", "scala", "ts", "go", "rb", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` spells each supertype's kind");
        }
    }

    /// The enclosing-namespace key (S-595, FR-RS-45): C# alone declares it.
    /// PHP, Kotlin, Scala and Java — whose namespaces and packages do not nest
    /// their types' visibility this way — are absent.
    #[test]
    fn enclosing_namespace_extensions_collect_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.enclosing_namespace_extensions();
        #[cfg(feature = "lang-c-sharp")]
        assert!(declaring.contains("cs"), "`cs` sees its enclosing namespaces");
        for ext in ["rs", "java", "kt", "kts", "php", "py", "scala", "ts", "go", "rb", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` binds exactly as before");
        }
    }

    /// The free-only bare-call key (S-590, FR-RS-07): Go, Rust, Python, PHP,
    /// TypeScript (with JavaScript) and TSX declare `implicit_receiver = "none"`
    /// explicitly. Java declares nothing, and the `"self"` languages — C#,
    /// Kotlin, Scala, C++, Ruby — reach the instance, so all are absent.
    #[test]
    fn free_only_bare_call_extensions_collect_only_the_explicit_none_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.free_only_bare_call_extensions();
        for ext in ["rs", "go", "py", "php", "ts", "js", "mjs", "cjs", "tsx", "jsx"] {
            assert!(declaring.contains(ext), "`{ext}`'s bare call reaches no member");
        }
        for ext in ["java", "cs", "kt", "kts", "scala", "cpp", "rb", "c", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` binds exactly as before");
        }
    }

    /// The arity sets (S-592, FR-RS-43): the overloading languages are Java,
    /// Kotlin, Scala, C# and C++; an implicit-instance call falls through to a
    /// free function in Kotlin alone — in C#, Scala, C++ and Ruby a member of
    /// that name hides every outer one; and only JavaScript's extensions —
    /// claimed by the two TypeScript grammars — are never filtered, while
    /// `.ts`/`.tsx` are.
    #[test]
    fn the_arity_extension_sets_name_the_declaring_grammars_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let overloaded = reg.overloaded_call_extensions();
        for ext in ["java", "kt", "kts", "scala", "cs", "cpp", "hpp"] {
            assert!(overloaded.contains(ext), "`{ext}` overloads by name");
        }
        for ext in ["rs", "go", "py", "php", "ts", "js", "rb", "c", "md"] {
            assert!(!overloaded.contains(ext), "`{ext}`'s bare call binds by name");
        }
        let fallthrough = reg.free_call_fallthrough_extensions();
        for ext in ["kt", "kts"] {
            assert!(fallthrough.contains(ext), "`{ext}`'s instance call falls through");
        }
        for ext in ["cs", "scala", "cpp", "rb", "java", "rs", "go", "py", "php", "ts", "js", "c"] {
            assert!(!fallthrough.contains(ext), "`{ext}`'s call never falls through");
        }
        let roots = reg.implicit_root_members();
        assert_eq!(roots.keys().filter(|e| e.starts_with("kt")).count(), 2, "{roots:?}");
        assert_eq!(roots["kt"], ["equals", "hashCode", "toString"]);
        let mut unchecked: Vec<String> = reg.arity_unchecked_extensions().into_iter().collect();
        unchecked.sort();
        assert_eq!(unchecked, ["cjs", "js", "jsx", "mjs"]);
    }

    /// The wrapper-method table (S-588, FR-RS-42): Rust declares what `Arc`,
    /// `Rc` and `Box` provide themselves — `clone` among them — and no
    /// reference wrapper; no other shipped grammar peels a receiver.
    #[test]
    fn wrapper_methods_are_declared_by_rust_alone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declared = reg.wrapper_methods();
        assert_eq!(declared.keys().collect::<Vec<_>>(), ["rs"], "only Rust peels a receiver");
        let rust = &declared["rs"];
        for wrapper in ["Arc", "Rc", "Box"] {
            assert!(rust[wrapper].iter().any(|m| m == "clone"), "{wrapper} provides `clone`");
        }
        assert!(!rust.contains_key("&") && !rust.contains_key("&mut"), "a reference provides nothing");
    }

    /// A synthetic artifact-class grammar (S-062, CR-010): it reuses the Rust
    /// `LanguageFn` (any compiled grammar works — `for_path` is a pure lookup, no
    /// parse) but declares `artifact = true` and basename claims, proving the
    /// substrate admits a filename-claimed format from **pure descriptor data**
    /// with no core edit (NFR-MA-01).
    const ARTIFACT_MANIFEST: &str = r#"
        name = "dockerfile"
        extensions = ["dockerfile"]
        module_separator = "/"
        abi_version = 15
        capabilities = []
        artifact = true
        filenames = ["Dockerfile"]
    "#;

    fn artifact_entry() -> GrammarEntry {
        GrammarEntry {
            manifest_label: "dockerfile/plugin.toml",
            manifest_toml: ARTIFACT_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[],
        }
    }

    #[test]
    fn for_path_admits_by_extension_or_claimed_basename() {
        let mut entries = grammars::compiled();
        entries.push(artifact_entry());
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("the artifact grammar loads from pure data");

        // The artifact plugin is the third class — flagged, with its basename claim
        // surfaced.
        let docker = reg
            .for_extension("dockerfile")
            .expect("claims the .dockerfile extension");
        assert!(docker.is_artifact(), "the descriptor is an artifact plugin");
        assert_eq!(docker.filenames(), ["Dockerfile"]);

        // Extension-or-basename admission (FR-IX-02 as modified):
        // 1. extension still resolves code files.
        assert_eq!(reg.for_path("src/lib.rs").map(|p| p.name()), Some("rust"));
        // 2. an extensionless `Dockerfile` resolves via its exact basename claim.
        assert_eq!(
            reg.for_path("Dockerfile").map(|p| p.name()),
            Some("dockerfile"),
            "extensionless Dockerfile admitted by its basename claim"
        );
        assert_eq!(
            reg.for_path("deploy/Dockerfile").map(|p| p.name()),
            Some("dockerfile"),
            "a nested Dockerfile resolves by basename regardless of directory"
        );
        // 3. the `Name.*` prefix rule binds `Dockerfile.dev` to the same plugin.
        assert_eq!(
            reg.for_path("Dockerfile.dev").map(|p| p.name()),
            Some("dockerfile"),
            "Dockerfile.dev binds via the Name.* prefix rule"
        );
        // A look-alike that is neither the exact name nor a `Name.` prefix is not
        // claimed — `Dockerfileish` shares a prefix but no dot boundary.
        assert!(
            reg.for_path("Dockerfileish").is_none(),
            "the prefix rule requires a `.` boundary, never a bare prefix"
        );
        // An unrelated extensionless file is unclaimed.
        assert!(reg.for_path("LICENSE").is_none());
    }

    #[test]
    fn loads_the_real_rust_grammar() {
        let reg = LanguageRegistry::load_from(
            &grammars::compiled(),
            AbiRange::runtime(),
            None,
            &mut |_| {},
        )
        .expect("embedded grammars load cleanly");

        assert!(!reg.is_empty());
        let rust = reg.for_extension("rs").expect("rust claims .rs");
        assert_eq!(rust.name(), "rust");
        assert!(rust.capabilities().iter().any(|c| c == "symbols"));
        assert!(rust.query("symbols").is_some());
    }

    #[test]
    fn extension_lookup_is_case_and_dot_insensitive() {
        let reg = LanguageRegistry::load_from(
            &grammars::compiled(),
            AbiRange::runtime(),
            None,
            &mut |_| {},
        )
        .unwrap();
        assert!(reg.for_extension("rs").is_some());
        assert!(reg.for_extension(".rs").is_some());
        assert!(reg.for_extension(".RS").is_some());
        // `.py` resolves exactly when the Python grammar is compiled in (S-015).
        assert_eq!(
            reg.for_extension("py").is_some(),
            cfg!(feature = "lang-python")
        );
        assert!(reg.for_extension("zig").is_none());
    }

    #[test]
    fn abi_mismatch_skips_only_the_affected_grammar_and_warns() {
        let mut warnings = Vec::new();
        let mut entries = grammars::compiled();
        entries.push(skip_entry());

        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("an ABI mismatch must not abort the run (FR-PL-03)");

        // The genuine grammar still works...
        assert!(reg.for_extension("rs").is_some(), "rust must still load");
        // ...the bogus one is skipped, not loaded...
        assert!(
            reg.for_extension("rsx").is_none(),
            "rustskip must be skipped"
        );
        // ...and recorded with the disagreeing-ABI reason.
        assert_eq!(reg.skipped().len(), 1);
        assert_eq!(reg.skipped()[0].name, "rustskip");
        // ...with a warning emitted to the sink (stderr in production).
        assert!(
            warnings.iter().any(|w| w.contains("rustskip")),
            "expected a skip warning naming rustskip, got {warnings:?}"
        );
    }

    /// FR-PL-03 / S-061 (CR-009 CRA-01): a *forced* Scala grammar failure
    /// skips Scala alone with a warning and never aborts the run — the runtime
    /// half of the gate that makes the highest-risk grammar safe to ship. The
    /// real Scala `LanguageFn` (compiled ABI 15) is paired with a descriptor that
    /// declares ABI 14, so `assert_abi` reports a disagreement and the registry
    /// skips it while every genuine grammar still loads.
    #[cfg(feature = "lang-scala")]
    #[test]
    fn a_forced_scala_grammar_failure_skips_only_scala_and_warns() {
        const TAMPERED_SCALA: &str = r#"
            name = "scala"
            extensions = ["scala", "sc"]
            module_separator = "."
            abi_version = 14
            capabilities = ["symbols"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let tampered = GrammarEntry {
            manifest_label: "scala/plugin.toml",
            manifest_toml: TAMPERED_SCALA,
            language: tree_sitter_scala::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "scala/queries/symbols.scm",
                source: "(class_definition name: (identifier) @c)",
            }],
        };
        // The full compiled set, then the real Scala row replaced by the
        // tampered one (push is enough: last declaration wins on the `scala`/`sc`
        // extensions, so the tampered row is the one the ABI check sees).
        let mut entries = grammars::compiled();
        entries.push(tampered);

        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("a forced Scala failure must not abort the run (FR-PL-03)");

        // Every genuine grammar still loaded (Rust is the harness's control).
        assert!(reg.for_extension("rs").is_some(), "rust must still load");
        // Scala was skipped with the disagreeing-ABI reason, naming scala.
        assert!(
            reg.skipped().iter().any(|s| s.name == "scala"),
            "scala must be skipped, got {:?}",
            reg.skipped()
        );
        assert!(
            warnings.iter().any(|w| w.contains("scala")),
            "expected a skip warning naming scala, got {warnings:?}"
        );
    }

    /// A plugin declaring the `invocations` capability with no loadable query
    /// file must fail the load outright, not degrade to silent no-capture
    /// (S-340, [FR-WS-08], [FR-PL-03]). The descriptor's `[queries]` entry
    /// points at a relative path with no matching [`grammars::EmbeddedQuery`] —
    /// the shape a language would ship if its `invocations.scm` were forgotten
    /// from `grammars::compiled` while its `plugin.toml` already claimed the
    /// capability. This is a hard [`PluginError`], distinct from the ABI-mismatch
    /// skip-and-warn path ([FR-PL-03]): a missing query is a descriptor bug, not
    /// a version disagreement, so it must never be silently tolerated.
    ///
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    /// [FR-PL-03]: ../../../docs/specs/requirements/FR-PL-03.md
    #[test]
    fn a_plugin_declaring_invocations_with_no_embedded_query_fails_the_load() {
        const MISSING_QUERY_MANIFEST: &str = r#"
            name = "toyinvocations"
            extensions = ["tia"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols", "invocations"]
            [queries]
            symbols = "queries/symbols.scm"
            invocations = "queries/invocations.scm"
        "#;
        let entry = GrammarEntry {
            manifest_label: "toyinvocations/plugin.toml",
            manifest_toml: MISSING_QUERY_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            // Only `symbols` is embedded — `invocations` has no backing source,
            // exactly as if the language's `.scm` file were never wired into
            // `grammars::compiled`.
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "toyinvocations/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        };
        let mut entries = grammars::compiled();
        entries.push(entry);

        let err = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect_err("a declared-but-unloadable `invocations` query must fail the load");

        match err {
            PluginError::Manifest { file, detail } => {
                assert_eq!(file, "toyinvocations/plugin.toml");
                assert!(
                    detail.contains("invocations"),
                    "error must name the offending capability, got {detail:?}"
                );
            }
            other => panic!("expected Manifest error, got {other:?}"),
        }
    }

    #[test]
    fn narrow_abi_range_skips_an_in_spec_grammar() {
        // A runtime that understands no real ABI (1..=2) can load none of the
        // compiled grammars (ABI 14/15): every one is skipped, the registry is
        // empty, and the run is not aborted.
        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(
            &grammars::compiled(),
            AbiRange { min: 1, max: 2 },
            None,
            &mut |w| warnings.push(w.to_string()),
        )
        .expect("ABI skip is not an error");
        assert!(reg.is_empty());
        assert_eq!(reg.skipped().len(), grammars::compiled().len());
        assert!(!warnings.is_empty());
    }

    /// Write `source` as the on-disk override of Rust's `capability` query under
    /// `root`, returning the override file's path.
    fn write_rust_override(root: &Path, capability: &str, source: &str) -> std::path::PathBuf {
        let dir = override_dir_for(root, "rust").join("queries");
        std::fs::create_dir_all(&dir).expect("override dir");
        let file = dir.join(format!("{capability}.scm"));
        std::fs::write(&file, source).expect("override file");
        file
    }

    /// The compiled `capability` query Rust carries in `reg`, as an address —
    /// two registries share a compiled query exactly when these are equal.
    fn rust_query(reg: &LanguageRegistry, capability: &str) -> *const tree_sitter::Query {
        reg.for_extension("rs")
            .expect("rust claims .rs")
            .query(capability)
            .unwrap_or_else(|| panic!("rust compiles a `{capability}` query"))
    }

    /// HF-3: two loads over two roots with no overrides share each compiled
    /// query rather than recompiling it — the cost a workspace's member engine
    /// starts paid once per member.
    #[test]
    fn loads_over_two_roots_without_overrides_share_every_compiled_query() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let reg_a = LanguageRegistry::load(a.path()).expect("root a loads");
        let reg_b = LanguageRegistry::load(b.path()).expect("root b loads");

        let mut compared = 0;
        for plugin in reg_a.iter() {
            let other = reg_b
                .for_extension(&plugin.extensions()[0])
                .expect("both roots load the same grammars");
            for cap in plugin.capabilities() {
                assert!(
                    std::ptr::eq(plugin.query(cap).unwrap(), other.query(cap).unwrap()),
                    "{}/{cap}: two roots compiled the same query twice",
                    plugin.name()
                );
                compared += 1;
            }
        }
        assert!(compared > 0, "no capability query was compared");
    }

    /// HF-3: a root overriding one capability gets its own compiled query for
    /// that capability — never a neighbour's — and the shared one for the rest.
    #[test]
    fn an_override_gets_its_own_query_and_shares_the_rest() {
        let (plain, tuned) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write_rust_override(
            tuned.path(),
            "symbols",
            "; HF-3 override, own entry\n(function_item name: (identifier) @symbol.function)",
        );
        let reg_plain = LanguageRegistry::load(plain.path()).expect("plain root loads");
        let reg_tuned = LanguageRegistry::load(tuned.path()).expect("tuned root loads");

        let tuned_rust = reg_tuned.for_extension("rs").unwrap();
        assert_eq!(tuned_rust.overridden_capabilities(), ["symbols"]);
        assert!(
            !std::ptr::eq(rust_query(&reg_plain, "symbols"), rust_query(&reg_tuned, "symbols")),
            "the overriding root must not be served the embedded `symbols` query"
        );
        assert_eq!(
            tuned_rust.query("symbols").unwrap().capture_names(),
            ["symbol.function"],
            "the overriding root runs its own query text"
        );
        assert!(
            std::ptr::eq(
                rust_query(&reg_plain, "references"),
                rust_query(&reg_tuned, "references")
            ),
            "a capability the root does not override is shared"
        );
    }

    /// HF-3: the cache is keyed on the query's content, not on where it came
    /// from — two roots carrying byte-identical overrides share one compiled
    /// query, so repeated loads grow nothing per root.
    #[test]
    fn identical_overrides_at_two_roots_share_one_compiled_query() {
        let source = "; HF-3 identical override\n(function_item name: (identifier) @f)";
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write_rust_override(a.path(), "symbols", source);
        write_rust_override(b.path(), "symbols", source);
        let reg_a = LanguageRegistry::load(a.path()).expect("root a loads");
        let reg_b = LanguageRegistry::load(b.path()).expect("root b loads");
        assert!(std::ptr::eq(
            rust_query(&reg_a, "symbols"),
            rust_query(&reg_b, "symbols")
        ));
    }

    /// HF-3: an override that fails to compile still fails the load naming its
    /// file, every time, and leaves nothing behind that a later load could be
    /// served: a valid root loads, and the same file fixed in place compiles
    /// its new text.
    #[test]
    fn a_failing_override_fails_loud_and_does_not_poison_the_cache() {
        let (bad, good) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let file = write_rust_override(bad.path(), "symbols", "(no_such_node_kind) @x");

        for attempt in 1..=2 {
            match LanguageRegistry::load(bad.path()) {
                Err(PluginError::QueryCompile { file: named, .. }) => {
                    assert_eq!(named, file.display().to_string(), "attempt {attempt}")
                }
                other => panic!("attempt {attempt}: expected QueryCompile, got {other:?}"),
            }
        }

        LanguageRegistry::load(good.path()).expect("a valid root still loads");

        std::fs::write(
            &file,
            "; HF-3 fixed override\n(function_item name: (identifier) @fixed)",
        )
        .unwrap();
        let fixed = LanguageRegistry::load(bad.path()).expect("the fixed override loads");
        assert_eq!(
            fixed.for_extension("rs").unwrap().query("symbols").unwrap().capture_names(),
            ["fixed"],
            "the fixed file's text is what runs"
        );

        // Editing a valid override again recompiles it: the entry is the text's,
        // never the path's, so a load is never served the file's previous text.
        std::fs::write(
            &file,
            "; HF-3 edited override\n(function_item name: (identifier) @edited)",
        )
        .unwrap();
        let edited = LanguageRegistry::load(bad.path()).expect("the edited override loads");
        assert_eq!(
            edited.for_extension("rs").unwrap().query("symbols").unwrap().capture_names(),
            ["edited"],
            "the edited file's text is what runs, not the one it replaced"
        );
    }

    /// HF-3 measurement, not a guard: times `N` consecutive
    /// [`load`](LanguageRegistry::load)s over `N` fresh roots. Run it alone so
    /// the first load is the process's first:
    /// `cargo test -p logos-core --lib measure_consecutive_registry_loads -- --ignored --nocapture`
    #[test]
    #[ignore = "measurement: run alone with --ignored --nocapture"]
    fn measure_consecutive_registry_loads() {
        const N: usize = 20;
        let roots: Vec<_> = (0..N).map(|_| tempfile::tempdir().unwrap()).collect();
        let times: Vec<Duration> = roots
            .iter()
            .map(|root| {
                let t = Instant::now();
                LanguageRegistry::load(root.path()).expect("load");
                t.elapsed()
            })
            .collect();
        let rest = &times[1..];
        let total: Duration = times.iter().sum();
        eprintln!(
            "HF-3 {N} loads: first {:?}; loads 2..={N} mean {:?} (min {:?}, max {:?}); total {:?}",
            times[0],
            rest.iter().sum::<Duration>() / rest.len() as u32,
            rest.iter().min().unwrap(),
            rest.iter().max().unwrap(),
            total
        );
    }

    /// Load `entries` with no override root and compile every query in it —
    /// the test-time fail-fast for embedded queries (CR-197, [FR-PL-02]).
    /// Returns how many capability queries compiled.
    ///
    /// [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
    fn compile_every_embedded_query(entries: &[GrammarEntry]) -> Result<usize, PluginError> {
        let reg = LanguageRegistry::load_from(entries, AbiRange::runtime(), None, &mut |_| {})?;
        reg.compile_all_queries()?;
        Ok(reg
            .iter()
            .flat_map(|p| p.capabilities().iter().map(move |c| p.query(c)))
            .filter(Option::is_some)
            .count())
    }

    /// A language entry whose one `symbols` query has `source` — a fresh
    /// language per call site, so its compiles and first-use reports are its
    /// own however the test binary interleaves.
    fn toy_entry(name: &'static str, source: &'static str) -> GrammarEntry {
        toy_grammar(name, "", "", "symbols", source)
    }

    /// A language entry over the Rust grammar whose one query serves
    /// `capability` with `source`, its descriptor carrying `top_keys` and then
    /// `tables` (the descriptor checks' fixtures: a namespace model, a
    /// fall-through key).
    fn toy_grammar(
        name: &'static str,
        top_keys: &str,
        tables: &str,
        capability: &str,
        source: &'static str,
    ) -> GrammarEntry {
        GrammarEntry {
            manifest_label: format!("{name}/plugin.toml").leak(),
            manifest_toml: format!(
                r#"
                name = "{name}"
                extensions = ["{name}"]
                module_separator = "."
                abi_version = 15
                {top_keys}
                capabilities = ["{capability}"]
                {tables}
                [queries]
                {capability} = "queries/{capability}.scm"
                "#
            )
            .leak(),
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: vec![grammars::EmbeddedQuery {
                relative_path: format!("queries/{capability}.scm").leak(),
                label: format!("{name}/queries/{capability}.scm").leak(),
                source,
            }]
            .leak(),
        }
    }

    /// CR-197 / FR-PL-02: every embedded query of every compiled-in grammar
    /// compiles. Embedded queries no longer compile at load, so this is what
    /// keeps a broken shipped query from ever reaching a user.
    #[test]
    fn every_embedded_query_compiles() {
        let entries = grammars::compiled();
        let compiled = compile_every_embedded_query(&entries)
            .unwrap_or_else(|err| panic!("an embedded query does not compile: {err}"));
        let declared: usize = entries
            .iter()
            .map(|e| PluginManifest::parse(e.manifest_label, e.manifest_toml).unwrap())
            .map(|m| m.capabilities.len())
            .sum();
        assert!(compiled > 0, "no embedded query was compiled");
        assert_eq!(compiled, declared, "every declared capability compiled a query");
    }

    /// CR-197: a broken embedded query loads — nothing compiles at load — and
    /// fails the compile-every-query check naming its file.
    #[test]
    fn a_broken_embedded_query_fails_the_compile_check_naming_its_file() {
        let mut entries = grammars::compiled();
        entries.push(toy_entry("toybroken", "(no_such_node_kind) @x"));

        LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("a broken embedded query does not fail the load");
        match compile_every_embedded_query(&entries) {
            Err(PluginError::QueryCompile { file, .. }) => {
                assert_eq!(file, "toybroken/queries/symbols.scm")
            }
            other => panic!("expected QueryCompile naming the file, got {other:?}"),
        }
    }

    /// CR-197: a load compiles no embedded query; a language compiles, whole,
    /// on the first query anyone asks of it, and no other language does.
    #[test]
    fn a_load_compiles_no_embedded_query_until_its_language_is_used() {
        let root = tempfile::tempdir().unwrap();
        let reg = LanguageRegistry::load(root.path()).expect("embedded grammars load");
        let compiled = |reg: &LanguageRegistry| -> Vec<String> {
            reg.plugins
                .iter()
                .filter(|p| p.queries_compiled())
                .map(|p| p.name().to_string())
                .collect()
        };
        assert!(compiled(&reg).is_empty(), "compiled at load: {:?}", compiled(&reg));

        let rust = reg.for_extension("rs").expect("rust claims .rs");
        assert!(rust.query("symbols").is_some());
        assert_eq!(compiled(&reg), ["rust"], "only the used language compiled");
        assert!(
            rust.capabilities().iter().all(|c| rust.query(c).is_some()),
            "the first use compiled every capability of the language"
        );
    }

    /// CR-197 / S-592: building the layout the binder reads — every key a
    /// resolution, sync or `status` run consults, the arity ones included —
    /// compiles no language's queries. The fall-through key is read off the
    /// descriptor; a key derived from a compiled query would compile every
    /// language declaring it, whatever the repository holds.
    #[test]
    fn building_the_binders_layout_compiles_no_language() {
        let root = tempfile::tempdir().unwrap();
        let reg = LanguageRegistry::load(root.path()).expect("embedded grammars load");
        let layout = crate::resolve::package_key::PackageLayout::from_registry(&reg);
        assert!(layout.falls_through_to_free_calls("src/a.kt", "m"), "the key is read");
        let compiled: Vec<&str> =
            reg.plugins.iter().filter(|p| p.queries_compiled()).map(|p| p.name()).collect();
        assert!(compiled.is_empty(), "compiled by a layout build: {compiled:?}");
    }

    /// CR-197 / FR-PL-04: an override compiles its language at load — the
    /// embedded siblings too, as one unit — so a broken one fails where the
    /// operator sees it.
    #[test]
    fn an_override_compiles_its_whole_language_at_load() {
        let root = tempfile::tempdir().unwrap();
        write_rust_override(
            root.path(),
            "symbols",
            "; CR-197 eager override\n(function_item name: (identifier) @symbol.function)",
        );
        let reg = LanguageRegistry::load(root.path()).expect("the override loads");
        let eager: Vec<&str> = reg
            .plugins
            .iter()
            .filter(|p| p.queries_compiled())
            .map(|p| p.name())
            .collect();
        assert_eq!(eager, ["rust"], "only the overridden language compiled at load");
    }

    /// CR-197: several threads over several registries hitting one language's
    /// first use at once compile it once, all get the same query, and the
    /// compile is reported once for the process.
    #[test]
    fn concurrent_first_use_of_one_language_compiles_it_once() {
        const REGISTRIES: usize = 3;
        const THREADS: usize = 4;
        let mut entries = grammars::compiled();
        entries.push(toy_entry(
            "toyrace",
            "; CR-197 race\n(function_item name: (identifier) @symbol.function)",
        ));
        let regs: Vec<LanguageRegistry> = (0..REGISTRIES)
            .map(|_| {
                LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
                    .expect("loads")
            })
            .collect();
        let barrier = std::sync::Barrier::new(REGISTRIES * THREADS);
        let addresses: Vec<usize> = std::thread::scope(|scope| {
            let handles: Vec<_> = regs
                .iter()
                .flat_map(|reg| std::iter::repeat_n(reg, THREADS))
                .map(|reg| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let toy = reg.for_extension("toyrace").expect("toyrace loads");
                        barrier.wait();
                        let query: *const tree_sitter::Query =
                            toy.query("symbols").expect("compiles");
                        query as usize
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        assert_eq!(addresses.len(), REGISTRIES * THREADS);
        assert!(
            addresses.iter().all(|&a| a == addresses[0]),
            "every first use got the one compiled query: {addresses:?}"
        );
        let reports: Vec<_> = queries::first_use_compiles()
            .into_iter()
            .filter(|r| r.language == "toyrace")
            .collect();
        assert_eq!(reports.len(), 1, "one first-use report per process: {reports:?}");
        assert_eq!(reports[0].compiled, 1, "the one query compiled once: {reports:?}");
        assert!(reports[0].elapsed > Duration::ZERO, "the compile time is reported: {reports:?}");
    }

    /// [`load_with_timings`](LanguageRegistry::load_with_timings) is a
    /// near-duplicate of [`load`](LanguageRegistry::load)'s loop, kept
    /// separate so the [CR-116] measurement path never touches production
    /// cold start. A regression guard against the two drifting apart: same
    /// project root, same compiled-in grammar table, so both must load the
    /// same grammars and skip the same ones.
    ///
    /// [CR-116]: ../../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
    #[test]
    fn load_with_timings_matches_load() {
        let root = tempfile::TempDir::new().expect("temp root");
        let plain = LanguageRegistry::load(root.path()).expect("load succeeds");
        let (timed, timings) =
            LanguageRegistry::load_with_timings(root.path()).expect("load_with_timings succeeds");

        assert_eq!(
            plain.len(),
            timed.len(),
            "load_with_timings loads the same number of grammars as load"
        );
        assert_eq!(
            plain.skipped().len(),
            timed.skipped().len(),
            "load_with_timings skips the same grammars as load"
        );
        // Identity, not just count: two loaders that admit the same NUMBER of
        // grammars could still disagree on WHICH ones (or on WHICH extensions
        // each claims) and this test would not have noticed.
        let names = |r: &LanguageRegistry| -> std::collections::BTreeSet<String> {
            r.iter().map(|p| p.name().to_string()).collect()
        };
        assert_eq!(
            names(&plain),
            names(&timed),
            "load_with_timings loads the identical set of grammars as load, not merely the same count"
        );
        let extensions = |r: &LanguageRegistry| -> std::collections::BTreeSet<String> {
            r.iter().flat_map(|p| p.extensions().iter().cloned()).collect()
        };
        assert_eq!(
            extensions(&plain),
            extensions(&timed),
            "load_with_timings claims the identical extension set as load"
        );
        let skipped_names = |r: &LanguageRegistry| -> std::collections::BTreeSet<String> {
            r.skipped().iter().map(|s| s.name.clone()).collect()
        };
        assert_eq!(
            skipped_names(&plain),
            skipped_names(&timed),
            "load_with_timings skips the identical set of grammars as load"
        );
        assert!(
            timings.manifest_parse + timings.query_compile + timings.construction > Duration::ZERO,
            "a non-empty grammar table records some phase time: {timings:?}"
        );
    }
}
