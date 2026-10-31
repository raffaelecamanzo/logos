//! How one declaration completes another (S-577, [FR-EX-29]): an out-of-line
//! member definition merges into the in-class declaration it defines, a
//! prototype pairs with the definition of its scope, and a callable written in a
//! class body is a method whatever pattern named it. A class pattern's
//! declaration may likewise be marked an enumeration (S-559).
//!
//! Which declarations take part is read off captures a plugin's own `symbols`
//! query declares ([NFR-MA-01]): a plugin that declares none of them extracts
//! exactly what it did before. Two readings are C-family by construction, as
//! [`super::lift_to_declaration`] is: comparing two callables' parameters
//! ([`same_parameters`]) reads the C/C++ declarator, parameter and qualifier
//! node kinds, and a scope path ([`scope_path`]) is split on `::` with
//! `<…>` template arguments dropped. On any other grammar the comparison finds
//! no function declarator and answers "different", so an overloaded name is
//! never merged or paired there — it fails closed, never fabricating a match
//! ([NFR-RA-05]).
//!
//! # The `symbols`-query vocabulary (the `decl.` group)
//!
//! - `@decl.method` — a declaration written in a class body. A callable the
//!   query names as a function (a C++ constructor or template member, whose
//!   declarator is a plain identifier) is recorded as a [`NodeKind::Method`].
//!   Function and method share one descriptor, so its symbol is unchanged.
//! - `@decl.member` — the name of a member function's declarator (a C++
//!   `field_identifier`): its declaration is a method when a class body holds
//!   it at any depth, through any nesting of `#if`/`#elif`/`#else`, and stays a
//!   function otherwise — a header torn by a macro the grammar cannot see
//!   strands `UV_EXTERN int f(…);` outside every class body (S-582).
//! - `@decl.enum` — an enumeration a plugin's class pattern also names (Kotlin's
//!   `enum class` is a `class_declaration`, written with an `enum` modifier). A
//!   class-kinded declaration so marked is a [`NodeKind::Enum`]; both kinds
//!   render a type descriptor, so its symbol is unchanged. A marker, not a
//!   second pattern naming the declaration, because two patterns naming one
//!   declaration are taken first-wins and which comes first is not the
//!   query's to say.
//! - `@decl.prototype` — a bodiless declaration that a definition elsewhere
//!   completes: an in-class member prototype, or a free-function prototype.
//! - `@decl.scope` — the qualified name of an out-of-line definition
//!   (`ns::Cls::m`). Its text up to the last `::` is the scope path the
//!   definition belongs to, template arguments dropped (`B<T>::f` → `B`).
//!   A qualified name that error recovery built — a macro before a
//!   constructor read as `MACRO::name` across a MISSING `::` — names no
//!   scope: the definition is an artifact of the parse error and is not
//!   extracted ([FR-EX-30]), as it was not before the vocabulary existed.
//!
//! # What completion does ([`complete`])
//!
//! 1. **Members are methods, enumerations enums.** A function-kinded
//!    declaration marked `@decl.method` becomes a method, and a class-kinded
//!    one marked `@decl.enum` an enum.
//! 2. **An out-of-line definition finds its container.** Its scope path is
//!    walked through the file's classes, structs and namespaces, from its own
//!    lexical scope outwards, as C++ name lookup reads it. Found, the
//!    definition belongs to that container: it **merges** into the one
//!    prototype of its name there (the prototype whose parameters match, when
//!    the name is overloaded), so the in-class declaration's node — its
//!    symbol unchanged — now carries the definition's body. With no such
//!    prototype it is a member of the container in its own right. Not
//!    found — the class is
//!    declared in another file, the header-and-source split — the definition
//!    keeps its lexical scope and renders its scope path into its own symbol
//!    (`a.cpp/Cls#m().`), a method of a class this file does not declare.
//!    Merging it into the header's node needs the include map
//!    ([FR-RS-38]) and a node one file cannot own ([BR-74]), so it is not done
//!    here.
//! 3. **A prototype pairs with its definition.** A prototype and a bodied
//!    declaration of one scope, name and parameters are a pair. Both keep
//!    their nodes and symbols. The prototype binds nothing — its node is
//!    recorded as a signature (`NodeFact::signature`), the bodyless
//!    declaration [`crate::resolve`] leaves out of every candidate set, so a
//!    call binds the bodied node exactly once. Only the persisted marker is
//!    set: the `@item.signature` facts a Rust signature also carries (whether
//!    it takes `self`) are not recorded for a prototype. It records a `References` link to its
//!    definition, and the definition is emitted first, so the lowest-id rule
//!    a bare-name lookup ends on ([FR-NV-15]) lands on the body.
//!
//! # Identity
//!
//! Every symbol a declaration had before the vocabulary existed is unchanged
//! ([ADR-07]). An out-of-line definition is new; a merged one renders its
//! prototype's symbol, and an unmerged one is numbered after every sibling that
//! is not an out-of-line definition ([`super::assign_ordinals`]) — one
//! rendering its scope path, only among the definitions of that path. A
//! declaration nested in an out-of-line body (a local class, a macro the
//! grammar reads as a prototype) keeps the scope it had before the body was a
//! declaration.
//!
//! [FR-EX-29]: ../../../docs/specs/requirements/FR-EX-29.md
//! [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
//! [FR-RS-38]: ../../../docs/specs/requirements/FR-RS-38.md
//! [FR-NV-15]: ../../../docs/specs/requirements/FR-NV-15.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
//! [BR-74]: ../../../docs/specs/software-spec.md#35-extraction--symbol-ids

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::model::{EdgeKind, LogosSymbol, NodeKind};

