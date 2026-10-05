//! The **package-shaped** module key ([CR-149], [FR-RS-01]) — the one place a
//! package is derived from a file path.
//!
//! The binder's default module model is the **path** model
//! ([`PackageLayout::module_key`]): the directory before the last `src/` names
//! the crate, every later directory is a module. A language whose plugin declares `[package_modules]`
//! ([`PackageModules`](crate::plugin::PackageModules)) is keyed instead by the
//! path *after* its source root, so `mailbox-core/src/main/java/com/x/Svc.java`
//! is `("mailbox_core", [com, x, Svc])` — the fully-qualified name its imports
//! spell — where the default model made it `[main, java, com, x, Svc]`.
//!
//! A language whose plugin declares the **declared-namespace** model
//! (`[module_model] kind = "namespace"`, S-518, [FR-RS-13]) is keyed by the
//! namespace or package each file *declares* instead — PHP's `namespace`, C#'s
//! file-scoped or block `namespace`, Kotlin's and Scala's `package` — so a
//! namespace that differs from its directory still names the file. Extraction
//! records that name ([`namespace_text`]); the layout is handed it per file
//! ([`PackageLayout::with_declared_namespaces`]) and keys the file
//! `(crate, namespace ++ [stem])`, its package being the namespace itself. A
//! file of such a language whose namespace is not known — its top-level
//! declarations sit in two different namespaces, or it was indexed before the
//! namespace was recorded — keeps the default model's key, as it had before.
//!
//! A language whose plugin declares **path-model data** (S-519, [FR-RS-14])
//! keeps the path model with two refinements, both descriptor data
//! ([`PathModelDecl`]): its **package-file stems** name their directory
//! (Rust's `mod`/`lib`/`main`, Python's `__init__` — a stem no plugin declares,
//! a JavaScript `main.js`, is the module `main`), and its **import roots** key
//! a file by its path under the root that holds it — `src/` when a package sits
//! beneath it, else the repository root ([`PackageLayout::with_detected_import_roots`],
//! replaced by `.logos/config.toml` through
//! [`PackageLayout::with_import_root_overrides`]) — under its family's own crate
//! ([`family_crate`]), so one language's module tree never reaches another's.
//!
//! **This is the single source of FQN derivation.** Anything that needs the
//! package a file declares, or the fully-qualified name of a type it declares,
//! asks [`PackageLayout::package_of`] / [`PackageLayout::type_fqn`] — never a
//! second split of the path (the sprint-81 risk register names the divergence a
//! re-derivation would cause), and never a second reading of a namespace.
//!
//! [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
//! [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::plugin::{CallTargets, LanguagePlugin, LanguageRegistry, ModuleModelKind, PathModelDecl};

/// A module identity: `(crate name, module path segments)` — the binder's
/// `ModKey`.
pub type ModuleKey = (String, Vec<String>);

/// Which file extensions are package-shaped, and under which source roots;
/// which take the namespace they declare, and what each such file declares.
///
/// Empty ([`Default`]) is the default module model for every file, which is
/// what a synthetic graph with no registry behind it is resolved with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageLayout {
    /// Normalised extension → its language's source roots, each split into path
    /// segments, in declaration order.
    roots_by_ext: HashMap<String, Vec<Vec<String>>>,
    /// Normalised extensions whose language declares the declared-namespace
    /// model (S-518).
    namespace_exts: HashSet<String>,
    /// Project-relative path → the namespace that file declares, as segments
    /// (empty for the global namespace) — only for a file of a
    /// [`namespace_exts`](Self::namespace_exts) language whose namespace was
    /// recorded.
    namespaces: HashMap<String, Vec<String>>,
    /// Normalised extension → its language's path-model data (S-519): the
    /// package-file stems, and the import roots when it declares them.
    path_models: HashMap<String, PathModel>,
    /// Normalised extension → the interop family its language binds within
    /// (S-519) — what the type and namespace indexes are partitioned by.
    families: HashMap<String, String>,
    /// Normalised extension → what a call of its language may bind besides a
    /// callable (S-521, [FR-RS-16]). An absent extension admits a callable only.
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    call_targets: HashMap<String, CallTargets>,
    /// Normalised extensions whose language leaves a supertype's kind unsaid,
    /// so its edge kind follows the type it binds (S-522, [FR-RS-15]).
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    kind_following_supertypes: HashSet<String>,
}

/// One language's path-model data in a layout (S-519).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathModel {
    /// File stems that name their directory.
    stems: Vec<String>,
    /// Import-root keying, when the language declares it.
    roots: Option<ImportRoots>,
}

/// An import-root language's roots (S-519, [FR-RS-14]).
///
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportRoots {
    /// The plugin's name — the key the config override is read under.
    language: String,
    /// The crate every file of the language is keyed under ([`family_crate`]).
    crate_name: String,
    /// The declared candidate roots, as path segments, in declaration order.
    candidates: Vec<Vec<String>>,
    /// The roots in force: the candidates that hold a package
    /// ([`PackageLayout::with_detected_import_roots`]), or the configured
    /// override. The repository root (no segments) is always the fallback.
    selected: Vec<Vec<String>>,
    /// Whether `selected` is the configured override, which detection never
    /// replaces.
    overridden: bool,
}

/// The crate an import-root language's files are keyed under: its family,
/// bracketed so that it can never equal a crate-name head an import spells
/// (`import python` names a module, not this crate). One crate per family, so
/// every root of the language shares one namespace — `sys.path` is one
/// namespace — and no other family's module is ever under it (S-519,
/// [NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn family_crate(family: &str) -> String {
    format!("<{family}>")
}

