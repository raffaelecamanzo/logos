//! **The structural arm of the absence audit: every relational answer type,
//! enumerated from its definition, carries the resolution denominator**
//! ([S-443], [CR-143] §3.3, [FR-NV-14], [NFR-CC-04]).
//!
//! # Why a second arm exists
//!
//! `absence_taxonomy_audit.rs` is **lexical**: it walks three surfaces for the
//! [`absence::SENTINELS`] spellings, and says of itself that it cannot catch a
//! site that reports an absence in words nobody has used before. A relational
//! answer that reports nothing uses no words at all — `{"affected":[],
//! "warnings":[]}` has no lexis — so the six answer sites [CR-143] §1 names sat
//! inside the census's roots and the census could not see them. This arm checks
//! **shape** instead: an answer type that could come back empty must carry the
//! field that says what it was computed over.
//!
//! # How the two arms divide the class
//!
//! | An absence that… | Arm | Where |
//! |---|---|---|
//! | is **spelled** — words on a CLI line, an SPA label, a read-model tag | lexical census | `absence_taxonomy_audit.rs`, over `logos-core/src`, `cli/src`, `web/ui/src` |
//! | is **silent** — an empty or short set with no words beside it, on a navigation answer type | structural (this file) | the answer types of `logos-core/src/models/navigation.rs` |
//! | is the denominator's **own** wording (`unindexed`, `no-language-recorded`, `n/a`) | lexical census | the `models/navigation.rs` rows of its `CENSUS`, and its `the_resolution_denominator_speaks_only_the_lexicon` |
//!
//! The two meet at `resolution_denominator`: this arm proves the field is
//! **there** on every relational answer; the census proves what it **says**
//! is in the closed lexicon. Neither arm can do the other's half — a shape check
//! is blind to a new spelling, and a spelling check is blind to an empty list.
//!
//! **The next surface's author.** A new *navigation answer* — a new `pub`
//! type in `models/navigation.rs` that an `Engine` method returns — is enrolled
//! here by construction and fails until it either carries
//! `resolution_denominator: ResolutionDenominator` or is entered in
//! [`OUTSIDE_THE_CLASS`] with the reason. A new *rendering* of an absence, on any
//! surface, is the census's.
//!
//! # Enumerated from the definitions, and why the one hand list is safe
//!
//! The answer types are not listed here. They are read out of the source by
//! parsing two files with the Rust grammar the product already links:
//!
//! 1. **`models/navigation.rs`** — every top-level `pub` struct or enum **no
//!    other type in that file holds as a field** is a *root* of the file's
//!    containment graph. `ImpactEntry` is held by `ImpactResult`, so it is a
//!    part; nothing holds `ImpactResult`, so it is a root.
//! 2. **`engine.rs`** — a root is an *answer* when a `pub fn` of `impl Engine`
//!    returns it, because every surface reaches the navigation service through
//!    that façade ([ADR-01]). A root no method returns is answered to no one;
//!    it is counted and **named** in the denominator line rather than checked
//!    (today one: `WorkItem`, the parsed input of `impact_intersection`).
//!
//! [`the_enumeration_agrees_with_the_engine`] then holds the two derivations
//! to each other: every `Engine` method that delegates to `crate::navigate`
//! must return a root. That is what stops a relational answer escaping by
//! being *nested* — fold `CallersResult` into another type and it stops being a
//! root, but `Engine::callers` still returns it — and it is what makes the
//! count non-vacuous without a floor: a parse that read nothing leaves every
//! delegating method returning no root, and fails.
//!
//! The one hand-maintained list is [`OUTSIDE_THE_CLASS`], and it lists what is
//! **exempt**, not what is checked. [S-434]'s correction was to a hand-listed
//! *inclusion* boundary — a narrower root let live renderings hide, because a
//! site nobody listed was a site nobody read. An exemption list fails the other
//! way: a type nobody listed is **required** to carry the denominator, so the
//! list cannot hide a type, only excuse one by name and with a reason. Each
//! excuse is itself checked by [`the_exemptions_are_live`]: it must name an
//! answer type that exists and that lacks the field, so an exemption cannot
//! outlive its subject or survive the type growing the field.
//!
//! # The result, with its denominator and its date
//!
//! **2026-09-23 — 17 roots enumerated from `models/navigation.rs`, 1 returned
//! by no `Engine` method; of the 16 answer types, 7 carry the denominator, 9
//! are outside the class with a stated reason, 0 violate.** The 7 are the ones [S-442] shipped the field on — `callers`,
//! `callees`, `impact`, `impact_intersection`, `precedent`, `branch_overlap`,
//! `affected` — found by the parse, not named to it.
//! [`the_arm_reports_its_count_with_its_denominator`] pins the figures and the
//! arithmetic between them; like the census, it is a dated record, not a floor.
//!
//! # What this cannot catch
//!
//! - **A relational answer type defined outside `models/navigation.rs` and
//!   returned by an `Engine` method that does not delegate to
//!   `crate::navigate`.** The universe is the navigation-service's read-model
//!   module, as its own header declares; the agreement check widens it to
//!   whatever `crate::navigate` returns, and no further. The `Engine` also
//!   returns quality, wiki, history and config read-models, none of which
//!   traverses the resolved edge set.
//! - **A root no `Engine` method returns yet.** It is named in the denominator
//!   line and the dated record below moves, but its shape is not checked until
//!   a method returns it — at which point it is.
//! - **A denominator that is present but wrong.** That is behaviour, owned by
//!   `relational_denominator.rs` and the lexical census.
//! - **Type identity by name.** The parse compares type names, not resolved
//!   paths; a same-named type from another module would read as a navigation
//!   type. Path-qualified names outside `navigation` are excluded.
//!
//! It adds no runtime surface and no dispatch arm: it is a test that reads two
//! source files ([NFR-MA-02]).
//!
//! Gated on `lang-rust`, as `observability.rs` is: the parse needs the Rust
//! grammar, which the default build links.
//!
//! [`absence::SENTINELS`]: logos_core::models::quality::absence::SENTINELS
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
//! [S-434]: ../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
//! [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
//! [S-443]: ../../docs/planning/journal.md#s-443-the-absence-audit-gains-a-structural-arm-over-the-relational-result-types
//! [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
//! [FR-NV-14]: ../../docs/specs/requirements/FR-NV-14.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-MA-02]: ../../docs/specs/requirements/NFR-MA-02.md
#![cfg(feature = "lang-rust")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tree_sitter::{Node, Parser, Tree};

