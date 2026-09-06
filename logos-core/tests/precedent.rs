//! Structural precedent (S-359 / [FR-NV-12], [FR-NV-04], [NFR-CC-04], CR-114),
//! exercised end-to-end through `Engine::precedent` against real temp-directory
//! fixtures.
//!
//! Coverage by acceptance criterion:
//! - analogous nodes come back ranked by a **stated, deterministic** notion —
//!   every number the order is computed from rides on the payload, and the order
//!   is re-derived by hand here from those numbers alone (AC 1);
//! - every result names **why** it is analogous, through which nodes (AC 2);
//! - asked for one language plugin's capability arm, the sibling arms come back
//!   ahead of everything else (AC 3);
//! - an empty answer always carries its reason from a closed vocabulary, and
//!   never a relaxed-notion guess (AC 4).
//!
//! [FR-NV-12]: ../../docs/specs/requirements/FR-NV-12.md
//! [FR-NV-04]: ../../docs/specs/requirements/FR-NV-04.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use logos_core::models::navigation::{
    Precedent, PrecedentFacet, PrecedentResult, PrecedentTargetKind, MIN_SHARED_CALLEES,
};
use logos_core::Engine;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A language-plugin registry, the shape [FR-NV-12] AC 3 names: one trait, three
/// per-language capability arms implementing it through the same two helpers,
/// and one registry module that imports all three types.
///
/// Deliberately named so that **no** textual similarity connects the arms to one
/// another beyond the word `Arm` — the point of the query is that it finds the
/// sibling by structure, at the moment a grep for a name you have not chosen yet
/// cannot.
fn plugin_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/plugin.rs",
        "pub trait LanguagePlugin {\n    fn extract(&self);\n}\n",
    );
    write(
        tmp.path(),
        "src/shared.rs",
        "pub fn parse_source() {}\npub fn emit_facts() {}\n",
    );
    for (file, ty) in [
        ("src/rust_arm.rs", "RustArm"),
        ("src/python_arm.rs", "PythonArm"),
        ("src/go_arm.rs", "GoArm"),
    ] {
        write(
            tmp.path(),
            file,
            &format!(
                "use crate::plugin::LanguagePlugin;\n\
                 use crate::shared::{{parse_source, emit_facts}};\n\n\
                 pub struct {ty};\n\n\
                 impl LanguagePlugin for {ty} {{\n\
                 \x20   fn extract(&self) {{\n\
                 \x20       parse_source();\n\
                 \x20       emit_facts();\n\
                 \x20   }}\n\
                 }}\n\n\
                 pub fn {init}() {{}}\n",
                init = arm_init(file)
            ),
        );
    }
    // A dispatcher that CALLS each arm's init. This is what a registration is:
    // a node that does something with all of them. It is deliberately not a
    // module `use` list — `is_registration_edge` records why co-import was
    // tried, measured, and rejected.
    write(
        tmp.path(),
        "src/registry.rs",
        "use crate::rust_arm::rust_arm_init;\n\
         use crate::python_arm::python_arm_init;\n\
         use crate::go_arm::go_arm_init;\n\n\
         pub fn register_all() {\n\
         \x20   rust_arm_init();\n\
         \x20   python_arm_init();\n\
         \x20   go_arm_init();\n\
         }\n",
    );
    // A symbol attached to nothing at all: no supertype, no registrar, no call.
    write(tmp.path(), "src/lonely.rs", "pub fn lonely_helper() {}\n");
    tmp
}

/// The init-function name for the arm defined in `file` (`src/rust_arm.rs` →
/// `rust_arm_init`).
fn arm_init(file: &str) -> String {
    let stem = file.trim_start_matches("src/").trim_end_matches(".rs");
    format!("{stem}_init")
}

