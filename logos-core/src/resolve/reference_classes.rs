//! The reference classes ([FR-RS-49], S-635, [CR-212]): every reported
//! reference-ledger row, classified exactly once as a **doc reference**, a
//! **provably external** code reference, or an **internal candidate**.
//!
//! A code reference is provably external when it is unbound and its target
//! cannot name a symbol of this graph:
//!
//! - **(i) named nowhere** — no name the graph declares is the target, or a
//!   suffix of it that follows a path separator (`::`, `.`, `/`, `\`) — or,
//!   for an import, any of its segments, since an import can bind its parent
//!   package (`hc.local_settings` binding `hc`). Every binder rule ends in a
//!   lookup of one such name, so a target none of them spells binds to
//!   nothing here. "A name the graph declares" is wider than a node's name,
//!   because a lookup does not always end on one ([`NameUniverse::build`]);
//! - **(ii) external-type** — the binder's own residue walk proved the row's
//!   path or receiver type leaves the repository (`HashMap::new` beside a
//!   repository `new`). Only where the walk runs — the languages with a
//!   `call_residue` — and only for `Calls`; `status` adds the rows the walk
//!   counted by target ([`CallResidue::external_type_targets`]) that (i)
//!   left internal, so no row is counted twice.
//!
//! The name test applies to the kinds a binder binds by name ([`NAME_BOUND`]).
//! An artifact, route or broker reference binds through its own matcher — an
//! OpenAPI binding `POST /api/blobs/{digest}` binds an operation by its path,
//! not by any node's name — so it stays an internal candidate.
//!
//! **Excluded: Go imports** ([`IMPORTS_NOT_NAMED`]). An import of the module's
//! own root package (`go.uber.org/zap`, inside zap) binds the root directory's
//! files through the `go.mod` `module` directive — a name the graph does not
//! hold — so the name test cannot judge a Go import, and every one stays an
//! internal candidate. Go's calls and type references are judged as any
//! language's.
//!
//! **Excluded: calls through a local value.** A call the binder marked as made
//! through a parameter or local (S-594's `local-value` row) binds nothing by
//! design, and its target is the local's name, not a declaration's: the name
//! test would call it external exactly when that name is declared nowhere else.
//! [`GraphStore::reference_population`] leaves such rows out of the unbound
//! targets, so each stays an internal candidate.
//!
//! [`GraphStore::reference_population`]: crate::graph_store::GraphStore::reference_population
//!
//! **Why the universe is this wide.** CR-212's assumption A1 — "a target whose
//! final segment is declared by no node cannot bind" — was measured false
//! before this module was written: across the inspection estate and the
//! sample workspace, bound rows named an import alias (`from x import
//! Request as _SansIORequest`, then `class R(_SansIORequest)`), an included
//! header (`.::webview.h`), a Go package directory, a C# namespace, a
//! directory-index import (`@::components::layouts`), a Kotlin file facade
//! (`ScopeJVMKt`), a parent package and a Go module root. With those names
//! admitted and Go imports excluded, no bound row of 1.40 million, in fourteen
//! languages, names nothing the universe holds — the soundness the
//! `bound_rows_are_never_named_nowhere` test pins per language on the reach
//! fixtures.
//!
//! The classifier reads only the graph and writes nothing. It errs toward the
//! internal-candidate class — a name declared only by a test or vendored file
//! keeps an external call a candidate — which lowers the headline, the safe
//! direction for a lower bound.
//!
//! [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
//! [CR-212]: ../../../docs/requests/CR-212-reference-resolution-is-stated-over-the-references-that-could-be-internal.md
//! [`CallResidue::external_type_targets`]: crate::models::CallResidue

use std::collections::{BTreeMap, HashSet};

use anyhow::Result;

use super::package_key::PackageLayout;
use crate::graph_store::{
    DeclaredNameSources, GraphStore, ReferencePopulation, UnboundCodeTarget, UnboundInvocation,
};
use crate::model::EdgeKind;
use crate::models::{DocResolution, InternalResolution};

/// The path separators of the indexed languages' targets. A suffix following
/// any of them is a name the target could end on; testing all of them for
/// every language is sound for each, because each language's own separators
/// are among them.
const SEPARATORS: [&str; 4] = ["::", ".", "/", "\\"];