/// The date the figures in this module's header were taken.
const AUDITED_ON: &str = "2026-09-23";

/// The field every relational answer carries, and its type — named uniformly
/// across the answer types by [S-442] so that this arm can key on it.
///
/// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
const DENOMINATOR_FIELD: &str = "resolution_denominator";
const DENOMINATOR_TYPE: &str = "ResolutionDenominator";

/// The two source files the enumeration reads, relative to `logos-core`.
const NAVIGATION_MODELS: &str = "src/models/navigation.rs";
const ENGINE: &str = "src/engine.rs";

/// Answer types that are **not** relational answers, each with the reason.
///
/// This is an exemption list, never an inclusion list: an answer type absent
/// from it must carry the denominator (see the module header). Two entries
/// carry edges and are recorded as **not adjudicated** rather than argued out
/// of the class — [CR-143] enumerated its relational queries and named neither,
/// and deciding them is a scope question for a change request, not for the
/// check that enforces the scope.
///
/// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
const OUTSIDE_THE_CLASS: [(&str, &str); 9] = [
    (
        "SearchResult",
        "a ranked bundle, not a relational verdict; CR-143 §3.6 excludes it because no \
         consumer draws a clearance from its emptiness",
    ),
    (
        "ContextBundle",
        "a ranked bundle, not a relational verdict; CR-143 §3.6 excludes it because no \
         consumer draws a clearance from its emptiness",
    ),
    (
        "ExploreResult",
        "a ranked bundle, not a relational verdict; CR-143 §3.6 excludes it because no \
         consumer draws a clearance from its emptiness",
    ),
    (
        "ImplementorsResult",
        "computed over doc→code `doc_reference`/`traces_to` edges, which the per-language \
         code-resolution rows a denominator carries do not measure",
    ),
    (
        "ReferencingDocsResult",
        "computed over doc→code `doc_reference`/`traces_to` edges, which the per-language \
         code-resolution rows a denominator carries do not measure",
    ),
    (
        "StatusInfo",
        "the readout the denominator's rows are read from (`resolution_by_language`); it \
         states resolution rather than being computed over it",
    ),
    (
        "LanguageComposition",
        "node and file counts per language; it traverses no edge",
    ),
    (
        "NodeInfo",
        "NOT ADJUDICATED: carries a symbol's immediate edges, but CR-143 §1 does not name \
         `node` among the relational queries — recorded by S-443 for a scope decision",
    ),
    (
        "GraphElements",
        "NOT ADJUDICATED: the SPA canvas's bounded nodes+edges drawing, carrying its own \
         elision denominator; CR-143 §1 does not name it — recorded by S-443 for a scope \
         decision",
    ),
];

