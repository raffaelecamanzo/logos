//! Route groups, bracket tokens and paths that ignore a class prefix (S-554,
//! [CR-177], S-821, [CR-255], [FR-FW-03], [FR-FW-05], [NFR-RA-05]).
//!
//! # A minimal API's prefix is a value, not a declaration
//!
//! `compose_prefixes` joins a route onto the prefix of the *declaration* its
//! source bytes sit in. ASP.NET Core's minimal APIs have no such declaration:
//!
//! ```text
//! var api = app.MapGroup("api/catalog").HasApiVersion(1, 0);
//! api.MapGet("/items", GetAll);
//! ```
//!
//! the prefix belongs to the **receiver** `api`, a local bound to a chain of
//! builder calls. Two groups of one method are siblings, so containment would
//! lend each the other's prefix; this module resolves the receiver instead.
//!
//! The dialect's query captures three kinds of fact, each a plain syntactic
//! range and nothing language-specific to this pass:
//!
//! - `@fw.group.route.receiver` on a registration — the expression the verb is
//!   called on (`@fw.route.receiver` is the TypeScript query's predicate-only
//!   helper, so this one is namespaced with the group facts);
//! - `@fw.group.call` + `@fw.group.receiver` — every call `x.m(…)` as a *link*
//!   of a chain, with `@fw.group.path` / `@fw.group.path.opaque` on the one
//!   that adds a prefix (`MapGroup`). A link that names no path is transparent:
//!   `HasApiVersion(1.0)` returns the group it was called on. A link marked
//!   `@fw.group.foreign` is a call the dialect reads nothing about: called on a
//!   plain builder it is one, called on a group it is unknown;
//! - `@fw.group.name` + `@fw.group.scope` — a name the code binds over a
//!   range: to an initializer (`@fw.group.init`), to a value known to be a
//!   plain endpoint builder (`@fw.group.root`), or to nothing the pass can read
//!   (a parameter or lambda parameter, which may be a group handed in).
//!
//! # What a receiver resolves to
//!
//! [`Facts::resolve`] walks the expression and answers one of three things:
//!
//! - **no prefix** — the chain ends at an identifier the dialect proves a root
//!   (`@fw.group.root.name`), or at one bound to a plain builder: the route
//!   keeps its own path;
//! - **a prefix** — the chain crossed one or more group links, each joined by
//!   [`join_under`] — under `@fw.group.absorbs`, a path of exactly `/` is the
//!   group itself; a name bound to an initializer is followed into it;
//! - **unknown** — the chain ends at a call or expression the pass cannot read
//!   (`Build(app)`), at a name nothing binds or proves a root, at a name bound
//!   to a parameter, crosses a group whose path is not a written literal, or
//!   crosses a foreign call made on a group. The route is **refused and counted**, never
//!   promoted at the partial path ([NFR-RA-05], [`super::RouteRefusal::PathNotComposed`]).
//!
//! Under `@fw.group.root.default` (Gin, whose receivers are mostly a plain
//! engine — `r := gin.Default()`, a field, a parameter) the default flips: a
//! name nothing binds and an expression the pass cannot read are plain
//! builders, so only a chain that crosses a group link, or a name bound to a
//! parameter the dialect captures, can be unknown. Without the marker every
//! such receiver is unknown (ASP.NET Core).
//!
//! Known ceiling, the transparent-link rule's own: a builder method that
//! returns a *different* group than the one it was called on reads as the same
//! group.
//!
//! # A package-level group used from another file (S-852)
//!
//! One file cannot see a name another file of its package declares, so under
//! `@fw.group.root.default` a name it does not bind reads as a plain router:
//!
//! ```text
//! // routes.go                           // handlers.go
//! var api = router.Group("/api")         api.GET("/x", h)
//! ```
//!
//! promotes `GET /x` where Gin serves `GET /api/x`. A dialect that captures the
//! package it declares (`@fw.group.package`) therefore holds back every
//! registration whose receiver chain ends at such a name — directly, or through
//! a local bound to a chain that does ([`Facts::unbound_root`]) — and
//! [`join_packages`] settles them once every file of the run is scanned: a name
//! that another file of the same package (the same directory and package clause)
//! declares at package level as a group is refused and counted, anything else
//! keeps the route it has. A package-level variable is a *group* when its
//! initializer resolves to anything but a plain builder ([`Facts::package_groups`]);
//! the package's other files are read for them even where they name no framework
//! themselves, because the file that declares a group often names none.
//!
//! Sync ≡ reindex: the join reads only this run's scans and the indexed files'
//! text, and a sync with a framework footprint re-scans every candidate, so an
//! edit to either file moves the route exactly as a fresh index does.
//!
//! Known ceilings: a group the package declares through a helper
//! (`var api = newGroup()`) is read as a plain builder, as it is within one
//! file; a package-level variable declared without a value and assigned a group
//! in a function (`var api *gin.RouterGroup`, then `api = r.Group("/api")`) is
//! no declaration this pass reads; and a group declared in a file the index does
//! not hold (an ignored file, a build-tagged twin) is not seen.
//!
//! # Bracket tokens and absolute paths
//!
//! A framework that writes `[controller]` / `[action]` in a route template
//! (ASP.NET Core) has the dialect capture the declaring type's name
//! (`@fw.route.token.controller`) on its prefix, which [`expand_tokens`]
//! substitutes at scan time; `[action]` is the registration's handler, known per
//! route once its prefix is joined, so [`settle_tokens`] expands it afterwards and
//! refuses a route whose path still names a token.
//!
//! A path the framework serves from the root whatever a prefix above it says
//! (`"/health"`, `"~/health"`) opts in with `@fw.route.absolute`
//! ([`absolute_path`]); `compose_prefixes` then leaves that registration alone
//! ([`Facts::is_final`]). The shape test is the shared pass's, never a predicate
//! of a query.
//!
//! [CR-177]: ../../../../docs/requests/CR-177-csharp-calls-members-routes-partials-and-typed-receivers-resolve.md
//! [CR-255]: ../../../../docs/requests/CR-255-route-groups-and-mounts-compose-their-prefixes.md
//! [FR-FW-03]: ../../../../docs/specs/requirements/FR-FW-03.md
//! [FR-FW-05]: ../../../../docs/specs/requirements/FR-FW-05.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::{
    dedup_routes, drop_non_path_routes, is_resolvable_prefix, join_under, refuse, rooted_path, text, FileMatches,
    RouteMatch,
};

