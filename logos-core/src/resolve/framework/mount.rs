//! A router carries the prefix it is mounted at (S-822, [CR-255], [FR-FW-03],
//! [FR-FW-05], [NFR-RA-05], [NFR-RA-06]).
//!
//! # A mount is a prefix stated somewhere else
//!
//! Express serves a router's routes under the path it is mounted at, and the
//! mount is usually in another file:
//!
//! ```text
//! // items.js                          // app.js
//! const router = express.Router();     const items = require('./items');
//! router.get('/items', list);          app.use('/api', items);
//! module.exports = router;
//! ```
//!
//! `router.get('/items')` is `GET /api/items`. Neither file states that alone, so
//! [`group`](super::group)'s per-file receiver walk cannot compose it; the join is
//! run once per framework run, over every candidate file's facts ([`join`]).
//!
//! The dialect's query captures, as plain syntactic facts:
//!
//! - `@fw.mount.router` + `@fw.mount.scope` — a name bound to a router over a
//!   range, with what made it: `@fw.mount.router.module` (the `express` of
//!   `express.Router()`), `@fw.mount.router.ctor` (a bare `Router()`) or
//!   `@fw.mount.router.from` (the string of `require('express').Router()`). It is
//!   a router only when that maker is the framework's: a string the query marks
//!   `@fw.mount.framework`, or a name imported from one. `@fw.mount.shadow` binds
//!   a name to nothing over a range (a parameter), which hides an outer router;
//! - `@fw.mount.import.*` — a name bound to a module (`require`, `import`), with
//!   the export it reads (`.export`, `.named` for the same name, `.namespace`;
//!   neither is the default export);
//! - `@fw.mount.export.*` — a local the module exports (`.name`, `.same`;
//!   neither is the default export);
//! - `@fw.mount.route.receiver` — the receiver a registration is called on, and
//!   `@fw.mount.route.verb` its method name, which a mount call's `@fw.mount.verb`
//!   (its `use`) is matched against exactly: a chained `a.use(x).use(…)` holds two
//!   calls in one callee;
//! - `@fw.mount.callee` — a mount call `x.use(…)`, with its receiver
//!   (`@fw.mount.receiver`), the arguments that may be the mounted router
//!   (`@fw.mount.arg`, `@fw.mount.arg.module` for an inline `require`), and its
//!   first argument (`@fw.mount.first`), which is a path when it is a string
//!   (`@fw.mount.path`) and none when it is middleware (`@fw.mount.middleware`).
//!
//! # What a mount composes
//!
//! A registration on a router is held back from the file's routes
//! ([`defer`]). The join then reads each mount whose argument is a router — a
//! local one, one an import names through its module's export, or an inline
//! `require` of a module whose default export is one — and gives the router's
//! routes the mount's path, joined under the prefixes of the router it is
//! mounted on in turn ([`join_under`]; under `@fw.mount.absorbs`, which Express's
//! dialect states, a router's `/` is the mount path itself). A router mounted at
//! two paths has one route per path. A
//! mount is not a route: the `ANY` registration its `use` call made is dropped.
//! A `use` whose arguments are no router is middleware, and keeps its route.
//!
//! **Refused and counted** ([`super::RouteRefusal::PathNotComposed`],
//! [NFR-RA-05]), never promoted at the partial path: a registration on a router
//! mounted at a path that is not a written literal (a variable, a template, an
//! expression — or a first argument that may be one), and one on a router that
//! is never mounted, or mounted only from a file outside the candidates.
//!
//! A candidate is a file the framework pass reads at all (it names the framework,
//! or binds an `app`/`router` parameter): a mount in any other indexed file is
//! not read. And a router that reaches its mount by being returned from a
//! function or passed to one is not followed — the stated ceiling — so it counts
//! as mounted nowhere.
//!
//! A registration on any other receiver — an `app` or `router` parameter (the
//! [S-541] module shape), `this.router`, a name the facts do not bind to a router
//! — keeps its written path.
//!
//! # Sync ≡ reindex
//!
//! The join reads only the facts of this run's scans. On a sync with a framework
//! footprint the pass re-scans every candidate file, so an edit to the mounting
//! file moves the mounted module's routes exactly as a fresh index does
//! ([NFR-RA-06]).
//!
//! [CR-255]: ../../../../docs/requests/CR-255-route-groups-and-mounts-compose-their-prefixes.md
//! [S-541]: ../../../../docs/planning/journal.md#s-541-express-router-modules-and-template-literal-paths-yield-routes
//! [FR-FW-03]: ../../../../docs/specs/requirements/FR-FW-03.md
//! [FR-FW-05]: ../../../../docs/specs/requirements/FR-FW-05.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, HashMap, HashSet};