use super::arity::owning_decl;
use super::shape::callable_body;
use super::symbol::{descriptor_family, DescriptorFamily};
use super::{Decl, EdgeFact, NodeFact};

/// The capture-name prefix of the vocabulary.
const GROUP: &str = "decl.";

/// What the vocabulary says about one declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DeclDefinitionFacts {
    /// Written in a class body (`@decl.method`).
    method: bool,
    /// An enumeration its class pattern also names (`@decl.enum`).
    enumeration: bool,
    /// A prototype a definition may complete (`@decl.prototype`).
    prototype: bool,
    /// The scope path of an out-of-line definition (`@decl.scope`).
    scope: Option<Vec<String>>,
    /// A prototype paired with a definition of its file: emitted as a
    /// signature (`NodeFact::signature`), so it binds nothing.
    pub(super) paired: bool,
    /// An out-of-line definition whose qualified name error recovery built:
    /// `super::collect_decls` drops it before any parent is assigned.
    pub(super) artifact: bool,
}

impl DeclDefinitionFacts {
    /// Whether the declaration is an out-of-line definition.
    pub(super) fn out_of_line(&self) -> bool {
        self.scope.is_some()
    }
}

/// Whether a class body (`field_declaration_list`) encloses `node`. One that
/// error recovery tore its declaration out of (a header whose macros the
/// grammar cannot see) is under an ERROR region and no body (S-582).
fn in_class_body(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if n.kind() == "field_declaration_list" {
            return true;
        }
        ancestor = n.parent();
    }
    false
}

/// The `decl.` captures of one file, gathered during the declaration walk
/// ([`note`](Self::note)) and resolved once it ends.
#[derive(Default)]
pub(super) struct DefinitionCaptures<'tree> {
    methods: HashSet<usize>,
    enums: HashSet<usize>,
    prototypes: HashSet<usize>,
    scopes: Vec<Node<'tree>>,
}

impl<'tree> DefinitionCaptures<'tree> {
    /// Record `capture` when it belongs to the vocabulary; `false` for any
    /// other capture, which the caller then reads itself.
    pub(super) fn note(&mut self, capture: &str, node: Node<'tree>) -> bool {
        let Some(name) = capture.strip_prefix(GROUP) else {
            return false;
        };
        match name {
            "method" => {
                self.methods.insert(node.id());
            }
            // A member function's declarator name: its declaration is a
            // method when a class body holds it at any depth — through any
            // nesting of `#if`/`#elif`/`#else` — and a function otherwise.
            "member" => {
                if in_class_body(node) {
                    self.methods.insert(super::lift_to_declaration(node).node.id());
                }
            }
            "enum" => {
                self.enums.insert(node.id());
            }
            "prototype" => {
                self.prototypes.insert(node.id());
            }
            "scope" => self.scopes.push(node),
            _ => {}
        }
        true
    }

