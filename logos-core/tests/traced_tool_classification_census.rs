//! **Every registered tool is classified or explicitly excluded, with a
//! reason** ([S-446], [FR-OB-04], [NFR-MA-02], [CR-144]).
//!
//! [S-445] gave four tools — `callers`/`impact`/`precedent`/`affected` — a
//! `CallOutcome` impl and wired each through `traced_with`, so its telemetry
//! event records what the call **answered** rather than `NULL`. Every other
//! registered tool still records `NULL`, and nothing stopped a later tool from
//! joining that silent majority forever: the enum and the classification live
//! in the same file, but nothing enumerated one against the other. This is
//! that enumeration.
//!
//! # Enumerated from the definitions, never from a duplicate list
//!
//! Both populations are read out of the actual source with the Rust grammar
//! the product already links (`tree-sitter-rust`), the same technique
//! `relational_denominator_audit.rs` ([S-443]) uses for its own structural
//! arm — never a hand-copied name list, which is exactly the boundary failure
//! [S-434] had to correct once already (a duplicate enumeration that had
//! drifted from its source).
//!
//! 1. **The registered tools** — every `Variant => "wire_name"` entry inside
//!    `tool.rs`'s `registered_tools! { … }` invocation ([`registered_tools`]).
//!    This is the exact list [`Tool::ALL`](../src/observability/tool.rs) is
//!    generated from, so a new variant is enumerated the moment it is added —
//!    there is no second place to remember to update.
//! 2. **The classified tools** — every `Tool::Variant` passed as the first
//!    argument to a `traced_with(…)` call in `engine.rs`
//!    ([`classified_variants`]), the single chokepoint file every `Engine`
//!    method funnels telemetry through ([ADR-13], `observability/mod.rs`'s own
//!    module doc). A tool is classified if and only if its chokepoint calls
//!    `traced_with` rather than `traced` — this walk is exactly that
//!    definition, not an approximation of it.
//!
//! Every registered tool not found classified must appear in [`EXCLUSIONS`]
//! with a stated reason ([`every_registered_tool_is_classified_or_excluded`]).
//! Adding a tool that is neither is exactly the failure this file exists to
//! catch, and it fails loudly rather than recording another silent `NULL`.
//!
//! # Why `engine.rs` alone, and what widening it would cost
//!
//! Today `traced_with` is called from exactly one file
//! (`grep -rn 'traced_with(' --include='*.rs'` finds it only in `engine.rs`'s
//! four chokepoints and in `observability/tests.rs`'s unit tests exercising
//! them). Scanning only `engine.rs` keeps this walk simple and matches the
//! architecture's own single-chokepoint invariant. If a future story ever adds
//! a `traced_with` call in another production file, this walk will not see it:
//! the tool then reads as neither classified nor excluded (since it is new) and
//! [`every_registered_tool_is_classified_or_excluded`] fails — loudly, the one
//! direction a census may be wrong in — and the fix is to widen
//! [`classified_variants`] to that file, not to add a false [`EXCLUSIONS`]
//! entry.
//!
//! # What a mutation looks like, and how it was demonstrated
//!
//! A tool that is registered but neither classified nor excluded is exactly
//! what [`every_registered_tool_is_classified_or_excluded`] exists to catch.
//! This was demonstrated by hand rather than left asserted: a fifth
//! `registered_tools!` entry (`MutationCensusProbe => "mutation_census_probe"`)
//! was added to `tool.rs`, together with the two minimal match-arm additions
//! `event_class`/`tool_class` require to keep the crate compiling (their own
//! exhaustiveness checks, not this file's), with **no** `traced_with` call site
//! and **no** [`EXCLUSIONS`] entry. `cargo test -p logos-core --test
//! traced_tool_classification_census` failed on
//! [`every_registered_tool_is_classified_or_excluded`]:
//!
//! ```text
//! these registered tools are enumerated in tool.rs but are neither classified
//! (a `traced_with` call in engine.rs) nor excluded with a reason in
//! EXCLUSIONS: ["MutationCensusProbe"] — classify it or add an EXCLUSIONS entry
//! ```
//!
//! All three edits were then reverted (`git diff` against the committed tree
//! showed no residue) and `tool.rs` was `touch`ed so cargo would not serve a
//! stale mutant on the next build ([S-445]'s own falsifiability sweep records
//! the same discipline). The suite was re-run green afterward. This is the
//! *admission* direction — the mutation **adds** an unclassified tool — which
//! is deliberately the one this file's own sprint records a self-run mutation
//! sweep can miss when it only ever deletes (Sprint 78 Lessons).
//!
//! # The denominator
//!
//! **2026-09-26 — 75 registered tools enumerated, 4 classified, 71 excluded
//! with a reason**, pinned by
//! [`the_census_reports_its_denominator`], which prints the three counts.
//!
//! `CallOutcome` is `pub(crate)` in `observability::tool`, so this integration
//! test cannot name it as a type or link to it — it reads the source instead,
//! which is the whole point.
//!
//! [S-434]: ../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
//! [S-443]: ../../docs/planning/journal.md#s-443-the-absence-audit-gains-a-structural-arm-over-the-relational-result-types
//! [S-445]: ../../docs/planning/journal.md#s-445-a-telemetry-event-records-what-the-call-answered-with-its-denominator
//! [S-446]: ../../docs/planning/journal.md#s-446-every-traced-tool-is-classified-or-explicitly-excluded
//! [CR-144]: ../../docs/requests/CR-144-telemetry-records-the-answer-not-only-the-call.md
//! [FR-OB-04]: ../../docs/specs/requirements/FR-OB-04.md
//! [NFR-MA-02]: ../../docs/specs/requirements/NFR-MA-02.md
//! [ADR-13]: ../../docs/specs/architecture/decisions/ADR-13.md
#![cfg(feature = "lang-rust")]

