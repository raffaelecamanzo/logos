//! The builder of the binder's lookup index (S-828, [CR-258]): one snapshot
//! digested into an [`Index`] ([`IndexBuilder::new`]), then each fact the
//! store records beside the snapshot given by a `with_*` step, and the index
//! yielded ([`IndexBuilder::build`]) — immutable from there on, so one
//! resolution run binds every row against it in parallel. A step that binds
//! (the hierarchy, the impl headers, the imported scope) reads the index as
//! built so far, so the order of the steps is the order the passes give them
//! in. The sub-builders below each derive one lookup structure.
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::config::BindingPolicy;
use crate::graph_store::{
    EdgeRow, ImplBlockRow, LocalBindingRow, NodeArity, NodeItemFacts, NodeRow, UnresolvedRefRow,
};
use crate::model::{EdgeKind, NodeId, NodeKind, RefForm};

use super::super::go_module::GoModule;
use super::super::load_path::LoadPath;
use super::super::package_key::{constant_fqn, PackageLayout};
use super::super::rel_path::extension_of;
use super::super::route_template::route_key;
use super::super::ts_config::TsResolution;
use super::hierarchy::{Hierarchy, Mixins};
use super::impl_index::{ImplHeader, ImplIndex};
use super::includes::IncludeScope;
use super::module_index::{ModuleExports, ModuleIndex};
use super::package_index::{FamilyConstants, PackageIndex};
use super::prototype_pairs::PrototypePairs;
use super::self_types::SelfTypes;
use super::declaration_facts::DeclarationFacts;
use super::{
    bind, is_class_like, is_top_level_term, is_type_like, relation_want_of, split, ts_export_of, unique_trait, Ctx,
    FileScope, Index, Members, ModKey, NodeInfo, Outcome, PartialTypes, Res, TsExport, Want, MAX_CONTAINS_DEPTH,
    MIXIN_EXTEND_ALIAS, MIXIN_PREPEND_ALIAS, PARTIAL_TYPE_ALIAS, STATIC_WILDCARD_ALIAS, TRAIT_USE_ALIAS,
};

/// The lookup index of one resolution run, being built: what
/// [`IndexBuilder::new`] digested of the snapshot, and each fact a step gave
/// it since.
pub(crate) struct IndexBuilder {
    ix: Index,
}

