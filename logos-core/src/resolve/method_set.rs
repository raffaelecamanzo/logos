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
//! - **A package** is a directory and the `package` clause its files declare
//!   ([`PackageLayout::directory_package`], S-528, S-830): `foo` and the
//!   external test package `foo_test` of one directory are two. A file whose
//!   clause is not recorded is in no known package, and the pass reads
//!   nothing of it.
//! - **A type's method set** is its own methods — every method recorded with
//!   that type as its self type in the type's package, by pointer and value
//!   receiver alike — and those each embedded field promotes
//!   by Go's depth rule: an embedded interface's methods, an embedded type's
//!   own and promoted ones, the shallowest name winning and a name two
//!   embeddings promote at one depth in neither. An embedding the pass cannot
//!   resolve stops promotion below it, which can only miss an `Implements`,
//!   never invent one.
//! - **An interface's method set** is its method specs with every embedded
//!   interface expanded. A spec recorded with an unknown parameter range — every
//!   spec of a generic interface (the plugin's query says so) — makes the
//!   interface **not computed**, and so does a generic interface whatever it
//!   declares (its recorded signature binds type parameters, S-830), and an
//!   embedding that resolves to no in-repository interface. An interface
//!   declaring no method (`any`, a constraint's type set) is implemented by
//!   everything and recorded for none.
//! - **Methods match** by name and by signature — parameter and result types,
//!   each spelt one way ([`signature`], S-830) — the signature known on both
//!   sides: a method with none, or with two (build-tag twins), matches nothing.
//!   An unexported name (not capitalised) is its package's own, so it matches
//!   only a method declared in the package that declares the spec.
//!
//! An embedding is the `Extends` row the struct's embedded field or the
//! interface's embedded interface records. A bare name is the package's own
//! type: the one type of that name in the embedder's package. A qualified
//! one (`zapcore::Core`) is the one type of that name in the package of the
//! directory the file's import of that package name binds to (never its
//! external test package), as the binder binds the import row; failing that,
//! the type the binder binds the row itself to.
//!
//! The run's figures are kept per language ([`Stats`]) and stored with the
//! graph by the commit, so `status` reads them out on each method-set
//! language's row and writes nothing ([`FIGURES_KEY`]).
//!
//! [CR-172]: ../../../docs/requests/CR-172-go-resolves-as-packages-with-its-types-routes-and-interfaces.md
//! [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
//! [`PackageLayout::directory_package`]: super::package_key::PackageLayout::directory_package
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use anyhow::Result;

use crate::graph_store::{BatchWriter, EdgeRow, NodeArity, NodeItemFacts, NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind, RefForm};
use crate::models::navigation::{InterfaceFigures, NotComputedInterfaces};
use crate::plugin::registry::declares_by_extension;

use super::binder::Outcome;
use super::dir_of;
use super::package_key::PackageLayout;

// One spelling per type in a recorded signature (S-830). See its module docs.
mod signature;

pub(crate) use crate::graph_store::METHOD_SET_FIGURES_KEY as FIGURES_KEY;

/// How far an embedding chain is followed — a bound against a cycle the
/// recursion guard already breaks, and against a malformed graph.
const MAX_EMBEDDING_DEPTH: usize = 32;

/// What one run of the pass found over one language, traced by the resolution
/// pass and stored with the graph for `status` ([FR-RS-18]: a generic
/// interface is counted as not computed; [`Stats::figures`]).
///
/// [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    /// The interfaces in scope.
    pub interfaces: usize,
    /// Those whose method set was computed and holds at least one method.
    pub computed: usize,
    /// Those not computed because they are generic: their record binds type
    /// parameters, or a spec's parameter count is unknown.
    pub generic: usize,
    /// Those not computed because an embedding resolves to no in-repository
    /// interface.
    pub unresolved_embedding: usize,
    /// Those declaring no method of their own and embedding none the pass
    /// expands: the empty interface and type-set constraints.
    pub empty: usize,
    /// The `Implements` relations recorded to the language's interfaces.
    pub implements: usize,
}

