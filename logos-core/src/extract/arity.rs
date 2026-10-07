//! A callable's **parameter range** and a call's **argument count** (S-591,
//! [CR-190], [FR-EX-32]), and whether a Rust `impl` function takes `self`
//! ([CR-200]).
//!
//! Both facts are read off the parse tree while it is in hand, from captures a
//! plugin's own queries declare — no language is named here ([NFR-MA-01]). A
//! plugin that declares none records **unknown** for every callable and every
//! call, and an unknown fact never filters a candidate ([FR-RS-43]).
//!
//! # The `symbols`-query vocabulary — a callable's parameters
//!
//! - `@arity.parameters` — a callable's parameter list (a lone parameter written
//!   without one, the `x` of `x => …`, may be captured as both the list and its
//!   parameter). Owned by the nearest enclosing declaration, and only when it
//!   sits outside that declaration's body; a declaration owning two lists (Scala
//!   `def f(a)(b)`) takes the first, the one a call's first argument list fills.
//! - `@arity.required` — a parameter every call must pass.
//! - `@arity.optional` — a parameter with a default value: it raises only `max`.
//! - `@arity.variadic` — `...`, `params`, `vararg`, `*args`, `**kw`: `max` is
//!   unbounded.
//! - `@arity.receiver` — a receiver written in the list (Rust `self`, Python's
//!   first method parameter, Java's `C this`, TS `this: C`): not counted.
//! - `@arity.skip` — a list child that is no parameter (an attribute, a
//!   modifier, a default value written beside its parameter, C's `(void)`).
//! - `@arity.unknown` — a form the plugin cannot count (a Scala `using` clause,
//!   a C# extension method): the range is unknown.
//!
//! A class capture belongs to the list that is its nearest ancestor-or-self of
//! a captured list's kind, so a function-typed parameter's own parameter list
//! (`cb: fn(i32)`) never adds to the outer count. Each named list child must be
//! covered by a capture — the child itself, or a node beneath it (Go's
//! `a, b int` is one child naming two parameters, each name captured) — or the
//! range is unknown rather than miscounted. Where captures disagree on one
//! child the strongest wins: `unknown` > `variadic` > `receiver` > `optional` >
//! `skip` > `required`, so a query may capture every parameter `required` and
//! refine the defaulted ones — and a first parameter that is variadic (Python's
//! `def m(*args: int)`, whose `args` holds `self`) is never mistaken for a lone
//! receiver.
//!
//! # The `references`-query vocabulary — a call's arguments
//!
//! - `@arity.arguments` — an argument list; its named children are counted.
//! - `@arity.block` — an argument list of exactly one argument (Scala's
//!   `f { … }`, Python's lone generator `f(x for x in xs)`).
//! - `@arity.lambda` — a trailing lambda (Kotlin's `f(1) { … }`): one more
//!   argument of the call it trails.
//! - `@arity.spread` — a spread argument (`*xs`, `...xs`, `xs: _*`): the count
//!   is unknown.
//! - `@arity.skip` — an argument-list child that is no argument (a Ruby
//!   `&block`).
//! - `@arity.none` — a call written with no argument list (Ruby's `x.m`): it
//!   passes none.
//! - `@arity.opaque` — an argument form the plugin cannot count (a tagged
//!   template's `` tag`…` ``): the count is unknown.
//! - `@arity.unknown` — an argument list whose count says nothing about the
//!   callee's parameters (S-592: Python's `cls.m(…)`, which passes the instance
//!   itself to an instance method but not to a class method): the count is
//!   unknown.
//! - `@arity.receiver` — a call's receiver written as its direct child (Ruby's
//!   `User` in `User.find(1)`, Java's `object`): a row captured there is no
//!   callee of that call, and records unknown.
//!
//! A call row's count is read from the call its captured callee is written in:
//! the nearest ancestor holding an argument capture beside the callee. Its first
//! argument list only — a Scala `f(a)(b)` passes one — plus the trailing
//! lambdas of each enclosing call that takes it as its callee and has no list
//! of its own. A callee inside an argument list, or one no call encloses before
//! a declaration does, records unknown.
//!
//! [CR-190]: ../../../docs/requests/CR-190-a-self-call-binds-only-a-callable-whose-arity-admits-it.md
//! [CR-200]: ../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
//! [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
//! [FR-RS-43]: ../../../docs/specs/requirements/FR-RS-43.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::{BTreeMap, HashMap, HashSet};

