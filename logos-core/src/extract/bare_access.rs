//! A bare name that reads a field of the enclosing class (S-556, [CR-177],
//! [FR-EX-22]): C#'s `_count`, `Name` and `_log.Add(x)`, which name the class's
//! own members with no `this.` written — and Scala's `semaphore.acquire(n)` and
//! `rate - n` (S-575, [FR-EX-28]), whose fields include the class's constructor
//! parameters — and Kotlin's `count + x` and `repo.find(id)` (S-562, [FR-EX-25]),
//! whose properties include a primary constructor's `val`s. And a bare name that calls
//! a method (S-568, [FR-RS-35]): Ruby's `to_app` in `to_app.call(env)`, a name the
//! grammar parses as an identifier whether it reads a local or calls a method with no
//! receiver and no argument.
//!
//! The `references` query marks the three, and records no row of its own for
//! any:
//!
//! - `@ref.access.bare` — a name in a value position ([`ACCESS_CAPTURE`]);
//! - `@ref.method.bare` — a name in a value position that, unless a local
//!   hides it, is a call ([`CALL_CAPTURE`]);
//! - `@ref.access.shadow` — a name a method declares: a parameter, a local, a
//!   pattern variable ([`SHADOW_CAPTURE`]).
//!
//! A name is an access when the method sits in a class that **declares** a field
//! of that name itself — so a name that is a type, a constant of another type or
//! a member the class inherits records nothing — and when the method declares no
//! local of the name. The method is read as one scope — `CallShadows` reads a whole
//! file — and that can lose a bare read beside an unrelated local of the same
//! name and never invents one ([NFR-RA-05]). The row is the `Method`-form
//! `Accesses` that `this.x` records, so the binder proves an exactly-one `Field`
//! in the class ([`resolve_member_access`]) and an ambiguous name stays unbound.
//!
//! A call is a name in a method that the method declares no local of. Its row
//! is the `Method`-form `Calls` of no argument that `name()` records, marked as
//! written with no receiver, so the receiver pass gives it the shape the
//! plugin's implicit receiver says ([`Receivers::mark_implicit`]): in a class,
//! a call on the instance, bound along the class's ancestors.
//!
//! Rows are appended after the walk ([`BareAccesses::finish`]): a shadow may be
//! matched after the read it hides, and an append moves no row the receiver pass
//! addresses by index.
//!
//! [CR-177]: ../../../docs/requests/CR-177-csharp-calls-members-routes-partials-and-typed-receivers-resolve.md
//! [FR-EX-22]: ../../../docs/specs/requirements/FR-EX-22.md
//! [FR-EX-25]: ../../../docs/specs/requirements/FR-EX-25.md
//! [FR-EX-28]: ../../../docs/specs/requirements/FR-EX-28.md
//! [FR-RS-35]: ../../../docs/specs/requirements/FR-RS-35.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [`resolve_member_access`]: crate::resolve
//! [`Receivers::mark_implicit`]: super::receiver::Receivers::mark_implicit

use std::collections::HashSet;

use tree_sitter::Node;

use super::receiver::{outer_class, Receivers};
use super::{access_row, Decl, RefFact};
use crate::model::{EdgeKind, LogosSymbol, NodeKind, RefForm};
use crate::resolve::is_class_like;

/// The `references`-query capture of a bare name in a value position.
pub(super) const ACCESS_CAPTURE: &str = "ref.access.bare";

/// The `references`-query capture of a bare name that calls a method unless a
/// local hides it (S-568).
pub(super) const CALL_CAPTURE: &str = "ref.method.bare";

/// The `references`-query capture of a name a method declares.
pub(super) const SHADOW_CAPTURE: &str = "ref.access.shadow";

/// The bare reads of a file and the names its methods declare.
#[derive(Default)]
pub(super) struct BareAccesses<'tree> {
    /// `(member, name)`: the member of a class (its method) declares `name`.
    shadowed: HashSet<(usize, String)>,
    sites: Vec<Site>,
    calls: Vec<Call<'tree>>,
}

