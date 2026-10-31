//! A receiver call records its receiver's **shape** (S-514, [CR-169],
//! [FR-EX-13]) and, where the file proves it, its receiver's **type** (S-467,
//! [CR-150] §3.2 A).
//!
//! A receiver call `x.send()` is recorded as a Method-form row naming `send`
//! alone. Two passes over the file's rows, run once after every query match,
//! say more about it. Both are driven by **marker** captures in the
//! `references` query (`@ref.receiver.*`): a marker records no row of its own,
//! and a grammar whose query declares none is untouched.
//!
//! **1. Typing (S-467, Java's markers).** Where the FILE proves the receiver's
//! type `T`, the row is retyped in place to a type-qualified PATH-form
//! `T::send`. The binder's package rung (S-465) binds it to `T`'s one own
//! `send`, then up `T`'s in-repository supertypes (S-468). One row per site:
//! the typed row replaces the bare one, never joins it. Path form
//! deliberately: a Method-form target containing `::` is the binder's Rust
//! trait-object dispatch branch (S-281, [FR-RS-08]), which a Java row must
//! never reach.
//!
//! | marker | receiver | `T` |
//! |---|---|---|
//! | `name` | `x.send()` | [`DeclaredTypes::get`]: the one type the file declares `x` with, where `x` is declared in scope at the call (a field of the enclosing class, in the callable around it, or across a `declared` marker's `.scope`); a name the file never declares as a variable is a static type name when its single-type import or its own declaration names that type |
//! | `field` | `this.x.send()` | [`DeclaredTypes::field`] (S-398), where the enclosing class declares the field `x` itself — and, in a path-import language, no `unproven` marker names `x` (S-537: a field declared with a qualified type) |
//! | `this` | `this.send()` | the enclosing class |
//! | `super` | `super.send()` | the enclosing class's `extends`, as S-466's row records it ([`declared_superclass`]) — in a package-model language only; every other language's `super` call keeps its shape and binds through the proven hierarchy (S-522) |
//! | `implicit` | `send()`, on a `@ref.call` row | the enclosing class, when it declares `send` itself, or when nothing else in scope — an outer class, a static import — could supply `send` |
//!
//! Everything else keeps its bare row: a chained call; a name the file also
//! declares where [`DeclaredTypes`] cannot read its type (an untyped lambda
//! parameter, a for-each, catch, pattern or varargs variable, one declared with
//! a qualified type the simple name would lose — the `unproven` marker); a
//! variable declared nowhere in scope at the call (an inherited or outer
//! class's field); a name declared with two disagreeing types; a generic type
//! variable, an array, `var`; `Outer.super.send()` (the `refused` marker),
//! whose receiver is `Outer`'s superclass; and any `this` / `super` / bare call
//! inside an anonymous class body, whose class has no name. `T` is written as
//! the file names it (`Mailer`, or `a::b::Mailer` from a qualified `extends`);
//! its scope — the file's single-type imports, then the same package — is the
//! binder's package rung (S-465), the one FQN derivation there is.
//!
//! `name`, `field`, `this`, `refused` and `unproven` are Java's vocabulary: they
//! prove types through [`DeclaredTypes`], whose declaration reading is the
//! grammar's field names. C# speaks it too (S-558, [FR-RS-30]), with `name`,
//! `field` and `unproven`, and so does Scala (S-576, [FR-RS-37]), whose
//! `val`/`var` names are `declared` (a `pattern:`, which the walk never reads
//! as a name); TypeScript, Python and PHP speak `field`
//! (S-537, [FR-RS-21]; S-546, [FR-RS-24]: `self.x`; S-552, [FR-RS-27]);
//! any other grammar uses the shape markers below until a typing story of its
//! own opts it in. A language opts in through its query alone — the mechanism is
//! this one, and its declarations are read by [`DeclaredTypes::build_with`]:
//!
//! | marker | what it states |
//! |---|---|
//! | `declared` + `.type` | a declaration whose type the companion node names, read in place of the field walk's reading of the name: one the walk misreads (C#'s `var x = new T()`, `T? x`) or never sees (C#'s `foreach (T x in xs)`). The companion is the language's type-name normalisation: the node whose text the simple-name rule reads ([`Declarations`]) |
//! | `declared` + `.scope` | the name is in scope in the type the companion declares, beyond the callable it sits in — a C# primary-constructor parameter, which its type declares no field of — wherever that type is the call's own (never in a nested type) and records no supertype (an inherited member of the name would hide it) |
//! | `declared` + `.field` | the declaration is a field of its class besides, wherever it sits ([`Declarations::field`]): a TypeScript constructor parameter property (`constructor(private svc: Svc)`, S-537), which types `this.svc` as an annotated field does. Any other stated declaration is a field by its position alone, outside every callable |
//! | `declared` + `.untyped` | the declaration names no type a call is typed by (S-537: an unannotated, union or qualified TypeScript field), which refuses its name — unless a `.type` statement names the same node |
//!
//! **A proven-rows language: structural types (S-537, TypeScript; the
//! plugin's `structural_types`) or import roots (S-546, Python; the plugin's
//! `import_roots`).** Its module walk never reaches a class's inherited
//! members — TypeScript's no member at all, Python's a class's own alone
//! (S-544) — so a typed row keeps its `other` shape, as a Rust proven row does
//! (1b): the binder binds it among `T`'s members, up `T`'s proven `Extends`,
//! by the proven-receiver rung, never by a scope. A TypeScript `T` is qualified by the named import the file brought
//! it in through (`..::users::users.service::UsersService`), as a type
//! relation's is (S-535); a Python `T` is kept as written, and the binder reads
//! it through the file's imports as it reads a written qualifier (S-544). A
//! plain name is typed by Rust's `variable` / `binding` / `proof`
//! markers (1b) — the one binding in scope at the call — never by the
//! file-wide [`DeclaredTypes`], which a TypeScript block, arrow or module
//! scope, or a Python rebinding, would defeat. A `this.x` / `self.x` reads the
//! enclosing class's own field declarations, all of which the query states
//! ([`Declarations::stated_only`]), so another class's `x` never answers for
//! it.
//!
//! **1b. Rust typing (S-587, [CR-188], [FR-RS-42]).** Rust's bindings are
//! patterns, which [`DeclaredTypes`] does not read, so Rust declares its own
//! markers, and its query states every binding and its scope. A proven call is
//! retyped to the Path-form `T::send` and **keeps its `other` shape**: it is a
//! call on another object whose type the file proves, not a written
//! `T::send()`, and the binder binds it among `T`'s own methods (S-588). The wrappers
//! peeled off the declared type to reach `T` — `&`, `&mut`, `Box`, `Arc`, `Rc`
//! ([`PEELED_WRAPPERS`]), no other — are recorded on the row
//! (`RefFact::peeled`), outermost first.
//!
//! | marker | receiver | `T` |
//! |---|---|---|
//! | `variable` | `x.send()` | the one `binding` of `x` in scope at the call, when it carries a `proof`: a declared type (`x: T`, `let x: T`), a constructor `let x = T::g(…)`, `T` one segment, whose every `g` declared in the caller's module on an impl naming `T` returns `Self` or `T`, or a struct literal `let x = T { … }` of one segment (`E::V { … }` builds an `E`) |
//! | `self_field` | `self.x.send()` | the declared type of the field `x` of the caller's own struct — the struct the caller's module declares under the caller's self type (S-493) |
//!
//! A `binding` marker's pattern binds every name it spells; its companion
//! `.scope` or `.after` says where (a parameter's callable, a `let`'s block
//! after it), `.local` the innermost callable around its node (a Python
//! local), `.class` a class body that no method of it sees (a Python
//! class-body name, which outranks any other scope of the name), `.member`
//! a class whose own members read the name with no receiver written (a Kotlin
//! property, S-563), and none means the whole callable, which can only refuse
//! more.
//! A shadowed, re-bound or two-typed name — two bindings in scope — proves
//! nothing, and neither does a generic parameter, `impl Trait`, a slice, tuple,
//! pointer or `fn` type, an associated type, a chain, or `Self` outside an impl.
//! `T` is written as the file names it (`Store`, `crate::db::Store`), its
//! generic arguments dropped; `Self` is the caller's self type where its impl
//! names it by one segment (`impl crate::m::S` may name another `S`).
//!
//! TypeScript speaks `variable`, `binding` and `proof` too (S-537): a
//! parameter's binding is scoped to its function, a `catch` or loop variable's
//! to its clause, a `const`/`let`'s to the rest of its block (`.after`), a
//! `var`'s to the whole callable; its proofs are `.type` (an annotation `T`,
//! `T<A>`) and `.literal` (`const x = new T()`), and it peels no wrapper.
//! So does Python (S-546): a parameter's binding is scoped to its function's
//! or lambda's body, never its default values; every other binding form — an assignment, a `for`, `with` or
//! `except` target, a walrus, a `def` or `class`, an import, a `case` pattern
//! — to the innermost function around its statement and the functions nested
//! in it (`.local`; at module level, the calls outside every function), so a
//! sibling function's local is never in scope; and a class body's names to
//! that body alone (`.class`), which no method sees; its proofs are
//! `.type` (an annotation, normalised by the query to the `T` it names) and
//! `.literal` (`x = T(…)`).
//!
//! PHP speaks them too (S-552, [FR-RS-27]): a parameter's binding is scoped to
//! its callable, and every other binding — an assignment, a `foreach`, `catch`,
//! `global` or `static` variable — to the whole callable, as PHP scopes a
//! variable; its proofs are `.type` (`T`, `?T`) and `.literal` (`$x = new
//! T()`). A `variable` receiver is read by its one leaf, as a `binding`'s
//! names are, so `$m` is the `m` its parameter binds. A proven PHP row is the
//! written `T::m` a static call records, with no shape: only Rust's impl-block
//! lookup, a proven-rows language and a companion-paths one have a
//! proven-receiver rung ([`Receivers::impl_blocks`]).
//!
//! Kotlin speaks them too (S-563, [FR-RS-32]): a parameter's binding is scoped
//! to its callable, a lambda, `for`, `catch` or `when` variable's to its
//! construct, a local's to the rest of its block or lambda (`.after`), a
//! top-level property's to its file, and a class property's — a constructor
//! `val`/`var` or a body property — to the members of its class (`.member`,
//! [`Scope::Member`]): only where that class is the call's own, outside every
//! anonymous body and every lambda the `rebinds` marker names (`x.apply { }`),
//! whose own `this` may declare the name. Its proofs are `.type` (`T`, `T?`)
//! and `.literal` (`val x = T()`). A type lends its name to its companion
//! there ([`Receivers::companion_paths`]), so the written `T::m` is the
//! companion's: a proven Kotlin row keeps its `other` shape, and the binder
//! binds it among `T`'s own members. So does a proven Scala row (S-576), typed
//! by Java's `name` and `field` markers: a Scala class lends its name to its
//! companion `object` (S-573), whose methods the written `T::m` reaches.
//!
//! C++ speaks them too (S-581, [FR-RS-39]), with one more receiver marker:
//! `pointee`, the `p` of `p->m()`, proven only by a `.pointer` proof — `T*`,
//! `std::unique_ptr<T>`, `std::shared_ptr<T>`, the companion naming `T` —
//! which in turn proves no `variable`: `p.reset()` is the smart pointer's own
//! member, and `x->m()` on a value its `operator->`. Its `.type` proofs are a
//! declared `T`, `T&`, `const T&`, `T&&`, and no wrapper is peeled: only Rust's
//! impl-block lookup reads a peeled row, so a C++ `Box<T>` is a `Box`. A
//! parameter list sits in the declarator, never around the body, so every
//! binding names its scope: a parameter's or condition variable's function,
//! lambda, `catch` or statement (`.scope`), a local's block or `case` after
//! it (`.after`), a data member's class (`.member`) — and, because a form no
//! pattern scopes would otherwise leave an outer binding the only one in
//! scope, every name a declarator declares is a `hides` marker too: one no
//! `binding` states is bound with no proof across the innermost
//! `hiding_scope` (a function or lambda) around it
//! ([`Receivers::bind_hiders`]). A proven C++ row is the
//! written `T::m` a qualified call records, with no shape, bound through the
//! includes up `T`'s proven bases (`binder::includes`).
//!
//! **1c. A receiver inside a macro (S-610).** A macro's token tree is never
//! parsed as expressions, so the query marks no receiver there. The token-tree
//! walk ([`macro_call_refs`](super::refs::macro_call_refs)) reads the two
//! receivers a proof form can type — `x.f()`'s plain `x` and `self.x.f()`'s
//! field — and hands each to [`Receivers::macro_site`], whose row [`Receivers::finish`]
//! proves by the same `variable` / `self_field` rules, at the macro's own
//! position: no scope boundary lies inside a token tree, so the macro sees the
//! bindings its call sees. A name a pattern inside the macro binds — a closure
//! parameter, a `let` / `for`, a match arm or `matches!` guard — is no typable
//! receiver (`refs::bound_names`); a binding form of a user macro is not seen.
//! A chain, a path, a literal or a call result is no receiver and stays `other`.
//!
//! **2. Shape (S-514, every language).** Every Method-form row typing left
//! bare records one shape from a closed lexicon ([`ReceiverShape`]), which the
//! binder dispatches on ([FR-RS-12]): `self` binds among the caller's own
//! class's members, `super` only through a proven `Extends`, `other` never
//! through the caller's scope. A row whose call no marker names records **no**
//! shape, which the binder treats exactly as `other`.
//!
//! | marker | captured node | the row records |
//! |---|---|---|
//! | `self` | the receiver: `this`, `self`, `$this`, … | `self` |
//! | `super` | the receiver: `super`, `super()`, `parent`, … | `super` |
//! | `other` | the receiver | `other` — or `self` when its text is the `self_name` the caller declares |
//! | `implicit` | the name of a call written with no receiver, on a `@ref.method` row — or a bare name no local binds, its own invocation (Ruby's `to_app`, [`Receivers::mark_implicit`]) | per the plugin's `implicit_receiver` policy ([`ImplicitReceiver`]): `self` inside a named class under `"self"` — unless a callable between the call and that class declares the name (a nested `def`, a local function), which shadows the member; otherwise the row becomes the Path-form free call `@ref.call` records, which the lexical scope binds |
//! | `self_name` | the identifier a declaration binds its own instance to (Go's receiver parameter) | nothing; read by `other` |
//! | `anonymous` | a body whose `self`/`this` is not an instance of the enclosing named class: an anonymous class body (`new T() { … }`, Kotlin `object : T { … }`, Scala `new T { … }`, PHP `new class { … }`), a TS class expression, a TS/JS object-literal method or non-arrow `function`, a Kotlin extension function's body, a Ruby `module`, `def self.x`, `class << self` or `Struct.new` block | nothing; typing stops there, and a `self` / `super` / `this` call inside one records `other` — except a `self` call to a callable that body declares itself, which becomes the free call the lexical scope binds to it |
//!
//! Java's typing markers map onto the lexicon: `this` is `self`; `name`,
//! `field` and `refused` are `other`. A `self` call whose caller declares a
//! self type (S-493, `@symbol.self_type`) is recorded as the Path-form
//! `Self::send` — the target a written `Self::send()` records, bound through
//! that type ([FR-RS-11]) — keeping the shape `self`, which a written one never
//! carries, when the call wrote the `self` keyword (S-606, [`self_call`]); the
//! S-493 capture `@ref.method.self` is a `self`-marked `@ref.method`.
//!
//! **How a marker finds its call.** The captured node's parent is the node the
//! call's `@ref.method` / `@ref.call` capture also has for parent — the member
//! or invocation expression both sit in (`this.send()` → the `this` node and the
//! `send` node share it). For `field` it is the grandparent. When markers
//! disagree: `refused` makes the call `other`; `self` and `super` together make
//! it `other`; either beats `other`, which beats `implicit`.
//!
//! [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
//! [FR-RS-21]: ../../../docs/specs/requirements/FR-RS-21.md
//! [FR-RS-24]: ../../../docs/specs/requirements/FR-RS-24.md
//! [FR-RS-30]: ../../../docs/specs/requirements/FR-RS-30.md
//! [FR-RS-32]: ../../../docs/specs/requirements/FR-RS-32.md
//! [FR-RS-37]: ../../../docs/specs/requirements/FR-RS-37.md
//! [FR-RS-39]: ../../../docs/specs/requirements/FR-RS-39.md
//! [FR-RS-27]: ../../../docs/specs/requirements/FR-RS-27.md
//! [CR-188]: ../../../docs/requests/CR-188-rust-receiver-typing.md
//! [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
//! [CR-169]: ../../../docs/requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md
//! [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
//! [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
//! [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
//! [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use tree_sitter::Node;

