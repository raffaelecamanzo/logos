//! Interface satisfaction by **method set** (S-529, [CR-172], [FR-RS-18]) — the
//! `Implements` a language writes no clause for.
//!
//! In a language whose plugin declares `method_set_implements` (Go), a type
//! implements an in-repository interface when its method set holds every method
//! the interface declares. The resolution pass ([`super::run`]) computes that
//! relation here, over the whole snapshot on every run, and reconciles the
//! committed edges to it — so a full index and an incremental sync converge on
//! one set ([NFR-RA-06]), and the pass is the only author of those edges: no
//! ledger row records one, and a capture-before-delete row of one restores
//! nothing ([`Plan::captures`]).
//!
//! - **A type's method set** is its own methods — every method recorded with
//!   that type as its self type in the type's directory (a Go package), by
//!   pointer and value receiver alike — and those each embedded field promotes
//!   by Go's depth rule: an embedded interface's methods, an embedded type's
//!   own and promoted ones, the shallowest name winning and a name two
//!   embeddings promote at one depth in neither. An embedding the pass cannot
//!   resolve stops promotion below it, which can only miss an `Implements`,
//!   never invent one.
//! - **An interface's method set** is its method specs with every embedded
//!   interface expanded. A spec recorded with an unknown parameter range — every
//!   spec of a generic interface (the plugin's query says so) — makes the
//!   interface **not computed**, and so does an embedding that resolves to no
//!   in-repository interface. An interface declaring no method (`any`, a
//!   constraint's type set, a generic interface built only by embedding, whose
//!   embeddings the query does not record) is implemented by everything and
//!   recorded for none.
//! - **Methods match** by name and parameter count, the count known on both
//!   sides. An unexported name (not capitalised) is its package's own, so it
//!   matches only a method declared in the directory that declares the spec.
//!
//! An embedding is the `Extends` row the struct's embedded field or the
//! interface's embedded interface records. A bare name is the package's own
//! type: the one type of that name in the embedder's directory. A qualified
//! one (`zapcore::Core`) is the one type of that name in the directory the
//! file's import of that package name binds to, as the binder binds the import
//! row; failing that, the type the binder binds the row itself to.
//!
//! [CR-172]: ../../../docs/requests/CR-172-go-resolves-as-packages-with-its-types-routes-and-interfaces.md
//! [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::graph_store::{BatchWriter, EdgeRow, NodeArity, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind, ParamRange, RefForm};
use crate::plugin::registry::declares_by_extension;

use super::binder::Outcome;
use super::dir_of;

/// How far an embedding chain is followed — a bound against a cycle the
/// recursion guard already breaks, and against a malformed graph.
const MAX_EMBEDDING_DEPTH: usize = 32;

/// What one run of the pass found, traced by the resolution pass and read by
/// the inspection figures ([FR-RS-18]: a generic interface is counted as not
/// computed).
///
/// [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    /// The interfaces in scope.
    pub interfaces: usize,
    /// Those whose method set was computed and holds at least one method.
    pub computed: usize,
    /// Those not computed because a spec's parameter count is unknown: a
    /// generic interface.
    pub generic: usize,
    /// Those not computed because an embedding resolves to no in-repository
    /// interface.
    pub unresolved_embedding: usize,
    /// Those declaring no method of their own and embedding none the pass
    /// expands: the empty interface, type-set constraints, and a generic
    /// interface built only by embedding (`interface { *T; Marshaler }`), whose
    /// embeddings the plugin's query does not record.
    pub empty: usize,
    /// The `Implements` relations recorded.
    pub implements: usize,
}

/// The pass's verdict for one run: the edges it wants, the edges it retracts,
/// and the capture rows it spends.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Every `(type, interface)` that implements, sorted.
    pub desired: BTreeSet<(NodeId, NodeId)>,
    /// Every committed `Implements` edge out of a type in scope that is not
    /// desired — what the run deletes.
    pub retract: Vec<(NodeId, NodeId)>,
    /// The types in scope: the sources whose `Implements` the pass alone
    /// decides.
    pub owned: HashSet<NodeId>,
    /// The capture-before-delete rows of an `Implements` out of an owned type:
    /// spent without restoring anything, as a fresh index holds none.
    pub captures: HashSet<i64>,
    /// The figures.
    pub stats: Stats,
}

