//! Nesting route scopes and handlers written on a typed value (S-561, [CR-178],
//! [FR-FW-03], [FR-FW-05], [NFR-RA-05]).
//!
//! # A nesting scope chains; a type's prefix does not
//!
//! Spring's `@RequestMapping("/v1")` governs the methods of its own type and
//! nothing outside it, so [`compose_prefixes`](super::compose_prefixes) reads
//! the **innermost** scope alone. Spring's functional DSL and Ktor nest the
//! other way: `"/admin".nest { GET("/users") }` inside `(accept(JSON) and
//! "/api").nest { … }` serves `/api/admin/users`, and Ktor's `route("/p") {
//! route("/q") { get("/x") } }` serves `/p/q/x`. A dialect declares such a
//! scope with `@fw.route.prefix.nest` beside its `@fw.route.prefix.scope`, and
//! [`Facts::chain`] joins every nesting scope that contains the registration,
//! outermost first, with the one separator rule [`join_route_path`](super::join_route_path)
//! already owns — under a scope marked `@fw.route.prefix.absorbs` (Rails), a path
//! of exactly `/` is that scope's prefix itself ([`join_under`]).
//!
//! # A nested registration is a route only inside a nesting scope
//!
//! `get("/x") { … }` is also what a MockMvc test or a cache lookup looks like.
//! A dialect marks the registrations that are routes only under a router block
//! (`@fw.route.nested` — Spring's `GET`, Ktor's `get`) and declares the block
//! itself a nesting scope (`coRouter { }`, `routing { }`, each stating the root
//! `/` through `@fw.route.prefix.root`). A marked registration outside every
//! nesting scope establishes nothing and is dropped without a trace, exactly
//! as a pathless registration with no prefix is. A scope whose prefix cannot be
//! read — a `route(PATH) { }`, a Ktor `fun Route.users()` whose mount point is
//! another file's — refuses every registration under it and counts it
//! ([`RouteRefusal::PathNotComposed`]): an under-composed address would claim
//! a provider the service does not serve.
//!
//! A dialect whose prefixing blocks mean something only inside its router block
//! marks them `@fw.route.prefix.within` beside `@fw.route.prefix.nest` (S-567):
//! such a scope joins its prefix onto a chain but cannot establish one. Rails'
//! `namespace :admin do … end` is a router prefix inside `routes.draw` and an
//! unrelated call outside it (a routes file another one `draw`s, a Grape API, a
//! test), so a registration under `within` scopes alone is dropped as one
//! outside every scope is — never promoted at a path missing the root.
//!
//! A registration's own handler body is not a router block either: the calls
//! inside `get("/h") { respond(buildJsonObject { put("status", "ok") }) }` are
//! the handler's code. A dialect marks the trailing lambda of a registration
//! `@fw.route.barrier` (naming the call, which counts only if the descriptor
//! maps it) and a registration inside one is dropped like one outside every
//! scope — otherwise a `put("status", …)` in a handler would be a `PUT /status`
//! route, and a kotlinx.html `head { }` a `HEAD /`.
//!
//! # A handler written on a typed value binds through its declared type
//!
//! `GET("/talk", talkHandler::find)` names a callable reference on a value, not
//! on a type, so the written `talkHandler::find` binds nothing. A dialect
//! captures the typed bindings of a type — a constructor property or a member
//! declaring its type (`@fw.binding.name`, `@fw.binding.type`,
//! `@fw.binding.scope`) — and [`Facts::type_handler`] reads the head as the
//! type the enclosing declaration states for it: `TalkHandler::find`, which the
//! binder proves or leaves unbound like any other handler text. Only a type
//! written as one bare name is read; a generic, nullable or qualified type
//! names a head the binder is not asked about.
//!
//! Known gap (the receiver rule's own, [`super::handler`]): the match is the
//! head's *text*, so a name a lambda rebinds still reads as the property.
//!
//! [CR-178]: ../../../../docs/requests/CR-178-kotlin-calls-declarations-accesses-routes-and-typed-receivers-resolve.md
//! [FR-FW-03]: ../../../../docs/specs/requirements/FR-FW-03.md
//! [FR-FW-05]: ../../../../docs/specs/requirements/FR-FW-05.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [`RouteRefusal::PathNotComposed`]: super::RouteRefusal::PathNotComposed

use std::collections::HashSet;

use tree_sitter::Node;

use super::{extend_unique, join_under, refuse, text, FoldedConstant, MergedScope, RouteMatch, RouteRefusal};

/// One typed binding a declaration states: the `name` it binds, the bare type
/// name `ty` it declares for it, and the byte range `[start, end)` of the
/// declaration whose members can see it.
#[derive(Debug, PartialEq, Eq)]
struct Binding {
    name: String,
    ty: String,
    start: usize,
    end: usize,
}