// ── The parse ──────────────────────────────────────────────────────────────

fn parse(source: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("the Rust grammar loads");
    parser.parse(source, None).expect("tree-sitter returns a tree")
}

fn text<'s>(node: Node<'_>, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

fn is_pub(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let is_pub = node
        .children(&mut cursor)
        .any(|child| child.kind() == "visibility_modifier");
    is_pub
}

/// Every `type_identifier` under `node`, as text.
fn type_names(node: Node<'_>, source: &str, into: &mut BTreeSet<String>) {
    if node.kind() == "type_identifier" {
        into.insert(text(node, source).to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        type_names(child, source, into);
    }
}

/// One field of a navigation type, as the check needs to see it.
#[derive(Debug, Clone)]
struct Field {
    name: String,
    ty: String,
    /// The attribute items written directly above the field.
    attributes: Vec<String>,
}

/// One top-level `pub` struct or enum of `models/navigation.rs`.
#[derive(Debug, Clone)]
struct NavType {
    fields: Vec<Field>,
    /// The other types this one holds, by name, through any field or variant.
    holds: BTreeSet<String>,
}

fn fields_of(list: Node<'_>, source: &str) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut attributes = Vec::new();
    let mut cursor = list.walk();
    for child in list.named_children(&mut cursor) {
        match child.kind() {
            "attribute_item" => attributes.push(text(child, source).to_string()),
            "field_declaration" => {
                let name = child
                    .child_by_field_name("name")
                    .map_or_else(String::new, |n| text(n, source).to_string());
                let ty = child
                    .child_by_field_name("type")
                    .map_or_else(String::new, |n| text(n, source).to_string());
                fields.push(Field {
                    name,
                    ty,
                    attributes: std::mem::take(&mut attributes),
                });
            }
            // Doc comments sit between an attribute and its field; they belong
            // to the field and must not detach the attribute from it.
            "line_comment" | "block_comment" => {}
            _ => attributes.clear(),
        }
    }
    fields
}

/// Every top-level `pub` struct and enum of a navigation-models source.
fn navigation_types(source: &str) -> BTreeMap<String, NavType> {
    let tree = parse(source);
    let root = tree.root_node();
    let mut types = BTreeMap::new();
    let mut cursor = root.walk();
    for item in root.named_children(&mut cursor) {
        if !matches!(item.kind(), "struct_item" | "enum_item") || !is_pub(item) {
            continue;
        }
        let Some(name) = item.child_by_field_name("name") else {
            continue;
        };
        let name = text(name, source).to_string();
        let mut holds = BTreeSet::new();
        let mut fields = Vec::new();
        if let Some(body) = item.child_by_field_name("body") {
            // Only field and variant *types* hold a type: attributes carry
            // identifiers, not type identifiers, and doc text is a comment.
            if body.kind() == "field_declaration_list" {
                fields = fields_of(body, source);
            }
            type_names(body, source, &mut holds);
        }
        holds.remove(&name);
        types.insert(name, NavType { fields, holds });
    }
    types
}

/// One `pub fn` of `impl Engine`: what it returns, and whether it delegates to
/// the navigation service.
#[derive(Debug)]
struct EngineMethod {
    name: String,
    returns: BTreeSet<String>,
    delegates_to_navigate: bool,
}

/// Whether a body calls into the navigation service by any path spelling —
/// `crate::navigate::…`, `navigate::…` under a `use crate::navigate`, or
/// `self::…::navigate::…` — rather than one literal prefix.
fn mentions_navigate(node: Node<'_>, source: &str) -> bool {
    if node.kind() == "scoped_identifier"
        && text(node, source).split("::").any(|segment| segment.trim() == "navigate")
    {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|child| mentions_navigate(child, source));
    found
}

/// Bare type names in a return type, dropping any path-qualified name whose
/// path is not the navigation module — or `models`, which re-exports it
/// (`pub use navigation::*`), so `crate::models::CallersResult` is the same
/// type as `crate::models::navigation::CallersResult`.
fn returned_names(node: Node<'_>, source: &str, into: &mut BTreeSet<String>) {
    if node.kind() == "scoped_type_identifier" {
        let from_navigation = node.child_by_field_name("path").is_some_and(|p| {
            matches!(
                text(p, source).rsplit("::").next().map(str::trim),
                Some("navigation" | "models")
            )
        });
        if !from_navigation {
            // Still walk the generic arguments, never the qualified name.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "type_arguments" {
                    returned_names(child, source, into);
                }
            }
            return;
        }
        if let Some(name) = node.child_by_field_name("name") {
            into.insert(text(name, source).to_string());
        }
        return;
    }
    if node.kind() == "type_identifier" {
        into.insert(text(node, source).to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        returned_names(child, source, into);
    }
}

