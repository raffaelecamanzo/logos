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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tree_sitter::Language;

use super::abi::{assert_abi, AbiRange};
use super::error::{PluginError, SkippedGrammar};
use super::grammars::{self, GrammarEntry};
use super::manifest::{CallTargets, PluginManifest};
use super::plugin::{CompiledPlugin, LanguagePlugin, Semantics};
use super::queries::{self, CaptureChecks, LanguageQueries};

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
    /// The extensions two plugins claim, one declaring it shared (S-582,
    /// [FR-PL-07]): C's `.h`. `by_extension` maps each to its default reader,
    /// the other plugin; which one reads it in this repository is decided by
    /// its sources ([`shared_owners_for`](Self::shared_owners_for)).
    ///
    /// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
    shared: Vec<SharedExtension>,
    /// Grammars skipped at load (ABI mismatch), recorded for `languages` and
    /// diagnostics ([FR-PL-03]).
    skipped: Vec<SkippedGrammar>,
}

/// One extension two plugins claim (S-582): `sharer` declares it in
/// `shared_extensions`, `owner` claims it outright and reads it unless the
/// repository's decision gives it to the sharer.
#[derive(Debug)]
struct SharedExtension {
    /// The normalised extension.
    ext: String,
    /// Index of the plugin that declares the extension shared.
    sharer: usize,
    /// Index of the plugin that claims it outright — its default reader.
    owner: usize,
    /// Whether this repository gives the extension to `sharer`. Set by the
    /// pipeline from the files it indexes, and at engine start from the
    /// decision the store records, so every reader of the registry routes a
    /// file as the graph was built.
    sharer_reads: AtomicBool,
}

/// The shared extensions a repository gives to the plugin that shares them
/// (S-582, [FR-PL-07]) — `{"h"}` in a C repository, empty otherwise. The
/// decision [`LanguageRegistry::shared_owners_for`] makes, and the value the
/// store records so a later process routes `.h` as the graph was built.
///
/// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedOwners(BTreeSet<String>);

impl SharedOwners {
    /// The decision as the store records it: the extensions, sorted and
    /// comma-joined (`"h"`); empty when no extension goes to its sharer.
    pub fn record(&self) -> String {
        self.0.iter().map(String::as_str).collect::<Vec<_>>().join(",")
    }

    /// The decision a [`record`](Self::record) holds; an empty record is the
    /// default, every shared extension with its outright claimant.
    pub fn from_record(record: &str) -> Self {
        Self(record.split(',').map(str::trim).filter(|e| !e.is_empty()).map(normalize_ext).collect())
    }