use std::collections::HashSet;
use std::path::Path;

use tree_sitter::{Node, Parser, Tree};

/// The date the figures in this module's header were taken.
const AUDITED_ON: &str = "2026-09-26";

/// The two source files the enumeration and the classification are each read
/// from, relative to `logos-core` (`env!("CARGO_MANIFEST_DIR")`).
const TOOL_RS: &str = "src/observability/tool.rs";
const ENGINE_RS: &str = "src/engine.rs";

/// Every registered tool **not** found classified, with the reason it has no
/// `CallOutcome` vocabulary yet.
///
/// An exemption list, never an inclusion list, following
/// `relational_denominator_audit.rs`'s `OUTSIDE_THE_CLASS` precedent: a tool
/// absent from here must be classified, and
/// [`every_registered_tool_is_classified_or_excluded`] enforces exactly that.
///
/// Grouped by the same five-way breakdown `Tool::tool_class` already declares
/// in `tool.rs` (Navigation / QualityGate / SessionGate / EngineInternal /
/// ReadModel) — reusing an axis the product already maintains rather than
/// inventing a sixth one for this table alone. `tool_class` moving a tool
/// between groups has no bearing on this file's own correctness: the grouping
/// is documentation for the reason, not a fact this file checks.
const REASON_NAVIGATION_UNCLASSIFIED: &str =
    "Navigation (Tool::tool_class() == Navigation): a query of the same shape as the four \
     classified tools, but not yet given a CallOutcome impl. Classifying it needs the same \
     design rigor S-445 gave callers/impact/precedent/affected (FR-OB-14 Notes, 'Scope of \
     classification'); deferred to a future story rather than guessed here.";
const REASON_QUALITY_GATE: &str =
    "Quality-gate (Tool::tool_class() == QualityGate): reports a verdict about the repository, \
     but has not been given an Answered/Empty/Unresolved split. Classifying it needs the same \
     design rigor S-445 gave callers/impact/precedent/affected (FR-OB-14 Notes, 'Scope of \
     classification'); deferred rather than guessed here.";
const REASON_SESSION_GATE: &str =
    "Session-gate (Tool::tool_class() == SessionGate): a lifecycle bracket, not a query — it \
     has no answer to classify.";
const REASON_ENGINE_INTERNAL: &str =
    "Engine-internal (Tool::tool_class() == EngineInternal): a mutation or build step over \
     Logos's own artifacts (the index, the wiki store, the config store), not a query with an \
     answer to classify. `ok` already reports whether it succeeded; the \
     Answered/Empty/Unresolved/Failed vocabulary describes what a query found, not what a write \
     or build step did.";
const REASON_READ_MODEL: &str =
    "Read-model (Tool::tool_class() == ReadModel): reports Logos's own state rather than a fact \
     about the indexed code — the same self-referential subject Tool::event_class excludes from \
     the usage figures (FR-OB-09) — and no answered/empty/unresolved split has been defined for \
     it.";

