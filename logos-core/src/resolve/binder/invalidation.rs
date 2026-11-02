//! The sync invalidation of the binder's index (S-828, [CR-258]): which ledger
//! rows a sync must re-bind, given the tokens it dirtied and the paths it
//! changed, because what they read may have moved ([`Invalidation`]). Every
//! rule reads other concerns of the index — the node facts, the file scopes,
//! the impl headers, the hierarchy — so the concern is a view borrowing the
//! [`Index`], as each rung borrows the row's `Ctx`; the facts only a sync reads
//! once the index is built are the [`Hierarchy`](super::hierarchy::Hierarchy)'s
//! tokens, families and imported-supertype mark, and the partial types' marked
//! declarations. A rung whose rows read a fact no target spells
//! adds its rule here, and [`Invalidation::ref_affected`] asks it.
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md

use std::collections::HashSet;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, NodeId, NodeKind, ReceiverShape, RefForm};

use super::super::tokens;
use super::{
    proven_twin, Index, LambdaReceiver, Res, LOCAL_IMPORT_ALIAS, MAX_ALIAS_DEPTH, MAX_CONTAINS_DEPTH,
    MAX_DEREF_DEPTH, SELF_TYPE_HEAD,
};

/// The sync rules of one index: which rows a sync re-binds
/// (`resolve::run`, `resolve::is_affected`).
#[derive(Clone, Copy)]
pub(crate) struct Invalidation<'i> {
    pub(super) ix: &'i Index,
}

