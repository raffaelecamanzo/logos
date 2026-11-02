//! A cross-artifact reference bound under the exactly-one discipline (CR-011,
//! S-069, [FR-CG-07]): an artifact path, a Terraform module directory, an
//! artifact or code type name, an OpenAPI operation's route
//! ([`Artifacts::resolve_artifact`]).
//!
//! [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md

use std::ops::Deref;

use crate::graph_store::UnresolvedRefRow;
use crate::model::{ArtifactRelation, EdgeKind, NodeId, NodeKind, RefForm};

use super::super::rel_path::{dir_segments, fold_path};
use super::super::route_method::preferred_candidates;
use super::super::route_template::route_key;
use super::{exactly_one, is_type_like, Ctx, Outcome, Res};

/// A cross-artifact reference bound to the one artifact or code node it names.
#[derive(Clone, Copy)]
pub(super) struct Artifacts<'c, 'a> {
    pub(super) cx: &'c Ctx<'a>,
}

impl<'a> Deref for Artifacts<'_, 'a> {
    type Target = Ctx<'a>;

    fn deref(&self) -> &Ctx<'a> {
        self.cx
    }
}

impl Artifacts<'_, '_> {
    /// Bind one cross-artifact reference under never-fabricate (CR-011,
    /// [ADR-26], [FR-CG-07]).
    ///
    /// The third and fourth matcher clients after code and docs, dispatched by
    /// `(kind, form)` to a substrate resolution primitive:
    ///
    /// - **`ArtifactRef` + `Path`** — a workspace-relative artifact path (a proto
    ///   import, a shell `source`, a Terraform local module source) binds to the
    ///   one [`NodeKind::ConfigFile`] at that path
    ///   ([`resolve_artifact_path`](Artifacts::resolve_artifact_path)).
    /// - **`ArtifactRef` + `Method`** — a literal artifact name (a proto type
    ///   reference, a GraphQL type reference) binds to the one artifact node of
    ///   that name ([`resolve_artifact_name`](Artifacts::resolve_artifact_name)).
    /// - **`ArtifactBinding` + `Method`** — a schema-declared type name binds to
    ///   the one type-like **code** symbol of that name, with no synthesized
    ///   candidates ([`resolve_code_type_name`](Artifacts::resolve_code_type_name)).
    ///
    /// Every primitive ends in [`exactly_one`], so an ambiguous or unindexed
    /// reference stays unbound and retries on sync ([NFR-RA-05]). The consumer
    /// stories extend this dispatch with their richer matchers (e.g. the OpenAPI
    /// positional-template `Route` match) by adding an arm here. On a bind, the
    /// relation class (`r.payload`) is stamped onto the edge so navigation can
    /// surface which relation it expresses ([FR-CG-11]).
    ///
    /// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
    /// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
    /// [FR-CG-11]: ../../../../docs/specs/requirements/FR-CG-11.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn resolve_artifact(&self, r: &UnresolvedRefRow) -> Outcome {
        // The relation class travels on the ledger row's payload; recover it so a
        // name match can be fenced to the relation's own artifact kind.
        let relation = r.payload.as_deref().and_then(ArtifactRelation::from_wire);
        // A Terraform local module call is the one multi-target relation: its
        // `source` names a directory, so it fans out to every admitted `.tf`
        // `ConfigFile` in that directory (CR-011, FR-CG-08). It returns directly
        // rather than through the single-target `exactly_one` gate below.
        if r.kind == EdgeKind::ArtifactRef
            && r.form == RefForm::Path
            && relation == Some(ArtifactRelation::TfModuleCall)
        {
            return self.resolve_module_dir(&r.target, r.payload.clone());
        }
        let found = match (r.kind, r.form) {
            (EdgeKind::ArtifactRef, RefForm::Path) => self.resolve_artifact_path(&r.target),
            (EdgeKind::ArtifactRef, RefForm::Method) => self
                .resolve_artifact_name(&r.target, relation.and_then(ArtifactRelation::target_kind)),
            (EdgeKind::ArtifactBinding, RefForm::Method) => self.resolve_code_type_name(&r.target),
            // The OpenAPI `ApiOperation`→`route` match (S-069): the target is the
            // operation rendered `"METHOD /template"`, bound to the one route
            // whose positionally-normalized template matches and whose method
            // serves it — `ANY` is a wildcard, and an exact-method route outranks
            // it ([CR-109]; see `resolve_route`).
            (EdgeKind::ArtifactBinding, RefForm::Path) => self.resolve_route(&r.target),
            // Other (kind, form) shapes are consumer-story extension points:
            // unbound here, never fabricated.
            _ => Res::NotFound,
        };
        match found {
            Res::Found(target) => Outcome::Bound {
                source: self.source,
                target,
                kind: r.kind,
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        }
    }