/// One bare name in a method that calls a method unless a local hides it.
struct Call<'tree> {
    node: Node<'tree>,
    source: LogosSymbol,
    owner: usize,
    name: String,
    member: usize,
}

/// One bare read that names a field of the method's class.
struct Site {
    source: LogosSymbol,
    line: u32,
    name: String,
    member: usize,
}

impl<'tree> BareAccesses<'tree> {
    /// Take `node`, the match of `capture`, recording no row yet
    /// ([`finish`](Self::finish) does, once every shadow is known). `owner`
    /// finds the declaration enclosing `node`.
    pub(super) fn note(
        &mut self,
        capture: &str,
        node: Node<'tree>,
        source: &[u8],
        decls: &[Decl<'_>],
        symbols: &[Option<LogosSymbol>],
        owner: impl FnOnce() -> Option<usize>,
    ) {
        match capture {
            SHADOW_CAPTURE => self.shadow(node, source, decls, owner()),
            ACCESS_CAPTURE => {
                if let Some(owner) = owner() {
                    if let Some(symbol) = symbols[owner].clone() {
                        self.site(node, source, decls, owner, symbol);
                    }
                }
            }
            CALL_CAPTURE => {
                if let Some(owner) = owner() {
                    if let Some(symbol) = symbols[owner].clone() {
                        self.call(node, source, decls, owner, symbol);
                    }
                }
            }
            _ => {}
        }
    }

    /// Note a bare name `node` in the callable `owner`, a call unless a local
    /// of the method hides it. Only in a method or function: a name in a class
    /// or module body (`private`, `module_function`) is a statement of the
    /// body, not of a method, and records nothing.
    fn call(&mut self, node: Node<'tree>, source: &[u8], decls: &[Decl<'_>], owner: usize, symbol: LogosSymbol) {
        if !matches!(decls[owner].kind, NodeKind::Method | NodeKind::Function) {
            return;
        }
        let Ok(name) = node.utf8_text(source) else {
            return;
        };
        self.calls.push(Call {
            node,
            source: symbol,
            owner,
            name: name.trim().to_string(),
            member: member_of(decls, owner),
        });
    }

    /// Note that the member holding `owner` declares `node`'s name.
    fn shadow(&mut self, node: Node<'_>, source: &[u8], decls: &[Decl<'_>], owner: Option<usize>) {
        if let (Some(owner), Ok(name)) = (owner, node.utf8_text(source)) {
            self.shadowed
                .insert((member_of(decls, owner), name.trim().to_string()));
        }
    }

    /// Note a bare read of `node`'s name in the callable `owner`, when it
    /// can be an access to a field the callable's class declares.
    fn site(
        &mut self,
        node: Node<'_>,
        source: &[u8],
        decls: &[Decl<'_>],
        owner: usize,
        symbol: LogosSymbol,
    ) {
        if !matches!(decls[owner].kind, NodeKind::Method | NodeKind::Function)
            || names_a_member_of_another(node, source)
            || on_another_receiver(node, source)
        {
            return;
        }
        let Ok(name) = node.utf8_text(source) else {
            return;
        };
        let name = name.trim();
        let Some(class) =
            outer_class(decls, owner).filter(|&class| declares_own_field(decls, class, name))
        else {
            return;
        };
        if is_receiver(node) && type_named_member(decls, class, name, source) {
            return;
        }
        if extension_parameter(node, source, name) {
            return;
        }
        self.sites.push(Site {
            source: symbol,
            line: node.start_position().row as u32 + 1,
            name: name.to_string(),
            member: member_of(decls, owner),
        });
    }

    /// Append a `Method`-form `Accesses` row for every read no local hides,
    /// and a `Method`-form `Calls` row of no argument for every bare call no
    /// local hides — registered with `receivers` as written with no receiver,
    /// so the receiver pass shapes it as it does `name()`.
    pub(super) fn finish(self, out: &mut Vec<RefFact>, mut receivers: Option<&mut Receivers<'tree>>) {
        for site in self.sites {
            let key = (site.member, site.name);
            if self.shadowed.contains(&key) {
                continue;
            }
            out.push(access_row(site.source, key.1, site.line));
        }
        for call in self.calls {
            let key = (call.member, call.name);
            if self.shadowed.contains(&key) {
                continue;
            }
            if let Some(receivers) = receivers.as_deref_mut() {
                receivers.mark_implicit(call.node);
                receivers.site(out.len(), Some(call.node), Some(call.owner));
            }
            out.push(RefFact {
                source: call.source,
                target: key.1,
                alias: None,
                form: RefForm::Method,
                kind: EdgeKind::Calls,
                line: call.node.start_position().row as u32 + 1,
                relation: None,
                receiver: None,
                peeled: None,
                arg_count: Some(0),
                exported: None,
            });
        }
    }
}