use tree_sitter::Node;

use super::group::is_identifier;
use super::{dedup_routes, drop_non_path_routes, is_resolvable_prefix, join_under, refuse, text, FileMatches, RouteMatch};

/// A byte range.
type Span = (usize, usize);

/// What a name bound to a router was made by. Proven against the file's
/// framework strings by [`Facts::is_framework_router`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Maker {
    /// `x.Router()`: `x` must name the framework module.
    Module(String),
    /// `Router()`: the name must be imported from the framework module.
    Ctor(String),
    /// `require('…').Router()`: the string must be the framework's.
    Required(Span),
    /// Any other binding — a parameter, a declaration of another value, a loop
    /// or `catch` variable: never a router this file made.
    Shadow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Router {
    name: String,
    /// Start byte of the declaring name: the binding is visible after it.
    decl: usize,
    scope: Span,
    maker: Maker,
}

/// The export of a module an import reads.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Imported {
    Default,
    Named(String),
    /// `import * as x`: the module object itself, never a router.
    Namespace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Import {
    /// Start byte of the name it binds: a `const x = require(…)` is also a
    /// binding of `x` there, which is the import, not a name hiding it.
    decl: usize,
    /// The specifier as written, unquoted.
    module: String,
    module_at: Span,
    imported: Imported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Export {
    /// `None` for the default export.
    name: Option<String>,
    local: String,
    /// Where the export reads `local`.
    at: usize,
}

/// One argument of a mount call that may be the mounted router.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Arg {
    Name(String),
    /// An inline `require('…')`, with its specifier.
    Required(String),
}

/// One `x.use(…)` call, keyed by its callee's range in [`Facts::mounts`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Mount {
    /// Start byte of the call's `use`: the route the call made is the
    /// registration whose verb starts there.
    verb: Option<usize>,
    /// The receiver, when it is a plain name.
    receiver: Option<String>,
    /// The first argument, when it is a string: unquoted.
    path: Option<String>,
    /// The first argument is middleware (a call, a function): the mount is at
    /// the root.
    middleware_first: bool,
    first: Option<Span>,
    args: Vec<(Span, Arg)>,
}

impl Mount {
    /// The path this mount gives `arg`: a written, joinable literal; the root
    /// when the router is the first argument or follows middleware; `None` when
    /// the first argument is anything else (a variable may hold a path).
    fn path_for(&self, arg: Span) -> Option<String> {
        match &self.path {
            Some(path) => is_resolvable_prefix(path).then(|| path.trim().to_string()),
            None if self.middleware_first || self.first == Some(arg) => Some("/".to_string()),
            None => None,
        }
    }
}

/// Every mount fact one file's scan collected, and the registrations it held
/// back for [`join`]. Empty for a dialect whose query captures none, which
/// leaves the scan untouched.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Facts {
    /// Ranges of the strings that name the framework module (`'express'`).
    framework: HashSet<Span>,
    /// `true` once a capture carried `@fw.mount.absorbs`: the framework serves a
    /// router's path of exactly `/` at the mount path itself ([`join_under`]).
    /// Held per file, like [`super::group`]'s `absorbs`.
    absorbs: bool,
    routers: Vec<Router>,
    /// Imports by the name they bind; a module-level fact, so not scoped.
    imports: HashMap<String, Import>,
    exports: Vec<Export>,
    /// A registration's receiver name, keyed by its start byte (`RouteMatch::at`).
    receivers: HashMap<usize, String>,
    /// A registration's verb (its method name's start byte), keyed the same way.
    route_verbs: HashMap<usize, usize>,
    mounts: BTreeMap<Span, Mount>,
    /// Registrations on a router of this file, with the router's index in
    /// `routers`: drained by [`join`].
    pending: Vec<(usize, RouteMatch)>,
}

impl Facts {
    /// `true` when the router binding was made by the framework module.
    fn is_framework_router(&self, router: &Router) -> bool {
        let imported_from_framework = |name: &str, named: bool| {
            self.imports.get(name).is_some_and(|import| {
                self.framework.contains(&import.module_at) && (matches!(import.imported, Imported::Named(_)) == named)
            })
        };
        match &router.maker {
            Maker::Required(at) => self.framework.contains(at),
            Maker::Module(name) => imported_from_framework(name, false),
            Maker::Ctor(name) => imported_from_framework(name, true),
            Maker::Shadow => false,
        }
    }