fn engine_methods(source: &str) -> Vec<EngineMethod> {
    let tree = parse(source);
    let root = tree.root_node();
    let mut methods = Vec::new();
    let mut cursor = root.walk();
    for item in root.named_children(&mut cursor) {
        let inherent_engine_impl = item.kind() == "impl_item"
            && item.child_by_field_name("trait").is_none()
            && item
                .child_by_field_name("type")
                .is_some_and(|t| text(t, source) == "Engine");
        if !inherent_engine_impl {
            continue;
        }
        let Some(body) = item.child_by_field_name("body") else {
            continue;
        };
        let mut body_cursor = body.walk();
        for function in body.named_children(&mut body_cursor) {
            if function.kind() != "function_item" || !is_pub(function) {
                continue;
            }
            let name = function
                .child_by_field_name("name")
                .map_or_else(String::new, |n| text(n, source).to_string());
            let mut returns = BTreeSet::new();
            if let Some(ret) = function.child_by_field_name("return_type") {
                returned_names(ret, source, &mut returns);
            }
            let delegates_to_navigate = function
                .child_by_field_name("body")
                .is_some_and(|b| mentions_navigate(b, source));
            methods.push(EngineMethod {
                name,
                returns,
                delegates_to_navigate,
            });
        }
    }
    methods
}

// ── The audit ──────────────────────────────────────────────────────────────

/// What the arm found over one pair of sources.
#[derive(Debug, Default)]
struct Audit {
    /// Every navigation type no other navigation type holds.
    roots: BTreeSet<String>,
    /// The roots no `Engine` method returns — reachable from no surface, so
    /// not an answer (today `WorkItem`, the parsed input of
    /// `impact_intersection`). Counted and named, never silently dropped.
    unreturned: BTreeSet<String>,
    /// Every answer type: a root an `Engine` method returns.
    answers: BTreeSet<String>,
    /// The answers that carry the denominator as required.
    carrying: BTreeSet<String>,
    /// Answers outside [`OUTSIDE_THE_CLASS`] that do not carry it, with why.
    missing: Vec<String>,
    /// Exemptions that name no answer, or an answer that carries the field.
    stale_exemptions: Vec<String>,
    /// Disagreements between the containment roots and the `Engine`'s returns.
    disagreements: Vec<String>,
}

fn denominator_problem(ty: &NavType) -> Option<String> {
    let Some(field) = ty.fields.iter().find(|f| f.name == DENOMINATOR_FIELD) else {
        return Some(format!("has no `{DENOMINATOR_FIELD}` field"));
    };
    // Compared by its last path segment, so a fully qualified spelling of the
    // same type is the same type; `Option<…>` and every other wrapper is not.
    if field.ty.rsplit("::").next() != Some(DENOMINATOR_TYPE) {
        return Some(format!(
            "declares `{DENOMINATOR_FIELD}: {}`, not `{DENOMINATOR_TYPE}` — the answer must \
             always state it, so an optional or re-typed field is the silent absence again",
            field.ty
        ));
    }
    // Any attribute but a doc attribute can take the field off the default
    // wire: `cfg` compiles it out of a build, `serde(skip…)` omits it,
    // `serde(flatten)` dissolves its key into the parent, `serde(rename…)`
    // moves it. The field must serialise as written, under its own name, in
    // every build — so none may qualify it.
    if let Some(attribute) = field
        .attributes
        .iter()
        .find(|a| !a.trim_start_matches("#[").trim_start().starts_with("doc"))
    {
        return Some(format!(
            "carries `{DENOMINATOR_FIELD}` qualified by `{attribute}`, which can keep it off \
             the wire under its own name"
        ));
    }
    None
}