/// What a query declared about nesting, rooting and typed bindings — everything
/// [`Facts`]' consumers need that no route or prefix match carries itself.
/// Held apart from [`RouteMatch`] and [`super::PrefixScope`] so a dialect that
/// declares none of it leaves their shape — and every site that builds one —
/// untouched.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Facts {
    /// The `at` of every registration marked `@fw.route.nested`.
    nested: HashSet<usize>,
    /// The ranges of the prefix scopes marked `@fw.route.prefix.nest`.
    chained: HashSet<(usize, usize)>,
    /// The ranges of the nesting scopes also marked `@fw.route.prefix.within`:
    /// prefixes that join a chain but cannot establish one (S-567).
    within: HashSet<(usize, usize)>,
    /// The ranges of the opaque paths and prefixes written under
    /// `@fw.route.rooted` or `@fw.route.slashed`: what a fold must give its
    /// leading slash.
    rooted: HashSet<(usize, usize)>,
    bindings: Vec<Binding>,
    /// The `at` of every registration whose handler [`Facts::type_handler`]
    /// rewrote to `Type::member`.
    typed: HashSet<usize>,
    /// The ranges of the handler bodies of registrations (`@fw.route.barrier`):
    /// a call inside one is the handler's own code, never a registration.
    barriers: Vec<(usize, usize)>,
}

/// The nesting, rooting and binding captures of one query match, read off
/// during [`generic_match`](super::generic_match)'s walk of its captures and
/// written into the file's [`Facts`] once the match is whole.
#[derive(Default)]
pub(super) struct Captures<'t> {
    nested: bool,
    nest_scope: bool,
    within: bool,
    /// `@fw.route.slashed`: the framework serves a written path from the root,
    /// but an empty one is not a path to give a slash (Spring).
    slashed: bool,
    binding_name: Option<Node<'t>>,
    binding_type: Option<Node<'t>>,
    binding_scope: Option<Node<'t>>,
    /// `@fw.route.barrier` (the lambda) and `@fw.route.barrier.name` (the call
    /// it is the trailing lambda of, when the call is named).
    barrier: Option<Node<'t>>,
    barrier_name: Option<Node<'t>>,
}

impl<'t> Captures<'t> {
    /// Read the capture `name` on `node` when it is one of this module's:
    /// `true` when it was (and nothing else is to be done with it).
    pub(super) fn read(&mut self, name: &str, node: Node<'t>) -> bool {
        match name {
            "fw.route.nested" => self.nested = true,
            "fw.route.prefix.nest" => self.nest_scope = true,
            "fw.route.prefix.within" => self.within = true,
            "fw.route.slashed" => self.slashed = true,
            "fw.binding.name" => drop(self.binding_name.get_or_insert(node)),
            "fw.binding.type" => drop(self.binding_type.get_or_insert(node)),
            "fw.binding.scope" => drop(self.binding_scope.get_or_insert(node)),
            "fw.route.barrier" => drop(self.barrier.get_or_insert(node)),
            "fw.route.barrier.name" => drop(self.barrier_name.get_or_insert(node)),
            _ => return false,
        }
        true
    }

    pub(super) fn slashed(&self) -> bool {
        self.slashed
    }

    /// Record the match's registration facts: its typed binding, whether it is
    /// a nested registration (at byte `at`), the handler body it closes off
    /// (`methods` is the descriptor's registration table: a named call is a
    /// registration only if it maps), and the spans of the opaque paths and
    /// prefixes a fold must root when the match is `rooted` or slashed.
    pub(super) fn record(
        &self,
        facts: &mut Facts,
        src: &[u8],
        at: usize,
        rooted: bool,
        opaque_spans: impl Iterator<Item = (usize, usize)>,
        methods: &std::collections::BTreeMap<String, String>,
    ) {
        if let Some(body) = self.barrier {
            let registers = self.barrier_name.is_none_or(|name| methods.contains_key(text(name, src).trim()));
            if registers {
                facts.mark_barrier(body.start_byte(), body.end_byte());
            }
        }
        if let (Some(name), Some(ty), Some(scope)) = (self.binding_name, self.binding_type, self.binding_scope) {
            facts.bind(text(name, src), text(ty, src), (scope.start_byte(), scope.end_byte()));
        }
        if self.nested {
            facts.mark_nested(at);
        }
        if rooted || self.slashed {
            opaque_spans.for_each(|span| facts.mark_rooted(span));
        }
    }

    /// Record that the prefix scope `[start, end)` is a nesting scope, when the
    /// match said so.
    pub(super) fn record_scope(&self, facts: &mut Facts, start: usize, end: usize) {
        if self.nest_scope {
            facts.mark_chained(start, end);
            if self.within {
                facts.within.insert((start, end));
            }
        }
    }
}

/// What a nested registration's enclosing nesting scopes make of it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Chain {
    /// No nesting scope contains it: not a route.
    Outside,
    /// A scope on the way declared a prefix that cannot be read.
    Unreadable,
    /// The prefix(es) it is served under, each with the constants it was
    /// folded from, and whether the innermost scope that stated them absorbs a
    /// path of exactly `/` ([`join_under`]).
    Under(Vec<(String, Vec<FoldedConstant>)>, bool),
}

