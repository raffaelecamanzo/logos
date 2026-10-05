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
//! | `name` | `x.send()` | [`DeclaredTypes::get`]: the one type the file declares `x` with, where `x` is declared in scope at the call (a field of the enclosing class, or in the callable around it); a name the file never declares as a variable is a static type name when its single-type import or its own declaration names that type |
//! | `field` | `this.x.send()` | [`DeclaredTypes::field`] (S-398), where the enclosing class declares the field `x` itself |
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
//! prove types through [`DeclaredTypes`], whose declaration reading has only
//! been measured on Java. A second grammar uses the shape markers below; it
//! opts into typing in a typing story of its own.
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
//! | `implicit` | the name of a call written with no receiver, on a `@ref.method` row | per the plugin's `implicit_receiver` policy ([`ImplicitReceiver`]): `self` inside a named class under `"self"` — unless a callable between the call and that class declares the name (a nested `def`, a local function), which shadows the member; otherwise the row becomes the Path-form free call `@ref.call` records, which the lexical scope binds |
//! | `self_name` | the identifier a declaration binds its own instance to (Go's receiver parameter) | nothing; read by `other` |
//! | `anonymous` | a body whose `self`/`this` is not an instance of the enclosing named class: an anonymous class body (`new T() { … }`, Kotlin `object : T { … }`, Scala `new T { … }`, PHP `new class { … }`), a TS class expression, a TS/JS object-literal method or non-arrow `function`, a Kotlin extension function's body, a Ruby `module`, `def self.x`, `class << self` or `Struct.new` block | nothing; typing stops there, and a `self` / `super` / `this` call inside one records `other` — except a `self` call to a callable that body declares itself, which becomes the free call the lexical scope binds to it |
//!
//! Java's typing markers map onto the lexicon: `this` is `self`; `name`,
//! `field` and `refused` are `other`. A `self` call whose caller declares a
//! self type (S-493, `@symbol.self_type`) is recorded as the Path-form
//! `Self::send` — the row a written `Self::send()` records, bound through that
//! type ([FR-RS-11]); the S-493 capture `@ref.method.self` is a `self`-marked
//! `@ref.method`.
//!
//! **How a marker finds its call.** The captured node's parent is the node the
//! call's `@ref.method` / `@ref.call` capture also has for parent — the member
//! or invocation expression both sit in (`this.send()` → the `this` node and the
//! `send` node share it). For `field` it is the grandparent. When markers
//! disagree: `refused` makes the call `other`; `self` and `super` together make
//! it `other`; either beats `other`, which beats `implicit`.
//!
//! [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
//! [CR-169]: ../../../docs/requests/CR-169-a-call-on-another-object-never-binds-to-the-callers-own-method.md
//! [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
//! [FR-RS-08]: ../../../docs/specs/requirements/FR-RS-08.md
//! [FR-RS-11]: ../../../docs/specs/requirements/FR-RS-11.md
//! [FR-RS-12]: ../../../docs/specs/requirements/FR-RS-12.md

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::config::accessor::{outermost_callable, DeclaredTypes};
use super::{
    declared_superclass, type_parameters_in_scope, Decl, RefFact, SELF_RECEIVER_METHOD_CAPTURE,
};
use crate::model::{EdgeKind, NodeKind, ReceiverShape, RefForm};
use crate::plugin::ImplicitReceiver;
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
    unproven: HashSet<String>,
    /// The names the file's NON-static single-type imports bring into scope — a
    /// static import names a member, never a type.
    type_imports: HashSet<String>,
    /// The bodies the `anonymous` marker names, by node id — anonymous class
    /// bodies and every other body whose `this` is not the enclosing class's.
    anonymous: HashSet<usize>,
    /// declaration index → the name it binds its own instance to (`self_name`);
    /// `None` when two of its markers disagree.
    self_names: HashMap<usize, Option<String>>,
    /// Every row a receiver could retype or shape.
    sites: Vec<Site<'tree>>,
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
}

/// What a file's declarations say, for the receiver passes.
pub(super) struct FileDecls<'a, 'tree> {
    pub root: Node<'tree>,
    pub source: &'a [u8],
    pub decls: &'a [Decl<'tree>],
    pub symbols: &'a [Option<crate::model::LogosSymbol>],
    pub id_to_idx: &'a HashMap<usize, usize>,
}