impl PackageLayout {
    /// A layout from `extension → source roots` (`"src/main/java"` form), the
    /// shape [`LanguageRegistry::package_source_roots`] returns.
    pub fn new(roots_by_ext: HashMap<String, Vec<String>>) -> Self {
        let roots_by_ext = roots_by_ext
            .into_iter()
            .map(|(ext, roots)| {
                let split = roots
                    .iter()
                    .map(|r| {
                        r.split('/')
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .collect();
                (ext.to_ascii_lowercase(), split)
            })
            .collect();
        Self {
            roots_by_ext,
            ..Self::default()
        }
    }

    /// The layout the loaded plugins declare: their package roots and their
    /// declared-namespace languages. It knows no file's namespace until it is
    /// given them ([`with_declared_namespaces`](Self::with_declared_namespaces)).
    pub fn from_registry(registry: &LanguageRegistry) -> Self {
        Self::new(registry.package_source_roots())
            .with_namespace_extensions(registry.namespace_extensions())
            .with_path_models(registry.path_models())
            .with_families(registry.families())
            .with_call_targets(registry.call_targets())
            .with_kind_following_supertypes(registry.supertype_kind_follows_target())
    }

    /// This layout, with each extension's path-model data (S-519; the shape
    /// [`LanguageRegistry::path_models`] returns). Import roots start at their
    /// declared candidates until [`with_detected_import_roots`](Self::with_detected_import_roots)
    /// or [`with_import_root_overrides`](Self::with_import_root_overrides)
    /// selects among them.
    pub fn with_path_models(mut self, models: HashMap<String, PathModelDecl>) -> Self {
        for (ext, decl) in models {
            let roots = decl.import_roots.as_ref().map(|candidates| {
                let candidates: Vec<Vec<String>> =
                    candidates.iter().map(|r| root_segments(r)).collect();
                ImportRoots {
                    language: decl.language.clone(),
                    crate_name: family_crate(&decl.family),
                    selected: candidates.clone(),
                    candidates,
                    overridden: false,
                }
            });
            self.path_models.insert(
                ext.trim_start_matches('.').to_ascii_lowercase(),
                PathModel {
                    stems: decl.package_stems,
                    roots,
                },
            );
        }
        self
    }

    /// This layout, with each extension's interop family (S-519; the shape
    /// [`LanguageRegistry::families`] returns).
    pub fn with_families(mut self, families: HashMap<String, String>) -> Self {
        self.families.extend(
            families
                .into_iter()
                .map(|(ext, family)| (ext.trim_start_matches('.').to_ascii_lowercase(), family)),
        );
        self
    }

    /// This layout, with each extension's call targets (S-521; the shape
    /// [`LanguageRegistry::call_targets`] returns).
    pub fn with_call_targets(mut self, targets: HashMap<String, CallTargets>) -> Self {
        self.call_targets.extend(
            targets
                .into_iter()
                .map(|(ext, t)| (ext.trim_start_matches('.').to_ascii_lowercase(), t)),
        );
        self
    }

    /// This layout, with the extensions whose supertype's edge kind follows
    /// its target (S-522; the set
    /// [`LanguageRegistry::supertype_kind_follows_target`] returns).
    pub fn with_kind_following_supertypes(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        self.kind_following_supertypes.extend(
            exts.into_iter()
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase()),
        );
        self
    }

    /// This layout, with the import roots `.logos/config.toml` declares per
    /// language (`[resolution.import_roots]`, S-519) in force for that
    /// language's files: they **replace** the detected ones — the repository
    /// root among them, unless listed (`"."`). A language that declares no import
    /// roots ignores its entry.
    pub fn with_import_root_overrides(mut self, overrides: &BTreeMap<String, Vec<String>>) -> Self {
        for roots in self.path_models.values_mut().filter_map(|m| m.roots.as_mut()) {
            if let Some(configured) = overrides.get(&roots.language) {
                roots.selected = configured.iter().map(|r| root_segments(r)).collect();
                roots.overridden = true;
            }
        }
        self
    }

    /// This layout, with each import-root language's roots detected from the
    /// files indexed (`paths`, project-relative): a candidate root is in force
    /// when it **holds a package** — a package-stem file of the language in a
    /// directory beneath it (`src/werkzeug/__init__.py` selects `src`). Read
    /// from the indexed paths rather than the disk, so a cold index and a sync
    /// over the same tree select the same roots ([NFR-RA-06]); a configured
    /// override is never replaced.
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn with_detected_import_roots<'a>(mut self, paths: impl IntoIterator<Item = &'a str>) -> Self {
        let mut held: HashMap<String, HashSet<usize>> = HashMap::new();
        for path in paths {
            let Some(ext) = extension(path) else { continue };
            let Some(model) = self.path_models.get(&ext) else { continue };
            let Some(roots) = model.roots.as_ref().filter(|r| !r.overridden) else {
                continue;
            };
            for (idx, candidate) in roots.candidates.iter().enumerate() {
                if holds_package(path, candidate, &model.stems) {
                    held.entry(ext.clone()).or_default().insert(idx);
                }
            }
        }
        for (ext, model) in &mut self.path_models {
            let Some(roots) = model.roots.as_mut().filter(|r| !r.overridden) else {
                continue;
            };
            let held = held.get(ext);
            roots.selected = roots
                .candidates
                .iter()
                .enumerate()
                .filter(|(idx, _)| held.is_some_and(|h| h.contains(idx)))
                .map(|(_, c)| c.clone())
                .collect();
        }
        self
    }

