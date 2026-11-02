//! Which declarations a bare call can reach (S-560, S-590, CR-068,
//! [FR-RS-07]): a free function over same-named methods, never an instance
//! member its language's bare call cannot reach, never an extension it cannot
//! be shown to reach ([`CallReach::bare_call_cannot_reach`]).
//!
//! [FR-RS-07]: ../../../../docs/specs/requirements/FR-RS-07.md

use std::ops::Deref;

use crate::model::{NodeId, NodeKind};

use super::{is_class_like, Ctx, Index, Residue, Want, MAX_CONTAINS_DEPTH};
use super::extension_calls::ExtensionCalls;

/// Which declarations a bare call can reach.
#[derive(Clone, Copy)]
pub(super) struct CallReach<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for CallReach<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl CallReach<'_, '_> {
    /// CR-068 Part B bare-path method exclusion, expressed as a **tie-break**
    /// ([FR-RS-07]): while resolving a single-segment bare-path call (gated on
    /// [`bare_path_call`](Ctx::bare_path_call)), a free [`NodeKind::Function`] at a
    /// scope outranks same-named [`NodeKind::Method`]s there. So a same-module bare
    /// call binds
    /// the one free function even when same-named associated methods collapse to
    /// that module scope (`impl` is not a captured scope, [`is_type_like`](super::is_type_like)) — the
    /// `graph_store` `insert_node`/`insert_edge`/`upsert_symbol` cluster the graph
    /// previously left [`Res::Ambiguous`](super::Res::Ambiguous).
    ///
    /// In a language that does not declare `implicit_receiver = "none"`, a
    /// **tie-break, not a filter**: methods are dropped only when a free
    /// function is actually present. So there it is strictly monotonic and
    /// never-fabricate ([NFR-RA-05]):
    /// - one free fn + same-named methods → binds the free fn (the recovery);
    /// - two-or-more free fns → still ambiguous, stays unresolved;
    /// - no free fn (a language whose free callables are `Method`, e.g. a Ruby
    ///   top-level `def`, or a lone associated method) → the full callable set
    ///   stands, so no previously-resolved edge is lost.
    ///
    /// Only methods are dropped. A class or macro a [`Want::DeclaredCall`]
    /// admits (S-521) stays beside the free function, so a function and a class
    /// of one name in one scope are two candidates here exactly as on every
    /// other rung. Under [`Want::Callable`] the candidates are functions and
    /// methods alone, so dropping the methods is what keeping the functions was.
    ///
    /// Path-qualified (`Type::f` via [`descend`](super::scope_hierarchy::ScopeHierarchy::descend)) and typed calls
    /// never reach this step, and a receiver-method call ([`RefForm::Method`](crate::model::RefForm::Method))
    /// never reaches the scope walk at all — it binds by its receiver's shape
    /// (S-514). The step is gated on [`bare_path_call`](Ctx::bare_path_call).
    ///
    /// In a language that declares `implicit_receiver = "none"` explicitly
    /// (S-590), the step is first a **filter**: every
    /// [unreachable member](CallReach::is_unreachable_member) is dropped, so a scope
    /// holding only the method(s) contributes nothing and the walk goes on
    /// outward to a free function — or ends unbound. A bare `f()` inside a method
    /// `f` then makes no self-loop. Java declares nothing, and keeps the
    /// tie-break alone: its bare in-class call does mean `this.m()`.
    ///
    /// [FR-RS-07]: ../../../../docs/specs/requirements/FR-RS-07.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn prefer_free_functions(&self, mut candidates: Vec<NodeId>) -> Vec<NodeId> {
        if !self.bare_path_call.get() {
            return candidates;
        }
        candidates.retain(|&id| !self.is_unreachable_member(id));
        let kind = |id: &NodeId| self.ix.info.get(id).map(|i| i.kind);
        if candidates.iter().any(|id| kind(id) == Some(NodeKind::Function)) {
            candidates.retain(|id| kind(id) != Some(NodeKind::Method));
        }
        candidates
    }

    /// Whether `id` is a declaration a bare call written in the source's
    /// language can never reach (S-590, [FR-RS-07], [FR-RS-12]): the language
    /// declares `implicit_receiver = "none"` explicitly, and the candidate is an
    /// instance member — either
    /// - (a) a member of a class-like container ([`is_class_like`]): a Python
    ///   or PHP method, a JS/TS class member, a class a Python class nests (a
    ///   method's bare name never sees class scope); or
    /// - (b) a declaration with a recorded self type
    ///   ([`IndexBuilder::with_self_types`](super::IndexBuilder::with_self_types), [FR-RS-11] / [FR-EX-12]): a Go or Rust
    ///   method, which sits at module scope beside the free functions — or,
    ///   in an impl-block language, any function of a recorded `impl` block
    ///   (S-607, [`ImplIndex::assoc_owner`](super::impl_index::ImplIndex::assoc_owner)), whatever its header.
    ///
    /// A free function, a function nested in another, and a module-level class
    /// a Python call constructs are never one.
    ///
    /// [FR-RS-07]: ../../../../docs/specs/requirements/FR-RS-07.md
    /// [FR-RS-11]: ../../../../docs/specs/requirements/FR-RS-11.md
    /// [FR-RS-12]: ../../../../docs/specs/requirements/FR-RS-12.md
    /// [FR-EX-12]: ../../../../docs/specs/requirements/FR-EX-12.md
    pub(super) fn is_unreachable_member(&self, id: NodeId) -> bool {
        if !self.bare_calls_free_only {
            return false;
        }
        self.ix.facts.self_types.contains_key(&id)
            || self.ix.impls.assoc_owner.contains_key(&id)
            || self
                .ix
                .parent
                .get(&id)
                .and_then(|p| self.ix.info.get(p))
                .is_some_and(|p| is_class_like(p.kind))
    }

    /// Whether a bare call cannot reach `target`, whichever rung found it — a
    /// `use` or glob import, the package, the workspace fallback:
    /// - an instance member a bare call of its language never reaches (S-590,
    ///   [FR-RS-07], [`is_unreachable_member`](CallReach::is_unreachable_member));
    ///   the scope walk already passed over one
    ///   ([`prefer_free_functions`](CallReach::prefer_free_functions)), to reach a
    ///   free function further out;
    /// - an extension it cannot be shown to reach, or one a member of the
    ///   enclosing extension's receiver hides (S-560, [FR-RS-31];
    ///   [`is_unreachable_extension`](CallReach::is_unreachable_extension),
    ///   [`hidden_by_receiver_member`](CallReach::hidden_by_receiver_member)) — which
    ///   records [`Residue::NoReceiverEvidence`].
    ///
    /// [FR-RS-07]: ../../../../docs/specs/requirements/FR-RS-07.md
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    pub(super) fn bare_call_cannot_reach(&self, target: NodeId, want: Want) -> bool {
        if self.is_unreachable_member(target) {
            return true;
        }
        let hidden = self.is_unreachable_extension(target) || self.hidden_by_receiver_member(target);
        if hidden {
            self.note(want, || Residue::NoReceiverEvidence);
        }
        hidden
    }

    /// Whether `id` is an extension function (S-560, [FR-RS-31]: a top-level
    /// function with a recorded self type) that a bare call of a language
    /// declaring them cannot be shown to reach. A bare `f()` reaches an
    /// extension only through an implicit receiver, which the graph sees in two
    /// places:
    /// - the body of an extension, its own callable or one enclosing it —
    ///   koin's `single { X(get()) }` calls the lambda receiver's
    ///   `Scope.get()`, never a same-package `KoinTest.get()`;
    /// - a class `C`, or a class with an in-repository supertype `C`, reaching
    ///   `fun C.f()` (S-833, [`caller_class_is`](super::extension_calls::ExtensionCalls::caller_class_is)):
    ///   its implicit `this` is a `C`. A class whose base the graph does not
    ///   hold never gets here — its bare call stays on the instance.
    ///
    /// Even there a member of the name wins
    /// ([`member_named`](super::extension_calls::ExtensionCalls::member_named)), as for a receiver call.
    ///
    /// [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
    fn is_unreachable_extension(&self, id: NodeId) -> bool {
        let extensions = ExtensionCalls { cx: self.cx };
        let Some(receiver) = self.ix.facts.self_types.get(&id).filter(|_| self.extension_calls) else {
            return false;
        };
        let implicit = self.extension_receiver().is_some() || extensions.caller_class_is(receiver);
        !implicit || self.ix.info.get(&id).is_some_and(|i| extensions.member_named(&i.name))
    }

    /// Whether a member of the receiver of the extension the source sits in
    /// hides `target` from a bare call (S-560, [FR-RS-12]): inside `fun
    /// Box.twice() = size()` the implicit `this` is a `Box`, whose own `size`
    /// Kotlin reads before any top-level `size`. Decided only where the
    /// receiver names a class-like type of the source's family declaring a
    /// callable of `target`'s name that `target` is not; a library receiver
    /// (`String`) is not read here.
    fn hidden_by_receiver_member(&self, target: NodeId) -> bool {
        let ix: &Index = self.ix;
        if !self.extension_calls {
            return false;
        }
        let (Some(receiver), Some(name)) = (self.extension_receiver(), ix.info.get(&target).map(|i| &i.name)) else {
            return false;
        };
        let extensions = ExtensionCalls { cx: self.cx };
        extensions
            .family_classes_named(receiver)
            .into_iter()
            .any(|ty| ix.members_named(ty, name, Want::Callable).iter().any(|&m| m != target))
    }

    /// The receiver type of the extension whose body the source is in (S-560):
    /// the self type of the source or of the nearest callable enclosing it,
    /// `None` outside every extension.
    fn extension_receiver(&self) -> Option<&str> {
        let mut cursor = Some(self.source);
        for _ in 0..MAX_CONTAINS_DEPTH {
            let at = cursor.filter(|at| self.ix.info.get(at).is_some_and(|i| Want::Callable.admits(i.kind)))?;
            if let Some(receiver) = self.ix.facts.self_types.get(&at) {
                return Some(receiver);
            }
            cursor = self.ix.parent.get(&at).copied();
        }
        None
    }
}