impl Invalidation<'_> {
    /// Whether a sync that dirtied `dirty` and changed the paths `changed` can
    /// move the type hierarchy a supertype walk climbs (`resolve::run`): it
    /// touched a hierarchy type ([`hierarchy_touched`](Invalidation::hierarchy_touched)),
    /// or it moved the hierarchy while dirtying none of its tokens
    /// ([`Hierarchy::moved_by_descriptor`](super::hierarchy::Hierarchy::moved_by_descriptor)).
    pub(crate) fn hierarchy_moved(&self, dirty: &HashSet<String>, changed: &HashSet<String>) -> bool {
        let hierarchy: &super::hierarchy::Hierarchy = &self.ix.hierarchy;
        self.hierarchy_touched(dirty) || hierarchy.moved_by_descriptor(changed)
    }

    /// Whether `dirty` — the tokens a sync added or removed — names a type a
    /// supertype row of the type hierarchy names (S-468, S-609). A supertype walk that crosses
    /// such a type can change its answer although the calling row spells none
    /// of its names, so the incremental run re-binds every call that can walk
    /// it ([`walks_hierarchy`](Invalidation::walks_hierarchy)) when this holds
    /// (`resolve::is_affected`).
    pub(crate) fn hierarchy_touched(&self, dirty: &HashSet<String>) -> bool {
        let hierarchy: &super::hierarchy::Hierarchy = &self.ix.hierarchy;
        hierarchy.touched(dirty)
    }

    /// Whether a call written in the file at `path` can bind through the type
    /// hierarchy (S-468; S-522): a package-shaped file, or one of an interop
    /// family whose types record an `Extends` the hierarchy holds — a Kotlin
    /// `.kts` script's call can climb a `.kt` class's. A family that records
    /// none — Rust — has no call a moved hierarchy can move.
    pub(crate) fn walks_hierarchy(&self, path: &str) -> bool {
        self.ix.layout.is_package_shaped(path)
            || self.ix
                .layout
                .family(path)
                .is_some_and(|family| self.ix.hierarchy.families.contains(&family))
    }

    /// Whether re-binding `r` could change its outcome given `dirty` — the tokens
    /// a sync added or removed (see [`tokens`]).
    ///
    /// A row is affected when a token of its target lands in `dirty`, or when an
    /// `as`-alias in the row's file rewrites that target's head into a path that
    /// does. Globs need no expansion: a glob-imported call resolves under its own
    /// bare name, already a target token; only renaming aliases route a reference
    /// to a name it does not spell. The alias chase is bounded by
    /// [`MAX_ALIAS_DEPTH`], the same cap [`ScopeHierarchy::resolve_path`](super::scope_hierarchy::ScopeHierarchy::resolve_path) honours, so a
    /// self-referential import cannot loop here either.
    ///
    /// A package-shaped file's globs **do** need their own tokens ([CR-149]): a
    /// wildcard's target is a type or package whose identity can change while
    /// the member name a call spells does not — a second `C` under
    /// `import static a.b.C.*` makes `m()` ambiguous without touching `m`. So a
    /// row of such a file is also affected when a token of any of its globs is
    /// dirty. Every other language's selection is unchanged.
    ///
    /// A `Self::m` row (S-493) is also affected when a token of its caller's
    /// self type is dirty: a second type of that name elsewhere in the crate
    /// moves its binding to the caller's module
    /// ([`SelfTypes::resolve_recorded_self_type_call`](super::self_types::SelfTypes::resolve_recorded_self_type_call)) without touching `m` — and,
    /// in an impl-block language, so are it and a `self.m()` row when a token
    /// of its caller's impl header is (S-607, [`SelfTypes::self_type_call`](super::self_types::SelfTypes::self_type_call)).
    ///
    /// A call through a TypeScript-family import followed through re-exports
    /// or a default-export alias (S-534, [`ModuleIndex::exported_definitions`](super::module_index::ModuleIndex::exported_definitions))
    /// needs no clause here: it is a `<import target>::<name>` row of a
    /// path-specifier file, which `resolve::is_affected` re-selects on every
    /// non-empty delta before this is asked, as it does every `Imports` row
    /// — the export rows included — so a barrel, an alias or a definition
    /// behind them changing in another file re-binds it. A call through an
    /// imported class, `<import target>::<Class>::<method>` (S-536,
    /// [`ImportedScope::imported_class_members`](super::imported_scope::ImportedScope::imported_class_members)), is such a row too: a method added to
    /// or removed from the class in another file re-binds it.
    ///
    /// A row sourced inside a partial type's part (S-557) is also affected when
    /// a token of that type's name is dirty ([`inside_dirty_partial`](super::Invalidation::inside_dirty_partial)):
    /// it binds through the other parts, which its target need not spell.
    ///
    /// A lexical-constant row (S-565, [`LexicalConstants::resolve_constant_row`](super::lexical_constants::LexicalConstants::resolve_constant_row)) needs no
    /// clause of its own: the constant rung reads the declarations its target
    /// spells — each level's `Foo`, the member `bar` — and the source's own
    /// nesting, which lives in the source's file. A nearer `Foo`, a reopening,
    /// a compact `class A::Foo`, or `def self.bar` turning into `def bar`, each
    /// dirties a token the target spells.
    ///
    /// A Ruby call along its class's ancestors (S-568, [`SupertypeClimb::ancestor_walk`](super::supertype_climb::SupertypeClimb::ancestor_walk))
    /// spells the method it reaches; what else the walk reads is the type
    /// hierarchy, which `resolve::is_affected` re-selects every Ruby call for
    /// once a sync dirties one of its tokens ([`Invalidation::hierarchy_moved`](super::Invalidation::hierarchy_moved)) —
    /// every superclass's and mixed-in module's name, and the name of every
    /// class or module that records either. A reopening in another file that
    /// takes away the caller's class's last superclass or mixin dirties that
    /// class's name, which the hierarchy no longer holds, so the call is also
    /// affected when a token of a class or module enclosing its caller is dirty
    /// ([`inside_dirty_constant`](super::Invalidation::inside_dirty_constant)). A superclass
    /// or mixin row itself spells the constant it binds through the constant
    /// index, as every constant row does.
    ///
    /// Nor does a qualified call of an import-root file (S-544,
    /// [`ImportRootPaths::resolve_module_qualified_call`](super::import_roots::ImportRootPaths::resolve_module_qualified_call)): an extracted row's qualifier is
    /// the import path it spells, read down the module tree segment by segment
    /// and among its parent's members by its last segment, and a written one
    /// (a framework handler's `views::add`) goes through the file's imports,
    /// which the alias chase below re-selects; its member is looked up by the
    /// name the target ends in, among the members of the module or class it
    /// reached. A module or member renamed, added or removed, or a class turned
    /// into a function, dirties a token the target spells.
    ///
    /// Nor does a Go call bound within its directory package (S-528,
    /// [`ScopeHierarchy::directory_package_name`](super::scope_hierarchy::ScopeHierarchy::directory_package_name)): the rung reads the declarations of the
    /// name the target spells among the files of the source's directory that
    /// share its `package` clause. A declaration of the name added, removed or
    /// duplicated in another file, and a file of the directory changing its
    /// clause — the sync dirties every node name of a file it re-extracts,
    /// before and after, and the clause itself — each dirties a token the
    /// target spells; a change to the source's own clause re-extracts the
    /// source's file, whose rows are re-bound whatever they spell.
    ///
    /// Nor do Scala's companion pairs and top-level `def`s (S-573,
    /// [`Companions::walk_from_companions`](super::companions::Companions::walk_from_companions), `build_top_level_callables`): a row through a
    /// pair spells the pair's name and the member it reaches, and one reaching a
    /// top-level `def` spells the `def`'s name. A companion object added or
    /// removed, a member moved between the class and the object, or a `def`
    /// added at the top level, each dirties a token the target spells. So does
    /// what a supertype row reads of a pair (S-575, [`Companions::pair_supertype`](super::companions::Companions::pair_supertype)):
    /// the trait and its object are declared in one file under the name the row
    /// spells, so a sync that adds, removes or re-kinds either dirties it.
    ///
    /// Nor do Kotlin's calls (S-560): a free call to a top-level function, an
    /// extension call and a call on a class each spell the name they bind, and
    /// a member of that name that rules an extension out spells it too. A Java
    /// row through a file facade (`mixit::util::UtilsKt`, or `UtilsKt::f`
    /// beside that import) names the class after its file, which no node
    /// spells — but its path spells the file's declared package, which a sync
    /// that extracts or deletes the file dirties (S-518), and its member the
    /// function's own name.
    ///
    /// Nor do Kotlin's companion members (I4-INT, [`Companions::companion_of`](super::companions::Companions::companion_of)): a
    /// call on a class (`Ticket::empty`) and a bare call inside it each spell
    /// the member they reach, and a companion is declared in its class's file,
    /// whose node names a sync dirties before and after — the companion added
    /// or removed, a member moved between it and the class. A supertype in
    /// another file gaining the name, which a bare call reads before the
    /// companion, spells that name too, and so does a superclass's companion,
    /// declared in that superclass's file.
    ///
    /// Nor does a TypeScript call on a proven receiver (S-537,
    /// [`ProvenReceivers::resolve_structural_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_structural_receiver_call)) need a rule here: its target
    /// spells `::`, so `resolve::is_import_scoped` re-selects it on every sync
    /// — whatever moves its type's import, its members, its required
    /// signatures or its `Extends` chain.
    ///
    /// Nor does a Python call on a proven receiver (S-546,
    /// [`ProvenReceivers::resolve_import_root_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_import_root_receiver_call)): its `T::m` is read as a
    /// written qualifier is, so the alias chase below re-selects it when an
    /// import of `T` moves; `T` declared, renamed or re-kinded — in the
    /// caller's own file too, where an import of `T` beside it makes the row
    /// ambiguous — and `m` added or removed on `T` or a base, as a method or as
    /// an attribute that hides one, each dirties a token the target spells;
    /// and a base gained or lost moves the hierarchy, which re-selects every
    /// Python call (`resolve::is_affected`'s hierarchy rule).
    ///
    /// Nor does a Kotlin call on a proven receiver (S-563,
    /// [`ProvenReceivers::resolve_companion_language_receiver_call`](super::proven_receivers::ProvenReceivers::resolve_companion_language_receiver_call)): its `T::m` spells
    /// both names, so `T` declared, renamed, re-kinded or imported elsewhere
    /// — through the alias chase and wildcard rule below, Kotlin being
    /// package-shaped — and a top-level function `T` added or removed
    /// ([`ExtensionCalls::factory_named`](super::extension_calls::ExtensionCalls::factory_named)) each dirty a token it spells; `m` added or
    /// removed on `T` or a base, and an extension `m` the fallback reaches
    /// ([`ExtensionCalls::resolve_extension_call`](super::extension_calls::ExtensionCalls::resolve_extension_call)), dirty `m`; and a supertype gained or
    /// lost moves the hierarchy (`resolve::is_affected`'s hierarchy rule).
    /// What an extension call reads of its extension's receiver type (S-833)
    /// — a class of that name declared, for an unproven receiver or a bare
    /// call inside the class — is a name the row does not spell, so its own
    /// rule selects it ([`reads_extension_receiver`](Invalidation::reads_extension_receiver)).
    /// A bare call inside a lambda whose receiver extraction proved (S-834)
    /// spells only `m`, its type `T` riding on its alias: it is selected as
    /// its proven twin `T::m` is ([`reads_lambda_receiver`](Invalidation::reads_lambda_receiver)), and what it falls
    /// through to is the bare call its own target spells.
    ///
    /// Nor do PHP's global functions (S-551, [`PackageNames::global_function`](super::package_names::PackageNames::global_function)): a bare
    /// call from a file of the global namespace, and one from a namespaced file
    /// its namespace declares no function for, each spell the function's name,
    /// so a global function added, removed or declared twice in another file
    /// (a file of unknown namespace declaring it included, [`PackageNames::namespace_callables`](super::package_names::PackageNames::namespace_callables)),
    /// a file gaining or dropping its `namespace` — the sync dirties every node
    /// name of a file it re-extracts, before and after — and the namespace
    /// gaining a function of the name, each dirties a token the target spells;
    /// a change to the caller's own namespace re-extracts the caller's file.
    /// Nor does a PHP include (`require 'lib/a.php'`, recorded `.::lib::a.php`),
    /// which binds the file it names whole: the file's module is named its
    /// stem, a token the target spells, which the sync dirties when the file is
    /// added or removed.
    ///
    /// Nor does the arity filter of the namespace rungs (S-602,
    /// [`PackageNames::admits_arity`](super::package_names::PackageNames::admits_arity)): a row spells the name of the type it writes,
    /// and a type of that name gaining, losing or changing its type
    /// parameters is re-extracted in its own file, whose node names the sync
    /// dirties before and after; a row's own written arity changes only with
    /// its file, whose rows are re-bound whatever they spell.
    ///
    /// A namespace block's wildcards (S-603, [`FileScope::block_globs`](super::FileScope::block_globs)) join
    /// the file's wildcards below: a row reads them at their level as it reads
    /// the file's after every level, so a sync dirtying a token one spells
    /// selects it. A `using` moved into or out of a block changes only its own
    /// file, whose rows are re-bound whatever they spell.
    ///
    /// A row of an include-roots language (S-579,
    /// [`Invalidation::reads_include_scope`](super::Invalidation::reads_include_scope)) is always affected: an `#include` reads
    /// the include roots and the files beside it, a call, a `using` or a base
    /// class reads every file the includes reach, transitively, and a `this->`
    /// call or an access reads the hierarchy those bases make — a header two
    /// includes away gaining a declaration, or an include another file gains,
    /// moves it under no name its target spells. C's link-unit rung (S-584,
    /// `link_unit`) needs no rule of its own for the same reason: a C row is an
    /// include-roots row, so a definition gained, lost or made `static` in any
    /// file re-binds it.
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    pub(crate) fn ref_affected(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        if tokens(&r.target).iter().any(|t| dirty.contains(t)) {
            return true;
        }
        // A `Self::m` call (S-493) also reads how many types its caller's self
        // type names in the crate, a name its target does not spell — and, in
        // an impl-block language, what its caller's impl header resolves to
        // (S-607): the header's self type and trait, which a `self.m()` left
        // unrewritten spells no more than `Self::m` does.
        // A receiver read (S-529) reads it too: the type its field binds among.
        let through_self = r.target.split_once("::").is_some_and(|(head, _)| head == SELF_TYPE_HEAD)
            || (r.form == RefForm::Method && r.receiver == Some(ReceiverShape::SelfInstance))
            || r.kind == EdgeKind::Accesses;
        if through_self {
            let source = self.ix.by_symbol.get(&r.source_symbol);
            let self_type = source.and_then(|id| self.ix.facts.self_types.get(id));
            let header = source
                .and_then(|id| self.ix.impls.assoc_owner.get(id))
                .map(|&h| &self.ix.impls.impl_headers[h]);
            let spelt = self_type
                .into_iter()
                .chain(header.map(|h| &h.self_path))
                .chain(header.and_then(|h| h.trait_path.as_ref()));
            if spelt.flat_map(|text| tokens(text)).any(|t| dirty.contains(&t)) {
                return true;
            }
        }
        if self.inside_dirty_partial(r, dirty) || self.inside_dirty_constant(r, dirty) {
            return true;
        }
        if self.reads_extension_receiver(r, dirty) {
            return true;
        }
        if self.reads_lambda_receiver(r, dirty) {
            return true;
        }
        if self.reads_include_scope(r) {
            return true;
        }
        let Some(file_id) = r.file_id else {
            return false;
        };
        let Some(scope) = self.ix.modules.file_scopes.get(&file_id) else {
            return false;
        };
        let package_shaped = || {
            self.ix.by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.ix.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.ix.layout.is_package_shaped(p))
        };
        // A block's wildcards (S-603) are read as the file's are, at a level.
        if !(scope.globs.is_empty() && scope.static_globs.is_empty() && scope.block_globs.is_empty())
            && scope
                .globs
                .iter()
                .chain(&scope.static_globs)
                .chain(scope.block_globs.values().flatten())
                .flatten()
                .flat_map(|seg| tokens(seg))
                .any(|t| dirty.contains(&t))
            && package_shaped()
        {
            return true;
        }
        // A package-shaped row reads every expansion of its head, not only the
        // first (`FileScope::alias_expansions`), so each must be able to select
        // it — and so does an import-root file's row (S-519), and a call through
        // a local import in any language (S-594, `LocalBindings::resolve_local_import_call`).
        let first = r.target.split("::").next().unwrap_or_default();
        let import_root = || {
            self.ix.by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.ix.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.ix.layout.has_import_roots(p))
        };
        if scope
            .alias_expansions
            .get(first)
            .is_some_and(|all| all.len() > 1)
            && scope.alias_expansions[first]
                .iter()
                .flatten()
                .flat_map(|seg| tokens(seg))
                .any(|t| dirty.contains(&t))
            && (package_shaped() || import_root() || r.alias.as_deref() == Some(LOCAL_IMPORT_ALIAS))
        {
            return true;
        }
        // Chase the head segment through `as`-alias rewrites: each hop's path may
        // name something dirty even though the written target does not.
        let mut head = first.to_string();
        for _ in 0..MAX_ALIAS_DEPTH {
            let Some(path) = scope.aliases.get(&head) else {
                return false;
            };
            if path
                .iter()
                .flat_map(|seg| tokens(seg))
                .any(|t| dirty.contains(&t))
            {
                return true;
            }
            let Some(next) = path.first() else {
                return false;
            };
            head = next.clone();
        }
        false
    }

    /// Whether `r` is sourced inside a declaration its plugin marks partial
    /// (S-557, [`PartialTypes::marked`](super::PartialTypes::marked)), itself included, whose name a token of
    /// `dirty` spells. What a row inside a part binds reads the type's other
    /// parts — their members, their supertypes, which part is first — and a
    /// change to any part spells the type's name, since every part declares
    /// it, while the row's own target need not: a call to a method of another
    /// part names the method alone.
    fn inside_dirty_partial(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        if self.ix.partials.marked.is_empty() {
            return false;
        }
        let mut cursor = self.ix.by_symbol.get(&r.source_symbol).copied();
        for _ in 0..MAX_CONTAINS_DEPTH {
            let Some(id) = cursor else {
                return false;
            };
            if self.ix.partials.marked.contains_key(&id)
                && self.ix.info.get(&id).is_some_and(|i| tokens(&i.name).iter().any(|t| dirty.contains(t)))
            {
                return true;
            }
            cursor = self.ix.parent.get(&id).copied();
        }
        false
    }

    /// Whether `r` is a call on the instance in a lexical-constant file (Ruby,
    /// S-568) whose caller sits in a class or module whose name a token of
    /// `dirty` spells — a sync re-extracting any declaration of it, whose
    /// superclass and mixins the [`SupertypeClimb::ancestor_walk`](super::supertype_climb::SupertypeClimb::ancestor_walk) pools, dirties that
    /// name, before and after.
    fn inside_dirty_constant(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        if r.kind != EdgeKind::Calls || !matches!(r.receiver, Some(ReceiverShape::SelfInstance | ReceiverShape::Super)) {
            return false;
        }
        let mut cursor = self.ix.by_symbol.get(&r.source_symbol).copied();
        let path = cursor.and_then(|id| self.ix.info.get(&id)).and_then(|i| i.file_path.as_deref());
        if !path.is_some_and(|p| self.ix.layout.resolves_lexical_constants(p)) {
            return false;
        }
        for _ in 0..MAX_CONTAINS_DEPTH {
            let Some(id) = cursor else {
                return false;
            };
            if self.ix.info.get(&id).is_some_and(|i| {
                matches!(i.kind, NodeKind::Class | NodeKind::Module) && tokens(&i.name).iter().any(|t| dirty.contains(t))
            }) {
                return true;
            }
            cursor = self.ix.parent.get(&id).copied();
        }
        false
    }

    /// The names of the impl functions whose block header a token of `dirty`
    /// spells — its self type or its trait, as written, or its trait's name —
    /// beyond `dirty` (S-607). Which calls such a function answers moves with
    /// what its header resolves to, and with where its trait is in scope, both
    /// read through other files' `use`s: a `pub use` retargeted or removed in a
    /// third file spells the header's name and never the call's. Read beside
    /// [`ref_affected`](super::Invalidation::ref_affected) for every `Calls` row
    /// (`resolve::run`), as the [`renamed_import_tokens`](super::Invalidation::renamed_import_tokens)
    /// precedent is.
    ///
    /// So are the impl functions of every type a `Deref` chain reaches from a
    /// type the change spells, and the names of the trait methods those types'
    /// blocks lend them (S-608): a method call on a type whose chain crosses
    /// the change — a `Deref` impl added or removed — may now reach, or no
    /// longer reach, a method of a type further down that neither the call nor
    /// the change spells.
    ///
    /// And the names of every type whose `Deref` chain reaches a type the
    /// change spells: a removed hop or a removed impl further down leaves
    /// nothing in this (post-change) index that lends the method, so the call
    /// on the type at the top of the chain is selected by that type's name.
    pub(crate) fn impl_header_tokens(&self, dirty: &HashSet<String>) -> HashSet<String> {
        let spells = |text: &str| tokens(text).iter().any(|t| dirty.contains(t));
        let seed: Vec<NodeId> = self.ix
            .impls.deref_targets
            .iter()
            .flat_map(|(&ty, target)| [Some(ty), if let Res::Found(t) = target { Some(*t) } else { None }])
            .flatten()
            .filter(|ty| self.ix.info.get(ty).is_some_and(|i| spells(&i.name)))
            .collect();
        let mut upstream: HashSet<NodeId> = seed.iter().copied().collect();
        for _ in 0..=MAX_DEREF_DEPTH {
            let next: Vec<NodeId> = self.ix
                .impls.deref_targets
                .iter()
                .filter(|(ty, t)| !upstream.contains(*ty) && matches!(t, Res::Found(x) if upstream.contains(x)))
                .map(|(&ty, _)| ty)
                .collect();
            if next.is_empty() {
                break;
            }
            upstream.extend(next);
        }
        let upstream_names = upstream
            .iter()
            .filter_map(|ty| self.ix.info.get(ty).map(|i| i.name.clone()))
            .flat_map(|name| tokens(&name))
            .filter(|t| !dirty.contains(t));
        let mut downstream: HashSet<NodeId> = HashSet::new();
        let mut level = seed;
        for _ in 0..=MAX_DEREF_DEPTH {
            level.retain(|&ty| downstream.insert(ty));
            level = level
                .iter()
                .filter_map(|ty| match self.ix.impls.deref_targets.get(ty) {
                    Some(&Res::Found(next)) => Some(next),
                    _ => None,
                })
                .collect();
        }
        let moved: HashSet<usize> = self.ix
            .impls.impl_headers
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                spells(&h.self_path)
                    || h.trait_path.as_deref().is_some_and(spells)
                    || h.trait_node.and_then(|t| self.ix.info.get(&t)).is_some_and(|i| spells(&i.name))
                    || matches!(h.self_type, Res::Found(ty) if downstream.contains(&ty))
            })
            .map(|(at, _)| at)
            .collect();
        if moved.is_empty() {
            return upstream_names.collect();
        }
        let lent = moved
            .iter()
            .filter(|&&at| matches!(self.ix.impls.impl_headers[at].self_type, Res::Found(ty) if downstream.contains(&ty)))
            .filter_map(|&at| self.ix.impls.impl_headers[at].trait_node)
            .flat_map(|tr| self.ix.members.get(&tr).into_iter().flat_map(|m| m.keys()));
        self.ix.impls.assoc_owner
            .iter()
            .filter(|(_, at)| moved.contains(at))
            .filter_map(|(f, _)| self.ix.info.get(f).map(|i| &i.name))
            .chain(lent)
            .flat_map(|name| tokens(name))
            .filter(|t| !dirty.contains(t))
            .chain(upstream_names)
            .collect()
    }

    /// The tokens a proven Rust receiver's row (S-588) can read through
    /// another module's renaming import, beyond `dirty`: the name of every
    /// `use … as N` (in any file) whose imported name a token of `dirty` — or
    /// of a rename already added — spells, [`MAX_ALIAS_DEPTH`] hops at most.
    ///
    /// [`RustGlobs::reexported_type`](super::rust_globs::RustGlobs::reexported_type) follows any module's import, so with
    /// `pub use crate::inner::Store as Db;` a proven `Db::get` row binds to
    /// `Store`'s method, and a change that spells only `Store` — a second
    /// `Store` in the crate, the next re-export hop retargeted — moves it.
    /// Neither the row's target nor its own file's imports spell `store`, so
    /// [`ref_affected`](super::Invalidation::ref_affected) alone would not select it. Read
    /// only for those rows (`resolve::run`): every other row's selection is
    /// unchanged.
    pub(crate) fn renamed_import_tokens(&self, dirty: &HashSet<String>) -> HashSet<String> {
        let mut extra: HashSet<String> = HashSet::new();
        for _ in 0..MAX_ALIAS_DEPTH {
            let before = extra.len();
            for (name, paths) in self.ix.modules.file_scopes.values().flat_map(|s| &s.alias_expansions) {
                if renames_a_dirty_name(name, paths, dirty, &extra) {
                    extra.extend(tokens(name).into_iter().filter(|t| !dirty.contains(t)));
                }
            }
            if extra.len() == before {
                break;
            }
        }
        extra
    }
    /// Whether `r` binds through the include scope (S-579,
    /// [`ref_affected`](Invalidation::ref_affected)): any row of an
    /// include-roots file but a capture-before-delete one. An `#include`, a
    /// `using`, a Path-form call and a base read the include roots, the files
    /// the file's includes reach, and theirs; a `this->` or in-class call and a
    /// member access read the supertype hierarchy those bases make — none of
    /// which the row's target spells — and a call reads the
    /// prototype/definition pairs and the parameter shapes of the files those
    /// include (S-820, [`super::prototype_pairs`]). So it is re-bound on every
    /// non-empty delta, the stance the path-specifier fallback takes for an
    /// import.
    fn reads_include_scope(&self, r: &UnresolvedRefRow) -> bool {
        r.form != RefForm::Symbol
            && self
                .ix
                .by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.ix.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.ix.layout.resolves_includes(p))
    }

    /// Whether a sync moved what an extension call reads beyond the names its
    /// target spells (S-833, [FR-RS-31]): a call reaches `fun R.m()` only through
    /// a receiver that can be an `R` — an unproven receiver only once a class `R`
    /// is declared, a bare call only inside an `R` or a subtype — so a sync that
    /// declares, removes or re-kinds a type named `R` re-selects every row of an
    /// extension-calls file whose target ends in `m`
    /// ([`ref_affected`](Invalidation::ref_affected)).
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    fn reads_extension_receiver(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        let ix: &Index = self.ix;
        // The language test first: `self_types` also holds other languages'
        // method receivers, which this rule never reads.
        let binds = ix.by_symbol.get(&r.source_symbol).and_then(|&id| ix.file_of(id));
        if !binds.is_some_and(|p| ix.layout.binds_extension_calls(p)) {
            return false;
        }
        let name = r.target.rsplit("::").next().unwrap_or_default();
        ix.by_name
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|id| ix.facts.self_types.get(id))
            .flat_map(|ty| tokens(ty))
            .any(|t| dirty.contains(&t))
    }

    /// Whether a sync moved what a proven receiver lambda's bare call reads past
    /// its own target (S-834, [`ref_affected`](Invalidation::ref_affected)):
    /// what its proven twin reads — `T`, through the file's imports, and the
    /// extensions of the name.
    fn reads_lambda_receiver(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        match r.alias.as_deref().and_then(LambdaReceiver::of) {
            Some(LambdaReceiver { ty: Some(ty), .. }) => self.ref_affected(&proven_twin(r, ty), dirty),
            _ => false,
        }
    }
}

/// Whether the alias `name`, with the import `paths` it expands to, renames a
/// name a token of `dirty` or of `extra` spells — an imported name other than
/// `name` itself ([`Invalidation::renamed_import_tokens`]).
fn renames_a_dirty_name(name: &str, paths: &[Vec<String>], dirty: &HashSet<String>, extra: &HashSet<String>) -> bool {
    paths
        .iter()
        .filter_map(|p| p.last())
        .filter(|last| *last != name)
        .any(|imported| tokens(imported).iter().any(|t| dirty.contains(t) || extra.contains(t)))
}