impl Stats {
    /// The figures `status` reads out ([`InterfaceFigures`]).
    pub(crate) fn figures(&self) -> InterfaceFigures {
        let n = |v: usize| v as u64;
        InterfaceFigures {
            in_scope: n(self.interfaces),
            computed: n(self.computed),
            implements: n(self.implements),
            not_computed: NotComputedInterfaces { generic: n(self.generic), unresolved_embedding: n(self.unresolved_embedding) },
            empty: n(self.empty),
        }
    }
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
    /// The figures, by language.
    pub stats: BTreeMap<String, Stats>,
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
    /// idempotently, and store the figures beside them ([`FIGURES_KEY`]) —
    /// cleared when no language is in scope. The count of edges inserted.
    ///
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    pub(crate) fn commit(&self, w: &BatchWriter<'_>) -> Result<u64> {
        if self.stats.is_empty() {
            w.clear_project_metadata(FIGURES_KEY)?;
        } else {
            let figures: BTreeMap<&str, InterfaceFigures> =
                self.stats.iter().map(|(language, stats)| (language.as_str(), stats.figures())).collect();
            w.set_project_metadata(FIGURES_KEY, &serde_json::to_string(&figures)?)?;
        }
        reconcile(w, EdgeKind::Implements, &self.retract, &self.desired)
    }
}

/// Reconcile one pass's `kind` edges inside the resolution pass's write batch
/// ([ADR-02]): delete each of `retract`, then insert each of `desired`
/// idempotently, only between live nodes. The count of edges inserted. The
/// method-set relation's and the C/C++ pair links' (S-820) one commit.
///
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
pub(crate) fn reconcile<'d>(
    w: &BatchWriter<'_>,
    kind: EdgeKind,
    retract: &[(NodeId, NodeId)],
    desired: impl IntoIterator<Item = &'d (NodeId, NodeId)>,
) -> Result<u64> {
    for (source, target) in retract {
        w.delete_edge(*source, *target, kind)?;
    }
    let mut created = 0;
    for (source, target) in desired {
        if w.insert_edge_with_payload_if_live(*source, *target, kind, None)? == Some(true) {
            created += 1;
        }
    }
    Ok(created)
}

/// The languages one resolution run computes the relation for, their files'
/// packages and their methods' self types — taken from the snapshot before
/// the binder's index takes its own. No language declaring
/// `method_set_implements` (a synthetic graph with no registry, a build
/// without Go) makes every plan empty.
pub(crate) struct Scope {
    extensions: HashSet<String>,
    packages: HashMap<String, Package>,
    self_types: Vec<(NodeId, String)>,
}

/// An in-scope file's package — its directory and `package` clause — and its
/// language, as the layout reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Package {
    pub dir: String,
    pub clause: String,
    pub language: String,
}

impl Scope {
    /// The scope of `extensions` (the method-set languages') over the
    /// snapshot's `nodes` and `self_types`, each file's package and language
    /// read off `layout`.
    pub(crate) fn of(
        extensions: HashSet<String>,
        layout: &PackageLayout,
        nodes: &[NodeRow],
        self_types: &[(NodeId, String)],
    ) -> Self {
        let declaring: Vec<&NodeRow> = nodes
            .iter()
            .filter(|n| !extensions.is_empty() && declares_by_extension(n.file_path.as_deref(), &extensions))
            .collect();
        let packages = declaring
            .iter()
            .filter_map(|n| n.file_path.as_deref())
            .filter_map(|path| {
                let (dir, clause) = layout.directory_package(path)?;
                let language = layout.language(path)?.to_string();
                Some((path.to_string(), Package { dir: dir.to_string(), clause: clause.to_string(), language }))
            })
            .collect();
        let ids: HashSet<NodeId> = declaring.iter().map(|n| n.id).collect();
        let self_types = self_types.iter().filter(|(id, _)| ids.contains(id)).cloned().collect();
        Scope { extensions, packages, self_types }
    }