/// The reference kinds a binder binds by a name lookup — the only ones the
/// name test (i) classifies. Every other code kind is an internal candidate.
pub(crate) const NAME_BOUND: [EdgeKind; 8] = [
    EdgeKind::Calls,
    EdgeKind::Imports,
    EdgeKind::References,
    EdgeKind::Implements,
    EdgeKind::Extends,
    EdgeKind::Instantiates,
    EdgeKind::TypeUses,
    EdgeKind::Accesses,
];

/// The languages whose imports the name test cannot judge (see the module
/// docs): each of their imports is an internal candidate.
pub(crate) const IMPORTS_NOT_NAMED: [&str; 1] = ["go"];

/// Every name a reference's target could bind to in one graph.
#[derive(Debug, Clone, Default)]
pub(crate) struct NameUniverse {
    names: HashSet<String>,
}

impl NameUniverse {
    /// The universe of one graph's [`DeclaredNameSources`]:
    ///
    /// - every node name, and every ledger alias that renames — `Y` of
    ///   `import X as Y`, which a later row names. An alias that is its
    ///   target's own last segment (`use std::collections::HashMap`) renames
    ///   nothing, and would otherwise make every import name itself;
    /// - every segment of every indexed file's path, and the file name less
    ///   each of its extensions (`fixture.copy.ts` → `fixture.copy`,
    ///   `fixture`): an include, a directory-index import or a Go package path
    ///   binds a file or a directory;
    /// - every segment of a declared namespace (a C# `using`, a PHP `use`);
    /// - every file facade the file's language declares (`Utils.kt` →
    ///   `UtilsKt`), from `layout`.
    pub(crate) fn build(sources: &DeclaredNameSources, layout: &PackageLayout) -> Self {
        let mut names: HashSet<String> = sources.names.iter().cloned().collect();
        names.extend(
            sources
                .aliases
                .iter()
                .filter(|(alias, target)| segments(target).last() != Some(alias.as_str()))
                .map(|(alias, _)| alias.clone()),
        );
        for (path, namespace) in &sources.files {
            names.extend(path.split('/').map(str::to_string));
            let file = path.rsplit('/').next().unwrap_or(path);
            let mut stem = file;
            while let Some(dot) = stem.rfind('.').filter(|&dot| dot > 0) {
                stem = &stem[..dot];
                names.insert(stem.to_string());
            }
            if let Some(namespace) = namespace {
                names.extend(segments(namespace).map(str::to_string));
            }
            names.extend(layout.file_facade(path));
        }
        names.remove("");
        Self { names }
    }

    /// Whether a name of this graph could be what `target` names: the whole
    /// target, or a suffix of it following a separator. A target that ends on a
    /// separator (`.`, a directory-index import) names a place rather than a
    /// name and cannot be judged, so it could.
    pub(crate) fn could_name(&self, target: &str) -> bool {
        if target.is_empty() || self.names.contains(target) {
            return true;
        }
        // The separators are ASCII, so every match starts and ends on a char
        // boundary.
        target.char_indices().any(|(at, _)| {
            SEPARATORS.iter().any(|sep| {
                target[at..].starts_with(sep) && {
                    let rest = &target[at + sep.len()..];
                    rest.is_empty() || self.names.contains(rest)
                }
            })
        })
    }

    /// Whether a name of this graph could be what the import `target` names:
    /// as [`could_name`](Self::could_name), or through any one of its segments
    /// — an import binds a package by a prefix of its path too.
    pub(crate) fn could_name_import(&self, target: &str) -> bool {
        self.could_name(target) || segments(target).any(|segment| self.names.contains(segment))
    }

    /// Take every name of `other` into this universe — the workspace's union of
    /// its members' universes (S-637), in which a name any member declares
    /// could be what a target names.
    pub(crate) fn absorb(&mut self, other: Self) {
        self.names.extend(other.names);
    }

    /// The name test (i) for one unbound code reference: `true` — an
    /// internal candidate — unless its kind binds by name and no name of this
    /// graph could be what it names. The one decision the classification and
    /// the residue walk's `external-type` count both make.
    pub(crate) fn could_be_internal(&self, language: Option<&str>, kind: i64, target: &str) -> bool {
        if !NAME_BOUND.iter().any(|bound| i64::from(bound.as_i32()) == kind) {
            return true;
        }
        if kind == i64::from(EdgeKind::Imports.as_i32()) {
            return language.is_some_and(|language| IMPORTS_NOT_NAMED.contains(&language))
                || self.could_name_import(target);
        }
        self.could_name(target)
    }
}