    /// Whether adding or removing the file at `path` can change which import
    /// roots [`with_detected_import_roots`](Self::with_detected_import_roots)
    /// selects: a package-stem file beneath a candidate root of a language whose
    /// roots are detected, not configured. A sync that changes one re-binds
    /// every row of that language's files (S-519, [NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn moves_import_roots(&self, path: &str) -> bool {
        let Some(model) = extension(path).and_then(|ext| self.path_models.get(&ext)) else {
            return false;
        };
        model.roots.as_ref().is_some_and(|roots| {
            !roots.overridden
                && roots
                    .candidates
                    .iter()
                    .any(|c| holds_package(path, c, &model.stems))
        })
    }

    /// Whether the file at `path` is keyed under import roots (S-519) — its
    /// relative imports keep their level and every directory above it under its
    /// root is a module.
    pub fn has_import_roots(&self, path: &str) -> bool {
        self.path_model(path).is_some_and(|m| m.roots.is_some())
    }

    /// The key of the package a relative import in the file at `path` starts
    /// from — `from . import x` names it (S-519, [FR-RS-14]): the directory the
    /// file sits in, which a package file (`__init__.py`) is itself the module
    /// of. `None` unless the file is keyed under import roots.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    pub fn package_dir(&self, path: &str) -> Option<ModuleKey> {
        let model = self.path_model(path)?;
        model.roots.as_ref()?;
        let (crate_name, mut mods) = self.module_key(path);
        if !self.is_package_file(path) {
            mods.pop();
        }
        Some((crate_name, mods))
    }

    /// The directory modules above the file at `path` (S-519, [FR-RS-14]):
    /// every directory between its import root and the file, as module keys,
    /// outermost first — modules whether or not a package file names them, so a
    /// namespace package still descends. Empty unless the file is keyed under
    /// import roots.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    pub fn directory_modules(&self, path: &str) -> Vec<ModuleKey> {
        let Some((crate_name, dir)) = self.package_dir(path) else {
            return Vec::new();
        };
        (1..=dir.len())
            .map(|len| (crate_name.clone(), dir[..len].to_vec()))
            .collect()
    }

    /// Whether the file at `path` is a package file of its language — its stem
    /// is one its plugin declares (`__init__.py`; `mod.rs`).
    pub fn is_package_file(&self, path: &str) -> bool {
        let Some(model) = self.path_model(path) else {
            return false;
        };
        path.rsplit('/')
            .find(|s| !s.is_empty())
            .is_some_and(|file| model.stems.contains(&file_stem(file)))
    }

    /// Whether `crate_name` is an import-root family's crate ([`family_crate`]):
    /// a closed namespace whose workspace fallbacks never reach another crate.
    pub fn is_family_crate(&self, crate_name: &str) -> bool {
        self.path_models
            .values()
            .filter_map(|m| m.roots.as_ref())
            .any(|r| r.crate_name == crate_name)
    }

    /// The interop family of the file at `path`'s language (S-519): the family
    /// its plugin declares, else the plugin's own name. A file of no loaded
    /// language (a synthetic graph's, under a layout told no families) is the
    /// family of its extension, so two extensions never share one by default.
    /// `None` only for a path with no extension.
    pub fn family(&self, path: &str) -> Option<String> {
        let ext = extension(path)?;
        Some(self.families.get(&ext).cloned().unwrap_or(ext))
    }

    /// What a call written in the file at `path` may bind besides a callable
    /// (S-521, [FR-RS-16]): its language's declaration, or none.
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    pub fn call_targets(&self, path: &str) -> CallTargets {
        extension(path)
            .and_then(|ext| self.call_targets.get(&ext).copied())
            .unwrap_or_default()
    }

    /// Whether a supertype written in the file at `path` binds whichever of a
    /// class, an interface or a trait it names, its edge kind following the
    /// target (S-522, [FR-RS-15]): its language's clause does not say.
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    pub fn supertype_kind_follows_target(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.kind_following_supertypes.contains(&ext))
    }

    fn path_model(&self, path: &str) -> Option<&PathModel> {
        self.path_models.get(&extension(path)?)
    }

    /// A registry-less layout folding the package-file stems the rust plugin
    /// declares (`mod`, `lib`, `main`) for `.rs` files — what a synthetic test
    /// graph of Rust paths is keyed with (`binder::Index::build`). Pinned
    /// against the loaded descriptor by
    /// `rust_tests::the_test_layout_folds_the_stems_the_rust_plugin_declares`,
    /// so the two cannot drift.
    #[cfg(test)]
    pub(crate) fn rust_stems_for_tests() -> Self {
        Self::default().with_path_models(HashMap::from([(
            "rs".to_string(),
            PathModelDecl {
                language: "rust".to_string(),
                family: "rust".to_string(),
                package_stems: ["mod", "lib", "main"].map(str::to_string).to_vec(),
                import_roots: None,
            },
        )]))
    }

    /// This layout, with the files of `exts` (extensions, with or without a
    /// leading dot) keyed by the namespace they declare.
    pub fn with_namespace_extensions(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        self.namespace_exts.extend(
            exts.into_iter()
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase()),
        );
        self
    }

    /// This layout, given the namespace each file declares — `(path, name)`
    /// pairs, the name in [`namespace_text`] form (`""` for the global
    /// namespace). A pair whose file is not of a declared-namespace language is
    /// ignored, so a stray row can never re-key a Rust or Java file.
    pub fn with_declared_namespaces(
        mut self,
        declared: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        for (path, name) in declared {
            if self.declares_namespaces(&path) {
                self.namespaces.insert(path, namespace_segments(&name));
            }
        }
        self
    }

    /// The layout one plugin declares — [`from_registry`](Self::from_registry)
    /// narrowed to a single language, for the single-file
    /// [`extract`](crate::extract::extract) entry point, which is handed a
    /// plugin and no registry. For every extension of that plugin it is the
    /// registry's layout exactly.
    pub fn from_plugin(plugin: &dyn LanguagePlugin) -> Self {
        let semantics = plugin.semantics();
        let exts = || {
            plugin
                .extensions()
                .iter()
                .map(|ext| ext.trim_start_matches('.').to_string())
        };
        let layout = match (semantics.module_model, semantics.package_modules.as_ref()) {
            (ModuleModelKind::Package, Some(pm)) => {
                Self::new(exts().map(|ext| (ext, pm.source_roots.clone())).collect())
            }
            (ModuleModelKind::Namespace, _) => Self::default().with_namespace_extensions(exts()),
            _ => Self::default(),
        };
        layout
            .with_path_models(
                PathModelDecl::of(plugin)
                    .into_iter()
                    .flat_map(|decl| exts().map(move |ext| (ext, decl.clone())))
                    .collect(),
            )
            .with_families(exts().map(|ext| (ext, semantics.family.clone())).collect())
            .with_call_targets(exts().map(|ext| (ext, semantics.call_targets)).collect())
            .with_kind_following_supertypes(
                exts().filter(|_| semantics.supertype_kind_follows_target),
            )
    }

    /// `true` when the file at `path` takes the package rungs: its language
    /// declares a package-shaped module path, or it is a declared-namespace
    /// file whose namespace is known.
    pub fn is_package_shaped(&self, path: &str) -> bool {
        self.roots_of(path).is_some() || self.namespaces.contains_key(path)
    }

    /// `true` when `path`'s language declares the declared-namespace model,
    /// whether or not this layout knows the namespace the file declares.
    pub fn declares_namespaces(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.namespace_exts.contains(&ext))
    }

    /// The module key of the file at the project-relative `path`.
    ///
    /// For a file of a package-shaped language under one of its source roots:
    /// the crate is the directory before the root (normalised `-` → `_`,
    /// `crate` when the root starts the path), and the modules are the
    /// directories after it plus the file stem. A file outside every root, and
    /// every file of any other language, keeps the path model's key — for a
    /// package-shaped language the file stem is always kept, because its plugin
    /// declares no package-file stem.
    ///
    /// The **rightmost** match of any root wins, as the default model takes the
    /// last `src/`, so a nested module's own root is the one that counts.
    ///
    /// A declared-namespace file whose namespace is known is keyed by it: the
    /// default model's crate, then the namespace, then the file stem —
    /// `src/Ordering/Order.cs` declaring `namespace Shop.Domain;` is
    /// `("crate", [Shop, Domain, Order])`.
    pub fn module_key(&self, path: &str) -> ModuleKey {
        if let Some(namespace) = self.namespaces.get(path) {
            let (crate_name, _, _) = default_layout(path);
            let mut mods = namespace.clone();
            if let Some(file) = path.rsplit('/').find(|s| !s.is_empty()) {
                mods.push(file_stem(file));
            }
            return (crate_name, mods);
        }
        let (crate_name, mut mods, stem) = match self.roots_of(path) {
            Some(roots) => rooted(path, roots).unwrap_or_else(|| default_layout(path)),
            None => return self.path_key(path),
        };
        if let Some(stem) = stem {
            mods.push(stem);
        }
        (crate_name, mods)
    }

    /// The package the file at `path` declares by its location — its module key
    /// without the file stem — or `None` when its language is not
    /// package-shaped. `src/main/java/com/x/Svc.java` → `[com, x]`. For a
    /// declared-namespace file it is the namespace the file declares, whatever
    /// its directory, and `None` when that is not known.
    pub fn package_of(&self, path: &str) -> Option<Vec<String>> {
        if let Some(namespace) = self.namespaces.get(path) {
            return Some(namespace.clone());
        }
        self.roots_of(path)?;
        let (_, mut mods) = self.module_key(path);
        mods.pop();
        Some(mods)
    }

    /// The fully-qualified name of a top-level type named `type_name` declared in
    /// the file at `path` — its [`package_of`](Self::package_of) plus the name —
    /// or `None` when the file's language is not package-shaped.
    pub fn type_fqn(&self, path: &str, type_name: &str) -> Option<Vec<String>> {
        let mut fqn = self.package_of(path)?;
        fqn.push(type_name.to_string());
        Some(fqn)
    }

    /// The source root the file at `path` sits under — the root whose match
    /// keyed it ([`module_key`](Self::module_key)), as its path segments — or
    /// `None` when its language is not package-shaped or no root precedes it.
    /// `svc/src/test/java/com/x/SvcTest.java` → `[src, test, java]`.
    pub fn source_root(&self, path: &str) -> Option<&[String]> {
        let roots = self.roots_of(path)?;
        let (_, _, idx) = matched_root(path, roots)?;
        Some(&roots[idx])
    }

    fn roots_of(&self, path: &str) -> Option<&[Vec<String>]> {
        self.roots_by_ext.get(&extension(path)?).map(Vec::as_slice)
    }

    /// The path model's key of `path` (S-519, [FR-RS-14]). Under import roots:
    /// the language's family crate, then the directories after the **longest**
    /// root in force that prefixes the path — the repository root when none
    /// does — then the stem. Otherwise the default model's `src/` crate rule.
    /// Either way the stem is dropped when it is one of the language's
    /// package-file stems, so the file names its directory; a stem no plugin
    /// declares (a JavaScript `main.js`) is kept.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    fn path_key(&self, path: &str) -> ModuleKey {
        let model = self.path_model(path);
        let (crate_name, mut mods, stem) = match model.and_then(|m| m.roots.as_ref()) {
            Some(roots) => under_import_root(path, roots),
            None => default_layout(path),
        };
        let folded = |s: &String| model.is_some_and(|m| m.stems.contains(s));
        if let Some(stem) = stem.filter(|s| !folded(s)) {
            mods.push(stem);
        }
        (crate_name, mods)
    }
}

