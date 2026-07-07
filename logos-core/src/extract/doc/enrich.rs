//! swe-skills typed-node enrichment (S-039, [CR-003], [FR-DG-07], [ADR-19]).
//!
//! The generic documentation layer ([`super`], S-033) extracts every markdown
//! file into a [`NodeKind::DocFile`] root and a nested [`NodeKind::DocSection`]
//! tree — the layer that is primary on *any* repository. This module adds the
//! **additive** swe-skills enrichment on top: when a repository carries the
//! swe-skills convention layout, the convention artifacts are *relabelled* to
//! the typed [`NodeKind::Requirement`]/[`NodeKind::Adr`]/[`NodeKind::Story`]
//! kinds ([FR-DG-07]).
//!
//! # Relabel, not rebuild — identity is preserved
//!
//! Promotion changes only a node's *kind*, never its symbol. A doc symbol is a
//! pure function of `(path, scope-chain, ordinal)` ([`super::super::symbol`],
//! [ADR-07]) and is **kind-independent**: a `DocFile` and a `Requirement` at the
//! same path share one symbol string, as do a `DocSection` and a `Story` at the
//! same heading (the [`super::super::symbol::descriptor_for`] mapping emits the
//! same descriptor for both). So [`promote_facts`] is a one-column relabel that
//! leaves the byte-identical re-index ([NFR-RA-06]) and ID-stability ([NFR-RA-03])
//! guarantees untouched — toggling enrichment on or off never churns an id.
//!
//! # Never a prerequisite — auto-detected, generic-first
//!
//! [`conventions_present`] is the auto-detection signal: a repository "is a
//! swe-skills repo" when its indexed file set contains the convention layout
//! (a `requirements/FR-*.md`/`NFR-*.md` file or an `ADR-NN.md` decision file).
//! On a plain markdown repo the signal is false and nothing is promoted — only
//! generic doc nodes appear, and nothing errors ([FR-DG-07] AC, [ADR-19]
//! "generic-first"). The resolved trace web (`Story` → the `Requirement` it
//! implements, a `Requirement`'s dependency links) is the existing doc→doc
//! [`EdgeKind::DocReference`] resolution re-typed to [`EdgeKind::TracesTo`] in
//! the binder once both endpoints are typed — never fabricated, always through
//! the exactly-one-candidate ledger ([NFR-RA-05]).
//!
//! [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
//! [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
//! [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
//! [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
//! [NFR-RA-03]: ../../../../docs/specs/requirements/NFR-RA-03.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md

use crate::extract::Facts;
use crate::model::NodeKind;

/// The basename prefixes that mark a *requirement* convention file
/// (`FR-DG-07.md`, `NFR-RA-05.md`). Matched case-sensitively — the swe-skills
/// convention is upper-case — and only under a `requirements/` directory, so a
/// stray `fr-notes.md` elsewhere is never mistaken for a requirement.
const REQUIREMENT_PREFIXES: [&str; 2] = ["FR-", "NFR-"];

/// The directory segment that anchors the requirement convention
/// (`docs/specs/requirements/FR-*.md`). Anchoring on the directory keeps the
/// detector specific to the swe-skills layout.
const REQUIREMENTS_DIR: &str = "requirements";

/// `true` if `base` (a file basename) has a markdown extension. The doc plugin
/// already gated admission on the markdown grammar (S-034), so this only guards
/// the classifier against a non-markdown convention-named sibling.
fn is_markdown(base: &str) -> bool {
    matches!(
        base.rsplit_once('.'),
        Some((stem, "md" | "markdown")) if !stem.is_empty()
    )
}

/// `true` if `rel` is a swe-skills **requirement** file: an `FR-*`/`NFR-*`
/// markdown file directly inside a `requirements/` directory ([FR-DG-07]).
pub(crate) fn is_requirement_path(rel: &str) -> bool {
    let Some((parent, base)) = rel.rsplit_once('/') else {
        return false; // a top-level file is never a requirement (needs the dir anchor)
    };
    if parent.split('/').next_back() != Some(REQUIREMENTS_DIR) {
        return false;
    }
    is_markdown(base) && REQUIREMENT_PREFIXES.iter().any(|p| base.starts_with(p))
}

/// `true` if `rel` is a swe-skills **ADR** file: a basename of the form
/// `ADR-<digits>...md` (e.g. `ADR-19.md`, `ADR-07-determinism.md`). ADRs are
/// detected by basename alone — they conventionally live under a `decisions/`
/// directory, but the `ADR-NN` numbered prefix is the specific signal
/// ([FR-DG-07]).
pub(crate) fn is_adr_path(rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    if !is_markdown(base) {
        return false;
    }
    let Some(rest) = base.strip_prefix("ADR-") else {
        return false;
    };
    // At least one digit must immediately follow `ADR-` (`ADR-19`, `ADR-07-…`).
    rest.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// `true` if `heading` names a swe-skills **story**: a heading whose text begins
/// with the `S-<digits>` story id (e.g. `S-039: swe-skills typed-node
/// enrichment`). Stories live as journal/sprint *sections*, so this classifies a
/// [`NodeKind::DocSection`] by its heading, not a file by its path ([FR-DG-07]).
pub(crate) fn is_story_heading(heading: &str) -> bool {
    let Some(rest) = heading.trim_start().strip_prefix("S-") else {
        return false;
    };
    // The digits run, then a boundary (`:`, whitespace, or end) — so `S-39:` and
    // `S-39 Foo` match but `S-39x` (an unrelated token) does not.
    let mut chars = rest.chars();
    let mut saw_digit = false;
    for c in chars.by_ref() {
        if c.is_ascii_digit() {
            saw_digit = true;
        } else {
            return saw_digit && (c == ':' || c.is_whitespace());
        }
    }
    saw_digit
}

/// The typed node kind a *file* (`DocFile`) is promoted to, or `DocFile` itself
/// when the path matches no file-level convention. Requirement and ADR files are
/// whole-file artifacts; stories are sections and are classified separately by
/// [`is_story_heading`].
pub(crate) fn doc_file_kind(rel: &str) -> NodeKind {
    if is_requirement_path(rel) {
        NodeKind::Requirement
    } else if is_adr_path(rel) {
        NodeKind::Adr
    } else {
        NodeKind::DocFile
    }
}

/// `true` if any path in `paths` is a swe-skills convention file — the
/// auto-detection signal ([FR-DG-07]).
///
/// Anchored on the *file-level* conventions (requirement/ADR files), which carry
/// the distinctive `requirements/FR-*` layout and `ADR-NN` numbering. A repo with
/// only `S-NNN`-looking headings and no such files is not treated as a swe-skills
/// repo — story headings alone are too weak a signal to flip the whole repo on.
pub(crate) fn conventions_present<'a, I>(paths: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    paths
        .into_iter()
        .any(|p| is_requirement_path(p) || is_adr_path(p))
}