impl Plan {
    /// Turn every capture-before-delete row of an owned type's `Implements`
    /// [`Outcome::Unbound`]: it restores nothing, the pass decides the edge.
    pub(crate) fn spend_captures(&self, outcomes: &mut [(i64, bool, Outcome)]) {
        for (id, _, outcome) in outcomes {
            if self.captures.contains(id) {
                *outcome = Outcome::Unbound;
            }
        }
    }

    /// Leave the owned types' `Implements` out of the edges the resolution
    /// pass retracts as no longer produced: the pass reconciles them itself.
    pub(crate) fn keep_owned(&self, stale: &mut Vec<(NodeId, NodeId, EdgeKind)>) {
        stale.retain(|(source, _, kind)| !(*kind == EdgeKind::Implements && self.owned.contains(source)));
    }

    /// Reconcile the committed edges to the plan inside the resolution pass's
    /// write batch ([ADR-02]): delete what it retracts, insert what it wants,
    /// idempotently. The count of edges inserted.
    ///
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    pub(crate) fn commit(&self, w: &BatchWriter<'_>) -> Result<u64> {
        for (source, target) in &self.retract {
            w.delete_edge(*source, *target, EdgeKind::Implements)?;
        }
        let mut created = 0;
        for (source, target) in &self.desired {
            if w.insert_edge_with_payload_if_live(*source, *target, EdgeKind::Implements, None)? == Some(true) {
                created += 1;
            }
        }
        Ok(created)
    }
}

/// The languages one resolution run computes the relation for, and their
/// methods' self types — taken from the snapshot before the binder's index
/// takes its own. No language declaring `method_set_implements` (a synthetic
/// graph with no registry, a build without Go) makes every plan empty.
pub(crate) struct Scope {
    extensions: HashSet<String>,
    self_types: Vec<(NodeId, String)>,
}

impl Scope {
    /// The scope of `extensions` over the snapshot's `nodes` and `self_types`.
    pub(crate) fn of(extensions: HashSet<String>, nodes: &[NodeRow], self_types: &[(NodeId, String)]) -> Self {
        let declaring: HashSet<NodeId> = nodes
            .iter()
            .filter(|n| !extensions.is_empty() && declares_by_extension(n.file_path.as_deref(), &extensions))
            .map(|n| n.id)
            .collect();
        let self_types = self_types.iter().filter(|(id, _)| declaring.contains(id)).cloned().collect();
        Scope { extensions, self_types }
    }

    /// This run's [`Plan`] over the snapshot, empty when no language is in
    /// scope; its figures are traced.
    pub(crate) fn plan(
        &self,
        nodes: &[NodeRow],
        edges: &[EdgeRow],
        refs: &[UnresolvedRefRow],
        arities: &[NodeArity],
        bind: &dyn Fn(&UnresolvedRefRow) -> Vec<NodeId>,
    ) -> Plan {
        if self.extensions.is_empty() {
            return Plan::default();
        }
        let input = Input { nodes, edges, refs, self_types: &self.self_types, arities, extensions: &self.extensions };
        let plan = plan(&input, bind);
        tracing::debug!(stats = ?plan.stats, "method-set implements");
        plan
    }
}

/// A method in a method set: its parameter range (`None`: unknown) and the
/// directory declaring it, which an unexported name is matched within.
#[derive(Debug, Clone)]
struct Method<'a> {
    range: Option<ParamRange>,
    dir: &'a str,
}

type MethodSet<'a> = BTreeMap<&'a str, Method<'a>>;

/// Why an interface's method set was not computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotComputed {
    Generic,
    UnresolvedEmbedding,
}

/// One embedding depth of a type's promotion walk ([`Graph::next_depth`]).
#[derive(Default)]
struct Depth<'a> {
    /// The methods promoted at this depth, by name — one per embedding.
    found: BTreeMap<&'a str, Vec<Method<'a>>>,
    /// The embedded non-interface types, whose embeddings are the next depth.
    next: Vec<NodeId>,
    /// Whether an embedding here was unresolved, or an uncomputed interface.
    open: bool,
}

