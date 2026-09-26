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
//!    argument to a `traced_with(…)` call in **any production file** under
//!    `logos-core/src` ([`classified_variants`]). A tool is classified if and
//!    only if some call site traces it through `traced_with` rather than
//!    `traced` — this walk is exactly that definition, not an approximation of
//!    it.
//!
//! Every registered tool not found classified must appear in [`EXCLUSIONS`]
//! with a stated reason ([`every_registered_tool_is_classified_or_excluded`]).
//! Adding a tool that is neither is exactly the failure this file exists to
//! catch, and it fails loudly rather than recording another silent `NULL`.
//!
//! # Why every production file, and why an unreadable call site panics
//!
//! Today the four `traced_with` call sites all sit in `engine.rs`, but plain
//! `traced*` calls already live in `config/workspace_tier.rs`,
//! `federation/manifest.rs` and `pipeline/mod.rs`, and `watch/mod.rs` emits
//! telemetry events directly. The walk once scanned `engine.rs` alone and
//! claimed a call site added anywhere else would fail loudly. That held only
//! for a *new* tool. Switching an already-excluded tool to `traced_with` in
//! another file, or even in `engine.rs` through a qualified path
//! (`crate::observability::Tool::Callees`), left the census green while the
//! tool's [`EXCLUSIONS`] entry became false: the admission direction, which is
//! the one a census that fails only on omissions cannot see (Sprint 78 review).
//!
//! So the walk now covers every `.rs` file under `src/` except test-only code
//! (a file named `*tests.rs`, or an item under `#[cfg(test)]`). A `traced_with`
//! call whose first argument is not a literal `Tool::Variant` (with or
//! without a leading path) **panics** instead of being skipped, because the
//! census cannot tell which tool it classifies. The fix is to pass the variant
//! literally, never to add an [`EXCLUSIONS`] entry. One more route to a
//! non-`NULL` outcome exists: the telemetry layer accepts an `outcome` field
//! from any event on the telemetry target. The walk therefore also fails on any
//! telemetry-target macro outside `observability/mod.rs` (the home of
//! `traced_inner`'s own emit) that names an `outcome` field
//! ([`no_telemetry_event_outside_the_seam_carries_an_outcome`]).
//!
//! Demonstrated in the admission direction, each mutation reverted and
//! `touch`ed afterwards: `ConfigRead` switched to `traced_with` in
//! `federation/manifest.rs`, and `Callees` switched to `traced_with` in
//! `engine.rs` through `crate::observability::Tool::Callees`, each fail the
//! contradictory/stale checks and read `(75, 5, 71)` (both were green before
//! this widening). `Tool::Impact` passed through a local fails with the
//! literal-argument panic, and an `outcome = "answered"` field added to a
//! `watch/mod.rs` telemetry event fails the seam test. A `traced_with` placed
//! inside `engine.rs`'s `#[cfg(test)]` module stays out of the count.
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

/// The registry file, and the production tree the classification is read
/// from, both relative to `logos-core` (`env!("CARGO_MANIFEST_DIR")`).
const TOOL_RS: &str = "src/observability/tool.rs";
const SRC_DIR: &str = "src";

/// The one file allowed to put an `outcome` field on a telemetry-target event:
/// `traced_inner`'s own emit, which is what `traced_with` feeds.
const OUTCOME_EMITTER: &str = "src/observability/mod.rs";

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
     about the indexed code, and no answered/empty/unresolved split has been defined for it. \
     (Only stats and status are also excluded from the usage figures as self-referential \
     reads, Tool::event_class / FR-OB-09; languages, wiki_status and config_read are counted.)";

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

/// Every production `.rs` file under `src/`, as `(path relative to
/// logos-core, source)`, sorted. Test-only files (`*tests.rs`) are left out
/// here; inline `#[cfg(test)]` items are skipped during the walk.
fn production_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|x| x == "rs")
                && !path.file_name().is_some_and(|n| n.to_string_lossy().ends_with("tests.rs"))
            {
                let relative = path
                    .strip_prefix(root)
                    .expect("under the manifest dir")
                    .to_string_lossy()
                    .replace('\\', "/");
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                out.push((relative, source));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    walk(&root.join(SRC_DIR), root, &mut out);
    out.sort();
    assert!(
        out.iter().any(|(p, _)| p == "src/engine.rs"),
        "the production walk did not reach src/engine.rs — it is reading the wrong tree"
    );
    out
}

/// Whether `node` is an item gated behind `#[cfg(test)]`: its preceding
/// attribute siblings (comments between them allowed) include one.
fn is_cfg_test_item(node: Node<'_>, source: &str) -> bool {
    let mut sibling = node.prev_sibling();
    while let Some(s) = sibling {
        match s.kind() {
            "attribute_item" if text(s, source).replace(' ', "").contains("cfg(test)") => {
                return true;
            }
            "attribute_item" | "line_comment" | "block_comment" => sibling = s.prev_sibling(),
            _ => return false,
        }
    }
    false
}

