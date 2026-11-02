//! The declarations of the binder's index keyed by the package layout (S-828,
//! [CR-258]): the fully-qualified types, top-level callables, file facades,
//! namespace files and lexical constants a package-shaped, declared-namespace
//! or lexical-constant file names, each partitioned by interop family, and
//! each type's generic arity ([`PackageIndex`]). A sub-index the
//! [`Index`](super::Index) owns, with its fields; the
//! [`IndexBuilder`](super::IndexBuilder) fills it. The layout predicates
//! themselves are the [`PackageLayout`]'s, read through
//! [`Index::layout`](super::Index::layout).
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md

use std::collections::HashMap;

use crate::model::NodeId;

use super::PackageLayout;

/// Interop family → fully-qualified name → declarations, id-sorted: the shape
/// of the lexical-constant index ([`PackageIndex::constants`](super::package_index::PackageIndex::constants), S-565).
pub(super) type FamilyConstants = HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>;

/// The layout-keyed declaration indexes, each partitioned by interop family
/// (S-519), and the types' generic arities.
#[derive(Debug, Default)]
pub(crate) struct PackageIndex {
    /// Fully-qualified name → the **top-level** type nodes declared under it in
    /// a package-shaped file, id-sorted — the universe a Java import, a
    /// same-package name and a wildcard resolve against ([CR-149]). Two entries
    /// under one name (a `src/main` and a `src/test` declaration) are an
    /// ambiguity, never a pick ([NFR-RA-05]) — but the parts of one partial
    /// type are one entry, its first-declared part (S-557, [`Index::partials`](super::Index::partials)).
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    ///
    /// Partitioned by **interop family** first (S-519, [NFR-RA-05];
    /// [`PackageLayout::family`]): a source reaches only the types of its own
    /// family, so a C# import never names a PHP class of the same
    /// fully-qualified spelling, while Java and Kotlin — one `jvm` family —
    /// still name each other's.
    pub(super) types_by_fqn: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// Fully-qualified name → the top-level functions declared under it, by
    /// interop family (S-573, `build_top_level_callables`): Scala 3's
    /// top-level `def`s, read only by a source whose language declares them
    /// ([`ExtensionCalls::top_level_callables`](super::extension_calls::ExtensionCalls::top_level_callables)).
    pub(super) callables_by_fqn: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// Bare name → the top-level functions of the files whose namespace is not
    /// known, by interop family (S-551, `build_unplaced_callables`): PHP's
    /// file declaring `namespace { … }` beside `namespace Foo { … }`. Any one
    /// may be a global function, so each is a rival of a global function of its
    /// name ([`PackageNames::namespace_callables`](super::package_names::PackageNames::namespace_callables)).
    pub(super) unplaced_callables: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// Facade class → the file-root modules whose top-level functions it holds,
    /// by interop family (S-560, `build_file_facades`): Kotlin's `UtilsKt`,
    /// read only by a source whose language declares no facade itself
    /// ([`Ctx::reads_facades`](super::Ctx::reads_facades)) — Java's `UtilsKt.f()`.
    pub(super) facades_by_fqn: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// Declared namespace → the file-root module nodes of the declared-namespace
    /// files declaring it, id-sorted (S-518, [FR-RS-13]) — what a namespace
    /// wildcard (C#'s `using N;`, Kotlin's `import n.*`) binds to: a namespace
    /// has no node of its own, and the files declaring it are the code it names,
    /// as a Go import path names its directory's files (S-439). Empty for every
    /// file of any other module model. Partitioned by interop family first,
    /// as [`types_by_fqn`](super::package_index::PackageIndex::types_by_fqn) is (S-519).
    ///
    /// [FR-RS-13]: ../../../../docs/specs/requirements/FR-RS-13.md
    pub(super) files_by_namespace: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    /// The lexical-constant index (S-565, [FR-RS-33]): a constant's
    /// fully-qualified name → every class and module declaring it, id-sorted,
    /// for each language that names its classes and modules by lexical
    /// constants ([`PackageLayout::resolves_lexical_constants`]: Ruby) —
    /// partitioned by interop family as [`types_by_fqn`](super::package_index::PackageIndex::types_by_fqn)
    /// is. A reopened class is one key with several declarations
    /// (`build_constants`).
    ///
    /// [FR-RS-33]: ../../../../docs/specs/requirements/FR-RS-33.md
    pub(super) constants: FamilyConstants,
    /// Each declaration of [`constants`](super::package_index::PackageIndex::constants) → the constant it
    /// declares: the lexical levels a reference inside it reads (S-565).
    pub(super) constant_of: HashMap<NodeId, Vec<String>>,
    /// Each type's generic arity (S-602, [CR-199]): the type-parameter count
    /// recorded for a type of a language that tells types apart by it
    /// ([`PackageLayout::matches_type_arity`]). What the own-namespace,
    /// enclosing-namespace and namespace-wildcard rungs match a written arity
    /// against ([`PackageNames::admits_arity`](super::package_names::PackageNames::admits_arity)); a type absent from it admits any. Empty
    /// unless the run was given the store's facts ([`IndexBuilder::with_type_arities`](super::IndexBuilder::with_type_arities)).
    ///
    /// [CR-199]: ../../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    pub(super) type_arities: HashMap<NodeId, u32>,
}

impl PackageIndex {
    /// The top-level package-shaped type nodes declared under the
    /// fully-qualified name `fqn`, id-sorted — empty when no file of this graph
    /// declares it, two or more when several do (a `src/main` and a `src/test`
    /// declaration). The index the import rungs read ([CR-149]), exposed so the
    /// framework pass finds a constant's declaring type the same way (S-470)
    /// rather than deriving a second FQN from a path.
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    ///
    /// Read within the interop family `layout` — the index's own,
    /// [`Index::layout`](super::Index::layout) — gives the file at
    /// `source_path` (S-519): a type of another family is never the type a
    /// source names.
    pub(crate) fn package_types(&self, layout: &PackageLayout, source_path: &str, fqn: &[String]) -> &[NodeId] {
        layout
            .family(source_path)
            .and_then(|family| self.types_by_fqn.get(&family))
            .and_then(|types| types.get(fqn))
            .map_or(&[], Vec::as_slice)
    }

    /// Every fully-qualified name a top-level package-shaped type of this graph
    /// is declared under, sorted — what a workspace compares across members to
    /// tell a type another member declares from one no member does (S-468,
    /// [CR-150] §3.2 C). Read off the same index the import rungs bind against,
    /// never a second derivation.
    ///
    /// [CR-150]: ../../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    pub(crate) fn declared_type_names(&self) -> Vec<Vec<String>> {
        let mut names: Vec<Vec<String>> = self
            .types_by_fqn
            .values()
            .flat_map(|types| types.keys().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}