/// An import root's segments from its declared or configured text: `"src"` →
/// `[src]`; `"."` or `""`, the repository root, → none.
fn root_segments(root: &str) -> Vec<String> {
    root.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(str::to_string)
        .collect()
}

/// Whether `path` is a package file of one of `stems` in a directory beneath
/// `root` — the evidence that `root` is an import root (`src/pkg/__init__.py`
/// for `src`). A package file directly in the root is not: it would make the
/// root itself a package.
fn holds_package(path: &str, root: &[String], stems: &[String]) -> bool {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some((file, dirs)) = segs.split_last() else {
        return false;
    };
    dirs.len() > root.len()
        && dirs.iter().zip(root).all(|(d, r)| *d == r)
        && stems.contains(&file_stem(file))
}

/// `(family crate, directories after the longest root in force prefixing the
/// path, stem)` — the repository root (no segments) when no root does. A root
/// is matched from the path's start, never in its middle: an import root is a
/// directory of the repository, where a package root (`src/main/java`) is
/// matched anywhere because a Maven module may sit at any depth.
fn under_import_root(path: &str, roots: &ImportRoots) -> (String, Vec<String>, Option<String>) {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some((file, dirs)) = segs.split_last() else {
        return (roots.crate_name.clone(), Vec::new(), None);
    };
    let skip = roots
        .selected
        .iter()
        .filter(|r| r.len() <= dirs.len() && dirs.iter().zip(r.iter()).all(|(d, s)| *d == s))
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    let mods = dirs[skip..].iter().map(|s| (*s).to_string()).collect();
    (roots.crate_name.clone(), mods, Some(file_stem(file)))
}