/// An indexed engine over `tmp`.
fn indexed_engine(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// The canonical symbol of the `extract` method defined in `file`.
///
/// Spelled out rather than resolved by bare name on purpose: three methods are
/// named `extract`, and a test that let the resolver pick would be asserting
/// against whichever one it picked.
fn extract_in(file: &str) -> String {
    format!("logos . . . src/`{file}`/extract().")
}

/// The precedent named `name`, or a panic naming what did come back.
fn by_name<'a>(result: &'a PrecedentResult, name: &str) -> &'a Precedent {
    result
        .precedents
        .iter()
        .find(|p| p.symbol.name == name)
        .unwrap_or_else(|| panic!("no precedent named {name}: {}", summarise(result)))
}

/// A one-line-per-result rendering, for assertion failure messages.
fn summarise(result: &PrecedentResult) -> String {
    let mut out = format!(
        "target_kind={:?} total_found={} empty={:?}",
        result.target_kind, result.total_found, result.empty_reason
    );
    for p in &result.precedents {
        let _ = write!(
            out,
            "\n  {} [{}] facets={} sup={} reg={} cal={}",
            p.symbol.name,
            p.symbol.file.as_deref().unwrap_or("-"),
            p.rank.facets,
            p.rank.shared_supertypes,
            p.rank.shared_registrations,
            p.rank.shared_callees
        );
    }
    out
}

/// The rank key exactly as the payload's own `ranked_by` sentence describes it:
/// facet count, then the three per-facet counts, all descending, then canonical
/// symbol ascending. Negated so a plain ascending sort reproduces the order.
fn stated_rank_key(p: &Precedent) -> (i64, i64, i64, i64, &str) {
    (
        -(p.rank.facets as i64),
        -(p.rank.shared_supertypes as i64),
        -(p.rank.shared_registrations as i64),
        -(p.rank.shared_callees as i64),
        p.symbol.symbol.as_str(),
    )
}

// ── FR-NV-12 AC 3: the headline — the sibling arms of one capability ─────────

/// Asked for one language plugin's capability arm, the query returns the other
/// arms — ahead of everything else, because they match two facets rather than
/// one.
///
/// This is the question the requirement was written for. Note what the answer
/// does **not** rest on: the three arms share no distinctive name, live in three
/// files, and are never called from one another. What connects them is the trait
/// they implement and the helpers they call, and that is what the payload says.
#[test]
fn one_capability_arm_returns_its_sibling_arms() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent(&extract_in("rust_arm.rs"), None);

    assert!(
        result.empty_reason.is_none(),
        "a sibling exists: {}",
        summarise(&result)
    );
    let files: Vec<&str> = result
        .precedents
        .iter()
        .map(|p| p.symbol.file.as_deref().unwrap_or("-"))
        .collect();
    assert_eq!(
        files,
        vec!["src/go_arm.rs", "src/python_arm.rs"],
        "both sibling arms, and only them: {}",
        summarise(&result)
    );
    for precedent in &result.precedents {
        assert_eq!(precedent.symbol.name, "extract");
        assert_eq!(precedent.rank.facets, 2, "{}", summarise(&result));
        assert_eq!(precedent.rank.shared_supertypes, 1);
        assert_eq!(precedent.rank.shared_callees, 2);
    }
    assert_eq!(result.target_kind, PrecedentTargetKind::Symbol);
    assert_eq!(result.total_found, 2);
    assert_eq!(result.elided, 0);
}

// ── FR-NV-12 AC 2: each result names WHY ─────────────────────────────────────