/// How deep a receiver chain or a chain of bindings is followed before the
/// answer is "unknown". Resolution ends by construction (a link's receiver is a
/// strictly smaller range, and an initializer is read at its own declaration, so
/// no name is visible to itself); this bounds the recursion's stack on a
/// pathological generated chain, and a chain of more than this many links is
/// refused like any other the pass cannot read.
const MAX_DEPTH: u8 = 24;

/// An expression the scan recorded: where it is and, when it is a plain name,
/// what the name is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Expr {
    range: (usize, usize),
    /// The text, only when it is a plain identifier — the one expression kind a
    /// binding can be looked up by.
    name: Option<String>,
}

impl Expr {
    fn of(node: Node<'_>, src: &[u8]) -> Self {
        let written = text(node, src).trim();
        Self {
            range: (node.start_byte(), node.end_byte()),
            name: is_identifier(written).then(|| written.to_string()),
        }
    }
}

/// What a call link adds to the prefix of the expression it was called on.
/// Ordered by how much the scan read of the call: the pattern that read the
/// most about one call wins ([`Capture::finish`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum LinkPath {
    /// A call the dialect reads nothing about (`@fw.group.foreign`): its result
    /// is a plain builder when it was called on one, and unknown when it was
    /// called on a group, which it may or may not return.
    Foreign,
    /// Nothing: the call returns the group it was called on.
    Transparent,
    /// A path the pass cannot read (a variable, a raw string, a named argument).
    Opaque,
    /// A written, joinable literal.
    Literal(String),
}

/// One call `receiver.m(…)` of a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Link {
    receiver: Expr,
    path: LinkPath,
}

/// What a name is bound to over its scope.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Bound {
    /// A local with an initializer: the name is whatever the initializer is.
    Init(Expr),
    /// A value known to be a plain endpoint builder — no prefix.
    Root,
    /// A parameter, whose group (if it is one) the caller chose.
    Opaque,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    name: String,
    /// Start byte of the declaring name: the binding is visible after it.
    decl: usize,
    /// The range the binding is visible over.
    scope: (usize, usize),
    bound: Bound,
    /// Declared across its whole scope (`@fw.group.hoisted`): a member, or a
    /// package-level variable.
    hoisted: bool,
}

impl Binding {
    /// Where the binding's initializer is read from. A local reads what is
    /// declared before its own name, which is never itself. A hoisted binding
    /// is visible across its scope, and so are its hoisted siblings: it reads
    /// from the scope's last byte, so `var v1 = api.Group("/v1")` sees the
    /// package-level `api` declared beside it, before it or after it. A
    /// binding that reads itself that way is a cycle, which [`MAX_DEPTH`]
    /// ends as unknown.
    fn reads_from(&self) -> usize {
        if self.hoisted {
            self.scope.1.saturating_sub(1)
        } else {
            self.decl
        }
    }
}

/// What a receiver resolves to.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// `None` — no group in the chain; `Some` — the joined group prefix.
    Prefix(Option<String>),
    Unknown,
}

/// Every group fact one file's scan collected, plus the registrations that
/// carry a receiver. Consumed by [`compose`]; empty for a dialect whose query
/// captures none, which leaves the scan untouched.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Facts {
    /// A registration's receiver, keyed by the registration's start byte
    /// (`RouteMatch::at`).
    receivers: HashMap<usize, Expr>,
    /// Every call link, keyed by the call expression's byte range.
    links: HashMap<(usize, usize), Link>,
    /// Every binding, keyed by the name it binds — a lookup reads one name's
    /// bindings, not every binding of the file.
    bindings: HashMap<String, Vec<Binding>>,
    /// Names the dialect proves are a plain builder with no group in front of
    /// them wherever they are spelled (`@fw.group.root.name`): the static
    /// `WebApplication` of `WebApplication.CreateBuilder(args)`. A name nothing
    /// binds and nothing proves is not one.
    root_names: HashSet<String>,
    /// Names the file assigns to after their declaration
    /// (`@fw.group.assigned`), each with the value of every assignment, keyed by
    /// the assigned name's start byte: such a name is whatever it was last given,
    /// which no declaration states. A value is `None` where the dialect captured
    /// none (`@fw.group.assigned.value`), which makes the name unknown.
    assigned: HashMap<String, HashMap<usize, Option<Expr>>>,
    /// Registrations whose path is final as captured: composed here, or
    /// written absolute. `compose_prefixes` does not join a class prefix onto
    /// them.
    finals: HashSet<usize>,
    /// `true` once a group link carried `@fw.group.absorbs`: the dialect's
    /// groups serve a path of exactly `/` (or none) as the group itself ([`join_under`]),
    /// the group spelling of a prefix scope's `@fw.route.prefix.absorbs`. Held per
    /// file, not per link: groups are one dialect's mechanism, and a file is one
    /// language.
    absorbs: bool,
    /// `true` once a registration's receiver carried `@fw.group.root.default`:
    /// the dialect's receivers are plain builders unless a group link or a
    /// bound parameter says otherwise — a name nothing binds, and an expression
    /// the pass cannot read, keep the route's own path. Held per file, like
    /// [`Facts::absorbs`].
    root_default: bool,
    /// The package the file declares (`@fw.group.package`): present only for a
    /// dialect whose packages span files, which opts it into [`join_packages`].
    package: Option<String>,
    /// Registrations held back from `routes` because their receiver chain ends at
    /// a name the file does not bind — the name, and the route composed as this
    /// file reads it. [`join_packages`] refuses the ones whose name a sibling
    /// file declares as a group and returns the rest.
    held: Vec<(String, RouteMatch)>,
}