/// The declaration directly under the nearest class that holds `at`: the method
/// a nested callable sits in, or `at` itself when its parent is a class.
fn member_of(decls: &[Decl<'_>], mut at: usize) -> usize {
    while let Some(parent) = decls[at].parent {
        if is_class_like(decls[parent].kind) {
            break;
        }
        at = parent;
    }
    at
}

/// Whether an `extension (a: Int)` that encloses `node` names `name` among its
/// parameters (S-575, Scala 3): the receiver of the extension's methods, not the
/// class's field. The extension is no declaration, so its parameters cannot be
/// keyed to the method a shadow is read under ([`member_of`]); the enclosing
/// extension is read here instead.
fn extension_parameter(node: Node<'_>, source: &[u8], name: &str) -> bool {
    let mut at = node.parent();
    while let Some(n) = at {
        if n.kind() == "extension_definition" {
            let mut cursor = n.walk();
            let declares = n.children_by_field_name("parameters", &mut cursor).any(|list| {
                let mut c = list.walk();
                let named = list
                    .named_children(&mut c)
                    .filter_map(|p| p.child_by_field_name("name"))
                    .any(|id| id.utf8_text(source).is_ok_and(|t| t.trim() == name));
                named
            });
            if declares {
                return true;
            }
        }
        at = n.parent();
    }
    false
}

/// Whether `node` is the `X` of `X.Member`.
fn is_receiver(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        parent.kind() == "member_access_expression"
            && parent
                .child_by_field_name("expression")
                .is_some_and(|e| e.id() == node.id())
    })
}

/// Whether the class at `class` declares a member `name` of the type `name`
/// (`Status Status { get; }`), which C# reads as the member or the type by what
/// follows: `Status.Open` is the enum's, and nothing in the syntax says so.
fn type_named_member(decls: &[Decl<'_>], class: usize, name: &str, source: &[u8]) -> bool {
    decls
        .iter()
        .filter(|d| d.parent == Some(class) && d.name == name && d.kind == NodeKind::Field)
        .any(|d| {
            // The type sits on the declaration (a property) or on its
            // parent (a field's `variable_declaration`).
            [Some(d.node), d.node.parent()]
                .into_iter()
                .flatten()
                .find_map(|n| n.child_by_field_name("type"))
                .and_then(|t| t.utf8_text(source).ok())
                .is_some_and(|t| {
                    // `Status`, `Status?`, `Status<T>`, `App.Status`, `global::App.Status`.
                    let head = t.split(['<', '?']).next().unwrap_or(t).trim();
                    head.rsplit(['.', ':']).next() == Some(name)
                })
        })
}

/// Whether `node` sits where the class's instance is not the only receiver in
/// scope (S-562, Kotlin): inside an `object : T { … }` body, whose supertype may
/// declare the name, an extension function's body (`fun G.ext() { … }`), whose
/// `G` may, or the trailing lambda of `x.apply { … }`, `x.run { … }` or
/// `with(x) { … }`, whose receiver `x` may. Which one the name reads depends on a
/// type no row records, so none is claimed ([NFR-RA-05]). The search stops at
/// the nearest class.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn on_another_receiver(node: Node<'_>, source: &[u8]) -> bool {
    let mut at = node.parent();
    while let Some(n) = at {
        match n.kind() {
            "object_literal" => return true,
            "class_declaration" | "object_declaration" => return false,
            "function_declaration" if has_receiver_type(n) => return true,
            "lambda_literal" if is_receiver_lambda(n, source) => return true,
            _ => {}
        }
        at = n.parent();
    }
    false
}