/// What the pass reads of the snapshot.
pub(crate) struct Input<'a> {
    pub nodes: &'a [NodeRow],
    pub edges: &'a [EdgeRow],
    pub refs: &'a [UnresolvedRefRow],
    pub self_types: &'a [(NodeId, String)],
    pub arities: &'a [NodeArity],
    /// The normalised extensions whose language declares
    /// `method_set_implements`
    /// ([`LanguageRegistry::method_set_extensions`](crate::plugin::LanguageRegistry::method_set_extensions)).
    pub extensions: &'a HashSet<String>,
}

/// Whether a method name is exported: capitalised.
fn exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// The resolved graph the computation walks.
struct Graph<'a> {
    kind: HashMap<NodeId, NodeKind>,
    /// Each type's own methods.
    own: HashMap<NodeId, MethodSet<'a>>,
    /// Each interface's specs, `None` once one's range is unknown.
    specs: HashMap<NodeId, Option<MethodSet<'a>>>,
    /// Each type's embeddings: `Some(target)` resolved, `None` not.
    embeds: HashMap<NodeId, Vec<Option<NodeId>>>,
}

impl<'a> Graph<'a> {
    fn build(input: &Input<'a>, bind: &dyn Fn(&UnresolvedRefRow) -> Vec<NodeId>) -> Self {
        let nodes: Vec<&'a NodeRow> = input
            .nodes
            .iter()
            .filter(|n| declares_by_extension(n.file_path.as_deref(), input.extensions))
            .collect();
        let kind: HashMap<NodeId, NodeKind> = nodes.iter().map(|n| (n.id, n.kind)).collect();
        let dir: HashMap<NodeId, &'a str> = nodes
            .iter()
            .map(|n| (n.id, dir_of(n.file_path.as_deref().unwrap_or_default())))
            .collect();
        let is_type = |k: NodeKind| matches!(k, NodeKind::Struct | NodeKind::TypeAlias | NodeKind::Interface);
        let mut types: HashMap<(&'a str, &'a str), Vec<NodeId>> = HashMap::new();
        for n in nodes.iter().filter(|n| is_type(n.kind)) {
            types.entry((dir[&n.id], n.name.as_str())).or_default().push(n.id);
        }
        let one_type = |dir: &str, name: &str| match types.get(&(dir, name)).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        };
        let range: HashMap<NodeId, Option<ParamRange>> =
            input.arities.iter().map(|(id, range, _)| (*id, *range)).collect();
        let by_id: HashMap<NodeId, &'a NodeRow> = nodes.iter().map(|n| (n.id, *n)).collect();

        let mut own: HashMap<NodeId, MethodSet<'a>> = HashMap::new();
        for (id, self_type) in input.self_types {
            let Some(method) = by_id.get(id).filter(|n| n.kind == NodeKind::Method) else {
                continue;
            };
            let Some(owner) = one_type(dir[id], self_type).filter(|t| kind[t] != NodeKind::Interface) else {
                continue;
            };
            // A method declared twice (two build-tag files) with two ranges has
            // none known, whichever node came last (NFR-RA-06).
            let entry = Method { range: range.get(id).copied().flatten(), dir: dir[id] };
            own.entry(owner)
                .or_default()
                .entry(method.name.as_str())
                .and_modify(|known| {
                    if known.range != entry.range {
                        known.range = None;
                    }
                })
                .or_insert(entry);
        }

        let mut specs: HashMap<NodeId, Option<MethodSet<'a>>> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Interface)
            .map(|n| (n.id, Some(MethodSet::new())))
            .collect();
        for e in input.edges.iter().filter(|e| e.kind == EdgeKind::Contains) {
            let (Some(set), Some(spec)) = (specs.get_mut(&e.source), by_id.get(&e.target)) else {
                continue;
            };
            if spec.kind != NodeKind::Method {
                continue;
            }
            match (set.as_mut(), range.get(&spec.id).copied().flatten()) {
                (Some(methods), Some(r)) => {
                    methods.insert(spec.name.as_str(), Method { range: Some(r), dir: dir[&spec.id] });
                }
                _ => *set = None,
            }
        }