    /// The binding of `name` governing byte `at`, with its index: of the
    /// bindings visible there, the innermost, then the latest.
    fn binding_at(&self, name: &str, at: usize) -> Option<(usize, &Router)> {
        self.routers
            .iter()
            .enumerate()
            .filter(|(_, r)| r.name == name && r.decl < at && r.scope.0 <= at && at < r.scope.1)
            .max_by_key(|(_, r)| (r.scope.0, r.decl))
    }

    /// The router `name` names at byte `at` — `None` when the binding governing
    /// it is no router, or there is none.
    fn router_at(&self, name: &str, at: usize) -> Option<usize> {
        self.binding_at(name, at).filter(|(_, r)| self.is_framework_router(r)).map(|(index, _)| index)
    }

    /// The router this module exports as `name` (`None`: the default export).
    fn exported_router(&self, name: Option<&str>) -> Option<usize> {
        self.exports
            .iter()
            .filter(|e| e.name.as_deref() == name)
            .find_map(|e| self.router_at(&e.local, e.at))
    }
}

/// Hold back every registration whose receiver is a router of this file: its
/// path is the mount's to give ([`join`]). Runs in the per-file scan, before
/// composition and dedup, so a router's `GET /` never collapses into the app's.
pub(super) fn defer(out: &mut FileMatches) {
    let FileMatches { routes, mounts: facts, .. } = out;
    if facts.routers.is_empty() || facts.receivers.is_empty() {
        return;
    }
    let mut held: BTreeMap<usize, Vec<RouteMatch>> = BTreeMap::new();
    for route in std::mem::take(routes) {
        match facts.receivers.get(&route.at).and_then(|name| facts.router_at(name, route.at)) {
            Some(router) => held.entry(router).or_default().push(route),
            None => routes.push(route),
        }
    }
    // Two patterns match one registration: one held route each, per router.
    for (router, mut routes) in held {
        dedup_routes(&mut routes);
        facts.pending.extend(routes.into_iter().map(|route| (router, route)));
    }
}

/// A router of one scanned file: (file index, router index).
type RouterId = (usize, usize);

/// One mount of a router: the path it gives, and the router it is mounted on
/// (`None`: the application).
struct Edge {
    path: Option<String>,
    parent: Option<RouterId>,
}

/// Every file's facts, and how a module specifier names a file.
struct Universe<'f, R> {
    files: &'f [(&'f str, &'f Facts)],
    index_of: HashMap<&'f str, usize>,
    resolve: R,
}

impl<R: Fn(&str, &str) -> Option<String>> Universe<'_, R> {
    /// The router `name` names at byte `at` of file `file`: a local router, or
    /// the router export an import of that name reads.
    fn router(&self, file: usize, name: &str, at: usize) -> Option<RouterId> {
        let (rel, facts) = self.files[file];
        let import = facts.imports.get(name);
        // A local binding of the name hides the import — unless it is the
        // import's own declaration: it is the router or nothing.
        if let Some((index, binding)) = facts.binding_at(name, at) {
            if import.is_none_or(|import| import.decl != binding.decl) {
                return facts.is_framework_router(binding).then_some((file, index));
            }
        }
        let import = import?;
        let export = match &import.imported {
            Imported::Default => None,
            Imported::Named(export) => Some(export.as_str()),
            Imported::Namespace => return None,
        };
        self.exported(rel, &import.module, export)
    }

    /// The router the module `spec`, imported from `importer`, exports as
    /// `export`.
    fn exported(&self, importer: &str, spec: &str, export: Option<&str>) -> Option<RouterId> {
        let target = *self.index_of.get((self.resolve)(importer, spec)?.as_str())?;
        Some((target, self.files[target].1.exported_router(export)?))
    }

    /// Every mount of every router, keyed by the router mounted, and the verb of
    /// every mount call that mounted one, by file index.
    fn edges(&self) -> (HashMap<RouterId, Vec<Edge>>, HashMap<usize, HashSet<usize>>) {
        let mut edges: HashMap<RouterId, Vec<Edge>> = HashMap::new();
        let mut mounting: HashMap<usize, HashSet<usize>> = HashMap::new();
        let calls = self
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, (_, facts))| facts.mounts.iter().map(move |(&callee, mount)| (file, callee, mount)));
        for (file, callee, mount) in calls {
            for (child, path) in self.mounted(file, callee.0, mount) {
                let parent = mount.receiver.as_deref().and_then(|name| self.router(file, name, callee.0));
                edges.entry(child).or_default().push(Edge { path, parent });
                mounting.entry(file).or_default().extend(mount.verb);
            }
        }
        (edges, mounting)
    }

    /// The routers one mount call at byte `at` of `file` mounts, each with the
    /// path the call gives it.
    fn mounted(&self, file: usize, at: usize, mount: &Mount) -> Vec<(RouterId, Option<String>)> {
        let rel = self.files[file].0;
        mount
            .args
            .iter()
            .filter_map(|(span, arg)| {
                let child = match arg {
                    Arg::Name(name) => self.router(file, name, at),
                    Arg::Required(spec) => self.exported(rel, spec, None),
                }?;
                Some((child, mount.path_for(*span)))
            })
            .collect()
    }
}