/// `path`'s extension, lower-cased — the key both models are declared under.
fn extension(path: &str) -> Option<String> {
    Some(Path::new(path).extension()?.to_str()?.to_ascii_lowercase())
}

/// A declared namespace's segments from its recorded text (`"Shop.Domain"` →
/// `[Shop, Domain]`; `""`, the global namespace, → none) — the inverse of
/// [`namespace_text`].
pub fn namespace_segments(text: &str) -> Vec<String> {
    text.split('.')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The recorded text of a declared namespace: its segments joined by `.`,
/// `""` for the global namespace. Every language's namespace is recorded in
/// this one form, whatever separator it is written with (PHP's `\`), so the
/// layout reads it back with one split ([`namespace_segments`]).
pub fn namespace_text(segments: &[String]) -> String {
    segments.join(".")
}

/// `(crate, package directories, stem)` of `path` under the rightmost match of
/// any of `roots`, or `None` when no root precedes the file name.
fn rooted(path: &str, roots: &[Vec<String>]) -> Option<(String, Vec<String>, Option<String>)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (file, dirs) = segs.split_last()?;
    let (start, len, _) = matched_root(path, roots)?;
    let crate_name = match start {
        0 => "crate".to_string(),
        i => normalize_crate(dirs[i - 1]),
    };
    let package = dirs[start + len..]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    Some((crate_name, package, Some(file_stem(file))))
}

/// `(start, length, root index)` of the rightmost match of any of `roots`
/// among `path`'s directories — the one match both [`rooted`] and
/// [`PackageLayout::source_root`] read, so the root that keys a file and the
/// root reported for it can never differ. Of two roots matching at one start,
/// the longer wins, then the later-declared.
fn matched_root(path: &str, roots: &[Vec<String>]) -> Option<(usize, usize, usize)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (_, dirs) = segs.split_last()?;
    roots
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_empty() && r.len() <= dirs.len())
        .flat_map(|(idx, r)| {
            (0..=dirs.len() - r.len())
                .filter(move |&i| dirs[i..i + r.len()].iter().zip(r).all(|(d, s)| *d == s))
                .map(move |i| (i, r.len(), idx))
        })
        .max()
}

/// The default model's parts of `path`: the crate, the module directories after
/// its last `src/`, and the file stem (`None` when nothing follows the crate
/// root) — before the package-stem fold [`PackageLayout::module_key`] applies
/// for the stems the file's plugin declares (Rust's `mod`/`lib`/`main`, S-519).
/// Shared by every model so they agree on every file outside a package root.
fn default_layout(path: &str) -> (String, Vec<String>, Option<String>) {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (crate_name, mods_start) = match segs.iter().rposition(|s| *s == "src") {
        Some(0) => ("crate".to_string(), 1),
        Some(pos) => (normalize_crate(segs[pos - 1]), pos + 1),
        None => ("crate".to_string(), 0),
    };
    let mut mods: Vec<String> = segs
        .get(mods_start..)
        .unwrap_or_default()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let stem = mods.pop().map(|last| file_stem(&last));
    (crate_name, mods, stem)
}

/// Normalise a crate directory name to its extern-path form (`-` → `_`) —
/// shared by both models, and by the binder's crate-name path heads.
pub(crate) fn normalize_crate(name: &str) -> String {
    name.replace('-', "_")
}

/// A file name without its extension (`Svc.java` → `Svc`; the name itself when
/// it has none) — the module stem both models append.
fn file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}


// The fixtures are Maven paths, so the layout is the one the loaded Java
// descriptor declares — read from the registry, never spelled here: this
// directory holds no language id (the `jvm_parity` guard).
#[cfg(all(test, feature = "lang-java"))]
mod tests {
    use super::*;

    fn java() -> PackageLayout {
        let tmp = tempfile::tempdir().expect("tempdir");
        PackageLayout::from_registry(&LanguageRegistry::load(tmp.path()).expect("registry loads"))
    }