/// A target's segments under every separator, empty ones dropped.
fn segments(target: &str) -> impl Iterator<Item = &str> {
    target
        .split([':', '.', '/', '\\'])
        .filter(|segment| !segment.is_empty())
}

/// One language's provably-external rows of the two classes its
/// `resolution_by_language` row states.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExternalByClass {
    /// Provably external `Calls` rows.
    pub(crate) calls: u64,
    /// Provably external `Imports` rows.
    pub(crate) imports: u64,
}

/// The reported ledger, classified: the doc and code halves, the provably
/// external code references in all, and per language for the two stated
/// classes. Internal candidates are `code_references − provably_external`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReferenceClasses {
    /// Reported doc-reference rows.
    pub(crate) doc_references: u64,
    /// Of those, the bound ones.
    pub(crate) doc_resolved: u64,
    /// Every other reported row.
    pub(crate) code_references: u64,
    /// Of those, the bound ones.
    pub(crate) code_resolved: u64,
    /// Code references classified provably external, by (i) or (ii).
    pub(crate) provably_external: u64,
    /// Per `files.language`; a file recording none belongs to no entry.
    pub(crate) by_language: BTreeMap<String, ExternalByClass>,
}

/// One target's `external-type` rows, by the fully-qualified names each could
/// be ([`CallResidue::external_type_targets`]).
///
/// [`CallResidue::external_type_targets`]: crate::models::CallResidue
pub(crate) type ExternalTypeRows = BTreeMap<Vec<Vec<String>>, u64>;

impl ReferenceClasses {
    /// Classify `population` by the name test (i) over `universe`.
    #[cfg(test)]
    pub(crate) fn classify(population: &ReferencePopulation, universe: &NameUniverse) -> Self {
        Self::classify_with(population, universe, |_| {})
    }

    /// [`classify`](Self::classify), handing every group the name test (i)
    /// classified provably external to `on_external` — the rows a workspace
    /// re-reads against its other members' names (S-637).
    fn classify_with(
        population: &ReferencePopulation,
        universe: &NameUniverse,
        mut on_external: impl FnMut(&UnboundCodeTarget),
    ) -> Self {
        let mut classes = Self {
            doc_references: population.doc_references,
            doc_resolved: population.doc_resolved,
            code_references: population.code_references,
            code_resolved: population.code_resolved,
            ..Self::default()
        };
        let (calls, imports) = (
            i64::from(EdgeKind::Calls.as_i32()),
            i64::from(EdgeKind::Imports.as_i32()),
        );
        for group in &population.unbound_code {
            if universe.could_be_internal(group.language.as_deref(), group.kind, &group.target) {
                continue;
            }
            classes.provably_external += group.rows;
            on_external(group);
            let Some(language) = &group.language else {
                continue;
            };
            let row = classes.by_language.entry(language.clone()).or_default();
            if group.kind == calls {
                row.calls += group.rows;
            } else if group.kind == imports {
                row.imports += group.rows;
            }
        }
        classes
    }

    /// Add the `Calls` of `language` the binder proved `external-type`
    /// (`targets`, rows per target) that the name test over `universe` left
    /// internal candidates — test (ii). A target (i) already counted is
    /// skipped, so no row is counted twice.
    pub(crate) fn add_external_type(
        &mut self,
        universe: &NameUniverse,
        language: &str,
        targets: &BTreeMap<String, ExternalTypeRows>,
    ) {
        let rows: u64 = external_type_counted(universe, language, targets)
            .flat_map(|(_, by_candidates)| by_candidates.values())
            .sum();
        if rows == 0 {
            return;
        }
        self.provably_external += rows;
        self.by_language.entry(language.to_string()).or_default().calls += rows;
    }

    /// `language`'s provably-external rows per stated class, `0` for one with
    /// none.
    pub(crate) fn external_of(&self, language: &str) -> ExternalByClass {
        self.by_language.get(language).copied().unwrap_or_default()
    }

    /// The headline over these classes.
    pub(crate) fn internal_resolution(&self) -> InternalResolution {
        InternalResolution::measured(self.code_references, self.code_resolved, self.provably_external)
    }

    /// The doc-reference figure over these classes.
    pub(crate) fn doc_resolution(&self) -> DocResolution {
        DocResolution::measured(self.doc_references, self.doc_resolved)
    }
}

