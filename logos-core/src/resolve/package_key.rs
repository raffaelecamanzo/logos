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
//! re-derivation would cause), and never a second reading of a namespace. A
//! lexical constant's name (S-565) has no path to read: it is its enclosing
//! constant joined to the name it is written with, and [`constant_fqn`] is the
//! one place that join is made.
//!
//! [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
//! [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::plugin::{BareSpecifiers, CallTargets, LanguagePlugin, LanguageRegistry, ModuleModelKind, PathModelDecl};

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
    /// Normalised extensions whose language's classes inherit the bodies of
    /// the interface members they implement (S-609, [FR-RS-48]).
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    interface_body_exts: HashSet<String>,
    /// Normalised extensions whose language declares structural types: every
    /// type relation it records relates its declaration (S-535, [FR-EX-17]).
    ///
    /// [FR-EX-17]: ../../../docs/specs/requirements/FR-EX-17.md
    structural_type_exts: HashSet<String>,
    /// Normalised extensions whose language declares that a namespace sees the
    /// types of its enclosing namespaces (S-595, [FR-RS-45]).
    ///
    /// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
    enclosing_namespace_exts: HashSet<String>,
    /// Normalised extensions whose language tells a type apart from a
    /// same-named one by its generic arity (S-602, [CR-199]).
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    type_arity_exts: HashSet<String>,
    /// Normalised extensions whose language names its classes and modules by
    /// lexical constants (S-565, [FR-RS-33]).
    ///
    /// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
    lexical_constant_exts: HashSet<String>,
    /// Normalised extensions whose language declares that the files of one
    /// directory declaring one `package` clause share one package scope
    /// (S-528, [FR-RS-17]).
    ///
    /// [FR-RS-17]: ../../../docs/specs/requirements/FR-RS-17.md
    directory_package_exts: HashSet<String>,
    /// Normalised extensions whose language declares that a bare call no
    /// included declaration supplies binds across its link unit (S-584,
    /// [FR-RS-40]).
    ///
    /// [FR-RS-40]: ../../../docs/specs/requirements/FR-RS-40.md
    link_unit_exts: HashSet<String>,
    /// Project-relative path → the `package` clause that file declares — only
    /// for a file of a [`directory_package_exts`](Self::directory_package_exts)
    /// language whose clause was recorded (S-528).
    package_clauses: HashMap<String, String>,
    /// Normalised extensions whose language declares that a type and the
    /// same-named singleton beside it are one name (S-573, [FR-RS-36]).
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    companion_object_exts: HashSet<String>,
    /// Normalised extensions whose language declares that a file's top-level
    /// functions are members of its declared namespace (S-573, [FR-RS-36]).
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    top_level_callable_exts: HashSet<String>,
    /// Normalised extensions whose language declares that a wildcard import
    /// outranks the namespace's other files (S-573, [FR-RS-36]).
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    wildcard_first_exts: HashSet<String>,
    /// Normalised extensions whose language declares that a top-level function
    /// with a receiver type is callable on a value (S-560, [FR-RS-31]).
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    extension_function_exts: HashSet<String>,
    /// Normalised extension → the suffix of the class another language reaches
    /// a file's top-level functions through (S-560, [FR-RS-31]).
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    file_facades: HashMap<String, String>,
    /// Normalised extension → the name its language gives a type's anonymous
    /// companion object (I4-INT, [FR-RS-31]).
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    companion_names: HashMap<String, String>,
    /// Normalised extensions whose language declares that an unqualified call
    /// its namespace does not supply falls back to the global function (S-551,
    /// [FR-RS-26]).
    ///
    /// [FR-RS-26]: ../../../docs/specs/requirements/FR-RS-26.md
    global_function_fallback_exts: HashSet<String>,
    /// Normalised extension → its language's plugin name (S-560): what tells
    /// two languages of one interop family apart.
    languages: HashMap<String, String>,
    /// Normalised extensions whose language binds its `#include` against the
    /// including file's directory and the repository's include roots, and its
    /// calls and type relations through what it includes (S-579, [FR-RS-38]).
    ///
    /// [FR-RS-38]: ../../../docs/specs/requirements/FR-RS-38.md
    include_root_exts: HashSet<String>,
    /// Normalised extensions whose language explicitly declares
    /// `implicit_receiver = "none"`, so a bare call binds free callables only
    /// (S-590, [FR-RS-07]).
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    free_only_bare_call_exts: HashSet<String>,
    /// Normalised extensions whose unqualified in-class call, when no member
    /// of its class admits it, goes on to the free functions and imports in
    /// scope (S-592, [FR-RS-43]).
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    free_call_fallthrough_exts: HashSet<String>,
    /// Normalised extension → the members the root every class of its language
    /// inherits declares (S-592), names that never fall through.
    implicit_root_members: HashMap<String, HashSet<String>>,
    /// Normalised extensions whose language overloads callables by name, so a
    /// bare call binds only a callable whose parameter range admits it (S-592,
    /// [FR-RS-43]).
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    overloaded_call_exts: HashSet<String>,
    /// The extensions whose language decides a call to a type's method by the
    /// one associated-item lookup over recorded `impl` blocks (S-607,
    /// [FR-RS-47]).
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    impl_block_lookup_exts: HashSet<String>,
    /// Normalised extensions of a language that enforces no arity —
    /// JavaScript's — whose calls no parameter range filters (S-592,
    /// [FR-RS-43]).
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    arity_unchecked_exts: HashSet<String>,
    /// Normalised extension → the methods each peeled receiver wrapper of its
    /// language provides itself (S-588, [FR-RS-42]). An absent extension, or
    /// an absent wrapper, provides none.
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    wrapper_methods: HashMap<String, BTreeMap<String, Vec<String>>>,
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
            .with_interface_bodies(registry.interface_body_inheriting_extensions())
            .with_structural_types(registry.structural_type_extensions())
            .with_enclosing_namespaces(registry.enclosing_namespace_extensions())
            .with_type_arity(registry.type_arity_extensions())
            .with_lexical_constants(registry.lexical_constant_extensions())
            .with_directory_packages(registry.directory_package_extensions())
            .with_link_units(registry.link_unit_extensions())
            .with_companion_objects(registry.companion_object_extensions())
            .with_top_level_callables(registry.top_level_callable_extensions())
            .with_wildcards_first(registry.wildcard_first_extensions())
            .with_extension_functions(registry.extension_function_extensions())
            .with_file_facades(registry.file_facades())
            .with_companion_names(registry.companion_names())
            .with_global_function_fallback(registry.global_function_fallback_extensions())
            .with_languages(registry.languages())
            .with_include_roots(registry.include_root_extensions())
            .with_free_only_bare_calls(registry.free_only_bare_call_extensions())
            .with_free_call_fallthrough(registry.free_call_fallthrough_extensions())
            .with_implicit_root_members(registry.implicit_root_members())
            .with_overloaded_calls(registry.overloaded_call_extensions())
            .with_impl_block_lookup(registry.impl_block_lookup_extensions())
            .with_arity_unchecked(registry.arity_unchecked_extensions())
            .with_wrapper_methods(registry.wrapper_methods())
    }

    /// This layout, with the methods each extension's peeled receiver wrappers
    /// provide (S-588; the map [`LanguageRegistry::wrapper_methods`] returns).
    pub fn with_wrapper_methods(
        mut self,
        declared: HashMap<String, BTreeMap<String, Vec<String>>>,
    ) -> Self {
        extend_normalised_map(&mut self.wrapper_methods, declared);
        self
    }

    /// This layout, with the extensions whose bare call binds free callables
    /// only (S-590; the set
    /// [`LanguageRegistry::free_only_bare_call_extensions`] returns).
    pub fn with_free_only_bare_calls(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.free_only_bare_call_exts, exts);
        self
    }

    /// This layout, with the extensions whose unqualified in-class call falls
    /// through to a free function (S-592; the set
    /// [`LanguageRegistry::free_call_fallthrough_extensions`] returns).
    pub fn with_free_call_fallthrough(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.free_call_fallthrough_exts, exts);
        self
    }

    /// This layout, with the members each extension's root class declares
    /// (S-592; the map [`LanguageRegistry::implicit_root_members`] returns).
    pub fn with_implicit_root_members(mut self, declared: HashMap<String, Vec<String>>) -> Self {
        self.implicit_root_members.extend(
            declared
                .into_iter()
                .map(|(ext, names)| (ext.trim_start_matches('.').to_ascii_lowercase(), names.into_iter().collect())),
        );
        self
    }

    /// This layout, with the extensions whose language overloads callables by
    /// name (S-592; the set [`LanguageRegistry::overloaded_call_extensions`]
    /// returns).
    pub fn with_overloaded_calls(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.overloaded_call_exts, exts);
        self
    }

    /// This layout, with the extensions whose calls to a type's method bind
    /// through the one associated-item lookup (S-607; the set
    /// [`LanguageRegistry::impl_block_lookup_extensions`] returns).
    pub fn with_impl_block_lookup(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.impl_block_lookup_exts, exts);
        self
    }

    /// This layout, with the extensions whose language enforces no arity
    /// (S-592; the set [`LanguageRegistry::arity_unchecked_extensions`]
    /// returns).
    pub fn with_arity_unchecked(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.arity_unchecked_exts, exts);
        self
    }

    /// This layout, with the extensions whose namespace sees its enclosing
    /// namespaces' types (S-595; the set
    /// [`LanguageRegistry::enclosing_namespace_extensions`] returns).
    pub fn with_enclosing_namespaces(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.enclosing_namespace_exts, exts);
        self
    }

    /// This layout, with the extensions whose types are told apart by their
    /// generic arity (S-602; the set [`LanguageRegistry::type_arity_extensions`]
    /// returns).
    pub fn with_type_arity(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.type_arity_exts, exts);
        self
    }

    /// This layout, with the extensions whose classes and modules are named by
    /// lexical constants (S-565; the set
    /// [`LanguageRegistry::lexical_constant_extensions`] returns).
    pub fn with_lexical_constants(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.lexical_constant_exts, exts);
        self
    }

    /// This layout, with the extensions whose files of one directory and one
    /// `package` clause share one package scope (S-528; the set
    /// [`LanguageRegistry::directory_package_extensions`] returns).
    pub fn with_directory_packages(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.directory_package_exts, exts);
        self
    }

    /// This layout, with the extensions whose bare call binds across its link
    /// unit (S-584; the set [`LanguageRegistry::link_unit_extensions`]
    /// returns).
    pub fn with_link_units(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.link_unit_exts, exts);
        self
    }

    /// This layout, with the extensions whose type and same-named singleton
    /// are one name (S-573; the set
    /// [`LanguageRegistry::companion_object_extensions`] returns).
    pub fn with_companion_objects(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.companion_object_exts, exts);
        self
    }

    /// This layout, with the extensions whose top-level functions are members
    /// of the declared namespace (S-573; the set
    /// [`LanguageRegistry::top_level_callable_extensions`] returns).
    pub fn with_top_level_callables(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.top_level_callable_exts, exts);
        self
    }

    /// This layout, with the extensions whose wildcard imports outrank the
    /// namespace's other files (S-573; the set
    /// [`LanguageRegistry::wildcard_first_extensions`] returns).
    pub fn with_wildcards_first(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.wildcard_first_exts, exts);
        self
    }

    /// This layout, with the extensions whose top-level functions with a
    /// receiver type are callable on a value (S-560; the set
    /// [`LanguageRegistry::extension_function_extensions`] returns).
    pub fn with_extension_functions(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.extension_function_exts, exts);
        self
    }

    /// This layout, with each extension's file-facade suffix (S-560; the map
    /// [`LanguageRegistry::file_facades`] returns).
    pub fn with_file_facades(mut self, facades: HashMap<String, String>) -> Self {
        extend_normalised_map(&mut self.file_facades, facades);
        self
    }

    /// This layout, with each extension's companion name (I4-INT; the map
    /// [`LanguageRegistry::companion_names`] returns).
    pub fn with_companion_names(mut self, names: HashMap<String, String>) -> Self {
        extend_normalised_map(&mut self.companion_names, names);
        self
    }

    /// This layout, with the extensions whose unqualified calls fall back to
    /// the global function (S-551; the set
    /// [`LanguageRegistry::global_function_fallback_extensions`] returns).
    pub fn with_global_function_fallback(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.global_function_fallback_exts, exts);
        self
    }

    /// This layout, with each extension's language (S-560; the map
    /// [`LanguageRegistry::languages`] returns).
    pub fn with_languages(mut self, languages: HashMap<String, String>) -> Self {
        extend_normalised_map(&mut self.languages, languages);
        self
    }

    /// This layout, with the extensions whose includes bind against include
    /// roots (S-579; the set [`LanguageRegistry::include_root_extensions`]
    /// returns).
    pub fn with_include_roots(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.include_root_exts, exts);
        self
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
        extend_normalised_map(&mut self.families, families);
        self
    }

    /// This layout, with each extension's call targets (S-521; the shape
    /// [`LanguageRegistry::call_targets`] returns).
    pub fn with_call_targets(mut self, targets: HashMap<String, CallTargets>) -> Self {
        extend_normalised_map(&mut self.call_targets, targets);
        self
    }

    /// This layout, with the extensions whose supertype's edge kind follows
    /// its target (S-522; the set
    /// [`LanguageRegistry::supertype_kind_follows_target`] returns).
    pub fn with_kind_following_supertypes(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.kind_following_supertypes, exts);
        self
    }

    /// This layout, with the extensions whose classes inherit the bodies of
    /// the interface members they implement (S-609; the set
    /// [`LanguageRegistry::interface_body_inheriting_extensions`] returns).
    pub fn with_interface_bodies(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.interface_body_exts, exts);
        self
    }

    /// This layout, with the extensions whose language declares structural
    /// types (S-535; the set [`LanguageRegistry::structural_type_extensions`]
    /// returns).
    pub fn with_structural_types(mut self, exts: impl IntoIterator<Item = String>) -> Self {
        extend_normalised(&mut self.structural_type_exts, exts);
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

    /// Whether a type name written in the file at `path` is also looked up in
    /// each enclosing namespace of the namespace that file declares (S-595,
    /// [FR-RS-45]): its language's declaration. The global namespace is never
    /// one of those levels.
    ///
    /// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
    pub fn sees_enclosing_namespaces(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.enclosing_namespace_exts.contains(&ext))
    }

    /// Whether a type name written in the file at `path` binds only a type of
    /// its own written arity in the own-namespace, enclosing-namespace and
    /// namespace-wildcard rungs (S-602, [CR-199]): its language's declaration.
    /// A reference of unknown arity admits any.
    ///
    /// [CR-199]: ../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    pub fn matches_type_arity(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.type_arity_exts.contains(&ext))
    }

    /// Whether the classes and modules of the file at `path` are the constants
    /// their nesting spells, and a constant its references name is read through
    /// the lexical nesting outward, then the top level (S-565, [FR-RS-33]): its
    /// language's declaration. The file's module key is unchanged.
    ///
    /// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
    pub fn resolves_lexical_constants(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.lexical_constant_exts.contains(&ext))
    }

    /// Whether the files of one directory that declare one `package` clause
    /// share one package scope (S-528, [FR-RS-17]): the language of the file
    /// at `path` declares it — Go's. The file's module key is unchanged.
    ///
    /// [FR-RS-17]: ../../../docs/specs/requirements/FR-RS-17.md
    pub fn declares_directory_packages(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.directory_package_exts.contains(&ext))
    }

    /// Whether the file at `path` is of a language whose files make one link
    /// unit (S-584, [FR-RS-40]): a bare call written there that no included
    /// declaration supplies binds the one non-`static` definition of the name
    /// among those files, and a definition there is a candidate for one —
    /// C's `.c` files, and its `.h` ones where the repository's headers are
    /// read as C (S-582). The file's module key is unchanged.
    ///
    /// [FR-RS-40]: ../../../docs/specs/requirements/FR-RS-40.md
    pub fn links_units(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.link_unit_exts.contains(&ext))
    }

    /// The directory package the file at `path` sits in (S-528,
    /// [FR-RS-17]): its directory (`""` at the repository root) and the
    /// `package` clause it declares — `None` when its language declares no
    /// directory packages, or its clause was not recorded. `server/routes.go`
    /// declaring `package server` is `("server", "server")`; a
    /// `server/routes_test.go` declaring `package server_test` is another
    /// package of the same directory.
    ///
    /// [FR-RS-17]: ../../../docs/specs/requirements/FR-RS-17.md
    pub fn directory_package<'p>(&'p self, path: &'p str) -> Option<(&'p str, &'p str)> {
        let clause = self.package_clauses.get(path)?;
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        Some((dir, clause.as_str()))
    }

    /// Whether a type of the file at `path` and the same-named singleton
    /// declared beside it are one name (S-573, [FR-RS-36]): its language's
    /// declaration. A call on the name binds among the singleton's callables.
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub fn pairs_companion_objects(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.companion_object_exts.contains(&ext))
    }

    /// Whether the functions the file at `path` declares at its top level are
    /// members of the namespace it declares (S-573, [FR-RS-36]): its
    /// language's declaration.
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub fn declares_top_level_callables(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.top_level_callable_exts.contains(&ext))
    }

    /// Whether a name a wildcard import of the file at `path` brings into view
    /// outranks one its namespace declares in another file (S-573,
    /// [FR-RS-36]): its language's declaration — Scala's precedence.
    ///
    /// [FR-RS-36]: ../../../docs/specs/requirements/FR-RS-36.md
    pub fn reads_wildcards_first(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.wildcard_first_exts.contains(&ext))
    }

    /// Whether a top-level function with a receiver type, declared in the
    /// language of the file at `path`, is callable on a value there (S-560,
    /// [FR-RS-31]): its language's declaration.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn binds_extension_calls(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.extension_function_exts.contains(&ext))
    }

    /// The language of the file at `path` (S-560): its plugin's name, `None`
    /// for an extension no code plugin claims.
    pub fn language(&self, path: &str) -> Option<&str> {
        self.languages.get(&extension(path)?).map(String::as_str)
    }

    /// Whether the language of the file at `path` declares file facades
    /// (S-560, [FR-RS-31]) — and so never names one itself: Kotlin calls a
    /// top-level function by its own name, never through `UtilsKt`.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn declares_file_facades(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.file_facades.contains_key(&ext))
    }

    /// The class another language of the family reaches the top-level
    /// functions of the file at `path` through (S-560, [FR-RS-31]), `None`
    /// when its language declares no facade: the file's stem, every character
    /// that cannot spell a class name replaced by `_` and the first letter
    /// upper-cased, then the declared suffix — `src/util/Utils.kt` →
    /// `UtilsKt`, `date-utils.kt` → `Date_utilsKt`.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn file_facade(&self, path: &str) -> Option<String> {
        let suffix = self.file_facades.get(&extension(path)?)?;
        let stem = Path::new(path).file_stem()?.to_str()?;
        let mut chars = stem.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' });
        let first = chars.next()?;
        Some(first.to_uppercase().chain(chars).chain(suffix.chars()).collect())
    }

    /// The name the language of the file at `path` gives a type's anonymous
    /// companion object (I4-INT, [FR-RS-31]) — Kotlin's `Companion` — `None`
    /// when its language declares no companions.
    ///
    /// [FR-RS-31]: ../../../docs/specs/requirements/FR-RS-31.md
    pub fn companion_name(&self, path: &str) -> Option<&str> {
        self.companion_names.get(&extension(path)?).map(String::as_str)
    }

    /// Whether an unqualified call of the file at `path` that its namespace
    /// declares no function for binds to the global namespace's function
    /// (S-551, [FR-RS-26]): its language's declaration — PHP's fallback.
    ///
    /// [FR-RS-26]: ../../../docs/specs/requirements/FR-RS-26.md
    pub fn falls_back_to_global_functions(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.global_function_fallback_exts.contains(&ext))
    }

    /// Whether the file at `path` binds its `#include` against its own
    /// directory and the repository's include roots, and its calls and type
    /// relations through the declarations its includes make visible (S-579,
    /// [FR-RS-38]): its language's declaration. The file's module key is
    /// unchanged.
    ///
    /// [FR-RS-38]: ../../../docs/specs/requirements/FR-RS-38.md
    pub fn resolves_includes(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.include_root_exts.contains(&ext))
    }

    /// Whether a bare call written in the file at `path` binds free callables
    /// only — never a member of a class-like container nor a callable with a
    /// recorded self type (S-590, [FR-RS-07]): its language declares
    /// `implicit_receiver = "none"` explicitly.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    pub fn bare_calls_free_only(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.free_only_bare_call_exts.contains(&ext))
    }

    /// Whether an unqualified in-class call written in the file at `path`, when
    /// no member of its class admits it, goes on to the free functions and
    /// imports in scope (S-592, [FR-RS-43]): its language declares
    /// `implicit_call_falls_through`
    /// ([`LanguageRegistry::free_call_fallthrough_extensions`]), and `name` is
    /// none of the members the root every class inherits declares — Kotlin's
    /// `hashCode()` beside a top-level `hashCode` is `Any.hashCode()`.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub fn falls_through_to_free_calls(&self, path: &str, name: &str) -> bool {
        extension(path).is_some_and(|ext| {
            self.free_call_fallthrough_exts.contains(&ext)
                && !self.implicit_root_members.get(&ext).is_some_and(|roots| roots.contains(name))
        })
    }

    /// Whether the language of the file at `path` overloads callables by name,
    /// so a bare call there binds only a callable whose parameter range admits
    /// it (S-592, [FR-RS-43]).
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub fn overloads_calls(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.overloaded_call_exts.contains(&ext))
    }

    /// Whether a call to a type's method written in the file at `path` binds
    /// through the one associated-item lookup over recorded `impl` blocks
    /// (S-607, [FR-RS-47]), every unbound one with a reason.
    ///
    /// [FR-RS-47]: ../../../docs/specs/requirements/FR-RS-47.md
    pub fn looks_up_impl_blocks(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.impl_block_lookup_exts.contains(&ext))
    }

    /// Whether a call written in the file at `path` is filtered by its
    /// candidates' parameter ranges at all (S-592, [FR-RS-43]): `false` for a
    /// language that enforces no arity — a JavaScript file.
    ///
    /// [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
    pub fn checks_arity(&self, path: &str) -> bool {
        !extension(path).is_some_and(|ext| self.arity_unchecked_exts.contains(&ext))
    }

    /// Whether the file at `path`'s language proves a receiver's declared type
    /// through peeled wrappers (S-588, [FR-RS-42]): it declares a non-empty
    /// `[wrapper_methods]` table — the declaration is the signal, as the binder
    /// binds a proven `T::m` row by its shape alone. `status` reports why such a
    /// language's calls stay unbound (S-589).
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub fn peels_receivers(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.wrapper_methods.contains_key(&ext))
    }

    /// Whether `wrapper`, peeled from a receiver in the file at `path`, provides
    /// `method` itself (S-588, [FR-RS-42]): its language declares it so. A call
    /// through `Arc<T>` naming `clone` calls `Arc::clone`, never `T`'s.
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub fn wrapper_provides(&self, path: &str, wrapper: &str, method: &str) -> bool {
        extension(path)
            .and_then(|ext| self.wrapper_methods.get(&ext))
            .and_then(|declared| declared.get(wrapper))
            .is_some_and(|methods| methods.iter().any(|m| m == method))
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

    /// Whether a type declared in the file at `path` inherits the bodies of
    /// the interface members it implements (S-609, [FR-RS-48]), so a supertype
    /// walk goes on from its `Extends` chain to its interfaces.
    ///
    /// [FR-RS-48]: ../../../docs/specs/requirements/FR-RS-48.md
    pub fn inherits_interface_bodies(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.interface_body_exts.contains(&ext))
    }

    /// Whether the file at `path`'s language declares structural types
    /// (S-535, [FR-EX-17]): a type use or instantiation binds from any of its
    /// declarations, a supertype clause binds any type-like declaration it
    /// names, and a relation written through an import binds within what the
    /// import binds to. A call on a proven receiver binds among its type's
    /// members, required signatures included (S-537).
    ///
    /// [FR-EX-17]: ../../../docs/specs/requirements/FR-EX-17.md
    pub fn has_structural_types(&self, path: &str) -> bool {
        extension(path).is_some_and(|ext| self.structural_type_exts.contains(&ext))
    }

    fn path_model(&self, path: &str) -> Option<&PathModel> {
        self.path_models.get(&extension(path)?)
    }

    /// A registry-less layout folding the package-file stems the rust plugin
    /// declares (`mod`, `lib`, `main`) for `.rs` files — what a synthetic test
    /// graph of Rust paths is keyed with (`binder::Index::build`) — and its
    /// `impl_block_lookup` (S-607). Pinned against the loaded descriptor by
    /// `rust_tests::the_test_layout_folds_the_stems_the_rust_plugin_declares`,
    /// so the two cannot drift.
    #[cfg(test)]
    pub(crate) fn rust_stems_for_tests() -> Self {
        Self::stems_only_for_tests().with_impl_block_lookup(["rs".to_string()])
    }

    /// [`rust_stems_for_tests`](Self::rust_stems_for_tests) without the
    /// associated-item lookup: the `.rs` path model alone, as a language
    /// recording no `impl` block would key its files (S-607).
    #[cfg(test)]
    pub(crate) fn stems_only_for_tests() -> Self {
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
        extend_normalised(&mut self.namespace_exts, exts);
        self
    }

    /// This layout, given the namespace each file declares — `(path, name)`
    /// pairs, the name in [`namespace_text`] form (`""` for the global
    /// namespace). A file of a directory-package language records its
    /// `package` clause in the same column (S-528), and its pair is taken as
    /// that clause ([`directory_package`](Self::directory_package)), never as a
    /// namespace: it re-keys nothing. A pair whose file is of neither kind is
    /// ignored, so a stray row can never re-key a Rust or Java file.
    pub fn with_declared_namespaces(
        mut self,
        declared: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        for (path, name) in declared {
            if self.declares_namespaces(&path) {
                self.namespaces.insert(path, namespace_segments(&name));
            } else if self.declares_directory_packages(&path) {
                self.package_clauses.insert(path, name);
            }
        }
        self
    }

    /// The layout one plugin declares — [`from_registry`](Self::from_registry)
    /// narrowed to a single language, for the single-file
    /// [`extract`](crate::extract::extract) entry point, which is handed a
    /// plugin and no registry. For every extension the registry gives that
    /// plugin it is the registry's layout exactly; a shared one (S-582: `.h`)
    /// reads as this plugin's, as the plugin parses it.
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
            .with_interface_bodies(exts().filter(|_| semantics.inherits_interface_bodies))
            .with_structural_types(exts().filter(|_| semantics.structural_types))
            .with_enclosing_namespaces(exts().filter(|_| semantics.enclosing_namespaces))
            .with_type_arity(exts().filter(|_| semantics.type_arity))
            .with_lexical_constants(exts().filter(|_| semantics.lexical_constants))
            .with_directory_packages(exts().filter(|_| semantics.directory_packages))
            .with_link_units(exts().filter(|_| semantics.link_unit))
            .with_companion_objects(exts().filter(|_| semantics.companion_objects))
            .with_top_level_callables(exts().filter(|_| semantics.top_level_callables))
            .with_wildcards_first(exts().filter(|_| semantics.wildcards_outrank_package))
            .with_extension_functions(exts().filter(|_| semantics.extension_functions))
            .with_file_facades(
                exts()
                    .filter_map(|ext| Some((ext, semantics.file_facade.clone()?)))
                    .collect(),
            )
            .with_companion_names(
                exts()
                    .filter_map(|ext| Some((ext, semantics.companion_name.clone()?)))
                    .collect(),
            )
            .with_global_function_fallback(exts().filter(|_| semantics.global_function_fallback))
            .with_languages(exts().map(|ext| (ext, plugin.name().to_string())).collect())
            .with_include_roots(exts().filter(|_| semantics.bare_specifiers == BareSpecifiers::IncludeRoots))
            .with_free_only_bare_calls(exts().filter(|_| semantics.bare_calls_free_only))
            .with_free_call_fallthrough(exts().filter(|_| semantics.implicit_call_falls_through))
            .with_implicit_root_members(
                exts()
                    .filter(|_| !semantics.implicit_root_members.is_empty())
                    .map(|ext| (ext, semantics.implicit_root_members.clone()))
                    .collect(),
            )
            .with_overloaded_calls(exts().filter(|_| semantics.overloaded_calls))
            .with_impl_block_lookup(exts().filter(|_| semantics.impl_block_lookup))
            .with_arity_unchecked(semantics.arity_unchecked_extensions.iter().cloned())
            .with_wrapper_methods(
                exts()
                    .filter(|_| !semantics.wrapper_methods.is_empty())
                    .map(|ext| (ext, semantics.wrapper_methods.clone()))
                    .collect(),
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

/// Add `exts` to `set`, each normalised as every extension key here is —
/// without a leading dot, lower-cased — the one body of the per-key `with_*`
/// set builders (S-592: written once, not once per key).
fn extend_normalised(set: &mut HashSet<String>, exts: impl IntoIterator<Item = String>) {
    set.extend(exts.into_iter().map(|e| e.trim_start_matches('.').to_ascii_lowercase()));
}

/// [`extend_normalised`] for a map keyed by extension: each key normalised the
/// same way, its value kept.
fn extend_normalised_map<V>(map: &mut HashMap<String, V>, entries: HashMap<String, V>) {
    map.extend(
        entries
            .into_iter()
            .map(|(ext, value)| (ext.trim_start_matches('.').to_ascii_lowercase(), value)),
    );
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

/// The fully-qualified name of a constant declared as `written` (`B`, or a
/// compact `A::B`'s `[A, B]`) inside the declaration whose constant is
/// `enclosing` (S-565, [FR-RS-33]) — Ruby's own naming: a declaration nested in
/// `module A` is `A::B`, a compact `class A::B` at the top level is `A::B`, and
/// a compact one headed by the fully-qualified head (`class ::A::B`, recorded
/// [`FULLY_QUALIFIED_HEAD`](crate::resolve::FULLY_QUALIFIED_HEAD)) is read from
/// the top level whatever encloses it. A compact name's scope is read under the
/// enclosing constant, never looked up outward: a declaration names one
/// constant, decided by where it is written.
///
/// [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
pub fn constant_fqn(enclosing: &[String], written: &[String]) -> Vec<String> {
    match written.split_first() {
        Some((head, rest)) if head == crate::resolve::FULLY_QUALIFIED_HEAD => rest.to_vec(),
        _ => enclosing.iter().chain(written).cloned().collect(),
    }
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
    /// stems the loaded rust plugin declares, declares no import roots, and
    /// looks its calls up through impl blocks as the plugin does (S-607).
    #[cfg(feature = "lang-rust")]
    #[test]
    fn the_test_layout_folds_the_stems_the_rust_plugin_declares() {
        let registry = registry_layout();
        let test = PackageLayout::rust_stems_for_tests();
        assert_eq!(registry.path_models.get("rs"), test.path_models.get("rs"));
        assert!(registry.path_models["rs"].roots.is_none());
        assert_eq!(registry.impl_block_lookup_exts, test.impl_block_lookup_exts);
        assert!(!PackageLayout::stems_only_for_tests().looks_up_impl_blocks("src/lib.rs"));
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

    /// The extensions the registry gives `plugin` — every one it claims, less
    /// a shared one (S-582: C's `.h`) another plugin reads by default.
    fn read_extensions<'p>(registry: &LanguageRegistry, plugin: &'p dyn LanguagePlugin) -> Vec<&'p String> {
        plugin
            .extensions()
            .iter()
            .filter(|ext| registry.for_extension(ext).is_some_and(|reader| reader.name() == plugin.name()))
            .collect()
    }

    /// [`PackageLayout::from_plugin`] is the registry's layout for every
    /// extension the registry gives its plugin — call targets included — and at least one
    /// shipped plugin declares some, so the comparison is not over defaults.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_call_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let mut declaring = 0;
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in read_extensions(&registry, plugin) {
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

    /// The same parity for the structural-types key (S-535): the single-plugin
    /// layout answers every extension as the registry's does, over a set some
    /// shipped plugin is in.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_structural_types_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let mut declaring = 0;
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let structural = own.has_structural_types(&path);
                assert_eq!(structural, full.has_structural_types(&path), "{path}");
                declaring += usize::from(structural);
            }
        }
        assert!(declaring > 0, "some shipped plugin declares the structural-types key");
    }

    /// The same parity for the enclosing-namespace key (S-595): the
    /// single-plugin layout answers every extension as the registry's does,
    /// over a set some shipped plugin is in — and a plugin that does not
    /// declare the key is out of it.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_enclosing_namespace_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let (mut declaring, mut silent) = (0, 0);
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let sees = own.sees_enclosing_namespaces(&path);
                assert_eq!(sees, full.sees_enclosing_namespaces(&path), "{path}");
                declaring += usize::from(sees);
                silent += usize::from(!sees);
            }
        }
        assert!(declaring > 0, "some shipped plugin declares the enclosing-namespace key");
        assert!(silent > 0, "a plugin that does not declare it binds as before");
    }

    /// The same parity for the type-arity key (S-602): the single-plugin
    /// layout answers every extension as the registry's does, and only C#'s
    /// `.cs` declares it — every other language binds as before.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_type_arity_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let mut declaring: Vec<String> = Vec::new();
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let matches = own.matches_type_arity(&path);
                assert_eq!(matches, full.matches_type_arity(&path), "{path}");
                if matches {
                    declaring.push(path);
                }
            }
        }
        let expected: Vec<String> = if cfg!(feature = "lang-c-sharp") { vec!["dir/file.cs".into()] } else { Vec::new() };
        assert_eq!(declaring, expected);
    }

    /// The same parity for the companion-object, top-level-callable and
    /// wildcard-precedence keys (S-573), the directory-package key (S-528),
    /// the extension-function and file-facade keys (S-560), the
    /// companion-name key (I4-INT) and the global-function-fallback key
    /// (S-551): for each, the single-plugin layout
    /// answers every extension as the registry's does, over a set some shipped
    /// plugin is in, and a plugin that does not declare it is out of it.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_companion_and_top_level_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        type Key = fn(&PackageLayout, &str) -> bool;
        let keys: [(&str, Key); 8] = [
            ("companion_objects", PackageLayout::pairs_companion_objects),
            ("top_level_callables", PackageLayout::declares_top_level_callables),
            ("wildcards_outrank_package", PackageLayout::reads_wildcards_first),
            ("directory_packages", PackageLayout::declares_directory_packages),
            // S-560's keys.
            ("extension_functions", PackageLayout::binds_extension_calls),
            ("file_facade", PackageLayout::declares_file_facades),
            ("companion_name", |layout, path| layout.companion_name(path).is_some()),
            // S-551's key.
            ("global_function_fallback", PackageLayout::falls_back_to_global_functions),
        ];
        for (key, read) in keys {
            let (mut declaring, mut silent) = (0, 0);
            for plugin in registry.iter() {
                let own = PackageLayout::from_plugin(plugin);
                for ext in plugin.extensions() {
                    let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                    let on = read(&own, &path);
                    assert_eq!(on, read(&full, &path), "{key}: {path}");
                    declaring += usize::from(on);
                    silent += usize::from(!on);
                }
            }
            assert!(declaring > 0, "some shipped plugin declares {key}");
            assert!(silent > 0, "a plugin that does not declare {key} binds as before");
            assert!(!read(&full, "Makefile"), "{key}: a path with no extension");
        }
    }

    /// FR-RS-31 (S-560): a file's facade class is its stem, every character a
    /// class name cannot spell replaced by `_` and the first letter
    /// upper-cased, then the declared suffix — and a language that declares
    /// none has no facade.
    #[test]
    fn a_files_facade_class_is_its_capitalised_stem_and_the_declared_suffix() {
        let layout = PackageLayout::default().with_file_facades(HashMap::from([(".KT".to_string(), "Kt".to_string())]));
        let cases = [
            ("src/main/kotlin/mixit/util/Utils.kt", Some("UtilsKt")),
            ("a/b/defaultContextExt.kt", Some("DefaultContextExtKt")),
            ("a/date-utils.v2.kt", Some("Date_utils_v2Kt")),
            ("a/_private.kt", Some("_privateKt")),
            ("a/Utils.java", None),
            ("Makefile", None),
        ];
        for (path, facade) in cases {
            assert_eq!(layout.file_facade(path).as_deref(), facade, "{path}");
            assert_eq!(layout.declares_file_facades(path), facade.is_some(), "{path}");
        }
    }

    /// The same parity for the interface-body key (S-609): the single-plugin
    /// layout answers every extension as the registry's does, over a set some
    /// shipped plugin is in — and a plugin that does not declare the key is
    /// out of it.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_interface_body_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let (mut declaring, mut silent) = (0, 0);
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let inherits = own.inherits_interface_bodies(&path);
                assert_eq!(inherits, full.inherits_interface_bodies(&path), "{path}");
                declaring += usize::from(inherits);
                silent += usize::from(!inherits);
            }
        }
        assert!(declaring > 0, "some shipped plugin declares the interface-body key");
        assert!(silent > 0, "a plugin that does not declare it walks as before");
    }

    /// The same parity for the free-only bare-call key (S-590): the
    /// single-plugin layout answers every extension as the registry's does,
    /// over a set some shipped plugin is in — and one Java, which omits the
    /// key, is out of.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_free_only_bare_call_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let (mut declaring, mut silent) = (0, 0);
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let free_only = own.bare_calls_free_only(&path);
                assert_eq!(free_only, full.bare_calls_free_only(&path), "{path}");
                declaring += usize::from(free_only);
                silent += usize::from(!free_only);
            }
        }
        assert!(declaring > 0, "some shipped plugin declares the free-only key");
        assert!(silent > 0, "a plugin that does not declare it binds as before");
    }

    /// The same parity for the three arity keys (S-592): the single-plugin
    /// layout answers every extension as the registry's does, and each key is
    /// held by some shipped extension and not by another — the unchecked one
    /// inside a single plugin, `.js` in and `.ts` out.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_arity_keys() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let keys: [fn(&PackageLayout, &str) -> bool; 4] = [
            |layout, path| layout.falls_through_to_free_calls(path, "m"),
            PackageLayout::overloads_calls,
            PackageLayout::checks_arity,
            PackageLayout::looks_up_impl_blocks,
        ];
        let mut held = [(0, 0); 4];
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in read_extensions(&registry, plugin) {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                for (key, (yes, no)) in keys.iter().zip(held.iter_mut()) {
                    let answer = key(&own, &path);
                    assert_eq!(answer, key(&full, &path), "{path}");
                    *yes += usize::from(answer);
                    *no += usize::from(!answer);
                }
            }
        }
        assert!(held.iter().all(|&(yes, no)| yes > 0 && no > 0), "{held:?}");
        assert!(full.checks_arity("a/b.ts") && !full.checks_arity("a/b.js"));
        assert!(!full.falls_through_to_free_calls("a/b.kt", "hashCode"), "a root member never falls through");
    }

    /// The same parity for the wrapper-method table (S-588): the single-plugin
    /// layout answers every extension's `Arc::clone` as the registry's does,
    /// over a set Rust is in and every other shipped grammar is out of.
    #[test]
    fn a_single_plugin_layout_carries_the_registrys_wrapper_methods() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let full = PackageLayout::from_registry(&registry);
        let (mut declaring, mut silent) = (0, 0);
        for plugin in registry.iter() {
            let own = PackageLayout::from_plugin(plugin);
            for ext in plugin.extensions() {
                let path = format!("dir/file.{}", ext.trim_start_matches('.'));
                let provides = own.wrapper_provides(&path, "Arc", "clone");
                assert_eq!(provides, full.wrapper_provides(&path, "Arc", "clone"), "{path}");
                assert!(!own.wrapper_provides(&path, "&", "clone"), "{path}: a reference provides nothing");
                declaring += usize::from(provides);
                silent += usize::from(!provides);
            }
        }
        assert!(declaring > 0, "Rust declares the wrapper methods");
        assert!(silent > 0, "a plugin that does not declare them peels nothing");
    }

    /// A file peels receivers exactly when its language declares a
    /// `[wrapper_methods]` table (S-589): the shipped Rust plugin does, and no
    /// other shipped language — package-shaped Java included — does.
    #[test]
    fn only_a_language_declaring_wrapper_methods_peels_receivers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("registry loads");
        let layout = PackageLayout::from_registry(&registry);
        let peeling: Vec<&str> = registry
            .iter()
            .filter(|plugin| {
                plugin.extensions().iter().any(|ext| {
                    layout.peels_receivers(&format!("dir/file.{}", ext.trim_start_matches('.')))
                })
            })
            .map(|plugin| plugin.name())
            .collect();
        assert_eq!(peeling, ["rust"]);
        assert!(layout.peels_receivers("crates/a/src/lib.rs"));
        assert!(!layout.peels_receivers("crates/a/src/lib.rsx"), "an extension one letter off");
        assert!(!layout.peels_receivers("src/main/java/a/A.java"));
        assert!(!layout.peels_receivers("Makefile"), "a path with no extension");
    }
}