impl IndexBuilder {
    /// Digest a snapshot into the binding index, every file keyed by `layout`
    /// — in production the layout the loaded plugins declare
    /// ([`PackageLayout::from_registry`]): a package-shaped file by its package
    /// ([CR-149]), a declared-namespace file by its namespace, a path-model file
    /// by its path under its plugin's stems and import roots, the roots
    /// detected here from this graph's files (S-519). The index's own build
    /// step; every other step is a `with_*` of this builder, and
    /// [`build`](IndexBuilder::build) yields the index.
    ///
    /// A thin orchestrator over focused sub-builders, each owning one lookup
    /// structure. Ordering is load-bearing: `members` lists and `by_name`
    /// lists are id-sorted, `by_symbol` is first-wins on a duplicate key, and
    /// no node id decides which module holds a key two share — file roots go
    /// in path order, inline modules before files (S-751) — so a store a sync
    /// renumbered builds the `Index` a cold index does ([NFR-RA-06]).
    ///
    /// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    pub(crate) fn new(
        nodes: &[NodeRow],
        edges: &[EdgeRow],
        refs: &[UnresolvedRefRow],
        layout: PackageLayout,
    ) -> IndexBuilder {
        // Built once and shared: module-tree construction and containment both
        // look nodes up by id.
        let node_by_id: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();
        // The import roots in force are read off the files this graph holds
        // (S-519), so a cold index and a sync over one tree key alike.
        let layout = layout.with_detected_import_roots(nodes.iter().filter_map(|n| {
            (n.kind == NodeKind::Module).then_some(n.file_path.as_deref()).flatten()
        }));

        // Contains topology first — module-tree construction needs it.
        let (parent, members) = build_containment(edges, &node_by_id);
        let (modules, module_key) =
            build_module_tree(nodes, &parent, &members, &node_by_id, &layout);
        let crates: HashSet<String> = modules.keys().map(|(c, _)| c.clone()).collect();
        let dir_modules = build_directory_modules(nodes, &parent, &layout);

        let info = build_node_info(nodes, &parent, &module_key, &layout);
        let mut types_by_fqn = build_package_types(nodes, &parent, &members, &node_by_id, &layout);
        let callables_by_fqn = build_top_level_callables(nodes, &parent, &members, &node_by_id, &layout);
        let unplaced_callables = build_unplaced_callables(nodes, &parent, &members, &node_by_id, &layout);
        let facades_by_fqn = build_file_facades(nodes, &parent, &layout);
        let files_by_namespace = build_namespace_files(nodes, &parent, &layout);
        let by_symbol = build_by_symbol(nodes);
        let ConstantIndex {
            constants,
            constant_of,
        } = build_constants(nodes, &parent, &by_symbol, refs, &layout);
        let mut by_name = build_by_name(nodes);
        // The parts of a partial type are one type (S-557): its name names the
        // first-declared part, wherever the parts sit.
        let partials = build_partial_types(refs, &by_symbol, &parent, &members, &types_by_fqn, &node_by_id);
        partials.collapse(types_by_fqn.values_mut().flat_map(HashMap::values_mut));
        partials.collapse(by_name.values_mut());
        let by_file_path = build_by_file_path(nodes);
        let (file_roots_by_stem, file_roots_by_dir) = build_file_roots(nodes, &parent);
        let routes_by_template = build_routes_by_template(nodes);
        let file_scopes = build_file_scopes(refs);
        let impls_by_trait_method =
            build_impls_by_trait_method(refs, &by_symbol, &by_name, &info, &layout);
        let trait_methods = build_trait_methods(nodes, edges, &info);

        let mut index = Index {
            by_symbol,
            info,
            parent,
            members,
            by_name,
            partials,
            layout,
            modules: ModuleIndex {
                by_key: modules,
                dir_modules,
                module_key,
                crates,
                by_file_path,
                file_roots_by_stem,
                file_roots_by_dir,
                routes_by_template,
                file_scopes,
                ..ModuleIndex::default()
            },
            packages: PackageIndex {
                types_by_fqn,
                callables_by_fqn,
                unplaced_callables,
                facades_by_fqn,
                files_by_namespace,
                constants,
                constant_of,
                type_arities: HashMap::new(),
            },
            hierarchy: Hierarchy::default(),
            impls: ImplIndex {
                impls_by_trait_method,
                trait_methods,
                ..ImplIndex::default()
            },
            facts: DeclarationFacts::default(),
            includes: IncludeScope::default(),
            pairs: PrototypePairs::default(),
        };
        index.includes = IncludeScope::build(nodes, refs, &index);
        let mut hierarchy = build_supertypes(refs, &index);
        let partials: &PartialTypes = &index.partials;
        partials.unite_hierarchy(&mut hierarchy, &node_by_id);
        let installed: &mut Hierarchy = &mut index.hierarchy;
        installed.install(hierarchy, &HashSet::new());
        // A capture-before-delete row (`Symbol`) is filed under its *target's*
        // file while its source sits in another (ADR-10): it never says which
        // file its source's path is, or a sync would read that file's scope
        // from the synced file's rows where a cold index — which holds no
        // capture row — reads its own (S-588).
        index.modules.file_ids = refs
            .iter()
            .filter(|r| r.form != RefForm::Symbol)
            .filter_map(|r| {
                let file_id = r.file_id?;
                let path = index.by_symbol.get(&r.source_symbol).and_then(|id| index.info.get(id))?;
                Some((path.file_path.clone()?, file_id))
            })
            .collect();
        let modules: &mut ModuleIndex = &mut index.modules;
        modules.apply_global_globs(refs, &index.by_symbol, &index.info, &index.layout);
        IndexBuilder { ix: index }
    }

    /// Declare which files write their import specifiers as paths, and the Go
    /// modules a Go import path is anchored on (S-439).
    pub(crate) fn with_path_specifiers(
        mut self,
        specifier_targets: HashMap<String, HashSet<String>>,
        go_modules: Vec<GoModule>,
    ) -> IndexBuilder {
        self.ix.modules.specifier_targets = specifier_targets;
        self.ix.modules.go_modules = go_modules;
        self
    }

    /// Declare the configs and workspace packages a bare specifier of the
    /// TypeScript family binds through (S-531). Call before
    /// [`IndexBuilder::with_imported_bindings`](super::IndexBuilder::with_imported_bindings), whose scope reads the import rows'
    /// outcomes.
    pub(crate) fn with_ts_resolution(mut self, ts: TsResolution) -> IndexBuilder {
        self.ix.modules.ts = ts;
        self
    }

    /// Declare the load roots a bare specifier of a load-path language binds
    /// against (S-566). Call before [`IndexBuilder::with_imported_bindings`](super::IndexBuilder::with_imported_bindings), whose
    /// scope reads the import rows' outcomes.
    pub(crate) fn with_load_path(mut self, load_path: LoadPath) -> IndexBuilder {
        self.ix.modules.load_path = load_path;
        self
    }

    /// Give the index the callables an implementing type never inherits from
    /// an interface (S-609, [FR-RS-48];
    /// [`GraphStore::uninherited_members`]): what a supertype walk never binds
    /// at an interface level.
    ///
    /// [FR-RS-48]: ../../../../docs/specs/requirements/FR-RS-48.md
    /// [`GraphStore::uninherited_members`]: crate::graph_store::GraphStore::uninherited_members
    pub(crate) fn with_uninherited(mut self, ids: impl IntoIterator<Item = NodeId>) -> IndexBuilder {
        self.ix.facts.uninherited = ids.into_iter().collect();
        self
    }

    /// Give the index the link-unit facts (S-584, [FR-RS-40]): the unexported
    /// functions ([`GraphStore::unexported_functions`]), a link-unit file's
    /// being what the link-unit rung never offers another file — `None`, a
    /// store recording no linkage or a run whose languages make no link unit,
    /// leaves the rung reading nothing — and the link-unit files' local
    /// bindings (the S-593 facts [`GraphStore::local_bindings_with_extensions`]
    /// reads), a name one binds at a call's line shadowing every function the
    /// rung could bind.
    ///
    /// [FR-RS-40]: ../../../../docs/specs/requirements/FR-RS-40.md
    /// [`GraphStore::unexported_functions`]: crate::graph_store::GraphStore::unexported_functions
    /// [`GraphStore::local_bindings_with_extensions`]: crate::graph_store::GraphStore::local_bindings_with_extensions
    pub(crate) fn with_link_unit(mut self, file_local: Option<Vec<NodeId>>, locals: Vec<LocalBindingRow>) -> IndexBuilder {
        self.ix.facts.file_local = file_local.map(|ids| ids.into_iter().collect());
        for b in locals {
            let scope = (b.line, b.scope_start_line, b.scope_end_line);
            self.ix.facts.link_unit_locals.entry((b.file_path, b.name)).or_default().push(scope);
        }
        self
    }

    /// Give the index each type's generic arity (S-602, [CR-199];
    /// [`GraphStore::type_arities`]): what the namespace rungs match a written
    /// type's arity against ([`PackageNames::admits_arity`](super::package_names::PackageNames::admits_arity)). An index given none (the
    /// framework pass) admits every type of a name, as before.
    ///
    /// The supertype walk is then read again for every type of a language that
    /// tells types apart by arity ([`PackageLayout::matches_type_arity`]), from
    /// `refs`' supertype rows: the [`Hierarchy`]
    /// [`new`](IndexBuilder::new) installed bound them
    /// with no arity known, so `class Plain : Result` would climb to the
    /// `Result<T>` its `Extends` edge does not name. Each such type's entries
    /// are replaced whole — partial parts united again, `nodes` naming their
    /// kinds — as [`with_structural_supertypes`](super::IndexBuilder::with_structural_supertypes)
    /// replaces a structural type's, so every other language's hierarchy is
    /// untouched. Read once the file scopes are complete, the walk binds each
    /// supertype as its edge does, through the global namespace wildcards too.
    ///
    /// [CR-199]: ../../../../docs/requests/CR-199-csharp-resolution-honours-type-arity-and-using-scope.md
    /// [`GraphStore::type_arities`]: crate::graph_store::GraphStore::type_arities
    pub(crate) fn with_type_arities(
        mut self,
        arities: impl IntoIterator<Item = (NodeId, u32)>,
        nodes: &[NodeRow],
        refs: &[UnresolvedRefRow],
    ) -> IndexBuilder {
        self.ix.packages.type_arities = arities.into_iter().collect();
        if self.ix.packages.type_arities.is_empty() {
            return self;
        }
        let matches = |id: &NodeId| {
            let path = self.ix.info.get(id).and_then(|i| i.file_path.as_deref());
            path.is_some_and(|p| self.ix.layout.matches_type_arity(p))
        };
        let replacing: HashSet<NodeId> = self.ix.info.keys().copied().filter(matches).collect();
        let rows: Vec<UnresolvedRefRow> = refs
            .iter()
            .filter(|r| matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements))
            .filter(|r| self.ix.by_symbol.get(&r.source_symbol).is_some_and(|id| replacing.contains(id)))
            .cloned()
            .collect();
        let node_by_id: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();
        let mut hierarchy = build_supertypes(&rows, &self.ix);
        let partials: &PartialTypes = &self.ix.partials;
        partials.unite_hierarchy(&mut hierarchy, &node_by_id);
        let installed: &mut Hierarchy = &mut self.ix.hierarchy;
        installed.install(hierarchy, &replacing);
        self
    }

    /// Give the index the **required signatures** among `signatures`, each
    /// placed by a `Contains` edge of `edges`: a structural-types language's
    /// (S-537, [FR-RS-21]; [`DeclarationFacts::declared_signatures`]) — each one
    /// under a type-like node of the index whose file has structural types —
    /// and a trait's (S-712; [`ImplIndex::trait_methods`]) — each one under a
    /// trait of the index, which a trait fan-out through it names in its
    /// [`Dispatch`](super::Dispatch), never as a target. `nodes` and `edges` are the whole
    /// snapshot's, signatures included — the index itself holds none
    /// ([`bindable`](super::bindable)).
    ///
    /// [FR-RS-21]: ../../../../docs/specs/requirements/FR-RS-21.md
    pub(crate) fn with_signatures(
        mut self,
        nodes: &[NodeRow],
        edges: &[EdgeRow],
        signatures: &HashSet<NodeId>,
    ) -> IndexBuilder {
        if signatures.is_empty() {
            return self;
        }
        let names: HashMap<NodeId, &str> = nodes
            .iter()
            .filter(|n| signatures.contains(&n.id))
            .map(|n| (n.id, n.name.as_str()))
            .collect();
        let mut declared: HashMap<(NodeId, String), Vec<NodeId>> = HashMap::new();
        for e in edges.iter().filter(|e| e.kind == EdgeKind::Contains) {
            let Some(name) = names.get(&e.target) else { continue };
            let structural_type = self.ix.info.get(&e.source).is_some_and(|i| {
                is_type_like(i.kind) && i.file_path.as_deref().is_some_and(|p| self.ix.layout.has_structural_types(p))
            });
            if structural_type {
                declared.entry((e.source, (*name).to_string())).or_default().push(e.target);
            }
        }
        for ids in declared.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        self.ix.facts.declared_signatures = declared;
        let signature_nodes: Vec<NodeRow> = nodes.iter().filter(|n| signatures.contains(&n.id)).cloned().collect();
        for (key, symbol) in build_trait_methods(&signature_nodes, edges, &self.ix.info) {
            self.ix.impls.trait_methods.entry(key).or_insert(symbol);
        }
        self
    }

    /// Give the index the self types the store records per node (S-493,
    /// [FR-RS-11]; [`GraphStore::node_self_types`]) — what a `self.m()` /
    /// `Self::m()` call binds through in a language without impl blocks (Go),
    /// and what marks a method no bare call reaches. Rows naming a node the
    /// snapshot does not hold, non-callables, and the functions of an
    /// impl-block language (which bind through [`IndexBuilder::with_associated_items`](super::IndexBuilder::with_associated_items))
    /// contribute no candidate.
    ///
    /// [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
    /// [`GraphStore::node_self_types`]: crate::graph_store::GraphStore::node_self_types
    pub(crate) fn with_self_types(mut self, self_types: Vec<(NodeId, String)>) -> IndexBuilder {
        let modules: &ModuleIndex = &self.ix.modules;
        let mut by_type: HashMap<(String, String, String), Vec<NodeId>> = HashMap::new();
        for (id, self_type) in &self_types {
            let Some(info) = self.ix.info.get(id) else { continue };
            // A method of a type its file imports from outside the crate is a
            // foreign type's: never a candidate for a call on a crate type. A
            // method of an impl-block language binds through the one lookup
            // instead (S-607, [`IndexBuilder::with_associated_items`](super::IndexBuilder::with_associated_items)).
            if !Want::Callable.admits(info.kind)
                || info.file_path.as_deref().is_some_and(|p| self.ix.layout.looks_up_impl_blocks(p))
                || modules.imports_foreign(info, self_type)
            {
                continue;
            }
            by_type
                .entry((info.crate_name.clone(), self_type.clone(), info.name.clone()))
                .or_default()
                .push(*id);
        }
        for ids in by_type.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        let mut type_names: HashMap<(String, String), usize> = HashMap::new();
        for info in self.ix.info.values().filter(|i| is_type_like(i.kind)) {
            *type_names
                .entry((info.crate_name.clone(), info.name.clone()))
                .or_default() += 1;
        }
        let spelt: HashSet<&String> = self_types.iter().map(|(_, t)| t).collect();
        let mut classes: HashMap<(String, String), Vec<NodeId>> = HashMap::new();
        for (id, info) in &self.ix.info {
            if is_class_like(info.kind) && spelt.contains(&info.name) {
                classes.entry((info.crate_name.clone(), info.name.clone())).or_default().push(*id);
            }
        }
        for ids in classes.values_mut() {
            ids.sort_unstable();
        }
        self.ix.facts.self_types = self_types.into_iter().collect();
        self.ix.facts.methods_by_self_type = by_type;
        self.ix.facts.type_names = type_names;
        self.ix.facts.self_type_classes = classes;
        self
    }

    /// Give the index the arity facts the store records per node (S-604,
    /// [CR-200]; [`GraphStore::node_arities`]): the callables recorded as not
    /// taking `self`, which a method-syntax call never binds
    /// ([`DeclarationFacts::without_self`](super::declaration_facts::DeclarationFacts::without_self)), those recorded as taking it, to which a path
    /// call passes its receiver as the first argument ([`DeclarationFacts::with_self`](super::declaration_facts::DeclarationFacts::with_self),
    /// S-607), and each callable's parameter range, which a call's argument
    /// count must fit (S-592, [`DeclarationFacts::arities`](super::declaration_facts::DeclarationFacts::arities)). A fact that is unknown
    /// records nothing.
    ///
    /// [CR-200]: ../../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
    /// [`GraphStore::node_arities`]: crate::graph_store::GraphStore::node_arities
    pub(crate) fn with_arities(mut self, arities: &[NodeArity]) -> IndexBuilder {
        self.ix.facts.without_self = arities
            .iter()
            .filter(|(_, _, takes_self)| *takes_self == Some(false))
            .map(|(id, _, _)| *id)
            .collect();
        self.ix.facts.with_self = arities
            .iter()
            .filter(|(_, _, takes_self)| *takes_self == Some(true))
            .map(|(id, _, _)| *id)
            .collect();
        self.ix.facts.arities = arities
            .iter()
            .filter_map(|(id, range, _)| Some((*id, (*range)?)))
            .collect();
        self
    }

    /// Give the index the `impl` blocks and item facts the store records
    /// (S-606, [FR-EX-34]; [`GraphStore::impl_blocks`],
    /// [`GraphStore::node_item_facts`]) — the universe of the one
    /// associated-item lookup (S-607, [FR-RS-47], [`Ctx::lookup`]).
    ///
    /// Each block's functions are the callables of its file whose first line
    /// it holds — the innermost such block, and none when two blocks of one
    /// span hold it (two `impl`s on one line) — less a function nested in one
    /// of the block's own functions. A block holding none (an empty
    /// `impl Greet for X {}`) is recorded too (S-608): it lends its type its
    /// trait's default bodies. Its header is resolved
    /// **once**, in its own file's scope, as a proven receiver's type is
    /// ([`ProvenReceivers::resolve_receiver_type`](super::proven_receivers::ProvenReceivers::resolve_receiver_type)): its `use` declarations, its module,
    /// its crate and the re-exports they follow, never a workspace guess. A
    /// header that resolves to one repository type adds the block's functions
    /// to that type's candidates ([`ImplIndex::assoc_items`](super::impl_index::ImplIndex::assoc_items)); a reference header
    /// (`impl Tr for &X`), a primitive, tuple or slice, a generic parameter and
    /// an external type add none. Its trait, when the repository declares it,
    /// is resolved the same way ([`ImplHeader::trait_node`]), and so is an
    /// `impl Deref`'s `Target` ([`ImplIndex::deref_targets`](super::impl_index::ImplIndex::deref_targets)). The scope of a block
    /// is that of its first function, and an empty block's the innermost
    /// module of its file holding its first line.
    ///
    /// Read only for the files of an impl-block language
    /// ([`PackageLayout::looks_up_impl_blocks`]). Reads no arity fact: the
    /// facts [`with_arities`](super::IndexBuilder::with_arities) gives only sharpen the
    /// bind-time filters, which an index given none skips. The resolution and
    /// framework passes both give them (the framework pass since S-567).
    ///
    /// [FR-EX-34]: ../../../../docs/specs/requirements/FR-EX-34.md
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    /// [`GraphStore::impl_blocks`]: crate::graph_store::GraphStore::impl_blocks
    /// [`GraphStore::node_item_facts`]: crate::graph_store::GraphStore::node_item_facts
    pub(crate) fn with_associated_items(
        mut self,
        nodes: &[NodeRow],
        blocks: &[ImplBlockRow],
        facts: &[NodeItemFacts],
    ) -> IndexBuilder {
        self.ix.facts.receiver_modes = facts
            .iter()
            .filter_map(|f| Some((f.id, f.receiver_mode?)))
            .collect();
        self.ix.facts.variants = facts
            .iter()
            .filter_map(|f| Some((f.id, f.variants.as_deref()?.split_whitespace().map(str::to_string).collect())))
            .collect();
        let lines: HashMap<NodeId, (i64, i64)> = nodes
            .iter()
            .filter_map(|n| Some((n.id, (n.start_line?, n.end_line.or(n.start_line)?))))
            .collect();
        let mut by_file: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, b) in blocks.iter().enumerate() {
            if self.ix.layout.looks_up_impl_blocks(&b.file_path) {
                by_file.entry(b.file_path.as_str()).or_default().push(i);
            }
        }
        let mut members: Vec<Vec<NodeId>> = vec![Vec::new(); blocks.len()];
        for n in nodes {
            let (Some(path), Some(line)) = (n.file_path.as_deref(), n.start_line) else { continue };
            if !Want::Callable.admits(n.kind) || !self.ix.info.contains_key(&n.id) {
                continue;
            }
            let Some(candidates) = by_file.get(path) else { continue };
            let holding: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| i64::from(blocks[i].start_line) <= line && line <= i64::from(blocks[i].end_line))
                .collect();
            let span = |i: usize| blocks[i].end_line - blocks[i].start_line;
            let Some(narrowest) = holding.iter().map(|&i| span(i)).min() else { continue };
            // Two blocks of one span holding the line (`impl A { … } impl B {
            // … }` on one line): which holds the function is unknown, so
            // neither does — no candidate rather than a guess.
            let [i] = holding.iter().copied().filter(|&i| span(i) == narrowest).collect::<Vec<_>>()[..] else {
                continue;
            };
            // A function nested in one of the block's own functions, or in a
            // trait the block's function declares, is no associated item.
            let nested = self.ix.parent.get(&n.id).is_some_and(|p| {
                self.ix.info.get(p).is_some_and(|pi| pi.kind != NodeKind::Module)
                    && lines.get(p).is_none_or(|&(start, _)| start >= i64::from(blocks[i].start_line))
            });
            if !nested {
                members[i].push(n.id);
            }
        }
        // The modules of each file holding a block, an empty block's scope.
        let mut modules: HashMap<&str, Vec<FileModule>> = HashMap::new();
        for n in nodes.iter().filter(|n| n.kind == NodeKind::Module) {
            let Some(path) = n.file_path.as_deref().filter(|p| by_file.contains_key(p)) else { continue };
            // A file's own module has no parent and spans the file; an inline
            // one spans its lines.
            let span = match (self.ix.parent.contains_key(&n.id), lines.get(&n.id)) {
                (false, _) => None,
                (true, Some(&span)) => Some(span),
                (true, None) => continue,
            };
            modules.entry(path).or_default().push((n.id, span));
        }
        let resolved: Vec<(Vec<NodeId>, ImplHeader, Option<Res>)> = by_file
            .values()
            .flatten()
            .copied()
            .collect::<std::collections::BTreeSet<usize>>()
            .into_iter()
            .filter_map(|i| {
                let (block, mut fns) = (&blocks[i], std::mem::take(&mut members[i]));
                fns.sort_unstable();
                // The header is read where it is written: the scope holding
                // the block's first function, or an empty block's module.
                let scope = match fns.first() {
                    Some(&first) => self.ix.parent.get(&first).copied().unwrap_or(first),
                    None => block_module(modules.get(block.file_path.as_str())?, i64::from(block.start_line))?,
                };
                let ctx = Ctx::for_scope(&self.ix, scope, self.ix.modules.file_ids.get(&block.file_path).copied());
                let self_types = SelfTypes { cx: &ctx };
                let self_type = if block.self_ref {
                    Res::NotFound
                } else {
                    self_types.header_type(&block.self_type)
                };
                let trait_node = block.trait_path.as_deref().and_then(|t| match self_types.header_type(t) {
                    Res::Found(id) if self.ix.info.get(&id).is_some_and(|i| i.kind == NodeKind::Trait) => Some(id),
                    _ => None,
                });
                let deref = block.deref_target.as_deref().map(|t| self_types.header_type(t));
                let header = ImplHeader {
                    self_type,
                    self_path: block.self_type.clone(),
                    trait_path: block.trait_path.clone(),
                    trait_node,
                    fns: fns.clone(),
                };
                Some((fns, header, deref))
            })
            .collect();
        let mut assoc: HashMap<(NodeId, String), Vec<NodeId>> = HashMap::new();
        for (fns, header, deref) in resolved {
            let at = self.ix.impls.impl_headers.len();
            for &f in &fns {
                self.ix.impls.assoc_owner.insert(f, at);
                if let (Res::Found(ty), Some(info)) = (header.self_type, self.ix.info.get(&f)) {
                    assoc.entry((ty, info.name.clone())).or_default().push(f);
                }
            }
            if let (Res::Found(ty), Some(_)) = (header.self_type, header.trait_node) {
                self.ix.impls.trait_impls.entry(ty).or_default().push(at);
            }
            if let (Res::Found(ty), Some(target)) = (header.self_type, deref) {
                // Two `Deref` impls of one type (`impl Deref for W<A>` and
                // `W<B>`, generics stripped) naming two targets decide none.
                self.ix.impls.deref_targets
                    .entry(ty)
                    .and_modify(|known| {
                        if *known != target {
                            *known = Res::Ambiguous;
                        }
                    })
                    .or_insert(target);
            }
            self.ix.impls.impl_headers.push(header);
        }
        for ids in assoc.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        self.ix.impls.assoc_items = assoc;
        self
    }

    /// Give the index every callable's parameter shape (the
    /// `callable_signature` of its [`NodeItemFacts`]), with the prototypes of
    /// the include-roots languages paired with their definitions in other
    /// files ([`PrototypePairs`], S-820). Run after the include scope, whose
    /// reach and class paths it reads, and after the bodiless members
    /// ([`with_uninherited`](IndexBuilder::with_uninherited)) and linkage facts
    /// ([`with_link_unit`](IndexBuilder::with_link_unit)) are given: with no
    /// linkage facts no definition is offered, as a `static` could not be told
    /// apart. A callable the index does not hold — a same-file pair's
    /// prototype, which binds nothing — takes no part.
    pub(crate) fn with_prototype_pairs(mut self, facts: &[NodeItemFacts]) -> IndexBuilder {
        self.ix.pairs = PrototypePairs::build(&self.ix, facts);
        self
    }

    /// Bind every path-grammar `Imports` row of `refs` and record what each
    /// bound to, per file — the imported scope [`ImportedScope::resolve_imported_call`](super::imported_scope::ImportedScope::resolve_imported_call)
    /// consults (S-440, [CR-142] D2, [FR-RS-03]).
    ///
    /// The scope is the **outcome of the import's own binding** ([`bind`], under
    /// the same `policy`), so a call can only ever resolve into a file its
    /// import edge reaches, however that edge came to bind. Computed here, before
    /// any call is bound, so one pass binds imports and the calls through them
    /// alike: a cold index needs no second run to see its own import edges.
    /// Call after [`IndexBuilder::with_path_specifiers`](super::IndexBuilder::with_path_specifiers) — without the specifier context
    /// no file is path-grammar and the scope stays empty.
    ///
    /// [CR-142]: ../../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
    /// [FR-RS-03]: ../../../../docs/specs/requirements/FR-RS-03.md
    ///
    /// A row marked exported (S-534, [FR-RS-19]) is also recorded in its
    /// module's [`ModuleExports`]: a re-export's specifier is part of the
    /// imported scope like any import's, keyed by the module it names, and a
    /// local export alias binds no scope.
    ///
    /// [FR-RS-19]: ../../../../docs/specs/requirements/FR-RS-19.md
    pub(crate) fn with_imported_bindings(
        mut self,
        refs: &[UnresolvedRefRow],
        policy: BindingPolicy,
    ) -> IndexBuilder {
        let modules: &ModuleIndex = &self.ix.modules;
        let mut imported: HashMap<i64, HashMap<String, Vec<NodeId>>> = HashMap::new();
        let mut exports: HashMap<NodeId, ModuleExports> = HashMap::new();
        for r in refs {
            if r.kind != EdgeKind::Imports {
                continue;
            }
            let Some(file_id) = r.file_id else { continue };
            let Some(&source) = self.ix.by_symbol.get(&r.source_symbol) else { continue };
            let path_grammar = self.ix
                .info
                .get(&source)
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| modules.is_path_specifier_file(p));
            if !path_grammar {
                continue;
            }
            let export = ts_export_of(r);
            if let Some(export) = &export {
                let module = exports.entry(self.ix.file_root_of(source)).or_default();
                module.file_id = file_id;
                match export {
                    TsExport::Local { local, exported } => {
                        module.local.entry(exported.to_string()).or_default().push(local.to_string());
                        continue;
                    }
                    TsExport::Named { module: spec, name, exported } => module
                        .named
                        .entry(exported.to_string())
                        .or_default()
                        .push((spec.to_string(), name.to_string())),
                    TsExport::Star { module: spec } => module.star.push(spec.to_string()),
                }
            }
            let targets = match bind(r, &self.ix, policy) {
                Outcome::Bound { target, .. } => vec![target],
                Outcome::BoundMany { targets, .. } => targets,
                Outcome::Unbound => Vec::new(),
            };
            let key = match &export {
                Some(TsExport::Named { module, .. } | TsExport::Star { module }) => module.to_string(),
                _ => r.target.clone(),
            };
            imported.entry(file_id).or_default().entry(key).or_default().extend(targets);
        }
        for by_target in imported.values_mut() {
            for targets in by_target.values_mut() {
                targets.sort();
                targets.dedup();
            }
        }
        for module in exports.values_mut() {
            for list in module.named.values_mut() {
                list.sort();
                list.dedup();
            }
            for list in module.local.values_mut() {
                list.sort();
                list.dedup();
            }
            module.star.sort();
            module.star.dedup();
        }
        self.ix.modules.imported = imported;
        self.ix.modules.exports = exports;
        self.with_structural_supertypes(refs)
    }

    /// This index, with the supertypes of every structural-types file's types
    /// (S-535, TypeScript) read again now that the imported scope is known
    /// ([`with_imported_bindings`](super::IndexBuilder::with_imported_bindings)). Its
    /// supertype is usually named through an import, `<import target>::<name>`
    /// ([`ImportedScope::resolve_imported_type`](super::imported_scope::ImportedScope::resolve_imported_type)), which the [`Hierarchy`]
    /// [`new`](IndexBuilder::new) reads cannot bind yet:
    /// without this, `super.m()` would climb a chain the `Extends` edge proves
    /// and the walk never sees. Each such type's hierarchy entries are replaced
    /// whole — a type's are read off its own rows alone ([`build_supertypes`])
    /// — so every other language's hierarchy is untouched.
    fn with_structural_supertypes(mut self, refs: &[UnresolvedRefRow]) -> IndexBuilder {
        let source_of = |r: &UnresolvedRefRow| {
            let id = *self.ix.by_symbol.get(&r.source_symbol)?;
            let path = self.ix.info.get(&id)?.file_path.as_deref()?;
            self.ix.layout.has_structural_types(path).then_some(id)
        };
        let rows: Vec<UnresolvedRefRow> = refs
            .iter()
            .filter(|r| matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements))
            .filter(|r| source_of(r).is_some())
            .cloned()
            .collect();
        if rows.is_empty() {
            return self;
        }
        let imported = rows.iter().any(|r| r.form == RefForm::Path && r.target.contains("::"));
        let sources: HashSet<NodeId> = rows.iter().filter_map(source_of).collect();
        let mut hierarchy = build_supertypes(&rows, &self.ix);
        hierarchy.imported = imported;
        let installed: &mut Hierarchy = &mut self.ix.hierarchy;
        installed.install(hierarchy, &sources);
        self
    }

    /// The index, every step given.
    pub(crate) fn build(self) -> Index {
        self.ix
    }
}

