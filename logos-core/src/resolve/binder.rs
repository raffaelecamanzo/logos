//! The scope-hierarchy binder — the pure core of the resolution engine
//! (S-011, [FR-RS-01], [FR-RS-02], [FR-RS-03], [NFR-RA-05]).
//!
//! [`Index::build`] digests an immutable graph snapshot (nodes, `Contains`
//! edges, the reference ledger) into the lookup structures binding needs;
//! [`bind`] then resolves one ledger row against it. Nothing here touches the
//! store — the caller reads the snapshot and commits the outcomes — so the
//! whole algorithm is deterministic, `Sync`, and unit-testable in memory, and
//! the component's *parallel compute, serial commit* contract holds by
//! construction ([resolution-engine]).
//!
//! # The binding hierarchy ([FR-RS-03])
//!
//! A reference is tried against ever-wider scopes, in order:
//!
//! 1. **function-local / lexical** — the `Contains` ancestor chain of the
//!    referencing declaration, innermost first (a nested `fn`, the enclosing
//!    item, … up to the file module);
//! 2. **module** — the file-module scope is the last step of (1); sibling and
//!    child *file* modules resolve through the path-derived module tree;
//! 3. **imports** — the file's `use`-alias map (including `as` renames), then
//!    its glob imports ([FR-RS-01], [FR-RS-02]);
//! 4. **crate** — explicit `crate::`/`self::`/`super::` paths and
//!    crate-name-headed paths through the module tree;
//! 5. **workspace** — policy-gated unique-candidate fallbacks
//!    ([`BindingPolicy`]).
//!
//! # Never fabricate ([NFR-RA-05])
//!
//! Every level ends in the same acceptance rule: bind **iff the candidate set
//! has exactly one element**. Zero candidates falls through to the next level;
//! two or more is [`Res::Ambiguous`] and aborts the whole attempt — escalating
//! past a *known* ambiguity is how mis-binds happen ([AR-05]). The policy knob
//! widens the search, never the acceptance rule.
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [FR-RS-02]: ../../../docs/specs/requirements/FR-RS-02.md
//! [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
//! [`BindingPolicy`]: crate::config::BindingPolicy

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::BindingPolicy;
use crate::extract::doc::heading_slug;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{ArtifactRelation, EdgeKind, NodeId, NodeKind, RefForm};

use super::route_template::route_key;

/// A module's identity: `(crate name, module path segments)`.
type ModKey = (String, Vec<String>);

/// `Contains` membership: scope → name → member nodes, each list id-sorted for
/// a deterministic candidate order ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
type Members = HashMap<NodeId, HashMap<String, Vec<NodeId>>>;

/// The result of one binding attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Exactly one candidate was found: create `source --kind--> target`.
    Bound {
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
        /// The cross-artifact relation class to stamp on the edge
        /// ([`ArtifactRelation`](crate::model::ArtifactRelation) wire token) — set
        /// for an `ArtifactRef`/`ArtifactBinding` bind (CR-011), `None` for every
        /// code/doc/access bind.
        payload: Option<String>,
    },
    /// One reference that fans out to **several** targets — a Terraform local
    /// module call binding to every admitted `.tf` [`NodeKind::ConfigFile`] in its
    /// source directory (CR-011, [FR-CG-08]). The one cross-artifact relation whose
    /// single ledger row legitimately yields more than one edge: a module `source`
    /// names a *directory*, not a file, and pulls in every `.tf` under it. Each
    /// target becomes one edge sharing the relation payload; the row is resolved
    /// because at least one bound. `targets` is non-empty and `NodeId`-sorted, so
    /// the produced edge set is deterministic ([NFR-RA-06]). Still never-fabricate:
    /// every target is a real indexed `ConfigFile` ([NFR-RA-05]).
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    BoundMany {
        source: NodeId,
        targets: Vec<NodeId>,
        kind: EdgeKind,
        payload: Option<String>,
    },
    /// Zero candidates anywhere, or an ambiguity — the ref stays in the
    /// ledger and is retried on the next sync ([FR-RS-03]).
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    Unbound,
}

/// An intermediate lookup result. `Ambiguous` is sticky: once a scope level
/// *knows* there are two candidates, no wider level may overrule it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Res {
    Found(NodeId),
    NotFound,
    Ambiguous,
}

/// Which node kinds satisfy a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// A call target: `Function` or `Method`.
    Callable,
    /// An import target: any node (module preferred over a same-named item).
    Any,
    /// A glob's target: a module only.
    Module,
    /// A member-access target: a `Field` only (CR-005, [FR-EX-08]).
    ///
    /// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
    Field,
}

impl Want {
    fn admits(self, kind: NodeKind) -> bool {
        match self {
            Want::Callable => matches!(kind, NodeKind::Function | NodeKind::Method),
            Want::Any => true,
            Want::Module => kind == NodeKind::Module,
            Want::Field => kind == NodeKind::Field,
        }
    }
}

/// `true` for a class-bearing container whose lexically-enclosed `Field` members
/// a member access can resolve against (CR-005, [FR-EX-08]): a `Class`/`Struct`/
/// `Interface`/`Enum`/`Trait`. The scope a `self.x`/`this.x` access is bound
/// within — never wider, so an own-field access can never bind to a field of a
/// different type ([NFR-RA-05]).
///
/// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn is_class_like(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Class | NodeKind::Struct | NodeKind::Interface | NodeKind::Enum | NodeKind::Trait
    )
}

/// `true` for kinds whose associated items collapse to module scope (the
/// `Type::func` rule — `impl` blocks are not captured scopes, S-007).
fn is_type_like(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Struct
            | NodeKind::Enum
            | NodeKind::Trait
            | NodeKind::Class
            | NodeKind::Interface
            | NodeKind::TypeAlias
    )
}

/// Maximum alias-chain hops before a lookup gives up — terminates resolution
/// on a self-referential import cycle (the S-011 sprint test) instead of
/// recursing forever.
const MAX_ALIAS_DEPTH: u8 = 8;

/// Hard cap on the `Contains`-hierarchy walk in [`Ctx::typed_owner`] (S-039) — a
/// defensive bound against a malformed cycle, far above any real doc/code
/// nesting depth (markdown headings reach 6; module nesting follows directory
/// depth).
const MAX_CONTAINS_DEPTH: u32 = 64;

/// The per-file scope facts derived from the ledger's import rows.
///
/// A re-extracted file's import rows are replaced wholesale by `persist_file`
/// (delete + reinsert) *before* the resolution pass runs, and untouched
/// files' rows persist (flagged, not deleted, when bound) — so at bind time
/// every file's current `use` aliases are present here. A deferred call ref
/// in an untouched file therefore still sees its file's aliases on every
/// retry.
#[derive(Debug, Default)]
struct FileScope {
    /// In-scope name → the `::`-split path it abbreviates.
    aliases: HashMap<String, Vec<String>>,
    /// Glob-imported module paths (`use m::*` → `["m"]`), unresolved form.
    globs: Vec<Vec<String>>,
}

/// One node's binding-relevant facts.
#[derive(Debug)]
struct NodeInfo {
    kind: NodeKind,
    crate_name: String,
    /// The human-facing name — the heading text of a `DocSection`, the symbol
    /// name of a code node. Carried here for doc resolution (S-035), which
    /// matches a link's `#anchor` against a section's slugified name.
    name: String,
    /// The defining file's project-relative path, when bound to one — the key
    /// doc-link/path references resolve against (S-035).
    file_path: Option<String>,
}

/// The immutable lookup index one resolution run binds against.
pub(crate) struct Index {
    /// Canonical symbol string → node.
    by_symbol: HashMap<String, NodeId>,
    /// Per-node facts.
    info: HashMap<NodeId, NodeInfo>,
    /// `Contains`: child → parent.
    parent: HashMap<NodeId, NodeId>,
    /// `Contains`: scope → name → members, each list sorted by `NodeId`.
    members: Members,
    /// Module tree: `(crate, path)` → module node (file modules from their
    /// paths, inline `mod`s appended beneath them).
    modules: HashMap<ModKey, NodeId>,
    /// The reverse of `modules`, for "what module am I in" walks.
    module_key: HashMap<NodeId, ModKey>,
    /// name → every node carrying it, sorted by `NodeId` (the unique-match
    /// fallback universe).
    by_name: HashMap<String, Vec<NodeId>>,
    /// file path → every node defined in that file, sorted by `NodeId` — the
    /// universe a doc link/path reference resolves against (S-035).
    by_file_path: HashMap<String, Vec<NodeId>>,
    /// `(METHOD, normalized-template)` → the [`NodeKind::Route`] nodes matching
    /// it, sorted by `NodeId` — the universe an OpenAPI `ApiOperation`→route
    /// reference resolves against (S-069, [FR-CG-09]). A route whose template
    /// does not normalize cleanly (catch-all, regex) is **absent** from this map
    /// and so is never a candidate — honestly unresolved, never approximately
    /// matched ([NFR-RA-05]).
    ///
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    routes_by_key: HashMap<(String, String), Vec<NodeId>>,
    /// file id → scope facts from its import rows.
    file_scopes: HashMap<i64, FileScope>,
    /// Normalised crate names present in the graph.
    crates: HashSet<String>,
    /// `(trait node, method name)` → the concrete workspace impl method nodes of
    /// that trait method, id-sorted and deduplicated — the fan-out universe for a
    /// `dyn T` method call (S-281, [CR-073], [FR-RS-08]). Built from the
    /// `Implements` reference rows (impl method → its trait), so it is available
    /// on the very first index pass, before any `Implements` edge is committed.
    ///
    /// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
    /// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
    impls_by_trait_method: HashMap<(NodeId, String), Vec<NodeId>>,
}