use tree_sitter::Node;

use crate::model::ParamRange;

use super::shape;
use super::Decl;

/// The capture-name prefix of the arity vocabulary in both queries.
const GROUP: &str = "arity.";

/// One parameter-list child's class, weakest first: the derived `Ord` is the
/// precedence a child captured twice is decided by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Required,
    Skip,
    Optional,
    Receiver,
    Variadic,
    Unknown,
}

impl Class {
    fn of(name: &str) -> Option<Self> {
        Some(match name {
            "required" => Class::Required,
            "skip" => Class::Skip,
            "optional" => Class::Optional,
            "variadic" => Class::Variadic,
            "receiver" => Class::Receiver,
            "unknown" => Class::Unknown,
            _ => return None,
        })
    }
}

/// What one callable's parameter list says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Params {
    /// The argument counts a call may pass; `None` when a child is uncovered or
    /// captured `unknown`.
    pub(super) range: Option<ParamRange>,
    /// Whether the list writes a receiver (`@arity.receiver`).
    pub(super) receiver: bool,
}

/// The `symbols`-query arity captures of one file, gathered during the
/// declaration walk ([`note`](Self::note)) and resolved against the
/// declarations once it ends ([`per_decl`](Self::per_decl)).
#[derive(Default)]
pub(super) struct ParamCaptures<'tree> {
    lists: Vec<Node<'tree>>,
    classes: Vec<(Node<'tree>, Class)>,
}

impl<'tree> ParamCaptures<'tree> {
    /// Record `capture` when it belongs to the vocabulary; `false` for any
    /// other capture, which the caller then reads itself.
    pub(super) fn note(&mut self, capture: &str, node: Node<'tree>) -> bool {
        let Some(name) = capture.strip_prefix(GROUP) else {
            return false;
        };
        if name == "parameters" {
            self.lists.push(node);
        } else if let Some(class) = Class::of(name) {
            self.classes.push((node, class));
        }
        true
    }