/// A module of a file and the lines it spans — `None` for the file's own
/// module, which spans the file ([`block_module`]).
type FileModule = (NodeId, Option<(i64, i64)>);

/// The innermost of a file's `modules` holding `line` — an inline module whose
/// lines hold it, else the file's own module (spanless) — where an empty `impl`
/// block's header is read (S-608, [`IndexBuilder::with_associated_items`](super::IndexBuilder::with_associated_items)).
fn block_module(modules: &[FileModule], line: i64) -> Option<NodeId> {
    modules
        .iter()
        .filter_map(|&(id, span)| match span {
            Some((start, end)) if start <= line && line <= end => Some((end - start, id)),
            Some(_) => None,
            None => Some((i64::MAX, id)),
        })
        .min()
        .map(|(_, id)| id)
}

/// `Contains` topology: child→parent, and scope→name→members with each member
/// list id-sorted for a deterministic candidate order regardless of edge
/// iteration order. `parent` records every `Contains` edge; a `members` entry
/// is added only when the target is a known node (its name is needed).
fn build_containment(
    edges: &[EdgeRow],
    node_by_id: &HashMap<NodeId, &NodeRow>,
) -> (HashMap<NodeId, NodeId>, Members) {
    let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
    let mut members: Members = HashMap::new();
    for e in edges {
        if e.kind != EdgeKind::Contains {
            continue;
        }
        parent.insert(e.target, e.source);
        if let Some(n) = node_by_id.get(&e.target) {
            members
                .entry(e.source)
                .or_default()
                .entry(n.name.clone())
                .or_default()
                .push(e.target);
        }
    }
    for by_name in members.values_mut() {
        for list in by_name.values_mut() {
            list.sort();
        }
    }
    (parent, members)
}

