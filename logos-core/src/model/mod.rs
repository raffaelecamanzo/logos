//! The canonical Logos data model ([model component], [ADR-06]).
//!
//! `model` is the **leaf** of the core: it depends on nothing internal, and the
//! rest of the engine ([graph-store], [plugin-registry], [config]) depends on
//! it. It defines the SCIP-conformant type vocabulary —
//!
//! - [`LogosSymbol`] — the canonical SCIP symbol-identity newtype;
//! - [`NodeId`] — the storage-local node handle;
//! - [`NodeKind`] (34) and [`EdgeKind`] (15) — the frozen-discriminant ontology
//!   ([FR-EX-05]);
//! - [`RefForm`] (4) — the frozen target-form discriminants of the
//!   `unresolved_refs` reference ledger (S-011);
//! - [`ArtifactRelation`] — the cross-artifact edge **payload** vocabulary and
//!   its deterministic external-target classifier (CR-011, [FR-CG-07]);
//! - [`Annotations`] — free-form node metadata;
//!
//! — and confines the `scip` rust-protobuf codec behind the private
//! [`convert`] seam. **No `scip`/protobuf type appears in any public signature
//! of this module or anywhere else in the workspace** ([NFR-MA-07], [AR-07]):
//! `convert` is private to the crate and re-exports nothing.
//!
//! [model component]: ../../../docs/specs/architecture/components/model.md
//! [ADR-06]: ../../../docs/specs/architecture/decisions/ADR-06.md
//! [graph-store]: ../../../docs/specs/architecture/components/graph-store.md
//! [plugin-registry]: ../../../docs/specs/architecture/components/plugin-registry.md
//! [config]: ../../../docs/specs/architecture/components/config.md
//! [FR-EX-05]: ../../../docs/specs/requirements/FR-EX-05.md
//! [NFR-MA-07]: ../../../docs/specs/requirements/NFR-MA-07.md
//! [AR-07]: ../../../docs/specs/architecture.md

// The `scip` containment seam. Private to the crate — this is the architectural
// invariant the whole component exists to enforce, so it is deliberately NOT
// `pub`: no path outside `model` can reach a `scip` type through it.
mod convert;

mod annotations;
mod artifact;
mod kinds;
mod symbol;

pub use annotations::Annotations;
pub use artifact::{
    ArtifactRelation, BridgeNamespace, BridgeRole, MatchDiscipline, TargetClass,
};
pub use kinds::{EdgeKind, NodeKind, RefForm, UnknownKind};
pub use symbol::{LogosSymbol, NodeId};