impl Facts {
    /// A route path read under `@fw.route.absolute` ([`absolute_path`]): the path
    /// as it is served, and — when it is absolute — the registration at `at`
    /// marked final, so no prefix is joined onto it.
    pub(super) fn read_absolute(&mut self, at: usize, written: &str, marked: bool) -> String {
        let (path, absolute) = absolute_path(written, marked);
        if absolute {
            self.finals.insert(at);
        }
        path
    }

    /// `true` when the registration at `at` is already at its final path.
    pub(super) fn is_final(&self, at: usize) -> bool {
        self.finals.contains(&at)
    }

    /// The binding of `name` governing byte `at`: of the scopes containing it
    /// and declared before it, the innermost, then the latest.
    fn visible(&self, name: &str, at: usize) -> Option<&Binding> {
        self.bindings
            .get(name)?
            .iter()
            .filter(|b| b.decl < at && b.scope.0 <= at && at < b.scope.1)
            .max_by_key(|b| (b.scope.0, b.decl))
    }

    /// `true` when the variable `name` names at byte `at` is assigned anything
    /// but a plain builder: a value the dialect did not capture, or one that
    /// resolves to a group or to unknown. An assignment re-binds the variable
    /// its own position sees — the same binding, or, where the facts bind
    /// neither, the same unbound name — so an `r = t` in another function, on
    /// another `r`, says nothing about this one. A name only ever assigned plain
    /// builders (`router = gin.New()`) is read as its declaration says. A value
    /// that reads the name itself (`g = g.Group("/x")`) recurses until
    /// [`MAX_DEPTH`], which answers unknown.
    fn assigned_other_than_a_root(&self, name: &str, at: usize, depth: u8) -> bool {
        let Some(values) = self.assigned.get(name) else {
            return false;
        };
        let binding_at = |pos: usize| self.visible(name, pos).map(|b| (b.decl, b.scope));
        let seen = binding_at(at);
        values.iter().any(|(&pos, value)| {
            binding_at(pos) == seen
                && value
                    .as_ref()
                    .is_none_or(|value| self.resolve(value, value.range.0, depth + 1) != Outcome::Prefix(None))
        })
    }

    /// What a receiver the facts say nothing about is: a plain builder under
    /// [`Facts::root_default`], otherwise unknown.
    fn unread(&self) -> Outcome {
        if self.root_default {
            Outcome::Prefix(None)
        } else {
            Outcome::Unknown
        }
    }

    /// The name a receiver chain ends at when nothing in this file declares it:
    /// the chain's links are walked down to their first expression, and a local
    /// bound to an initializer is followed into it. `None` where the chain ends
    /// at a name this file binds (a parameter, a plain builder, a proven root),
    /// at an expression that is no name, or runs past [`MAX_DEPTH`].
    fn unbound_root(&self, expr: &Expr, at: usize, depth: u8) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        if let Some(link) = self.links.get(&expr.range) {
            return self.unbound_root(&link.receiver, at, depth + 1);
        }
        let name = expr.name.as_ref()?;
        match self.visible(name, at) {
            None => (!self.root_names.contains(name)).then(|| name.clone()),
            Some(binding) => match &binding.bound {
                Bound::Init(init) => self.unbound_root(init, binding.reads_from(), depth + 1),
                Bound::Root | Bound::Opaque => None,
            },
        }
    }

    /// The package-level variables of this file that are groups: a hoisted
    /// binding whose initializer resolves to a prefix or to anything the pass
    /// cannot read, or which the file assigns something else. One initialized to
    /// a plain builder (`var router = gin.Default()`) is not.
    fn package_groups(&self) -> impl Iterator<Item = &str> {
        self.bindings.values().flatten().filter_map(|binding| {
            let Bound::Init(init) = &binding.bound else {
                return None;
            };
            if !binding.hoisted {
                return None;
            }
            let at = binding.reads_from();
            let group = self.resolve(init, at, 0) != Outcome::Prefix(None)
                || self.assigned_other_than_a_root(&binding.name, at, 0);
            group.then_some(binding.name.as_str())
        })
    }

    fn resolve(&self, expr: &Expr, at: usize, depth: u8) -> Outcome {
        if depth > MAX_DEPTH {
            return Outcome::Unknown;
        }
        if let Some(link) = self.links.get(&expr.range) {
            let parent = self.resolve(&link.receiver, at, depth + 1);
            return match (&link.path, parent) {
                (_, Outcome::Unknown) | (LinkPath::Opaque, _) => Outcome::Unknown,
                (LinkPath::Foreign, Outcome::Prefix(None)) => Outcome::Prefix(None),
                (LinkPath::Foreign, Outcome::Prefix(Some(_))) => Outcome::Unknown,
                (LinkPath::Transparent, parent) => parent,
                (LinkPath::Literal(path), Outcome::Prefix(parent)) => Outcome::Prefix(Some(match parent {
                    Some(parent) => join_under(&parent, path, self.absorbs),
                    None => path.clone(),
                })),
            };
        }
        let Some(name) = &expr.name else {
            return self.unread();
        };
        if self.assigned_other_than_a_root(name, at, depth) {
            return Outcome::Unknown;
        }
        match self.visible(name, at) {
            // A name nothing binds is a field, a pattern variable, a loop
            // variable, a type: only one the dialect proves is a root is.
            None if self.root_names.contains(name) => Outcome::Prefix(None),
            None => self.unread(),
            Some(binding) => match &binding.bound {
                Bound::Root => Outcome::Prefix(None),
                Bound::Opaque => Outcome::Unknown,
                Bound::Init(init) => self.resolve(init, binding.reads_from(), depth + 1),
            },
        }
    }
}