/// The path-derived module tree as `(modules, module_key)` — the forward
/// `(crate, path)`→node map and its reverse. A parentless `Module` node is a
/// *file module* keyed by its file path; inline `mod`s hang beneath it. No
/// node id decides a key two modules share (S-751): an inline module holds it
/// against a file module, the first file in path order holds it against
/// another, and a key a bodyless `mod x;` claims goes to the file it declares
/// ([`hand_declarations_to_their_files`], S-585).
fn build_module_tree(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
) -> (HashMap<ModKey, NodeId>, HashMap<NodeId, ModKey>) {
    let mut modules: HashMap<ModKey, NodeId> = HashMap::new();
    let mut module_key: HashMap<NodeId, ModKey> = HashMap::new();
    let mut file_roots: Vec<&NodeRow> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module && !parent.contains_key(&n.id))
        .collect();
    // The path decides a tie, never a node id: an id changes on every
    // re-extraction, so a sync that re-extracts one of two files sharing a key
    // would hand it to the other, which a cold index never does ([NFR-RA-06]).
    // Two files share a key routinely: `src/lib.rs` and `src/main.rs` (the
    // crate root), and a fixture crate nested under a crate of its own name
    // (S-751). A cold index writes files in discovery order — component-wise
    // path order — so that is the order here. Import-root files (S-519; a
    // module and its stub, `foo.py` and `foo.pyi`) keep their own string order,
    // which a cold index already followed; they sit in their family's own
    // crate, so the two orders never compete for a key.
    file_roots.sort_by_key(|n| {
        let path = n.file_path.as_deref();
        let import_root_path = path.filter(|p| layout.has_import_roots(p));
        (import_root_path, path.map(Path::new), n.id)
    });
    let mut files_at: HashMap<ModKey, Vec<(NodeId, String)>> = HashMap::new();
    let mut keyed_roots: Vec<(NodeId, ModKey)> = Vec::new();
    for root in file_roots {
        let Some(path) = &root.file_path else {
            continue; // an orphaned module node cannot anchor a tree
        };
        let key = layout.module_key(path);
        module_key.insert(root.id, key.clone());
        files_at
            .entry(key.clone())
            .or_default()
            .push((root.id, extension_of(path)));
        append_inline_modules(
            root.id,
            key.clone(),
            members,
            node_by_id,
            &mut modules,
            &mut module_key,
        );
        keyed_roots.push((root.id, key));
    }
    // A file takes its key only where no inline module holds it: an inline
    // `mod inner { … }` is the module, an `inner.rs` beside it no file declares
    // is not (FR-RS-41) — whichever path sorts first. A declaration holding a
    // key goes to its file below.
    for (id, key) in keyed_roots {
        modules.entry(key).or_insert(id);
    }
    hand_declarations_to_their_files(nodes, parent, members, &files_at, &mut modules, &module_key);
    (modules, module_key)
}

/// Give each key a bodyless `mod x;` declaration claims to the file module it
/// declares (S-585, [FR-RS-41]). The declaration is a node of the declaring
/// file, appended beneath it like an inline `mod`, so its key is exactly the
/// key of `x.rs` or `x/mod.rs` under the declaring module's directory — and
/// the first-by-id tie-break used to hand the key to whichever came first,
/// usually the empty declaration, so every path through it found nothing.
///
/// The key goes to the **one** file module of the declaration's own language
/// (its file extension) at it — the path model keys `src/x.js` where it keys
/// `src/x.rs`, and a Rust declaration never names a JavaScript file. With none
/// (a `#[path]` attribute, which is not read, a missing file, or only another
/// language's file at that path) the declaration keeps the key and binds
/// nothing beneath; with two (`x.rs` beside `x/mod.rs`, which rustc refuses)
/// it keeps it too, so neither is guessed ([NFR-RA-05]). No file has won a
/// claimed key when this runs: nested modules take their keys before any
/// file does ([`build_module_tree`], S-751). Where no file
/// answers and two declarations claim one key (`mod x;` in both `src/lib.rs`
/// and `src/main.rs`), the first in path order holds it. Declarations are
/// visited in path order, never node order, so the outcome reads no node id: a
/// sync and a cold index agree ([NFR-RA-06]). No node, symbol or edge changes
/// — only which node answers the key.
///
/// A declaration is a nested `Module` with no `Contains` child on a single
/// line. An inline `mod x { … }` with contents, or with braces over several
/// lines, is not one, and keeps the key exactly as before — also against a
/// declaration of the same key from the other crate root (`src/lib.rs` and
/// `src/main.rs` share the crate's root key): only another declaration
/// holding a key is ever displaced.
///
/// [FR-RS-41]: ../../../../docs/specs/requirements/FR-RS-41.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn hand_declarations_to_their_files(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    files_at: &HashMap<ModKey, Vec<(NodeId, String)>>,
    modules: &mut HashMap<ModKey, NodeId>,
    module_key: &HashMap<NodeId, ModKey>,
) {
    let is_declaration = |n: &NodeRow| {
        n.kind == NodeKind::Module
            && parent.contains_key(&n.id)
            && !members.contains_key(&n.id)
            && n.start_line == n.end_line
    };
    let mut declarations: Vec<&NodeRow> = nodes.iter().filter(|n| is_declaration(n)).collect();
    // A sync renumbers a re-extracted file's nodes; the path does not move.
    declarations.sort_by(|a, b| {
        (a.file_path.as_deref(), a.start_line, a.name.as_str())
            .cmp(&(b.file_path.as_deref(), b.start_line, b.name.as_str()))
    });
    let is_declared: HashSet<NodeId> = declarations.iter().map(|d| d.id).collect();
    let mut claimed: HashSet<&ModKey> = HashSet::new();
    for declaration in declarations {
        let Some(key) = module_key.get(&declaration.id) else { continue };
        let files = files_at.get(key).map_or(&[][..], Vec::as_slice);
        let language = declaration.file_path.as_deref().map(extension_of);
        let declared: Vec<NodeId> = files
            .iter()
            .filter(|(_, ext)| language.as_ref() == Some(ext))
            .map(|(id, _)| *id)
            .collect();
        let holder = modules.get(key).copied();
        let file_holds = holder.is_some_and(|h| files.iter().any(|(id, _)| *id == h));
        if !file_holds && !holder.is_some_and(|h| is_declared.contains(&h)) {
            continue; // an inline module with contents holds it
        }
        // Otherwise the first declaration in path order holds the key.
        let first = claimed.insert(key);
        match declared.as_slice() {
            [file] => {
                modules.insert(key.clone(), *file);
            }
            _ if first => {
                modules.insert(key.clone(), declaration.id);
            }
            _ => {}
        }
    }
}

/// Append every inline `mod` beneath `root` to the module maps, depth-first
/// (`Contains` is a tree). Each scope's `Module` children are visited
/// `(name, id)`-sorted so keys land deterministically regardless of map
/// iteration order; `modules` is first-wins per key, `module_key` last-wins
/// (a node has one parent, so it is appended once). The per-scope children are
/// pre-flattened into one sorted list and the non-module guard is a guard
/// clause, so the DFS body stays within nesting depth 3.
fn append_inline_modules(
    root: NodeId,
    root_key: ModKey,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    modules: &mut HashMap<ModKey, NodeId>,
    module_key: &mut HashMap<NodeId, ModKey>,
) {
    let mut stack = vec![(root, root_key)];
    while let Some((scope, scope_key)) = stack.pop() {
        for (name, id) in sorted_children(members, scope) {
            // Only inline `mod`s extend the module tree; skip every other kind.
            let kind = node_by_id.get(&id).map(|n| n.kind);
            if kind != Some(NodeKind::Module) {
                continue;
            }
            let mut child_key = scope_key.clone();
            child_key.1.push(name.clone());
            modules.entry(child_key.clone()).or_insert(id);
            module_key.insert(id, child_key.clone());
            stack.push((id, child_key));
        }
    }
}

