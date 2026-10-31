//! Route handlers written as a method value, and registration patterns that
//! carry their method (S-526, [CR-172], [FR-FW-03], [NFR-RA-05]).
//!
//! # A method-value handler binds through the registering method's receiver
//!
//! Gin and `net/http` services register `r.GET("/pull", s.PullHandler)` inside
//! a method of the type that owns the handler. The handler's written text is
//! the selector `s.PullHandler`, canonicalised to `s::PullHandler` — a path no
//! scope names, so it never bound. The dialect's query captures each method's
//! receiver name and range (`@fw.receiver.name` / `@fw.receiver.scope`); a
//! handler whose head is the name of the receiver of the method the
//! registration sits in is a call on that method's own instance, and binds
//! exactly as the call `s.PullHandler()` does there — `Self::PullHandler` from
//! the method, among the callables recorded with its self type
//! ([`binder::Index::with_self_types`], S-509). The type is the one the
//! declaration proves; nothing is looked up by name alone.
//!
//! Any other head — a parameter, a local, a field chain (`s.deps.Pull`) — names
//! a value whose type this pass does not prove. The route keeps its node, gets
//! no edge, and the registration is counted
//! ([`FrameworkStats::handlers_receiver_unproven`]); a head that is an imported
//! package (`http.NotFound`) is a package's function, not a value, and binds
//! through its import as it always did.
//!
//! Known gap, the receiver-shape marker's own (S-517): the match is the head's
//! *text* equalling the receiver's name, so a name the method rebinds (`s :=
//! …`, a closure parameter) still reads as the receiver.
//!
//! # A leading verb in a pattern is the route's method
//!
//! Go 1.22's `ServeMux` reads `"GET /api/status"` as the method `GET` and the
//! path `/api/status`. An all-verbs registration (`Handle`, `HandleFunc`) that
//! writes a standard verb, upper-cased, and a space or tab before the rest is
//! [`split_pattern_verb`]-split into that method and path, so its route is
//! named `GET /api/status` and not `ANY GET /api/status`. A dialect opts in by
//! capturing its paths `@fw.route.pattern`; no other path is ever split.
//!
//! [CR-172]: ../../../../docs/requests/CR-172-go-resolves-as-packages-with-its-types-routes-and-interfaces.md
//! [FR-FW-03]: ../../../../docs/specs/requirements/FR-FW-03.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [`FrameworkStats::handlers_receiver_unproven`]: crate::models::pipeline::FrameworkStats::handlers_receiver_unproven

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::config::BindingPolicy;
use crate::graph_store::{NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::{bind_text, binder, text, FileMatches, RouteMatch};
use crate::resolve::route_method::WILDCARD_METHOD;

/// The standard HTTP methods a pattern's leading token may name. Case-sensitive
/// and closed: `GETX` and `get` are path text, as they were before the split.
const PATTERN_VERBS: [&str; 9] = [
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "CONNECT", "OPTIONS", "TRACE",
];

/// One method's receiver binding and the lines of the declaration it governs,
/// exactly as one query match declared it (`@fw.receiver.name` and
/// `@fw.receiver.scope`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReceiverScope {
    /// First byte of the method declaration.
    start: usize,
    /// One past the last byte of the method declaration.
    end: usize,
    /// 1-based first line of the declaration — how its graph node is found.
    start_line: u32,
    /// 1-based last line of the declaration.
    end_line: u32,
    /// The identifier the declaration binds its own instance to.
    name: String,
}

impl ReceiverScope {
    /// The scope a match's `scope` and `name` captures declare.
    pub(super) fn capture(scope: Node<'_>, name: Node<'_>, src: &[u8]) -> Self {
        ReceiverScope {
            start: scope.start_byte(),
            end: scope.end_byte(),
            start_line: scope.start_position().row as u32 + 1,
            end_line: scope.end_position().row as u32 + 1,
            name: text(name, src).trim().to_string(),
        }
    }
}

/// The separators `ServeMux` accepts between a pattern's method and the rest.
const PATTERN_BLANKS: [char; 2] = [' ', '\t'];

/// The method and path a registration `pattern` states, given the `method` its
/// registering call maps to: an all-verbs registration (`ANY`) whose pattern
/// opens with a standard verb and a space or tab is that verb and the rest, and
/// every other registration is its own method and verbatim path. `is_pattern`
/// is the dialect's declaration that its paths are `ServeMux` patterns
/// (`@fw.route.pattern`); a path of any other dialect is never split.
pub(super) fn split_pattern_verb(method: &str, pattern: &str, is_pattern: bool) -> (String, String) {
    if is_pattern && method == WILDCARD_METHOD {
        if let Some((verb, rest)) = pattern.split_once(PATTERN_BLANKS) {
            let rest = rest.trim_start_matches(PATTERN_BLANKS);
            if PATTERN_VERBS.contains(&verb) && !rest.is_empty() {
                return (verb.to_string(), rest.to_string());
            }
        }
    }
    (method.to_string(), pattern.to_string())
}