        let by_symbol: HashMap<&str, NodeId> = nodes
            .iter()
            .filter(|n| is_type(n.kind))
            .map(|n| (n.symbol.as_str(), n.id))
            .collect();
        // Each file's imports by the package name they bind (`zapcore`).
        let imports: HashMap<(Option<i64>, &str), &UnresolvedRefRow> = input
            .refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Imports && r.form == RefForm::Path)
            .filter_map(|r| Some(((r.file_id, r.alias.as_deref()?), r)))
            .collect();
        let qualified = |r: &UnresolvedRefRow, head: &str, name: &str| {
            let through_import = imports.get(&(r.file_id, head)).and_then(|import| {
                let dirs: BTreeSet<&str> = bind(import).iter().filter_map(|t| dir.get(t)).copied().collect();
                match dirs.iter().collect::<Vec<_>>()[..] {
                    [one] => one_type(one, name),
                    _ => None,
                }
            });
            through_import.or_else(|| match bind(r)[..] {
                [one] if kind.get(&one).is_some_and(|&k| is_type(k)) => Some(one),
                _ => None,
            })
        };
        let mut embeds: HashMap<NodeId, Vec<Option<NodeId>>> = HashMap::new();
        for r in input.refs.iter().filter(|r| r.kind == EdgeKind::Extends && r.form != RefForm::Symbol) {
            let Some(&source) = by_symbol.get(r.source_symbol.as_str()) else {
                continue;
            };
            let target = match r.target.rsplit_once("::") {
                Some((head, name)) => qualified(r, head, name),
                None => one_type(dir[&source], &r.target),
            };
            embeds.entry(source).or_default().push(target);
        }
        Graph { kind, own, specs, embeds }
    }

    /// An interface's method set with its embedded interfaces expanded.
    fn interface_set(&self, id: NodeId, seen: &mut Vec<NodeId>) -> Result<MethodSet<'a>, NotComputed> {
        if seen.contains(&id) || seen.len() > MAX_EMBEDDING_DEPTH {
            return Err(NotComputed::UnresolvedEmbedding);
        }
        let mut set = self.specs.get(&id).cloned().flatten().ok_or(NotComputed::Generic)?;
        seen.push(id);
        for embedded in self.embeds.get(&id).into_iter().flatten() {
            let Some(inner) = embedded.filter(|t| self.kind[t] == NodeKind::Interface) else {
                seen.pop();
                return Err(NotComputed::UnresolvedEmbedding);
            };
            match self.interface_set(inner, seen) {
                Ok(methods) => {
                    for (name, method) in methods {
                        set.entry(name).or_insert(method);
                    }
                }
                Err(why) => {
                    seen.pop();
                    return Err(why);
                }
            }
        }
        seen.pop();
        Ok(set)
    }

    /// A concrete type's method set: its own methods, then those its embedded
    /// fields promote by Go's depth rule, one embedding depth at a time — a
    /// name found at a shallower depth hides every deeper one, and a name two
    /// embeddings promote at one depth is an ambiguous selector, in no method
    /// set. An embedding the pass cannot resolve (or an embedded interface it
    /// does not compute) may hide any name deeper than it, so promotion stops
    /// below its depth: a miss, never an invention ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn type_set(&self, id: NodeId) -> MethodSet<'a> {
        let mut set = self.own.get(&id).cloned().unwrap_or_default();
        let mut hidden: HashSet<&'a str> = set.keys().copied().collect();
        let mut seen: HashSet<NodeId> = HashSet::from([id]);
        let mut level = vec![id];
        for _ in 0..MAX_EMBEDDING_DEPTH {
            let depth = self.next_depth(&level, &mut seen);
            for (name, mut methods) in depth.found {
                if hidden.insert(name) && methods.len() == 1 {
                    set.insert(name, methods.remove(0));
                }
            }
            if depth.open || depth.next.is_empty() {
                break;
            }
            level = depth.next;
        }
        set
    }

    /// The methods the embeddings of the types at one depth (`level`) promote,
    /// by name, the embedded types whose own embeddings form the next depth,
    /// and whether an embedding there was left unresolved or uncomputed. A type
    /// already `seen` is not walked twice.
    fn next_depth(&self, level: &[NodeId], seen: &mut HashSet<NodeId>) -> Depth<'a> {
        let mut depth = Depth::default();
        for embedded in level.iter().flat_map(|ty| self.embeds.get(ty).into_iter().flatten()) {
            // An unresolved embedding leaves the depth open; a type walked
            // already adds nothing.
            let Some(inner) = embedded.filter(|inner| seen.insert(*inner)) else {
                depth.open |= embedded.is_none();
                continue;
            };
            let promoted = if self.kind[&inner] == NodeKind::Interface {
                self.interface_set(inner, &mut Vec::new()).ok()
            } else {
                depth.next.push(inner);
                Some(self.own.get(&inner).cloned().unwrap_or_default())
            };
            match promoted {
                Some(methods) => {
                    for (name, method) in methods {
                        depth.found.entry(name).or_default().push(method);
                    }
                }
                None => depth.open = true,
            }
        }
        depth
    }
}