/// `scope`'s `Contains` children as a flat `(name, id)` list sorted by name then
/// id — the canonical visitation order the module-tree DFS appends in. Empty
/// when the scope has no members.
fn sorted_children(members: &Members, scope: NodeId) -> Vec<(&String, NodeId)> {
    let Some(by_name) = members.get(&scope) else {
        return Vec::new();
    };
    let mut children: Vec<(&String, NodeId)> = by_name
        .iter()
        .flat_map(|(name, ids)| ids.iter().map(move |&id| (name, id)))
        .collect();
    children.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    children
}

/// Per-node binding facts ([`NodeInfo`]) for every node.
fn build_node_info(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    module_key: &HashMap<NodeId, ModKey>,
    layout: &PackageLayout,
) -> HashMap<NodeId, NodeInfo> {
    let mut info: HashMap<NodeId, NodeInfo> = HashMap::new();
    for n in nodes {
        info.insert(
            n.id,
            NodeInfo {
                kind: n.kind,
                crate_name: crate_of_node(n, parent, module_key, layout),
                name: n.name.clone(),
                file_path: n.file_path.clone(),
            },
        );
    }
    info
}

/// The crate a node belongs to (for crate-first fallbacks): the nearest module
/// ancestor's crate, walking the `Contains` chain from the node itself; failing
/// that, the crate derived from the node's own file path, else empty.
fn crate_of_node(
    n: &NodeRow,
    parent: &HashMap<NodeId, NodeId>,
    module_key: &HashMap<NodeId, ModKey>,
    layout: &PackageLayout,
) -> String {
    let mut cursor = Some(n.id);
    while let Some(id) = cursor {
        if let Some((c, _)) = module_key.get(&id) {
            return c.clone();
        }
        cursor = parent.get(&id).copied();
    }
    match &n.file_path {
        Some(path) => layout.module_key(path).0,
        None => String::new(),
    }
}

/// The package-shaped type universe ([CR-149]): every top-level type-like
/// member of a package-shaped file's root module, keyed by its fully-qualified
/// name ([`PackageLayout::type_fqn`], the one FQN derivation). A package's
/// types named `n` are the entry `package ++ [n]`, so a wildcard and the
/// same-package rung read this map too. `nodes` is id-ordered and each file's
/// children are visited name-/id-sorted, but every list is sorted explicitly
/// anyway, so the map is deterministic regardless of visit order
/// ([NFR-RA-06]). Empty under the default layout.
///
/// Partitioned by the declaring file's interop family (S-519,
/// [`PackageLayout::family`]).
///
/// [CR-149]: ../../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_package_types(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let admits = |path: &str, kind: NodeKind| layout.is_package_shaped(path) && is_type_like(kind);
    build_package_index(nodes, parent, members, node_by_id, layout, admits, |path, name| layout.type_fqn(path, name))
}

/// The index of every file-root module's top-level children that `admits`
/// takes (`admits(path, kind)`, by the file's path and the child's kind),
/// keyed by `key(path, name)` — [`PackageLayout::type_fqn`] for
/// [`build_package_types`] and [`build_top_level_callables`] (S-573), the bare
/// name for [`build_unplaced_callables`] (S-551) — and partitioned by the
/// declaring file's interop family. A child `key` names nothing is left out.
/// Every list is id-sorted ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_package_index(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
    admits: impl Fn(&str, NodeKind) -> bool,
    key: impl Fn(&str, &str) -> Option<Vec<String>>,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let mut by_family: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> = HashMap::new();
    for root in nodes {
        if root.kind != NodeKind::Module || parent.contains_key(&root.id) {
            continue;
        }
        let Some(path) = &root.file_path else { continue };
        let Some(family) = layout.family(path) else { continue };
        for (name, id) in sorted_children(members, root.id) {
            if !node_by_id.get(&id).is_some_and(|n| admits(path, n.kind)) {
                continue;
            }
            if let Some(fqn) = key(path, name) {
                by_family.entry(family.clone()).or_default().entry(fqn).or_default().push(id);
            }
        }
    }
    for list in by_family.values_mut().flat_map(HashMap::values_mut) {
        list.sort();
        list.dedup();
    }
    by_family
}

/// The top-level functions — and the top-level properties, a `Variable` or a
/// `Constant` (S-831: Kotlin's `val`, `var` and `const val`) — of every file
/// whose language declares them members of its namespace (S-573,
/// [`PackageLayout::declares_top_level_callables`]: Scala 3's top-level
/// `def`, Kotlin's top-level `fun`), keyed by fully-qualified name as
/// [`build_package_types`] keys types, and partitioned the same way. A map of
/// its own, never the type index: a type and a function may share one name (a
/// class `Foo` and a factory `def Foo`), and a source whose language declares
/// no top-level functions — a Java file of the same `jvm` family — reaches a
/// Kotlin file's only through its facade, and Kotlin and Scala never name each
/// other's ([`ExtensionCalls::top_level_callables`](super::extension_calls::ExtensionCalls::top_level_callables)).
/// A property is a candidate of an import ([`Want::Any`]) and of no call: every
/// reader filters the list by what its lookup admits
/// ([`PackageNames::package_callables`](super::package_names::PackageNames::package_callables)).
fn build_top_level_callables(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let admits = |path: &str, kind: NodeKind| {
        layout.declares_top_level_callables(path) && (Want::Callable.admits(kind) || is_top_level_term(kind))
    };
    build_package_index(nodes, parent, members, node_by_id, layout, admits, |path, name| layout.type_fqn(path, name))
}

/// The top-level functions of every file whose namespace is not known — its
/// top-level declarations sit in two namespaces, so no package keys it
/// ([`PackageLayout::package_of`]) — in a language whose unqualified calls
/// fall back to the global function (S-551,
/// [`PackageLayout::falls_back_to_global_functions`]: PHP), keyed by bare name
/// and partitioned as [`build_top_level_callables`] partitions. Such a function
/// may be a global one, so the global namespace is not known in full where
/// one shares the name ([`PackageNames::namespace_callables`](super::package_names::PackageNames::namespace_callables)).
fn build_unplaced_callables(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let admits = |path: &str, kind: NodeKind| {
        layout.falls_back_to_global_functions(path) && layout.package_of(path).is_none() && Want::Callable.admits(kind)
    };
    build_package_index(nodes, parent, members, node_by_id, layout, admits, |_, name| Some(vec![name.to_string()]))
}

/// The directory modules of every import-root file (S-519,
/// [`ModuleIndex::dir_modules`](super::module_index::ModuleIndex::dir_modules)): each directory between a file-root module's import
/// root and its file ([`PackageLayout::directory_modules`]). Empty under a
/// layout that declares no import roots.
fn build_directory_modules(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    layout: &PackageLayout,
) -> HashSet<ModKey> {
    nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module && !parent.contains_key(&n.id))
        .filter_map(|n| n.file_path.as_deref())
        .flat_map(|path| layout.directory_modules(path))
        .collect()
}

/// Declared namespace → the file-root modules of the declared-namespace files
/// declaring it (S-518, [`PackageIndex::files_by_namespace`](super::package_index::PackageIndex::files_by_namespace)): every parentless
/// `Module` node whose file the layout knows a declared namespace for
/// ([`PackageLayout::package_of`], the one reading of it), id-sorted
/// ([NFR-RA-06]). Empty under a layout that knows no namespace.
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_namespace_files(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    build_file_index(nodes, parent, layout, |path| {
        layout.declares_namespaces(path).then(|| layout.package_of(path)).flatten()
    })
}

/// Facade class → the file-root modules whose top-level functions it holds
/// (S-560, [`PackageIndex::facades_by_fqn`](super::package_index::PackageIndex::facades_by_fqn)): every parentless `Module` node whose
/// language declares a file facade, keyed by its declared namespace plus its
/// facade's name ([`PackageLayout::file_facade`]) — `mixit.util.UtilsKt` for
/// `mixit/util/Utils.kt` — id-sorted ([NFR-RA-06]). Empty under a layout that
/// declares no facade.
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_file_facades(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    layout: &PackageLayout,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    build_file_index(nodes, parent, layout, |path| {
        let mut fqn = layout.package_of(path)?;
        fqn.push(layout.file_facade(path)?);
        Some(fqn)
    })
}

/// The file-root modules `key` names a key for (`key(path)`, by the file's
/// path), partitioned by the file's interop family and id-sorted
/// ([NFR-RA-06]) — what [`build_namespace_files`] and [`build_file_facades`]
/// each read off one keying.
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_file_index(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    layout: &PackageLayout,
    key: impl Fn(&str) -> Option<Vec<String>>,
) -> HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> {
    let mut by_family: HashMap<String, HashMap<Vec<String>, Vec<NodeId>>> = HashMap::new();
    for root in nodes {
        if root.kind != NodeKind::Module || parent.contains_key(&root.id) {
            continue;
        }
        let Some(path) = root.file_path.as_deref() else { continue };
        let (Some(family), Some(key)) = (layout.family(path), key(path)) else {
            continue;
        };
        by_family.entry(family).or_default().entry(key).or_default().push(root.id);
    }
    for list in by_family.values_mut().flat_map(HashMap::values_mut) {
        list.sort();
        list.dedup();
    }
    by_family
}

/// What [`build_constants`] derives (S-565): the fields of [`Index`] of the same
/// names.
struct ConstantIndex {
    constants: FamilyConstants,
    constant_of: HashMap<NodeId, Vec<String>>,
}