/// What binding one route's handler text came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Link {
    /// The handler is this callable.
    Bound(NodeId),
    /// The handler binds to nothing the graph proves.
    Unbound,
    /// The handler is a method value on a receiver this pass does not prove —
    /// unbound, and counted.
    ReceiverUnproven,
}

/// The names each file imports, by file id — what separates a handler written
/// through a package (`http.NotFound`) from one written through a value.
///
/// A ledger row names the import's path, not the package's name, and Go lets the
/// two differ: the name is the path's last element, unless that is a major
/// version (`echo/v4` → `echo`), carries a version suffix (`yaml.v3` → `yaml`)
/// or is spelt `go-bar` / `bar-go` (→ `bar`). Each of those readings is taken
/// as imported, beside the row's alias. The names are the count's heuristic
/// only: a package named otherwise reads as a value, and is counted.
fn imported_names(refs: &[UnresolvedRefRow]) -> HashMap<i64, HashSet<&str>> {
    let mut imported: HashMap<i64, HashSet<&str>> = HashMap::new();
    for r in refs.iter().filter(|r| r.kind == EdgeKind::Imports) {
        let Some(file_id) = r.file_id else { continue };
        let names = imported.entry(file_id).or_default();
        names.extend(r.alias.as_deref());
        let segments: Vec<&str> = r.target.split("::").collect();
        let last = segments.last().copied().unwrap_or_default();
        let is_version = last.strip_prefix('v').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        let named = if is_version && segments.len() > 1 { segments[segments.len() - 2] } else { last };
        names.extend([
            Some(named),
            named.split('.').next(),
            named.strip_prefix("go-"),
            named.strip_suffix("-go"),
        ].into_iter().flatten().filter(|n| !n.is_empty()));
    }
    imported
}

/// Binds the handler of each route of a file, with the receiver-aware rule
/// above in front of the plain path bind.
pub(super) struct HandlerBinder<'a> {
    index: &'a binder::Index,
    policy: BindingPolicy,
    by_file_name: &'a HashMap<(&'a str, &'a str), Vec<(NodeId, NodeKind)>>,
    /// Every method node by its file and line range — the node a
    /// [`ReceiverScope`] governs; `None` where two methods share the range
    /// (declarations written on one line), which names no method at all.
    methods_at: HashMap<(&'a str, i64, i64), Option<&'a NodeRow>>,
    imported: HashMap<i64, HashSet<&'a str>>,
    /// Every function and method node: what a typed handler may bind to.
    callables: HashSet<NodeId>,
}

impl<'a> HandlerBinder<'a> {
    pub(super) fn new(
        index: &'a binder::Index,
        policy: BindingPolicy,
        by_file_name: &'a HashMap<(&'a str, &'a str), Vec<(NodeId, NodeKind)>>,
        nodes: &'a [NodeRow],
        refs: &'a [UnresolvedRefRow],
    ) -> Self {
        let mut methods_at: HashMap<_, Option<&NodeRow>> = HashMap::new();
        let methods = nodes.iter().filter(|n| n.kind == NodeKind::Method);
        for (key, method) in methods.filter_map(|n| Some(((n.file_path.as_deref()?, n.start_line?, n.end_line?), n))) {
            methods_at.entry(key).and_modify(|slot| *slot = None).or_insert(Some(method));
        }
        HandlerBinder {
            index,
            policy,
            by_file_name,
            methods_at,
            imported: imported_names(refs),
            callables: nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
                .map(|n| n.id)
                .collect(),
        }
    }