/// Relabel the doc nodes in `facts` to their typed kinds in place, when
/// enrichment is `active` ([FR-DG-07]).
///
/// A pure post-extraction relabel (see the module docs): a `DocFile` root takes
/// [`doc_file_kind`] of its file path; a `DocSection` whose heading
/// [`is_story_heading`] becomes a [`NodeKind::Story`]. Already-generic code facts
/// carry no `DocFile`/`DocSection` nodes, so they are untouched. A no-op when
/// `active` is false — the generic layer stands alone.
pub(crate) fn promote_facts(facts: &mut [Facts], active: bool) {
    if !active {
        return;
    }
    for f in facts.iter_mut() {
        // The file path is the DocFile root's identity source; classify it once.
        let file_kind = doc_file_kind(&f.path);
        for node in &mut f.nodes {
            match node.kind {
                NodeKind::DocFile => node.kind = file_kind,
                NodeKind::DocSection if is_story_heading(&node.name) => {
                    node.kind = NodeKind::Story;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_paths_need_the_directory_and_prefix() {
        assert!(is_requirement_path("docs/specs/requirements/FR-DG-07.md"));
        assert!(is_requirement_path("docs/specs/requirements/NFR-RA-05.md"));
        assert!(is_requirement_path("requirements/FR-1.md"));
        // Wrong directory: an FR-named file outside requirements/ is not promoted.
        assert!(!is_requirement_path("docs/FR-DG-07.md"));
        // Wrong prefix / casing.
        assert!(!is_requirement_path("docs/specs/requirements/fr-dg-07.md"));
        assert!(!is_requirement_path("docs/specs/requirements/guide.md"));
        // Not markdown.
        assert!(!is_requirement_path("docs/specs/requirements/FR-DG-07.txt"));
        // Top-level file (no directory anchor).
        assert!(!is_requirement_path("FR-DG-07.md"));
    }

    #[test]
    fn adr_paths_are_detected_by_numbered_basename() {
        assert!(is_adr_path("docs/specs/architecture/decisions/ADR-19.md"));
        assert!(is_adr_path(
            "docs/specs/architecture/decisions/ADR-07-determinism.md"
        ));
        assert!(is_adr_path("ADR-1.markdown"));
        // No digit after the prefix → not an ADR (an `ADR-foo.md` note).
        assert!(!is_adr_path("docs/ADR-overview.md"));
        assert!(!is_adr_path("docs/adr-19.md"));
        assert!(!is_adr_path("docs/decisions/notes.md"));
    }

    #[test]
    fn story_headings_match_the_s_nnn_id_at_a_boundary() {
        assert!(is_story_heading("S-039: swe-skills typed-node enrichment"));
        assert!(is_story_heading("S-33 Foo"));
        assert!(is_story_heading("  S-1  "));
        assert!(is_story_heading("S-7"));
        // A bare prose heading, or an `S-`-looking token that is not a story id.
        assert!(!is_story_heading("Setup"));
        assert!(!is_story_heading("S-39x mutant"));
        assert!(!is_story_heading("S-"));
        assert!(!is_story_heading("Section 1"));
    }

    #[test]
    fn doc_file_kind_maps_each_convention() {
        assert_eq!(
            doc_file_kind("docs/specs/requirements/FR-DG-07.md"),
            NodeKind::Requirement
        );
        assert_eq!(
            doc_file_kind("docs/specs/architecture/decisions/ADR-19.md"),
            NodeKind::Adr
        );
        assert_eq!(doc_file_kind("docs/planning/journal.md"), NodeKind::DocFile);
        assert_eq!(doc_file_kind("README.md"), NodeKind::DocFile);
    }

    #[test]
    fn conventions_present_keys_on_file_level_artifacts() {
        assert!(conventions_present(
            ["src/lib.rs", "docs/specs/requirements/FR-DG-07.md"].into_iter()
        ));
        assert!(conventions_present(
            ["docs/specs/architecture/decisions/ADR-19.md"].into_iter()
        ));
        // A plain markdown repo: no requirement/ADR files → not a swe-skills repo.
        assert!(!conventions_present(
            ["README.md", "docs/guide.md", "src/main.rs"].into_iter()
        ));
        assert!(!conventions_present(std::iter::empty()));
    }
}