    /// The facts of each declaration, by index into `decls`. A marker names
    /// the declaration node itself; a scope names the qualified declarator,
    /// owned by the definition it sits in.
    pub(super) fn per_decl(&self, decls: &[Decl<'tree>], source: &[u8]) -> Vec<DeclDefinitionFacts> {
        let mut out: Vec<DeclDefinitionFacts> = decls
            .iter()
            .map(|d| DeclDefinitionFacts {
                method: self.methods.contains(&d.node.id()),
                enumeration: self.enums.contains(&d.node.id()),
                prototype: self.prototypes.contains(&d.node.id()),
                scope: None,
                paired: false,
                artifact: false,
            })
            .collect();
        let decl_at: HashMap<usize, usize> = decls.iter().enumerate().map(|(i, d)| (d.node.id(), i)).collect();
        for &node in &self.scopes {
            let (Some(i), Ok(text)) = (owning_decl(node, &decl_at), node.utf8_text(source)) else {
                continue;
            };
            let path = scope_path(text);
            out[i].artifact = node.has_error() || path.is_empty();
            out[i].scope = Some(path);
        }
        out
    }
}

/// The scope path of a qualified name: its `::` segments but the last,
/// template arguments dropped and blanks skipped (`ns::B<std::string>::f` →
/// `["ns", "B"]`, `::f` → `[]`).
fn scope_path(qualified: &str) -> Vec<String> {
    let mut bare = String::with_capacity(qualified.len());
    let mut depth = 0usize;
    for c in qualified.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 && !c.is_whitespace() => bare.push(c),
            _ => {}
        }
    }
    let mut segments: Vec<String> = bare.split("::").map(str::to_string).collect();
    segments.pop();
    segments.retain(|s| !s.is_empty());
    segments
}

/// What [`complete`] decided for the file, by index into the declarations.
#[derive(Debug, Default)]
pub(super) struct Completion {
    /// The prototype/definition pairs, `(prototype, definition)`.
    pairs: Vec<(usize, usize)>,
    /// Each merged declaration's out-of-line definition, whose body its node
    /// carries: `declaration → definition`.
    pub(super) bodies: HashMap<usize, usize>,
}

impl Completion {
    /// Push each pair's link, a `References` edge from the prototype to its
    /// definition, onto `edges`, and return the pairs by symbol for
    /// [`hoist_definitions`]. Both nodes and symbols stay; the prototype binds
    /// nothing, and the definition is the node a bare-name lookup ends on.
    pub(super) fn link_pairs(
        &self,
        symbols: &[Option<LogosSymbol>],
        edges: &mut Vec<EdgeFact>,
    ) -> Vec<(LogosSymbol, LogosSymbol)> {
        let paired: Vec<(LogosSymbol, LogosSymbol)> = self
            .pairs
            .iter()
            .filter_map(|&(p, f)| Some((symbols[p].clone()?, symbols[f].clone()?)))
            .collect();
        for (prototype, definition) in &paired {
            edges.push(EdgeFact {
                source: prototype.clone(),
                target: definition.clone(),
                kind: EdgeKind::References,
            });
        }
        paired
    }
}

/// Complete the file's declarations (see the module docs): run after
/// [`super::assign_parents`] and before [`super::assign_ordinals`], which
/// numbers what this leaves in place.
pub(super) fn complete(decls: &mut [Decl<'_>], source: &[u8], body_kinds: &[String]) -> Completion {
    for decl in decls.iter_mut() {
        if decl.def.method && decl.kind == NodeKind::Function {
            decl.kind = NodeKind::Method;
        }
        if decl.def.enumeration && decl.kind == NodeKind::Class {
            decl.kind = NodeKind::Enum;
        }
    }

    // A declaration nested in an out-of-line body keeps the scope it had
    // before the body was captured, so its symbol is unchanged. Done before
    // any definition moves to its container: the body's lexical scope is the
    // one the nested declaration had.
    for i in 0..decls.len() {
        while let Some(p) = decls[i].parent.filter(|&p| decls[p].def.out_of_line()) {
            decls[i].parent = decls[p].parent;
        }
    }

    let mut scopes = Scopes::of(decls);
    let mut bodies: HashMap<usize, usize> = HashMap::new();
    let out_of_line: Vec<usize> = (0..decls.len()).filter(|&i| decls[i].def.out_of_line()).collect();
    for d in out_of_line {
        let path = decls[d].def.scope.clone().unwrap_or_default();
        let Some(container) = scopes.resolve(decls, d, &path) else {
            decls[d].qualifier = path;
            continue;
        };
        decls[d].parent = Some(container);
        scopes.parent[d] = Some(container);
        if decls[container].kind == NodeKind::Module {
            decls[d].kind = NodeKind::Function;
        }
        if let Some(p) = merge_target(decls, &scopes, d, container, &bodies, source) {
            bodies.insert(p, d);
            decls[d].completes = Some(p);
        }
    }

    let pairs = pairs(decls, &scopes, &bodies, source, body_kinds);
    for &(prototype, _) in &pairs {
        decls[prototype].def.paired = true;
    }
    Completion { pairs, bodies }
}

/// `true` for a kind an out-of-line scope path walks through.
fn is_container(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Class | NodeKind::Struct | NodeKind::Module)
}