/// Join every receiver-bearing route onto the group prefix its receiver
/// resolves to; refuse (and count) the ones whose prefix is unknown. Runs
/// before [`super::compose_prefixes`], which then leaves these routes alone.
pub(super) fn compose(out: &mut FileMatches) {
    if out.groups.receivers.is_empty() {
        return;
    }
    let mut refused: HashSet<usize> = HashSet::new();
    let mut finals: Vec<usize> = Vec::new();
    let routes = std::mem::take(&mut out.routes);
    for mut route in routes {
        let Some(receiver) = out.groups.receivers.get(&route.at) else {
            out.routes.push(route);
            continue;
        };
        match out.groups.resolve(receiver, route.at, 0) {
            Outcome::Prefix(prefix) => {
                if let Some(prefix) = prefix {
                    route.path = join_under(&prefix, &route.path, out.groups.absorbs);
                }
                finals.push(route.at);
                match out.groups.package.as_ref().and_then(|_| out.groups.unbound_root(receiver, route.at, 0)) {
                    Some(name) => out.groups.held.push((name, route)),
                    None => out.routes.push(route),
                }
            }
            Outcome::Unknown => refuse(&mut out.refusals, &mut refused, &route),
        }
    }
    out.groups.finals.extend(finals);
}

/// The directory part of a project-relative path (`""` for one at the root).
pub(super) fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// `true` when the file held back a registration ([`join_packages`] settles it).
pub(super) fn holds_registrations(out: &FileMatches) -> bool {
    !out.groups.held.is_empty()
}

/// Settle the registrations [`compose`] held back, across every scanned file:
/// one whose receiver chain ends at a name another file of its package declares
/// at package level as a group is refused and counted, once per registration
/// ([NFR-RA-05]); every other keeps the route it was composed to.
///
/// `siblings` are files of the run's packages that are no candidate themselves
/// (they name no framework) — they declare groups and promote nothing.
///
/// A package is the directory and the package clause the file declares; a file
/// whose dialect captures no package is in none, so it is never refused here.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
pub(super) fn join_packages(scanned: &mut [(&str, &mut FileMatches)], siblings: &[(&str, FileMatches)]) {
    if scanned.iter().all(|(_, m)| m.groups.held.is_empty()) {
        return;
    }
    // Every group each file declares at package level, by package.
    type Package<'a> = (&'a str, &'a str);
    let mut declared: HashMap<Package<'_>, Vec<(&str, HashSet<&str>)>> = HashMap::new();
    let files = scanned
        .iter()
        .map(|(rel, m)| (*rel, &**m))
        .chain(siblings.iter().map(|(rel, m)| (*rel, m)));
    for (rel, matches) in files {
        let Some(package) = matches.groups.package.as_deref() else {
            continue;
        };
        declared
            .entry((parent_dir(rel), package))
            .or_default()
            .push((rel, matches.groups.package_groups().collect()));
    }
    // Decided before any file is touched: whether each held registration's name
    // is a group another file of its package declares.
    let doomed: Vec<Vec<bool>> = scanned
        .iter()
        .map(|(rel, m)| {
            let package = m.groups.package.as_deref().map(|package| (parent_dir(rel), package));
            m.groups
                .held
                .iter()
                .map(|(name, _)| {
                    package.and_then(|package| declared.get(&package)).is_some_and(|files| {
                        files.iter().any(|(other, groups)| other != rel && groups.contains(name.as_str()))
                    })
                })
                .collect()
        })
        .collect();
    for ((_, out), doomed) in scanned.iter_mut().zip(doomed) {
        let held = std::mem::take(&mut out.groups.held);
        if held.is_empty() {
            continue;
        }
        let (refusing, kept): (Vec<_>, Vec<_>) = held.into_iter().zip(doomed).partition(|(_, doomed)| *doomed);
        // A refused registration is one only if it would have been promoted: the
        // path rule judges it as it judges a route that is kept.
        let mut refusing: Vec<RouteMatch> = refusing.into_iter().map(|((_, route), _)| route).collect();
        drop_non_path_routes(&mut refusing);
        let mut refused: HashSet<usize> = HashSet::new();
        for route in &refusing {
            refuse(&mut out.refusals, &mut refused, route);
        }
        out.routes.extend(kept.into_iter().map(|((_, route), _)| route));
        settle_tokens(out);
        drop_non_path_routes(&mut out.routes);
        dedup_routes(&mut out.routes);
    }
}

/// Substitute the `[controller]` and `[action]` tokens of a route template.
///
/// `[controller]` is the declaring type's name without its `Controller` suffix
/// (a type named just `Controller` keeps it); an absent value leaves the token
/// as written, which no URL path can be matched against.
pub(super) fn expand_tokens(template: &str, controller: Option<&str>, action: Option<&str>) -> String {
    let mut expanded = template.to_string();
    if let Some(controller) = controller {
        let name = controller
            .strip_suffix("Controller")
            .filter(|rest| !rest.is_empty())
            .unwrap_or(controller);
        expanded = expanded.replace("[controller]", name);
    }
    if let Some(action) = action {
        expanded = expanded.replace("[action]", action);
    }
    expanded
}

/// Expand the `[action]` token of every composed route from its handler (the
/// action method, `Async` suffix trimmed as ASP.NET Core does by default), and
/// refuse — and count — a route whose path still names a token afterwards
/// (`[area]`, a `[controller]` written in an action's own path, an `[action]` of a
/// registration with no handler): that is not a URL path, and no consumer can
/// match it. Runs after [`super::compose_prefixes`], because the token may sit in
/// the class prefix a route has just been joined onto.
pub(super) fn settle_tokens(out: &mut FileMatches) {
    let mut refused: HashSet<usize> = HashSet::new();
    let routes = std::mem::take(&mut out.routes);
    for mut route in routes {
        if route.path.contains('[') {
            let action = route.handler.as_deref().and_then(|h| h.rsplit("::").next()).map(action_name);
            route.path = expand_tokens(&route.path, None, action);
            if names_a_token(&route.path) {
                refuse(&mut out.refusals, &mut refused, &route);
                continue;
            }
        }
        out.routes.push(route);
    }
}

