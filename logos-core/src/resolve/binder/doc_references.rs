//! A documentation link or code-name token bound to its one target (S-035,
//! S-039, [FR-DG-03], [FR-DG-04]), elevated to a typed trace between two typed
//! documentation artifacts ([`DocReferences::bind_doc_ref`]).
//!
//! [FR-DG-03]: ../../../../docs/specs/requirements/FR-DG-03.md
//! [FR-DG-04]: ../../../../docs/specs/requirements/FR-DG-04.md

use std::ops::Deref;

use crate::extract::doc::heading_slug;
use crate::model::{EdgeKind, NodeId, NodeKind};

use super::super::rel_path::{dir_segments, fold_path};
use super::{
    exactly_one, is_doc_file_kind, is_doc_kind, is_doc_section_kind, is_typed_doc_kind,
    split_anchor, Ctx, Outcome, Res, MAX_CONTAINS_DEPTH,
};

/// A documentation link or code-name token bound to its one target.
#[derive(Clone, Copy)]
pub(super) struct DocReferences<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for DocReferences<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl DocReferences<'_, '_> {
    /// Resolve a documentation link/path reference to its target node (S-035,
    /// [FR-DG-03]/[FR-DG-04]).
    ///
    /// The href is normalised relative to the source doc's path. With a
    /// `#anchor`, the target is the one [`NodeKind::DocSection`] in that file
    /// whose [`heading_slug`] matches; without one, the [`NodeKind::DocFile`] at
    /// that path, else the file-root module of a code file. Every branch ends in
    /// [`exactly_one`] — a missing or ambiguous target stays unresolved
    /// ([NFR-RA-05]).
    ///
    /// [FR-DG-03]: ../../../../docs/specs/requirements/FR-DG-03.md
    /// [FR-DG-04]: ../../../../docs/specs/requirements/FR-DG-04.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_doc_link(&self, target: &str) -> Res {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Res::NotFound;
        };
        let (path_part, anchor) = split_anchor(target);

        // A bare `#anchor` targets the source file itself.
        if path_part.is_empty() {
            return self.resolve_doc_at_path(source_file, anchor.as_deref());
        }

        // Try two interpretations of the path, in order, falling through only on
        // a clean miss (never on a known ambiguity — that would fabricate):
        //   1. relative to the source doc's directory — markdown-link semantics;
        //   2. relative to the repository root — explicit repo-file-path
        //      semantics (FR-DG-04), e.g. an inline-code `crate/src/x.rs`.
        // A leading `/` already forces root in both, so they coincide there.
        let source_rel = fold_path(&dir_segments(source_file), path_part);
        let root_rel = fold_path(&[], path_part);
        for path in [source_rel, root_rel].into_iter().flatten() {
            match self.resolve_doc_at_path(&path, anchor.as_deref()) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// Resolve an already-normalised doc target `path` (+ optional `anchor`) to a
    /// node: with an anchor, the one [`NodeKind::DocSection`] in that file whose
    /// [`heading_slug`] matches; without one, the [`NodeKind::DocFile`] there,
    /// else the file-root module of a code file. Always exactly-one-or-nothing.
    fn resolve_doc_at_path(&self, path: &str, anchor: Option<&str>) -> Res {
        let in_file = self.ix.modules.by_file_path.get(path);

        if let Some(anchor) = anchor {
            // Re-slugify the anchor so a link and its heading agree on casing and
            // punctuation; well-formed anchors are already slugs (idempotent).
            let want = heading_slug(anchor);
            let Some(ids) = in_file else {
                return Res::NotFound;
            };
            let candidates: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.ix.info.get(id).is_some_and(|i| {
                        is_doc_section_kind(i.kind) && heading_slug(&i.name) == want
                    })
                })
                .collect();
            return exactly_one(&candidates);
        }

        // No anchor: a DocFile at that path (a doc target) wins; otherwise the
        // file-root module of a code file the link points at.
        if let Some(ids) = in_file {
            let doc_files: Vec<NodeId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.ix
                        .info
                        .get(id)
                        .is_some_and(|i| is_doc_file_kind(i.kind))
                })
                .collect();
            if !doc_files.is_empty() {
                return exactly_one(&doc_files);
            }
        }
        // A package-shaped file (CR-149) is found by its path, never by its
        // module key: a `src/main` and a `src/test` file of one package and
        // name share a key, and the tree keeps only the first.
        if self.ix.layout.is_package_shaped(path) {
            let roots: Vec<NodeId> = in_file
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| {
                    !self.ix.parent.contains_key(id)
                        && self.ix.info.get(id).is_some_and(|i| i.kind == NodeKind::Module)
                })
                .collect();
            return exactly_one(&roots);
        }
        match self.ix.modules.by_key.get(&self.ix.layout.module_key(path)) {
            Some(&m) => Res::Found(m),
            None => Res::NotFound,
        }
    }

    /// Resolve a documentation code-name token to the one *code* symbol of that
    /// name in the workspace (S-035, [FR-DG-04]).
    ///
    /// Documentation nodes are excluded so a token binds to code, never to
    /// another doc; the single [`exactly_one`] gate keeps the never-fabricate
    /// invariant — a name shared by two symbols, or by none, stays unresolved
    /// ([NFR-RA-05]).
    ///
    /// [FR-DG-04]: ../../../../docs/specs/requirements/FR-DG-04.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_doc_code_name(&self, name: &str) -> Res {
        self.one_named(name, |kind| !is_doc_kind(kind))
    }

    /// Turn a resolved documentation reference into an edge, elevating it to a
    /// typed trace when it connects two swe-skills artifacts (S-039, [FR-DG-07]).
    ///
    /// A hyperlink expresses a *trace* when the node it lives in and the node it
    /// resolves to are each owned by a typed artifact: [`typed_owner`] walks each
    /// endpoint up the `Contains` hierarchy to its nearest enclosing
    /// `Requirement`/`Adr`/`Story` (or itself). So a link in a `Requirement`
    /// file's `## Dependencies` section traces from the *requirement*, and a
    /// `Story` section's link to a requirement traces the "implements" relation —
    /// both as [`EdgeKind::TracesTo`] between the typed owners. A link touching a
    /// generic `DocFile`/`DocSection` or a code symbol (neither has a typed owner)
    /// stays a plain [`EdgeKind::DocReference`] from the resolved endpoints, and a
    /// link within a single artifact (`a == b`, an intra-file anchor) is not a
    /// self-trace.
    ///
    /// Re-typing never fabricates: the reference was already bound through the
    /// exactly-one-candidate ledger ([NFR-RA-05]); this only labels and elevates
    /// the edge the binder proved.
    ///
    /// [`typed_owner`]: DocReferences::typed_owner
    /// [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn bind_doc_ref(&self, source: NodeId, target: NodeId) -> Outcome {
        if let (Some(a), Some(b)) = (self.typed_owner(source), self.typed_owner(target)) {
            if a != b {
                return Outcome::Bound {
                    source: a,
                    target: b,
                    kind: EdgeKind::TracesTo,
                    payload: None,
                };
            }
        }
        Outcome::Bound {
            source,
            target,
            kind: EdgeKind::DocReference,
            payload: None,
        }
    }

    /// The nearest typed swe-skills artifact (`Requirement`/`Adr`/`Story`) that
    /// *owns* `id` — `id` itself if it is typed, else the nearest such ancestor
    /// reached by walking the `Contains` hierarchy upward, or `None` if none
    /// (a generic doc node, or a code symbol) (S-039, [FR-DG-07]).
    ///
    /// `Contains` is a tree, so the walk normally terminates at a typed node or
    /// the root. The hard [`MAX_CONTAINS_DEPTH`] cap is a defence-in-depth guard
    /// against a malformed cycle ever reaching the binder — mirroring the bounded
    /// [`MAX_ALIAS_DEPTH`](super::MAX_ALIAS_DEPTH) alias walk — so a corrupt edge degrades to "no typed
    /// owner" (a plain `DocReference`) instead of spinning forever.
    fn typed_owner(&self, mut id: NodeId) -> Option<NodeId> {
        for _ in 0..MAX_CONTAINS_DEPTH {
            if is_typed_doc_kind(self.ix.info.get(&id)?.kind) {
                return Some(id);
            }
            id = *self.ix.parent.get(&id)?;
        }
        None
    }
}