impl Index {
    /// Digest a snapshot into the binding index.
    ///
    /// A thin orchestrator over focused sub-builders, each owning one lookup
    /// structure. Ordering is load-bearing: `members` lists and `by_name`
    /// lists are id-sorted, `modules`/`by_symbol` are first-wins on a
    /// duplicate key, and the module tree is built by an id-/name-sorted DFS —
    /// so every helper preserves the same canonical iteration order the
    /// monolith had, keeping the built `Index` byte-identical ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub(crate) fn build(nodes: &[NodeRow], edges: &[EdgeRow], refs: &[UnresolvedRefRow]) -> Index {
        // Built once and shared: module-tree construction and containment both
        // look nodes up by id.
        let node_by_id: HashMap<NodeId, &NodeRow> = nodes.iter().map(|n| (n.id, n)).collect();

        // Contains topology first — module-tree construction needs it.
        let (parent, members) = build_containment(edges, &node_by_id);
        let (modules, module_key) = build_module_tree(nodes, &parent, &members, &node_by_id);
        let crates: HashSet<String> = modules.keys().map(|(c, _)| c.clone()).collect();

        let info = build_node_info(nodes, &parent, &module_key);
        let by_symbol = build_by_symbol(nodes);
        let by_name = build_by_name(nodes);
        let by_file_path = build_by_file_path(nodes);
        let routes_by_key = build_routes_by_key(nodes);
        let file_scopes = build_file_scopes(refs);
        let impls_by_trait_method =
            build_impls_by_trait_method(refs, &by_symbol, &by_name, &info);