/// Every result carries at least one reason; every reason names its facet, the
/// nodes the analogy runs through, and says so in words that mention those nodes
/// by name.
///
/// The last clause is the one that matters. An `explanation` that did not name
/// its `via` nodes would satisfy "has a reason" while telling the reader nothing
/// they could check — which is the fuzzy-score failure wearing prose.
#[test]
fn every_result_names_why_it_is_analogous_and_through_which_nodes() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent(&extract_in("rust_arm.rs"), None);

    for precedent in &result.precedents {
        assert!(
            !precedent.reasons.is_empty(),
            "a precedent with no reason is a guess: {}",
            summarise(&result)
        );
        assert_eq!(
            precedent.reasons.len() as u32,
            precedent.rank.facets,
            "one reason per matched facet"
        );
        for reason in &precedent.reasons {
            assert!(
                !reason.via.is_empty(),
                "{} names no shared node",
                reason.facet.as_str()
            );
            assert_eq!(reason.via_total, reason.via.len() as u32 + reason.via_elided);
            for via in &reason.via {
                assert!(
                    reason.explanation.contains(&via.name),
                    "the explanation {:?} must name the node {:?} it runs through",
                    reason.explanation,
                    via.name
                );
                assert!(
                    !via.symbol.is_empty(),
                    "a shared node must round-trip into another navigation tool"
                );
            }
        }
    }

    // The two facets that actually fire here, named and attributed.
    let sibling = by_name(&result, "extract");
    let supertype = sibling
        .reasons
        .iter()
        .find(|r| r.facet == PrecedentFacet::SharedSupertype)
        .expect("the shared trait is a reason");
    assert_eq!(
        supertype.via.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        vec!["LanguagePlugin"]
    );
    let callees = sibling
        .reasons
        .iter()
        .find(|r| r.facet == PrecedentFacet::SharedCallee)
        .expect("the matching call shape is a reason");
    assert_eq!(
        callees.via.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        vec!["emit_facts", "parse_source"]
    );
}

// ── FR-NV-12 AC 1: a stated, deterministic notion — never an opaque score ────

/// The order is re-derived here from the `rank` numbers on the payload alone and
/// must match the order shipped.
///
/// That is the whole content of "never an opaque score": a consumer holding only
/// the response can reproduce the ranking. If any hidden weight ever entered the
/// comparison, this recomputation would disagree with the shipped order.
#[test]
fn the_shipped_order_is_reproducible_from_the_payloads_own_numbers() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    // The file target is the richest case: two-facet method siblings and
    // one-facet type siblings in one list, so the ordering is actually tested
    // rather than trivially satisfied by a uniform list.
    let result = engine.precedent("src/rust_arm.rs", None);
    assert!(result.precedents.len() >= 4, "{}", summarise(&result));

    let shipped: Vec<&str> = result
        .precedents
        .iter()
        .map(|p| p.symbol.symbol.as_str())
        .collect();
    let mut recomputed: Vec<&Precedent> = result.precedents.iter().collect();
    recomputed.sort_by_key(|p| stated_rank_key(p));
    assert_eq!(
        shipped,
        recomputed
            .iter()
            .map(|p| p.symbol.symbol.as_str())
            .collect::<Vec<_>>(),
        "the shipped order must be exactly the stated rank key: {}",
        summarise(&result)
    );
    // And the stated rule really does separate these results, rather than the
    // list happening to be already sorted by symbol.
    assert_eq!(result.precedents[0].rank.facets, 2);
    assert_eq!(result.precedents.last().unwrap().rank.facets, 1);

    // Every count the ranking reads is on the wire, and the notion and the rule
    // that consume them are stated in full.
    assert!(result.notion.contains("shared_supertype"));
    assert!(result.notion.contains("shared_registration"));
    assert!(result.notion.contains("shared_callee"));
    assert!(
        result.notion.contains(&MIN_SHARED_CALLEES.to_string()),
        "the notion must quote the call-shape threshold it applies"
    );
    assert!(result.ranked_by.contains("canonical symbol ascending"));
}

/// Two runs over one index agree byte-for-byte ([NFR-RA-06]) — including the
/// tie-break, which is what decides the order of the two sibling arms.
#[test]
fn the_answer_is_deterministic_and_ties_break_on_the_canonical_symbol() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let first = serde_json::to_string(&engine.precedent(&extract_in("rust_arm.rs"), None)).unwrap();
    let second = serde_json::to_string(&engine.precedent(&extract_in("rust_arm.rs"), None)).unwrap();
    assert_eq!(first, second, "two identical calls must agree exactly");

    // `go_arm` before `python_arm`: identical ranks, so the canonical symbol
    // decides — and `g` sorts before `p`.
    let result = engine.precedent(&extract_in("rust_arm.rs"), None);
    assert!(
        result.precedents[0].symbol.symbol < result.precedents[1].symbol.symbol,
        "{}",
        summarise(&result)
    );
}