/// The lexical-constant index (S-565, [FR-RS-33], [`PackageIndex::constants`](super::package_index::PackageIndex::constants)): every
/// `Class` or `Module` node of a lexical-constant file but its file root, keyed
/// by the constant it declares — the constant of its enclosing class or module
/// (none at the top level) joined to the name it is written with
/// ([`constant_fqn`]). A compact `class A::B` is written `A::B`: its scope is
/// the `TypeUses` row its header records (the one row of that kind such a
/// declaration's own first line carries, `references.scm`). Returns the index,
/// partitioned by family and id-sorted ([NFR-RA-06]), and each declaration's
/// constant.
///
/// A compact declaration's node spans its whole body (S-564), so its members
/// are its own — a node nested in it reads the constant it declares — and what
/// follows its `end` is the scope's, as for any other declaration.
///
/// [FR-RS-33]: ../../../../docs/specs/requirements/FR-RS-33.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_constants(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    by_symbol: &HashMap<String, NodeId>,
    refs: &[UnresolvedRefRow],
    layout: &PackageLayout,
) -> ConstantIndex {
    let declarations: HashMap<NodeId, &NodeRow> = nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Class | NodeKind::Module) && parent.contains_key(&n.id))
        .filter(|n| n.file_path.as_deref().is_some_and(|p| layout.resolves_lexical_constants(p)))
        .map(|n| (n.id, n))
        .collect();
    if declarations.is_empty() {
        return ConstantIndex {
            constants: HashMap::new(),
            constant_of: HashMap::new(),
        };
    }
    let scopes: HashMap<NodeId, Vec<String>> = refs
        .iter()
        .filter(|r| r.kind == EdgeKind::TypeUses && r.form == RefForm::Path)
        .filter_map(|r| {
            let id = *by_symbol.get(&r.source_symbol)?;
            let n = declarations.get(&id)?;
            (r.line.is_some() && r.line == n.start_line).then(|| (id, split(&r.target)))
        })
        .collect();
    let written = |id: NodeId| -> Vec<String> {
        let mut path = scopes.get(&id).cloned().unwrap_or_default();
        path.push(declarations[&id].name.clone());
        path
    };
    let mut constant_of: HashMap<NodeId, Vec<String>> = HashMap::new();
    let mut by_family: FamilyConstants = HashMap::new();
    for (&id, n) in &declarations {
        // The declarations enclosing this one, innermost first, up to the file
        // root or the first enclosing node that is no class or module.
        let mut chain = vec![id];
        let mut at = id;
        while let Some(&up) = parent.get(&at).filter(|p| declarations.contains_key(p)) {
            chain.push(up);
            at = up;
        }
        let fqn = chain
            .iter()
            .rev()
            .fold(Vec::new(), |enclosing, &c| constant_fqn(&enclosing, &written(c)));
        let Some(family) = n.file_path.as_deref().and_then(|p| layout.family(p)) else { continue };
        by_family.entry(family).or_default().entry(fqn.clone()).or_default().push(id);
        constant_of.insert(id, fqn);
    }
    for list in by_family.values_mut().flat_map(HashMap::values_mut) {
        list.sort();
    }
    ConstantIndex {
        constants: by_family,
        constant_of,
    }
}

/// The partial types of the graph (S-557, [FR-RS-29]): the type declarations
/// whose own `Extends` row is aliased [`PARTIAL_TYPE_ALIAS`], united where they
/// declare one fully-qualified name ([`PartialTypes::unite`]).
///
/// A top-level part is keyed by its fully-qualified name, the
/// [`types_by_fqn`](super::package_index::PackageIndex::types_by_fqn) entry — across the files of one
/// directory tree ([`directory_trees`](super::directory_trees)), as C# unites a type's parts across
/// one project. A nested part is keyed by its enclosing
/// type — the first-declared part of that type, when it is partial itself — and
/// its name, so the parts of `Outer.Inner` written in the two parts of `Outer`
/// are one type too. Nested parts are read outermost first, so an enclosing
/// type's parts are united before its members are.
///
/// [FR-RS-29]: ../../../../docs/specs/requirements/FR-RS-29.md
fn build_partial_types(
    refs: &[UnresolvedRefRow],
    by_symbol: &HashMap<String, NodeId>,
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    types_by_fqn: &HashMap<String, HashMap<Vec<String>, Vec<NodeId>>>,
    node_by_id: &HashMap<NodeId, &NodeRow>,
) -> PartialTypes {
    let mut partials = PartialTypes::default();
    for r in refs {
        if r.kind != EdgeKind::Extends || r.alias.as_deref() != Some(PARTIAL_TYPE_ALIAS) {
            continue;
        }
        let Some(n) = by_symbol.get(&r.source_symbol).and_then(|id| node_by_id.get(id)) else {
            continue;
        };
        if is_type_like(n.kind) {
            partials.marked.insert(n.id, r.arg_count.unwrap_or(0));
        }
    }
    if partials.marked.is_empty() {
        return partials;
    }
    for ids in types_by_fqn.values().flat_map(HashMap::values) {
        partials.unite(ids, node_by_id);
    }
    let is_type = |id: &NodeId| node_by_id.get(id).is_some_and(|n| is_type_like(n.kind));
    let depth = |mut id: NodeId| {
        let mut depth = 0u32;
        while let Some(up) = parent.get(&id).copied().filter(is_type) {
            depth += 1;
            id = up;
            if depth >= MAX_CONTAINS_DEPTH {
                break;
            }
        }
        depth
    };
    let mut nested: Vec<(u32, NodeId)> = partials
        .marked
        .keys()
        .filter(|id| parent.get(id).is_some_and(is_type))
        .map(|&id| (depth(id), id))
        .collect();
    nested.sort_unstable();
    let mut seen: HashSet<(NodeId, String)> = HashSet::new();
    for (_, id) in nested {
        let (Some(&scope), Some(name)) = (parent.get(&id), node_by_id.get(&id).map(|n| n.name.clone())) else {
            continue;
        };
        let scope = partials.first_of(scope);
        if !seen.insert((scope, name.clone())) {
            continue;
        }
        let mut ids: Vec<NodeId> = partials
            .parts_of(&scope)
            .iter()
            .filter_map(|part| members.get(part).and_then(|m| m.get(&name)))
            .flatten()
            .copied()
            .filter(is_type)
            .collect();
        ids.sort_unstable();
        partials.unite(&ids, node_by_id);
    }
    partials
}

/// Canonical symbol → node. First-wins on a (model-prohibited) duplicate
/// symbol: `nodes` arrives id-ordered from `all_nodes()`, so this matches the
/// store's `node_id_for_symbol` min-id pick (a no-op in practice, ADR-07).
fn build_by_symbol(nodes: &[NodeRow]) -> HashMap<String, NodeId> {
    let mut by_symbol: HashMap<String, NodeId> = HashMap::with_capacity(nodes.len());
    for n in nodes {
        by_symbol
            .entry(n.symbol.as_str().to_string())
            .or_insert(n.id);
    }
    by_symbol
}

/// name → every node carrying it, id-sorted — the unique-match fallback
/// universe.
fn build_by_name(nodes: &[NodeRow]) -> HashMap<String, Vec<NodeId>> {
    let mut by_name: HashMap<String, Vec<NodeId>> = HashMap::new();
    for n in nodes {
        by_name.entry(n.name.clone()).or_default().push(n.id);
    }
    for list in by_name.values_mut() {
        list.sort();
    }
    by_name
}

/// file path → its nodes, for doc link/path resolution (S-035). `nodes` is
/// id-ordered from `all_nodes()`, so each list is already sorted.
fn build_by_file_path(nodes: &[NodeRow]) -> HashMap<String, Vec<NodeId>> {
    let mut by_file_path: HashMap<String, Vec<NodeId>> = HashMap::new();
    for n in nodes {
        if let Some(path) = &n.file_path {
            by_file_path.entry(path.clone()).or_default().push(n.id);
        }
    }
    by_file_path
}

/// The file-root module nodes (a parentless [`NodeKind::Module`] bound to a
/// file — one per code file) keyed two ways for path-specifier binding
/// (S-439): by the file path with its extension removed, and by directory.
/// `nodes` is id-ordered, so every list is already sorted ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
#[allow(clippy::type_complexity)]
fn build_file_roots(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
) -> (
    HashMap<String, Vec<NodeId>>,
    HashMap<String, Vec<(String, NodeId)>>,
) {
    let mut by_stem: HashMap<String, Vec<NodeId>> = HashMap::new();
    let mut by_dir: HashMap<String, Vec<(String, NodeId)>> = HashMap::new();
    for n in nodes {
        if n.kind != NodeKind::Module || parent.contains_key(&n.id) {
            continue;
        }
        let Some(path) = &n.file_path else { continue };
        let (dir, name) = match path.rfind('/') {
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("", path.as_str()),
        };
        let stem = match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => &path[..path.len() - name.len() + stem.len()],
            _ => path.as_str(),
        };
        by_stem.entry(stem.to_string()).or_default().push(n.id);
        by_dir
            .entry(dir.to_string())
            .or_default()
            .push((path.clone(), n.id));
    }
    (by_stem, by_dir)
}

/// Normalized template → its `(METHOD, route node)` candidates, for the OpenAPI
/// operation→route match (S-069). A route name is `"METHOD /path"`; the bucket
/// key is the positionally-normalized template (parameter names/syntax erased)
/// and the upper-cased method rides with the node. A route whose template does
/// not normalize cleanly is skipped, so it can never become a candidate
/// ([FR-CG-09], [NFR-RA-05]). `nodes` is id-ordered, so each bucket is already
/// sorted by id.
///
/// Bucketing on the template alone is what lets a wildcard (`ANY`) provider be
/// compared against a concrete consumer method at all: the compatibility and
/// precedence rule then runs over the bucket in
/// [`resolve_route`](super::artifacts::Artifacts::resolve_route) ([CR-109], [FR-CG-09]).
///
/// [CR-109]: ../../../../docs/requests/CR-109-wildcard-method-route-matching.md
fn build_routes_by_template(nodes: &[NodeRow]) -> HashMap<String, Vec<(String, NodeId)>> {
    let mut routes_by_template: HashMap<String, Vec<(String, NodeId)>> = HashMap::new();
    for n in nodes {
        if n.kind != NodeKind::Route {
            continue;
        }
        let Some((method, template)) = route_key(&n.name) else {
            continue;
        };
        routes_by_template
            .entry(template)
            .or_default()
            .push((method, n.id));
    }
    routes_by_template
}

/// Per-file scope facts (aliases + globs) from the ledger's `Imports` rows,
/// bound or not. Re-extracted files' rows were replaced by `persist_file`
/// before this pass; untouched files' rows persist — the map is whole at bind
/// time.
fn build_file_scopes(refs: &[UnresolvedRefRow]) -> HashMap<i64, FileScope> {
    let mut file_scopes: HashMap<i64, FileScope> = HashMap::new();
    for r in refs {
        if r.kind != EdgeKind::Imports {
            continue;
        }
        let Some(file_id) = r.file_id else { continue };
        add_import_row(file_scopes.entry(file_id).or_default(), r);
    }
    file_scopes
}

/// `scope`, its file's `Imports` row `r` recorded: a wildcard among the
/// static, block or file wildcards, an alias among the aliases and their
/// expansions ([`build_file_scopes`]).
fn add_import_row(scope: &mut FileScope, r: &UnresolvedRefRow) {
    let path: Vec<String> = r.target.split("::").map(str::to_string).collect();
    match r.form {
        RefForm::Glob if r.alias.as_deref() == Some(STATIC_WILDCARD_ALIAS) => {
            scope.static_globs.push(path)
        }
        RefForm::Glob => match r.alias.as_deref().and_then(wildcard_block) {
            // A block enclosing none of the file's declarations (`{}`):
            // its `using`s reach none of them (S-603).
            Some(block) if block.is_empty() => {}
            Some(block) => scope.block_globs.entry(block).or_default().push(path),
            None => {
                if r.exported == Some(true) {
                    scope.exported_globs.push(path.clone());
                }
                scope.globs.push(path);
            }
        },
        _ => {
            if let Some(alias) = &r.alias {
                // An identical import is one expansion, not a rival
                // (S-519): werkzeug's functions each re-run `import
                // warnings`, and a head that expands to itself would branch
                // once per copy at every alias level.
                let expansions = scope.alias_expansions.entry(alias.clone()).or_default();
                if !expansions.contains(&path) {
                    expansions.push(path.clone());
                }
                scope.aliases.entry(alias.clone()).or_insert(path);
            }
        }
    }
}