    /// `(crate, modules)` from a `/`-joined module path (`""` for none).
    fn key(c: &str, mods: &str) -> ModuleKey {
        (
            c.to_string(),
            mods.split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    #[test]
    fn a_file_under_a_source_root_is_keyed_by_its_package() {
        let l = java();
        assert_eq!(
            l.module_key("src/main/java/com/x/Svc.java"),
            key("crate", "com/x/Svc")
        );
        assert_eq!(
            l.module_key("src/test/java/com/x/SvcTest.java"),
            key("crate", "com/x/SvcTest")
        );
        // A multi-module Maven repository keeps each module's crate.
        assert_eq!(
            l.module_key("mailbox-core/src/main/java/com/x/Svc.java"),
            key("mailbox_core", "com/x/Svc")
        );
        // The default package: no directory after the root.
        assert_eq!(
            l.module_key("src/main/java/Main.java"),
            key("crate", "Main")
        );
        assert_eq!(l.package_of("src/main/java/Main.java"), Some(vec![]));
    }

    #[test]
    fn a_package_that_contains_a_src_directory_keeps_its_module_crate() {
        // The default model takes the LAST `src/` as the crate boundary, which
        // would make `com` the crate here; the root match does not.
        assert_eq!(
            java().module_key("app/src/main/java/com/src/Foo.java"),
            key("app", "com/src/Foo")
        );
    }

    #[test]
    fn a_file_outside_every_root_keeps_the_default_key() {
        let l = java();
        // A flat layout and a `src/`-rooted one are already package-shaped.
        assert_eq!(l.module_key("com/x/Svc.java"), key("crate", "com/x/Svc"));
        assert_eq!(
            l.module_key("src/com/x/Svc.java"),
            key("crate", "com/x/Svc")
        );
        assert_eq!(
            l.package_of("src/com/x/Svc.java"),
            Some(vec!["com".into(), "x".into()])
        );
        // A root is matched as whole segments, never as a substring.
        assert_eq!(
            l.module_key("mysrc/main/java/com/Svc.java"),
            key("crate", "mysrc/main/java/com/Svc")
        );
    }

    #[test]
    fn every_other_language_keeps_the_default_key_byte_for_byte() {
        let l = java();
        for path in [
            "logos-core/src/extract/mod.rs",
            "src/main.rs",
            "src/lib.rs",
            "a/src/main/java/b.rs",
        ] {
            assert_eq!(l.module_key(path), rust_tests::legacy_rust_key(path), "{path}");
            assert_eq!(l.package_of(path), None, "{path}");
        }
        assert_eq!(
            l.module_key("logos-core/src/extract/mod.rs"),
            key("logos_core", "extract")
        );
        // …and an empty layout is the default model for Java too.
        let none = PackageLayout::default();
        assert_eq!(
            none.module_key("src/main/java/com/x/Svc.java"),
            key("crate", "main/java/com/x/Svc")
        );
        assert!(!none.is_package_shaped("src/main/java/com/x/Svc.java"));
    }

    #[test]
    fn a_type_fqn_is_its_files_package_plus_its_own_name() {
        let l = java();
        assert_eq!(
            l.type_fqn("svc/src/main/java/com/x/Svc.java", "Extra"),
            Some(vec!["com".into(), "x".into(), "Extra".into()])
        );
        assert_eq!(l.type_fqn("src/lib.rs", "Extra"), None);
    }
}

// The declared-namespace model (S-518) needs no grammar: a layout is told which
// extensions declare it and what each file declares. The extensions are spelt
// here without a JVM language id (the `jvm_parity` guard reads this file).
#[cfg(test)]
mod namespace_tests {
    use super::*;

    fn segs(path: &str) -> Vec<String> {
        namespace_segments(&path.replace('/', "."))
    }

    /// A layout keying `.cs` and `.php` files by their declared namespace, with
    /// `declared` recorded.
    fn layout(declared: &[(&str, &str)]) -> PackageLayout {
        PackageLayout::default()
            .with_namespace_extensions(["cs".to_string(), ".PHP".to_string()])
            .with_declared_namespaces(
                declared
                    .iter()
                    .map(|(p, n)| ((*p).to_string(), (*n).to_string())),
            )
    }

    #[test]
    fn a_declared_namespace_keys_the_file_whatever_its_directory() {
        let l = layout(&[("src/Ordering/Order.cs", "Shop.Domain")]);
        assert!(l.is_package_shaped("src/Ordering/Order.cs"));
        assert_eq!(
            l.module_key("src/Ordering/Order.cs"),
            ("crate".to_string(), segs("Shop/Domain/Order"))
        );
        assert_eq!(l.package_of("src/Ordering/Order.cs"), Some(segs("Shop/Domain")));
        assert_eq!(
            l.type_fqn("src/Ordering/Order.cs", "OrderItem"),
            Some(segs("Shop/Domain/OrderItem")),
            "a type is its file's namespace plus its own name"
        );
        assert_eq!(l.source_root("src/Ordering/Order.cs"), None);
        // The crate is the default model's: the directory before the last `src/`.
        let nested = layout(&[("Basket.API/src/Grpc/Svc.cs", "Basket")]);
        assert_eq!(
            nested.module_key("Basket.API/src/Grpc/Svc.cs"),
            ("Basket.API".to_string(), segs("Basket/Svc"))
        );
    }

    #[test]
    fn the_global_namespace_is_an_empty_package() {
        let l = layout(&[("lib/helpers.php", "")]);
        assert!(l.is_package_shaped("lib/helpers.php"));
        assert_eq!(l.package_of("lib/helpers.php"), Some(Vec::new()));
        assert_eq!(l.type_fqn("lib/helpers.php", "Util"), Some(segs("Util")));
    }

    #[test]
    fn a_namespace_file_whose_namespace_is_unknown_keeps_the_default_key() {
        let l = layout(&[]);
        assert!(l.declares_namespaces("src/Ordering/Order.cs"));
        assert!(!l.is_package_shaped("src/Ordering/Order.cs"));
        assert_eq!(
            l.module_key("src/Ordering/Order.cs"),
            PackageLayout::default().module_key("src/Ordering/Order.cs")
        );
        assert_eq!(l.package_of("src/Ordering/Order.cs"), None);
    }

    #[test]
    fn a_recorded_namespace_never_rekeys_another_languages_file() {
        // A stray row for a file of another model is ignored, so a Rust or a
        // package-model file keeps its key byte for byte.
        let l = layout(&[("logos-core/src/extract/mod.rs", "Shop"), ("src/lib.rs", "")]);
        for path in ["logos-core/src/extract/mod.rs", "src/lib.rs"] {
            assert!(!l.declares_namespaces(path), "{path}");
            assert!(!l.is_package_shaped(path), "{path}");
            assert_eq!(l.module_key(path), PackageLayout::default().module_key(path), "{path}");
            assert_eq!(l.package_of(path), None, "{path}");
        }
    }

    #[test]
    fn a_namespace_reads_back_from_the_one_recorded_form() {
        assert_eq!(namespace_text(&segs("Monolog/Handler")), "Monolog.Handler");
        assert_eq!(namespace_segments("Monolog.Handler"), segs("Monolog/Handler"));
        assert_eq!(namespace_text(&[]), "");
        assert!(namespace_segments("").is_empty());
    }
}

// Rust's package-file stems moved from a hard-coded fold into the rust plugin
// (S-519). Every Rust module key must be byte-identical across the move
// (ADR-07): the oracle below is the fold as it was, verbatim.
#[cfg(test)]
pub(crate) mod rust_tests {
    use super::*;

    /// The pre-S-519 `module_key_for_file`, verbatim — the oracle every Rust
    /// key is compared against. Not a production path: nothing outside tests
    /// may fold a stem the plugins do not declare.
    pub(crate) fn legacy_rust_key(path: &str) -> ModuleKey {
        let (crate_name, mut mods, stem) = default_layout(path);
        if let Some(stem) = stem.filter(|s| !matches!(s.as_str(), "mod" | "lib" | "main")) {
            mods.push(stem);
        }
        (crate_name, mods)
    }

    #[cfg(feature = "lang-rust")]
    fn registry_layout() -> PackageLayout {
        let tmp = tempfile::tempdir().expect("tempdir");
        PackageLayout::from_registry(&LanguageRegistry::load(tmp.path()).expect("registry loads"))
    }

    /// The synthetic-graph layout (`binder::Index::build`) folds exactly the
    /// stems the loaded rust plugin declares, and declares no import roots.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_test_layout_folds_the_stems_the_rust_plugin_declares() {
        let registry = registry_layout();
        let test = PackageLayout::rust_stems_for_tests();
        assert_eq!(registry.path_models.get("rs"), test.path_models.get("rs"));
        assert!(registry.path_models["rs"].roots.is_none());
    }

    /// Every `.rs` file of this repository keeps the module key the hard-coded
    /// fold gave it (S-519, ADR-07) — the serialized before/after comparison,
    /// run over the live tree so a new file is covered the day it lands.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn every_rust_module_key_on_this_repository_is_byte_identical() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let layout = registry_layout();
        let mut stack = vec![root.to_path_buf()];
        let mut compared = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    if !name.starts_with('.') && name != "target" && name != "node_modules" {
                        stack.push(path);
                    }
                    continue;
                }
                if !name.ends_with(".rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                assert_eq!(layout.module_key(&rel), legacy_rust_key(&rel), "{rel}");
                compared += 1;
            }
        }
        assert!(compared > 300, "the walk must reach the repository's Rust files: {compared}");
    }
}