// ── The three facets, each isolated ──────────────────────────────────────────

/// The registration facet on its own: the three arm-init functions implement
/// nothing and call nothing, and are connected only by the dispatcher that
/// calls all three.
#[test]
fn a_shared_registration_alone_is_a_reason() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("rust_arm_init", None);

    let names: Vec<&str> = result
        .precedents
        .iter()
        .map(|p| p.symbol.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["go_arm_init", "python_arm_init"],
        "{}",
        summarise(&result)
    );
    let sibling = by_name(&result, "go_arm_init");
    assert_eq!(sibling.rank.facets, 1);
    assert_eq!(sibling.rank.shared_registrations, 1);
    assert_eq!(sibling.reasons[0].facet, PrecedentFacet::SharedRegistration);
    assert_eq!(
        sibling.reasons[0].via[0].name, "register_all",
        "the registrar is the dispatcher that calls both, not the module that \
         imports them: {}",
        summarise(&result)
    );
}

/// The regression that removing `EdgeKind::Imports` from the registration facet
/// exists to prevent: two symbols that share nothing but a `use` list are NOT
/// analogous.
///
/// Measured against this repository's own index, admitting co-import returned 67
/// "precedents" for one enum — a `usize` constant and a test function among them
/// — all tied at one facet, so the reported slice was the alphabetical head.
/// That is the low-confidence guess [FR-NV-12] AC 4 forbids, wearing a named
/// reason.
#[test]
fn co_import_is_not_a_registration() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/alpha.rs",
        "pub struct Alpha;\npub const ALPHA_LIMIT: usize = 3;\n",
    );
    write(tmp.path(), "src/beta.rs", "pub struct Beta;\n");
    // One consumer whose `use` list names all three, and which does nothing
    // else with them — the shape that flooded the real index.
    write(
        tmp.path(),
        "src/consumer.rs",
        "use crate::alpha::{Alpha, ALPHA_LIMIT};\nuse crate::beta::Beta;\n",
    );
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("Alpha", None);

    assert!(
        result.precedents.is_empty(),
        "appearing in the same `use` list is not structural analogy: {}",
        summarise(&result)
    );
    assert_eq!(
        result.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("no_structural_anchors"),
        "{}",
        summarise(&result)
    );
}

/// One shared callee is not a call shape ([`MIN_SHARED_CALLEES`]), and the
/// candidates the threshold drops are **counted** on the coverage block.
///
/// A threshold nobody can see is indistinguishable from a bug: without the
/// counter, a reader whose obvious sibling is missing has no way to tell whether
/// the query considered and rejected it or never saw it at all ([NFR-CC-04]).
#[test]
fn a_single_shared_callee_is_not_a_call_shape_and_the_drop_is_counted() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/util.rs", "pub fn helper() {}\npub fn other() {}\n");
    // `twin` shares BOTH helpers with `subject`; `passerby` shares only one.
    write(
        tmp.path(),
        "src/subject.rs",
        "use crate::util::{helper, other};\npub fn subject() {\n    helper();\n    other();\n}\n",
    );
    write(
        tmp.path(),
        "src/twin.rs",
        "use crate::util::{helper, other};\npub fn twin() {\n    helper();\n    other();\n}\n",
    );
    write(
        tmp.path(),
        "src/passerby.rs",
        "use crate::util::helper;\npub fn passerby() {\n    helper();\n}\n",
    );
    let engine = indexed_engine(&tmp);

    // The canonical symbol, not the bare name: `subject` also names the module
    // the file becomes, and the module is the lower-id resolution.
    let result = engine.precedent("logos . . . src/`subject.rs`/subject().", None);

    let names: Vec<&str> = result
        .precedents
        .iter()
        .map(|p| p.symbol.name.as_str())
        .collect();
    assert!(
        names.contains(&"twin"),
        "two shared callees is a call shape: {}",
        summarise(&result)
    );
    assert!(
        !names.contains(&"passerby"),
        "one shared callee is coincidence, not a call shape: {}",
        summarise(&result)
    );
    assert_eq!(
        result.coverage.dropped_single_callee_matches, 1,
        "the threshold's drops must be visible, not silent: {}",
        summarise(&result)
    );
    assert!(
        result.coverage.candidates_considered >= 2,
        "the dropped candidate was considered: {}",
        summarise(&result)
    );
}