/// The namespace block a wildcard's alias names ([`block_wildcard_alias`](super::block_wildcard_alias)), as
/// segments; `None` for every other alias.
fn wildcard_block(alias: &str) -> Option<Vec<String>> {
    let block = alias.strip_prefix('{')?.strip_suffix('}')?;
    Some(super::super::package_key::namespace_segments(block))
}

/// `(trait node, method name)` → the impl method nodes of that trait method,
/// id-sorted and deduplicated (S-281, [CR-073], [FR-RS-08]).
///
/// Built from the `Implements` reference rows an extraction pass emits, one per
/// method of an `impl T for X` block (`source` = the impl method's symbol,
/// `target` = the trait's written path). The trait is resolved by its **last
/// path segment** to the one workspace [`NodeKind::Trait`] of that name — a
/// non-unique or unindexed (external) trait contributes nothing, so a `dyn`
/// call whose trait is ambiguous or external never fabricates an impl set
/// ([NFR-RA-05]). Keyed by the method's own name (`info[source].name`) so the
/// fan-out is a direct lookup. Deterministic: lists are id-sorted and deduped,
/// mirroring [`build_by_name`] ([NFR-RA-06]).
///
/// [CR-073]: ../../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_impls_by_trait_method(
    refs: &[UnresolvedRefRow],
    by_symbol: &HashMap<String, NodeId>,
    by_name: &HashMap<String, Vec<NodeId>>,
    info: &HashMap<NodeId, NodeInfo>,
    layout: &PackageLayout,
) -> HashMap<(NodeId, String), Vec<NodeId>> {
    let mut map: HashMap<(NodeId, String), Vec<NodeId>> = HashMap::new();
    for r in refs {
        if r.kind != EdgeKind::Implements {
            continue;
        }
        let Some(&impl_method) = by_symbol.get(&r.source_symbol) else {
            continue; // the impl method's own node is not indexed — skip
        };
        // A type's `Implements` (S-466 for Java, S-522 for every language)
        // is the type-relation arm's: its source is a class rather than an
        // impl method, and it has no place in the `dyn T` fan-out universe.
        if relation_want_of(r, info.get(&impl_method), layout).is_some() {
            continue;
        }
        let last = r.target.rsplit("::").next().unwrap_or(&r.target);
        // Resolve the trait by the same unique-name rule the query-time fan-out
        // uses ([`Index::trait_by_name`]) so index-build and bind agree; an
        // ambiguous / external trait contributes nothing (never fabricate an impl
        // set, [NFR-RA-05]).
        let Some(trait_node) = unique_trait(by_name, info, last) else {
            continue;
        };
        let Some(method_name) = info.get(&impl_method).map(|i| i.name.clone()) else {
            continue;
        };
        map.entry((trait_node, method_name)).or_default().push(impl_method);
    }
    for ids in map.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }
    map
}

/// `(trait node, method name)` → the symbol of each callable of `nodes` that a
/// `Contains` edge of `edges` places directly under a trait of `info`
/// ([`ImplIndex::trait_methods`](super::impl_index::ImplIndex::trait_methods), S-712). The first edge of a name wins, and the
/// snapshot's edges are read in their stored order, so the map is
/// deterministic ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
fn build_trait_methods(
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    info: &HashMap<NodeId, NodeInfo>,
) -> HashMap<(NodeId, String), String> {
    let callables: HashMap<NodeId, &NodeRow> =
        nodes.iter().filter(|n| Want::Callable.admits(n.kind)).map(|n| (n.id, n)).collect();
    let mut map: HashMap<(NodeId, String), String> = HashMap::new();
    for e in edges.iter().filter(|e| e.kind == EdgeKind::Contains) {
        let Some(method) = callables.get(&e.target) else { continue };
        if info.get(&e.source).is_some_and(|i| i.kind == NodeKind::Trait) {
            map.entry((e.source, method.name.clone()))
                .or_insert_with(|| method.symbol.as_str().to_string());
        }
    }
    map
}

/// Build the [`Hierarchy`] from the `Extends` rows the type-relation arm binds
/// ([`type_relation_want`](super::type_relation_want)): a package-shaped source's, and in every language a
/// type's.
///
/// Each row is bound by [`bind`] itself — S-466's type-relation arm, which
/// reads the source's scope and module model and never the policy-gated
/// fallbacks (`scope_only`), so the policy passed here moves nothing. Only a
/// row that binds as `Extends` is a supertype: a silent-syntax supertype that
/// binds an interface records `Implements` ([`relation_edge_kind`](super::relation_edge_kind)) and stays
/// out, as a Java class's `implements` does. A row of any other arm is
/// skipped, and the map stays empty for a graph without one — a Rust graph —
/// exactly as before. A type with a supertype row that binds no walked
/// `Extends` — unbound, an interface, one of several bases — or a used trait
/// is [`open`](super::hierarchy::Hierarchy::open) (S-592): the walk cannot see all it
/// inherits.
///
/// The walk reads one superclass per level, nearest first — the order of
/// single inheritance. Two shapes break it, and neither binds a guess (S-522,
/// [NFR-RA-05]):
///
/// - **several bases** — two or more rows that may each name a base class: a
///   row bound to a class, or an unbound one (an external base). Python's
///   `class C(A, B)` looks a method up in its MRO (`C, A, A's bases, B`), not
///   level by level, and an unbound base ahead of a bound one may supply it.
///   Such a class **has no supertypes here**, so a `self`/`super` call climbs no
///   further than its own members. Where the list leaves kinds unsaid
///   ([`Want::Supertype`], C#, Kotlin) its one base class sits among
///   interfaces, so an unbound entry there is not counted. An interface's
///   super-interfaces are pooled, as before;
/// - **a used trait** (a PHP `use`, [`TRAIT_USE_ALIAS`]) — the trait's method
///   outranks every inherited one, and an unbound trait's methods are not in
///   the graph. Such a class is a [`trait_users`](super::hierarchy::Hierarchy::trait_users) entry,
///   which no walk climbs through.
///
/// A language that inherits interface member bodies (S-609, [FR-RS-48];
/// [`PackageLayout::inherits_interface_bodies`]) also keeps each type's
/// in-repository interfaces ([`Hierarchy::interfaces`](super::hierarchy::Hierarchy::interfaces)): a supertype row bound as
/// `Implements`, whether its clause spells it so or its kind follows the target.
/// Their names join the tokens, so a sync that gives an interface a default
/// body, or takes one away, re-selects the calls a walk may move. Such a class
/// with a supertype row that binds nothing and may name its base class is an
/// [`unseen_bases`](super::hierarchy::Hierarchy::unseen_bases) entry, as is a class of any language
/// with several bases. Every other language records no interface, so its
/// walks are exactly as before.
///
/// A Ruby class's superclass is an `Extends` like any other (S-568), and its
/// mixins are filed apart ([`add_mixin`], [`Hierarchy::mixins`](super::hierarchy::Hierarchy::mixins)): an `include` or
/// a `prepend` is no supertype of single inheritance, and the walk that reads
/// them is Ruby's own ([`SupertypeClimb::ancestor_walk`](super::supertype_climb::SupertypeClimb::ancestor_walk)). Both join the tokens, with
/// the source's own name.
///
/// Only the subtype's own rows count. A capture-before-delete `Symbol` row
/// ([ADR-10]) is filed under the **supertype's** file and outlives the
/// subtype's `extends` clause until that file is re-extracted, so reading it
/// would lend a type a supertype it no longer declares — and bind new calls
/// through it — where a cold index has none. Its target is also a whole
/// symbol, whose path tokens would re-select every package-shaped call on
/// every Java sync. The source's own `Path` row is always present while the
/// clause is, so skipping the capture loses no supertype.
///
/// [CR-150]: ../../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
/// [ADR-10]: ../../../../docs/specs/architecture/decisions/ADR-10.md
/// [FR-RS-48]: ../../../../docs/specs/requirements/FR-RS-48.md
fn build_supertypes(refs: &[UnresolvedRefRow], ix: &Index) -> Hierarchy {
    let mut hierarchy = Hierarchy::default();
    let mut bases: HashMap<NodeId, usize> = HashMap::new();
    for r in refs {
        if r.form == RefForm::Symbol || !matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements) {
            continue;
        }
        // A partial type part's own row names its type, never a supertype
        // (S-557): the parts' supertypes are united once the walk's map is
        // built ([`PartialTypes::unite_hierarchy`]).
        if r.alias.as_deref() == Some(PARTIAL_TYPE_ALIAS) {
            continue;
        }
        let Some(&source_id) = ix.by_symbol.get(&r.source_symbol) else {
            continue;
        };
        let source = ix.info.get(&source_id);
        let Some(want) = relation_want_of(r, source, &ix.layout) else {
            continue;
        };
        let path = source.and_then(|i| i.file_path.as_deref());
        // A Ruby class or module (S-568) is watched under its own name too:
        // its reopenings in other files pool their ancestry, which a sync
        // re-extracting one of them moves under that name alone.
        let constants = path.is_some_and(|p| ix.layout.resolves_lexical_constants(p));
        if constants {
            hierarchy.tokens.extend(source.into_iter().flat_map(|i| super::super::tokens(&i.name)));
        }
        if constants && r.kind == EdgeKind::Implements {
            hierarchy.tokens.extend(super::super::tokens(&r.target));
            hierarchy.families.extend(path.and_then(|p| ix.layout.family(p)));
            add_mixin(&mut hierarchy.mixins, source_id, r, ix);
            continue;
        }
        // Whether the source's language inherits interface member bodies
        // (S-609): only then is an interface a level the walk visits.
        let inherits = path.is_some_and(|p| ix.layout.inherits_interface_bodies(p));
        if r.kind == EdgeKind::Implements && !inherits {
            if r.alias.as_deref() == Some(TRAIT_USE_ALIAS) {
                hierarchy.trait_users.insert(source_id);
            }
            hierarchy.open.insert(source_id);
            continue;
        }
        hierarchy.tokens.extend(super::super::tokens(&r.target));
        hierarchy.families.extend(path.and_then(|p| ix.layout.family(p)));
        let class = source.is_some_and(|i| i.kind != NodeKind::Interface);
        match bind(r, ix, BindingPolicy::Strict) {
            Outcome::Bound {
                target,
                kind: EdgeKind::Extends,
                ..
            } if r.kind == EdgeKind::Extends => {
                hierarchy.supertypes.entry(source_id).or_default().push(target);
                *bases.entry(source_id).or_default() += usize::from(class);
            }
            Outcome::Bound {
                target,
                kind: EdgeKind::Implements,
                ..
            } if inherits => {
                hierarchy.interfaces.entry(source_id).or_default().push(target);
                hierarchy.open.insert(source_id);
            }
            Outcome::Unbound => {
                if want != Want::Supertype && r.kind == EdgeKind::Extends {
                    *bases.entry(source_id).or_default() += usize::from(class);
                }
                // An unbound `implements` names an interface, never a base.
                if inherits && class && r.kind == EdgeKind::Extends {
                    hierarchy.unseen_bases.insert(source_id);
                }
                hierarchy.open.insert(source_id);
            }
            _ => {
                hierarchy.open.insert(source_id);
            }
        }
    }
    for (id, _) in bases.into_iter().filter(|&(_, n)| n > 1) {
        hierarchy.supertypes.remove(&id);
        hierarchy.open.insert(id);
        hierarchy.unseen_bases.insert(id);
    }
    let mixins = hierarchy.mixins.values_mut().flat_map(|m| [&mut m.includes, &mut m.prepends]);
    for ids in hierarchy.supertypes.values_mut().chain(hierarchy.interfaces.values_mut()).chain(mixins) {
        ids.sort_unstable();
        ids.dedup();
    }
    hierarchy
}