    /// Whether the decision gives no extension to its sharer — the default.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the decision gives `ext` (normalised) to its sharer.
    pub fn contains(&self, ext: &str) -> bool {
        self.0.contains(&normalize_ext(ext))
    }
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
        let mut shared: Vec<SharedExtension> = Vec::new();
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
            for shadowing in claim_extensions(&plugins, &plugin, idx, &mut by_extension, &mut shared) {
                warn(&shadowing);
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
            shared,
            skipped,
        })
    }

    /// The plugin that claims `ext` (with or without a leading dot, any case).
    /// A shared extension (S-582: `.h`) is read by the plugin this
    /// repository's decision gives it to ([`set_shared_owners`](Self::set_shared_owners)).
    pub fn for_extension(&self, ext: &str) -> Option<&dyn LanguagePlugin> {
        self.reader_of(&normalize_ext(ext))
            .map(|i| &self.plugins[i] as &dyn LanguagePlugin)
    }

    /// The index of the plugin that reads files of the normalised `ext`: a
    /// shared extension's sharer when this repository gives it the extension,
    /// else the extension's outright claimant.
    fn reader_of(&self, ext: &str) -> Option<usize> {
        match self.shared.iter().find(|s| s.ext == ext) {
            Some(s) if s.sharer_reads.load(Ordering::Relaxed) => Some(s.sharer),
            _ => self.by_extension.get(ext).copied(),
        }
    }

    /// The normalised extensions of `plugin` that it reads in this repository
    /// (S-582): every one it claims, less a shared one the other claimant
    /// reads. The per-key extension sets and maps below are built from these,
    /// so a `.h` carries the semantics of the language that parses it.
    fn read_extensions<'a>(&'a self, plugin: &'a CompiledPlugin) -> impl Iterator<Item = String> + 'a {
        plugin
            .extensions()
            .iter()
            .map(|e| normalize_ext(e))
            .filter(move |e| self.reader_of(e).is_some_and(|i| std::ptr::eq(&self.plugins[i], plugin)))
    }

    /// Which shared extensions the repository whose admitted files are
    /// `paths` gives to the plugin that shares them (S-582, [FR-PL-07]): a
    /// shared extension goes to its sharer when the repository holds more
    /// files of the sharer's other extensions than of its outright
    /// claimant's. C reads `.h` in a repository whose `.c` files outnumber its
    /// `.cpp`/`.cc`/`.cxx`/`.hpp`/`.hh`/`.hxx` ones — redis's 475 C files beside
    /// 8 vendored C++ ones — and C++ reads it otherwise, a repository of
    /// headers alone included. The rule `status` disclosed C repositories by
    /// before C owned their headers (S-572). Shared files themselves are not
    /// counted, so the decision never depends on the one it makes.
    ///
    /// A pure reading of `paths`; [`set_shared_owners`](Self::set_shared_owners)
    /// applies it.
    ///
    /// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
    pub fn shared_owners_for<'p>(&self, paths: impl IntoIterator<Item = &'p str>) -> SharedOwners {
        if self.shared.is_empty() {
            return SharedOwners::default();
        }
        let mut sources = vec![0_usize; self.plugins.len()];
        for path in paths {
            let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()).map(normalize_ext) else {
                continue;
            };
            if self.shared.iter().any(|s| s.ext == ext) {
                continue;
            }
            if let Some(&i) = self.by_extension.get(&ext) {
                sources[i] += 1;
            }
        }
        SharedOwners(
            self.shared
                .iter()
                .filter(|s| sources[s.sharer] > sources[s.owner])
                .map(|s| s.ext.clone())
                .collect(),
        )
    }

    /// The decision this registry routes shared extensions by now.
    pub fn shared_owners(&self) -> SharedOwners {
        SharedOwners(
            self.shared
                .iter()
                .filter(|s| s.sharer_reads.load(Ordering::Relaxed))
                .map(|s| s.ext.clone())
                .collect(),
        )
    }

    /// Route shared extensions by `owners` from now on (S-582): each one it
    /// names to its sharer, every other to its outright claimant. Returns
    /// whether the routing changed.
    pub fn set_shared_owners(&self, owners: &SharedOwners) -> bool {
        let mut changed = false;
        for s in &self.shared {
            let reads = owners.0.contains(&s.ext);
            changed |= s.sharer_reads.swap(reads, Ordering::Relaxed) != reads;
        }
        changed
    }

    /// Whether `rel` has an extension two plugins share (S-582) — a file the
    /// repository's decision routes, so one a change of decision re-reads.
    pub fn is_shared_path(&self, rel: &str) -> bool {
        Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .map(normalize_ext)
            .is_some_and(|ext| self.shared.iter().any(|s| s.ext == ext))
    }

    /// The plugin that claims `rel` by its **extension or basename** (S-062,
    /// [CR-010], [FR-CG-01], [FR-IX-02] as modified).
    ///
    /// Resolution order, extension first so a code/doc file is unaffected:
    /// 1. the file's extension (`for_extension`);
    /// 2. else the **exact** basename among the `filenames` claims (so
    ///    `Dockerfile` binds the Dockerfile plugin);
    /// 3. else the documented **`Name.*` prefix** rule — a basename `Name.<rest>`
    ///    matches a claim `Name` of an **artifact** plugin (so `Dockerfile.dev`
    ///    binds the same plugin). A code plugin's claim is exact only: Ruby's
    ///    `Gemfile` does not take `Gemfile.lock` (S-564).
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
        // Exact basename claim first, then the `Name.*` prefix rule — which only
        // an artifact plugin's claim takes (`Dockerfile.dev`): a code plugin's
        // claim is its exact name, so `Gemfile.lock` is not Ruby (S-564).
        let idx = self
            .filename_claims
            .iter()
            .find(|(claim, _)| claim == base)
            .or_else(|| {
                self.filename_claims.iter().find(|(claim, i)| {
                    self.plugins[*i].is_artifact()
                        && base.len() > claim.len()
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
            .flat_map(|p| self.read_extensions(p))
            .collect()
    }

    /// Each normalised extension whose code plugin declares `entry_points`
    /// (S-530, [FR-AN-01]), mapped to those names: the functions the language's
    /// runtime calls itself (Go's `main` and `init`). The [annotation-engine]
    /// roots a top-level function of one of these names in a file of that
    /// extension; an extension absent from the map adds no root.
    ///
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    pub fn entry_point_names(&self) -> HashMap<String, Vec<String>> {
        self.code_extension_map(|s| (!s.entry_points.is_empty()).then(|| s.entry_points.clone()))
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
                self.read_extensions(p).map(move |e| (e, targets.clone()))
            })
            .collect()
    }

    /// The file extensions (normalised: lower-case, no leading dot) whose loaded
    /// plugin declares `bare_specifiers = "tsconfig"`
    /// ([`BareSpecifiers::Tsconfig`](super::BareSpecifiers::Tsconfig); S-531):
    /// the files whose bare specifiers bind through their nearest
    /// `tsconfig.json`/`jsconfig.json` and the workspace packages
    /// ([`crate::resolve`]'s `ts_config`). Every other extension's bare
    /// specifier names an external package.
    pub fn tsconfig_extensions(&self) -> std::collections::HashSet<String> {
        self.extensions_declaring(|s| s.bare_specifiers == super::BareSpecifiers::Tsconfig)
    }

    /// The file extensions (normalised) whose loaded plugin declares
    /// `bare_specifiers = "load-path"`
    /// ([`BareSpecifiers::LoadPath`](super::BareSpecifiers::LoadPath); S-566):
    /// the files whose bare specifiers bind against the repository's load
    /// roots — each gem's `require_paths` and the repository root
    /// ([`crate::resolve`]'s `load_path`).
    pub fn load_path_extensions(&self) -> std::collections::HashSet<String> {
        self.extensions_declaring(|s| s.bare_specifiers == super::BareSpecifiers::LoadPath)
    }

    /// The file extensions (normalised) whose loaded plugin declares
    /// `bare_specifiers = "include-roots"`
    /// ([`BareSpecifiers::IncludeRoots`](super::BareSpecifiers::IncludeRoots);
    /// S-579): the files whose `#include` binds against the including file's
    /// directory and the repository's include roots, and whose calls and type
    /// relations bind through what they include. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`].
    pub fn include_root_extensions(&self) -> std::collections::HashSet<String> {
        self.extensions_declaring(|s| s.bare_specifiers == super::BareSpecifiers::IncludeRoots)
    }

    /// The normalised extensions of every loaded plugin whose semantics
    /// `declares` holds for — the one walk the semantics-keyed extension sets
    /// share.
    fn extensions_declaring(&self, declares: impl Fn(&Semantics) -> bool) -> HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| declares(p.semantics()))
            .flat_map(|p| self.read_extensions(p))
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
        self.read_extension_map(|p| p.semantics().package_modules.as_ref().map(|pm| pm.source_roots.clone()))
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
        self.extensions_declaring(|s| s.module_model == super::ModuleModelKind::Namespace)
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
        self.read_extension_map(|p| PathModelDecl::of(p))
    }

    /// Each extension a loaded plugin reads, mapped to what `value` gives that
    /// plugin — every plugin, documentation and artifact ones included; the
    /// code-only twin is [`code_extension_map`](Self::code_extension_map).
    fn read_extension_map<V: Clone>(&self, value: impl Fn(&CompiledPlugin) -> Option<V>) -> HashMap<String, V> {
        self.plugins
            .iter()
            .filter_map(|p| Some((p, value(p)?)))
            .flat_map(|(p, v)| self.read_extensions(p).map(move |e| (e, v.clone())))
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
            .flat_map(|p| self.read_extensions(p))
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
            .flat_map(|(p, v)| self.read_extensions(p).map(move |e| (e, v.clone())))
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
    /// declares that a class inherits the bodies of the interface members it
    /// implements (S-609, [FR-RS-48]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set walks its `Extends` chain alone, as before.
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    pub fn interface_body_inheriting_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.inherits_interface_bodies)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares structural types (S-535, [FR-EX-17]): every type relation such
    /// a file records relates its declaration. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds type relations as before.
    ///
    /// [FR-EX-17]: ../../../docs/specs/requirements/FR-EX-17.md
    pub fn structural_type_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.structural_types)
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
    /// tells a type apart from a same-named one by its generic arity (S-602,
    /// [CR-199]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    pub fn type_arity_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.type_arity)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// names its classes and modules by lexical constants (S-565,
    /// [FR-RS-33]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
    pub fn lexical_constant_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.lexical_constants)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that the files of one directory declaring one `package` clause
    /// share one package scope (S-528, [FR-RS-17]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-17]: ../../../docs/specs/requirements/FR-RS-17.md
    pub fn directory_package_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.directory_packages)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a bare call no included declaration supplies binds the
    /// one non-`static` definition of its name among the language's files
    /// (S-584, [FR-RS-40]). A shared extension is one only where its sharer
    /// reads it: C's `.h` in a repository whose headers C reads. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-40]: ../../../docs/specs/requirements/FR-RS-40.md
    pub fn link_unit_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.link_unit)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a type and the same-named singleton beside it are one
    /// name (S-573, [FR-RS-36]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub fn companion_object_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.companion_objects)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a file's top-level functions are members of its declared
    /// namespace (S-573, [FR-RS-36]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub fn top_level_callable_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.top_level_callables)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a wildcard import outranks the namespace's other files
    /// (S-573, [FR-RS-36]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub fn wildcard_first_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.wildcards_outrank_package)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that a top-level function with a receiver type is callable on
    /// a value (S-560, [FR-RS-31]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn extension_function_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.extension_functions)
    }

    /// Extension → the file-facade suffix its code plugin declares (S-560,
    /// [FR-RS-31]), normalised as in
    /// [`package_source_roots`](Self::package_source_roots). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the map has no facade.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn file_facades(&self) -> HashMap<String, String> {
        self.code_extension_map(|s| s.file_facade.clone())
    }

    /// Extension → the name its code plugin gives a type's anonymous companion
    /// object (I4-INT, [FR-RS-31]), normalised as in
    /// [`package_source_roots`](Self::package_source_roots). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the map has no companions.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn companion_names(&self) -> HashMap<String, String> {
        self.code_extension_map(|s| s.companion_name.clone())
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares that an unqualified call its namespace does not supply falls
    /// back to the global function (S-551, [FR-RS-26]). Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds exactly as before.
    ///
    /// [FR-RS-26]: ../../../docs/specs/requirements/FR-RS-26.md
    pub fn global_function_fallback_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.global_function_fallback)
    }

    /// Every code plugin's file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)), each mapped to its
    /// plugin's name (S-560) — what tells two languages of one interop family
    /// apart where they do not name each other's declarations: a Kotlin file
    /// never calls a Scala top-level `def` by its package. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`].
    pub fn languages(&self) -> HashMap<String, String> {
        self.plugins
            .iter()
            .filter(|p| !p.is_documentation() && !p.is_artifact())
            .flat_map(|p| self.read_extensions(p).map(move |e| (e, p.name().to_string())))
            .collect()
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
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares the **member-access capability** (S-523, [CR-171], [ADR-39]),
    /// less the ones it carves out in `member_access_excluded_extensions`
    /// (Python's `.pyi` stubs): the metrics engine scores
    /// Cohesion only for a class defined in one of them ([FR-QM-11]), and a
    /// class of any other language is not applicable rather than scored from
    /// calls alone. The Cohesion twin of
    /// [`reachability_extensions`](Self::reachability_extensions).
    ///
    /// [CR-171]: ../../../docs/requests/CR-171-the-quality-signal-and-test-classification-stay-honest-on-a-thin-graph.md
    /// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
    /// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
    pub fn member_access_extensions(&self) -> HashSet<String> {
        // Each plugin's carve-out applies to its own extensions only.
        self.plugins
            .iter()
            .filter(|p| !p.is_documentation() && !p.is_artifact() && p.semantics().member_access)
            .flat_map(|p| {
                let excluded: HashSet<String> =
                    p.semantics().member_access_excluded_extensions.iter().map(|e| normalize_ext(e)).collect();
                self.read_extensions(p).filter(move |ext| !excluded.contains(ext))
            })
            .collect()
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares `impl_block_lookup = true` (S-607, [FR-RS-47]): a call to a
    /// type's method there binds through the one associated-item lookup over
    /// recorded `impl` blocks. Consumed through
    /// [`crate::resolve::package_key::PackageLayout`]; an extension absent from
    /// the set binds as before.
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    pub fn impl_block_lookup_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.impl_block_lookup)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) whose code plugin
    /// declares `method_set_implements = true` (S-529, [FR-RS-18]): a type
    /// declared there implements an interface declared there by its method set.
    /// Consumed by the resolution pass ([`crate::resolve`]); an extension absent
    /// from the set records no such relation.
    ///
    /// [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
    pub fn method_set_extensions(&self) -> HashSet<String> {
        self.code_extensions_where(|s| s.method_set_implements)
    }

    /// The file extensions (normalised as in
    /// [`package_source_roots`](Self::package_source_roots)) a code plugin
    /// declares in `arity_unchecked_extensions` (S-592, [FR-RS-43]): files of a
    /// language that enforces no arity — JavaScript — whose calls are never
    /// filtered by a parameter range.
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
        let mut shared: Vec<SharedExtension> = Vec::new();
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
            for shadowing in claim_extensions(&plugins, &plugin, idx, &mut by_extension, &mut shared) {
                tracing::warn!("{shadowing}");
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
                shared,
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
        CaptureChecks {
            namespace_model: manifest.module_model_kind() == super::ModuleModelKind::Namespace,
            falls_through: manifest.implicit_call_falls_through,
            interface_bodies: manifest.inherits_interface_bodies,
            type_arity: manifest.type_arity(),
        },
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

/// Record `plugin`'s claim (it will sit at `idx`) on each of its extensions,
/// the step both loaders share; returns a warning per extension it shadows.
///
/// An extension already claimed goes to the later plugin, and the shadowing
/// is warned rather than silent — unless exactly one of the two declares it
/// shared (S-582, C's `.h`): then the other keeps it as its default reader,
/// the pair is recorded in `shared`, and nothing is shadowed.
fn claim_extensions(
    plugins: &[CompiledPlugin],
    plugin: &CompiledPlugin,
    idx: usize,
    by_extension: &mut HashMap<String, usize>,
    shared: &mut Vec<SharedExtension>,
) -> Vec<String> {
    let shares = |p: &CompiledPlugin, ext: &str| p.shared_extensions().iter().any(|e| normalize_ext(e) == ext);
    let mut shadowed = Vec::new();
    for ext in plugin.extensions() {
        let key = normalize_ext(ext);
        let Some(&prev) = by_extension.get(&key) else {
            by_extension.insert(key, idx);
            continue;
        };
        let (sharer, owner) = match (shares(plugin, &key), shares(&plugins[prev], &key)) {
            (true, false) => (idx, prev),
            (false, true) => (prev, idx),
            _ => {
                by_extension.insert(key, idx);
                shadowed.push(format!(
                    "extension '{ext}' claimed by '{}' shadows '{}'",
                    plugin.name(),
                    plugins[prev].name()
                ));
                continue;
            }
        };
        by_extension.insert(key.clone(), owner);
        shared.push(SharedExtension {
            ext: key,
            sharer,
            owner,
            sharer_reads: AtomicBool::new(false),
        });
    }
    shadowed
}

/// Whether the file at `file_path` is in a language whose normalised extension
/// is in `exts` — one of the capability sets the registry builds
/// ([`LanguageRegistry::reachability_extensions`],
/// [`LanguageRegistry::member_access_extensions`]). The one membership test
/// every capability gate reads ([ADR-39]): a node with no file path, or a file
/// with no extension, is **not** in the set, so an unidentifiable language never
/// earns a capability it cannot be shown to declare ([NFR-CC-04]).
///
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn declares_by_extension(file_path: Option<&str>, exts: &HashSet<String>) -> bool {
    file_path
        .and_then(|p| Path::new(p).extension())
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| exts.contains(&ext))
}