use super::config::accessor::{outermost_callable, Declarations, DeclaredTypes};
use super::local_shadow::{LocalShadows, Shadow};
use super::refs::MacroReceiver;
use super::{
    declared_superclass, type_parameters_in_scope, Decl, RefFact, SELF_RECEIVER_METHOD_CAPTURE,
};
use crate::model::{EdgeKind, NodeKind, ReceiverShape, RefForm};
use crate::plugin::{ImplicitReceiver, ModuleModelKind, Semantics};
use crate::resolve::{is_class_like, SELF_TYPE_HEAD, STATIC_WILDCARD_ALIAS};

/// The prefix every receiver marker capture carries.
const MARKER: &str = "ref.receiver.";

/// A receiver a typing marker names (S-467).
enum Typed {
    /// `x.send()` — a simple name.
    Name(String),
    /// `this.x.send()` — a field of the enclosing class.
    Field(String),
    /// `this.send()`.
    This,
    /// `super.send()`.
    Super,
    /// `send()` — no receiver at all.
    Implicit,
    /// `x.send()` — a plain name, typed by the one binding of it in scope at
    /// the call (S-587, Rust's `variable` marker).
    Variable(String),
    /// `p->send()` — a plain name, typed by the one binding of it in scope at
    /// the call when that binding's proof is a pointer's (S-581, C++'s
    /// `pointee` marker): the call reaches the type pointed to.
    Pointee(String),
    /// `self.x.send()` — a field of the caller's own struct, reached through
    /// the caller's self type (S-587, Rust's `self_field` marker).
    SelfField(String),
}

/// What one call's markers say about its receiver.
#[derive(Default)]
struct Marks {
    /// The receiver the typing pass reads, when a typing marker names one.
    typed: Option<Typed>,
    /// `self` (or Java's `this`): the caller's own instance.
    own: bool,
    /// `super`: the caller's base.
    base: bool,
    /// `other`, `name` or `field`: any other receiver.
    other: bool,
    /// An `other` marker's receiver text, compared with the caller's
    /// `self_name`.
    other_text: Option<String>,
    /// `implicit`: no receiver was written.
    implicit: bool,
    /// `refused`: never typed, and `other` whatever else is marked.
    refused: bool,
}

/// Where a call sits relative to the class-like declarations around it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Enclosing {
    /// Inside the class-like declaration at this index.
    Class(usize),
    /// Inside the body the `anonymous` marker names with this node id (an
    /// anonymous class body, or any body whose `this` is not the enclosing
    /// class's instance), nearer than any named class.
    Anonymous(usize),
    /// Inside no class at all.
    Outside,
}

impl Enclosing {
    fn class(self) -> Option<usize> {
        match self {
            Enclosing::Class(i) => Some(i),
            _ => None,
        }
    }
}

/// What the shape pass records for one Method-form row.
enum Recorded {
    Shape(ReceiverShape),
    /// An implicit call that is not on the current instance: the free call a
    /// `@ref.call` records.
    FreeCall,
    /// An implicit call a local binding in force shadows (S-594): the free
    /// call, marked as a call through that binding.
    Local(Shadow),
    Nothing,
}