    /// Bind `route`'s handler, written in the file `rel` (id `file_id`, module
    /// node `module`) whose scan is `matches`. A route with no handler is
    /// [`Link::Unbound`].
    pub(super) fn link(
        &self,
        module: &NodeRow,
        file_id: i64,
        rel: &str,
        route: &RouteMatch,
        matches: &FileMatches,
    ) -> Link {
        let Some(handler) = route.handler.as_deref() else {
            return Link::Unbound;
        };
        // A `controller#action` handler names a class's instance method
        // (S-567): bound through the constant index, never as a call.
        if matches.actions.is_named(route.at) {
            return self.bind_action(module, handler);
        }
        let value = matches
            .method_value_handlers
            .then(|| handler.split_once("::"))
            .flatten();
        if let Some((head, rest)) = value {
            let own = matches
                .receivers
                .iter()
                .find(|r| r.start <= route.at && route.at < r.end && r.name == head);
            if let (Some(scope), false) = (own, rest.contains("::")) {
                return self.bind_own(scope, file_id, rel, rest);
            }
        }
        // A handler written on a typed value is a reference to the type's
        // instance member, which a call on the type would not reach (a Kotlin
        // type with a companion reads the companion alone): it binds as a
        // reference, and only to a callable.
        let typed = matches.nest.is_typed(route.at);
        let target = bind_text(
            self.index,
            module,
            file_id,
            handler,
            if typed { EdgeKind::References } else { EdgeKind::Calls },
            self.policy,
            rel,
            self.by_file_name,
        )
        .filter(|id| !typed || self.callables.contains(id));
        match (target, value) {
            (Some(target), _) => Link::Bound(target),
            (None, Some((head, _))) if !self.imports(file_id, head) => Link::ReceiverUnproven,
            (None, _) => Link::Unbound,
        }
    }

    /// `rest` called on the instance of the method `scope` governs: the
    /// `Self::rest` its own call records, bound from that method.
    fn bind_own(&self, scope: &ReceiverScope, file_id: i64, rel: &str, rest: &str) -> Link {
        let key = (rel, i64::from(scope.start_line), i64::from(scope.end_line));
        let Some(Some(method)) = self.methods_at.get(&key) else {
            return Link::Unbound;
        };
        let target = format!("{}::{rest}", binder::SELF_TYPE_HEAD);
        bind_text(
            self.index,
            method,
            file_id,
            &target,
            EdgeKind::Calls,
            self.policy,
            rel,
            self.by_file_name,
        )
        .map_or(Link::Unbound, Link::Bound)
    }

    /// `handler`, read as `Constant::action` by [`super::action`], bound to the
    /// one instance method `action` of the class that constant names.
    fn bind_action(&self, module: &NodeRow, handler: &str) -> Link {
        let segments: Vec<String> = handler.split("::").map(str::to_string).collect();
        let Some((action, constant)) = segments.split_last() else {
            return Link::Unbound;
        };
        binder::bind_constant_action(self.index, module.id, constant, action, self.policy)
            .map_or(Link::Unbound, Link::Bound)
    }

    fn imports(&self, file_id: i64, name: &str) -> bool {
        self.imported.get(&file_id).is_some_and(|names| names.contains(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standard_verb_and_whitespace_open_a_pattern_of_an_all_verbs_registration() {
        let split = |method: &str, pattern: &str| split_pattern_verb(method, pattern, true);
        let owned = |m: &str, p: &str| (m.to_string(), p.to_string());
        assert_eq!(split("ANY", "GET /api/status"), owned("GET", "/api/status"));
        assert_eq!(split("ANY", "DELETE \t /a/{id}"), owned("DELETE", "/a/{id}"));
        assert_eq!(split("ANY", "GET example.com/a"), owned("GET", "example.com/a"));
        for verb in PATTERN_VERBS {
            assert_eq!(split("ANY", &format!("{verb} /a")), owned(verb, "/a"), "{verb}");
        }
        assert_eq!(split("ANY", "PUT\t/tab"), owned("PUT", "/tab"));
        // Near misses: not a standard verb, not upper-case, nothing after it,
        // no whitespace between, or a path that merely starts like one.
        assert_eq!(split("ANY", "GETX /a"), owned("ANY", "GETX /a"));
        assert_eq!(split("ANY", "get /a"), owned("ANY", "get /a"));
        assert_eq!(split("ANY", "GET"), owned("ANY", "GET"));
        assert_eq!(split("ANY", "GET   "), owned("ANY", "GET   "));
        assert_eq!(split("ANY", "GET/a"), owned("ANY", "GET/a"));
        // ServeMux separates on a space or tab only: a no-break space or a
        // newline is path text.
        assert_eq!(split("ANY", "GET\u{a0}/a"), owned("ANY", "GET\u{a0}/a"));
        assert_eq!(split("ANY", "GET\n/p"), owned("ANY", "GET\n/p"));
        assert_eq!(split("ANY", "/GET /a"), owned("ANY", "/GET /a"));
        // A dialect that does not declare its paths patterns never splits one.
        assert_eq!(split_pattern_verb("ANY", "GET /a", false), owned("ANY", "GET /a"));
        // A registration that names its own verb keeps it.
        assert_eq!(split("POST", "GET /a"), owned("POST", "GET /a"));
    }
}