/// Whether `capture` is a receiver marker.
pub(super) fn is_marker(capture: &str) -> bool {
    capture.starts_with(MARKER)
}

impl<'tree> Receivers<'tree> {
    /// The receiver state for a query, or [`None`] when it declares no marker
    /// and no `@ref.method.self` — then nothing is recorded and no row is ever
    /// retyped or shaped.
    pub(super) fn for_query(
        capture_names: &[&str],
        implicit_receiver: ImplicitReceiver,
        types_super: bool,
    ) -> Option<Self> {
        capture_names
            .iter()
            .any(|c| is_marker(c) || *c == SELF_RECEIVER_METHOD_CAPTURE)
            .then(|| Self {
                marks: HashMap::new(),
                unproven: HashSet::new(),
                type_imports: HashSet::new(),
                anonymous: HashSet::new(),
                self_names: HashMap::new(),
                sites: Vec::new(),
                implicit_receiver,
                types_super,
            })
    }

    /// Record one marker capture, matched on the whole capture name — never on
    /// a node kind or a grammar field. `declaration` is the innermost
    /// declaration enclosing the captured node, asked only by `self_name`.
    pub(super) fn mark(
        &mut self,
        capture: &str,
        node: Node<'tree>,
        source: &[u8],
        declaration: impl FnOnce() -> Option<usize>,
    ) {
        let text = || node.utf8_text(source).ok().map(|t| t.trim().to_string());
        match capture {
            "ref.receiver.unproven" => {
                self.unproven.extend(text());
                return;
            }
            "ref.receiver.anonymous" => {
                self.anonymous.insert(node.id());
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
            "ref.receiver.field" => node.parent().and_then(|p| p.parent()),
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
            "ref.receiver.self" => marks.own = true,
            "ref.receiver.other" => {
                marks.other = true;
                marks.other_text = text();
            }
            _ => {}
        }
    }

    /// Mark `invocation`'s receiver as the caller's own instance — what the
    /// S-493 capture `@ref.method.self` says of its call.
    pub(super) fn mark_self(&mut self, invocation: Option<Node<'tree>>) {
        if let Some(invocation) = invocation {
            self.marks.entry(invocation.id()).or_default().own = true;
        }
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

    /// Note a non-static single-type import's name.
    pub(super) fn type_import(&mut self, name: &str) {
        self.type_imports.insert(name.to_string());
    }

    /// Retype every site whose receiver the file proves, then shape every
    /// Method-form row typing left bare, in place. Runs once, after every
    /// match: a `super` receiver reads the `Extends` rows this same pass
    /// recorded, and a bare call the file's imports.
    pub(super) fn finish(self, refs: &mut [RefFact], file: &FileDecls<'_, 'tree>) {
        if self.sites.is_empty() {
            return;
        }
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
                    (row.target, row.form, row.receiver) = self_call(caller, &row.target);
                }
                Recorded::Shape(shape) => row.receiver = Some(shape),
                Recorded::FreeCall => row.form = RefForm::Path,
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
            let own_name = site
                .caller
                .and_then(|c| self.self_names.get(&c))
                .and_then(Option::as_deref);
            let own = own_name.is_some() && marks.other_text.as_deref() == own_name;
            return Recorded::Shape(if own {
                ReceiverShape::SelfInstance
            } else {
                ReceiverShape::Other
            });
        }
        if marks.implicit {
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

    /// Retype every site whose receiver the file proves, in place (S-467).
    fn retype(&self, refs: &mut [RefFact], file: &FileDecls<'_, 'tree>) {
        // The one per-file walk this costs, paid only by a file with a site
        // that asks it (a simple-name or `this.x` receiver) — NFR-PE-02.
        let declared: OnceCell<DeclaredTypes> = OnceCell::new();
        let types = || declared.get_or_init(|| DeclaredTypes::build(file.root, file.source));
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
        let mut typed: Vec<(usize, String)> = Vec::new();
        for site in &self.sites {
            let (row, invocation) = (site.row, site.invocation);
            let Some(marks) = self.marks.get(&invocation.id()) else {
                continue;
            };
            let Some(receiver) = marks.typed.as_ref().filter(|_| !marks.refused) else {
                continue;
            };
            let name = refs[row].target.as_str();
            let provable = |t: &str| provable_type(t, invocation, file.source);
            let class = || enclosing_class(invocation, file, anonymous).class();
            let head = match receiver {
                Typed::Name(x) if self.unproven.contains(x) => None,
                // A variable: typed only where it is declared in scope at the
                // call — the file-wide type is then its type, because every
                // declaration of the name in the file agrees. Declared nowhere
                // in scope, it is inherited or an outer class's: unproven.
                Typed::Name(x) if types().declares(x) => {
                    declared_in_scope(invocation, x, file, anonymous, &supertyped, &mut scopes)
                        .then(|| types().get(x))
                        .flatten()
                        .filter(|t| provable(t))
                        .map(str::to_string)
                }
                Typed::Name(x) => (self.type_imports.contains(x) || declares_type(file.decls, x)).then(|| x.clone()),
                // `this.x` is the enclosing class's field only when that class
                // declares it; an inherited `x` is another class's.
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
            };
            if let Some(head) = head {
                typed.push((row, format!("{head}::{name}")));
            }
        }
        for (row, target) in typed {
            refs[row].target = target;
            refs[row].form = RefForm::Path;
        }
    }
}

/// What a call to `name` on the caller's own instance records, made by the
/// declaration `caller`: through its recorded self type (S-493), the Path-form
/// `Self::name` — the row a written `Self::name()` records; otherwise the
/// Method-form `name` of shape `self`. The one spelling of that rule, shared
/// by the shape pass and Rust's macro-argument walker.
pub(super) fn self_call(caller: Option<&Decl<'_>>, name: &str) -> (String, RefForm, Option<ReceiverShape>) {
    if caller.is_some_and(|d| d.self_type.is_some()) {
        (format!("{SELF_TYPE_HEAD}::{name}"), RefForm::Path, None)
    } else {
        (name.to_string(), RefForm::Method, Some(ReceiverShape::SelfInstance))
    }
}

/// Whether the declared type `t` names a type a call at `invocation` can be
/// typed by: one identifier (not an array, whose recorded name keeps its
/// `[]`; not a union), not Java's `var`, and not a type parameter of any
/// declaration enclosing the call — a type variable is no type ([NFR-RA-05]).
///
/// The type-parameter scan ([`type_parameters_in_scope`]) reads the grammar's
/// `type_parameters` field, which Java, TypeScript, Rust and C# all name so; a
/// grammar that names it otherwise excludes no type variable, and a typing
/// story for it says so. No other language names a type `var`.
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

/// Whether `name`, a variable the file declares, is declared in scope at
/// `invocation`: anywhere in the outermost callable around the call (its
/// parameters, its locals, a local or anonymous class's members) — read by
/// [`DeclaredTypes`] over that callable alone, so the rule for what a
/// declaration is stays the one [`DeclaredTypes::build`] has — or as a field of
/// the enclosing class, or of a class enclosing THAT one reached only through
/// classes that inherit nothing (`supertyped`: a class recording an `extends`
/// or `implements` row may inherit a same-named field from another file, which
/// would shadow the outer one). A `@Nested` test class reading its outer
/// class's field is the shape this admits.
fn declared_in_scope(
    invocation: Node<'_>,
    name: &str,
    file: &FileDecls<'_, '_>,
    anonymous: &HashSet<usize>,
    supertyped: &HashSet<&str>,
    scopes: &mut HashMap<usize, DeclaredTypes>,
) -> bool {
    let mut class = enclosing_class(invocation, file, anonymous).class();
    while let Some(i) = class {
        if declares_field(file.decls, i, name) {
            return true;
        }
        let inherits = file.symbols[i]
            .as_ref()
            .is_none_or(|s| supertyped.contains(s.as_str()));
        if inherits {
            break;
        }
        class = outer_class(file.decls, i);
    }
    outermost_callable(invocation).is_some_and(|callable| {
        scopes
            .entry(callable.id())
            .or_insert_with(|| DeclaredTypes::build(callable, file.source))
            .declares(name)
    })
}

/// The class-like declaration enclosing the one at `class`, if any.
fn outer_class(decls: &[Decl<'_>], class: usize) -> Option<usize> {
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
fn declares_field(decls: &[Decl<'_>], class: usize, name: &str) -> bool {
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