/// `true` when `parent` is `child` or is mounted, through any chain of mounts,
/// on `child`: a mount of `child` on `parent` closes a cycle.
fn mounted_under(parent: RouterId, child: RouterId, edges: &HashMap<RouterId, Vec<Edge>>) -> bool {
    let mut seen: HashSet<RouterId> = HashSet::new();
    let mut stack = vec![parent];
    while let Some(router) = stack.pop() {
        if router == child {
            return true;
        }
        if seen.insert(router) {
            stack.extend(edges.get(&router).into_iter().flatten().filter_map(|edge| edge.parent));
        }
    }
    false
}

/// Every prefix `router` is served under, `None` for one that cannot be read.
/// Sorted and deduplicated, so the routes it yields come out in one order, and
/// memoised per router in `memo`.
///
/// A mount that closes a cycle (`a.use('/b', b)`, `b.use('/a', a)`, or a router
/// mounted on itself) serves the router under endlessly many paths, none of
/// which the source states as the one a request takes: it is `None`, refused
/// and counted, never unrolled. Skipping those mounts leaves the mounts above
/// every router acyclic, so the walk ends, and each router is computed once.
///
/// `absorbs[file]` is whether the routers of that file absorb a mount path of
/// exactly `/` ([`join_under`]): the framework's rule, which its dialect states
/// as `@fw.mount.absorbs`, read per file as a group's `@fw.group.absorbs` is.
fn prefixes(
    router: RouterId,
    edges: &HashMap<RouterId, Vec<Edge>>,
    absorbs: &[bool],
    memo: &mut HashMap<RouterId, Vec<Option<String>>>,
) -> Vec<Option<String>> {
    if let Some(known) = memo.get(&router) {
        return known.clone();
    }
    let mut out: Vec<Option<String>> = Vec::new();
    for edge in edges.get(&router).into_iter().flatten() {
        let Some(path) = &edge.path else {
            out.push(None);
            continue;
        };
        match edge.parent {
            None => out.push(Some(path.clone())),
            Some(parent) if mounted_under(parent, router, edges) => out.push(None),
            Some(parent) => out.extend(
                prefixes(parent, edges, absorbs, memo)
                    .into_iter()
                    .map(|above| above.map(|above| join_under(&above, path, absorbs[router.0]))),
            ),
        }
    }
    out.sort();
    out.dedup();
    memo.insert(router, out.clone());
    out
}