fn audit(navigation_source: &str, engine_source: &str) -> Audit {
    let types = navigation_types(navigation_source);
    let names: BTreeSet<String> = types.keys().cloned().collect();
    let held: BTreeSet<String> = types
        .iter()
        .flat_map(|(name, ty)| ty.holds.iter().filter(move |h| *h != name))
        .filter(|h| names.contains(*h))
        .cloned()
        .collect();

    let methods = engine_methods(engine_source);
    let returned: BTreeSet<&String> = methods
        .iter()
        .flat_map(|m| m.returns.iter())
        .filter(|r| names.contains(*r))
        .collect();

    let roots: BTreeSet<String> = names.difference(&held).cloned().collect();
    // Every surface reaches the navigation service through the `Engine`
    // façade (ADR-01), so a root it never returns is answered to no one.
    let (answers, unreturned): (BTreeSet<String>, BTreeSet<String>) =
        roots.iter().cloned().partition(|r| returned.contains(r));
    let mut report = Audit {
        roots,
        unreturned,
        answers,
        ..Audit::default()
    };
    let exempt: BTreeMap<&str, &str> = OUTSIDE_THE_CLASS.iter().copied().collect();

    for answer in &report.answers {
        let problem = denominator_problem(&types[answer]);
        match (exempt.contains_key(answer.as_str()), problem) {
            (false, None) => {
                report.carrying.insert(answer.clone());
            }
            (false, Some(problem)) => report.missing.push(format!("{answer} {problem}")),
            (true, None) => report.stale_exemptions.push(format!(
                "{answer} carries `{DENOMINATOR_FIELD}` yet is exempted — drop the exemption"
            )),
            (true, Some(_)) => {}
        }
    }
    for (name, why) in OUTSIDE_THE_CLASS {
        if why.trim().is_empty() {
            report
                .stale_exemptions
                .push(format!("{name} is exempted without a reason"));
        }
        if !report.answers.contains(name) {
            report.stale_exemptions.push(format!(
                "{name} is exempted but is not an answer type of {NAVIGATION_MODELS}"
            ));
        }
    }

    for method in methods.iter().filter(|m| m.delegates_to_navigate) {
        let navigation_returns: Vec<&String> =
            method.returns.iter().filter(|r| names.contains(*r)).collect();
        if navigation_returns.is_empty() {
            report.disagreements.push(format!(
                "Engine::{} delegates to crate::navigate but returns no type of {NAVIGATION_MODELS}",
                method.name
            ));
        }
        for returned in navigation_returns {
            if !report.answers.contains(returned) {
                report.disagreements.push(format!(
                    "Engine::{} returns {returned}, which another navigation type holds — a \
                     nested answer this enumeration would not see",
                    method.name
                ));
            }
        }
    }
    report
}

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn live() -> Audit {
    audit(&read(NAVIGATION_MODELS), &read(ENGINE))
}

/// The denominator every failure message states, so "every type was checked"
/// is read off the message rather than assumed.
fn denominator_line(report: &Audit) -> String {
    format!(
        "{} root type(s) enumerated from {NAVIGATION_MODELS}, {} returned by no `Engine` \
         method ({}); of the {} answer type(s), {} carry `{DENOMINATOR_FIELD}`, {} are \
         outside the class, {} are missing it",
        report.roots.len(),
        report.unreturned.len(),
        report.unreturned.iter().cloned().collect::<Vec<_>>().join(", "),
        report.answers.len(),
        report.carrying.len(),
        OUTSIDE_THE_CLASS.len(),
        report.missing.len()
    )
}

// ── The live tree ──────────────────────────────────────────────────────────

/// **Every relational answer type carries the resolution denominator** — the
/// check this file exists for.
#[test]
fn every_relational_answer_type_carries_the_resolution_denominator() {
    let report = live();
    println!("{}", denominator_line(&report));
    println!("carrying: {:?}", report.carrying);
    assert!(
        report.missing.is_empty(),
        "{}\n\nan answer type of {NAVIGATION_MODELS} must carry `{DENOMINATOR_FIELD}: \
         {DENOMINATOR_TYPE}` (FR-NV-14), or be entered in OUTSIDE_THE_CLASS with the reason \
         it is not a relational answer:\n  {}",
        denominator_line(&report),
        report.missing.join("\n  ")
    );
}