/// A method's action name: ASP.NET Core drops a trailing `Async` unless told not
/// to (`SuppressAsyncSuffixInActionNames`, default on), and a method named just
/// `Async` keeps it.
fn action_name(method: &str) -> &str {
    method.strip_suffix("Async").filter(|rest| !rest.is_empty()).unwrap_or(method)
}

/// `true` when the path holds a `[token]` — a bracketed run of letters, which no
/// path segment a framework serves spells.
fn names_a_token(path: &str) -> bool {
    path.match_indices('[').any(|(at, _)| {
        let rest = &path[at + 1..];
        rest.find(']').is_some_and(|end| end > 0 && rest[..end].chars().all(|c| c.is_ascii_alphabetic()))
    })
}

/// A route path under `@fw.route.absolute`: whether the framework serves it
/// from the root whatever prefix its declaring type states, and the path with
/// the root marker read. A leading `/` is absolute, and so is `~/` (ASP.NET
/// Core's "from the application root"), which reads as `/`. Without the marker
/// no path is absolute and the text is untouched.
fn absolute_path(written: &str, marked: bool) -> (String, bool) {
    if !marked {
        return (written.to_string(), false);
    }
    match written.strip_prefix("~/") {
        Some(rest) => (format!("/{rest}"), true),
        None => (written.to_string(), written.starts_with('/')),
    }
}

/// `true` for a plain identifier — the expression a binding is looked up by.
pub(super) fn is_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().enumerate().all(|(at, c)| c == '_' || c.is_alphabetic() || (at > 0 && c.is_alphanumeric()))
}

/// The `@fw.group.*` captures of one query match, accumulated while its
/// captures are read and turned into [`Facts`] once they all are.
#[derive(Default)]
pub(super) struct Capture<'t> {
    call: Option<Node<'t>>,
    receiver: Option<Node<'t>>,
    path: Option<Node<'t>>,
    opaque: Option<Node<'t>>,
    route_receiver: Option<Node<'t>>,
    rooted: bool,
    name: Option<Node<'t>>,
    init: Option<Node<'t>>,
    scope: Option<Node<'t>>,
    root: bool,
    root_name: Option<Node<'t>>,
    assigned: Option<Node<'t>>,
    assigned_value: Option<Node<'t>>,
    hoisted: bool,
    absolute: bool,
    absorbs: bool,
    foreign: bool,
    root_default: bool,
    controller: Option<Node<'t>>,
    package: Option<Node<'t>>,
}

/// The role a capture plays for [`Capture::take`], or `None` for a capture that
/// is not one of this module's: the `@fw.group.*` facts, and the two route-level
/// captures that belong with them (`@fw.route.absolute`,
/// `@fw.route.token.controller`).
pub(super) fn role_of(capture: &str) -> Option<&str> {
    match capture {
        "fw.route.absolute" => Some("absolute"),
        "fw.route.token.controller" => Some("token.controller"),
        _ => capture.strip_prefix("fw.group."),
    }
}

impl<'t> Capture<'t> {
    /// Take one capture of the match; `role` is the name after `fw.group.`.
    /// Roles that exist only for the query's own predicates are ignored.
    pub(super) fn take(&mut self, role: &str, node: Node<'t>) {
        match role {
            "call" => self.call = Some(node),
            "receiver" => self.receiver = Some(node),
            "path" => self.path = Some(node),
            "path.opaque" => self.opaque = Some(node),
            "route.receiver" => self.route_receiver = Some(node),
            "rooted" => self.rooted = true,
            "name" => self.name = Some(node),
            "init" => self.init = Some(node),
            "scope" => self.scope = Some(node),
            "root" => self.root = true,
            "root.name" => self.root_name = Some(node),
            "assigned" => self.assigned = Some(node),
            "assigned.value" => self.assigned_value = Some(node),
            "hoisted" => self.hoisted = true,
            "absolute" => self.absolute = true,
            "absorbs" => self.absorbs = true,
            "foreign" => self.foreign = true,
            "root.default" => self.root_default = true,
            "token.controller" => self.controller = Some(node),
            "package" => self.package = Some(node),
            _ => {}
        }
    }

    /// Whether the match carries `@fw.route.absolute`.
    pub(super) fn absolute(&self) -> bool {
        self.absolute
    }