// The path model's data (S-519, FR-RS-14), exercised without a grammar: a layout
// is told each extension's declaration, as the registry tells it. The language
// names are fixture data, not a roster in core.
#[cfg(test)]
mod path_model_tests {
    use super::*;

    fn py_decl(roots: &[&str]) -> PathModelDecl {
        PathModelDecl {
            language: "python".to_string(),
            family: "python".to_string(),
            package_stems: vec!["__init__".to_string()],
            import_roots: Some(roots.iter().map(|r| (*r).to_string()).collect()),
        }
    }

    /// A layout with a Python-shaped import-root model for `.py`, roots
    /// detected from `files`.
    fn py(files: &[&str]) -> PackageLayout {
        PackageLayout::default()
            .with_path_models(HashMap::from([("py".to_string(), py_decl(&["src"]))]))
            .with_detected_import_roots(files.iter().copied())
    }

    fn key(mods: &str) -> ModuleKey {
        (
            family_crate("python"),
            mods.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect(),
        )
    }

    const WERKZEUG: &[&str] = &[
        "src/werkzeug/__init__.py",
        "src/werkzeug/_internal.py",
        "src/werkzeug/routing/__init__.py",
        "src/werkzeug/routing/rules.py",
        "tests/test_routing.py",
    ];
    const HEALTHCHECKS: &[&str] = &[
        "hc/__init__.py",
        "hc/api/__init__.py",
        "hc/api/models.py",
        "manage.py",
    ];

    #[test]
    fn a_src_root_holding_a_package_is_detected_and_a_package_file_names_its_directory() {
        let l = py(WERKZEUG);
        assert_eq!(l.module_key("src/werkzeug/__init__.py"), key("werkzeug"));
        assert_eq!(l.module_key("src/werkzeug/routing/rules.py"), key("werkzeug/routing/rules"));
        assert_eq!(l.module_key("src/werkzeug/routing/__init__.py"), key("werkzeug/routing"));
        // A file outside every root is keyed from the repository root — and a
        // root is matched from the path's start, never in its middle.
        assert_eq!(l.module_key("tests/test_routing.py"), key("tests/test_routing"));
        assert_eq!(l.module_key("docs/src/conf.py"), key("docs/src/conf"));
        assert!(l.has_import_roots("src/werkzeug/_internal.py"));
        assert!(l.is_package_file("src/werkzeug/__init__.py"));
        assert!(!l.is_package_file("src/werkzeug/_internal.py"));
    }

    #[test]
    fn without_a_package_under_src_the_repository_root_is_the_import_root() {
        let l = py(HEALTHCHECKS);
        assert_eq!(l.module_key("hc/api/models.py"), key("hc/api/models"));
        assert_eq!(l.module_key("hc/__init__.py"), key("hc"));
        // A `src/` holding only a script is no package root.
        let script = py(&["src/tool.py", "app/__init__.py"]);
        assert_eq!(script.module_key("src/tool.py"), key("src/tool"));
        assert_eq!(script.module_key("app/__init__.py"), key("app"));
        // Nor is a package file directly in `src/`, nor a `src` deeper down.
        let flat = py(&["src/__init__.py", "docs/src/pkg/__init__.py"]);
        assert_eq!(flat.module_key("docs/src/pkg/__init__.py"), key("docs/src/pkg"));
        assert_eq!(flat.module_key("src/__init__.py"), key("src"));
    }