/// A helper called from everywhere is not evidence of analogy. Above the stated
/// fan bound the anchor contributes nothing and is **named** in the coverage
/// block, so an empty or thin answer stays diagnosable.
#[test]
fn a_ubiquitous_helper_is_named_rather_than_treated_as_evidence() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/util.rs", "pub fn ubiquitous() {}\n");
    // 240 callers — comfortably past the stated 200-node fan bound.
    let mut body = String::from("use crate::util::ubiquitous;\n");
    for n in 0..240 {
        let _ = write!(body, "pub fn caller_{n}() {{\n    ubiquitous();\n}}\n");
    }
    write(tmp.path(), "src/callers.rs", &body);
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("caller_0", None);

    // NOT `anchors_are_unshared`: the anchor is attached to 239 other nodes,
    // which is exactly why it was dropped. A consumer branching on the closed
    // vocabulary must not read "nothing resembles you" off a graph that
    // resembles you too much.
    assert_eq!(
        result.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("anchors_are_ubiquitous"),
        "{}",
        summarise(&result)
    );
    assert!(
        result
            .empty_reason
            .as_ref()
            .unwrap()
            .detail
            .contains("shared with everything"),
        "the detail must state the opposite claim from `anchors_are_unshared`"
    );
    let named: Vec<&str> = result
        .coverage
        .ubiquitous_anchors
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert!(
        named.contains(&"ubiquitous"),
        "the discarded anchor must be named: {named:?}"
    );
    let anchor = result
        .coverage
        .ubiquitous_anchors
        .iter()
        .find(|a| a.name == "ubiquitous")
        .unwrap();
    assert_eq!(anchor.facet, PrecedentFacet::SharedCallee);
    assert!(anchor.sharers >= 240, "{} sharers reported", anchor.sharers);
}

// ── FR-NV-12: a file target ([FR-NV-04] resolution, symbol first) ────────────

/// A file target compares every symbol the file defines, and the precedents are
/// still nodes — so a sibling *file* surfaces as a cluster of its symbols, each
/// naming its file.
#[test]
fn a_file_target_compares_every_symbol_the_file_defines() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("src/rust_arm.rs", None);

    assert_eq!(result.target_kind, PrecedentTargetKind::File);
    assert_eq!(result.target_file.as_deref(), Some("src/rust_arm.rs"));
    assert!(result.target_symbol.is_none());
    let compared: Vec<&str> = result
        .coverage
        .compared
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(
        compared,
        vec!["rust_arm", "RustArm", "extract", "rust_arm_init"],
        "every symbol the file defines is compared, in canonical-symbol order"
    );
    // Nothing from the target file may come back as its own precedent.
    for precedent in &result.precedents {
        assert_ne!(
            precedent.symbol.file.as_deref(),
            Some("src/rust_arm.rs"),
            "the target file cannot be its own precedent: {}",
            summarise(&result)
        );
    }
    // Both sibling arms surface, each naming its file — the "sibling file"
    // reading of the answer.
    let sibling_files: Vec<&str> = result
        .precedents
        .iter()
        .filter_map(|p| p.symbol.file.as_deref())
        .collect();
    assert!(sibling_files.contains(&"src/go_arm.rs"));
    assert!(sibling_files.contains(&"src/python_arm.rs"));
}

/// A `./`-prefixed path normalises to the stored project-relative form, as it
/// does for `affected`.
#[test]
fn a_dot_slash_prefixed_path_resolves_to_the_same_file() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let plain = engine.precedent("src/rust_arm.rs", None);
    let prefixed = engine.precedent("./src/rust_arm.rs", None);

    assert_eq!(prefixed.target_kind, PrecedentTargetKind::File);
    assert_eq!(prefixed.target_file, plain.target_file);
    assert_eq!(prefixed.total_found, plain.total_found);
}