/// `(registered variant, reason)`. Compared for equality with the walk, in
/// both directions: a registered, unclassified tool absent from here fails
/// [`every_registered_tool_is_classified_or_excluded`], and a stale entry
/// naming a tool that is no longer registered, or that is now classified,
/// fails [`no_stale_or_contradictory_exclusions`].
const EXCLUSIONS: &[(&str, &str)] = &[
    // ── Navigation, not yet classified ──────────────────────────────────────
    ("Search", REASON_NAVIGATION_UNCLASSIFIED),
    ("Context", REASON_NAVIGATION_UNCLASSIFIED),
    ("Explore", REASON_NAVIGATION_UNCLASSIFIED),
    ("Node", REASON_NAVIGATION_UNCLASSIFIED),
    ("Callees", REASON_NAVIGATION_UNCLASSIFIED),
    ("ImpactIntersection", REASON_NAVIGATION_UNCLASSIFIED),
    ("BranchOverlap", REASON_NAVIGATION_UNCLASSIFIED),
    ("Implements", REASON_NAVIGATION_UNCLASSIFIED),
    ("ReferencingDocs", REASON_NAVIGATION_UNCLASSIFIED),
    ("GraphElements", REASON_NAVIGATION_UNCLASSIFIED),
    ("WikiRead", REASON_NAVIGATION_UNCLASSIFIED),
    ("WikiSearch", REASON_NAVIGATION_UNCLASSIFIED),
    // ── Quality gate ─────────────────────────────────────────────────────────
    ("Scan", REASON_QUALITY_GATE),
    ("Rescan", REASON_QUALITY_GATE),
    ("Gate", REASON_QUALITY_GATE),
    ("CheckRules", REASON_QUALITY_GATE),
    ("Evolution", REASON_QUALITY_GATE),
    ("Dsm", REASON_QUALITY_GATE),
    ("DocGaps", REASON_QUALITY_GATE),
    ("Health", REASON_QUALITY_GATE),
    ("Doctor", REASON_QUALITY_GATE),
    ("Verify", REASON_QUALITY_GATE),
    ("Hotspots", REASON_QUALITY_GATE),
    ("LatestMetrics", REASON_QUALITY_GATE),
    ("LatestScan", REASON_QUALITY_GATE),
    ("LatestGate", REASON_QUALITY_GATE),
    ("LatestHealth", REASON_QUALITY_GATE),
    ("LatestTemporalReport", REASON_QUALITY_GATE),
    ("LatestHotspots", REASON_QUALITY_GATE),
    ("QualityReadout", REASON_QUALITY_GATE),
    ("LanguageComposition", REASON_QUALITY_GATE),
    ("CoverageStatus", REASON_QUALITY_GATE),
    // ── Session gate ─────────────────────────────────────────────────────────
    ("SessionStart", REASON_SESSION_GATE),
    ("SessionEnd", REASON_SESSION_GATE),
    // ── Engine-internal: indexing / sync pipeline ───────────────────────────
    ("Init", REASON_ENGINE_INTERNAL),
    ("Index", REASON_ENGINE_INTERNAL),
    ("Sync", REASON_ENGINE_INTERNAL),
    ("EnsureIndexed", REASON_ENGINE_INTERNAL),
    ("Discover", REASON_ENGINE_INTERNAL),
    ("Load", REASON_ENGINE_INTERNAL),
    ("Extract", REASON_ENGINE_INTERNAL),
    ("Resolve", REASON_ENGINE_INTERNAL),
    ("Annotate", REASON_ENGINE_INTERNAL),
    ("Persist", REASON_ENGINE_INTERNAL),
    ("WatchSync", REASON_ENGINE_INTERNAL),
    ("WatchCoverageIngest", REASON_ENGINE_INTERNAL),
    ("WorktreeSeed", REASON_ENGINE_INTERNAL),
    ("NavProloguePurge", REASON_ENGINE_INTERNAL),
    // ── Engine-internal: coverage ingestion ─────────────────────────────────
    ("CoverageIngest", REASON_ENGINE_INTERNAL),
    ("CoverageIngestAuto", REASON_ENGINE_INTERNAL),
    ("CoverageRefresh", REASON_ENGINE_INTERNAL),
    // ── Engine-internal: the wiki store's writes and generation ─────────────
    ("WikiWrite", REASON_ENGINE_INTERNAL),
    ("WikiDelete", REASON_ENGINE_INTERNAL),
    ("WikiPrunedLog", REASON_ENGINE_INTERNAL),
    ("WikiGenerate", REASON_ENGINE_INTERNAL),
    ("WikiNative", REASON_ENGINE_INTERNAL),
    ("WikiDocCategoryPresent", REASON_ENGINE_INTERNAL),
    ("WikiSrsMode", REASON_ENGINE_INTERNAL),
    ("WikiGuidePages", REASON_ENGINE_INTERNAL),
    ("WikiReconcile", REASON_ENGINE_INTERNAL),
    ("WikiMaterialize", REASON_ENGINE_INTERNAL),
    ("WikiSkillEmit", REASON_ENGINE_INTERNAL),
    ("WikiQualityReportHookEmit", REASON_ENGINE_INTERNAL),
    // ── Engine-internal: the project's own policy store ─────────────────────
    ("ConfigWrite", REASON_ENGINE_INTERNAL),
    ("ConfigWriteSecret", REASON_ENGINE_INTERNAL),
    ("ConfigApply", REASON_ENGINE_INTERNAL),
    // ── Read-model of Logos's own state ─────────────────────────────────────
    ("Stats", REASON_READ_MODEL),
    ("Status", REASON_READ_MODEL),
    ("Languages", REASON_READ_MODEL),
    ("WikiStatus", REASON_READ_MODEL),
    ("ConfigRead", REASON_READ_MODEL),
];