/// The file extensions (normalised as in [`declares_by_extension`]) whose code
/// plugin declares `bodied_traits_are_containers` (S-575, [FR-QM-11]): a trait
/// defined in one of them, with a bodied method, is a container for Focus and
/// Cohesion. Read off the compiled-in descriptors once per process, as
/// [`code_language_names`](super::grammars::code_language_names) is — a
/// descriptor is never overridden on disk, so every registry of the process
/// declares the same set, and the metrics engine needs no registry to read it.
/// A descriptor that fails to parse contributes nothing here; it already fails
/// [`LanguageRegistry::load`] loudly.
///
/// [FR-QM-11]: ../../../docs/specs/requirements/FR-QM-11.md
pub(crate) fn bodied_trait_container_extensions() -> &'static HashSet<String> {
    static EXTENSIONS: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
    EXTENSIONS.get_or_init(|| {
        grammars::compiled()
            .iter()
            .filter_map(|entry| PluginManifest::parse(entry.manifest_label, entry.manifest_toml).ok())
            .filter(|m| !m.documentation && !m.artifact && m.bodied_traits_are_containers)
            .flat_map(|m| m.extensions.into_iter().map(|e| normalize_ext(&e)).collect::<Vec<_>>())
            .collect()
    })
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

    /// S-609: a language declaring `inherits_interface_bodies` must mark the
    /// interface members a type does not inherit — its `symbols` query
    /// captures `@item.uninherited` — or its compile is refused naming the
    /// descriptor, as the fall-through's missing supertype capture is: a
    /// `static` or `private` interface member would otherwise read as
    /// inherited (sprint 92 review).
    #[test]
    fn an_interface_body_language_without_the_uninherited_capture_fails_its_compile() {
        fn entry(source: &'static str) -> GrammarEntry {
            toy_grammar("toyib", "inherits_interface_bodies = true", "", "symbols", source)
        }
        let load = |source: &'static str| {
            let mut entries = grammars::compiled();
            entries.push(entry(source));
            LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
                .expect("an embedded query compiles on first use")
        };
        let err = load("(function_item name: (identifier) @symbol.function)")
            .compile_all_queries()
            .expect_err("no uninherited capture fails the compile")
            .to_string();
        assert!(err.contains("toyib/plugin.toml") && err.contains("@item.uninherited"), "{err}");
        load("(function_item name: (identifier) @symbol.function)\n(function_signature_item) @item.uninherited")
            .compile_all_queries()
            .expect("the capture satisfies the check");
    }

    /// S-602: a language declaring `type_arity` must record its type
    /// declarations' type-parameter lists — its `symbols` query captures
    /// `@item.type_parameters` — or its compile is refused naming the
    /// descriptor: every generic type would otherwise read as a plain one.
    #[test]
    fn a_type_arity_language_without_the_type_parameters_capture_fails_its_compile() {
        fn entry(source: &'static str) -> GrammarEntry {
            let model = "[module_model]\nkind = \"namespace\"\ntype_arity = true\n";
            toy_grammar("toyta", "", model, "symbols", source)
        }
        let load = |source: &'static str| {
            let mut entries = grammars::compiled();
            entries.push(entry(source));
            LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
                .expect("an embedded query compiles on first use")
        };
        let err = load("(mod_item name: (identifier) @module.namespace)\n(struct_item name: (type_identifier) @symbol.struct)")
            .compile_all_queries()
            .expect_err("no type-parameters capture fails the compile")
            .to_string();
        assert!(err.contains("toyta/plugin.toml") && err.contains("@item.type_parameters"), "{err}");
        load(
            "(mod_item name: (identifier) @module.namespace)\n(struct_item name: (type_identifier) @symbol.struct)\n\
             (struct_item type_parameters: (type_parameters) @item.type_parameters)",
        )
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

    /// `.h` is shared, not shadowed (S-582): C declares it in
    /// `shared_extensions`, so loading C and C++ warns of nothing and C++ reads
    /// it until a repository's decision gives it to C — and then every
    /// per-key extension set reads `.h` with C's semantics.
    #[cfg(all(feature = "lang-c", feature = "lang-cpp"))]
    #[test]
    fn a_shared_extension_is_routed_by_the_repository_decision() {
        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(&grammars::compiled(), AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("embedded grammars load");
        assert!(warnings.iter().all(|w| !w.contains("'h'")), "{warnings:?}");
        assert_eq!(reg.for_path("src/a.h").map(|p| p.name()), Some("cpp"));
        assert!(reg.include_root_extensions().contains("h"), "C++ semantics while C++ reads it");

        let c_repo = reg.shared_owners_for(["src/a.c", "src/b.c", "src/a.h", "x.cpp", "README.md"]);
        assert_eq!(c_repo.record(), "h");
        assert!(reg.set_shared_owners(&c_repo), "the routing moved");
        assert!(!reg.set_shared_owners(&c_repo), "and moves once");
        assert_eq!(reg.for_path("src/a.h").map(|p| p.name()), Some("c"));
        assert_eq!(reg.for_path("src/A.H").map(|p| p.name()), Some("c"));
        assert_eq!(reg.for_path("src/a.hpp").map(|p| p.name()), Some("cpp"), "only the shared extension moves");
        assert_eq!(reg.languages().get("h").map(String::as_str), Some("c"));
        assert!(reg.include_root_extensions().contains("h"), "C reads its includes against the roots too (S-584)");
        // Every per-key set and map reads `.h` as C's, not C++'s.
        assert_eq!(reg.families().get("h"), reg.families().get("c"));
        assert_eq!(reg.call_targets().get("h"), reg.call_targets().get("c"));
        assert!(!reg.member_access_extensions().contains("h"), "C declares no member access");
        assert!(reg.specifier_target_extensions().contains_key("h"), "C's includes are paths, as C++'s are (S-584)");
        assert!(!reg.overloaded_call_extensions().contains("h"), "C does not overload");
        assert_eq!(reg.shared_owners(), c_repo);

        // A tie, a `.hh` source (the near miss: a header extension that is C++'s
        // own), and headers alone all leave `.h` with C++.
        for paths in [vec!["a.c", "b.cpp"], vec!["a.c", "s.hh"], vec!["a.h", "b.h"]] {
            let owners = reg.shared_owners_for(paths.iter().copied());
            assert!(owners.is_empty(), "{paths:?}: {owners:?}");
        }
        reg.set_shared_owners(&SharedOwners::default());
        assert_eq!(reg.for_path("src/a.h").map(|p| p.name()), Some("cpp"));
    }

    /// A sharer that loads after the plugin it shares with (S-582): the other
    /// arm of `claim_extensions` — the compiled grammars load C before C++, so
    /// only the shipped order exercises the first. No shadowing is warned, the
    /// earlier outright claimant reads by default, and the sharer once a
    /// decision gives it the extension.
    #[test]
    fn a_sharer_loaded_after_its_owner_shares_rather_than_shadows() {
        const SHARER: &str = r#"
            name = "rustshare"
            extensions = ["rs"]
            shared_extensions = ["rs"]
            module_separator = "::"
            abi_version = 15
            capabilities = ["symbols"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let mut entries = grammars::compiled();
        entries.push(GrammarEntry {
            manifest_label: "rustshare/plugin.toml",
            manifest_toml: SHARER,
            ..collide_entry()
        });
        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| warnings.push(w.to_string()))
            .expect("a sharing grammar loads");
        assert!(warnings.iter().all(|w| !w.contains("shadow")), "{warnings:?}");
        assert_eq!(reg.for_extension("rs").map(|p| p.name()), Some("rust"));
        reg.set_shared_owners(&SharedOwners::from_record("rs"));
        assert_eq!(reg.for_extension("rs").map(|p| p.name()), Some("rustshare"));
    }

    /// The recorded decision reads back as the decision (S-582).
    #[test]
    fn shared_owners_round_trip_their_record() {
        let owners = SharedOwners::from_record("h");
        assert!(owners.contains(".H") && !owners.is_empty());
        assert_eq!(SharedOwners::from_record(&owners.record()), owners);
        assert!(SharedOwners::from_record("").is_empty());
        assert_eq!(SharedOwners::from_record(" h ,, hh").record(), "h,hh");
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

    /// S-530: Go — and no other shipped grammar — names the functions its
    /// runtime calls itself, and Go still does not declare the reachability
    /// capability: its measured sample of dead verdicts held 30 live of 30.
    #[cfg(feature = "lang-go")]
    #[test]
    fn go_names_its_runtime_entry_points_and_declares_no_reachability() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let names = reg.entry_point_names();
        assert_eq!(names.keys().collect::<Vec<_>>(), ["go"], "{names:?}");
        assert_eq!(names["go"], ["main", "init"]);
        assert!(!reg.reachability_extensions().contains("go"), "Go's dead verdicts stay NULL");
    }

    /// S-523 / CR-171: exactly the declaring plugins' extensions carry the
    /// member-access capability Cohesion is gated on — Java, TypeScript, TSX,
    /// PHP, (S-556, FR-EX-22) C#, (S-543, FR-EX-20) Python and (S-575, FR-EX-28)
    /// Scala, (S-562, FR-EX-25) Kotlin, (S-580, FR-EX-22) C++ and (S-569,
    /// FR-EX-27) Ruby and (S-542, FR-EX-19) JavaScript (`js`, `mjs`, `cjs`,
    /// `jsx`) — less the `.pyi` stubs Python carves out; no other code
    /// language, Rust included.
    #[cfg(all(
        feature = "lang-java",
        feature = "lang-typescript",
        feature = "lang-php",
        feature = "lang-c-sharp",
        feature = "lang-python"
    ))]
    #[test]
    fn member_access_extensions_are_exactly_the_declaring_languages() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let mut exts: Vec<String> = reg.member_access_extensions().into_iter().collect();
        exts.sort();
        let scala = cfg!(feature = "lang-scala");
        let kotlin = cfg!(feature = "lang-kotlin");
        let mut want = vec!["cjs", "cs", "java", "js", "jsx", "mjs", "php", "py", "ts", "tsx"];
        if scala {
            want.extend(["sc", "scala"]);
        }
        if kotlin {
            want.extend(["kt", "kts"]);
        }
        let cpp = cfg!(feature = "lang-cpp");
        if cpp {
            want.extend(["cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"]);
        }
        let ruby = cfg!(feature = "lang-ruby");
        if ruby {
            want.extend(["gemspec", "rake", "rb", "ru"]);
        }
        want.sort_unstable();
        assert_eq!(exts, want);
        for (path, declared) in [
            (Some("src/Account.java"), true),
            (Some("src/Account.cs"), true),
            (Some("src/Account.scala"), scala),
            (Some("src/Account.kt"), kotlin),
            (Some("web/App.TSX"), true),
            (Some("app/javascript/user.js"), true),
            (Some("web/Widget.jsx"), true),
            (Some("lib/index.mjs"), true),
            (Some("hc/api/models.py"), true),
            (Some("src/werkzeug/datastructures/structures.pyi"), false),
            (Some("lib/model.rb"), ruby),
            (Some("src/widget.cpp"), cpp),
            (Some("include/widget.hpp"), cpp),
            (Some("src/lib.rs"), false),
            (Some("Makefile"), false),
            (None, false),
        ] {
            assert_eq!(
                declares_by_extension(path, &reg.member_access_extensions()),
                declared,
                "{path:?}"
            );
        }
    }

    /// S-575: exactly Scala's extensions make a bodied trait a container — not
    /// Rust's, whose trait default methods are no state-bearing container, nor
    /// PHP's.
    #[cfg(all(feature = "lang-scala", feature = "lang-php"))]
    #[test]
    fn bodied_trait_container_extensions_are_exactly_scalas() {
        let mut exts: Vec<&str> = bodied_trait_container_extensions().iter().map(String::as_str).collect();
        exts.sort_unstable();
        assert_eq!(exts, ["sc", "scala"]);
        assert!(declares_by_extension(Some("src/Svc.SCALA"), bodied_trait_container_extensions()));
        assert!(!declares_by_extension(Some("src/lib.rs"), bodied_trait_container_extensions()));
        assert!(!declares_by_extension(Some("src/Svc.php"), bodied_trait_container_extensions()));
    }

    /// The path-grammar extension map covers exactly the grammars that declare
    /// `import_specifier = "path"` (S-439): both TypeScript grammars and
    /// JavaScript (S-538), Go and Ruby (S-566), and
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
            // A TS/JS relative specifier resolves within the TS family only —
            // JavaScript's own plugin (S-538) declares the family's list.
            assert!(targets.contains("ts") && targets.contains("tsx"), "{ext}: {targets:?}");
            assert!(!targets.contains("py") && !targets.contains("go"), "{ext}: {targets:?}");
        }
        #[cfg(feature = "lang-go")]
        assert_eq!(
            exts.get("go").map(|t| t.len()),
            Some(0),
            "Go import paths are paths, and name no file extension"
        );
        // A Ruby `require` string is a path (S-566), resolving to `.rb` files only.
        #[cfg(feature = "lang-ruby")]
        assert_eq!(
            exts.get("rb").map(|t| t.iter().cloned().collect::<Vec<_>>()),
            Some(vec!["rb".to_string()])
        );
        for ext in ["rs", "py", "java", "kt", "cs", "php", "scala"] {
            assert!(!exts.contains_key(ext), "`{ext}` specifiers are names");
        }
    }

    /// Both TypeScript grammars and JavaScript (S-538), and only they, bind
    /// bare specifiers through `tsconfig` (S-531): Go's bare import paths take
    /// its `go.mod`.
    #[test]
    fn tsconfig_extensions_collect_only_the_typescript_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let exts = reg.tsconfig_extensions();
        #[cfg(feature = "lang-typescript")]
        for ext in ["ts", "js", "mjs", "cjs", "tsx", "jsx"] {
            assert!(exts.contains(ext), "`{ext}` binds bare specifiers through tsconfig");
        }
        for ext in ["go", "rs", "py", "java", "kt", "cs", "php", "rb", "scala"] {
            assert!(!exts.contains(ext), "`{ext}` does not");
        }
    }

    /// Ruby, and only Ruby, binds bare specifiers against its load roots
    /// (S-566): a `require 'a/b'` is a path under a gem's `require_paths` —
    /// from a `.rb` file, and from the `.rake`, `.ru` and `.gemspec` files the
    /// plugin also claims (S-564).
    #[test]
    fn load_path_extensions_collect_only_ruby() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let exts = reg.load_path_extensions();
        #[cfg(feature = "lang-ruby")]
        assert_eq!(exts, ["rb", "rake", "ru", "gemspec"].map(String::from).into_iter().collect::<HashSet<_>>());
        for ext in ["go", "rs", "py", "java", "kt", "cs", "php", "scala", "ts", "js"] {
            assert!(!exts.contains(ext), "`{ext}` does not");
        }
    }

    /// C++ (S-579) and C (S-584), and only they, bind their includes against
    /// include roots: an `#include <a/b.h>` is a path under an `include/` or
    /// `src/` directory.
    #[test]
    fn include_root_extensions_collect_only_cpp_and_c() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let exts = reg.include_root_extensions();
        #[cfg(feature = "lang-cpp")]
        for ext in ["cpp", "cc", "cxx", "hpp", "hh", "hxx", "h"] {
            assert!(exts.contains(ext), "`{ext}` binds its includes");
        }
        #[cfg(feature = "lang-c")]
        assert!(exts.contains("c"), "`c` binds its includes");
        for ext in ["go", "rs", "py", "java", "kt", "cs", "php", "rb", "scala", "ts", "js"] {
            assert!(!exts.contains(ext), "`{ext}` does not");
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
    /// and an interface in one list, and Scala's `extends A with B` may head
    /// with a trait (S-575), so their every extension declares it. Every other
    /// grammar — Java, PHP and Python above all, whose clauses say which is
    /// which — is absent.
    #[test]
    fn supertype_kind_follows_target_collects_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.supertype_kind_follows_target();
        #[cfg(all(feature = "lang-c-sharp", feature = "lang-kotlin"))]
        for ext in ["cs", "kt", "kts"] {
            assert!(declaring.contains(ext), "`{ext}` follows the bound supertype's kind");
        }
        #[cfg(feature = "lang-scala")]
        for ext in ["scala", "sc"] {
            assert!(declaring.contains(ext), "`{ext}` follows the bound supertype's kind");
        }
        for ext in ["rs", "java", "php", "py", "ts", "go", "rb", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` spells each supertype's kind");
        }
    }

    /// The structural-types key (S-535, FR-EX-17): TypeScript's two grammars
    /// and JavaScript (S-538) declare it, for every extension each parses;
    /// every other grammar —
    /// Java and C# above all, whose types are nominal — is absent.
    #[test]
    fn structural_type_extensions_collect_only_the_typescript_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.structural_type_extensions();
        #[cfg(feature = "lang-typescript")]
        for ext in ["ts", "js", "mjs", "cjs", "tsx", "jsx"] {
            assert!(declaring.contains(ext), "`{ext}` declares structural types");
        }
        for ext in ["rs", "java", "php", "py", "scala", "cs", "kt", "go", "rb", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` binds type relations as before");
        }
    }

    /// The interface-body key (S-609, FR-RS-48): Java and Kotlin classes
    /// inherit an implemented interface's member bodies, so their every
    /// extension declares it. C#'s default interface member is reachable only
    /// through an interface-typed receiver, and Scala and PHP are unchanged:
    /// every other grammar is absent.
    #[test]
    fn interface_body_inheritance_collects_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.interface_body_inheriting_extensions();
        #[cfg(all(feature = "lang-java", feature = "lang-kotlin"))]
        for ext in ["java", "kt", "kts"] {
            assert!(declaring.contains(ext), "`{ext}` inherits interface member bodies");
        }
        for ext in ["rs", "cs", "php", "py", "scala", "ts", "go", "rb", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` walks its `Extends` chain alone");
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

    /// The lexical-constants key (S-565, FR-RS-33): Ruby alone declares it.
    #[test]
    fn lexical_constant_extensions_collect_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.lexical_constant_extensions();
        #[cfg(feature = "lang-ruby")]
        assert!(declaring.contains("rb"), "`rb` names its classes by lexical constants");
        for ext in ["rs", "java", "kt", "kts", "php", "py", "scala", "ts", "go", "cs", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` binds exactly as before");
        }
    }

    /// The directory-packages key (S-528, FR-RS-17): Go alone declares it.
    #[test]
    fn directory_package_extensions_collect_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.directory_package_extensions();
        #[cfg(feature = "lang-go")]
        assert!(declaring.contains("go"), "`go` scopes a package by directory and clause");
        for ext in ["rs", "java", "kt", "kts", "php", "py", "scala", "ts", "rb", "cs", "c", "cpp", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` binds exactly as before");
        }
    }

    /// The link-unit key (S-584, FR-RS-40): C alone declares it, and its
    /// shared `.h`, which C++ reads by default, is not one by default.
    #[test]
    fn link_unit_extensions_collect_only_the_declaring_grammars() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let declaring = reg.link_unit_extensions();
        #[cfg(feature = "lang-c")]
        assert!(declaring.contains("c"), "`c` binds a bare call across its link unit");
        for ext in ["h", "cpp", "hpp", "rs", "java", "kt", "php", "py", "scala", "ts", "rb", "go", "cs", "md"] {
            assert!(!declaring.contains(ext), "`{ext}` binds exactly as before");
        }
    }

    /// The free-only bare-call key (S-590, FR-RS-07): Go, Rust, Python, PHP,
    /// TypeScript, TSX and JavaScript (S-538) declare `implicit_receiver = "none"`
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
    /// claimed by its own plugin since S-538 — are never filtered, while
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
        let lookup = reg.impl_block_lookup_extensions();
        assert_eq!(lookup, HashSet::from(["rs".to_string()]), "Rust alone records impl blocks");
        let method_set = reg.method_set_extensions();
        assert_eq!(method_set, HashSet::from(["go".to_string()]), "Go alone implements by method set");
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

    /// A code plugin's basename claim is exact (S-564): Ruby's `Gemfile` and
    /// `Rakefile` bind, and the `Name.*` variants an artifact's claim takes
    /// (`Dockerfile.dev`) do not extend to them — `Gemfile.lock` is not Ruby.
    #[cfg(all(feature = "lang-ruby", feature = "lang-dockerfile"))]
    #[test]
    fn a_code_plugins_basename_claim_is_exact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        for path in ["Gemfile", "sub/Rakefile", "tasks/x.rake", "config.ru"] {
            assert_eq!(reg.for_path(path).map(|p| p.name()), Some("ruby"), "{path}");
        }
        for path in ["Gemfile.lock", "Gemfile.dev", "gemfile", "Rakefileish"] {
            assert!(reg.for_path(path).is_none_or(|p| p.name() != "ruby"), "{path}");
        }
        assert_eq!(reg.for_path("Dockerfile.dev").map(|p| p.name()), Some("dockerfile"));
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

    /// JavaScript is its own plugin (S-538, FR-PL-08): it claims `.js`, `.mjs`,
    /// `.cjs` and `.jsx`, and the TypeScript grammars gave them up rather than
    /// being shadowed — a collision would let the later row win with only a
    /// warning, so the shipped table must raise none.
    #[cfg(feature = "lang-typescript")]
    #[test]
    fn javascript_claims_its_extensions_and_no_shipped_claim_is_shadowed() {
        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(&grammars::compiled(), AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .unwrap();
        assert!(
            !warnings.iter().any(|w| w.contains("shadows")),
            "no shipped extension claim is shadowed: {warnings:?}"
        );
        for ext in ["js", "mjs", "cjs", "jsx", "JS"] {
            assert_eq!(reg.for_extension(ext).map(|p| p.name()), Some("javascript"), "{ext}");
        }
        assert_eq!(reg.for_extension("ts").map(|p| p.name()), Some("typescript"));
        assert_eq!(reg.for_extension("tsx").map(|p| p.name()), Some("tsx"));
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