// ── FR-NV-12 AC 4: an empty result states its reason ─────────────────────────

/// The four ways an answer can legitimately be empty, each naming which one it
/// is from the closed vocabulary — never a relaxed-notion guess.
#[test]
fn every_empty_answer_names_its_reason_from_the_closed_vocabulary() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    // 1. Nothing answers to the text at all — with "did you mean" names.
    let unknown = engine.precedent("no_such_thing_anywhere", None);
    assert!(unknown.precedents.is_empty());
    assert_eq!(
        unknown.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("target_unresolved")
    );
    assert_eq!(unknown.target_kind, PrecedentTargetKind::Unresolved);
    assert!(unknown
        .empty_reason
        .as_ref()
        .unwrap()
        .detail
        .contains("no_such_thing_anywhere"));

    // 2. A real symbol attached to nothing: no supertype, no registrar, no call.
    let lonely = engine.precedent("lonely_helper", None);
    assert!(lonely.precedents.is_empty(), "{}", summarise(&lonely));
    assert_eq!(
        lonely.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("no_structural_anchors"),
        "{}",
        summarise(&lonely)
    );
    assert!(lonely.target_symbol.is_some(), "the target itself resolved");

    // 3. Nothing indexed at all — distinguished from "nothing analogous",
    //    because the two call for completely different next actions.
    let bare = TempDir::new().unwrap();
    let unindexed = Engine::start(bare.path()).expect("engine starts");
    let empty = unindexed.precedent("anything", None);
    assert!(matches!(
        empty.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("graph_empty") | Some("target_unresolved")
    ));

    // Every one of them still states the notion, the ranking rule and the
    // coverage limits: the answers that carry no data are the ones that most
    // need their limits stated ([NFR-CC-04]).
    for result in [&unknown, &lonely, &empty] {
        assert!(!result.notion.is_empty());
        assert!(!result.ranked_by.is_empty());
        assert!(result.coverage.statement.contains("not found"));
        assert_eq!(result.total_found, 0);
        assert_eq!(result.elided, 0);
    }
}

/// A documentation node is indexed but holds no vertex in the symbol graph, so
/// its structure cannot be compared — said out loud rather than reported as
/// "nothing analogous", which would be a different and false claim.
#[test]
fn a_documentation_target_reports_that_it_holds_no_vertex() {
    let tmp = plugin_fixture();
    write(
        tmp.path(),
        "docs/design.md",
        "# Design\n\nThe plugin arms are described here.\n",
    );
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("docs/design.md", None);

    // Pinned exactly, not as a disjunction: the three plausible codes make
    // materially different claims ("its structure cannot be compared" vs
    // "nothing analogous exists"), and accepting any of them would defeat the
    // point of a closed vocabulary.
    assert_eq!(
        result.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("target_absent_from_view"),
        "{}",
        summarise(&result)
    );
    assert!(result.precedents.is_empty());
    // The per-seed warning is the only signal that a resolved symbol was
    // silently left out of the comparison.
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("absent from the hydrated symbol view")),
        "the dropped seed must be named: {:?}",
        result.warnings
    );
}

/// An indexed project holding no code at all reports `graph_empty` — a
/// different claim from "nothing analogous", and one that calls for indexing
/// rather than for a different target.
#[test]
fn a_project_with_no_code_reports_graph_empty_not_nothing_analogous() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "docs/one.md", "# One\n\nProse.\n");
    write(tmp.path(), "docs/two.md", "# Two\n\nMore prose.\n");
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("docs/one.md", None);

    assert_eq!(
        result.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("graph_empty"),
        "{}",
        summarise(&result)
    );
    assert!(result
        .empty_reason
        .as_ref()
        .unwrap()
        .detail
        .contains("nothing is indexed"));
}