/// File the Ruby mixin row `r` of the class or module `source` (S-568,
/// [FR-RS-35]): an `include`'s or a `prepend`'s module, bound by [`bind`]
/// through the constant index, among its `mixins` — or, bound to none, the
/// mark that a module the walk cannot see sits there. An `extend` is no level
/// of the instances' ancestors and is filed nowhere.
///
/// [FR-RS-35]: ../../../../docs/specs/requirements/FR-RS-35.md
fn add_mixin(mixins: &mut HashMap<NodeId, Mixins>, source: NodeId, r: &UnresolvedRefRow, ix: &Index) {
    let prepend = match r.alias.as_deref() {
        Some(MIXIN_EXTEND_ALIAS) => return,
        alias => alias == Some(MIXIN_PREPEND_ALIAS),
    };
    let entry = mixins.entry(source).or_default();
    match bind(r, ix, BindingPolicy::Strict) {
        Outcome::Bound { target, .. } if prepend => entry.prepends.push(target),
        Outcome::Bound { target, .. } => entry.includes.push(target),
        _ if prepend => entry.unseen_prepends = true,
        _ => entry.unseen_includes = true,
    }
}

/// The build-time half of the partial types (S-557): the parts recorded, a
/// name's lists collapsed to their first-declared parts, the parts' supertypes
/// united. The reads binding makes (`first_of`, `parts_of`) are the kernel's.
impl PartialTypes {
    /// Record the declarations `ids` of one name as the parts of the types
    /// they are (S-557), one type per directory tree ([`directory_trees`]):
    /// a C# type's parts belong to one project, and the directory stands in for
    /// the project, whose `.csproj` is not read — the [`GLOBAL_WILDCARD_ALIAS`](super::GLOBAL_WILDCARD_ALIAS)
    /// precedent. Two projects' `partial class Program` are two types, as they
    /// are to the compiler — and so are `Box` and `Box<T>`, so a tree's parts
    /// are one type per arity. A declaration not marked partial in a tree makes
    /// none of its declarations a part: its arity is unrecorded.
    fn unite(&mut self, ids: &[NodeId], node_by_id: &HashMap<NodeId, &NodeRow>) {
        for tree in directory_trees(ids, node_by_id) {
            if !tree.iter().all(|id| self.marked.contains_key(id)) {
                continue;
            }
            let mut by_arity: BTreeMap<u32, Vec<NodeId>> = BTreeMap::new();
            for id in tree {
                by_arity.entry(self.marked[&id]).or_default().push(id);
            }
            for parts in by_arity.values() {
                self.unite_tree(parts, node_by_id);
            }
        }
    }

    /// Record `ids`, the declarations of one name in one directory tree, as the
    /// parts of one type when they are (S-557): two or more declarations,
    /// every one marked partial and all of one kind — a `partial` declaration
    /// beside a plain one of the same name is two types, an ambiguity never a
    /// union ([NFR-RA-05]). The parts are ordered by file path, then line, so
    /// the first-declared part a cold index and a sync pick is the same
    /// ([NFR-RA-06]).
    ///
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    fn unite_tree(&mut self, ids: &[NodeId], node_by_id: &HashMap<NodeId, &NodeRow>) {
        if ids.len() < 2 || !ids.iter().all(|id| self.marked.contains_key(id)) {
            return;
        }
        let kinds: HashSet<NodeKind> = ids.iter().filter_map(|id| node_by_id.get(id).map(|n| n.kind)).collect();
        if kinds.len() != 1 {
            return;
        }
        let mut parts = ids.to_vec();
        parts.sort_by_key(|id| {
            let n = node_by_id.get(id);
            (n.and_then(|n| n.file_path.clone()), n.and_then(|n| n.start_line), *id)
        });
        parts.dedup();
        let first = parts[0];
        for &part in &parts {
            self.first.insert(part, first);
        }
        self.parts.insert(first, parts);
    }

    /// Read each list of `lists` with every part replaced by its type's
    /// first-declared part, sorted and deduplicated: a name the parts of one
    /// type share names that one type ([FR-RS-29]).
    ///
    /// [FR-RS-29]: ../../../../docs/specs/requirements/FR-RS-29.md
    fn collapse<'a>(&self, lists: impl Iterator<Item = &'a mut Vec<NodeId>>) {
        for list in lists.filter(|list| list.iter().any(|id| self.first.contains_key(id))) {
            for id in list.iter_mut() {
                *id = self.first_of(*id);
            }
            list.sort_unstable();
            list.dedup();
        }
    }

    /// Give every part of each partial type the union of its parts' supertypes
    /// (S-557, [FR-RS-29]): a base list on one part is the whole type's, as are
    /// the interfaces any part implements. A part is never its own type's
    /// supertype. A class whose parts name two base classes has several bases
    /// (`build_supertypes`): none is a supertype, and the type is open with
    /// its bases unseen, as one declaration naming two would be.
    ///
    /// [FR-RS-29]: ../../../../docs/specs/requirements/FR-RS-29.md
    fn unite_hierarchy(&self, hierarchy: &mut Hierarchy, node_by_id: &HashMap<NodeId, &NodeRow>) {
        for parts in self.parts.values() {
            UnitedHierarchy::of(parts, hierarchy, node_by_id).give(parts, hierarchy);
        }
    }
}

/// What every part of one partial type has in the walk's [`Hierarchy`] (S-557,
/// [`PartialTypes::unite_hierarchy`]): the union of the parts' supertypes and
/// interfaces, none of them a part, and whether any part is open, has unseen
/// bases or uses a trait.
struct UnitedHierarchy {
    supertypes: Vec<NodeId>,
    interfaces: Vec<NodeId>,
    open: bool,
    unseen: bool,
    traits: bool,
}

impl UnitedHierarchy {
    /// Unite the facts `hierarchy` holds for `parts`, the first-declared part
    /// first. A class whose parts name two base classes has several bases.
    fn of(parts: &[NodeId], hierarchy: &Hierarchy, node_by_id: &HashMap<NodeId, &NodeRow>) -> Self {
        let union = |map: &HashMap<NodeId, Vec<NodeId>>| {
            let mut ids: Vec<NodeId> = parts
                .iter()
                .filter_map(|p| map.get(p))
                .flatten()
                .copied()
                .filter(|id| !parts.contains(id))
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        let any = |set: &HashSet<NodeId>| parts.iter().any(|p| set.contains(p));
        let mut united = UnitedHierarchy {
            supertypes: union(&hierarchy.supertypes),
            interfaces: union(&hierarchy.interfaces),
            open: any(&hierarchy.open),
            unseen: any(&hierarchy.unseen_bases),
            traits: any(&hierarchy.trait_users),
        };
        let class = node_by_id.get(&parts[0]).is_some_and(|n| n.kind != NodeKind::Interface);
        if class && united.supertypes.len() > 1 {
            united.supertypes.clear();
            united.open = true;
            united.unseen = true;
        }
        united
    }

    /// Record these facts for every part of `parts` in `hierarchy`.
    fn give(&self, parts: &[NodeId], hierarchy: &mut Hierarchy) {
        let assign = |map: &mut HashMap<NodeId, Vec<NodeId>>, part: NodeId, ids: &Vec<NodeId>| {
            if ids.is_empty() {
                map.remove(&part);
            } else {
                map.insert(part, ids.clone());
            }
        };
        for &part in parts {
            assign(&mut hierarchy.supertypes, part, &self.supertypes);
            assign(&mut hierarchy.interfaces, part, &self.interfaces);
            let sets = [
                (&mut hierarchy.open, self.open),
                (&mut hierarchy.unseen_bases, self.unseen),
                (&mut hierarchy.trait_users, self.traits),
            ];
            for (set, _) in sets.into_iter().filter(|(_, member)| *member) {
                set.insert(part);
            }
        }
    }
}

/// `ids` split by the directory tree their files lie in (S-557,
/// [`PartialTypes::unite`]): each tree is rooted at the directory of one of
/// them that no other's directory encloses, and holds every declaration in or
/// below it. `Models/Foo.cs` and `Models/Generated/Foo.g.cs` are one tree;
/// `Catalog/Program.cs` and `Ordering/Program.cs` are two. A declaration with
/// no file is in none. Each tree keeps the order of `ids`.
fn directory_trees(ids: &[NodeId], node_by_id: &HashMap<NodeId, &NodeRow>) -> Vec<Vec<NodeId>> {
    let dir_of = |id: &NodeId| {
        let path = node_by_id.get(id)?.file_path.as_deref()?;
        Some(path.rsplit_once('/').map_or("", |(dir, _)| dir))
    };
    let within = |dir: &str, root: &str| {
        root.is_empty() || dir == root || dir.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
    };
    let dirs: BTreeSet<&str> = ids.iter().filter_map(dir_of).collect();
    dirs.iter()
        .filter(|&&dir| !dirs.iter().any(|&other| other != dir && within(dir, other)))
        .map(|&root| ids.iter().copied().filter(|id| dir_of(id).is_some_and(|dir| within(dir, root))).collect())
        .collect()
}

/// The builder's test-only constructors. A test module, so it is test code to
/// the graph as it is to the compiler.
#[cfg(test)]
mod test_api {
    use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};

    use super::{Index, IndexBuilder, PackageLayout};

    impl IndexBuilder {
        /// [`new`](IndexBuilder::new) with the package-file stems the rust plugin
        /// declares for `.rs` ([`PackageLayout::rust_stems_for_tests`], S-519), so
        /// a synthetic `src/lib.rs` still names its crate — the builder of
        /// [`Index::build`].
        pub(crate) fn for_tests(nodes: &[NodeRow], edges: &[EdgeRow], refs: &[UnresolvedRefRow]) -> IndexBuilder {
            Self::new(nodes, edges, refs, PackageLayout::rust_stems_for_tests())
        }

        /// The builder of `ix` again, for a binder test that reads one index
        /// before and after a further step.
        pub(crate) fn from_index(ix: Index) -> IndexBuilder {
            IndexBuilder { ix }
        }
    }
}