// ── The parse ────────────────────────────────────────────────────────────────

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

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `token_tree` node of the `registered_tools! { … }` invocation in
/// `tool.rs`, found by walking the whole file rather than assumed at a byte
/// offset — a formatting change must not silently stop this file reading
/// anything.
fn registered_tools_token_tree<'t>(root: Node<'t>, source: &str) -> Node<'t> {
    fn visit<'t>(node: Node<'t>, source: &str) -> Option<Node<'t>> {
        if node.kind() == "macro_invocation" {
            let mut cursor = node.walk();
            let is_registered_tools = node.children(&mut cursor).any(|child| {
                child.kind() == "identifier" && text(child, source) == "registered_tools"
            });
            if is_registered_tools {
                let mut cursor = node.walk();
                let found =
                    node.children(&mut cursor).find(|child| child.kind() == "token_tree");
                if let Some(tt) = found {
                    return Some(tt);
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = visit(child, source) {
                return Some(found);
            }
        }
        None
    }
    visit(root, source).expect("tool.rs invokes registered_tools! { … }")
}

/// Every `Variant => "wire_name"` entry inside `registered_tools! { … }`, in
/// declaration order — the exact list [`Tool::ALL`] is generated from.
///
/// Reads the token stream structurally (identifiers, `=>`, string literals),
/// skipping comments and punctuation, rather than scanning lines: a doc
/// comment can never be mistaken for an entry because it is a different node
/// kind, not because its text happens not to contain `=>`.
///
/// [`Tool::ALL`]: ../src/observability/tool.rs
fn registered_tools(source: &str) -> Vec<(String, String)> {
    let tree = parse(source);
    let token_tree = registered_tools_token_tree(tree.root_node(), source);

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Tok {
        Ident,
        Arrow,
        Str,
    }
    let mut tokens: Vec<(Tok, &str)> = Vec::new();
    let mut cursor = token_tree.walk();
    for child in token_tree.children(&mut cursor) {
        match child.kind() {
            "identifier" => tokens.push((Tok::Ident, text(child, source))),
            "=>" => tokens.push((Tok::Arrow, text(child, source))),
            "string_literal" => tokens.push((Tok::Str, text(child, source))),
            _ => {} // `{`, `}`, `,`, `line_comment`: not part of an entry
        }
    }

    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 3 <= tokens.len() {
        let (k0, variant) = tokens[i];
        let (k1, _) = tokens[i + 1];
        let (k2, wire) = tokens[i + 2];
        assert!(
            k0 == Tok::Ident && k1 == Tok::Arrow && k2 == Tok::Str,
            "registered_tools! entry {i} did not parse as `Ident => \"wire\"`: \
             tool.rs's shape changed in a way this walk does not understand — \
             update registered_tools() rather than reformatting the file to fit it"
        );
        let wire = wire
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(wire);
        pairs.push((variant.to_string(), wire.to_string()));
        i += 3;
    }
    assert_eq!(
        i,
        tokens.len(),
        "registered_tools! token stream did not divide evenly into Ident/=>/Str triples"
    );
    pairs
}

