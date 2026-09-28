//! The [`LanguageRegistry`] — the plugin micro-kernel ([plugin-registry], [ADR-09]).
//!
//! At startup the registry walks the compiled-in grammar table
//! ([`super::grammars::compiled`]) and, for each grammar:
//!
//! 1. parses its embedded `plugin.toml` ([FR-PL-02]);
//! 2. builds the `Language` from its `LanguageFn` and asserts ABI — a mismatch
//!    is skipped-and-warned, never fatal ([FR-PL-03], [NFR-PC-03]);
//! 3. resolves each capability's query (on-disk override shadows embedded) and
//!    compiles it, failing fast and naming the file on error ([FR-PL-02],
//!    [FR-PL-04]) — once per process for each distinct source, so a later load
//!    that resolves the same text shares the compiled query (HF-3,
//!    [`queries::compile_shared`]);
//! 4. indexes the loaded grammar by extension for `for_extension` lookups.
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

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

use tree_sitter::Language;

use super::abi::{assert_abi, AbiRange};
use super::error::{PluginError, SkippedGrammar};
use super::grammars::{self, GrammarEntry};
use super::manifest::PluginManifest;
use super::plugin::{CompiledPlugin, CompiledQueries, LanguagePlugin};
use super::queries;

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
    /// descriptor or a query that fails to compile is a hard error naming the
    /// file ([FR-PL-02]).
    ///
    /// # Errors
    /// Returns [`PluginError`] on a descriptor parse error, a query compile
    /// error, or an unreadable override file.
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

    /// Grammars skipped at load due to an ABI mismatch ([FR-PL-03]).
    pub fn skipped(&self) -> &[SkippedGrammar] {
        &self.skipped
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
    /// Resolving and compiling every capability's query — or, for a query this
    /// process already compiled, fetching it from the cache (HF-3), so only a
    /// process's first load times the compile itself.
    pub query_compile: Duration,
    /// Everything else the load loop does: ABI assertion, override-dir
    /// resolution, plugin/extension/filename bookkeeping — the
    /// [NFR-PE-05]-enumerated "`LanguageRegistry` construction".
    ///
    /// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
    pub construction: Duration,
}

/// Resolve and compile every capability's query for one grammar.
///
/// Returns the capability → compiled query map and the list of query keys whose
/// source was an on-disk override. Each query comes from the process-wide
/// cache when this grammar's capability already compiled to the same text.
fn compile_capabilities(
    entry: &GrammarEntry,
    manifest: &PluginManifest,
    language: &Language,
    override_dir: Option<&Path>,
) -> Result<(CompiledQueries, Vec<String>), PluginError> {
    let mut compiled = BTreeMap::new();
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
        let query = queries::compile_shared(language, &manifest.name, &resolved)?;
        compiled.insert(key.to_string(), query);
    }

    Ok((compiled, overridden))
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

    /// Only Java declares a package-shaped module path (CR-149), under its two
    /// Maven roots; every other grammar — Rust above all, and Kotlin, which is
    /// package-shaped too but not opted in — keeps the default module model.
    #[test]
    fn package_source_roots_collects_only_the_opted_in_java_grammar() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let roots = reg.package_source_roots();
        #[cfg(feature = "lang-java")]
        assert_eq!(
            roots.get("java").map(Vec::as_slice),
            Some(["src/main/java".to_string(), "src/test/java".to_string()].as_slice())
        );
        for ext in ["rs", "py", "ts", "go", "kt", "cs", "php", "rb", "scala"] {
            assert!(!roots.contains_key(ext), "`{ext}` keeps the default module model");
        }
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