/// A near-miss target earns "did you mean" names ([FR-NV-09]).
///
/// Deleting the one line that populates `suggestions` was invisible to every
/// other test in this suite, the CLI suite and the web suite.
#[test]
fn a_near_miss_target_earns_did_you_mean_suggestions() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("parse_sour", None);

    assert_eq!(
        result.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("target_unresolved")
    );
    assert!(
        result.suggestions.iter().any(|s| s == "parse_source"),
        "a near miss must suggest the name it nearly hit: {:?}",
        result.suggestions
    );
}

/// `MAX_VIA_LISTED` truncates a reason's shared-node list, and the count and
/// the prose both stay honest ([NFR-CC-04]).
#[test]
fn a_long_shared_list_is_truncated_counted_and_says_so() {
    let tmp = TempDir::new().unwrap();
    let helpers: Vec<String> = (0..8).map(|n| format!("helper_{n}")).collect();
    write(
        tmp.path(),
        "src/util.rs",
        &helpers
            .iter()
            .map(|h| format!("pub fn {h}() {{}}\n"))
            .collect::<String>(),
    );
    for who in ["subject", "twin"] {
        let calls: String = helpers.iter().map(|h| format!("    {h}();\n")).collect();
        write(
            tmp.path(),
            &format!("src/{who}.rs"),
            &format!(
                "use crate::util::{{{}}};\npub fn {who}() {{\n{calls}}}\n",
                helpers.join(", ")
            ),
        );
    }
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("logos . . . src/`subject.rs`/subject().", None);

    let twin = by_name(&result, "twin");
    let reason = &twin.reasons[0];
    assert_eq!(reason.facet, PrecedentFacet::SharedCallee);
    assert_eq!(reason.via_total, 8, "the full count, not the listed length");
    assert_eq!(reason.via.len(), 5, "bounded by MAX_VIA_LISTED");
    assert_eq!(reason.via_elided, 3);
    assert!(
        reason.explanation.ends_with("+3 more"),
        "the prose must name the elision: {:?}",
        reason.explanation
    );
    // The listed slice is the deterministic head, not an arbitrary five.
    let listed: Vec<&str> = reason.via.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        listed,
        vec!["helper_0", "helper_1", "helper_2", "helper_3", "helper_4"],
        "the listed slice is canonical-symbol ascending"
    );
}

/// `MAX_PRECEDENT_LIMIT` caps an over-large request, and the remainder is
/// counted rather than silently dropped.
#[test]
fn an_over_large_limit_is_clamped_and_the_remainder_counted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/util.rs",
        "pub fn helper_a() {}\npub fn helper_b() {}\n",
    );
    // 150 siblings, all sharing the same two callees — comfortably past the
    // hard ceiling of 100.
    let mut body = String::from("use crate::util::{helper_a, helper_b};\n");
    for n in 0..150 {
        let _ = write!(
            body,
            "pub fn sibling_{n}() {{\n    helper_a();\n    helper_b();\n}}\n"
        );
    }
    write(tmp.path(), "src/siblings.rs", &body);
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("logos . . . src/`siblings.rs`/sibling_0().", Some(10_000));

    assert_eq!(
        result.precedents.len(),
        100,
        "the hard ceiling applies however large the request: {}",
        summarise(&result)
    );
    assert_eq!(result.total_found, 149, "{}", summarise(&result));
    assert_eq!(result.elided, 49);
}

// ── Bounds, ambiguity and the coverage block ─────────────────────────────────

/// The limit bounds the list and the remainder is **counted**, never silently
/// dropped ([NFR-CC-04]).
#[test]
fn the_limit_bounds_the_list_and_the_remainder_is_counted() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let full = engine.precedent("src/rust_arm.rs", None);
    let capped = engine.precedent("src/rust_arm.rs", Some(1));

    assert!(full.total_found >= 2, "{}", summarise(&full));
    assert_eq!(capped.precedents.len(), 1);
    assert_eq!(capped.total_found, full.total_found);
    assert_eq!(capped.elided, full.total_found - 1);
    // The one kept is the best one, not an arbitrary one.
    assert_eq!(
        capped.precedents[0].symbol.symbol,
        full.precedents[0].symbol.symbol
    );
}