/// Give every held-back registration the prefixes its router is mounted at,
/// across every scanned file, and drop the route each mount call made.
/// `resolve(importer, spec)` names the file a module specifier imports, among
/// the scanned ones.
///
/// A registration on a router with no readable mount is refused and counted,
/// once ([NFR-RA-05]); one mounted at a readable path and an unreadable one
/// keeps the readable route and is counted too.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
pub(super) fn join(scanned: &mut [(&str, &mut FileMatches)], resolve: impl Fn(&str, &str) -> Option<String>) {
    if scanned.iter().all(|(_, m)| m.mounts.pending.is_empty() && m.mounts.mounts.is_empty()) {
        return;
    }
    let (edges, mounting) = {
        let files: Vec<(&str, &Facts)> = scanned.iter().map(|(rel, m)| (*rel, &m.mounts)).collect();
        let index_of = files.iter().enumerate().map(|(index, (rel, _))| (*rel, index)).collect();
        Universe { files: &files, index_of, resolve }.edges()
    };
    let absorbs: Vec<bool> = scanned.iter().map(|(_, m)| m.mounts.absorbs).collect();
    let mut memo: HashMap<RouterId, Vec<Option<String>>> = HashMap::new();
    for (file, (_, out)) in scanned.iter_mut().enumerate() {
        let verbs = mounting.get(&file);
        let route_verbs = &out.mounts.route_verbs;
        let is_mount = |at: usize| route_verbs.get(&at).is_some_and(|verb| verbs.is_some_and(|v| v.contains(verb)));
        out.routes.retain(|route| !is_mount(route.at));
        let pending = std::mem::take(&mut out.mounts.pending);
        if pending.is_empty() {
            continue;
        }
        let mut refused: HashSet<usize> = HashSet::new();
        for (router, route) in pending.into_iter().filter(|(_, route)| !is_mount(route.at)) {
            let served = prefixes((file, router), &edges, &absorbs, &mut memo);
            if served.is_empty() || served.contains(&None) {
                refuse(&mut out.refusals, &mut refused, &route);
            }
            for prefix in served.into_iter().flatten() {
                let mut composed = route.clone();
                composed.path = join_under(&prefix, &route.path, absorbs[file]);
                out.routes.push(composed);
            }
        }
        drop_non_path_routes(&mut out.routes);
        dedup_routes(&mut out.routes);
    }
}

/// The `@fw.mount.*` captures of one query match, read off during
/// [`generic_match`](super::generic_match)'s walk of its captures and written
/// into the file's [`Facts`] once the match is whole.
#[derive(Default)]
pub(super) struct Captures<'t> {
    route_receiver: Option<Node<'t>>,
    route_verb: Option<Node<'t>>,
    verb: Option<Node<'t>>,
    callee: Option<Node<'t>>,
    receiver: Option<Node<'t>>,
    path: Option<Node<'t>>,
    first: Option<Node<'t>>,
    middleware: bool,
    absorbs: bool,
    arg: Option<Node<'t>>,
    arg_module: Option<Node<'t>>,
    router: Option<Node<'t>>,
    router_module: Option<Node<'t>>,
    router_ctor: Option<Node<'t>>,
    router_from: Option<Node<'t>>,
    shadow: Option<Node<'t>>,
    scope: Option<Node<'t>>,
    framework: Option<Node<'t>>,
    import_name: Option<Node<'t>>,
    import_module: Option<Node<'t>>,
    import_export: Option<Node<'t>>,
    import_named: bool,
    import_namespace: bool,
    export_local: Option<Node<'t>>,
    export_name: Option<Node<'t>>,
    export_same: bool,
}

impl<'t> Captures<'t> {
    /// Read the capture `name` on `node` when it is one of this module's:
    /// `true` when it was. A `@fw.mount.*` role not named here exists only for
    /// the query's predicates.
    pub(super) fn read(&mut self, name: &str, node: Node<'t>) -> bool {
        let Some(role) = name.strip_prefix("fw.mount.") else {
            return false;
        };
        let slot = match role {
            "route.receiver" => &mut self.route_receiver,
            "route.verb" => &mut self.route_verb,
            "verb" => &mut self.verb,
            "callee" => &mut self.callee,
            "receiver" => &mut self.receiver,
            "path" => &mut self.path,
            "first" => &mut self.first,
            "arg" => &mut self.arg,
            "arg.module" => &mut self.arg_module,
            "router" => &mut self.router,
            "router.module" => &mut self.router_module,
            "router.ctor" => &mut self.router_ctor,
            "router.from" => &mut self.router_from,
            "shadow" => &mut self.shadow,
            "scope" => &mut self.scope,
            "framework" => &mut self.framework,
            "import.name" => &mut self.import_name,
            "import.module" => &mut self.import_module,
            "import.export" => &mut self.import_export,
            "export.local" => &mut self.export_local,
            "export.name" => &mut self.export_name,
            "middleware" => {
                self.middleware = true;
                return true;
            }
            "absorbs" => {
                self.absorbs = true;
                return true;
            }
            "import.named" => {
                self.import_named = true;
                return true;
            }
            "import.namespace" => {
                self.import_namespace = true;
                return true;
            }
            "export.same" => {
                self.export_same = true;
                return true;
            }
            _ => return true,
        };
        slot.get_or_insert(node);
        true
    }