        Index {
            by_symbol,
            info,
            parent,
            members,
            modules,
            module_key,
            by_name,
            by_file_path,
            routes_by_key,
            file_scopes,
            crates,
            impls_by_trait_method,
        }
    }

    /// The one workspace [`NodeKind::Trait`] node named `name`, or `None` when
    /// zero or several carry the name — the never-fabricate acceptance rule
    /// ([NFR-RA-05]) applied to trait resolution: a `dyn T` call whose trait is
    /// ambiguous or external stays an honest miss rather than guessing one.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn trait_by_name(&self, name: &str) -> Option<NodeId> {
        unique_trait(&self.by_name, &self.info, name)
    }

    /// The concrete workspace impl method nodes of trait method `(trait, name)`,
    /// id-sorted; empty when none are recorded.
    fn impls_of(&self, trait_node: NodeId, name: &str) -> &[NodeId] {
        self.impls_by_trait_method
            .get(&(trait_node, name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// Members of `scope` named `name`, filtered by `want`.
    fn members_named(&self, scope: NodeId, name: &str, want: Want) -> Vec<NodeId> {
        self.members
            .get(&scope)
            .and_then(|m| m.get(name))
            .map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| self.info.get(id).is_some_and(|i| want.admits(i.kind)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The module key of the nearest enclosing module of `node` (itself
    /// included), if any.
    fn nearest_module(&self, node: NodeId) -> Option<&ModKey> {
        let mut cursor = Some(node);
        while let Some(id) = cursor {
            if let Some(key) = self.module_key.get(&id) {
                return Some(key);
            }
            cursor = self.parent.get(&id).copied();
        }
        None
    }

    /// Whether re-binding `r` could change its outcome given `dirty` — the tokens
    /// a sync added or removed (see [`tokens`](super::tokens)).
    ///
    /// A row is affected when a token of its target lands in `dirty`, or when an
    /// `as`-alias in the row's file rewrites that target's head into a path that
    /// does. Globs need no expansion: a glob-imported call resolves under its own
    /// bare name, already a target token; only renaming aliases route a reference
    /// to a name it does not spell. The alias chase is bounded by
    /// [`MAX_ALIAS_DEPTH`], the same cap [`Ctx::resolve_path`] honours, so a
    /// self-referential import cannot loop here either.
    pub(crate) fn ref_affected(&self, r: &UnresolvedRefRow, dirty: &HashSet<String>) -> bool {
        if super::tokens(&r.target).iter().any(|t| dirty.contains(t)) {
            return true;
        }
        let Some(file_id) = r.file_id else {
            return false;
        };
        let Some(scope) = self.file_scopes.get(&file_id) else {
            return false;
        };
        // Chase the head segment through `as`-alias rewrites: each hop's path may
        // name something dirty even though the written target does not.
        let mut head = r.target.split("::").next().unwrap_or_default().to_string();
        for _ in 0..MAX_ALIAS_DEPTH {
            let Some(path) = scope.aliases.get(&head) else {
                return false;
            };
            if path
                .iter()
                .flat_map(|seg| super::tokens(seg))
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
/// *file module* keyed by its file path; inline `mod`s hang beneath it. File
/// roots are visited id-sorted so the first id wins a (rare) path tie, exactly
/// as the monolith did.
fn build_module_tree(
    nodes: &[NodeRow],
    parent: &HashMap<NodeId, NodeId>,
    members: &Members,
    node_by_id: &HashMap<NodeId, &NodeRow>,
) -> (HashMap<ModKey, NodeId>, HashMap<NodeId, ModKey>) {
    let mut modules: HashMap<ModKey, NodeId> = HashMap::new();
    let mut module_key: HashMap<NodeId, ModKey> = HashMap::new();
    let mut file_roots: Vec<&NodeRow> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module && !parent.contains_key(&n.id))
        .collect();
    file_roots.sort_by_key(|n| n.id); // first-by-id wins a (rare) path tie
    for root in file_roots {
        let Some(path) = &root.file_path else {
            continue; // an orphaned module node cannot anchor a tree
        };
        let key = module_key_for_file(path);
        modules.entry(key.clone()).or_insert(root.id);
        module_key.insert(root.id, key.clone());
        append_inline_modules(
            root.id,
            key,
            members,
            node_by_id,
            &mut modules,
            &mut module_key,
        );
    }
    (modules, module_key)
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
) -> HashMap<NodeId, NodeInfo> {
    let mut info: HashMap<NodeId, NodeInfo> = HashMap::new();
    for n in nodes {
        info.insert(
            n.id,
            NodeInfo {
                kind: n.kind,
                crate_name: crate_of_node(n, parent, module_key),
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
) -> String {
    let mut cursor = Some(n.id);
    while let Some(id) = cursor {
        if let Some((c, _)) = module_key.get(&id) {
            return c.clone();
        }
        cursor = parent.get(&id).copied();
    }
    match &n.file_path {
        Some(path) => module_key_for_file(path).0,
        None => String::new(),
    }
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

/// `(METHOD, normalized-template)` → route nodes, for the OpenAPI
/// operation→route match (S-069). A route name is `"METHOD /path"`; its key is
/// the upper-cased method plus the positionally-normalized template (parameter
/// names/syntax erased). A route whose template does not normalize cleanly is
/// skipped, so it can never become a candidate ([FR-CG-09], [NFR-RA-05]).
/// `nodes` is id-ordered, so each list is already sorted by id.
fn build_routes_by_key(nodes: &[NodeRow]) -> HashMap<(String, String), Vec<NodeId>> {
    let mut routes_by_key: HashMap<(String, String), Vec<NodeId>> = HashMap::new();
    for n in nodes {
        if n.kind != NodeKind::Route {
            continue;
        }
        let Some(key) = route_key(&n.name) else {
            continue;
        };
        routes_by_key.entry(key).or_default().push(n.id);
    }
    routes_by_key
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
        let scope = file_scopes.entry(file_id).or_default();
        let path: Vec<String> = r.target.split("::").map(str::to_string).collect();
        match r.form {
            RefForm::Glob => scope.globs.push(path),
            _ => {
                if let Some(alias) = &r.alias {
                    scope.aliases.entry(alias.clone()).or_insert(path);
                }
            }
        }
    }
    file_scopes
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
/// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
/// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn build_impls_by_trait_method(
    refs: &[UnresolvedRefRow],
    by_symbol: &HashMap<String, NodeId>,
    by_name: &HashMap<String, Vec<NodeId>>,
    info: &HashMap<NodeId, NodeInfo>,
) -> HashMap<(NodeId, String), Vec<NodeId>> {
    let mut map: HashMap<(NodeId, String), Vec<NodeId>> = HashMap::new();
    for r in refs {
        if r.kind != EdgeKind::Implements {
            continue;
        }
        let Some(&impl_method) = by_symbol.get(&r.source_symbol) else {
            continue; // the impl method's own node is not indexed — skip
        };
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

/// The one workspace [`NodeKind::Trait`] node named `name`, or `None` when zero
/// or several carry it — the single source of truth for the never-fabricate
/// trait-resolution rule ([NFR-RA-05]) shared by [`Index::trait_by_name`] (query
/// time) and [`build_impls_by_trait_method`] (index-build time), so the two can
/// never drift.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn unique_trait(
    by_name: &HashMap<String, Vec<NodeId>>,
    info: &HashMap<NodeId, NodeInfo>,
    name: &str,
) -> Option<NodeId> {
    let traits: Vec<NodeId> = by_name
        .get(name)
        .into_iter()
        .flatten()
        .copied()
        .filter(|id| info.get(id).is_some_and(|i| i.kind == NodeKind::Trait))
        .collect();
    match traits.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// Reduce a candidate list to a [`Res`] — the single acceptance rule every
/// scope level shares ([NFR-RA-05]: exactly one, or nothing).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn exactly_one(candidates: &[NodeId]) -> Res {
    match candidates {
        [one] => Res::Found(*one),
        [] => Res::NotFound,
        _ => Res::Ambiguous,
    }
}

/// Derive a file's module identity from its project-relative path.
///
/// The segment before the last `src/` names the crate (normalised `-` → `_`,
/// `crate` when there is none); segments after it are modules, with the
/// `mod`/`lib`/`main` stems naming their enclosing module rather than adding a
/// segment. `logos-core/src/extract/mod.rs` → `("logos_core", ["extract"])`.
fn module_key_for_file(path: &str) -> ModKey {
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
    if let Some(last) = mods.pop() {
        let stem = Path::new(&last)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&last)
            .to_string();
        if !matches!(stem.as_str(), "mod" | "lib" | "main") {
            mods.push(stem);
        }
    }
    (crate_name, mods)
}

/// Normalise a crate directory name to its extern-path form (`-` → `_`).
fn normalize_crate(name: &str) -> String {
    name.replace('-', "_")
}

/// Bind one ledger row against the index under `policy`.
pub(crate) fn bind(r: &UnresolvedRefRow, ix: &Index, policy: BindingPolicy) -> Outcome {
    // No source node, no edge — a captured ref whose source file was removed
    // stays unbound (and harmless) until its file returns.
    let Some(&source) = ix.by_symbol.get(&r.source_symbol) else {
        return Outcome::Unbound;
    };
    let bound = |target: NodeId| Outcome::Bound {
        source,
        target,
        kind: r.kind,
        // Non-artifact binds carry no payload; an artifact bind routes through
        // `Ctx::resolve_artifact` below, which stamps the relation class.
        payload: r.payload.clone(),
    };

    let ctx = Ctx {
        source,
        file_id: r.file_id,
        ix,
        policy,
        in_glob_resolution: Cell::new(false),
        no_workspace_fallback: Cell::new(false),
        bare_path_call: Cell::new(false),
    };

    // A cross-artifact reference (CR-011, FR-CG-07): bind under the same
    // exactly-one-candidate discipline as code and docs, dispatched by
    // (kind, form). The relation class rides on `r.payload` onto the edge so
    // navigation can surface which relation a binding expresses; externals were
    // classified out before the ledger, so every ledger row here is a genuine
    // workspace-relative candidate ([NFR-RA-05], [ADR-26]).
    if r.kind.is_config_reference() {
        return ctx.resolve_artifact(r);
    }

    // A member-access fact (CR-005, FR-EX-08): bind to a `Field` of the source
    // method's own class-like container, on an exactly-one candidate — never
    // through the scope hierarchy and never policy-widened, so an own-field
    // access can only ever bind to a field of its own type or stay unresolved
    // (NFR-RA-05). The `target` is the bare field name (the `Method` ref form).
    if r.kind == EdgeKind::Accesses {
        return match ctx.resolve_member_access(&r.target) {
            Res::Found(target) => bound(target),
            _ => Outcome::Unbound,
        };
    }

    // A trait-implementation fact (S-281, CR-073, FR-RS-08): an `impl T for X`
    // method points at its trait `T`. Bind the impl method to the one workspace
    // Trait node named by the target's last segment — never on zero or several
    // (an external / ambiguous trait stays an honest miss, NFR-RA-05). This edge
    // is the structural link the `dyn T` fan-out below enumerates impls from; it
    // is a structural fact, not a code coupling, so hydration fences it out of
    // the dependency subgraph the gated metrics run on (mirroring `Accesses`).
    if r.kind == EdgeKind::Implements {
        let last = r.target.rsplit("::").next().unwrap_or(&r.target);
        return match ix.trait_by_name(last) {
            Some(target) => bound(target),
            None => Outcome::Unbound,
        };
    }

    match r.form {
        // Capture-before-delete refs carry an exact canonical symbol: pure
        // lookup, no inference (ADR-10).
        RefForm::Symbol => match ix.by_symbol.get(&r.target) {
            Some(&target) => bound(target),
            None => Outcome::Unbound,
        },
        RefForm::Glob => {
            let segs = split(&r.target);
            match ctx.resolve_path(&segs, Want::Module, MAX_ALIAS_DEPTH) {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            }
        }
        RefForm::Path => {
            // A documentation link/path (S-035): resolve the href (path +
            // optional `#anchor`) against the doc/code file at that path, never
            // through the code scope hierarchy ([FR-DG-03], [FR-DG-04]).
            if r.kind == EdgeKind::DocReference {
                return match ctx.resolve_doc_link(&r.target) {
                    Res::Found(target) => ctx.bind_doc_ref(source, target),
                    _ => Outcome::Unbound,
                };
            }
            let want = if r.kind == EdgeKind::Calls {
                Want::Callable
            } else {
                Want::Any
            };
            let segs = split(&r.target);
            // A *single-segment* bare-path call enables the CR-068 Part B tie-break
            // (free function over same-named associated methods, [FR-RS-07]). Gated
            // on `segs.len() == 1` so the flag is provably inert for a
            // path-qualified call (`Type::f`, routed through `descend`) and for an
            // import (`Want::Any`); scoped to this resolution, so the
            // receiver-method branch below — which also uses `Want::Callable` — is
            // unaffected.
            ctx.bare_path_call
                .set(r.kind == EdgeKind::Calls && segs.len() == 1);
            let resolved = ctx.resolve_path(&segs, want, MAX_ALIAS_DEPTH);
            ctx.bare_path_call.set(false);
            match resolved {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            }
        }
        RefForm::Method => {
            // A documentation code-name token (S-035): bind to the one code
            // symbol of that name workspace-wide — never on zero or several
            // ([FR-DG-04], [NFR-RA-05]). Policy-independent: doc→code is always
            // exactly-one-or-nothing.
            if r.kind == EdgeKind::DocReference {
                return match ctx.resolve_doc_code_name(&r.target) {
                    Res::Found(target) => ctx.bind_doc_ref(source, target),
                    _ => Outcome::Unbound,
                };
            }
            // A trait-object dynamic-dispatch call (S-281, CR-073, FR-RS-08):
            // extraction encodes a *provable* `&dyn T` receiver on `p.f()` as a
            // trait-qualified `T::f` target (a bare `.f()` stays a single segment
            // and never reaches here — the CR-066 receiver-method guard,
            // [FR-RS-06], is untouched). Fan out to the SET of that trait method's
            // targets: the trait's own default body (a member of the trait node)
            // ∪ every concrete workspace impl of it. Every target is a real
            // indexed node reached through the *proven* trait `T` — never a
            // same-named method on an unrelated type, never a free function, never
            // a stdlib/external target ([NFR-RA-05]). A trait that is external or
            // ambiguous, or one with neither a default body nor a workspace impl,
            // yields no target and stays an honest miss.
            if r.kind == EdgeKind::Calls && r.target.contains("::") {
                return ctx.resolve_dyn_dispatch(source, &r.target);
            }
            // A receiver-unqualified method call (`x.f()` → `f`): extraction
            // records only the bare method name and discards the receiver
            // ([FR-EX-08], `method_calls_record_only_the_name_as_method_form`),
            // so the receiver *type* is unknown. Resolve it through genuine
            // **scope evidence** only — the lexical / module / import / glob
            // hierarchy ([`resolve_name`]) — with the workspace unique-name
            // fallback **suppressed**. That fallback (`unique_by_name` →
            // `prefer_crate`) is what fabricated the pathology: a `.map()` in
            // one module binding to a lone `fn map` in an unrelated module,
            // funnelling ~29.5% of `Calls` edges into ~15 std-method-named
            // targets ([CR-066], [FR-RS-06], [NFR-RA-05]). A method call whose
            // name is not in the caller's own scope therefore stays in
            // `unresolved_refs` and retries on sync ([FR-RS-03]); a same-scope
            // call (a sibling/module-level callable) still binds on real scope
            // evidence, and a typed/path-qualified call is a `RefForm::Path`
            // bound through `resolve_path` above — no typed-call recall loss.
            //
            // [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
            // [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
            // [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
            ctx.no_workspace_fallback.set(true);
            let resolved = ctx.resolve_name(&r.target, Want::Callable, MAX_ALIAS_DEPTH);
            ctx.no_workspace_fallback.set(false);
            match resolved {
                Res::Found(target) => bound(target),
                _ => Outcome::Unbound,
            }
        }
    }
}

/// `true` for the documentation node kinds — the doc→code matcher excludes
/// these so a code-name token binds only to *code* ([FR-DG-04]). Thin alias
/// over the canonical [`NodeKind::is_doc`] so the rule has one source of truth;
/// it equals the file-level ∪ section-level families defined below.
fn is_doc_kind(kind: NodeKind) -> bool {
    kind.is_doc()
}

/// `true` for the swe-skills *typed* doc node kinds (S-039, [FR-DG-07]) — the
/// promoted `Requirement`/`Adr`/`Story`. A resolved doc→doc reference between
/// two of these is a typed trace ([`EdgeKind::TracesTo`]) rather than a generic
/// [`EdgeKind::DocReference`] (see [`Ctx::doc_reference_kind`]).
///
/// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
fn is_typed_doc_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Requirement | NodeKind::Adr | NodeKind::Story
    )
}

/// `true` for a *file-level* doc node: the generic [`NodeKind::DocFile`] or a
/// typed file artifact promoted from one (`Requirement`/`Adr`, S-039). A
/// no-anchor doc link resolves to exactly one of these, so promotion leaves
/// S-035 doc→doc resolution intact ([FR-DG-03], [FR-DG-07]).
fn is_doc_file_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::DocFile | NodeKind::Requirement | NodeKind::Adr
    )
}

/// `true` for a *section-level* doc node: the generic [`NodeKind::DocSection`]
/// or a typed `Story` promoted from one (S-039). An anchored doc link resolves
/// to exactly one of these ([FR-DG-03], [FR-DG-07]).
fn is_doc_section_kind(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::DocSection | NodeKind::Story)
}

/// Split a documentation link `target` into its path part and the lower-cased
/// `#anchor` (an empty anchor is treated as absent).
fn split_anchor(target: &str) -> (&str, Option<String>) {
    match target.split_once('#') {
        Some((p, a)) if !a.is_empty() => (p, Some(a.to_string())),
        Some((p, _)) => (p, None),
        None => (target, None),
    }
}

/// Fold a link `path_part`'s `.`/`..`/leading-`/` against `base_dir` (the
/// directory segments to resolve relative to) into a normalised
/// project-relative path, or `None` if it escapes the repository root.
///
/// Pure and deterministic — the link and its target agree because both sides
/// share this normalisation and [`heading_slug`].
fn fold_path(base_dir: &[&str], path_part: &str) -> Option<String> {
    // A leading `/` is repo-root-relative; otherwise resolve against `base_dir`.
    let mut segs: Vec<&str> = if path_part.starts_with('/') {
        Vec::new()
    } else {
        base_dir.to_vec()
    };
    for seg in path_part.split('/').filter(|s| !s.is_empty()) {
        match seg {
            "." => {}
            ".." => {
                // An escape above the repo root is not a resolvable target.
                segs.pop()?;
            }
            other => segs.push(other),
        }
    }
    Some(segs.join("/"))
}

/// The directory segments of `file` (the path with its file name dropped).
fn dir_segments(file: &str) -> Vec<&str> {
    let mut dir: Vec<&str> = file.split('/').collect();
    dir.pop(); // drop the file name, keep the directory
    dir.into_iter().filter(|s| !s.is_empty()).collect()
}

fn split(target: &str) -> Vec<String> {
    target
        .split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// One binding attempt's context: the referencing node and its file's scope.
///
/// Single-threaded: one `Ctx` per [`bind`] call, never shared across threads
/// (the parallel pass gives each ref its own), so the `Cell` guard below needs
/// no synchronization.
struct Ctx<'a> {
    source: NodeId,
    file_id: Option<i64>,
    ix: &'a Index,
    policy: BindingPolicy,
    /// Re-entrancy guard for [`through_globs`](Ctx::through_globs): set while a
    /// glob's own module path is being resolved, so that resolution cannot fan
    /// back out through the file's glob set. Without it, a file with `G` glob
    /// imports drives `O(G^MAX_ALIAS_DEPTH)` work per reference (CR-016).
    in_glob_resolution: Cell<bool>,
    /// Suppress the policy-gated **workspace unique-name** fallbacks
    /// ([`unique_by_name`](Ctx::unique_by_name) at the aggressive bare-name step,
    /// [`suffix_match`](Ctx::suffix_match) at the balanced multi-segment step) for
    /// the duration of one resolution. Set while resolving a receiver-unqualified
    /// **method** call: the receiver type is unknown, so a bare workspace
    /// name-match is not evidence and would fabricate a cross-module `Calls` edge
    /// ([CR-066], [FR-RS-06], [NFR-RA-05]). Genuine scope levels (lexical, module,
    /// imports, globs) still resolve — only the name-only workspace tier is off.
    ///
    /// [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
    /// [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
    no_workspace_fallback: Cell<bool>,
    /// Enable the CR-068 Part B free-function/associated-method tie-break in
    /// [`prefer_free_functions`](Ctx::prefer_free_functions) for the duration of
    /// one resolution. Set **only** while resolving a single-segment bare-**path**
    /// call ([`RefForm::Path`] + [`EdgeKind::Calls`]) — precisely the shape that
    /// must bind the one free function over same-named associated methods
    /// ([FR-RS-07]). Left `false` for a receiver-unqualified **method** call
    /// ([`RefForm::Method`]), which also resolves through
    /// [`resolve_name`](Ctx::resolve_name) with [`Want::Callable`] but must stay
    /// unchanged (the receiver type is unknown; the CR-066 discipline is not
    /// loosened) — so the tie-break cannot be keyed on `want` alone.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    bare_path_call: Cell<bool>,
}

impl Ctx<'_> {
    fn scope(&self) -> Option<&FileScope> {
        self.file_id.and_then(|id| self.ix.file_scopes.get(&id))
    }

    /// The source's crate and module path (the `self`/`super`/relative base).
    fn source_module(&self) -> Option<ModKey> {
        self.ix.nearest_module(self.source).cloned()
    }

    /// Resolve a multi-or-single segment path by the scope hierarchy.
    fn resolve_path(&self, segs: &[String], want: Want, depth: u8) -> Res {
        if segs.is_empty() || depth == 0 {
            return Res::NotFound;
        }
        if segs.len() == 1 {
            return self.resolve_name(&segs[0], want, depth);
        }

        let head = segs[0].as_str();
        let rest = &segs[1..];
        let source_mod = self.source_module();

        // 1) `crate::…` — the source's crate root.
        if head == "crate" {
            if let Some((krate, _)) = &source_mod {
                return self.descend(krate, &[], rest, want);
            }
            return Res::NotFound;
        }
        // 2) `self::…` — the source's module.
        if head == "self" {
            if let Some((krate, mods)) = &source_mod {
                return self.descend(krate, mods, rest, want);
            }
            return Res::NotFound;
        }
        // 3) `super::…` (possibly chained) — ancestors of the source module.
        if head == "super" {
            let Some((krate, mods)) = &source_mod else {
                return Res::NotFound;
            };
            let supers = segs.iter().take_while(|s| s.as_str() == "super").count();
            if supers > mods.len() {
                return Res::NotFound; // more supers than module depth
            }
            let base = &mods[..mods.len() - supers];
            return self.descend(krate, base, &segs[supers..], want);
        }
        // 4) A `use`-alias head: substitute and resolve the expansion
        //    (depth-limited — an import cycle terminates as NotFound).
        if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(head)) {
            let mut expanded = alias_path.clone();
            expanded.extend(rest.iter().cloned());
            match self.resolve_path(&expanded, want, depth - 1) {
                Res::NotFound => {} // fall through to wider scopes
                decided => return decided,
            }
        }
        // 5) A crate-name head (`logos_core::…`).
        let norm = normalize_crate(head);
        if self.ix.crates.contains(&norm) {
            match self.descend(&norm, &[], rest, want) {
                Res::NotFound => {} // a same-named module may still match below
                decided => return decided,
            }
        }
        // 6) Relative to the source module (`child_mod::item`), then to the
        //    crate root.
        if let Some((krate, mods)) = &source_mod {
            match self.descend(krate, mods, segs, want) {
                Res::NotFound => {}
                decided => return decided,
            }
            match self.descend(krate, &[], segs, want) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        // 7) Through the file's glob imports: candidates across every glob
        //    module, exactly-one overall.
        match self.through_globs(segs, want, depth) {
            Res::NotFound => {}
            decided => return decided,
        }
        // 8) Policy-gated workspace fallback: unique module-path-suffix match —
        //    off while resolving a receiver-method call (CR-066), so an alias
        //    expansion cannot reach the workspace name tier either.
        if self.policy != BindingPolicy::Strict && !self.no_workspace_fallback.get() {
            return self.suffix_match(segs, want);
        }
        Res::NotFound
    }

    /// Resolve a member-access fact to the one `Field` of the source method's
    /// own class-like container named `field` (CR-005, [FR-EX-08]).
    ///
    /// Walks the source's `Contains` ancestry to its nearest enclosing
    /// class-bearing container ([`is_class_like`]) and accepts the field iff that
    /// container directly contains **exactly one** `Field` of that name — the
    /// same single acceptance rule every binder level shares ([NFR-RA-05]). A
    /// language whose methods are not lexically nested under their type (so no
    /// class-like ancestor is found) or whose fields are not extracted as nodes
    /// yields no candidate, so the access stays honestly unresolved and retries
    /// on sync — never fabricated. Bounded by [`MAX_CONTAINS_DEPTH`] against a
    /// malformed `Contains` cycle, mirroring [`typed_owner`](Ctx::typed_owner).
    ///
    /// [FR-EX-08]: ../../../docs/specs/requirements/FR-EX-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_member_access(&self, field: &str) -> Res {
        let mut cursor = Some(self.source);
        for _ in 0..MAX_CONTAINS_DEPTH {
            let Some(id) = cursor else {
                return Res::NotFound;
            };
            if self.ix.info.get(&id).is_some_and(|i| is_class_like(i.kind)) {
                return exactly_one(&self.ix.members_named(id, field, Want::Field));
            }
            cursor = self.ix.parent.get(&id).copied();
        }
        Res::NotFound
    }

    /// Fan out a trait-object dynamic-dispatch call `T::f` to the SET of that
    /// trait method's targets (S-281, [CR-073], [FR-RS-08]).
    ///
    /// `target` is the trait-qualified form extraction emits for a *provable*
    /// `&dyn T` receiver — the method is the last `::` segment, the trait its
    /// predecessor. The fan-out set is the trait's own default body (a
    /// [`Want::Callable`] member of the resolved [`NodeKind::Trait`] node) ∪ every
    /// concrete workspace impl of that method ([`Index::impls_of`]). Union
    /// reachability: any impl — or the default, for an impl that does not override
    /// — is a legitimate runtime dispatch target ([FR-AN-01]).
    ///
    /// Never fabricate ([NFR-RA-05]): every target is a real indexed node reached
    /// through the **proven** trait `T`, so no same-named method on an unrelated
    /// type, free function, or external target can enter the set. A trait that is
    /// external or ambiguously named, or one with neither a default body nor a
    /// workspace impl of `f`, yields an empty set and stays an honest miss. The
    /// set is id-sorted and deduped for a deterministic edge order ([NFR-RA-06]).
    ///
    /// [CR-073]: ../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
    /// [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    fn resolve_dyn_dispatch(&self, source: NodeId, target: &str) -> Outcome {
        // `T::f` — the method is the last `::` segment, the trait its predecessor.
        // A malformed target (`::f`, `T::`, a single segment) cannot reach here
        // (`target.contains("::")` gated the call) but a missing predecessor is
        // handled defensively as an honest miss.
        let mut segs = target.rsplit("::");
        let (Some(method), Some(trait_name)) = (segs.next(), segs.next()) else {
            return Outcome::Unbound;
        };
        let Some(trait_node) = self.ix.trait_by_name(trait_name) else {
            return Outcome::Unbound; // external / ambiguous trait — honest miss
        };
        let mut targets = self.ix.members_named(trait_node, method, Want::Callable);
        targets.extend_from_slice(self.ix.impls_of(trait_node, method));
        targets.sort_unstable();
        targets.dedup();
        if targets.is_empty() {
            return Outcome::Unbound;
        }
        Outcome::BoundMany {
            source,
            targets,
            kind: EdgeKind::Calls,
            payload: None,
        }
    }

    /// Bind one cross-artifact reference under never-fabricate (CR-011,
    /// [ADR-26], [FR-CG-07]).
    ///
    /// The third and fourth matcher clients after code and docs, dispatched by
    /// `(kind, form)` to a substrate resolution primitive:
    ///
    /// - **`ArtifactRef` + `Path`** — a workspace-relative artifact path (a proto
    ///   import, a shell `source`, a Terraform local module source) binds to the
    ///   one [`NodeKind::ConfigFile`] at that path
    ///   ([`resolve_artifact_path`](Ctx::resolve_artifact_path)).
    /// - **`ArtifactRef` + `Method`** — a literal artifact name (a proto type
    ///   reference, a GraphQL type reference) binds to the one artifact node of
    ///   that name ([`resolve_artifact_name`](Ctx::resolve_artifact_name)).
    /// - **`ArtifactBinding` + `Method`** — a schema-declared type name binds to
    ///   the one type-like **code** symbol of that name, with no synthesized
    ///   candidates ([`resolve_code_type_name`](Ctx::resolve_code_type_name)).
    ///
    /// Every primitive ends in [`exactly_one`], so an ambiguous or unindexed
    /// reference stays unbound and retries on sync ([NFR-RA-05]). The consumer
    /// stories extend this dispatch with their richer matchers (e.g. the OpenAPI
    /// positional-template `Route` match) by adding an arm here. On a bind, the
    /// relation class (`r.payload`) is stamped onto the edge so navigation can
    /// surface which relation it expresses ([FR-CG-11]).
    ///
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_artifact(&self, r: &UnresolvedRefRow) -> Outcome {
        // The relation class travels on the ledger row's payload; recover it so a
        // name match can be fenced to the relation's own artifact kind.
        let relation = r.payload.as_deref().and_then(ArtifactRelation::from_wire);
        // A Terraform local module call is the one multi-target relation: its
        // `source` names a directory, so it fans out to every admitted `.tf`
        // `ConfigFile` in that directory (CR-011, FR-CG-08). It returns directly
        // rather than through the single-target `exactly_one` gate below.
        if r.kind == EdgeKind::ArtifactRef
            && r.form == RefForm::Path
            && relation == Some(ArtifactRelation::TfModuleCall)
        {
            return self.resolve_module_dir(&r.target, r.payload.clone());
        }
        let found = match (r.kind, r.form) {
            (EdgeKind::ArtifactRef, RefForm::Path) => self.resolve_artifact_path(&r.target),
            (EdgeKind::ArtifactRef, RefForm::Method) => self
                .resolve_artifact_name(&r.target, relation.and_then(ArtifactRelation::target_kind)),
            (EdgeKind::ArtifactBinding, RefForm::Method) => self.resolve_code_type_name(&r.target),
            // The OpenAPI `ApiOperation`→`route` match (S-069): the target is the
            // operation rendered `"METHOD /template"`, bound to the one route
            // whose method and positionally-normalized template match exactly.
            (EdgeKind::ArtifactBinding, RefForm::Path) => self.resolve_route(&r.target),
            // Other (kind, form) shapes are consumer-story extension points:
            // unbound here, never fabricated.
            _ => Res::NotFound,
        };
        match found {
            Res::Found(target) => Outcome::Bound {
                source: self.source,
                target,
                kind: r.kind,
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        }
    }

    /// Resolve a workspace-relative artifact path to the one
    /// [`NodeKind::ConfigFile`] at that path (CR-011, [FR-CG-07]).
    ///
    /// Folds the target relative to the source artifact's directory and then to
    /// the repository root — the same two-interpretation walk a documentation
    /// path takes ([`resolve_doc_link`](Ctx::resolve_doc_link)), minus the
    /// `#anchor` (artifact imports carry none). Accepts iff exactly one
    /// `ConfigFile` lives at the resolved path; a path that does not (yet) resolve
    /// stays unbound and retries on the sync that indexes its target.
    ///
    /// [FR-CG-07]: ../../../docs/specs/requirements/FR-CG-07.md
    fn resolve_artifact_path(&self, target: &str) -> Res {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Res::NotFound;
        };
        let source_rel = fold_path(&dir_segments(source_file), target);
        let root_rel = fold_path(&[], target);
        for path in [source_rel, root_rel].into_iter().flatten() {
            match self.config_file_at(&path) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// The one [`NodeKind::ConfigFile`] defined at `path`, or nothing.
    fn config_file_at(&self, path: &str) -> Res {
        let Some(ids) = self.ix.by_file_path.get(path) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.kind == NodeKind::ConfigFile)
            })
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve a Terraform local module call to **every** admitted `.tf`
    /// [`NodeKind::ConfigFile`] in its source directory (CR-011, [FR-CG-08]).
    ///
    /// A module `source = "./modules/net"` names a *directory*; the relation binds
    /// the calling `module` block to each `.tf` file living **directly** in that
    /// directory ([`config_tf_files_in_dir`](Ctx::config_tf_files_in_dir)). The
    /// directory is folded relative to the calling artifact's own directory then to
    /// the repository root — the two-interpretation walk
    /// [`resolve_artifact_path`](Ctx::resolve_artifact_path) uses, the first that
    /// resolves to at least one file winning. A directory with no indexed `.tf`
    /// stays unbound and retries on the sync that indexes its members; registry and
    /// remote sources were classified external before the ledger, so a row here is
    /// always a genuine local-path candidate ([NFR-RA-05]).
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_module_dir(&self, target: &str, payload: Option<String>) -> Outcome {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Outcome::Unbound;
        };
        let source_rel = fold_path(&dir_segments(source_file), target);
        let root_rel = fold_path(&[], target);
        for dir in [source_rel, root_rel].into_iter().flatten() {
            let targets = self.config_tf_files_in_dir(&dir);
            if !targets.is_empty() {
                return Outcome::BoundMany {
                    source: self.source,
                    targets,
                    kind: EdgeKind::ArtifactRef,
                    payload,
                };
            }
        }
        Outcome::Unbound
    }

    /// Every `.tf` [`NodeKind::ConfigFile`] whose file lives **directly** in `dir`
    /// (no recursion into nested module directories), `NodeId`-sorted for a
    /// deterministic edge set ([NFR-RA-06]). The fan-out targets of a local module
    /// call ([`resolve_module_dir`](Ctx::resolve_module_dir)).
    fn config_tf_files_in_dir(&self, dir: &str) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for (path, ids) in &self.ix.by_file_path {
            if !path.ends_with(".tf") {
                continue;
            }
            let parent = match path.rfind('/') {
                Some(i) => &path[..i],
                None => "",
            };
            if parent != dir {
                continue;
            }
            for &id in ids {
                if self
                    .ix
                    .info
                    .get(&id)
                    .is_some_and(|i| i.kind == NodeKind::ConfigFile)
                {
                    out.push(id);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Resolve a literal artifact name to the one artifact node carrying it
    /// (CR-011, [FR-CG-08]).
    ///
    /// The artifact→artifact name match — a proto type reference to its
    /// `ProtoMessage`, a GraphQL type reference to its `GqlType`, a Terraform
    /// `var`/`local`/`module` reference to its `TfBlock`, a SQL clause to its
    /// `SqlObject`. When the relation declares a specific target
    /// ([`ArtifactRelation::target_kind`]) the candidate set is fenced to **that**
    /// artifact kind, so a name shared across formats (a proto type and a same-named
    /// `TfBlock`) can never cross-bind ([NFR-RA-05], [ADR-26]); `want_kind` is
    /// `None` only for a relation with no declared artifact target, where any
    /// artifact-layer ([`NodeKind::is_config`]) node is a candidate. Code symbols
    /// are always excluded, and the single [`exactly_one`] gate keeps
    /// never-fabricate — a duplicated or absent name stays unresolved.
    ///
    /// [FR-CG-08]: ../../../docs/specs/requirements/FR-CG-08.md
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_artifact_name(&self, name: &str, want_kind: Option<NodeKind>) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let admits = |kind: NodeKind| match want_kind {
            // A declared relation target fences candidates to exactly that kind.
            Some(want) => kind == want,
            // No declared target: any artifact-layer node, code excluded.
            None => kind.is_config(),
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| admits(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve a schema-declared type name to the one **type-like code** symbol
    /// carrying it (CR-011, [FR-CG-10]).
    ///
    /// The artifact→code binding for a literal declared name (a proto/GraphQL type
    /// → the struct/class/… that implements it). Only type-like code kinds are
    /// candidates ([`is_type_like`]) — **no synthesized candidates**, no codegen
    /// case-mapping, no resolver conventions ([ADR-26]) — and the single
    /// [`exactly_one`] gate means a common name (`User` declared in two places) or
    /// an absent one stays unresolved ([NFR-RA-05]).
    ///
    /// [FR-CG-10]: ../../../docs/specs/requirements/FR-CG-10.md
    /// [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_code_type_name(&self, name: &str) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| is_type_like(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve an OpenAPI `ApiOperation` to the one framework-extracted `route`
    /// node it specifies (S-069, CR-011, [FR-CG-09]).
    ///
    /// `target` is the operation rendered `"METHOD /template"` (the same shape a
    /// [`NodeKind::Route`] node's `name` carries). Both sides are reduced to the
    /// shared `(METHOD, positionally-normalized template)`
    /// [`route_key`](super::route_template::route_key): parameter names and
    /// syntax are erased, but the HTTP method and the static skeleton must match
    /// exactly. The candidate set is the routes filed under that key in
    /// [`Index`], reduced by the shared [`exactly_one`] gate — so two routes
    /// sharing a normalized template + method leave the operation unresolved, a
    /// method mismatch never binds, and an operation whose key does not normalize
    /// (or matches no route) stays in the ledger for the next sync ([NFR-RA-05]).
    /// A catch-all/regex route is absent from the index entirely, so it is never
    /// approximately matched.
    ///
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_route(&self, target: &str) -> Res {
        let Some(key) = route_key(target) else {
            // The operation's own template does not normalize (or the target is
            // malformed): never approximately matched.
            return Res::NotFound;
        };
        match self.ix.routes_by_key.get(&key) {
            Some(candidates) => exactly_one(candidates),
            None => Res::NotFound,
        }
    }

    /// CR-068 Part B bare-path method exclusion, expressed as a **tie-break**
    /// ([FR-RS-07]): while resolving a single-segment bare-path call (gated on
    /// [`bare_path_call`](Ctx::bare_path_call)), a free [`NodeKind::Function`] at a
    /// scope outranks same-named [`NodeKind::Method`]s there. So a same-module bare
    /// call binds
    /// the one free function even when same-named associated methods collapse to
    /// that module scope (`impl` is not a captured scope, [`is_type_like`]) — the
    /// `graph_store` `insert_node`/`insert_edge`/`upsert_symbol` cluster the graph
    /// previously left [`Res::Ambiguous`].
    ///
    /// A **tie-break, not a filter**: methods are dropped only when a free
    /// function is actually present. So it is strictly monotonic and
    /// never-fabricate ([NFR-RA-05]):
    /// - one free fn + same-named methods → binds the free fn (the recovery);
    /// - two-or-more free fns → still ambiguous, stays unresolved;
    /// - no free fn (a language whose free callables are `Method`, e.g. a Ruby
    ///   top-level `def`, or a lone associated method) → the full callable set
    ///   stands, so no previously-resolved edge is lost.
    ///
    /// Path-qualified (`Type::f` via [`descend`](Ctx::descend)) and typed calls
    /// never reach this step; a receiver-unqualified method call
    /// ([`RefForm::Method`]) does reach it but leaves [`bare_path_call`](Ctx::bare_path_call)
    /// `false`, so it is a no-op there — receiver calls and the [CR-066]
    /// workspace unique-name fallback are untouched.
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
    fn prefer_free_functions(&self, mut candidates: Vec<NodeId>) -> Vec<NodeId> {
        if !self.bare_path_call.get() {
            return candidates;
        }
        let is_free = |id: &NodeId| {
            self.ix
                .info
                .get(id)
                .is_some_and(|i| i.kind == NodeKind::Function)
        };
        if candidates.iter().any(is_free) {
            candidates.retain(is_free);
        }
        candidates
    }

    /// Resolve a bare name by the scope hierarchy (function-local outward).
    ///
    /// Reached only for a **single-segment** name (multi-segment paths route
    /// through [`descend`](Ctx::descend)). Both a bare-path call and a
    /// receiver-unqualified method call arrive here with [`Want::Callable`]; the
    /// CR-068 Part B free-function tie-break ([`prefer_free_functions`]) fires
    /// only for the former, gated on [`bare_path_call`](Ctx::bare_path_call).
    fn resolve_name(&self, name: &str, want: Want, depth: u8) -> Res {
        // 1) Lexical Contains chain, innermost first: nested decls of the
        //    source itself, then each enclosing scope up to the file module.
        let mut cursor = Some(self.source);
        while let Some(scope) = cursor {
            let members = self.prefer_free_functions(self.ix.members_named(scope, name, want));
            match exactly_one(&members) {
                Res::NotFound => {}
                decided => return decided, // found — or a *known* ambiguity
            }
            cursor = self.ix.parent.get(&scope).copied();
        }
        // 2) A child module of the source module, a crate-root module
        //    (sibling files are linked via the path-derived module tree, not
        //    via Contains), or an extern crate's root (`use other::*` /
        //    `use other;` name the crate itself).
        if want != Want::Callable {
            if let Some((krate, mods)) = self.source_module() {
                let mut child = mods.clone();
                child.push(name.to_string());
                if let Some(&id) = self.ix.modules.get(&(krate.clone(), child)) {
                    return Res::Found(id);
                }
                if let Some(&id) = self.ix.modules.get(&(krate, vec![name.to_string()])) {
                    return Res::Found(id);
                }
            }
            let norm = normalize_crate(name);
            if let Some(&id) = self.ix.modules.get(&(norm, Vec::new())) {
                return Res::Found(id);
            }
        }
        // 3) The file's `use` aliases.
        if let Some(alias_path) = self.scope().and_then(|s| s.aliases.get(name)) {
            match self.resolve_path(alias_path, want, depth - 1) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        // 4) The file's glob imports.
        let single = [name.to_string()];
        match self.through_globs(&single, want, depth) {
            Res::NotFound => {}
            decided => return decided,
        }
        // 5) Workspace unique-name fallback — aggressive only for bare names,
        //    and never for a receiver-method call (its receiver type is unknown,
        //    so a bare workspace name-match is a fabrication, CR-066).
        if self.policy == BindingPolicy::Aggressive && !self.no_workspace_fallback.get() {
            return self.unique_by_name(name, want);
        }
        Res::NotFound
    }

    /// Resolve `segs` as members reached through each of the file's glob
    /// imports; accept iff exactly one distinct target across all globs.
    ///
    /// Each glob's *own* module path is resolved via [`resolve_path`](Ctx::resolve_path),
    /// whose import step lands back here — so on a file carrying `G` glob
    /// imports a single lookup fans out `G`-wide at every recursion level, i.e.
    /// `O(G^MAX_ALIAS_DEPTH)` work per reference. A real trigger (CR-016): a file
    /// with 14 `use super::*` / `use super::sub::*` imports drove a single bind
    /// to ~1.5e9 operations (~150 s on one core); across the parallel bind pass
    /// many such refs detonate at once and peg every core — the dominant cost of
    /// a cold full index (measured on the self-graph: a cold index climbed to a
    /// load of 27 within 27 s before this guard, 3.8 s at flat load after).
    ///
    /// The `in_glob_resolution` guard breaks the recursion: while a glob's
    /// module is being resolved, a re-entry here returns `NotFound` at once.
    /// This is **outcome-preserving** — a glob module reachable *only* through
    /// the same in-scope glob set never converged under the depth budget anyway
    /// (it bottomed out at `NotFound`), so we return that result without the
    /// exponential. The sync≡reindex equivalence net and the real self-graph
    /// (edge set unchanged) gate the equivalence.
    fn through_globs(&self, segs: &[String], want: Want, depth: u8) -> Res {
        let Some(scope) = self.scope() else {
            return Res::NotFound;
        };
        // Re-entrant glob-module resolution contributes nothing (see above).
        if self.in_glob_resolution.get() {
            return Res::NotFound;
        }
        let mut found: Vec<NodeId> = Vec::new();
        for glob in &scope.globs {
            // Resolve the glob's module itself (depth-limited like an alias),
            // with glob fan-out suppressed for the duration so the lookup cannot
            // recurse back through the file's glob set.
            self.in_glob_resolution.set(true);
            let resolved = self.resolve_path(glob, Want::Module, depth.saturating_sub(1));
            self.in_glob_resolution.set(false);
            let module = match resolved {
                Res::Found(m) => m,
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => continue,
            };
            let Some(key) = self.ix.module_key.get(&module) else {
                continue;
            };
            match self.descend(&key.0, &key.1, segs, want) {
                Res::Found(id) => found.push(id),
                Res::Ambiguous => return Res::Ambiguous,
                Res::NotFound => {}
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }

    /// Walk `segs` down the module tree from `(krate, base)`.
    fn descend(&self, krate: &str, base: &[String], segs: &[String], want: Want) -> Res {
        let mut key: ModKey = (krate.to_string(), base.to_vec());
        for (i, seg) in segs.iter().enumerate() {
            let is_last = i == segs.len() - 1;

            if !is_last {
                // Try the segment as a child module in place (push, check,
                // pop on miss) — no per-step key clone.
                key.1.push(seg.clone());
                if self.ix.modules.contains_key(&key) {
                    continue;
                }
                key.1.pop();
                // The `Type::func` collapse: associated items live at module
                // scope (S-007 does not capture `impl`), so when the
                // next-to-last segment names a type in the current module,
                // the final segment is looked up among the module's
                // functions.
                if i == segs.len() - 2 {
                    if let Some(&scope_node) = self.ix.modules.get(&key) {
                        let type_here = self
                            .ix
                            .members_named(scope_node, seg, Want::Any)
                            .into_iter()
                            .any(|id| self.ix.info.get(&id).is_some_and(|n| is_type_like(n.kind)));
                        if type_here {
                            return exactly_one(&self.ix.members_named(
                                scope_node,
                                &segs[i + 1],
                                Want::Callable,
                            ));
                        }
                    }
                }
                return Res::NotFound;
            }

            // Final segment: a child module (preferred for imports) or a
            // member item of the current module.
            if want != Want::Callable {
                let mut child = key.1.clone();
                child.push(seg.clone());
                if let Some(&m) = self.ix.modules.get(&(key.0.clone(), child)) {
                    return Res::Found(m);
                }
            }
            if want == Want::Module {
                return Res::NotFound;
            }
            let Some(&scope_node) = self.ix.modules.get(&key) else {
                return Res::NotFound;
            };
            return exactly_one(&self.ix.members_named(scope_node, seg, want));
        }
        Res::NotFound
    }

    /// Workspace fallback for a multi-segment path (balanced+): the final
    /// segment's name matches and the node's module path ends with the
    /// leading segments — crate-first, then workspace, exactly-one at each
    /// step ([FR-RS-03] crate → workspace levels).
    fn suffix_match(&self, segs: &[String], want: Want) -> Res {
        let (prefix, last) = segs.split_at(segs.len() - 1);
        let Some(all) = self.ix.by_name.get(&last[0]) else {
            return Res::NotFound;
        };
        let matches_suffix = |id: &NodeId| -> bool {
            let Some(info) = self.ix.info.get(id) else {
                return false;
            };
            if !want.admits(info.kind) {
                return false;
            }
            // The node's own module path must end with the path's prefix.
            let module_of = |id: NodeId| self.ix.nearest_module(id).cloned();
            match module_of(*id) {
                Some((_, mods)) => mods.ends_with(prefix),
                None => false,
            }
        };
        let candidates: Vec<NodeId> = all.iter().copied().filter(matches_suffix).collect();
        self.prefer_crate(&candidates)
    }

    /// Workspace unique-name fallback (method calls at balanced+, bare names
    /// at aggressive): crate-first, then workspace, exactly-one at each step.
    fn unique_by_name(&self, name: &str, want: Want) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| want.admits(i.kind)))
            .collect();
        self.prefer_crate(&candidates)
    }

    /// The crate → workspace acceptance step shared by every workspace
    /// fallback ([FR-RS-03] hierarchy order): exactly-one among the source
    /// crate's candidates wins; a crate-level ambiguity is final; only an
    /// empty crate set escalates to the workspace-wide exactly-one test.
    ///
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    fn prefer_crate(&self, candidates: &[NodeId]) -> Res {
        let source_crate = self
            .ix
            .info
            .get(&self.source)
            .map(|i| i.crate_name.as_str())
            .unwrap_or_default();
        let in_crate: Vec<NodeId> = candidates
            .iter()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.crate_name == source_crate)
            })
            .collect();
        match exactly_one(&in_crate) {
            Res::NotFound => exactly_one(candidates),
            decided => decided,
        }
    }

    /// Resolve a documentation link/path reference to its target node (S-035,
    /// [FR-DG-03]/[FR-DG-04]).
    ///
    /// The href is normalised relative to the source doc's path. With a
    /// `#anchor`, the target is the one [`NodeKind::DocSection`] in that file
    /// whose [`heading_slug`] matches; without one, the [`NodeKind::DocFile`] at
    /// that path, else the file-root module of a code file. Every branch ends in
    /// [`exactly_one`] — a missing or ambiguous target stays unresolved
    /// ([NFR-RA-05]).
    ///
    /// [FR-DG-03]: ../../../docs/specs/requirements/FR-DG-03.md
    /// [FR-DG-04]: ../../../docs/specs/requirements/FR-DG-04.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_doc_link(&self, target: &str) -> Res {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Res::NotFound;
        };
        let (path_part, anchor) = split_anchor(target);

        // A bare `#anchor` targets the source file itself.
        if path_part.is_empty() {
            return self.resolve_doc_at_path(source_file, anchor.as_deref());
        }

        // Try two interpretations of the path, in order, falling through only on
        // a clean miss (never on a known ambiguity — that would fabricate):
        //   1. relative to the source doc's directory — markdown-link semantics;
        //   2. relative to the repository root — explicit repo-file-path
        //      semantics (FR-DG-04), e.g. an inline-code `crate/src/x.rs`.
        // A leading `/` already forces root in both, so they coincide there.
        let source_rel = fold_path(&dir_segments(source_file), path_part);
        let root_rel = fold_path(&[], path_part);
        for path in [source_rel, root_rel].into_iter().flatten() {
            match self.resolve_doc_at_path(&path, anchor.as_deref()) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// Resolve an already-normalised doc target `path` (+ optional `anchor`) to a
    /// node: with an anchor, the one [`NodeKind::DocSection`] in that file whose
    /// [`heading_slug`] matches; without one, the [`NodeKind::DocFile`] there,
    /// else the file-root module of a code file. Always exactly-one-or-nothing.
    fn resolve_doc_at_path(&self, path: &str, anchor: Option<&str>) -> Res {
        let in_file = self.ix.by_file_path.get(path);

        if let Some(anchor) = anchor {
            // Re-slugify the anchor so a link and its heading agree on casing and
            // punctuation; well-formed anchors are already slugs (idempotent).
            let want = heading_slug(anchor);
            let Some(ids) = in_file else {
                return Res::NotFound;
            };
            let candidates: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.ix.info.get(id).is_some_and(|i| {
                        is_doc_section_kind(i.kind) && heading_slug(&i.name) == want
                    })
                })
                .collect();
            return exactly_one(&candidates);
        }

        // No anchor: a DocFile at that path (a doc target) wins; otherwise the
        // file-root module of a code file the link points at.
        if let Some(ids) = in_file {
            let doc_files: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.ix
                        .info
                        .get(id)
                        .is_some_and(|i| is_doc_file_kind(i.kind))
                })
                .collect();
            if !doc_files.is_empty() {
                return exactly_one(&doc_files);
            }
        }
        match self.ix.modules.get(&module_key_for_file(path)) {
            Some(&m) => Res::Found(m),
            None => Res::NotFound,
        }
    }

    /// Resolve a documentation code-name token to the one *code* symbol of that
    /// name in the workspace (S-035, [FR-DG-04]).
    ///
    /// Documentation nodes are excluded so a token binds to code, never to
    /// another doc; the single [`exactly_one`] gate keeps the never-fabricate
    /// invariant — a name shared by two symbols, or by none, stays unresolved
    /// ([NFR-RA-05]).
    ///
    /// [FR-DG-04]: ../../../docs/specs/requirements/FR-DG-04.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_doc_code_name(&self, name: &str) -> Res {
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| !is_doc_kind(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Turn a resolved documentation reference into an edge, elevating it to a
    /// typed trace when it connects two swe-skills artifacts (S-039, [FR-DG-07]).
    ///
    /// A hyperlink expresses a *trace* when the node it lives in and the node it
    /// resolves to are each owned by a typed artifact: [`typed_owner`] walks each
    /// endpoint up the `Contains` hierarchy to its nearest enclosing
    /// `Requirement`/`Adr`/`Story` (or itself). So a link in a `Requirement`
    /// file's `## Dependencies` section traces from the *requirement*, and a
    /// `Story` section's link to a requirement traces the "implements" relation —
    /// both as [`EdgeKind::TracesTo`] between the typed owners. A link touching a
    /// generic `DocFile`/`DocSection` or a code symbol (neither has a typed owner)
    /// stays a plain [`EdgeKind::DocReference`] from the resolved endpoints, and a
    /// link within a single artifact (`a == b`, an intra-file anchor) is not a
    /// self-trace.
    ///
    /// Re-typing never fabricates: the reference was already bound through the
    /// exactly-one-candidate ledger ([NFR-RA-05]); this only labels and elevates
    /// the edge the binder proved.
    ///
    /// [`typed_owner`]: Ctx::typed_owner
    /// [FR-DG-07]: ../../../docs/specs/requirements/FR-DG-07.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn bind_doc_ref(&self, source: NodeId, target: NodeId) -> Outcome {
        if let (Some(a), Some(b)) = (self.typed_owner(source), self.typed_owner(target)) {
            if a != b {
                return Outcome::Bound {
                    source: a,
                    target: b,
                    kind: EdgeKind::TracesTo,
                    payload: None,
                };
            }
        }
        Outcome::Bound {
            source,
            target,
            kind: EdgeKind::DocReference,
            payload: None,
        }
    }

    /// The nearest typed swe-skills artifact (`Requirement`/`Adr`/`Story`) that
    /// *owns* `id` — `id` itself if it is typed, else the nearest such ancestor
    /// reached by walking the `Contains` hierarchy upward, or `None` if none
    /// (a generic doc node, or a code symbol) (S-039, [FR-DG-07]).
    ///
    /// `Contains` is a tree, so the walk normally terminates at a typed node or
    /// the root. The hard [`MAX_CONTAINS_DEPTH`] cap is a defence-in-depth guard
    /// against a malformed cycle ever reaching the binder — mirroring the bounded
    /// [`MAX_ALIAS_DEPTH`] alias walk — so a corrupt edge degrades to "no typed
    /// owner" (a plain `DocReference`) instead of spinning forever.
    fn typed_owner(&self, mut id: NodeId) -> Option<NodeId> {
        for _ in 0..MAX_CONTAINS_DEPTH {
            if is_typed_doc_kind(self.ix.info.get(&id)?.kind) {
                return Some(id);
            }
            id = *self.ix.parent.get(&id)?;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_key_derives_crate_and_modules_from_src_layout() {
        assert_eq!(
            module_key_for_file("logos-core/src/extract/mod.rs"),
            ("logos_core".to_string(), vec!["extract".to_string()])
        );
        assert_eq!(
            module_key_for_file("logos-core/src/lib.rs"),
            ("logos_core".to_string(), vec![])
        );
        assert_eq!(
            module_key_for_file("src/engine.rs"),
            ("crate".to_string(), vec!["engine".to_string()])
        );
        assert_eq!(
            module_key_for_file("src/a/b.rs"),
            ("crate".to_string(), vec!["a".to_string(), "b".to_string()])
        );
        // No src/ layout (flat fixtures): everything is a module under
        // the anonymous crate.
        assert_eq!(
            module_key_for_file("alpha.rs"),
            ("crate".to_string(), vec!["alpha".to_string()])
        );
    }

    #[test]
    fn exactly_one_is_the_only_acceptance_rule() {
        assert_eq!(exactly_one(&[]), Res::NotFound);
        assert_eq!(exactly_one(&[NodeId(1)]), Res::Found(NodeId(1)));
        assert_eq!(exactly_one(&[NodeId(1), NodeId(2)]), Res::Ambiguous);
    }
}