/// A bare name matching several symbols resolves to one of them, and says so.
///
/// `impact` can leave this implicit because its caller reads the resolved node
/// back; here the answer is "copy this precedent", and being shown the
/// precedents of the wrong `extract` is a wasted edit ([NFR-CC-04]).
#[test]
fn an_ambiguous_bare_name_is_warned_about_rather_than_silently_picked() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("extract", None);

    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("matched 3 symbols by name")),
        "the guess must be disclosed: {:?}",
        result.warnings
    );
    assert!(result.target_symbol.is_some());
}

/// The coverage block reports what was compared and how much was set aside, on
/// a successful answer as well as an empty one.
#[test]
fn the_coverage_block_states_what_was_compared() {
    let tmp = plugin_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.precedent(&extract_in("rust_arm.rs"), None);

    assert_eq!(result.coverage.compared.len(), 1);
    assert_eq!(result.coverage.compared[0].name, "extract");
    assert_eq!(result.coverage.compared[0].file.as_deref(), Some("src/rust_arm.rs"));
    assert_eq!(result.coverage.compared_elided, 0);
    assert!(result.coverage.candidates_considered >= 2);
    assert!(
        result.coverage.statement.contains("Lexical containment is never a reason"),
        "the statement must rule out the analogy it deliberately does not draw"
    );
}

/// Sharing a parent module is not an analogy: two functions that sit in one file
/// and are otherwise unconnected are not precedents for one another.
///
/// The full symbol view this query runs on **does** carry the lexical `Contains`
/// edge (it is the view that has `Implements`, which the dependency view drops),
/// so this is a live risk rather than a hypothetical one.
#[test]
fn sharing_a_parent_module_is_not_an_analogy() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/neighbours.rs",
        "pub fn first() {}\npub fn second() {}\npub fn third() {}\n",
    );
    let engine = indexed_engine(&tmp);

    let result = engine.precedent("first", None);

    assert!(
        result.precedents.is_empty(),
        "co-residence in one module is not structural analogy: {}",
        summarise(&result)
    );
    assert_eq!(
        result.empty_reason.as_ref().map(|e| e.code.as_str()),
        Some("no_structural_anchors"),
        "{}",
        summarise(&result)
    );
}

/// The [FR-NV-12] AC 4 invariant, swept rather than spot-checked: across every
/// target shape this suite can reach, an empty `precedents` list **always**
/// carries a reason, and a non-empty one never does.
///
/// Spot checks pin the reasons this build produces today; this pins the property
/// a future facet or threshold must not break — a new early return that forgets
/// its reason would ship a silent empty answer, which reads as "nothing
/// analogous exists" and would be false.
#[test]
fn an_empty_list_always_names_its_reason() {
    let tmp = plugin_fixture();
    write(tmp.path(), "docs/design.md", "# Design\n\nProse.\n");
    let engine = indexed_engine(&tmp);

    for target in [
        &extract_in("rust_arm.rs") as &str,
        "src/rust_arm.rs",
        "./src/rust_arm.rs",
        "RustArm",
        "LanguagePlugin",
        "register_all",
        "lonely_helper",
        "docs/design.md",
        "src/no_such_file.rs",
        "no_such_thing_anywhere",
        "",
    ] {
        for limit in [None, Some(0), Some(1), Some(10_000)] {
            let result = engine.precedent(target, limit);
            assert_eq!(
                result.precedents.is_empty(),
                result.empty_reason.is_some(),
                "target {target:?} limit {limit:?}: an empty list must name its reason and a \
                 non-empty one must not carry one — {}",
                summarise(&result)
            );
            // Every answer, empty or not, states the notion it applied.
            assert!(!result.notion.is_empty(), "{target:?}");
            assert!(!result.ranked_by.is_empty(), "{target:?}");
            assert!(!result.coverage.statement.is_empty(), "{target:?}");
            // A `limit` of 0 must not silently produce a reasonless empty list.
            assert!(
                result.precedents.len() as u32 + result.elided >= result.total_found.min(1),
                "{target:?} limit {limit:?}: {}",
                summarise(&result)
            );
        }
    }
}