impl Facts {
    fn mark_nested(&mut self, at: usize) {
        self.nested.insert(at);
    }

    fn mark_barrier(&mut self, start: usize, end: usize) {
        self.barriers.push((start, end));
    }

    fn mark_chained(&mut self, start: usize, end: usize) {
        self.chained.insert((start, end));
    }

    fn mark_rooted(&mut self, span: (usize, usize)) {
        self.rooted.insert(span);
    }

    pub(super) fn is_rooted(&self, span: (usize, usize)) -> bool {
        self.rooted.contains(&span)
    }

    pub(super) fn has_nested(&self) -> bool {
        !self.nested.is_empty()
    }

    pub(super) fn is_nested(&self, at: usize) -> bool {
        self.nested.contains(&at)
    }

    fn bind(&mut self, name: &str, ty: &str, scope: (usize, usize)) {
        self.bindings.push(Binding {
            name: name.trim().to_string(),
            ty: ty.trim().to_string(),
            start: scope.0,
            end: scope.1,
        });
    }

    /// The nesting scopes containing `at` joined outermost first. `scopes` is
    /// sorted by start, so the containing ones come out in nesting order.
    pub(super) fn chain(&self, scopes: &[MergedScope], at: usize) -> Chain {
        // A call inside a registration's handler body is that handler's code
        // (`put("status", "ok")` in a JSON builder, kotlinx.html's `head { }`),
        // however many router blocks enclose the registration.
        if self.barriers.iter().any(|&(start, end)| start <= at && at < end) {
            return Chain::Outside;
        }
        let mut prefixes: Vec<(String, Vec<FoldedConstant>)> = vec![(String::new(), Vec::new())];
        // Each scope's path is joined under the rule of the scope it joins onto.
        let mut absorbs = false;
        let containing: Vec<&MergedScope> = scopes
            .iter()
            .filter(|s| s.start <= at && at < s.end && self.chained.contains(&(s.start, s.end)))
            .collect();
        // Only a scope not marked `within` establishes a chain: a Rails
        // `namespace` outside every `routes.draw` is no router (S-567).
        if !containing.iter().any(|s| !self.within.contains(&(s.start, s.end))) {
            return Chain::Outside;
        }
        for scope in containing {
            if scope.paths.is_empty() {
                if scope.opaque {
                    return Chain::Unreadable;
                }
                continue;
            }
            prefixes = prefixes
                .iter()
                .flat_map(|(outer, folded)| {
                    scope.paths.iter().map(move |path| {
                        let mut folded = folded.clone();
                        extend_unique(&mut folded, scope.folded_from.get(path).into_iter().flatten().cloned());
                        (join_under(outer, path, absorbs), folded)
                    })
                })
                .collect();
            absorbs = scope.absorbs;
        }
        Chain::Under(prefixes, absorbs)
    }

    /// Compose one nested registration with the nesting scopes around it:
    /// pushed once per prefix it is served under, dropped silently outside
    /// every scope, refused (once per site) under an unreadable prefix.
    pub(super) fn compose(
        &self,
        scopes: &[MergedScope],
        route: RouteMatch,
        composed: &mut Vec<RouteMatch>,
        refusals: &mut Vec<RouteRefusal>,
        refused_sites: &mut HashSet<usize>,
    ) {
        match self.chain(scopes, route.at) {
            Chain::Outside => {}
            Chain::Unreadable => refuse(refusals, refused_sites, &route),
            Chain::Under(prefixes, absorbs) => {
                for (prefix, folded) in prefixes {
                    let mut folded_from = route.folded_from.clone();
                    extend_unique(&mut folded_from, folded);
                    composed.push(RouteMatch {
                        path: join_under(&prefix, &route.path, absorbs),
                        folded_from,
                        ..route.clone()
                    });
                }
            }
        }
    }

    /// Rewrite a handler written `head::member`, whose head is a typed binding
    /// visible at the registration, into `Type::member`.
    pub(super) fn type_handler(&mut self, route: &mut RouteMatch) {
        let Some((head, member)) = route.handler.as_deref().and_then(|h| h.split_once("::")) else {
            return;
        };
        if member.contains("::") {
            return;
        }
        // The innermost declaration stating the name wins: declarations nest
        // rather than partially overlap, so it is the one that starts last.
        let declared = self
            .bindings
            .iter()
            .filter(|b| b.name == head && b.start <= route.at && route.at < b.end)
            .max_by_key(|b| b.start);
        if let Some(binding) = declared {
            route.handler = Some(format!("{}::{member}", binding.ty));
            self.typed.insert(route.at);
        }
    }

    /// `true` when the registration at `at` names its handler through a typed
    /// binding: `Type::member` there is a reference to an instance member, not a
    /// call on the type (a Kotlin type's call reads its companion alone).
    pub(super) fn is_typed(&self, at: usize) -> bool {
        self.typed.contains(&at)
    }
}