#[cfg(test)]
mod constant_tests {
    use super::{constant_fqn, PackageLayout};

    /// A constant is its enclosing constant joined to the name it is written
    /// with — a compact `A::B` included — and a fully-qualified head reads it
    /// from the top level whatever encloses it (S-565). Only an extension that
    /// declares lexical constants resolves them.
    #[test]
    fn a_constant_is_named_by_its_nesting_and_only_a_declaring_extension_reads_them() {
        let v = |s: &str| -> Vec<String> { s.split("::").filter(|x| !x.is_empty()).map(str::to_string).collect() };
        assert_eq!(constant_fqn(&[], &v("Keystore")), v("Keystore"));
        assert_eq!(constant_fqn(&v("Rack"), &v("Request")), v("Rack::Request"));
        assert_eq!(constant_fqn(&v("Rack::Auth"), &v("Abstract::Handler")), v("Rack::Auth::Abstract::Handler"));
        let rooted = vec![crate::resolve::FULLY_QUALIFIED_HEAD.to_string(), "Top".to_string()];
        assert_eq!(constant_fqn(&v("Rack"), &rooted), v("Top"));

        let layout = PackageLayout::default().with_lexical_constants(["rb".to_string()]);
        assert!(layout.resolves_lexical_constants("lib/rack/request.rb"));
        assert!(layout.resolves_lexical_constants("lib/rack/REQUEST.RB"), "normalised as every extension is");
        assert!(!layout.resolves_lexical_constants("lib/rack/request.rbx"), "an extension one letter off");
        assert!(!layout.resolves_lexical_constants("Rakefile"), "a path with no extension");
        assert!(!PackageLayout::default().resolves_lexical_constants("lib/a.rb"));
    }