/// The file's scopes as C++ name lookup reads them: a namespace reopened in a
/// second block (`namespace ns {…} namespace ns {…}`) is one scope with the
/// block that opened it first, so a definition in one block finds — and merges
/// or pairs with — a declaration in another. Every other declaration is its own
/// scope. Only the lookup sees this; each declaration keeps its own parent and
/// symbol.
struct Scopes {
    /// Each declaration's parent, a reopened namespace read as its first block.
    parent: Vec<Option<usize>>,
    /// The declarations of each `(scope, name)`, in source order.
    members: HashMap<(Option<usize>, String), Vec<usize>>,
}

impl Scopes {
    fn of(decls: &[Decl<'_>]) -> Self {
        let mut order: Vec<usize> = (0..decls.len()).collect();
        // A scope starts before what it holds, so each parent is read first.
        order.sort_by_key(|&i| decls[i].start_byte);
        let mut scope: Vec<usize> = (0..decls.len()).collect();
        let mut parent: Vec<Option<usize>> = vec![None; decls.len()];
        let mut members: HashMap<(Option<usize>, String), Vec<usize>> = HashMap::new();
        for i in order {
            parent[i] = decls[i].parent.map(|p| scope[p]);
            let named = members.entry((parent[i], decls[i].name.clone())).or_default();
            if decls[i].kind == NodeKind::Module {
                if let Some(&first) = named.iter().find(|&&j| decls[j].kind == NodeKind::Module) {
                    scope[i] = first;
                }
            }
            named.push(i);
        }
        Self { parent, members }
    }

    /// The declarations named `name` in `scope`.
    fn named<'s>(&'s self, scope: Option<usize>, name: &str) -> impl Iterator<Item = usize> + 's {
        self.members
            .get(&(scope, name.to_string()))
            .into_iter()
            .flatten()
            .copied()
    }

    /// The container `path` names from definition `d`'s lexical scope: walked
    /// from each enclosing scope outwards, the first walk that finds every
    /// segment wins. At each step the earliest same-named container is taken.
    fn resolve(&self, decls: &[Decl<'_>], d: usize, path: &[String]) -> Option<usize> {
        let walk = |from: Option<usize>| -> Option<usize> {
            let mut at = from;
            for segment in path {
                at = Some(
                    self.named(at, segment)
                        .find(|&j| is_container(decls[j].kind) && !decls[j].def.out_of_line())?,
                );
            }
            at
        };
        let mut scope = self.parent[d];
        loop {
            if let Some(found) = walk(scope) {
                return Some(found);
            }
            scope = self.parent[scope?];
        }
    }
}