    /// The parameters of each declaration, by index into `decls`: `None` for a
    /// declaration owning no captured list.
    pub(super) fn per_decl(&self, decls: &[Decl<'tree>], body_kinds: &[String]) -> Vec<Option<Params>> {
        let mut out: Vec<Option<Params>> = vec![None; decls.len()];
        if self.lists.is_empty() {
            return out;
        }
        let decl_at: HashMap<usize, usize> = decls.iter().enumerate().map(|(i, d)| (d.node.id(), i)).collect();
        let list_ids: HashSet<usize> = self.lists.iter().map(Node::id).collect();
        let list_kinds: HashSet<&str> = self.lists.iter().map(Node::kind).collect();

        // Each list child's captures: list id → slot id → class → captured nodes.
        // A slot is the list's child the capture sits in (or the list itself,
        // for a lone parameter captured as its own list).
        type Slots = BTreeMap<usize, BTreeMap<Class, HashSet<usize>>>;
        let mut slots: HashMap<usize, Slots> = HashMap::new();
        for &(node, class) in &self.classes {
            let mut slot = node;
            let mut owner = Some(node);
            while let Some(n) = owner {
                if list_kinds.contains(n.kind()) {
                    break;
                }
                slot = n;
                owner = n.parent();
            }
            let Some(list) = owner.filter(|l| list_ids.contains(&l.id())) else {
                continue; // an uncaptured list's parameter (a function type's)
            };
            slots
                .entry(list.id())
                .or_default()
                .entry(slot.id())
                .or_default()
                .entry(class)
                .or_default()
                .insert(node.id());
        }

        // Each declaration's first own list.
        let mut first: HashMap<usize, Node<'tree>> = HashMap::new();
        for &list in &self.lists {
            let Some(idx) = owning_decl(list, &decl_at) else {
                continue;
            };
            let decl = decls[idx].node;
            let in_body = shape::callable_body(decl, body_kinds)
                .is_some_and(|body| body.id() != decl.id() && list.start_byte() >= body.start_byte());
            if in_body {
                continue; // a nested callable's list, not the declaration's own
            }
            let entry = first.entry(idx).or_insert(list);
            if list.start_byte() < entry.start_byte() {
                *entry = list;
            }
        }
        let empty = Slots::new();
        for (idx, list) in first {
            out[idx] = Some(params(list, slots.get(&list.id()).unwrap_or(&empty)));
        }
        out
    }
}

/// The nearest captured declaration enclosing `list` — a parameter list, or
/// (for [`super::assoc`]) a receiver or an enum's variant list, none of which
/// is itself a declaration.
pub(super) fn owning_decl(list: Node<'_>, decl_at: &HashMap<usize, usize>) -> Option<usize> {
    let mut ancestor = list.parent();
    while let Some(n) = ancestor {
        if let Some(&idx) = decl_at.get(&n.id()) {
            return Some(idx);
        }
        ancestor = n.parent();
    }
    None
}

/// The range and receiver of one list from its slots' captures.
fn params(list: Node<'_>, slots: &BTreeMap<usize, BTreeMap<Class, HashSet<usize>>>) -> Params {
    let mut cursor = list.walk();
    let covered = list
        .named_children(&mut cursor)
        .filter(|c| !c.is_extra())
        .all(|c| slots.contains_key(&c.id()));
    let mut required = 0u32;
    let mut optional = 0u32;
    let mut variadic = false;
    let mut receiver = false;
    let mut unknown = !covered;
    for classes in slots.values() {
        // The strongest class decides the slot; its distinct nodes are how many
        // parameters the slot writes.
        let Some((&class, nodes)) = classes.iter().next_back() else {
            continue;
        };
        let n = u32::try_from(nodes.len()).unwrap_or(u32::MAX);
        match class {
            Class::Required => required = required.saturating_add(n),
            Class::Optional => optional = optional.saturating_add(n),
            Class::Variadic => variadic = true,
            Class::Receiver => receiver = true,
            Class::Skip => {}
            Class::Unknown => unknown = true,
        }
    }
    let range = (!unknown).then(|| ParamRange {
        min: required,
        max: (!variadic).then(|| required.saturating_add(optional)),
    });
    Params { range, receiver }
}

/// The `references`-query arity captures of one file, gathered during the
/// reference walk ([`note`](Self::note)) and read once it ends
/// ([`count`](Self::count)).
#[derive(Default)]
pub(super) struct ArgCaptures {
    lists: HashSet<usize>,
    blocks: HashSet<usize>,
    lambdas: HashSet<usize>,
    skips: HashSet<usize>,
    nones: HashSet<usize>,
    opaques: HashSet<usize>,
    receivers: HashSet<usize>,
    /// The argument lists a spread argument makes uncountable.
    spread_lists: HashSet<usize>,
}

impl ArgCaptures {
    /// Record `capture` when it belongs to the vocabulary; `false` for any
    /// other capture, which the caller then reads itself.
    pub(super) fn note(&mut self, capture: &str, node: Node<'_>) -> bool {
        let Some(name) = capture.strip_prefix(GROUP) else {
            return false;
        };
        let id = node.id();
        match name {
            "arguments" => {
                self.lists.insert(id);
            }
            "block" => {
                self.blocks.insert(id);
            }
            "lambda" => {
                self.lambdas.insert(id);
            }
            "skip" => {
                self.skips.insert(id);
            }
            "none" => {
                self.nones.insert(id);
            }
            "opaque" => {
                self.opaques.insert(id);
            }
            "receiver" => {
                self.receivers.insert(id);
            }
            "spread" => {
                if let Some(list) = node.parent() {
                    self.spread_lists.insert(list.id());
                }
            }
            "unknown" => {
                self.spread_lists.insert(id);
            }
            _ => {}
        }
        true
    }

