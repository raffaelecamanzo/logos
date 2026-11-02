//! The traits and impl blocks of the binder's index (S-828, [CR-258]): the
//! impls a trait method fans out to and the trait's own methods (S-281,
//! S-712), and the recorded `impl` blocks the one associated-item lookup
//! binds within (S-607, [FR-RS-47]) — [`ImplIndex`]. A sub-index the
//! [`Index`] owns, with its fields; the [`IndexBuilder`](super::IndexBuilder)
//! fills it.
//!
//! [CR-258]: ../../../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md
//! [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md

use std::collections::HashMap;

use crate::model::NodeId;

use super::{Index, Res};

/// One recorded `impl` block's header, resolved once (S-607,
/// [`IndexBuilder::with_associated_items`](super::IndexBuilder::with_associated_items)).
#[derive(Debug)]
pub(super) struct ImplHeader {
    /// The one repository type the self-type path resolves to in the block's
    /// own scope — [`Res::NotFound`] for an external type, a primitive, a
    /// generic parameter or a reference header, [`Res::Ambiguous`] when the
    /// scope names two.
    pub(super) self_type: Res,
    /// The self-type path as written, generics stripped — whose tokens a sync
    /// watches for a `Self::m` call ([`Invalidation::ref_affected`](super::Invalidation::ref_affected)).
    pub(super) self_path: String,
    /// The trait a trait impl implements, as written; `None` for an inherent
    /// impl.
    pub(super) trait_path: Option<String>,
    /// That trait's repository node, when the block's scope resolves it to one
    /// — `None` for an inherent impl and for an external trait (`Display`,
    /// `Default`), which the graph cannot place in or out of scope.
    pub(super) trait_node: Option<NodeId>,
    /// The block's functions, id-sorted — none for an empty block, which
    /// overrides no default of its trait (S-608).
    pub(super) fns: Vec<NodeId>,
}

/// The traits' fan-out universe and the recorded `impl` blocks, each map
/// empty until the builder fills it.
#[derive(Debug, Default)]
pub(super) struct ImplIndex {
    /// `(trait node, method name)` → the concrete workspace impl method nodes of
    /// that trait method, id-sorted and deduplicated — the fan-out universe for a
    /// `dyn T` method call (S-281, [CR-073], [FR-RS-08]) and every other
    /// trait-typed call (S-608, [`TraitDispatch::fan_out`](super::trait_dispatch::TraitDispatch::fan_out)). Built from the
    /// `Implements` reference rows (impl method → its trait), so it is available
    /// on the very first index pass, before any `Implements` edge is committed.
    ///
    /// [CR-073]: ../../../../docs/requests/CR-073-trait-object-dynamic-dispatch-reachability.md
    /// [FR-RS-08]: ../../../../docs/specs/requirements/FR-RS-08.md
    pub(super) impls_by_trait_method: HashMap<(NodeId, String), Vec<NodeId>>,
    /// `(trait node, method name)` → the symbol of the trait's own method of
    /// that name: its default body's, or its required signature's (S-712) —
    /// what a trait fan-out's [`Dispatch`](super::Dispatch) names. Built from the bindable
    /// nodes, the signatures added by [`IndexBuilder::with_signatures`](super::IndexBuilder::with_signatures), which
    /// [`bindable`](super::bindable) keeps out of the index.
    pub(super) trait_methods: HashMap<(NodeId, String), String>,
    /// `(type node, name)` → the functions of every recorded `impl` block whose
    /// self type **resolves** to that type node, id-sorted (S-607, [FR-RS-47];
    /// the `impl_blocks` table of migration 34) — the one universe
    /// [`Ctx::lookup`](super::Ctx::lookup) binds a call to a type's method within. From any
    /// repository crate and any file, the header's path resolved in its own
    /// file's scope ([`IndexBuilder::with_associated_items`](super::IndexBuilder::with_associated_items)); a header naming no
    /// repository type (`impl Display for Vec<X>`, `impl Tr for ()`, a generic
    /// parameter) contributes no key.
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) assoc_items: HashMap<(NodeId, String), Vec<NodeId>>,
    /// impl function → the index of its block in [`impl_headers`](super::impl_index::ImplIndex::impl_headers):
    /// what a `Self::m` / `self.m()` call reads its `T` from, and whether a
    /// candidate is a trait impl's.
    pub(super) assoc_owner: HashMap<NodeId, usize>,
    /// Every recorded `impl` block of an impl-block language, an empty one
    /// included (S-608), with its header resolved once ([`ImplHeader`]).
    pub(super) impl_headers: Vec<ImplHeader>,
    /// A type node → the trait impl blocks for it whose trait is a repository
    /// trait, as indexes into [`impl_headers`](super::impl_index::ImplIndex::impl_headers) (S-608,
    /// [FR-RS-47] rule 1): whose trait's default bodies are the type's
    /// candidates where its block does not override them. An empty block
    /// counts.
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) trait_impls: HashMap<NodeId, Vec<usize>>,
    /// A type node → the one type its `impl Deref` names as `Target`, resolved
    /// in the block's scope (S-608, [FR-RS-47] rule 4): [`Res::NotFound`] for a
    /// type the repository does not declare, [`Res::Ambiguous`] when two
    /// `Deref` impls of the type name two. What a method-syntax miss retries on
    /// ([`Ctx::lookup`](super::Ctx::lookup)).
    ///
    /// [FR-RS-47]: ../../../../docs/specs/requirements/FR-RS-47.md
    pub(super) deref_targets: HashMap<NodeId, Res>,
}

impl ImplIndex {
    /// The concrete workspace impl method nodes of trait method `(trait, name)`,
    /// id-sorted; empty when none are recorded.
    pub(super) fn impls_of(&self, trait_node: NodeId, name: &str) -> &[NodeId] {
        self.impls_by_trait_method
            .get(&(trait_node, name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The symbol of trait `trait_node`'s own method `name` — its default
    /// body's or its required signature's ([`ImplIndex::trait_methods`]) — or
    /// `None` when the graph holds neither.
    pub(super) fn trait_method_symbol(&self, trait_node: NodeId, name: &str) -> Option<&str> {
        self.trait_methods.get(&(trait_node, name.to_string())).map(String::as_str)
    }

    /// Whether the `impl` block `at` declares a function `name` — overriding
    /// its trait's default of that name (S-608). `ix` names the block's
    /// functions.
    pub(super) fn block_overrides(&self, ix: &Index, at: usize, name: &str) -> bool {
        self.impl_headers[at]
            .fns
            .iter()
            .any(|f| ix.info.get(f).is_some_and(|i| i.name == name))
    }
}