    /// The declaring type's name (`@fw.route.token.controller`), if captured.
    pub(super) fn controller<'s>(&self, src: &'s [u8]) -> Option<&'s str> {
        self.controller.map(|node| text(node, src).trim())
    }

    /// Record what the match declared: the receiver of the registration at `at`
    /// (`@fw.group.route.receiver`), a call link, or a name binding.
    pub(super) fn finish(self, at: usize, src: &[u8], facts: &mut Facts) {
        if let Some(receiver) = self.route_receiver {
            facts.receivers.insert(at, Expr::of(receiver, src));
        }
        facts.absorbs |= self.absorbs;
        facts.root_default |= self.root_default;
        if let Some(package) = self.package {
            facts.package = Some(text(package, src).trim().to_string());
        }
        if let (Some(call), Some(receiver)) = (self.call, self.receiver) {
            let path = self.link_path(src);
            let range = (call.start_byte(), call.end_byte());
            // One call is matched by a dialect's catch-all foreign pattern, by
            // the transparent one, by the one that marks its first argument
            // opaque and by the one that reads it as a literal: each is right
            // about what it matched, so the most that any of them read wins,
            // whatever the match order.
            if facts.links.get(&range).is_none_or(|known| known.path < path) {
                facts.links.insert(range, Link { receiver: Expr::of(receiver, src), path });
            }
        }
        if let Some(name) = self.assigned {
            // One assigned name is matched by the pattern that marks every
            // assigned name and by the one that pairs it with its value: the
            // value, where one was read, is kept whatever the match order.
            let value = self.assigned_value.map(|value| Expr::of(value, src));
            let slot = facts
                .assigned
                .entry(text(name, src).trim().to_string())
                .or_default()
                .entry(name.start_byte())
                .or_default();
            if value.is_some() {
                *slot = value;
            }
        }
        if let Some(name) = self.root_name {
            facts.root_names.insert(text(name, src).trim().to_string());
        }
        if let (Some(name), Some(scope)) = (self.name, self.scope) {
            let bound = match (self.init, self.root) {
                (Some(init), _) => Bound::Init(Expr::of(init, src)),
                (None, true) => Bound::Root,
                (None, false) => Bound::Opaque,
            };
            let binding = Binding {
                name: text(name, src).trim().to_string(),
                // A member is in scope across its whole type, declared before
                // its use or after it.
                decl: if self.hoisted { scope.start_byte() } else { name.start_byte() },
                scope: (scope.start_byte(), scope.end_byte()),
                bound,
                hoisted: self.hoisted,
            };
            facts.bindings.entry(binding.name.clone()).or_default().push(binding);
        }
    }

    /// What the call adds: its path where one is written and joinable. The
    /// literal pattern matches only a positional string in the first position,
    /// so where it matched that argument *is* the literal; an argument only the
    /// opaque pattern matched (a variable, a raw string, a named argument) is
    /// one the pass cannot read.
    fn link_path(&self, src: &[u8]) -> LinkPath {
        match (self.path, self.opaque) {
            (Some(path), _) => {
                let written = crate::extract::refs::unquote(text(path, src)).trim();
                let written = rooted_path(written, self.rooted);
                if is_resolvable_prefix(&written) {
                    LinkPath::Literal(written)
                } else {
                    LinkPath::Opaque
                }
            }
            (None, Some(_)) => LinkPath::Opaque,
            (None, None) if self.foreign => LinkPath::Foreign,
            (None, None) => LinkPath::Transparent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(start: usize, end: usize, name: Option<&str>) -> Expr {
        Expr { range: (start, end), name: name.map(str::to_string) }
    }

    fn bind(facts: &mut Facts, binding: Binding) {
        facts.bindings.entry(binding.name.clone()).or_default().push(binding);
    }

    fn link(facts: &mut Facts, call: (usize, usize), receiver: Expr, path: LinkPath) {
        facts.links.insert(call, Link { receiver, path });
    }

    #[test]
    fn tokens_expand_to_the_controller_without_its_suffix_and_to_the_action() {
        assert_eq!(expand_tokens("api/[controller]", Some("CatalogController"), None), "api/Catalog");
        assert_eq!(expand_tokens("[action]/x", None, Some("List")), "List/x");
        // A type named just `Controller` keeps its whole name.
        assert_eq!(expand_tokens("[controller]", Some("Controller"), None), "Controller");
        // Another casing is not the token; an absent value leaves it as written.
        assert_eq!(expand_tokens("[Controller]", Some("AController"), None), "[Controller]");
        assert_eq!(expand_tokens("[action]", Some("AController"), None), "[action]");
    }

    #[test]
    fn a_token_is_a_bracketed_run_of_letters_and_an_async_suffix_is_trimmed() {
        assert!(names_a_token("/api/[area]/x"));
        assert!(names_a_token("/[controller]"));
        assert!(!names_a_token("/api/{id:regex(^[a-z]+$)}"));
        assert!(!names_a_token("/api/x[]"));
        assert!(!names_a_token("/api/items"));
        assert_eq!(action_name("ListAsync"), "List");
        assert_eq!(action_name("Async"), "Async");
        assert_eq!(action_name("List"), "List");
    }

    #[test]
    fn only_a_plain_name_is_looked_up_as_a_binding() {
        for name in ["app", "_g", "g1", "Ünï"] {
            assert!(is_identifier(name), "{name}");
        }
        for not in ["", "1g", "a.b", "a b", "a()", "this.g"] {
            assert!(!is_identifier(not), "{not}");
        }
    }

    #[test]
    fn an_absolute_path_is_read_only_under_the_marker() {
        assert_eq!(absolute_path("/health", true), ("/health".to_string(), true));
        assert_eq!(absolute_path("~/health", true), ("/health".to_string(), true));
        assert_eq!(absolute_path("health", true), ("health".to_string(), false));
        assert_eq!(absolute_path("", true), (String::new(), false));
        assert_eq!(absolute_path("/health", false), ("/health".to_string(), false));
        assert_eq!(absolute_path("~/health", false), ("~/health".to_string(), false));
    }

    #[test]
    fn a_group_root_path_is_the_group_and_any_other_path_is_joined() {
        assert_eq!(join_under("/api", "/", true), "/api");
        assert_eq!(join_under("/api/", "/", true), "/api");
        assert_eq!(join_under("/", "/", true), "/");
        assert_eq!(join_under("/api", "/items", true), "/api/items");
        assert_eq!(join_under("/api/", "items", true), "/api/items");
        // A prefix that does not absorb keeps the trailing slash (Spring's rule).
        assert_eq!(join_under("/api", "/", false), "/api/");
    }

    #[test]
    fn a_group_absorbs_a_root_path_only_when_its_dialect_says_so() {
        // `app` ← `g = app.MapGroup("/books")`, and a route `g.MapGet("/")`.
        let mut facts = Facts::default();
        facts.root_names.insert("app".to_string());
        link(&mut facts, (10, 30), expr(10, 13, Some("app")), LinkPath::Literal("/books".into()));
        link(&mut facts, (40, 60), expr(40, 41, Some("g")), LinkPath::Literal("/".into()));
        bind(&mut facts, Binding {
            name: "g".into(),
            decl: 4,
            scope: (0, 100),
            bound: Bound::Init(expr(10, 30, None)),
            hoisted: false,
        });
        // A nested group written `/` is its parent only under the opt-in.
        assert_eq!(facts.resolve(&expr(40, 60, None), 70, 0), Outcome::Prefix(Some("/books/".to_string())));
        facts.absorbs = true;
        assert_eq!(facts.resolve(&expr(40, 60, None), 70, 0), Outcome::Prefix(Some("/books".to_string())));
    }

    #[test]
    fn a_name_resolves_through_its_nearest_preceding_binding_in_the_innermost_scope() {
        // `app` (unbound) ← `a = app.MapGroup("/a")` (0..100) and, inside the
        // block 40..90, a later `b = a.MapGroup("/b")`.
        let mut facts = Facts::default();
        facts.root_names.insert("app".to_string());
        link(&mut facts, (10, 30), expr(10, 13, Some("app")), LinkPath::Literal("/a".into()));
        link(&mut facts, (50, 70), expr(50, 51, Some("a")), LinkPath::Literal("/b".into()));
        bind(&mut facts, Binding {
            name: "a".into(),
            decl: 4,
            scope: (0, 100),
            bound: Bound::Init(expr(10, 30, None)),
            hoisted: false,
        });
        bind(&mut facts, Binding {
            name: "b".into(),
            decl: 44,
            scope: (40, 90),
            bound: Bound::Init(expr(50, 70, None)),
            hoisted: false,
        });
        let at_b = facts.resolve(&expr(80, 81, Some("b")), 80, 0);
        assert_eq!(at_b, Outcome::Prefix(Some("/a/b".to_string())));
        // Before its declaration, and outside its scope, `b` is just a name no one
        // proves a root: unknown.
        assert_eq!(facts.resolve(&expr(41, 42, Some("b")), 41, 0), Outcome::Unknown);
        assert_eq!(facts.resolve(&expr(95, 96, Some("b")), 95, 0), Outcome::Unknown);
        // A name the dialect proves a root is one wherever it is spelled.
        assert_eq!(facts.resolve(&expr(95, 96, Some("app")), 95, 0), Outcome::Prefix(None));
        assert_eq!(
            facts.resolve(&expr(95, 96, Some("a")), 95, 0),
            Outcome::Prefix(Some("/a".to_string()))
        );
    }

    #[test]
    fn a_parameter_an_opaque_path_or_an_unreadable_expression_is_unknown() {
        let mut facts = Facts::default();
        bind(&mut facts, Binding {
            name: "g".into(),
            decl: 1,
            scope: (0, 50),
            bound: Bound::Opaque,
            hoisted: false,
        });
        link(&mut facts, (10, 20), expr(10, 11, Some("app")), LinkPath::Opaque);
        assert_eq!(facts.resolve(&expr(30, 31, Some("g")), 30, 0), Outcome::Unknown);
        assert_eq!(facts.resolve(&expr(10, 20, None), 30, 0), Outcome::Unknown);
        // A call no link describes (`Build(app)`) is not a name and not a chain.
        assert_eq!(facts.resolve(&expr(60, 70, None), 80, 0), Outcome::Unknown);
    }

    #[test]
    fn an_initializer_reads_names_declared_before_it_never_the_binding_itself() {
        // `var g = g.MapGroup("/x")` at byte 1: the `g` it reads is not yet bound,
        // so the walk ends there — it can neither loop nor see itself. (Here that
        // `g` is the root type name some earlier declaration provides.)
        let mut facts = Facts::default();
        facts.root_names.insert("g".to_string());
        link(&mut facts, (20, 40), expr(20, 21, Some("g")), LinkPath::Literal("/x".into()));
        bind(&mut facts, Binding {
            name: "g".into(),
            decl: 1,
            scope: (0, 50),
            bound: Bound::Init(expr(20, 40, None)),
            hoisted: false,
        });
        assert_eq!(
            facts.resolve(&expr(45, 46, Some("g")), 45, 0),
            Outcome::Prefix(Some("/x".to_string()))
        );
    }

    #[test]
    fn under_root_default_only_a_group_or_a_bound_parameter_is_unknown() {
        // `s.router` (not a name), `r` (unbound), `gin.Default()` (a foreign call
        // on an unbound name), `g = r.Group("/api")`, `g.Other()` (a foreign call
        // on a group), `rg` (a bound parameter).
        let mut facts = Facts::default();
        link(&mut facts, (10, 20), expr(10, 13, Some("gin")), LinkPath::Foreign);
        link(&mut facts, (30, 40), expr(30, 31, Some("r")), LinkPath::Literal("/api".into()));
        link(&mut facts, (50, 60), expr(50, 51, Some("g")), LinkPath::Foreign);
        bind(&mut facts, Binding {
            name: "g".into(),
            decl: 25,
            scope: (0, 100),
            bound: Bound::Init(expr(30, 40, None)),
            hoisted: false,
        });
        bind(&mut facts, Binding {
            name: "rg".into(),
            decl: 1,
            scope: (0, 100),
            bound: Bound::Opaque,
            hoisted: false,
        });
        let field = expr(70, 78, None);
        let bare = expr(80, 81, Some("r"));
        // Without the marker, each receiver the facts say nothing about is unknown.
        assert_eq!(facts.resolve(&field, 90, 0), Outcome::Unknown);
        assert_eq!(facts.resolve(&bare, 90, 0), Outcome::Unknown);
        assert_eq!(facts.resolve(&expr(10, 20, None), 90, 0), Outcome::Unknown);
        facts.root_default = true;
        assert_eq!(facts.resolve(&field, 90, 0), Outcome::Prefix(None));
        assert_eq!(facts.resolve(&bare, 90, 0), Outcome::Prefix(None));
        assert_eq!(facts.resolve(&expr(10, 20, None), 90, 0), Outcome::Prefix(None));
        assert_eq!(facts.resolve(&expr(90, 91, Some("g")), 90, 0), Outcome::Prefix(Some("/api".to_string())));
        // A foreign call on a group may return anything; a bound parameter is
        // whichever group the caller chose.
        assert_eq!(facts.resolve(&expr(50, 60, None), 90, 0), Outcome::Unknown);
        assert_eq!(facts.resolve(&expr(90, 92, Some("rg")), 90, 0), Outcome::Unknown);
    }

    #[test]
    fn a_hoisted_initializer_reads_its_hoisted_siblings_and_a_cycle_is_unknown() {
        // A file 0..100 declaring, as package-level variables, `v1 =
        // api.Group("/v1")` before `api = r.Group("/api")`, and the cycle
        // `a = b.Group("/a")`, `b = a.Group("/b")`. A hoisted binding's
        // declaration is its scope's start.
        let mut facts = Facts { root_default: true, ..Facts::default() };
        link(&mut facts, (14, 30), expr(14, 17, Some("api")), LinkPath::Literal("/v1".into()));
        link(&mut facts, (44, 60), expr(44, 45, Some("r")), LinkPath::Literal("/api".into()));
        link(&mut facts, (64, 70), expr(64, 65, Some("b")), LinkPath::Literal("/a".into()));
        link(&mut facts, (74, 80), expr(74, 75, Some("a")), LinkPath::Literal("/b".into()));
        for (name, init) in [("v1", (14, 30)), ("api", (44, 60)), ("a", (64, 70)), ("b", (74, 80))] {
            bind(&mut facts, Binding {
                name: name.into(),
                decl: 0,
                scope: (0, 100),
                bound: Bound::Init(expr(init.0, init.1, None)),
                hoisted: true,
            });
        }
        assert_eq!(
            facts.resolve(&expr(90, 92, Some("v1")), 90, 0),
            Outcome::Prefix(Some("/api/v1".to_string()))
        );
        assert_eq!(facts.resolve(&expr(90, 91, Some("a")), 90, 0), Outcome::Unknown);
    }

    #[test]
    fn a_chain_is_rooted_at_the_name_nothing_in_the_file_declares() {
        // `api.Group("/v1")` (10..28) and a local `g = api.Group("/v1")`; `r` is
        // a bound plain router, `p` a parameter, `gin` a proven root.
        let mut facts = Facts::default();
        facts.root_names.insert("gin".to_string());
        link(&mut facts, (10, 28), expr(10, 13, Some("api")), LinkPath::Literal("/v1".into()));
        bind(&mut facts, Binding {
            name: "g".into(),
            decl: 4,
            scope: (0, 100),
            bound: Bound::Init(expr(10, 28, None)),
            hoisted: false,
        });
        bind(&mut facts, Binding { name: "r".into(), decl: 1, scope: (0, 100), bound: Bound::Root, hoisted: false });
        bind(&mut facts, Binding { name: "p".into(), decl: 2, scope: (0, 100), bound: Bound::Opaque, hoisted: false });
        let rooted = |expr: Expr| facts.unbound_root(&expr, 50, 0);
        // The name itself, a chain from it, and a local bound to that chain.
        assert_eq!(rooted(expr(60, 63, Some("api"))), Some("api".to_string()));
        assert_eq!(rooted(expr(10, 28, None)), Some("api".to_string()));
        assert_eq!(rooted(expr(60, 61, Some("g"))), Some("api".to_string()));
        // A bound name, a proven root and an expression that is no name are not.
        for bound in [expr(60, 61, Some("r")), expr(60, 61, Some("p")), expr(60, 63, Some("gin")), expr(60, 70, None)] {
            assert_eq!(rooted(bound.clone()), None, "{bound:?}");
        }
    }

    #[test]
    fn a_hoisted_initializer_that_reads_itself_ends_at_no_name() {
        // `var a = a.Group("/x")`: the chain starts at the name it initializes, so
        // the walk loops through its own binding until the depth bound ends it.
        let mut facts = Facts::default();
        link(&mut facts, (10, 28), expr(10, 11, Some("a")), LinkPath::Literal("/x".into()));
        bind(&mut facts, Binding {
            name: "a".into(),
            decl: 0,
            scope: (0, 100),
            bound: Bound::Init(expr(10, 28, None)),
            hoisted: true,
        });
        assert_eq!(facts.unbound_root(&expr(60, 61, Some("a")), 60, 0), None);
    }

    #[test]
    fn a_hoisted_name_assigned_a_group_is_a_package_group() {
        // `var api = router` (plain), and `api = router.Group("/api")` elsewhere.
        let mut facts = Facts { root_default: true, ..Facts::default() };
        link(&mut facts, (30, 50), expr(30, 36, Some("router")), LinkPath::Literal("/api".into()));
        bind(&mut facts, Binding {
            name: "api".into(),
            decl: 0,
            scope: (0, 100),
            bound: Bound::Init(expr(10, 16, Some("router"))),
            hoisted: true,
        });
        assert_eq!(facts.package_groups().count(), 0);
        facts.assigned.entry("api".into()).or_default().insert(60, Some(expr(30, 50, None)));
        assert_eq!(facts.package_groups().collect::<Vec<_>>(), ["api"]);
    }

    #[test]
    fn only_a_hoisted_initializer_that_is_not_a_plain_builder_is_a_package_group() {
        // `router = gin.Default()` (a plain builder), `api = router.Group("/api")`,
        // `opaque = router.Group(prefix)` — package-level — and a local `g`.
        let mut facts = Facts { root_default: true, ..Facts::default() };
        link(&mut facts, (10, 22), expr(10, 13, Some("gin")), LinkPath::Foreign);
        link(&mut facts, (30, 50), expr(30, 36, Some("router")), LinkPath::Literal("/api".into()));
        link(&mut facts, (60, 80), expr(60, 66, Some("router")), LinkPath::Opaque);
        let init = |span: (usize, usize)| Bound::Init(expr(span.0, span.1, None));
        for (name, span, hoisted) in
            [("router", (10, 22), true), ("api", (30, 50), true), ("opaque", (60, 80), true), ("g", (30, 50), false)]
        {
            bind(&mut facts, Binding { name: name.into(), decl: 0, scope: (0, 100), bound: init(span), hoisted });
        }
        let mut groups: Vec<&str> = facts.package_groups().collect();
        groups.sort_unstable();
        assert_eq!(groups, ["api", "opaque"]);
    }

    #[test]
    fn a_link_that_read_more_replaces_one_that_read_less() {
        assert!(LinkPath::Foreign < LinkPath::Transparent);
        assert!(LinkPath::Transparent < LinkPath::Opaque);
        assert!(LinkPath::Opaque < LinkPath::Literal("/x".into()));
    }
}