    /// This run's [`Plan`] over the snapshot, empty when no language is in
    /// scope; its figures are traced.
    pub(crate) fn plan(&self, snapshot: Snapshot<'_>, bind: &dyn Fn(&UnresolvedRefRow) -> Vec<NodeId>) -> Plan {
        if self.extensions.is_empty() {
            return Plan::default();
        }
        let signatures: HashMap<NodeId, &str> =
            snapshot.facts.iter().filter_map(|f| Some((f.id, f.callable_signature.as_deref()?))).collect();
        let input = Input {
            nodes: snapshot.nodes,
            edges: snapshot.edges,
            refs: snapshot.refs,
            self_types: &self.self_types,
            arities: snapshot.arities,
            signatures: &signatures,
            packages: &self.packages,
            extensions: &self.extensions,
        };
        let plan = plan(&input, bind);
        tracing::debug!(stats = ?plan.stats, "method-set implements");
        plan
    }
}

/// The snapshot rows a [`Scope`] plans over.
pub(crate) struct Snapshot<'s> {
    pub nodes: &'s [NodeRow],
    pub edges: &'s [EdgeRow],
    pub refs: &'s [UnresolvedRefRow],
    pub arities: &'s [NodeArity],
    /// The node item facts: each callable's recorded signature
    /// (`nodes.callable_signature`).
    pub facts: &'s [NodeItemFacts],
}

/// A package: a directory and its `package` clause.
type Pkg<'a> = (&'a str, &'a str);

/// A method in a method set: its signature, each type spelt one way (`None`:
/// none known — unrecorded, unreducible or recorded twice two ways), and the
/// package declaring it, which an unexported name is matched within.
#[derive(Debug, Clone)]
struct Method<'a> {
    sig: Option<Rc<str>>,
    pkg: Pkg<'a>,
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
    /// Each callable's and generic type's recorded signature.
    pub signatures: &'a HashMap<NodeId, &'a str>,
    /// Each in-scope file's package and language; a file absent is in no
    /// known package.
    pub packages: &'a HashMap<String, Package>,
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
    /// Each in-scope node's language.
    language: HashMap<NodeId, &'a str>,
    /// Each type's own methods.
    own: HashMap<NodeId, MethodSet<'a>>,
    /// Each interface's specs, `None` once it is generic.
    specs: HashMap<NodeId, Option<MethodSet<'a>>>,
    /// Each type's embeddings: `Some(target)` resolved, `None` not.
    embeds: HashMap<NodeId, Vec<Option<NodeId>>>,
}

/// Each `(file, package name)` import already followed, and where it led
/// (`None`: nowhere reducible).
type Leads<'a> = HashMap<(Option<i64>, String), Option<Imported<'a>>>;