    #[test]
    fn the_configured_import_roots_replace_the_detected_ones() {
        let lib = PackageLayout::default()
            .with_path_models(HashMap::from([("py".to_string(), py_decl(&["src"]))]))
            .with_import_root_overrides(&BTreeMap::from([(
                "python".to_string(),
                vec!["lib".to_string()],
            )]))
            .with_detected_import_roots(WERKZEUG.iter().copied().chain(["lib/pkg/__init__.py"]));
        assert_eq!(lib.module_key("lib/pkg/__init__.py"), key("pkg"));
        // `src/` holds a package, but the override replaced the detection.
        assert_eq!(lib.module_key("src/werkzeug/_internal.py"), key("src/werkzeug/_internal"));
        assert!(!lib.moves_import_roots("src/other/__init__.py"));
        // Nested roots: the longest one prefixing a path keys it, with the
        // repository root (`.`) still in force for every other file.
        let nested = PackageLayout::default()
            .with_path_models(HashMap::from([("py".to_string(), py_decl(&["src"]))]))
            .with_import_root_overrides(&BTreeMap::from([(
                "python".to_string(),
                vec![".".to_string(), "lib".to_string()],
            )]));
        assert_eq!(nested.module_key("lib/pkg/x.py"), key("pkg/x"));
        assert_eq!(nested.module_key("tools/y.py"), key("tools/y"));
        // `.` is the repository root; another language's entry is ignored.
        let dot = PackageLayout::default()
            .with_path_models(HashMap::from([("py".to_string(), py_decl(&["src"]))]))
            .with_import_root_overrides(&BTreeMap::from([
                ("python".to_string(), vec![".".to_string()]),
                ("ruby".to_string(), vec!["lib".to_string()]),
            ]))
            .with_detected_import_roots(WERKZEUG.iter().copied());
        assert_eq!(dot.module_key("src/werkzeug/_internal.py"), key("src/werkzeug/_internal"));
    }

    #[test]
    fn a_relative_import_starts_from_the_package_the_file_sits_in() {
        let l = py(WERKZEUG);
        assert_eq!(l.package_dir("src/werkzeug/routing/rules.py"), Some(key("werkzeug/routing")));
        // A package file is the module of its own package.
        assert_eq!(l.package_dir("src/werkzeug/routing/__init__.py"), Some(key("werkzeug/routing")));
        assert_eq!(
            l.directory_modules("src/werkzeug/routing/rules.py"),
            vec![key("werkzeug"), key("werkzeug/routing")]
        );
        // Not an import-root file: no package, no directory modules.
        assert_eq!(l.package_dir("src/werkzeug/routing/rules.js"), None);
        assert!(l.directory_modules("src/lib.rs").is_empty());
    }

    #[test]
    fn only_a_package_file_beneath_a_detected_candidate_moves_the_roots() {
        let l = py(HEALTHCHECKS);
        assert!(l.moves_import_roots("src/pkg/__init__.py"));
        assert!(l.moves_import_roots("src/pkg/sub/__init__.py"));
        assert!(!l.moves_import_roots("src/pkg/module.py"));
        assert!(!l.moves_import_roots("hc/__init__.py"));
        assert!(!l.moves_import_roots("src/pkg/__init__.rs"));
    }

    #[test]
    fn a_stem_no_plugin_declares_is_kept_and_a_declared_one_folds() {
        let l = PackageLayout::rust_stems_for_tests();
        assert_eq!(l.module_key("web/src/main.js"), ("web".to_string(), vec!["main".to_string()]));
        assert_eq!(l.module_key("web/src/lib.ts"), ("web".to_string(), vec!["lib".to_string()]));
        assert_eq!(l.module_key("cli/src/main.rs"), ("cli".to_string(), vec![]));
        assert_eq!(
            l.module_key("cli/src/cmd/mod.rs"),
            ("cli".to_string(), vec!["cmd".to_string()])
        );
        // Rust declares no import roots: no family crate, no directory modules.
        assert!(!l.has_import_roots("cli/src/cmd/mod.rs"));
        assert!(!l.is_family_crate("cli"));
    }

    #[test]
    fn a_family_is_declared_per_extension_and_otherwise_the_extension_itself() {
        // Two extensions declared into one family (fixture tokens: this
        // directory spells no language id, the `jvm_parity` guard).
        let l = py(WERKZEUG).with_families(HashMap::from([
            ("ja".to_string(), "shared".to_string()),
            ("kx".to_string(), "shared".to_string()),
        ]));
        assert_eq!(l.family("a/B.ja").as_deref(), Some("shared"));
        assert_eq!(l.family("a/B.KX").as_deref(), Some("shared"));
        assert_eq!(l.family("a/B.cs").as_deref(), Some("cs"));
        assert_eq!(l.family("Makefile"), None);
        assert!(l.is_family_crate(&family_crate("python")));
        assert!(!l.is_family_crate("crate"));
    }
}

// The call targets (S-521) a single-plugin layout carries, against the
// registry's. Every loaded plugin is walked, so no language id is spelt here.
#[cfg(test)]
mod call_target_tests {
    use super::*;

    /// [`PackageLayout::from_plugin`] is the registry's layout for every
    /// extension of its plugin — call targets included — and at least one
    /// shipped plugin declares some, so the comparison is not over defaults.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_call_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let mut declaring = 0;
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                assert_eq!(own.call_targets(&path), full.call_targets(&path), "{path}");
                declaring += usize::from(own.call_targets(&path).any());
            }
        }
        assert!(declaring > 0, "some shipped plugin declares a call target");
    }

    /// The same parity for the supertype key (S-522): the single-plugin layout
    /// answers every extension as the registry's does, over a set some shipped
    /// plugin is in.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_supertype_kind_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let mut declaring = 0;
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let follows = own.supertype_kind_follows_target(&path);
                assert_eq!(follows, full.supertype_kind_follows_target(&path), "{path}");
                declaring += usize::from(follows);
            }
        }
        assert!(declaring > 0, "some shipped plugin declares the supertype key");
    }
}