/// Whether `methods` covers every method of `required`: by name, with the same
/// known parameter range, an unexported one declared in the spec's directory.
fn covers(methods: &MethodSet<'_>, required: &MethodSet<'_>) -> bool {
    required.iter().all(|(name, spec)| {
        methods.get(name).is_some_and(|m| {
            m.range.is_some() && m.range == spec.range && (exported(name) || m.dir == spec.dir)
        })
    })
}

/// Compute the run's [`Plan`]. `bind` binds a ledger row as the binder does —
/// its targets, empty when it binds nothing — read for a qualified embedding's
/// import row and, failing that, its `Extends` row.
pub(crate) fn plan(input: &Input<'_>, bind: &dyn Fn(&UnresolvedRefRow) -> Vec<NodeId>) -> Plan {
    let graph = Graph::build(input, bind);
    let mut stats = Stats::default();

    let mut interfaces: Vec<(NodeId, MethodSet<'_>)> = Vec::new();
    let mut ids: Vec<NodeId> = graph.specs.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        stats.interfaces += 1;
        match graph.interface_set(id, &mut Vec::new()) {
            Ok(set) if set.is_empty() => stats.empty += 1,
            Ok(set) => {
                stats.computed += 1;
                interfaces.push((id, set));
            }
            Err(NotComputed::Generic) => stats.generic += 1,
            Err(NotComputed::UnresolvedEmbedding) => stats.unresolved_embedding += 1,
        }
    }

    let mut owned: Vec<NodeId> = graph
        .kind
        .iter()
        .filter(|(_, &k)| matches!(k, NodeKind::Struct | NodeKind::TypeAlias))
        .map(|(&id, _)| id)
        .collect();
    owned.sort_unstable();
    // Each interface is checked only against the types holding its
    // least-held method name — never every type against every interface.
    let types: Vec<(NodeId, MethodSet<'_>)> = owned
        .iter()
        .map(|&ty| (ty, graph.type_set(ty)))
        .filter(|(_, methods)| !methods.is_empty())
        .collect();
    let mut holders: HashMap<&str, Vec<usize>> = HashMap::new();
    for (at, (_, methods)) in types.iter().enumerate() {
        for name in methods.keys() {
            holders.entry(name).or_default().push(at);
        }
    }
    let mut desired: BTreeSet<(NodeId, NodeId)> = BTreeSet::new();
    for (iface, required) in &interfaces {
        let Some(key) = required.keys().min_by_key(|name| holders.get(*name).map_or(0, Vec::len)) else {
            continue;
        };
        for &at in holders.get(key).into_iter().flatten() {
            let (ty, methods) = &types[at];
            if covers(methods, required) {
                desired.insert((*ty, *iface));
            }
        }
    }
    stats.implements = desired.len();

    let owned: HashSet<NodeId> = owned.into_iter().collect();
    let retract = input
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements && owned.contains(&e.source))
        .map(|e| (e.source, e.target))
        .filter(|edge| !desired.contains(edge))
        .collect();
    let owned_symbols: HashSet<&str> = input
        .nodes
        .iter()
        .filter(|n| owned.contains(&n.id))
        .map(|n| n.symbol.as_str())
        .collect();
    let captures = input
        .refs
        .iter()
        .filter(|r| {
            r.form == RefForm::Symbol
                && r.kind == EdgeKind::Implements
                && owned_symbols.contains(r.source_symbol.as_str())
        })
        .map(|r| r.id)
        .collect();
    Plan { desired, retract, owned, captures, stats }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LogosSymbol;

    fn node(id: i64, name: &str, kind: NodeKind, file: &str) -> NodeRow {
        NodeRow {
            id: NodeId(id),
            symbol: LogosSymbol::parse(&format!("local sym{id}")).unwrap(),
            kind,
            name: name.to_string(),
            file_path: Some(file.to_string()),
            start_line: None,
            end_line: None,
        }
    }

    fn contains(source: i64, target: i64) -> EdgeRow {
        EdgeRow { source: NodeId(source), target: NodeId(target), kind: EdgeKind::Contains, dispatch: false }
    }

    fn extends(id: i64, source: i64, target: &str) -> UnresolvedRefRow {
        UnresolvedRefRow {
            id,
            file_id: Some(1),
            source_symbol: format!("local sym{source}"),
            target: target.to_string(),
            alias: None,
            form: RefForm::Path,
            kind: EdgeKind::Extends,
            line: None,
            resolved: false,
            payload: None,
            receiver: None,
            peeled: None,
            arg_count: None,
            exported: None,
        }
    }

    fn arity(id: i64, n: Option<u32>) -> NodeArity {
        (NodeId(id), n.map(|n| ParamRange { min: n, max: Some(n) }), None)
    }

    /// FR-RS-18: a generic interface (its specs' ranges unknown) is counted not
    /// computed, an embedding resolving to no interface likewise, and an
    /// interface declaring no method is counted empty — none is implemented;
    /// the one computed interface is, by the type covering it. The figures are
    /// what the inspection readout reports.
    #[test]
    fn each_interface_is_counted_computed_generic_unresolved_or_empty() {
        let nodes = vec![
            node(1, "Getter", NodeKind::Interface, "p/a.go"),
            node(2, "Get", NodeKind::Method, "p/a.go"),
            node(3, "Closer", NodeKind::Interface, "p/a.go"),
            node(4, "Close", NodeKind::Method, "p/a.go"),
            node(5, "Wide", NodeKind::Interface, "p/a.go"),
            node(6, "Any", NodeKind::Interface, "p/a.go"),
            node(7, "box", NodeKind::Struct, "p/b.go"),
            node(8, "Get", NodeKind::Method, "p/b.go"),
            node(9, "Close", NodeKind::Method, "p/b.go"),
        ];
        let edges = vec![contains(1, 2), contains(3, 4)];
        let refs = vec![extends(1, 5, "io::Reader")];
        let self_types = vec![(NodeId(8), "box".to_string()), (NodeId(9), "box".to_string())];
        let arities = vec![arity(2, None), arity(4, Some(0)), arity(8, Some(0)), arity(9, Some(0))];
        let extensions = HashSet::from(["go".to_string()]);
        let input = Input {
            nodes: &nodes,
            edges: &edges,
            refs: &refs,
            self_types: &self_types,
            arities: &arities,
            extensions: &extensions,
        };
        let plan = plan(&input, &|_| Vec::new());
        assert_eq!(
            plan.stats,
            Stats { interfaces: 4, computed: 1, generic: 1, unresolved_embedding: 1, empty: 1, implements: 1 }
        );
        assert_eq!(plan.desired, BTreeSet::from([(NodeId(7), NodeId(3))]));
    }

    /// A file outside the declaring languages contributes nothing.
    #[test]
    fn a_language_that_does_not_declare_the_key_is_out_of_scope() {
        let nodes = vec![
            node(3, "Closer", NodeKind::Interface, "p/a.rs"),
            node(4, "Close", NodeKind::Method, "p/a.rs"),
            node(7, "Box", NodeKind::Struct, "p/b.rs"),
            node(9, "Close", NodeKind::Method, "p/b.rs"),
        ];
        let edges = vec![contains(3, 4)];
        let self_types = vec![(NodeId(9), "Box".to_string())];
        let arities = vec![arity(4, Some(0)), arity(9, Some(0))];
        let extensions = HashSet::from(["go".to_string()]);
        let input =
            Input { nodes: &nodes, edges: &edges, refs: &[], self_types: &self_types, arities: &arities, extensions: &extensions };
        let plan = plan(&input, &|_| Vec::new());
        assert_eq!(plan.stats, Stats::default());
        assert!(plan.desired.is_empty() && plan.owned.is_empty());
    }
}