    /// The argument count of the call `callee` is written in, or `None` when
    /// it cannot be counted. `is_decl` names a captured declaration: no call
    /// encloses a callee beyond one.
    pub(super) fn count(&self, callee: Node<'_>, is_decl: impl Fn(Node<'_>) -> bool) -> Option<u32> {
        let mut node = callee;
        loop {
            let parent = node.parent()?;
            let id = parent.id();
            if self.lists.contains(&id) || self.blocks.contains(&id) || is_decl(parent) {
                return None; // an argument, or no call before the declaration
            }
            if let Some(count) = self.arguments_of(parent, node)? {
                // A row captured in the call's receiver is not its callee.
                return (!self.receivers.contains(&node.id())).then(|| self.with_trailing_lambdas(parent, count));
            }
            if self.nones.contains(&id) {
                return Some(0);
            }
            node = parent;
        }
    }

    /// The arguments `call` passes beside its callee `callee`: `Some(None)` when
    /// `call` holds no argument capture (it is not the call), `None` when it
    /// does and they cannot be counted.
    fn arguments_of(&self, call: Node<'_>, callee: Node<'_>) -> Option<Option<u32>> {
        let mut count = 0u32;
        let mut found = false;
        let mut cursor = call.walk();
        // A further argument list is a further application written as an
        // enclosing call (Scala's `f(a)(b)` applies `f(a)`'s result), so the
        // lists read here are the first one and any lambda written beside it.
        for child in call.named_children(&mut cursor).filter(|c| c.id() != callee.id()) {
            let id = child.id();
            if self.opaques.contains(&id) && !self.lists.contains(&id) && !self.blocks.contains(&id) {
                return None;
            }
            if self.lists.contains(&id) {
                count = count.saturating_add(self.list_count(child)?);
                found = true;
            } else if self.blocks.contains(&id) || self.lambdas.contains(&id) {
                count = count.saturating_add(1);
                found = true;
            }
        }
        Some(found.then_some(count))
    }

    /// The arguments one list holds: its named children less the skipped
    /// ones, or `None` when a spread makes the count unknown.
    fn list_count(&self, list: Node<'_>) -> Option<u32> {
        if self.spread_lists.contains(&list.id()) {
            return None;
        }
        let mut cursor = list.walk();
        let n = list
            .named_children(&mut cursor)
            .filter(|c| !c.is_extra() && !self.skips.contains(&c.id()))
            .count();
        u32::try_from(n).ok()
    }

    /// `count` plus the trailing lambdas of each call that takes `call` as its
    /// callee and writes no argument list of its own (Kotlin's `f(1) { … }`).
    fn with_trailing_lambdas(&self, call: Node<'_>, count: u32) -> u32 {
        let mut call = call;
        let mut count = count;
        while let Some(outer) = call.parent() {
            if outer.named_child(0).map(|c| c.id()) != Some(call.id()) {
                break;
            }
            let mut cursor = outer.walk();
            let (mut lambdas, mut lists) = (0u32, false);
            for child in outer.named_children(&mut cursor).skip(1) {
                let id = child.id();
                lambdas += u32::from(self.lambdas.contains(&id));
                lists |= self.lists.contains(&id) || self.blocks.contains(&id) || self.opaques.contains(&id);
            }
            if lists || lambdas == 0 {
                break;
            }
            count = count.saturating_add(lambdas);
            call = outer;
        }
        count
    }
}

#[cfg(test)]
#[path = "arity_tests.rs"]
mod tests;