/// The bare name a call expression's `function` node calls: `f`, `a::b::f`
/// and `f::<T>` all name `f`. A method call (`x.f()`) names nothing here,
/// because `traced_with` is a free function.
fn callee_name<'s>(function: Node<'_>, source: &'s str) -> Option<&'s str> {
    match function.kind() {
        "identifier" => Some(text(function, source)),
        "scoped_identifier" => function.child_by_field_name("name").map(|n| text(n, source)),
        "generic_function" => {
            callee_name(function.child_by_field_name("function")?, source)
        }
        _ => None,
    }
}

/// The `Variant` of a `Tool::Variant` / `…::Tool::Variant` argument, or `None`
/// when the argument is anything else (a local, a call, a field).
fn tool_variant<'s>(arg: Node<'_>, source: &'s str) -> Option<&'s str> {
    if arg.kind() != "scoped_identifier" {
        return None;
    }
    let path = arg.child_by_field_name("path")?;
    let last_segment = match path.kind() {
        "identifier" => text(path, source),
        "scoped_identifier" => text(path.child_by_field_name("name")?, source),
        _ => return None,
    };
    if last_segment != "Tool" {
        return None;
    }
    arg.child_by_field_name("name").map(|n| text(n, source))
}

/// Whether a macro's token tree names an `outcome` identifier anywhere.
fn names_outcome(node: Node<'_>, source: &str) -> bool {
    if node.kind() == "identifier" && text(node, source) == "outcome" {
        return true;
    }
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| names_outcome(c, source));
    found
}

/// What the production walk found: the classified variants, and every
/// telemetry-target macro outside [`OUTCOME_EMITTER`] that names an `outcome`
/// field (as `path:line`).
struct Walk {
    classified: HashSet<String>,
    stray_outcome_emits: Vec<String>,
}

/// Every `Tool::Variant` passed as the first argument of a `traced_with(…)`
/// call anywhere in the production tree — the tools actually wired through the
/// classifying seam, read structurally rather than by scanning for the
/// substring `"Tool::"` near the text `"traced_with("`.
///
/// # Panics
///
/// On a `traced_with` call whose first argument is not a literal
/// `Tool::Variant`: skipping it would silently under-count, and reporting the
/// tool unclassified would invite a false [`EXCLUSIONS`] entry.
fn walk_production() -> Walk {
    fn visit(node: Node<'_>, path: &str, source: &str, walk: &mut Walk) {
        if is_cfg_test_item(node, source) {
            return;
        }
        if node.kind() == "call_expression" {
            let function = node.child_by_field_name("function");
            if function.and_then(|f| callee_name(f, source)) == Some("traced_with") {
                let arguments = node.child_by_field_name("arguments");
                let first = arguments.and_then(|a| a.named_child(0));
                let line = node.start_position().row + 1;
                let variant = first.and_then(|f| tool_variant(f, source));
                match variant {
                    Some(variant) => {
                        walk.classified.insert(variant.to_string());
                    }
                    None => panic!(
                        "{path}:{line}: traced_with's first argument `{}` is not a literal \
                         `Tool::Variant`, so this census cannot tell which tool it classifies — \
                         pass the variant literally; never answer this with an EXCLUSIONS entry",
                        first.map_or("<none>", |f| text(f, source))
                    ),
                }
            }
        }
        if node.kind() == "macro_invocation" && path != OUTCOME_EMITTER {
            let body = text(node, source);
            if (body.contains("TELEMETRY_TARGET") || body.contains("logos::telemetry"))
                && names_outcome(node, source)
            {
                walk.stray_outcome_emits
                    .push(format!("{path}:{}", node.start_position().row + 1));
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, path, source, walk);
        }
    }
    let mut walk = Walk { classified: HashSet::new(), stray_outcome_emits: Vec::new() };
    for (path, source) in production_sources() {
        let tree = parse(&source);
        visit(tree.root_node(), &path, &source, &mut walk);
    }
    walk
}

fn classified_variants() -> HashSet<String> {
    walk_production().classified
}

// ── The census ───────────────────────────────────────────────────────────────

#[test]
fn every_registered_tool_is_classified_or_excluded() {
    let registered = registered_tools(&read(TOOL_RS));
    assert!(!registered.is_empty(), "the registry walk found no tools — it is reading nothing");
    let classified = classified_variants();
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
         (a `traced_with` call in logos-core/src) nor excluded with a reason in EXCLUSIONS: \
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
    let classified = classified_variants();

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

/// The second route to a non-`NULL` outcome: the telemetry layer reads an
/// `outcome` field off any event on the telemetry target, so a direct
/// `tracing::info!(target: TELEMETRY_TARGET, outcome = …)` would classify a
/// tool that has no `traced_with` call and that this census reads as excluded.
#[test]
fn no_telemetry_event_outside_the_seam_carries_an_outcome() {
    let stray = walk_production().stray_outcome_emits;
    assert!(
        stray.is_empty(),
        "these telemetry-target events carry an `outcome` field outside {OUTCOME_EMITTER}, \
         so they classify a tool behind this census's back: {stray:?} — route the call \
         through traced_with instead"
    );
}

/// The denominator, printed (`--nocapture`) and pinned as a dated fact — not a
/// floor a later reading must clear, the same discipline
/// `absence_taxonomy_audit.rs`'s own denominator test follows.
#[test]
fn the_census_reports_its_denominator() {
    let registered = registered_tools(&read(TOOL_RS));
    let classified = classified_variants();
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