/// **Each exemption names a live answer type that lacks the field** — so the
/// one hand list cannot outlive its subject, or excuse a type that no longer
/// needs excusing.
#[test]
fn the_exemptions_are_live() {
    let report = live();
    assert!(
        report.stale_exemptions.is_empty(),
        "{}:\n  {}",
        denominator_line(&report),
        report.stale_exemptions.join("\n  ")
    );
    let unique: BTreeSet<&str> = OUTSIDE_THE_CLASS.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        unique.len(),
        OUTSIDE_THE_CLASS.len(),
        "an answer type is exempted twice"
    );
}

/// **The containment roots and the `Engine`'s signatures name the same
/// answers** — the two derivations are independent, so their agreement is
/// what makes the enumeration non-vacuous and closes the nested-answer hole.
#[test]
fn the_enumeration_agrees_with_the_engine() {
    let report = live();
    assert!(
        report.disagreements.is_empty(),
        "{}:\n  {}",
        denominator_line(&report),
        report.disagreements.join("\n  ")
    );
}

/// **The arm states its count with its denominator** — a dated record of the
/// reading in the module header, not a floor.
#[test]
fn the_arm_reports_its_count_with_its_denominator() {
    let report = live();
    assert_eq!(
        (
            report.roots.len(),
            report.unreturned.len(),
            report.answers.len(),
            report.carrying.len(),
            OUTSIDE_THE_CLASS.len(),
            report.missing.len(),
            AUDITED_ON,
        ),
        (17, 1, 16, 7, 9, 0, "2026-09-23"),
        "{}. Change the module header and this tuple together — a count without its \
         denominator says nothing, and an undated one reads as a standing property",
        denominator_line(&report)
    );
    assert_eq!(
        report.roots.len(),
        report.unreturned.len() + report.answers.len(),
        "every root is exactly one of unreturned or an answer"
    );
    assert_eq!(
        report.answers.len(),
        report.carrying.len() + OUTSIDE_THE_CLASS.len() + report.missing.len(),
        "every answer type is exactly one of carrying, exempt or missing"
    );
}

// ── What the check admits: mutations, kept in the suite ────────────────────
//
// A green check over an unmutated tree proves nothing about what it admits.
// Each case below mutates the LIVE sources in memory — the parse is all the
// check reads, so a mutated string is a mutated tree — and asserts the check
// names the defect. They run on every suite, so the demonstration cannot rot.

/// The live navigation source with `declaration` appended as a new top-level
/// item, and the live engine source with `method` inserted into `impl Engine`.
fn mutated(declaration: &str, method: &str) -> Audit {
    let navigation = format!("{}\n{declaration}\n", read(NAVIGATION_MODELS));
    let engine = read(ENGINE);
    let anchor = "impl Engine {\n";
    let at = engine.find(anchor).expect("engine.rs declares `impl Engine {`") + anchor.len();
    let engine = format!("{}{method}\n{}", &engine[..at], &engine[at..]);
    audit(&navigation, &engine)
}

const NEW_METHOD: &str = "    pub fn dependents(&self, symbol: &str) -> DependentsResult {\n        \
     crate::navigate::dependents(self, symbol).unwrap_or_default()\n    }";

/// **The mutation the AC names: a new relational answer type without a
/// denominator fails.** Wired end to end — a model type and the `Engine`
/// method returning it — so only the missing field is wrong.
#[test]
fn a_new_relational_answer_without_the_denominator_fails() {
    let report = mutated(
        "#[derive(Debug, Default, Serialize)]\npub struct DependentsResult {\n    \
         pub query: String,\n    pub dependents: Vec<SymbolRef>,\n    \
         pub warnings: Vec<String>,\n}",
        NEW_METHOD,
    );
    assert_eq!(
        report.missing,
        vec![format!("DependentsResult has no `{DENOMINATOR_FIELD}` field")],
        "the new answer type is named as missing the denominator"
    );
    assert!(report.disagreements.is_empty(), "{:?}", report.disagreements);
    assert_eq!(report.answers.len(), 17, "the new type is enumerated");
}