/// The prototype in `container` definition `d` merges into: the one unclaimed
/// prototype of its name, or — the name overloaded — the one whose parameters
/// match.
fn merge_target(
    decls: &[Decl<'_>],
    scopes: &Scopes,
    d: usize,
    container: usize,
    bodies: &HashMap<usize, usize>,
    source: &[u8],
) -> Option<usize> {
    let candidates: Vec<usize> = scopes
        .named(Some(container), &decls[d].name)
        .filter(|&j| {
            decls[j].def.prototype
                && !decls[j].def.out_of_line()
                && descriptor_family(decls[j].kind) == DescriptorFamily::Method
                && !bodies.contains_key(&j)
        })
        .collect();
    match candidates.as_slice() {
        [only] => Some(*only),
        [] => None,
        _ => only_one(candidates.into_iter().filter(|&j| same_parameters(decls[j].node, decls[d].node, source))),
    }
}

/// Each prototype paired with the one bodied declaration of its scope and
/// name whose parameters match it.
fn pairs(
    decls: &[Decl<'_>],
    scopes: &Scopes,
    bodies: &HashMap<usize, usize>,
    source: &[u8],
    body_kinds: &[String],
) -> Vec<(usize, usize)> {
    let callable = |i: usize| {
        decls[i].completes.is_none()
            && decls[i].qualifier.is_empty()
            && descriptor_family(decls[i].kind) == DescriptorFamily::Method
    };
    let bodied = |i: usize| callable(i) && callable_body(decls[i].node, body_kinds).is_some();
    (0..decls.len())
        .filter(|&p| callable(p) && decls[p].def.prototype && !bodies.contains_key(&p))
        .filter(|&p| callable_body(decls[p].node, body_kinds).is_none())
        .filter_map(|p| {
            let definition = only_one(scopes.named(scopes.parent[p], &decls[p].name).filter(|&f| {
                // A definition moved to its class is listed where it was written.
                bodied(f) && scopes.parent[f] == scopes.parent[p] && same_parameters(decls[p].node, decls[f].node, source)
            }))?;
            Some((p, definition))
        })
        .collect()
}

/// The one item of `items`, `None` for none or several.
fn only_one(mut items: impl Iterator<Item = usize>) -> Option<usize> {
    let first = items.next()?;
    items.next().is_none().then_some(first)
}

/// `true` when two callables declare the same parameters and qualifiers.
fn same_parameters(a: Node<'_>, b: Node<'_>, source: &[u8]) -> bool {
    match (parameter_shape(a, source), parameter_shape(b, source)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// A callable's parameters as types alone — each parameter's text without its
/// name, its default value, a top-level `const` (not part of the signature:
/// `f(int)` and `f(const int)`, `f(FILE*)` and `f(FILE* const)` each declare one
/// function) or whitespace — then the declarator's own `const` and reference
/// qualifiers. `(void)` declares what `()` does.
fn parameter_shape(decl: Node<'_>, source: &[u8]) -> Option<String> {
    let declarator = declarator_of_kind(decl, "function_declarator")?;
    let list = declarator.child_by_field_name("parameters")?;
    let mut shape = String::new();
    let mut cursor = list.walk();
    for param in list.named_children(&mut cursor).filter(|p| !p.is_extra()) {
        let declarator = param.child_by_field_name("declarator");
        // The text ends with the declarator, or — an unnamed parameter — before
        // its default value's `=` (`int = 0`).
        let mut children = param.walk();
        let default_at = param.children(&mut children).find(|c| c.kind() == "=").map(|c| c.start_byte());
        let end = declarator.map_or(default_at.unwrap_or(param.end_byte()), |d| d.end_byte());
        let name = declarator_of_kind(param, "identifier");
        let mut skipped: Vec<std::ops::Range<usize>> = name.map(|n| n.byte_range()).into_iter().collect();
        // The qualifiers of the parameter itself: on the declaration for a
        // by-value parameter, named (`const int n`) or not (`const int`), and
        // after the `*` closest to the name for a pointer.
        let holder = match name {
            Some(name) => name.parent().filter(|p| *p == param || p.kind() == "pointer_declarator"),
            None => declarator.is_none().then_some(param),
        };
        if let Some(holder) = holder {
            let mut children = holder.walk();
            skipped.extend(
                holder
                    .children(&mut children)
                    .filter(|c| c.kind() == "type_qualifier")
                    .map(|c| c.byte_range()),
            );
        }
        let text: String = (param.start_byte()..end)
            .filter(|b| !skipped.iter().any(|r| r.contains(b)))
            .map(|b| source[b])
            .filter(|b| !b.is_ascii_whitespace())
            .map(char::from)
            .collect();
        shape.push_str(&text);
        shape.push(',');
    }
    if shape == "void," {
        shape.clear();
    }
    let mut cursor = declarator.walk();
    for qualifier in declarator
        .children(&mut cursor)
        .filter(|c| matches!(c.kind(), "type_qualifier" | "ref_qualifier"))
    {
        shape.push_str(qualifier.utf8_text(source).ok()?);
    }
    Some(shape)
}

/// The first node of `kind` down a declaration's `declarator` chain, through
/// its pointer and reference wrappers: a callable's `function_declarator`, a
/// parameter's declared `identifier`.
fn declarator_of_kind<'t>(decl: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut at = decl.child_by_field_name("declarator")?;
    for _ in 0..8 {
        if at.kind() == kind {
            return Some(at);
        }
        at = at.child_by_field_name("declarator").or_else(|| at.named_child(0))?;
    }
    None
}

/// Emit each pair's definition immediately before its earliest prototype, so
/// it takes the lower node id a bare-name lookup ends on. Run after
/// [`super::sort_facts`]; every node not a paired definition keeps its place.
pub(super) fn hoist_definitions(nodes: &mut Vec<NodeFact>, pairs: &[(LogosSymbol, LogosSymbol)]) {
    for (prototype, definition) in pairs {
        let at = |nodes: &[NodeFact], symbol: &LogosSymbol| nodes.iter().position(|n| n.symbol == *symbol);
        let (Some(p), Some(f)) = (at(nodes, prototype), at(nodes, definition)) else {
            continue;
        };
        if f > p {
            let node = nodes.remove(f);
            nodes.insert(p, node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scope_path;

    #[test]
    fn a_scope_path_drops_the_name_template_arguments_and_blanks() {
        assert_eq!(scope_path("ns::Cls::m"), ["ns", "Cls"]);
        assert_eq!(scope_path("B<std::map<int, T>>::f"), ["B"]);
        assert_eq!(scope_path("ns :: Cls :: m"), ["ns", "Cls"]);
        assert!(scope_path("::f").is_empty());
        assert!(scope_path("f").is_empty());
    }
}