/// Every `Tool::Variant` passed as the first argument of a `traced_with(…)`
/// call in `engine.rs` — the tools actually wired through the classifying
/// seam, read structurally rather than by scanning for the substring
/// `"Tool::"` near the text `"traced_with("`.
fn classified_variants(source: &str) -> HashSet<String> {
    fn visit(node: Node<'_>, source: &str, out: &mut HashSet<String>) {
        if node.kind() == "call_expression" {
            if let Some(function) = node.child_by_field_name("function") {
                if text(function, source).ends_with("traced_with") {
                    if let Some(arguments) = node.child_by_field_name("arguments") {
                        let mut cursor = arguments.walk();
                        let first = arguments
                            .children(&mut cursor)
                            .find(|c| c.kind() != "(" && c.kind() != ")" && c.kind() != ",");
                        if let Some(first) = first {
                            let arg = text(first, source);
                            if let Some(("Tool", variant)) = arg.split_once("::") {
                                out.insert(variant.to_string());
                            }
                        }
                    }
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, source, out);
        }
    }
    let tree = parse(source);
    let mut out = HashSet::new();
    visit(tree.root_node(), source, &mut out);
    out
}

// ── The census ───────────────────────────────────────────────────────────────

#[test]
fn every_registered_tool_is_classified_or_excluded() {
    let registered = registered_tools(&read(TOOL_RS));
    assert!(!registered.is_empty(), "the registry walk found no tools — it is reading nothing");
    let classified = classified_variants(&read(ENGINE_RS));
    let excluded: std::collections::HashMap<&str, &str> = EXCLUSIONS.iter().copied().collect();

    let mut unaccounted: Vec<&str> = Vec::new();
    let mut contradictory: Vec<&str> = Vec::new();
    for (variant, _wire) in &registered {
        let is_classified = classified.contains(variant);
        let is_excluded = excluded.contains_key(variant.as_str());
        match (is_classified, is_excluded) {
            (false, false) => unaccounted.push(variant),
            (true, true) => contradictory.push(variant),
            _ => {}
        }
    }

    assert!(
        unaccounted.is_empty(),
        "these registered tools are enumerated in tool.rs but are neither classified \
         (a `traced_with` call in engine.rs) nor excluded with a reason in EXCLUSIONS: \
         {unaccounted:?} — classify it or add an EXCLUSIONS entry"
    );
    assert!(
        contradictory.is_empty(),
        "these tools are both classified (a real `traced_with` call site) and excluded \
         (an EXCLUSIONS entry claiming they have none): {contradictory:?} — remove the \
         stale EXCLUSIONS entry"
    );
}

#[test]
fn no_stale_or_contradictory_exclusions() {
    let registered: HashSet<String> =
        registered_tools(&read(TOOL_RS)).into_iter().map(|(v, _)| v).collect();
    let classified = classified_variants(&read(ENGINE_RS));

    let mut seen = HashSet::new();
    let mut duplicated: Vec<&str> = Vec::new();
    let mut stale_not_registered: Vec<&str> = Vec::new();
    let mut stale_now_classified: Vec<&str> = Vec::new();
    for (variant, _reason) in EXCLUSIONS {
        if !seen.insert(*variant) {
            duplicated.push(variant);
        }
        if !registered.contains(*variant) {
            stale_not_registered.push(variant);
        }
        if classified.contains(*variant) {
            stale_now_classified.push(variant);
        }
    }

    assert!(duplicated.is_empty(), "EXCLUSIONS lists the same tool twice: {duplicated:?}");
    assert!(
        stale_not_registered.is_empty(),
        "EXCLUSIONS names a tool tool.rs no longer registers: {stale_not_registered:?} — \
         remove the stale entry"
    );
    assert!(
        stale_now_classified.is_empty(),
        "EXCLUSIONS names a tool that now has a real `traced_with` call site: \
         {stale_now_classified:?} — remove the stale entry, the tool is classified"
    );
}

#[test]
fn every_exclusion_carries_a_stated_reason() {
    let empty: Vec<&str> =
        EXCLUSIONS.iter().filter(|(_, reason)| reason.trim().is_empty()).map(|(v, _)| *v).collect();
    assert!(empty.is_empty(), "these EXCLUSIONS entries carry no reason: {empty:?}");
}

/// The denominator, printed (`--nocapture`) and pinned as a dated fact — not a
/// floor a later reading must clear, the same discipline
/// `absence_taxonomy_audit.rs`'s own denominator test follows.
#[test]
fn the_census_reports_its_denominator() {
    let registered = registered_tools(&read(TOOL_RS));
    let classified = classified_variants(&read(ENGINE_RS));
    let excluded: HashSet<&str> = EXCLUSIONS.iter().map(|(v, _)| *v).collect();

    let enumerated = registered.len();
    let classified_count =
        registered.iter().filter(|(v, _)| classified.contains(v)).count();
    let excluded_count =
        registered.iter().filter(|(v, _)| excluded.contains(v.as_str())).count();

    println!(
        "traced-tool classification census ({AUDITED_ON}): {enumerated} enumerated, \
         {classified_count} classified, {excluded_count} excluded"
    );
    assert_eq!(
        (enumerated, classified_count, excluded_count),
        (75, 4, 71),
        "traced-tool classification census as of {AUDITED_ON}: expected 75 enumerated tools, \
         4 classified via traced_with, 71 excluded with a reason"
    );
}