/// A row `collect_refs` pushed that its receiver may retype or shape.
struct Site<'tree> {
    /// The row's index in the file's refs.
    row: usize,
    /// The call's member or invocation expression — the node its markers'
    /// captures sit in.
    invocation: Node<'tree>,
    /// The innermost declaration enclosing the call, if any.
    caller: Option<usize>,
}

/// A row the macro walk pushed for a method call inside a macro's token tree
/// (S-610), whose receiver it read in code: no query marks a receiver there.
struct MacroSite<'tree> {
    /// The row's index in the file's refs.
    row: usize,
    /// The macro invocation: no node inside its token tree is a binding's scope,
    /// so every call in it sees the bindings the invocation sees.
    invocation: Node<'tree>,
    /// The innermost declaration enclosing the macro, if any.
    caller: Option<usize>,
    /// The receiver the walk read: `Variable` or `SelfField`.
    receiver: Typed,
}

/// Where a binding a `binding` marker records is in scope (S-587).
#[derive(Clone, Copy)]
enum Scope<'tree> {
    /// Inside this node (the companion `.scope`): a parameter's callable, a
    /// loop, a match arm.
    Whole(Node<'tree>),
    /// In this node's parent, after this node ends (the companion `.after`):
    /// a `let` binds for the rest of its block, never in its own initializer.
    After(Node<'tree>),
    /// Anywhere in the outermost callable around the binding — what a pattern
    /// no companion scopes is given, which can only refuse more.
    Callable,
    /// In the innermost callable enclosing this node (the companion `.local`),
    /// the callables nested in it included — or, at a file's top level, the
    /// calls outside every callable: a Python local, bound for the whole of
    /// its function wherever in it the statement sits (S-546).
    Local(Node<'tree>),
    /// In the body of this class (the companion `.class`), and in no callable
    /// inside it: a Python class-body name, which a method never sees (S-546).
    /// It outranks every other scope recorded for the same name.
    Class(Node<'tree>),
    /// In the members of this class-like declaration (the companion
    /// `.member`), read with no `this.` written: a Kotlin property (S-563).
    /// Only where that class is the call's own ([`Receivers::member_owner`]) —
    /// never in a nested or anonymous class, a companion, or a body a
    /// `rebinds` marker names, whose own receiver may declare the name.
    Member(Node<'tree>),
}

/// Where one name a pattern binds is in scope (S-587), resolved once when
/// the binding is recorded, so a call's test is a comparison.
#[derive(Clone, Copy)]
enum Binding {
    /// The calls whose bytes lie in `start..=end`: [`Scope::Whole`]'s node,
    /// or [`Scope::After`]'s parent from where its node ends.
    Bytes { start: usize, end: usize },
    /// The calls in the outermost callable with this node id (`None`: at a
    /// field position).
    Callable(Option<usize>),
    /// The calls whose bytes lie in `start..=end` that no callable inside
    /// that range encloses: [`Scope::Class`]'s body.
    ClassBody { start: usize, end: usize },
    /// The calls whose member owner is the class-like declaration with this
    /// node id: [`Scope::Member`]'s.
    Member(usize),
}

impl Binding {
    /// The binding `scope` gives the pattern leaf `leaf`.
    fn new(leaf: Node<'_>, scope: Scope<'_>) -> Self {
        match scope {
            Scope::Whole(node) => Binding::Bytes { start: node.start_byte(), end: node.end_byte() },
            Scope::After(node) => Binding::Bytes {
                start: node.end_byte(),
                end: node.parent().map_or(node.end_byte(), |p| p.end_byte()),
            },
            Scope::Callable => Binding::Callable(outermost_callable(leaf).map(|n| n.id())),
            Scope::Local(node) => match node.parent().and_then(innermost_callable) {
                Some(callable) => Binding::Bytes { start: callable.start_byte(), end: callable.end_byte() },
                None => Binding::Callable(None),
            },
            Scope::Class(node) => Binding::ClassBody { start: node.start_byte(), end: node.end_byte() },
            Scope::Member(node) => Binding::Member(node.id()),
        }
    }

    /// Whether this binding is in scope at `invocation`, the outermost
    /// callable around which has node id `callable`; `owner` answers whose
    /// members it reads with no receiver written ([`Receivers::member_owner`]).
    fn in_scope(self, invocation: Node<'_>, callable: Option<usize>, owner: impl FnOnce() -> Option<usize>) -> bool {
        let within = |start: usize, end: usize| start <= invocation.start_byte() && invocation.end_byte() <= end;
        match self {
            Binding::Bytes { start, end } => within(start, end),
            Binding::Callable(c) => c == callable,
            Binding::ClassBody { start, end } => {
                within(start, end) && innermost_callable(invocation).is_none_or(|c| c.start_byte() < start)
            }
            Binding::Member(class) => owner() == Some(class),
        }
    }
}

/// The innermost node at or around `node` that declares a callable — the
/// nearest one field-naming `parameters` ([`outermost_callable`]'s test).
fn innermost_callable(node: Node<'_>) -> Option<Node<'_>> {
    let mut at = Some(node);
    while let Some(current) = at {
        if current.child_by_field_name("parameters").is_some() {
            return Some(current);
        }
        at = current.parent();
    }
    None
}

/// What a `proof` marker says a binding's type is (S-587).
#[derive(Clone, Copy)]
enum Proof<'tree> {
    /// A declared type: `x: T`, `let x: T`.
    Declared(Node<'tree>),
    /// The callee `T::g` of `let x = T::g(…)`.
    Constructor(Node<'tree>),
    /// The type of `let x = T { … }`.
    Literal(Node<'tree>),
    /// The type a declared pointer points to: `T* p`, `std::unique_ptr<T> p`
    /// (S-581, C++). It proves a `pointee` receiver (`p->m()`) and never a
    /// `variable` one: `p.reset()` is a call on the pointer itself.
    Pointer(Node<'tree>),
}

/// A struct field's declared type, and the struct's declaration index.
type Member<'tree> = (Node<'tree>, Option<usize>);

/// A receiver's proven type: its head as the file writes it, and the wrappers
/// peeled off to reach it, outermost first.
struct Proven {
    head: String,
    peeled: Vec<&'static str>,
}

impl Proven {
    /// The retyped row's target `T::name` and its recorded wrappers.
    fn into_row(self, name: &str) -> (String, Option<String>) {
        let peeled = (!self.peeled.is_empty()).then(|| self.peeled.join(" "));
        (format!("{}::{name}", self.head), peeled)
    }
}

/// The wrappers a Rust receiver's declared type is peeled of ([FR-RS-42]):
/// a call through each reaches the wrapped type's methods. No other wrapper is
/// peeled — `Option`, `Vec`, `Mutex` are the receiver's type themselves.
///
/// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
const PEELED_WRAPPERS: [&str; 3] = ["Box", "Arc", "Rc"];

/// What `collect_refs` knows about a file's receivers while it walks the query
/// matches — built only for a query that declares a receiver marker or the
/// `@ref.method.self` capture.
pub(super) struct Receivers<'tree> {
    /// invocation node id → what its markers say.
    marks: HashMap<usize, Marks>,
    /// Every name the file declares in a way [`DeclaredTypes`] does not read — an
    /// untyped lambda parameter, a for-each variable, a catch parameter, a
    /// pattern variable, a varargs parameter, a qualified declared type
    /// (`com.b.Mailer`), whose simple name the imports would re-qualify to
    /// another class. File-scoped, like
    /// [`DeclaredTypes`]: the type the file declares for that name elsewhere is
    /// not proven to be this one's, so the name is poisoned for the whole file.
    /// A `variable` receiver of the name is refused too (S-546: a Python
    /// `global` or `nonlocal` name, which another function may rebind).
    unproven: HashSet<String>,
    /// The names the file's NON-static single-type imports bring into scope — a
    /// static import names a member, never a type.
    type_imports: HashSet<String>,
    /// The bodies the `anonymous` marker names, by node id — anonymous class
    /// bodies and every other body whose `this` is not the enclosing class's.
    anonymous: HashSet<usize>,
    /// The bodies the `rebinds` marker names, by node id (S-563): a lambda
    /// whose own receiver may declare any name — Kotlin's `x.apply { }` — so
    /// no [`Scope::Member`] binding of an enclosing class is in scope inside.
    rebinding: HashSet<usize>,
    /// declaration index → the name it binds its own instance to (`self_name`);
    /// `None` when two of its markers disagree.
    self_names: HashMap<usize, Option<String>>,
    /// Every row a receiver could retype or shape.
    sites: Vec<Site<'tree>>,
    /// Every method-call row inside a macro whose receiver a proof may type.
    macro_sites: Vec<MacroSite<'tree>>,
    /// What an unqualified call inside a class body means here.
    implicit_receiver: ImplicitReceiver,
    /// Whether a `super` call is typed by its class's one `Extends` row
    /// ([`declared_superclass`]): only in a package-model language (Java,
    /// S-467), whose package rungs walk the typed row into that type's members.
    /// Every other language's `super` call keeps its shape and binds through
    /// the proven hierarchy (S-522): a path-model module walk never reaches a
    /// class's members, and a C# or Kotlin class's one supertype may be an
    /// interface.
    types_super: bool,
    /// Whether a typed row is a proven row: it keeps its `other` shape, its
    /// type qualified through the file's named imports ([`FileDecls::imported`]),
    /// and is bound by the binder's proven-receiver rung; a `this.x` reads the
    /// class's own field declarations ([`Declarations::stated_only`]). A
    /// structural-types language's (S-537, TypeScript; the plugin's
    /// `structural_types`) and an import-root language's (S-546, Python; the
    /// plugin's `import_roots`): neither module walk reaches the members a
    /// class inherits.
    proven_rows: bool,
    /// Whether the binder binds a call through the one associated-item lookup
    /// over `impl` blocks (S-607, Rust; the plugin's `impl_block_lookup`). A
    /// proven row keeps its `other` shape only here and in a
    /// [`proven_rows`](Self::proven_rows) language, the two the binder's
    /// proven-receiver rung serves; anywhere else it is the written `T::m`
    /// path the language's own rungs bind (S-552, PHP's static call).
    impl_blocks: bool,
    /// Whether a type lends its name to a companion — a Kotlin `Companion`
    /// (S-563; the plugin's `companion_name`) or a Scala same-named `object`
    /// (S-573; the plugin's `companion_objects`): the written `T::m` reaches
    /// the companion's `m` (I4-INT, S-573), so a typed row is a proven row —
    /// it keeps its `other` shape and the binder's proven-receiver rung binds
    /// it among `T`'s own members — and its declared type is read as written,
    /// peeling no wrapper.
    companion_paths: bool,
    /// Every name a `binding` marker's pattern binds (S-587): name → pattern
    /// leaf id → the binding. Keyed by leaf so two captures of one pattern
    /// count it once.
    bindings: HashMap<String, BTreeMap<usize, Binding>>,
    /// Pattern leaf id → the type its `proof` marker proves.
    proofs: HashMap<usize, Proof<'tree>>,
    /// `(impl type's last segment, function name)` → the function's
    /// declaration index → its declared return type's text (`None`: it
    /// declares none) — every associated function the file declares, read by a
    /// `.constructor` proof.
    constructors: HashMap<(String, String), BTreeMap<usize, Option<String>>>,
    /// An associated function's declaration index → its impl's type as
    /// written (`S`, `S<T>`, `crate::m::S`) — what its `Self` names.
    impl_owners: HashMap<usize, String>,
    /// `(struct name, field name)` → the field's node id → its declared type
    /// and the struct's declaration index — read by a `self_field` receiver.
    members: HashMap<(String, String), BTreeMap<usize, Member<'tree>>>,
    /// The declarations the query states for [`DeclaredTypes`] (S-558, the
    /// `declared` marker's `.type` companion): read in place of the field
    /// walk's reading of the same name.
    declarations: Declarations<'tree>,
    /// name → the byte ranges of the type declarations a `declared` marker's
    /// `.scope` companion puts it in scope in (S-558): a C# primary-constructor
    /// parameter, which its type declares no field of.
    scoped: HashMap<String, Vec<(usize, usize)>>,
    /// Each operand-named access row (S-529), with its caller, its operand's
    /// node and that node's text — appended by [`finish`](Self::finish) when
    /// the operand is the caller's `self_name` and no local binding rebinds it
    /// there (S-594).
    accesses: Vec<(Option<usize>, Node<'tree>, String, RefFact)>,
    /// Every name a `hides` marker names: a name a declaration declares,
    /// which [`bind_hiders`](Self::bind_hiders) binds where no `binding`
    /// states it (S-581).
    hiders: Vec<Node<'tree>>,
    /// The byte ranges the `hiding_scope` marker names: a C++ function or
    /// lambda, across which an unstated declaration hides its name (S-581).
    hiding_scopes: Vec<(usize, usize)>,
}

/// What a file's declarations say, for the receiver passes.
pub(super) struct FileDecls<'a, 'tree> {
    pub root: Node<'tree>,
    pub source: &'a [u8],
    pub decls: &'a [Decl<'tree>],
    pub symbols: &'a [Option<crate::model::LogosSymbol>],
    pub id_to_idx: &'a HashMap<usize, usize>,
    /// The qualified target a type name the file imports by name is recorded
    /// under at a node — `<import target>::<exported name>`, read by a
    /// path-import language's typed row (S-537), as its type relations read it
    /// (S-535); `None` for a name no named import binds there.
    pub imported: &'a dyn Fn(Node<'tree>, &str) -> Option<String>,
    /// The local bindings in force at a node (S-594): a rebound `self_name`
    /// is no longer the receiver, and a local shadows an implicit call.
    pub shadows: &'a LocalShadows<'a>,
}

/// Whether `capture` is a receiver marker.
pub(super) fn is_marker(capture: &str) -> bool {
    capture.starts_with(MARKER)
}

impl<'tree> Receivers<'tree> {
    /// The receiver state for a query, or [`None`] when it declares no marker
    /// and no `@ref.method.self` — then nothing is recorded and no row is ever
    /// retyped or shaped.
    pub(super) fn for_query(capture_names: &[&str], semantics: &Semantics) -> Option<Self> {
        let proven_rows = semantics.structural_types || semantics.import_roots.is_some();
        capture_names
            .iter()
            .any(|c| is_marker(c) || *c == SELF_RECEIVER_METHOD_CAPTURE)
            .then(|| Self {
                marks: HashMap::new(),
                unproven: HashSet::new(),
                type_imports: HashSet::new(),
                anonymous: HashSet::new(),
                rebinding: HashSet::new(),
                self_names: HashMap::new(),
                sites: Vec::new(),
                macro_sites: Vec::new(),
                implicit_receiver: semantics.implicit_receiver,
                types_super: semantics.module_model == ModuleModelKind::Package,
                proven_rows,
                impl_blocks: semantics.impl_block_lookup,
                companion_paths: semantics.companion_name.is_some() || semantics.companion_objects,
                bindings: HashMap::new(),
                proofs: HashMap::new(),
                constructors: HashMap::new(),
                impl_owners: HashMap::new(),
                members: HashMap::new(),
                declarations: if proven_rows { Declarations::stated_only() } else { Declarations::default() },
                scoped: HashMap::new(),
                accesses: Vec::new(),
                hiders: Vec::new(),
                hiding_scopes: Vec::new(),
            })
    }

    /// Record one marker capture, matched on the whole capture name — never on
    /// a node kind or a grammar field. `declaration` is the innermost
    /// declaration enclosing the captured node, asked only by `self_name`;
    /// `companion` names another capture of the same match, the node an anchor
    /// marker (`binding`, `proof`, `constructor`, `member`) reads its
    /// `<anchor>.<role>` companions from (S-587). A companion is read with its
    /// anchor and records nothing of its own.
    pub(super) fn mark(
        &mut self,
        capture: &str,
        node: Node<'tree>,
        source: &[u8],
        declaration: impl FnOnce() -> Option<usize>,
        companion: impl Fn(&str) -> Option<Node<'tree>>,
    ) {
        let text = || node.utf8_text(source).ok().map(|t| t.trim().to_string());
        let read = |n: Node<'_>| n.utf8_text(source).ok().map(|t| t.trim().to_string());
        match capture {
            "ref.receiver.binding" => {
                let scope = companion("ref.receiver.binding.scope")
                    .map(Scope::Whole)
                    .or_else(|| companion("ref.receiver.binding.after").map(Scope::After))
                    .or_else(|| companion("ref.receiver.binding.local").map(Scope::Local))
                    .or_else(|| companion("ref.receiver.binding.class").map(Scope::Class))
                    .or_else(|| companion("ref.receiver.binding.member").map(Scope::Member))
                    .unwrap_or(Scope::Callable);
                for leaf in bound_names(node) {
                    if let Some(name) = read(leaf).filter(|n| is_identifier(n)) {
                        self.bind(name, leaf, scope);
                    }
                }
                return;
            }
            "ref.receiver.proof" => {
                let proof = companion("ref.receiver.proof.type")
                    .map(Proof::Declared)
                    .or_else(|| companion("ref.receiver.proof.constructor").map(Proof::Constructor))
                    .or_else(|| companion("ref.receiver.proof.literal").map(Proof::Literal))
                    .or_else(|| companion("ref.receiver.proof.pointer").map(Proof::Pointer));
                if let Some(proof) = proof {
                    self.proofs.insert(node.id(), proof);
                }
                return;
            }
            // The function's own declaration encloses its name.
            "ref.receiver.constructor" => {
                let owner = companion("ref.receiver.constructor.owner").and_then(read);
                if let (Some(owner), Some(name), Some(function)) = (owner, text(), declaration()) {
                    let returns = companion("ref.receiver.constructor.returns").and_then(read);
                    self.constructors
                        .entry((base_name(&owner).to_string(), name))
                        .or_default()
                        .insert(function, returns);
                    self.impl_owners.insert(function, owner);
                }
                return;
            }
            // The struct's declaration encloses its field's name.
            "ref.receiver.member" => {
                let owner = companion("ref.receiver.member.owner").and_then(read);
                let declared = companion("ref.receiver.member.type");
                if let (Some(owner), Some(name), Some(declared), Some(field)) = (owner, text(), declared, node.parent()) {
                    let entry = (declared, declaration());
                    self.members.entry((owner, name)).or_default().insert(field.id(), entry);
                }
                return;
            }
            // A declaration the field walk misreads or never sees, one in scope
            // beyond the callable it sits in (S-558), one declaring no type a
            // call is typed by, or a field from a parameter list (S-537).
            "ref.receiver.declared" => {
                if let Some(declared) = companion("ref.receiver.declared.type") {
                    self.declarations.declare(node, declared);
                }
                if companion("ref.receiver.declared.untyped").is_some() {
                    self.declarations.untyped(node);
                }
                if companion("ref.receiver.declared.field").is_some() {
                    self.declarations.field(node);
                }
                if let (Some(scope), Some(name)) = (companion("ref.receiver.declared.scope"), text()) {
                    self.scoped.entry(name).or_default().push((scope.start_byte(), scope.end_byte()));
                }
                return;
            }
            // A companion: read with its anchor above.
            c if ["binding.", "proof.", "constructor.", "member.", "declared."]
                .iter()
                .any(|anchor| c.strip_prefix(MARKER).is_some_and(|rest| rest.starts_with(anchor))) =>
            {
                return;
            }
            _ => {}
        }
        match capture {
            "ref.receiver.unproven" => {
                self.unproven.extend(text());
                return;
            }
            "ref.receiver.anonymous" => {
                self.anonymous.insert(node.id());
                return;
            }
            "ref.receiver.hides" => {
                self.hiders.push(node);
                return;
            }
            "ref.receiver.hiding_scope" => {
                self.hiding_scopes.push((node.start_byte(), node.end_byte()));
                return;
            }
            "ref.receiver.rebinds" => {
                self.rebinding.insert(node.id());
                return;
            }
            "ref.receiver.self_name" => {
                if let (Some(decl), Some(name)) = (declaration(), text()) {
                    self.self_names
                        .entry(decl)
                        .and_modify(|known| {
                            if known.as_deref() != Some(name.as_str()) {
                                *known = None;
                            }
                        })
                        .or_insert(Some(name));
                }
                return;
            }
            _ => {}
        }
        // The captured node's parent is the invocation — for `this.x`, the
        // captured `x` sits one level deeper, inside the field access.
        let invocation = match capture {
            "ref.receiver.field" | "ref.receiver.self_field" => node.parent().and_then(|p| p.parent()),
            _ => node.parent(),
        };
        let Some(invocation) = invocation else {
            return;
        };
        let marks = self.marks.entry(invocation.id()).or_default();
        match capture {
            "ref.receiver.refused" => marks.refused = true,
            "ref.receiver.name" => {
                marks.other = true;
                marks.typed = text().map(Typed::Name);
            }
            "ref.receiver.field" => {
                marks.other = true;
                marks.typed = text().map(Typed::Field);
            }
            "ref.receiver.this" => {
                marks.own = true;
                marks.typed = Some(Typed::This);
            }
            "ref.receiver.super" => {
                marks.base = true;
                marks.typed = Some(Typed::Super);
            }
            "ref.receiver.implicit" => {
                marks.implicit = true;
                marks.typed = Some(Typed::Implicit);
            }
            // Rust's typing markers (S-587) type the receiver and nothing
            // else: its `other` shape is the `other` marker's. A `variable` is
            // read as a `binding` pattern reads its names, by its one leaf: a
            // PHP `$m` is the `m` its parameter binds (S-552). A `pointee` is
            // read the same way (S-581, C++'s `p->m()`).
            "ref.receiver.variable" | "ref.receiver.pointee" => {
                let leaves = bound_names(node);
                let name = (leaves.len() == 1).then(|| read(leaves[0])).flatten();
                let through_pointer = capture == "ref.receiver.pointee";
                marks.typed = name.map(if through_pointer { Typed::Pointee } else { Typed::Variable });
            }
            "ref.receiver.self_field" => marks.typed = text().map(Typed::SelfField),
            "ref.receiver.self" => marks.own = true,
            "ref.receiver.other" => {
                marks.other = true;
                marks.other_text = text();
            }
            _ => {}
        }
    }

    /// Bind every name a `hides` marker names that no `binding` states
    /// (S-581): with no proof, across the innermost `hiding_scope` around it.
    /// A declaration form the query scopes no binding for — a labelled one, a
    /// `case`'s, one under nested `#if`s, a parameter of an unlisted
    /// declarator — then hides every other binding of its name there, so an
    /// outer one is never the one binding in scope: the walk fails closed,
    /// and a scope wider than the declaration's own only refuses more. A name
    /// outside every hiding scope (a prototype's parameter, a namespace-scope
    /// variable) binds nothing.
    fn bind_hiders(&mut self, source: &[u8]) {
        let stated: HashSet<usize> = self.bindings.values().flat_map(|leaves| leaves.keys().copied()).collect();
        for leaf in std::mem::take(&mut self.hiders) {
            if stated.contains(&leaf.id()) {
                continue;
            }
            let around = |&&(start, end): &&(usize, usize)| start <= leaf.start_byte() && leaf.end_byte() <= end;
            let innermost = self.hiding_scopes.iter().filter(around).min_by_key(|(start, end)| end - start);
            let (Some(&(start, end)), Some(name)) = (innermost, leaf.utf8_text(source).ok().map(str::trim)) else {
                continue;
            };
            self.proofs.remove(&leaf.id());
            self.bindings.entry(name.to_string()).or_default().insert(leaf.id(), Binding::Bytes { start, end });
        }
    }

    /// Record that the pattern leaf `leaf` binds `name` in `scope`. A leaf is
    /// recorded once, by the first match naming it — except that a class
    /// body's scope outranks any other the same leaf is recorded with,
    /// whichever match came first (S-546).
    fn bind(&mut self, name: String, leaf: Node<'tree>, scope: Scope<'tree>) {
        let binding = Binding::new(leaf, scope);
        let recorded = self.bindings.entry(name).or_default().entry(leaf.id());
        match (scope, recorded) {
            (Scope::Class(_), std::collections::btree_map::Entry::Occupied(mut held)) => {
                held.insert(binding);
            }
            (_, entry) => {
                entry.or_insert(binding);
            }
        }
    }

    /// Defer an access row whose selector's operand is the node `operand`,
    /// written `name` (S-529): kept by [`finish`](Self::finish) only when it is
    /// `caller`'s own receiver — `c.enc` inside `func (c *ioCore) …` — which
    /// the walk's `self_name` markers say only once it ends, and the body does
    /// not rebind the name where the access is (S-594).
    pub(super) fn defer_access(&mut self, caller: Option<usize>, operand: Node<'tree>, name: &str, row: RefFact) {
        self.accesses.push((caller, operand, name.to_string(), row));
    }

    /// The name the declaration at `caller` binds its own instance to — its
    /// `self_name` marker's text — or `None` when it records none or two that
    /// disagree. What an operand-named access is kept on (S-529).
    pub(super) fn self_name(&self, caller: Option<usize>) -> Option<&str> {
        caller.and_then(|c| self.self_names.get(&c)).and_then(Option::as_deref)
    }

    /// Mark `invocation`'s receiver as the caller's own instance — what the
    /// S-493 capture `@ref.method.self` says of its call.
    pub(super) fn mark_self(&mut self, invocation: Option<Node<'tree>>) {
        if let Some(invocation) = invocation {
            self.marks.entry(invocation.id()).or_default().own = true;
        }
    }

    /// Mark the bare name `name` as a call written with no receiver — what the
    /// `implicit` marker says of a call's name, for a call that is the name
    /// alone (S-568, Ruby's `to_app`): it is its own invocation.
    pub(super) fn mark_implicit(&mut self, name: Node<'tree>) {
        let marks = self.marks.entry(name.id()).or_default();
        marks.implicit = true;
        marks.typed = Some(Typed::Implicit);
    }

    /// Note that the row about to be pushed at `row` is a call from
    /// `invocation`, made by the declaration at `caller`, which its receiver
    /// may retype or shape.
    pub(super) fn site(
        &mut self,
        row: usize,
        invocation: Option<Node<'tree>>,
        caller: Option<usize>,
    ) {
        if let Some(invocation) = invocation {
            self.sites.push(Site {
                row,
                invocation,
                caller,
            });
        }
    }

    /// Note that the row about to be pushed at `row` is a method call written
    /// inside the token tree of `invocation`, made by the declaration at
    /// `caller`, whose `receiver` the walk read (S-610): it is proven by the
    /// forms [`Receivers::retype`] reads for a call outside a macro.
    pub(super) fn macro_site(
        &mut self,
        row: usize,
        invocation: Node<'tree>,
        caller: Option<usize>,
        receiver: MacroReceiver,
    ) {
        let receiver = match receiver {
            MacroReceiver::Name(x) => Typed::Variable(x),
            MacroReceiver::OwnField(x) => Typed::SelfField(x),
        };
        self.macro_sites.push(MacroSite { row, invocation, caller, receiver });
    }

    /// Note a non-static single-type import's name.
    pub(super) fn type_import(&mut self, name: &str) {
        self.type_imports.insert(name.to_string());
    }

    /// Append the deferred access rows on their caller's own receiver (S-529),
    /// then retype every site whose receiver the file proves and shape every
    /// Method-form row typing left bare, in place. Runs once, after every
    /// match: a `super` receiver reads the `Extends` rows this same pass
    /// recorded, and a bare call the file's imports.
    pub(super) fn finish(mut self, refs: &mut Vec<RefFact>, file: &FileDecls<'_, 'tree>) {
        // The deferred accesses on the caller's own receiver, appended: no
        // site's row moves. A receiver name the body rebinds where the access
        // is (S-594) — a `range` variable, a `:=`, a closure parameter — is
        // another value, as it is for a call (`shape`).
        let accesses = std::mem::take(&mut self.accesses);
        refs.extend(
            accesses
                .into_iter()
                .filter(|(caller, node, operand, _)| {
                    self.self_name(*caller) == Some(operand.as_str()) && !file.shadows.rebinds(*node, operand)
                })
                .map(|(_, _, _, row)| row),
        );
        if self.sites.is_empty() && self.macro_sites.is_empty() {
            return;
        }
        self.bind_hiders(file.source);
        // A row typing retyped is Path form now: only bare Method rows remain.
        self.retype(refs, file);
        for site in &self.sites {
            if refs[site.row].form != RefForm::Method {
                continue;
            }
            let Some(marks) = self.marks.get(&site.invocation.id()) else {
                continue;
            };
            let row = &mut refs[site.row];
            match self.shape(marks, site, file, &row.target) {
                Recorded::Shape(ReceiverShape::SelfInstance) => {
                    let caller = site.caller.map(|c| &file.decls[c]);
                    (row.target, row.form, row.receiver) = self_call(caller, &row.target, marks.own);
                }
                Recorded::Shape(shape) => row.receiver = Some(shape),
                Recorded::FreeCall => row.form = RefForm::Path,
                Recorded::Local(local) => {
                    row.form = RefForm::Path;
                    local.mark(row);
                }
                Recorded::Nothing => {}
            }
        }
    }

    /// The shape a site's markers give its Method-form row.
    fn shape(&self, marks: &Marks, site: &Site<'_>, file: &FileDecls<'_, '_>, name: &str) -> Recorded {
        if marks.refused {
            return Recorded::Shape(ReceiverShape::Other);
        }
        let class = || enclosing_class(site.invocation, file, &self.anonymous);
        match (marks.own, marks.base) {
            (true, true) => return Recorded::Shape(ReceiverShape::Other),
            // An anonymous class's instance has no node to bind through. A
            // `self` call to a callable its body declares itself reaches it as
            // the free call the lexical scope binds — the body's members sit
            // under the enclosing callable, nearer than any other class's.
            (true, false) | (false, true) if matches!(class(), Enclosing::Anonymous(_)) => {
                return match class() {
                    Enclosing::Anonymous(body) if marks.own && declares_in(file, body, name) => {
                        Recorded::FreeCall
                    }
                    _ => Recorded::Shape(ReceiverShape::Other),
                };
            }
            (true, false) => return Recorded::Shape(ReceiverShape::SelfInstance),
            (false, true) => return Recorded::Shape(ReceiverShape::Super),
            (false, false) => {}
        }
        if marks.other {
            // The caller's own receiver name — unless the body rebinds it where
            // the call is (S-594): a `range` variable, a `:=`, a closure
            // parameter or a type-switch binding of that name is another value.
            let own_name = self.self_name(site.caller);
            let own = own_name.is_some_and(|own| {
                marks.other_text.as_deref() == Some(own) && !file.shadows.rebinds(site.invocation, own)
            });
            return Recorded::Shape(if own {
                ReceiverShape::SelfInstance
            } else {
                ReceiverShape::Other
            });
        }
        if marks.implicit {
            // A local binding in force names the call's target (S-594).
            if let Some(local) = file.shadows.at(site.invocation, name) {
                return Recorded::Local(local);
            }
            return match (self.implicit_receiver, class()) {
                // A callable an enclosing callable declares shadows the
                // class's member, as an anonymous body's own member does: the
                // free call the lexical scope binds reaches it.
                (ImplicitReceiver::SelfInstance, Enclosing::Class(class))
                    if !declared_locally(file, site.caller, class, name) =>
                {
                    Recorded::Shape(ReceiverShape::SelfInstance)
                }
                _ => Recorded::FreeCall,
            };
        }
        Recorded::Nothing
    }

    /// Whether a typed row is bound by the binder's proven-receiver rung —
    /// keeping its `other` shape — rather than as the written `T::m` the
    /// language's own rungs bind: in an [`impl_blocks`](Self::impl_blocks),
    /// [`proven_rows`](Self::proven_rows) or
    /// [`companion_paths`](Self::companion_paths) language, where the written
    /// path is not the instance's (S-576: a Scala `T::m` reaches `T`'s
    /// companion `object`).
    fn proven_rung(&self) -> bool {
        self.impl_blocks || self.proven_rows || self.companion_paths
    }

    /// Retype every site whose receiver the file proves, in place (S-467).
    fn retype(&self, refs: &mut [RefFact], file: &FileDecls<'_, 'tree>) {
        // The one per-file walk this costs, paid only by a file with a site
        // that asks it (a simple-name or `this.x` receiver) — NFR-PE-02.
        let declared: OnceCell<DeclaredTypes> = OnceCell::new();
        let types = || declared.get_or_init(|| DeclaredTypes::build_with(file.root, file.source, &self.declarations));
        let imported: HashSet<&str> = refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Imports && r.form == RefForm::Path)
            .filter_map(|r| r.alias.as_deref())
            .collect();
        let static_wildcard = refs.iter().any(|r| {
            r.kind == EdgeKind::Imports
                && r.form == RefForm::Glob
                && r.alias.as_deref() == Some(STATIC_WILDCARD_ALIAS)
        });

        // Every class that records a supertype: its fields may be shadowed by
        // an inherited one this file cannot see.
        let supertyped: HashSet<&str> = refs
            .iter()
            .filter(|r| matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements))
            .map(|r| r.source.as_str())
            .collect();
        let anonymous = &self.anonymous;
        // A callable's own declarations, keyed by the callable — the scope a
        // simple-name receiver must be declared in (see `declared_in_scope`).
        let mut scopes: HashMap<usize, DeclaredTypes> = HashMap::new();
        // A class's own field declarations, keyed by the class — what a
        // proven-rows language's `this.x` / `self.x` reads.
        let mut classes: HashMap<usize, DeclaredTypes> = HashMap::new();
        let mut typed: Vec<(usize, String)> = Vec::new();
        // Rust's proven receivers (S-587): retyped keeping their `other` shape,
        // with the wrappers peeled to reach the type — and a path-import
        // language's typed rows (S-537), which peel nothing.
        let mut proven: Vec<(usize, String, Option<String>)> = Vec::new();
        for site in &self.sites {
            let (row, invocation) = (site.row, site.invocation);
            let Some(marks) = self.marks.get(&invocation.id()) else {
                continue;
            };
            let Some(receiver) = marks.typed.as_ref().filter(|_| !marks.refused) else {
                continue;
            };
            let name = refs[row].target.as_str();
            if let Typed::Variable(_) | Typed::Pointee(_) | Typed::SelfField(_) = receiver {
                // Only a Method-form row is a receiver call to retype.
                if refs[row].form != RefForm::Method {
                    continue;
                }
                let Some(found) = self.proven_receiver(receiver, invocation, site.caller, file) else {
                    continue;
                };
                let (target, peeled) = self.qualified(found, invocation, file).into_row(name);
                // Bound by the proven-receiver rung ([`proven_rung`](Self::proven_rung)),
                // or as the written `T::m` the language binds.
                if self.proven_rung() {
                    proven.push((row, target, peeled));
                } else {
                    typed.push((row, target));
                }
                continue;
            }
            let provable = |t: &str| provable_type(t, invocation, file.source);
            let class = || enclosing_class(invocation, file, anonymous).class();
            let head = match receiver {
                Typed::Name(x) if self.unproven.contains(x) => None,
                // A variable: typed only where it is declared in scope at the
                // call — the file-wide type is then its type, because every
                // declaration of the name in the file agrees. Declared nowhere
                // in scope, it is inherited or an outer class's: unproven.
                Typed::Name(x) if types().declares(x) => {
                    self.declared_in_scope(invocation, x, file, &supertyped, &mut scopes)
                        .then(|| types().get(x))
                        .flatten()
                        .filter(|t| provable(t))
                        .map(str::to_string)
                }
                Typed::Name(x) => (self.type_imports.contains(x) || declares_type(file.decls, x)).then(|| x.clone()),
                // `this.x` is the enclosing class's field only when that class
                // declares it; an inherited `x` is another class's.
                // A proven-rows language reads the class's own field
                // declarations alone, every one of which its query states.
                Typed::Field(x) if self.proven_rows => class()
                    .filter(|&i| declares_field(file.decls, i, x))
                    .and_then(|i| {
                        let own = classes.entry(i).or_insert_with(|| {
                            DeclaredTypes::build_with(file.decls[i].node, file.source, &self.declarations)
                        });
                        own.field(x).filter(|t| provable(t)).map(str::to_string)
                    }),
                Typed::Field(x) => class()
                    .filter(|&i| declares_field(file.decls, i, x))
                    .and_then(|_| types().field(x))
                    .filter(|t| provable(t))
                    .map(str::to_string),
                Typed::This => class().map(|i| file.decls[i].name.clone()),
                // Typed in a package-model language only ([`types_super`]):
                // everywhere else the shape pass's `super` binds through the
                // proven hierarchy (S-522).
                Typed::Super if !self.types_super => None,
                Typed::Super => class()
                    .and_then(|i| file.symbols[i].as_ref())
                    .and_then(|class| declared_superclass(refs, class))
                    .map(str::to_string),
                // A bare call recorded as a free call (`@ref.call`); a
                // Method-form implicit call is the shape pass's.
                Typed::Implicit if refs[row].form != RefForm::Path => None,
                Typed::Implicit => class().and_then(|i| {
                    let own = declares_member(file.decls, i, name);
                    // Anything else in scope that could supply `name` — an
                    // enclosing class's member, a static import — keeps the
                    // bare row and the lexical / import rungs that bind it.
                    let elsewhere = nested(file.decls, i) || imported.contains(name) || static_wildcard;
                    (own || !elsewhere).then(|| file.decls[i].name.clone())
                }),
                Typed::Variable(_) | Typed::Pointee(_) | Typed::SelfField(_) => None,
            };
            match head {
                // Bound by the proven-receiver rung ([`proven_rung`](Self::proven_rung)).
                Some(head) if self.proven_rung() => {
                    let found = self.qualified(Proven { head, peeled: Vec::new() }, invocation, file);
                    proven.push((row, format!("{}::{name}", found.head), None));
                }
                Some(head) => typed.push((row, format!("{head}::{name}"))),
                None => {}
            }
        }
        // A receiver read inside a macro (S-610) is proven the same way.
        for site in &self.macro_sites {
            if let Some(found) = self.proven_receiver(&site.receiver, site.invocation, site.caller, file) {
                let (target, peeled) = found.into_row(&refs[site.row].target);
                proven.push((site.row, target, peeled));
            }
        }
        for (row, target) in typed {
            refs[row].target = target;
            refs[row].form = RefForm::Path;
        }
        for (row, target, peeled) in proven {
            let row = &mut refs[row];
            row.target = target;
            row.form = RefForm::Path;
            row.receiver = Some(ReceiverShape::Other);
            row.peeled = peeled;
        }
    }

    /// Whether `name`, a variable the file declares, is declared in scope at
    /// `invocation`: anywhere in the outermost callable around the call (its
    /// parameters, its locals, a local or anonymous class's members) — read by
    /// [`DeclaredTypes`] over that callable alone, so the rule for what a
    /// declaration is stays the one [`DeclaredTypes::build_with`] has — or as a
    /// field of the enclosing class, or of a class enclosing THAT one reached
    /// only through classes that inherit nothing (`supertyped`: a class
    /// recording an `extends` or `implements` row may inherit a same-named
    /// field from another file, which would shadow the outer one), or in the
    /// type a `declared` marker scopes it to (S-558: a C# primary-constructor
    /// parameter) when that type is the call's own class and inherits nothing —
    /// a member it inherits hides the parameter, and a nested type never sees
    /// it. A `@Nested` test class reading its outer class's field is the shape
    /// the outer-class walk admits.
    fn declared_in_scope(
        &self,
        invocation: Node<'_>,
        name: &str,
        file: &FileDecls<'_, '_>,
        supertyped: &HashSet<&str>,
        scopes: &mut HashMap<usize, DeclaredTypes>,
    ) -> bool {
        let inherits = |i: usize| file.symbols[i].as_ref().is_none_or(|s| supertyped.contains(s.as_str()));
        let mut class = enclosing_class(invocation, file, &self.anonymous).class();
        // A `.scope` name is in scope in the type its companion declares, and
        // only where that type is the call's own: a nested type never sees it.
        // Any member the type inherits hides it, so a type that records a
        // supertype proves nothing of it.
        if let Some(i) = class.filter(|&i| !inherits(i)) {
            let own = file.decls[i].node;
            let declares = |&(start, end): &(usize, usize)| start == own.start_byte() && end == own.end_byte();
            if self.scoped.get(name).is_some_and(|ranges| ranges.iter().any(declares)) {
                return true;
            }
        }
        while let Some(i) = class {
            if declares_field(file.decls, i, name) {
                return true;
            }
            if inherits(i) {
                break;
            }
            class = outer_class(file.decls, i);
        }
        outermost_callable(invocation).is_some_and(|callable| {
            scopes
                .entry(callable.id())
                .or_insert_with(|| DeclaredTypes::build_with(callable, file.source, &self.declarations))
                .declares(name)
        })
    }

    /// The type a Rust receiver the file may prove has at `invocation`, a call
    /// made by the declaration at `caller` (S-587, S-610): `x.f()`'s `x`, or
    /// `self.x.f()`'s field `x`.
    fn proven_receiver(
        &self,
        receiver: &Typed,
        invocation: Node<'_>,
        caller: Option<usize>,
        file: &FileDecls<'_, '_>,
    ) -> Option<Proven> {
        // `Self` is the caller's self type only where its impl names it by one
        // segment, which the caller's module scope resolves as the call's does;
        // `impl crate::m::S` may name another `S`.
        let own_type = caller
            .filter(|c| self.impl_owners.get(c).is_some_and(|o| is_identifier(strip_generics(o).trim())))
            .and_then(|c| file.decls[c].self_type.as_deref());
        let module = caller.and_then(|c| module_of(file.decls, c));
        match receiver {
            Typed::Variable(x) => self.variable_type(x, false, invocation, own_type, module, file),
            Typed::Pointee(x) => self.variable_type(x, true, invocation, own_type, module, file),
            Typed::SelfField(x) => own_type.and_then(|own| self.member_type(own, x, module, file)),
            _ => None,
        }
    }

    /// `found` with its head qualified through the file's named import of it
    /// at `invocation`, in a proven-rows language (S-537): the
    /// `<import target>::<name>` a type relation records (S-535). A head no
    /// named import brings in — one the file declares, and every Python one
    /// (S-546: the binder reads it through the file's imports) — is kept as
    /// written.
    fn qualified(&self, mut found: Proven, invocation: Node<'tree>, file: &FileDecls<'_, 'tree>) -> Proven {
        if self.proven_rows {
            if let Some(imported) = (file.imported)(invocation, &found.head) {
                found.head = imported;
            }
        }
        found
    }

    /// The node id of the class-like declaration whose members a name at
    /// `invocation` reads with no receiver written (S-563, [`Scope::Member`]):
    /// the nearest one around it — `None` when an anonymous body, or a body a
    /// `rebinds` marker names, lies nearer, since that body's own receiver may
    /// declare the name.
    fn member_owner(&self, invocation: Node<'_>, file: &FileDecls<'_, '_>) -> Option<usize> {
        let class = file.decls[enclosing_class(invocation, file, &self.anonymous).class()?].node.id();
        let mut at = invocation.parent();
        while let Some(node) = at.filter(|n| n.id() != class) {
            if self.rebinding.contains(&node.id()) {
                return None;
            }
            at = node.parent();
        }
        Some(class)
    }

    /// The type of the plain-name receiver `x` at `invocation` (S-587): the
    /// one binding of `x` in scope there, when it carries a proof. A name with
    /// no binding in scope (a `static`, a binding a macro introduced), or with
    /// two (shadowed, re-bound, two-typed), proves nothing. `own_type` is the
    /// caller's self type, what `Self` names. A call through a pointer
    /// (`pointee`, S-581) is proven by a pointer's proof alone, and every
    /// other call by any proof but a pointer's.
    fn variable_type(
        &self,
        x: &str,
        pointee: bool,
        invocation: Node<'_>,
        own_type: Option<&str>,
        module: Option<usize>,
        file: &FileDecls<'_, '_>,
    ) -> Option<Proven> {
        // A name another scope may rebind (S-546: a Python `global` or
        // `nonlocal`) proves nothing anywhere in the file.
        if self.unproven.contains(x) {
            return None;
        }
        let source = file.source;
        let callable = outermost_callable(invocation).map(|n| n.id());
        let owner: OnceCell<Option<usize>> = OnceCell::new();
        let owner = || *owner.get_or_init(|| self.member_owner(invocation, file));
        let mut in_scope = self.bindings.get(x)?.iter().filter(|(_, b)| b.in_scope(invocation, callable, owner));
        let (Some((&leaf, _)), None) = (in_scope.next(), in_scope.next()) else {
            return None;
        };
        let text = |n: Node<'_>| n.utf8_text(source).ok().map(str::trim);
        let proof = *self.proofs.get(&leaf)?;
        if pointee != matches!(proof, Proof::Pointer(_)) {
            return None;
        }
        match proof {
            // Only Rust peels a wrapper, for its impl-block lookup: anywhere
            // else `Box<T>` is a `Box`, and a pointer names its type pointed to.
            Proof::Declared(declared) | Proof::Pointer(declared) if !self.impl_blocks => {
                let head = type_head(text(declared)?, declared, own_type, source)?;
                Some(Proven { head, peeled: Vec::new() })
            }
            Proof::Declared(declared) | Proof::Pointer(declared) => {
                declared_type(text(declared)?, declared, own_type, source)
            }
            // One segment only: `E::V { … }` builds an `E`, and its text is
            // the shape of a module-qualified `m::S { … }`.
            Proof::Literal(literal) => {
                let written = strip_generics(text(literal)?);
                if written.trim().trim_end_matches("::").contains("::") {
                    return None;
                }
                let head = type_head(&written, literal, own_type, source)?;
                Some(Proven { head, peeled: Vec::new() })
            }
            Proof::Constructor(callee) => {
                let (owner, function) = strip_generics(text(callee)?).rsplit_once("::").map(|(o, f)| (o.to_string(), f.to_string()))?;
                let head = type_head(&owner, callee, own_type, source)?;
                self.constructed(&head, &function, module, file.decls)
                    .map(|peeled| Proven { head, peeled })
            }
        }
    }

    /// The wrappers a value `T::function(…)` is wrapped in, when every
    /// `function` the file declares in `module` (the caller's) on an impl
    /// naming `T` by its one segment returns `Self` or `T` under the same
    /// wrappers — `None` when it declares none, or one returning anything else
    /// (`Result<Self, E>`, `()`, another type, `Self::Output`). A qualified
    /// `a::T::function` is never proven: the impl that names `T` in this module
    /// may be another `T`'s. Read from this file alone: a `T` whose impls sit
    /// in another file or module proves nothing here.
    fn constructed(&self, head: &str, function: &str, module: Option<usize>, decls: &[Decl<'_>]) -> Option<Vec<&'static str>> {
        // Keyed by the impl type's last segment, and matched against its whole
        // one-segment text: a qualified `head` finds no entry.
        let declared = self.constructors.get(&(head.to_string(), function.to_string()))?;
        let in_module = declared.iter().filter(|(&f, _)| {
            module_of(decls, f) == module && self.impl_owners.get(&f).is_some_and(|o| strip_generics(o).trim() == head)
        });
        let mut agreed: Option<Vec<&'static str>> = None;
        for (_, returns) in in_module {
            let (rest, peeled) = peel(returns.as_deref()?);
            let returned = strip_generics(rest);
            let returned = returned.trim();
            if !(returned == "Self" || base_name(returned) == head) || agreed.as_ref().is_some_and(|a| *a != peeled) {
                return None;
            }
            agreed = Some(peeled);
        }
        agreed
    }

    /// The type of `self.field` for a caller whose self type is `own` (S-587):
    /// the one field `field` that the struct `own` declared in `module` — the
    /// caller's, whose scope its field types are written in — declares, by its
    /// declared type.
    fn member_type(&self, own: &str, field: &str, module: Option<usize>, file: &FileDecls<'_, '_>) -> Option<Proven> {
        let declared = self.members.get(&(own.to_string(), field.to_string()))?;
        let mut fields = declared
            .values()
            .filter(|(_, owner)| owner.is_some_and(|o| module_of(file.decls, o) == module));
        let (Some(&(declared, _)), None) = (fields.next(), fields.next()) else {
            return None;
        };
        declared_type(declared.utf8_text(file.source).ok()?.trim(), declared, Some(own), file.source)
    }
}

/// The module a declaration sits in: the nearest enclosing module
/// declaration (an inline `mod m { … }`), `None` for the file's own.
fn module_of(decls: &[Decl<'_>], decl: usize) -> Option<usize> {
    let mut at = decls[decl].parent;
    while let Some(i) = at {
        if decls[i].kind == NodeKind::Module {
            return Some(i);
        }
        at = decls[i].parent;
    }
    None
}

/// Every pattern leaf `pattern` binds a name with — each named leaf under it.
/// Over-reads by design: a leaf that names a constant, an enum variant or a
/// struct-pattern field is counted as a binding of that name too, which can
/// only make a receiver of that name prove less.
fn bound_names(pattern: Node<'_>) -> Vec<Node<'_>> {
    let mut leaves = Vec::new();
    let mut stack = vec![pattern];
    while let Some(n) = stack.pop() {
        if n.is_extra() {
            continue;
        }
        if n.named_child_count() == 0 {
            if n.is_named() {
                leaves.push(n);
            }
            continue;
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    leaves
}

/// Whether `text` is one identifier: a name a pattern can bind.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// A declared type's proven head and the wrappers peeled to reach it
/// ([`peel`], [`type_head`]).
fn declared_type(text: &str, at: Node<'_>, own_type: Option<&str>, source: &[u8]) -> Option<Proven> {
    let (rest, peeled) = peel(text);
    let head = type_head(rest, at, own_type, source)?;
    Some(Proven { head, peeled })
}

/// Peel a Rust type's transparent wrappers off its text, outermost first:
/// `&`, `&mut` (with or without a lifetime) and [`PEELED_WRAPPERS`] of one
/// type argument. `(&'a Arc<Store>)` → `Store`, peeled `["&", "Arc"]`. Read
/// from the text, as `dyn` is not: a recursive peel is no query pattern.
fn peel(text: &str) -> (&str, Vec<&'static str>) {
    let mut rest = text.trim();
    let mut peeled = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix('&') {
            let mut after = after.trim_start();
            if let Some(lifetime) = after.strip_prefix('\'') {
                after = lifetime.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_').trim_start();
            }
            match after.strip_prefix("mut").filter(|m| m.starts_with(char::is_whitespace)) {
                Some(inner) => {
                    peeled.push("&mut");
                    rest = inner.trim_start();
                }
                None => {
                    peeled.push("&");
                    rest = after;
                }
            }
            continue;
        }
        let Some((head, args)) = rest.split_once('<') else {
            break;
        };
        let wrapper = PEELED_WRAPPERS.into_iter().find(|w| base_name(head.trim()) == *w);
        let inner = args.trim_end().strip_suffix('>').filter(|inner| !top_level_comma(inner));
        match (wrapper, inner) {
            (Some(wrapper), Some(inner)) => {
                peeled.push(wrapper);
                rest = inner.trim();
            }
            _ => break,
        }
    }
    (rest, peeled)
}

/// Whether `args` holds a `,` outside every nested `<…>`, `(…)` or `[…]` —
/// two type arguments, where a peeled wrapper takes one.
fn top_level_comma(args: &str) -> bool {
    let mut depth = 0usize;
    for c in args.chars() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// The type a peeled declared type names, as the file writes it with its
/// generic arguments dropped (`a::Store<K>` → `a::Store`) — or `None` when it
/// names no single type a call can be typed by ([NFR-RA-05]): a reference,
/// slice, array, tuple, pointer, `fn`, `dyn` or `impl Trait` (any text that is
/// not a `::`-path of identifiers), `_`, a generic parameter of a declaration
/// around `at`, or an associated type (`Self::Item`, `G::Output`). `Self` is
/// the caller's self type `own_type`, and nothing outside an impl.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn type_head(text: &str, at: Node<'_>, own_type: Option<&str>, source: &[u8]) -> Option<String> {
    let path = strip_generics(text);
    let path = path.trim().trim_end_matches("::");
    let segments: Vec<&str> = path.split("::").map(str::trim).collect();
    if path.is_empty() || segments.iter().any(|s| !is_identifier(s)) || path == "_" {
        return None;
    }
    let first = segments[0];
    if first == "Self" {
        return own_type.filter(|_| segments.len() == 1).map(str::to_string);
    }
    if type_parameters_in_scope(at, source).contains(first) {
        return None;
    }
    Some(segments.join("::"))
}

/// `text` with every `<…>` generic argument list removed:
/// `Vec::<u8>::new` → `Vec::::new` → read by its `::` segments.
fn strip_generics(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("::::", "::")
}

/// The last `::` segment of a type path, its generic arguments dropped.
fn base_name(path: &str) -> &str {
    let head = path.split('<').next().unwrap_or(path).trim();
    head.rsplit("::").next().unwrap_or(head).trim()
}

/// What a call to `name` on the caller's own instance records, made by the
/// declaration `caller`: through its recorded self type (S-493), the Path-form
/// `Self::name`; otherwise the Method-form `name` of shape `self`. The one
/// spelling of that rule, shared by the shape pass and Rust's macro-argument
/// walker.
///
/// The `Self::name` row keeps the shape `self` when the call wrote the `self`
/// keyword (`keyword`: the query captured it `@ref.method.self`, or the macro
/// walker read `self.f()`) — a language whose path twin `Self::name()` can be
/// written too, and records no shape, so the method syntax stays readable
/// (S-606, [CR-202] F2) and a caller writing both has two rows. A call shaped
/// `self` by a named receiver (Go's `s.m()`, S-509) has no path twin and
/// records the shapeless row it did before.
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
pub(super) fn self_call(caller: Option<&Decl<'_>>, name: &str, keyword: bool) -> (String, RefForm, Option<ReceiverShape>) {
    if caller.is_some_and(|d| d.self_type.is_some()) {
        let shape = keyword.then_some(ReceiverShape::SelfInstance);
        (format!("{SELF_TYPE_HEAD}::{name}"), RefForm::Path, shape)
    } else {
        (name.to_string(), RefForm::Method, Some(ReceiverShape::SelfInstance))
    }
}

/// Whether the declared type `t` names a type a call at `invocation` can be
/// typed by: one identifier (not an array, whose recorded name keeps its
/// `[]`; not a union), not `var` (Java's and C#'s implicit type), and not a
/// type parameter of any declaration enclosing the call — a type variable is
/// no type ([NFR-RA-05]).
///
/// The type-parameter scan ([`type_parameters_in_scope`]) reads the grammar's
/// `type_parameters` field, which Java, TypeScript, Rust and C# all name so
/// (C#'s class, struct and record declarations hold an unnamed
/// `type_parameter_list` instead, which it also reads, and Scala's list
/// field-names each parameter `name`, a `def`'s sitting in its `parameters`
/// field, S-576, and a C++ template names its `template_parameter_list`
/// `parameters`, S-581); a grammar that names it otherwise excludes no type variable,
/// and a typing story for it says so.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn provable_type(t: &str, invocation: Node<'_>, source: &[u8]) -> bool {
    let mut chars = t.chars();
    let identifier = chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    identifier && t != "var" && !type_parameters_in_scope(invocation, source).contains(t)
}

/// Where `invocation` sits: in the class-like declaration nearest it, in an
/// anonymous class body (the `anonymous` marker) nearer than any — whose
/// `this` has no name the file can write — or in no class at all.
fn enclosing_class(
    invocation: Node<'_>,
    file: &FileDecls<'_, '_>,
    anonymous: &HashSet<usize>,
) -> Enclosing {
    let mut at = invocation.parent();
    while let Some(node) = at {
        if anonymous.contains(&node.id()) {
            return Enclosing::Anonymous(node.id());
        }
        if let Some(&i) = file.id_to_idx.get(&node.id()) {
            if is_class_like(file.decls[i].kind) {
                return Enclosing::Class(i);
            }
        }
        at = node.parent();
    }
    Enclosing::Outside
}

/// The class-like declaration enclosing the one at `class`, if any.
pub(super) fn outer_class(decls: &[Decl<'_>], class: usize) -> Option<usize> {
    let mut at = decls[class].parent;
    while let Some(i) = at {
        if is_class_like(decls[i].kind) {
            return Some(i);
        }
        at = decls[i].parent;
    }
    None
}

/// Whether the class at `class` declares a field named `name` itself.
pub(super) fn declares_field(decls: &[Decl<'_>], class: usize, name: &str) -> bool {
    decls
        .iter()
        .any(|d| d.parent == Some(class) && d.name == name && d.kind == NodeKind::Field)
}

/// Whether the class at `class` declares a callable named `name` itself.
fn declares_member(decls: &[Decl<'_>], class: usize, name: &str) -> bool {
    decls.iter().any(|d| {
        d.parent == Some(class) && d.name == name && matches!(d.kind, NodeKind::Method | NodeKind::Function)
    })
}

/// Whether a callable between the call and its enclosing class — the caller
/// itself, or a callable enclosing it inside that class — declares a callable
/// named `name` whose symbol built: a Scala nested `def`, a Kotlin or C# local
/// function. A bare call names that local callable, never the class's member.
fn declared_locally(file: &FileDecls<'_, '_>, caller: Option<usize>, class: usize, name: &str) -> bool {
    let mut scope = caller;
    while let Some(s) = scope.filter(|&s| s != class) {
        let declares = file.decls.iter().enumerate().any(|(i, d)| {
            d.parent == Some(s)
                && d.name == name
                && matches!(d.kind, NodeKind::Method | NodeKind::Function)
                && file.symbols[i].is_some()
        });
        if declares {
            return true;
        }
        scope = file.decls[s].parent;
    }
    false
}

/// Whether the anonymous class body with node id `body` itself declares a
/// callable named `name` whose symbol built — the one a `self` call inside it
/// names (S-514).
fn declares_in(file: &FileDecls<'_, '_>, body: usize, name: &str) -> bool {
    file.decls.iter().enumerate().any(|(i, d)| {
        d.node.parent().is_some_and(|p| p.id() == body)
            && d.name == name
            && matches!(d.kind, NodeKind::Method | NodeKind::Function)
            && file.symbols[i].is_some()
    })
}

/// Whether the class at `class` sits inside another class-like declaration —
/// a nested, inner or local class, whose bare calls the outer class's members
/// can also answer.
fn nested(decls: &[Decl<'_>], class: usize) -> bool {
    outer_class(decls, class).is_some()
}

/// Whether the file declares a class-like type named `name`.
fn declares_type(decls: &[Decl<'_>], name: &str) -> bool {
    decls.iter().any(|d| is_class_like(d.kind) && d.name == name)
}

#[cfg(all(test, feature = "lang-rust"))]
#[path = "receiver_tests.rs"]
mod tests;