    /// Resolve a workspace-relative artifact path to the one
    /// [`NodeKind::ConfigFile`] at that path (CR-011, [FR-CG-07]).
    ///
    /// Folds the target relative to the source artifact's directory and then to
    /// the repository root — the same two-interpretation walk a documentation
    /// path takes ([`resolve_doc_link`](super::doc_references::DocReferences::resolve_doc_link)), minus the
    /// `#anchor` (artifact imports carry none). Accepts iff exactly one
    /// `ConfigFile` lives at the resolved path; a path that does not (yet) resolve
    /// stays unbound and retries on the sync that indexes its target.
    ///
    /// [FR-CG-07]: ../../../../docs/specs/requirements/FR-CG-07.md
    fn resolve_artifact_path(&self, target: &str) -> Res {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Res::NotFound;
        };
        let source_rel = fold_path(&dir_segments(source_file), target);
        let root_rel = fold_path(&[], target);
        for path in [source_rel, root_rel].into_iter().flatten() {
            match self.config_file_at(&path) {
                Res::NotFound => {}
                decided => return decided,
            }
        }
        Res::NotFound
    }

    /// The one [`NodeKind::ConfigFile`] defined at `path`, or nothing.
    fn config_file_at(&self, path: &str) -> Res {
        let Some(ids) = self.ix.modules.by_file_path.get(path) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.ix
                    .info
                    .get(id)
                    .is_some_and(|i| i.kind == NodeKind::ConfigFile)
            })
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve a Terraform local module call to **every** admitted `.tf`
    /// [`NodeKind::ConfigFile`] in its source directory (CR-011, [FR-CG-08]).
    ///
    /// A module `source = "./modules/net"` names a *directory*; the relation binds
    /// the calling `module` block to each `.tf` file living **directly** in that
    /// directory ([`config_tf_files_in_dir`](Artifacts::config_tf_files_in_dir)). The
    /// directory is folded relative to the calling artifact's own directory then to
    /// the repository root — the two-interpretation walk
    /// [`resolve_artifact_path`](Artifacts::resolve_artifact_path) uses, the first that
    /// resolves to at least one file winning. A directory with no indexed `.tf`
    /// stays unbound and retries on the sync that indexes its members; registry and
    /// remote sources were classified external before the ledger, so a row here is
    /// always a genuine local-path candidate ([NFR-RA-05]).
    ///
    /// [FR-CG-08]: ../../../../docs/specs/requirements/FR-CG-08.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_module_dir(&self, target: &str, payload: Option<String>) -> Outcome {
        let Some(source_file) = self
            .ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
        else {
            return Outcome::Unbound;
        };
        let source_rel = fold_path(&dir_segments(source_file), target);
        let root_rel = fold_path(&[], target);
        for dir in [source_rel, root_rel].into_iter().flatten() {
            let targets = self.config_tf_files_in_dir(&dir);
            if !targets.is_empty() {
                return Outcome::BoundMany {
                    source: self.source,
                    targets,
                    kind: EdgeKind::ArtifactRef,
                    payload,
                    dispatch: None,
                };
            }
        }
        Outcome::Unbound
    }

    /// Every `.tf` [`NodeKind::ConfigFile`] whose file lives **directly** in `dir`
    /// (no recursion into nested module directories), `NodeId`-sorted for a
    /// deterministic edge set ([NFR-RA-06]). The fan-out targets of a local module
    /// call ([`resolve_module_dir`](Artifacts::resolve_module_dir)).
    fn config_tf_files_in_dir(&self, dir: &str) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for (path, ids) in &self.ix.modules.by_file_path {
            if !path.ends_with(".tf") {
                continue;
            }
            let parent = match path.rfind('/') {
                Some(i) => &path[..i],
                None => "",
            };
            if parent != dir {
                continue;
            }
            for &id in ids {
                if self
                    .ix
                    .info
                    .get(&id)
                    .is_some_and(|i| i.kind == NodeKind::ConfigFile)
                {
                    out.push(id);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Resolve a literal artifact name to the one artifact node carrying it
    /// (CR-011, [FR-CG-08]).
    ///
    /// The artifact→artifact name match — a proto type reference to its
    /// `ProtoMessage`, a GraphQL type reference to its `GqlType`, a Terraform
    /// `var`/`local`/`module` reference to its `TfBlock`, a SQL clause to its
    /// `SqlObject`. When the relation declares a specific target
    /// ([`ArtifactRelation::target_kind`]) the candidate set is fenced to **that**
    /// artifact kind, so a name shared across formats (a proto type and a same-named
    /// `TfBlock`) can never cross-bind ([NFR-RA-05], [ADR-26]); `want_kind` is
    /// `None` only for a relation with no declared artifact target, where any
    /// artifact-layer ([`NodeKind::is_config`]) node is a candidate. Code symbols
    /// are always excluded, and the single [`exactly_one`] gate keeps
    /// never-fabricate — a duplicated or absent name stays unresolved.
    ///
    /// [FR-CG-08]: ../../../../docs/specs/requirements/FR-CG-08.md
    /// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_artifact_name(&self, name: &str, want_kind: Option<NodeKind>) -> Res {
        // A **keyless** row names nothing and can bind to nothing. The broker arm
        // records its `topic-not-literal` refusals as keyless ledger rows ([CR-107]),
        // and this is the gate that keeps one from binding to some artifact node that
        // happens to carry an empty name — a fabricated edge out of a refusal, which
        // is the opposite of what recording it is for ([NFR-RA-05]).
        //
        // [CR-107]: ../../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
        if name.trim().is_empty() {
            return Res::NotFound;
        }
        let Some(all) = self.ix.by_name.get(name) else {
            return Res::NotFound;
        };
        let admits = |kind: NodeKind| match want_kind {
            // A declared relation target fences candidates to exactly that kind.
            Some(want) => kind == want,
            // No declared target: any artifact-layer node, code excluded.
            None => kind.is_config(),
        };
        let candidates: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|id| self.ix.info.get(id).is_some_and(|i| admits(i.kind)))
            .collect();
        exactly_one(&candidates)
    }

    /// Resolve a schema-declared type name to the one **type-like code** symbol
    /// carrying it (CR-011, [FR-CG-10]).
    ///
    /// The artifact→code binding for a literal declared name (a proto/GraphQL type
    /// → the struct/class/… that implements it). Only type-like code kinds are
    /// candidates ([`is_type_like`]) — **no synthesized candidates**, no codegen
    /// case-mapping, no resolver conventions ([ADR-26]) — and the single
    /// [`exactly_one`] gate means a common name (`User` declared in two places) or
    /// an absent one stays unresolved ([NFR-RA-05]).
    ///
    /// [FR-CG-10]: ../../../../docs/specs/requirements/FR-CG-10.md
    /// [ADR-26]: ../../../../docs/specs/architecture/decisions/ADR-26.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    fn resolve_code_type_name(&self, name: &str) -> Res {
        self.one_named(name, is_type_like)
    }

    /// Resolve an OpenAPI `ApiOperation` to the one framework-extracted `route`
    /// node it specifies (S-069, CR-011, [FR-CG-09]).
    ///
    /// `target` is the operation rendered `"METHOD /template"` (the same shape a
    /// [`NodeKind::Route`] node's `name` carries). Both sides are reduced to the
    /// shared `(METHOD, positionally-normalized template)`
    /// [`route_key`](super::super::route_template::route_key): parameter names and
    /// syntax are erased, but the static skeleton must match exactly. The
    /// candidate set is that template's bucket in [`Index`](super::Index), narrowed by the
    /// shared [`route_method`](super::super::route_method) rule — an `ANY` provider
    /// serves every verb, an exact-method provider of the same template is the
    /// sole candidate beside a wildcard one ([CR-109]) — and then reduced by the
    /// shared [`exactly_one`] gate. So two equally-specific routes sharing a
    /// normalized template leave the operation unresolved, a method mismatch
    /// never binds, and an operation whose key does not normalize (or matches no
    /// route) stays in the ledger for the next sync ([NFR-RA-05]). A
    /// catch-all/regex route is absent from the index entirely, so it is never
    /// approximately matched.
    ///
    /// The cross-member bridge ([`crate::federation::bridge`]) and the coverage
    /// read-model narrow their own provider buckets through the very same rule,
    /// so no two of the three sites can drift on which input binds ([ADR-52]).
    ///
    /// [CR-109]: ../../../../docs/requests/CR-109-wildcard-method-route-matching.md
    /// [FR-CG-09]: ../../../../docs/specs/requirements/FR-CG-09.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [ADR-52]: ../../../../docs/specs/architecture/decisions/ADR-52.md
    fn resolve_route(&self, target: &str) -> Res {
        // A **keyless** row names nothing and can bind to nothing — the same guard
        // [`Artifacts::resolve_artifact_name`] carries for the broker arm's refusals
        // ([CR-107]), stated here because the HTTP client-call arm's refusals
        // ([CR-120], S-374) arrive on this path instead: they are
        // `(ArtifactBinding, Path)` rows, so `resolve_artifact` dispatches them
        // here. `route_key` refuses an empty `"METHOD /template"` on its own, so
        // this is explicit rather than load-bearing — and explicit is the point:
        // the property is a never-fabricate invariant ([NFR-RA-05]), not an
        // incidental consequence of a `split_once`.
        //
        // [CR-107]: ../../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
        // [CR-120]: ../../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
        if target.trim().is_empty() {
            return Res::NotFound;
        }
        let Some((method, template)) = route_key(target) else {
            // The operation's own template does not normalize (or the target is
            // malformed): never approximately matched.
            return Res::NotFound;
        };
        let Some(bucket) = self.ix.modules.routes_by_template.get(&template) else {
            return Res::NotFound;
        };
        let candidates: Vec<NodeId> = preferred_candidates(
            bucket.iter().map(|(m, id)| (Some(m.as_str()), id)),
            Some(&method),
        )
        .into_iter()
        .copied()
        .collect();
        exactly_one(&candidates)
    }
}