    /// Record the match's facts; `at` is the registration's start byte, when
    /// the match is one.
    pub(super) fn record(&self, facts: &mut Facts, src: &[u8], at: usize) {
        let name_of = |node: Node<'_>| Some(text(node, src).trim()).filter(|t| is_identifier(t)).map(str::to_string);
        let span = |node: Node<'_>| (node.start_byte(), node.end_byte());
        let unquoted = |node: Node<'_>| crate::extract::refs::unquote(text(node, src)).trim().to_string();
        if let Some(name) = self.route_receiver.and_then(name_of) {
            facts.receivers.insert(at, name);
        }
        if let Some(verb) = self.route_verb {
            facts.route_verbs.insert(at, verb.start_byte());
        }
        if let Some(string) = self.framework {
            facts.framework.insert(span(string));
        }
        facts.absorbs |= self.absorbs;
        if let Some(callee) = self.callee {
            let mount = facts.mounts.entry(span(callee)).or_default();
            if let Some(verb) = self.verb {
                mount.verb = Some(verb.start_byte());
            }
            if let Some(receiver) = self.receiver.and_then(name_of) {
                mount.receiver = Some(receiver);
            }
            if let Some(path) = self.path {
                mount.path = Some(unquoted(path));
            }
            if let Some(first) = self.first {
                mount.first = Some(span(first));
            }
            mount.middleware_first |= self.middleware;
            let arg = match (self.arg, self.arg_module) {
                (Some(arg), _) => name_of(arg).map(|name| (span(arg), Arg::Name(name))),
                (None, Some(module)) => Some((span(module), Arg::Required(unquoted(module)))),
                (None, None) => None,
            };
            if let Some(arg) = arg.filter(|arg| !mount.args.contains(arg)) {
                mount.args.push(arg);
            }
        }
        self.record_router(facts, src);
        if let (Some(name), Some(module)) = (self.import_name.and_then(name_of), self.import_module) {
            let imported = match self.import_export {
                _ if self.import_namespace => Imported::Namespace,
                Some(export) => Imported::Named(text(export, src).trim().to_string()),
                None if self.import_named => Imported::Named(name.clone()),
                None => Imported::Default,
            };
            let decl = self.import_name.map_or(0, |n| n.start_byte());
            facts.imports.insert(name, Import { decl, module: unquoted(module), module_at: span(module), imported });
        }
        if let Some(local) = self.export_local.and_then(name_of) {
            let name = match self.export_name {
                Some(name) => Some(text(name, src).trim().to_string()),
                None if self.export_same => Some(local.clone()),
                None => None,
            };
            facts.exports.push(Export { name, local, at: self.export_local.map_or(0, |n| n.start_byte()) });
        }
    }

    /// A router binding, or a parameter that hides one, with its scope; a router
    /// the match also exports (`export const r = Router()`) is that export too.
    fn record_router(&self, facts: &mut Facts, src: &[u8]) {
        let Some(scope) = self.scope else { return };
        let scope = (scope.start_byte(), scope.end_byte());
        let (node, decl, maker) = match (self.router, self.shadow) {
            (Some(router), _) => {
                let maker = match (self.router_module, self.router_ctor, self.router_from) {
                    (Some(module), _, _) => Maker::Module(text(module, src).trim().to_string()),
                    (None, Some(ctor), _) => Maker::Ctor(text(ctor, src).trim().to_string()),
                    (None, None, Some(from)) => Maker::Required((from.start_byte(), from.end_byte())),
                    (None, None, None) => return,
                };
                (router, router.start_byte(), maker)
            }
            // A parameter is visible from its own name across the body it
            // scopes, which an arrow's expression body starts with.
            (None, Some(shadow)) => (shadow, shadow.start_byte(), Maker::Shadow),
            (None, None) => return,
        };
        let scope = (scope.0.min(decl), scope.1);
        let name = text(node, src).trim().to_string();
        if self.export_same && self.router.is_some() {
            facts.exports.push(Export { name: Some(name.clone()), local: name.clone(), at: node.end_byte() });
        }
        // One declaration matched by several patterns is one binding, over the
        // wider scope: `export const r = …` is also a declaration whose parent is
        // the export statement, and every declaration is a binding the shadow
        // pattern reads too — a router's maker wins over that.
        if let Some(known) = facts.routers.iter_mut().find(|r| r.name == name && r.decl == decl) {
            if known.scope.1 - known.scope.0 < scope.1 - scope.0 {
                known.scope = scope;
            }
            if known.maker == Maker::Shadow {
                known.maker = maker;
            }
            return;
        }
        facts.routers.push(Router { name, decl, scope, maker });
    }
}

#[cfg(test)]
#[path = "mount_tests.rs"]
mod tests;