/// Where a file's import of a package name leads.
#[derive(Debug, Clone, Copy)]
enum Imported<'a> {
    /// An in-repository directory, its package the one not `_test`.
    Dir(&'a str),
    /// No in-repository node: a package outside it, by its import path.
    Outside(&'a str),
}

impl<'a> Graph<'a> {
    fn build(input: &Input<'a>, bind: &dyn Fn(&UnresolvedRefRow) -> Vec<NodeId>) -> Self {
        let in_scope: Vec<&'a NodeRow> = input
            .nodes
            .iter()
            .filter(|n| declares_by_extension(n.file_path.as_deref(), input.extensions))
            .collect();
        // A node's directory reads where an import leads; its package, which
        // only a recorded clause gives, puts it in scope.
        let dir: HashMap<NodeId, &'a str> =
            in_scope.iter().map(|n| (n.id, dir_of(n.file_path.as_deref().unwrap_or_default()))).collect();
        let package: HashMap<NodeId, &'a Package> =
            in_scope.iter().filter_map(|n| Some((n.id, input.packages.get(n.file_path.as_deref()?)?))).collect();
        let pkg: HashMap<NodeId, Pkg<'a>> =
            package.iter().map(|(id, p)| (*id, (p.dir.as_str(), p.clause.as_str()))).collect();
        let language: HashMap<NodeId, &'a str> = package.iter().map(|(id, p)| (*id, p.language.as_str())).collect();
        let nodes: Vec<&'a NodeRow> = in_scope.iter().copied().filter(|n| pkg.contains_key(&n.id)).collect();
        let kind: HashMap<NodeId, NodeKind> = nodes.iter().map(|n| (n.id, n.kind)).collect();
        let is_type = |k: NodeKind| matches!(k, NodeKind::Struct | NodeKind::TypeAlias | NodeKind::Interface);
        let mut types: HashMap<(Pkg<'a>, &'a str), Vec<NodeId>> = HashMap::new();
        for n in nodes.iter().filter(|n| is_type(n.kind)) {
            types.entry((pkg[&n.id], n.name.as_str())).or_default().push(n.id);
        }
        let own_type = |pkg: Pkg<'_>, name: &str| match types.get(&(pkg, name)).map(Vec::as_slice) {
            Some([one]) => signature::Own::Type(*one),
            Some(_) => signature::Own::Ambiguous,
            None => signature::Own::Absent,
        };
        let one_type = |pkg: Pkg<'_>, name: &str| match own_type(pkg, name) {
            signature::Own::Type(one) => Some(one),
            _ => None,
        };
        // The type a directory's importable package declares: never its
        // external test package's.
        let mut importable: HashMap<(&'a str, &'a str), Vec<NodeId>> = HashMap::new();
        for ((p, name), ids) in &types {
            if !p.1.ends_with("_test") {
                importable.entry((p.0, *name)).or_default().extend(ids);
            }
        }
        let imported_type = |dir: &str, name: &str| match importable.get(&(dir, name)).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        };
        let by_id: HashMap<NodeId, &'a NodeRow> = nodes.iter().map(|n| (n.id, *n)).collect();
        let ranged: HashMap<NodeId, bool> =
            input.arities.iter().map(|(id, range, _)| (*id, range.is_some())).collect();

        // Each file's imports by the package name they bind (`zapcore`), and
        // each file's id by its path, read off the rows its nodes record.
        let imports: HashMap<(Option<i64>, &str), &UnresolvedRefRow> = input
            .refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Imports && r.form == RefForm::Path)
            .filter_map(|r| Some(((r.file_id, r.alias.as_deref()?), r)))
            .collect();
        let path_of: HashMap<&str, &'a str> =
            in_scope.iter().filter_map(|n| Some((n.symbol.as_str(), n.file_path.as_deref()?))).collect();
        let file_of: HashMap<&'a str, i64> = input
            .refs
            .iter()
            .filter_map(|r| Some((*path_of.get(r.source_symbol.as_str())?, r.file_id?)))
            .collect();
        let leads: RefCell<Leads<'_>> = RefCell::new(HashMap::new());
        let import = |file: Option<i64>, head: &str| -> Option<Imported<'_>> {
            if let Some(known) = leads.borrow().get(&(file, head.to_string())) {
                return *known;
            }
            let row = imports.get(&(file, head))?;
            let targets = bind(row);
            let dirs: BTreeSet<&str> = targets.iter().filter_map(|t| dir.get(t)).copied().collect();
            let dirs: Vec<&str> = dirs.into_iter().collect();
            let lead = match (targets.is_empty(), dirs.as_slice()) {
                (true, _) => Some(Imported::Outside(row.target.as_str())),
                (false, [one]) => Some(Imported::Dir(one)),
                _ => None,
            };
            leads.borrow_mut().insert((file, head.to_string()), lead);
            lead
        };
        // A recorded signature, spelt one way in its own file and package.
        let reduce = |id: NodeId| -> Option<Rc<str>> {
            let recorded = input.signatures.get(&id)?;
            let at = pkg[&id];
            let file = by_id[&id].file_path.as_deref().and_then(|p| file_of.get(p)).copied();
            let own = |name: &str| own_type(at, name);
            let qualified = |head: &str, name: &str| match import(file, head)? {
                Imported::Dir(d) => imported_type(d, name).map(|t| format!("#{}", t.0)),
                Imported::Outside(path) => Some(format!("\"{path}\".{name}")),
            };
            signature::reduce(recorded, &signature::Names { own: &own, qualified: &qualified }).map(Rc::from)
        };

        let mut own: HashMap<NodeId, MethodSet<'a>> = HashMap::new();
        for (id, self_type) in input.self_types {
            let Some(method) = by_id.get(id).filter(|n| n.kind == NodeKind::Method) else {
                continue;
            };
            let Some(owner) = one_type(pkg[id], self_type).filter(|t| kind[t] != NodeKind::Interface) else {
                continue;
            };
            // A method declared twice (two build-tag files) with two signatures
            // has none known, whichever node came last (NFR-RA-06).
            let entry = Method { sig: reduce(*id), pkg: pkg[id] };
            own.entry(owner)
                .or_default()
                .entry(method.name.as_str())
                .and_modify(|known| {
                    if known.sig != entry.sig {
                        known.sig = None;
                    }
                })
                .or_insert(entry);
        }

        // A generic interface — its record binds type parameters — has no
        // specs the pass can compare, declared or not.
        let mut specs: HashMap<NodeId, Option<MethodSet<'a>>> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Interface)
            .map(|n| {
                let generic = input.signatures.get(&n.id).is_some_and(|s| signature::binds_type_parameters(s));
                (n.id, (!generic).then(MethodSet::new))
            })
            .collect();
        for e in input.edges.iter().filter(|e| e.kind == EdgeKind::Contains) {
            let (Some(set), Some(spec)) = (specs.get_mut(&e.source), by_id.get(&e.target)) else {
                continue;
            };
            if spec.kind != NodeKind::Method {
                continue;
            }
            // A spec of unknown range is a generic interface's too (the
            // plugin's query says so).
            match (set.as_mut(), ranged.get(&spec.id).copied().unwrap_or(false)) {
                (Some(methods), true) => {
                    methods.insert(spec.name.as_str(), Method { sig: reduce(spec.id), pkg: pkg[&spec.id] });
                }
                _ => *set = None,
            }
        }

        let by_symbol: HashMap<&str, NodeId> = nodes
            .iter()
            .filter(|n| is_type(n.kind))
            .map(|n| (n.symbol.as_str(), n.id))
            .collect();
        let qualified = |r: &UnresolvedRefRow, head: &str, name: &str| {
            let through_import = match import(r.file_id, head) {
                Some(Imported::Dir(d)) => imported_type(d, name),
                _ => None,
            };
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
                None => one_type(pkg[&source], &r.target),
            };
            embeds.entry(source).or_default().push(target);
        }
        Graph { kind, language, own, specs, embeds }
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
/// known signature, an unexported one declared in the spec's package.
fn covers(methods: &MethodSet<'_>, required: &MethodSet<'_>) -> bool {
    required.iter().all(|(name, spec)| {
        methods
            .get(name)
            .is_some_and(|m| m.sig.is_some() && m.sig == spec.sig && (exported(name) || m.pkg == spec.pkg))
    })
}