/// **The positive control: the same type with the field passes** — so the
/// case above fails on the field, not on the type being new.
#[test]
fn a_new_relational_answer_with_the_denominator_passes() {
    let report = mutated(
        "#[derive(Debug, Default, Serialize)]\npub struct DependentsResult {\n    \
         pub query: String,\n    pub dependents: Vec<SymbolRef>,\n    \
         /// The resolved edge set.\n    \
         pub resolution_denominator: ResolutionDenominator,\n    \
         pub warnings: Vec<String>,\n}",
        NEW_METHOD,
    );
    assert!(report.missing.is_empty(), "{:?}", report.missing);
    assert!(report.carrying.contains("DependentsResult"));
    // The same type, fully qualified, and a doc attribute are not near misses.
    let report = mutated(
        "#[derive(Debug, Default, Serialize)]\npub struct DependentsResult {\n    \
         pub query: String,\n    #[doc = \"The resolved edge set.\"]\n    \
         pub resolution_denominator: crate::models::navigation::ResolutionDenominator,\n}",
        NEW_METHOD,
    );
    assert!(report.missing.is_empty(), "{:?}", report.missing);
    assert!(report.disagreements.is_empty(), "{:?}", report.disagreements);
}

/// **The near misses: a field that is almost the denominator is not it.**
/// One character off the name, optional, re-typed, or qualified by an
/// attribute that can keep it off the wire (`skip`, `flatten`, `rename`,
/// `cfg`) — each is the silent absence again, and each is named.
#[test]
fn a_near_miss_denominator_field_is_not_the_denominator() {
    let cases = [
        (
            "pub resolution_denominators: ResolutionDenominator,",
            "has no `resolution_denominator` field",
        ),
        (
            "pub resolution_denominator: Option<ResolutionDenominator>,",
            "declares `resolution_denominator: Option<ResolutionDenominator>`",
        ),
        (
            "pub resolution_denominator: Vec<LanguageResolution>,",
            "declares `resolution_denominator: Vec<LanguageResolution>`",
        ),
        (
            "#[serde(skip)]\n    /// Hidden.\n    pub resolution_denominator: ResolutionDenominator,",
            "qualified by `#[serde(skip)]`",
        ),
        (
            "#[serde(skip_serializing_if = \"is_unread\")]\n    \
             pub resolution_denominator: ResolutionDenominator,",
            "qualified by `#[serde(skip_serializing_if = \"is_unread\")]`",
        ),
        (
            "#[serde(flatten)]\n    pub resolution_denominator: ResolutionDenominator,",
            "qualified by `#[serde(flatten)]`",
        ),
        (
            "#[serde(rename = \"coverage\")]\n    pub resolution_denominator: ResolutionDenominator,",
            "qualified by `#[serde(rename = \"coverage\")]`",
        ),
        (
            "#[cfg(feature = \"agents\")]\n    pub resolution_denominator: ResolutionDenominator,",
            "qualified by `#[cfg(feature = \"agents\")]`",
        ),
        (
            "#[cfg_attr(test, serde(skip))]\n    pub resolution_denominator: ResolutionDenominator,",
            "qualified by `#[cfg_attr(test, serde(skip))]`",
        ),
    ];
    for (field, expected) in cases {
        let report = mutated(
            &format!(
                "#[derive(Debug, Default, Serialize)]\npub struct DependentsResult {{\n    \
                 pub query: String,\n    {field}\n    pub warnings: Vec<String>,\n}}"
            ),
            NEW_METHOD,
        );
        assert_eq!(report.missing.len(), 1, "{field}: {:?}", report.missing);
        assert!(
            report.missing[0].starts_with("DependentsResult ") && report.missing[0].contains(expected),
            "{field}: expected `{expected}`, got {:?}",
            report.missing
        );
    }
}