/// The targets of `language`'s `external-type` rows that test (ii) counts —
/// the ones the name test over `universe` left internal candidates, so no row
/// is counted twice. The one filter [`ReferenceClasses::add_external_type`]
/// counts and [`MemberReferenceFacts`] keeps.
fn external_type_counted<'a>(
    universe: &'a NameUniverse,
    language: &'a str,
    targets: &'a BTreeMap<String, ExternalTypeRows>,
) -> impl Iterator<Item = (&'a String, &'a ExternalTypeRows)> {
    let calls = i64::from(EdgeKind::Calls.as_i32());
    targets
        .iter()
        .filter(move |(target, _)| universe.could_be_internal(Some(language), calls, target))
}

/// What a workspace re-reads one member's reference classes by (S-637,
/// [FR-RS-49]): its name universe, the rows it counted provably external — by
/// the name test (i) and by the binder's `external-type` (ii) — and its unbound
/// invocation call sites. A member's own figure counts a reference into
/// another member as provably external, because its own graph names nothing
/// there; the workspace, which holds every member's names, can tell.
///
/// Never serialised; it lives only for one `workspace status`.
///
/// [FR-RS-49]: ../../../docs/specs/requirements/FR-RS-49.md
#[derive(Debug, Default)]
pub(crate) struct MemberReferenceFacts {
    /// Every name this member's graph declares.
    pub(crate) universe: NameUniverse,
    /// The groups test (i) classified provably external.
    pub(crate) named_nowhere: Vec<UnboundCodeTarget>,
    /// The `external-type` rows test (ii) added, per target, by the names each
    /// could be.
    pub(crate) external_type: Vec<(String, ExternalTypeRows)>,
    /// The unbound invocation call sites of the reported ledger
    /// ([`ReferencePopulation::unbound_invocations`]).
    pub(crate) unbound_invocations: Vec<UnboundInvocation>,
}

/// One graph's reference classes as `status` states them, and — when asked
/// for — the facts a workspace re-reads them by.
pub(crate) struct ClassifiedPopulation {
    /// The classes, test (ii) included.
    pub(crate) classes: ReferenceClasses,
    /// The facts [`MemberReferenceFacts`] keeps; `None` unless asked for.
    pub(crate) facts: Option<MemberReferenceFacts>,
}

/// Classify `population` by the name test (i) over `universe`, then add each
/// language's `external-type` rows by test (ii) (`residues`: language → its
/// [`CallResidue::external_type_targets`]) — the classes `status` states.
/// With `keep_facts`, also keep every row either test counted provably
/// external, for a workspace to re-read (S-637); a repository's own `status`
/// keeps none.
///
/// [`CallResidue::external_type_targets`]: crate::models::CallResidue
pub(crate) fn classify_population<'a>(
    population: ReferencePopulation,
    universe: NameUniverse,
    residues: impl IntoIterator<Item = (&'a str, &'a BTreeMap<String, ExternalTypeRows>)>,
    keep_facts: bool,
) -> ClassifiedPopulation {
    let mut named_nowhere = Vec::new();
    let mut classes = ReferenceClasses::classify_with(&population, &universe, |group| {
        if keep_facts {
            named_nowhere.push(group.clone());
        }
    });
    let mut external_type = Vec::new();
    for (language, targets) in residues {
        classes.add_external_type(&universe, language, targets);
        if keep_facts {
            external_type.extend(
                external_type_counted(&universe, language, targets)
                    .map(|(target, by_candidates)| (target.clone(), by_candidates.clone())),
            );
        }
    }
    let facts = keep_facts.then(|| MemberReferenceFacts {
        universe,
        named_nowhere,
        external_type,
        unbound_invocations: population.unbound_invocations,
    });
    ClassifiedPopulation { classes, facts }
}

/// Read one graph's reported ledger — one snapshot
/// ([`GraphStore::reference_population`]) — with the universe its targets are
/// tested against, built with `layout`'s file facades: what
/// [`classify_population`] classifies.
///
/// # Errors
/// Returns an error if the graph cannot be read.
pub(crate) fn read_population(
    store: &dyn GraphStore,
    layout: &PackageLayout,
) -> Result<(ReferencePopulation, NameUniverse)> {
    let population = store.reference_population()?;
    let universe = NameUniverse::build(&population.names, layout);
    Ok((population, universe))
}

#[cfg(test)]
#[path = "reference_classes_tests.rs"]
mod tests;