/// Compute the run's [`Plan`]. `bind` binds a ledger row as the binder does —
/// its targets, empty when it binds nothing — read for a qualified embedding's
/// import row and, failing that, its `Extends` row.
pub(crate) fn plan(input: &Input<'_>, bind: &dyn Fn(&UnresolvedRefRow) -> Vec<NodeId>) -> Plan {
    let graph = Graph::build(input, bind);
    // Every in-scope language is stated, its figures zero when it declares no
    // interface.
    let mut stats: BTreeMap<String, Stats> =
        graph.language.values().map(|language| (language.to_string(), Stats::default())).collect();

    let mut interfaces: Vec<(NodeId, MethodSet<'_>)> = Vec::new();
    let mut ids: Vec<NodeId> = graph.specs.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(stats) = graph.language.get(&id).and_then(|language| stats.get_mut(*language)) else {
            continue;
        };
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
    for (_, iface) in &desired {
        if let Some(stats) = graph.language.get(iface).and_then(|language| stats.get_mut(*language)) {
            stats.implements += 1;
        }
    }

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
    use crate::model::{LogosSymbol, ParamRange};

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

    /// Every file of `nodes` in package `p` of its directory, a `go` file.
    fn packages(nodes: &[NodeRow]) -> HashMap<String, Package> {
        nodes
            .iter()
            .filter_map(|n| n.file_path.clone())
            .map(|path| {
                let package = Package { dir: dir_of(&path).to_string(), clause: "p".to_string(), language: "go".to_string() };
                (path, package)
            })
            .collect()
    }

    /// FR-RS-18: a generic interface (its specs' ranges unknown, or its record
    /// binding type parameters with no spec at all) is counted not computed,
    /// an embedding resolving to no interface likewise, and an interface
    /// declaring no method is counted empty — none is implemented; the one
    /// computed interface is, by the type covering it. The figures are what
    /// `status` reads out, by language.
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
            node(10, "Ptr", NodeKind::Interface, "p/a.go"),
        ];
        let edges = vec![contains(1, 2), contains(3, 4)];
        let refs = vec![extends(1, 5, "io::Reader")];
        let self_types = vec![(NodeId(8), "box".to_string()), (NodeId(9), "box".to_string())];
        let arities = vec![arity(2, None), arity(4, Some(0)), arity(8, Some(0)), arity(9, Some(0))];
        let signatures = HashMap::from([
            (NodeId(4), "()(error)"),
            (NodeId(8), "()(int)"),
            (NodeId(9), "()(error)"),
            (NodeId(10), "[T]"),
        ]);
        let packages = packages(&nodes);
        let extensions = HashSet::from(["go".to_string()]);
        let input = Input {
            nodes: &nodes,
            edges: &edges,
            refs: &refs,
            self_types: &self_types,
            arities: &arities,
            signatures: &signatures,
            packages: &packages,
            extensions: &extensions,
        };
        let plan = plan(&input, &|_| Vec::new());
        assert_eq!(
            plan.stats,
            BTreeMap::from([(
                "go".to_string(),
                Stats { interfaces: 5, computed: 1, generic: 2, unresolved_embedding: 1, empty: 1, implements: 1 }
            )])
        );
        assert_eq!(plan.desired, BTreeSet::from([(NodeId(7), NodeId(3))]));
    }

    /// A method whose signature differs from the spec's — or which records
    /// none — covers nothing; a file whose clause is unrecorded is in no
    /// package and out of scope.
    #[test]
    fn a_method_covers_a_spec_only_by_an_equal_signature_in_a_known_package() {
        let nodes = vec![
            node(3, "Closer", NodeKind::Interface, "p/a.go"),
            node(4, "Close", NodeKind::Method, "p/a.go"),
            node(7, "good", NodeKind::Struct, "p/b.go"),
            node(8, "Close", NodeKind::Method, "p/b.go"),
            node(11, "bad", NodeKind::Struct, "p/b.go"),
            node(12, "Close", NodeKind::Method, "p/b.go"),
            node(13, "blank", NodeKind::Struct, "p/b.go"),
            node(14, "Close", NodeKind::Method, "p/b.go"),
            node(15, "lost", NodeKind::Struct, "q/c.go"),
            node(16, "Close", NodeKind::Method, "q/c.go"),
        ];
        let edges = vec![contains(3, 4)];
        let self_types: Vec<(NodeId, String)> = [(8, "good"), (12, "bad"), (14, "blank"), (16, "lost")]
            .iter()
            .map(|(id, ty)| (NodeId(*id), ty.to_string()))
            .collect();
        let arities = vec![arity(4, Some(0))];
        let signatures = HashMap::from([
            (NodeId(4), "()(error)"),
            (NodeId(8), "()(error)"),
            (NodeId(12), "()(bool)"),
            (NodeId(16), "()(error)"),
        ]);
        let mut packages = packages(&nodes);
        packages.remove("q/c.go");
        let extensions = HashSet::from(["go".to_string()]);
        let input = Input {
            nodes: &nodes,
            edges: &edges,
            refs: &[],
            self_types: &self_types,
            arities: &arities,
            signatures: &signatures,
            packages: &packages,
            extensions: &extensions,
        };
        assert_eq!(plan(&input, &|_| Vec::new()).desired, BTreeSet::from([(NodeId(7), NodeId(3))]));
    }

    /// An import row of `file` binding `alias`.
    fn import(id: i64, file: i64, source: i64, alias: &str) -> UnresolvedRefRow {
        UnresolvedRefRow {
            file_id: Some(file),
            kind: EdgeKind::Imports,
            alias: Some(alias.to_string()),
            target: format!("example.com::{alias}"),
            ..extends(id, source, "")
        }
    }

    /// A qualified spelling (`zc.Entry`) is one type only when the file's
    /// import binds one directory: an import the binder binds to nodes of two
    /// directories names no type, and the method matches nothing.
    #[test]
    fn an_import_binding_two_directories_reduces_no_type() {
        let nodes = vec![
            node(1, "Entry", NodeKind::Struct, "zc/e.go"),
            node(2, "Entry", NodeKind::Struct, "zc2/e.go"),
            node(3, "Checker", NodeKind::Interface, "zc/c.go"),
            node(4, "Check", NodeKind::Method, "zc/c.go"),
            node(5, "impl", NodeKind::Struct, "q/q.go"),
            node(6, "Check", NodeKind::Method, "q/q.go"),
        ];
        let edges = vec![contains(3, 4)];
        let refs = vec![import(1, 9, 5, "zc")];
        let self_types = vec![(NodeId(6), "impl".to_string())];
        let arities = vec![arity(4, Some(1)), arity(6, Some(1))];
        let signatures = HashMap::from([(NodeId(4), "(Entry)()"), (NodeId(6), "(zc.Entry)()")]);
        let mut packages = packages(&nodes);
        for path in ["zc/e.go", "zc/c.go"] {
            packages.get_mut(path).unwrap().clause = "zc".to_string();
        }
        let extensions = HashSet::from(["go".to_string()]);
        let input = Input {
            nodes: &nodes,
            edges: &edges,
            refs: &refs,
            self_types: &self_types,
            arities: &arities,
            signatures: &signatures,
            packages: &packages,
            extensions: &extensions,
        };
        let one = plan(&input, &|row| if row.kind == EdgeKind::Imports { vec![NodeId(1)] } else { Vec::new() });
        assert_eq!(one.desired, BTreeSet::from([(NodeId(5), NodeId(3))]), "the control: one directory, one type");
        let two = plan(&input, &|row| if row.kind == EdgeKind::Imports { vec![NodeId(1), NodeId(2)] } else { Vec::new() });
        assert_eq!(two.desired, BTreeSet::new(), "two directories: no type");
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
        let signatures = HashMap::from([(NodeId(4), "()()"), (NodeId(9), "()()")]);
        let packages = packages(&nodes);
        let extensions = HashSet::from(["go".to_string()]);
        let input = Input {
            nodes: &nodes,
            edges: &edges,
            refs: &[],
            self_types: &self_types,
            arities: &arities,
            signatures: &signatures,
            packages: &packages,
            extensions: &extensions,
        };
        let plan = plan(&input, &|_| Vec::new());
        assert_eq!(plan.stats, BTreeMap::new());
        assert!(plan.desired.is_empty() && plan.owned.is_empty());
    }
}