/// **An answer returned through the `models` re-export is enrolled.**
/// `models/mod.rs` re-exports the navigation module, so a method spelling its
/// return `crate::models::X` returns the same type as one spelling it
/// `crate::models::navigation::X`, and must be checked the same way.
#[test]
fn an_answer_returned_through_the_models_re_export_is_enrolled() {
    for path in ["crate::models", "crate::models::navigation"] {
        let report = mutated(
            "#[derive(Debug, Default, Serialize)]\npub struct DependentsResult {\n    \
             pub query: String,\n}",
            &format!(
                "    pub fn dependents(&self, symbol: &str) -> {path}::DependentsResult {{\n        \
                 {path}::DependentsResult {{ query: symbol.to_string() }}\n    }}"
            ),
        );
        assert_eq!(
            report.missing,
            vec![format!("DependentsResult has no `{DENOMINATOR_FIELD}` field")],
            "{path}::DependentsResult"
        );
    }
}

/// **An answer cannot escape by being nested.** Folding `CallersResult` into
/// another navigation type removes it from the containment roots, but
/// `Engine::callers` still returns it — the agreement check names it, however
/// the delegation is spelled.
#[test]
fn a_nested_relational_answer_is_caught_by_the_engine_agreement() {
    let navigation = read(NAVIGATION_MODELS).replacen(
        "pub struct LanguageComposition {\n",
        "pub struct LanguageComposition {\n    pub callers: CallersResult,\n",
        1,
    );
    assert_ne!(navigation, read(NAVIGATION_MODELS), "the nesting mutation applied");
    let report = audit(&navigation, &read(ENGINE));
    assert!(!report.answers.contains("CallersResult"), "nesting hid it from the roots");
    assert!(
        report
            .disagreements
            .iter()
            .any(|d| d.starts_with("Engine::callers returns CallersResult")),
        "{:?}",
        report.disagreements
    );
    // The same fold, with `Engine::callers` delegating through a `use`d
    // `navigate` rather than the `crate::navigate::` prefix.
    let engine = read(ENGINE).replacen(
        "crate::navigate::callers(self, symbol, limit)",
        "navigate::callers(self, symbol, limit)",
        1,
    );
    assert_ne!(engine, read(ENGINE), "the delegation respelling applied");
    let report = audit(&navigation, &engine);
    assert!(
        report
            .disagreements
            .iter()
            .any(|d| d.starts_with("Engine::callers returns CallersResult")),
        "{:?}",
        report.disagreements
    );
}

/// **A root no `Engine` method returns is counted and named, not checked** —
/// no surface can answer with it, and the moment one does it is enrolled (the
/// case above). The denominator line names it, so it cannot pass unseen.
#[test]
fn a_root_no_engine_method_returns_is_named_in_the_denominator() {
    let report = mutated(
        "#[derive(Debug, Default, Serialize)]\npub struct OrphanResult {\n    \
         pub query: String,\n}",
        "",
    );
    assert!(report.unreturned.contains("OrphanResult"), "{:?}", report.unreturned);
    assert!(!report.answers.contains("OrphanResult"));
    assert!(report.missing.is_empty(), "{:?}", report.missing);
    assert!(denominator_line(&report).contains("OrphanResult"));
}

/// **An exemption stops excusing a type the moment it grows the field.**
#[test]
fn an_exemption_over_a_type_that_carries_the_field_is_stale() {
    let navigation = read(NAVIGATION_MODELS).replacen(
        "pub struct ImplementorsResult {\n",
        "pub struct ImplementorsResult {\n    \
         pub resolution_denominator: ResolutionDenominator,\n",
        1,
    );
    assert_ne!(navigation, read(NAVIGATION_MODELS), "the mutation applied");
    let report = audit(&navigation, &read(ENGINE));
    assert_eq!(
        report.stale_exemptions,
        vec![format!(
            "ImplementorsResult carries `{DENOMINATOR_FIELD}` yet is exempted — drop the exemption"
        )]
    );
}

/// **Removing the field from a live relational answer fails** — the
/// regression direction, on a type S-442 shipped.
#[test]
fn removing_the_denominator_from_a_shipped_answer_fails() {
    let live_source = read(NAVIGATION_MODELS);
    let start = live_source
        .find("pub struct AffectedResult {")
        .expect("AffectedResult is declared");
    let field = "    pub resolution_denominator: ResolutionDenominator,\n";
    let at = start + live_source[start..].find(field).expect("AffectedResult carries the field");
    let navigation = format!("{}{}", &live_source[..at], &live_source[at + field.len()..]);
    let report = audit(&navigation, &read(ENGINE));
    assert_eq!(
        report.missing,
        vec![format!("AffectedResult has no `{DENOMINATOR_FIELD}` field")]
    );
}