    /// A directory-package file's recorded clause places it in its directory's
    /// package (S-528, FR-RS-17), and re-keys nothing: its module key is its
    /// path's. A pair for a file of any other language is ignored, and a
    /// declared-namespace file's pair is still its namespace.
    #[test]
    fn a_recorded_package_clause_places_a_directory_package_file_and_rekeys_nothing() {
        let plain = PackageLayout::default().with_directory_packages([".GO".to_string()]);
        let layout = plain.clone().with_namespace_extensions(["cs".to_string()]).with_declared_namespaces([
            ("server/routes.go".to_string(), "server".to_string()),
            ("server/routes_test.go".to_string(), "server_test".to_string()),
            ("main.go".to_string(), "main".to_string()),
            ("src/lib.rs".to_string(), "lib".to_string()),
            ("App/Order.cs".to_string(), "Shop.Domain".to_string()),
        ]);
        assert_eq!(layout.directory_package("server/routes.go"), Some(("server", "server")));
        assert_eq!(layout.directory_package("server/routes_test.go"), Some(("server", "server_test")));
        assert_eq!(layout.directory_package("main.go"), Some(("", "main")), "the repository root is a directory");
        assert_eq!(layout.directory_package("server/images.go"), None, "no clause recorded");
        assert_eq!(layout.directory_package("src/lib.rs"), None, "rust declares no directory packages");
        assert_eq!(layout.directory_package("App/Order.cs"), None, "a namespace is never a clause");
        assert_eq!(layout.package_of("App/Order.cs"), Some(vec!["Shop".to_string(), "Domain".to_string()]));
        assert_eq!(layout.module_key("server/routes.go"), plain.module_key("server/routes.go"));
        assert_eq!(layout.package_of("server/routes.go"), None, "a Go file takes no package rung");
        assert!(layout.declares_directory_packages("x/a.go") && !layout.declares_directory_packages("x/a.gox"));
        assert!(!PackageLayout::default().declares_directory_packages("x/a.go"));
    }
}