/// Whether the `function_declaration` `function` names a receiver type before
/// its name (`fun String.slug()`); a return type follows the parameters.
fn has_receiver_type(function: Node<'_>) -> bool {
    function
        .child_by_field_name("name")
        .and_then(|name| name.prev_named_sibling())
        .is_some_and(|before| {
            matches!(
                before.kind(),
                "user_type" | "nullable_type" | "parenthesized_type"
            )
        })
}

/// Whether `lambda` is the trailing lambda of a call whose receiver is the
/// lambda's `this`: `x.apply { … }`, `x.run { … }` (the receiver form; a bare
/// `run { … }` has none) and `with(x) { … }`. Other receiver lambdas — a
/// `T.() -> R` parameter, a builder DSL — are not read.
fn is_receiver_lambda(lambda: Node<'_>, source: &[u8]) -> bool {
    let call = lambda
        .parent()
        .filter(|a| a.kind() == "annotated_lambda")
        .and_then(|a| a.parent())
        .filter(|c| c.kind() == "call_expression");
    // `with(x) { … }` is a call of the call `with(x)`, which the lambda follows.
    let callee = call
        .and_then(|c| c.named_child(0))
        .map(|first| match first.kind() {
            "call_expression" => first.named_child(0).unwrap_or(first),
            _ => first,
        });
    let Some(callee) = callee else {
        return false;
    };
    let text = |n: Node<'_>| n.utf8_text(source).map(str::trim).unwrap_or_default();
    match callee.kind() {
        "identifier" => text(callee) == "with",
        "navigation_expression" => callee
            .named_child(callee.named_child_count().saturating_sub(1))
            .is_some_and(|member| matches!(text(member), "apply" | "run")),
        _ => false,
    }
}

/// Whether the class at `class` declares a property `name` of its own — not one
/// declared in the body of an `object : T { … }` or of an enum entry that the
/// query's `class_body` capture also attributes to the class (S-562, Kotlin): a
/// bare `name` in the class's methods is not read from there.
fn declares_own_field(decls: &[Decl<'_>], class: usize, name: &str) -> bool {
    decls.iter().any(|d| {
        d.parent == Some(class)
            && d.name == name
            && d.kind == NodeKind::Field
            && !super::sits_within_any(d.node, decls[class].node, &["object_literal", "enum_entry"])
    })
}

/// Whether `node`, a name the query read in a value position, names a member
/// of **another** object or no value at all, which the query cannot tell:
///
/// - the `nameof(Name)` argument, or the head of its member path
///   (`nameof(Name.Length)`), which names a member without reading it;
/// - the left of an assignment in an object initializer
///   (`new Foo { Name = Name }`), which is a member of the constructed object.
fn names_a_member_of_another(node: Node<'_>, source: &[u8]) -> bool {
    // `nameof(Name.Length)` names the member path as a whole: climb from the
    // leftmost name to the expression it heads.
    let mut head = node;
    while let Some(access) = head.parent().filter(|p| {
        p.kind() == "member_access_expression"
            && p.child_by_field_name("expression")
                .is_some_and(|e| e.id() == head.id())
    }) {
        head = access;
    }
    let Some(parent) = head.parent() else {
        return false;
    };
    match parent.kind() {
        "argument" => parent
            .parent()
            .and_then(|list| list.parent())
            .filter(|call| call.kind() == "invocation_expression")
            .and_then(|call| call.child_by_field_name("function"))
            .is_some_and(|callee| callee.utf8_text(source).is_ok_and(|t| t.trim() == "nameof")),
        "assignment_expression" => {
            parent
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == node.id())
                && parent
                    .parent()
                    .is_some_and(|list| list.kind() == "initializer_expression")
        }
        _ => false,
    }
}
