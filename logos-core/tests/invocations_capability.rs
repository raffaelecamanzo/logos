//! The `invocations` capability vocabulary and its cross-plugin invariant
//! (S-340, [CR-108](../../docs/requests/CR-108-per-language-http-client-call-capture.md),
//! [FR-WS-08](../../docs/specs/requirements/FR-WS-08.md)).
//!
//! This file lands the descriptor plumbing and the shared guard **only** — it
//! ships no language's `invocations.scm` (that is [S-341] through [S-348]'s
//! work). Gated on the exact set of languages [FR-WS-08]'s normative table
//! names (the nine `frameworks`-shipping languages plus Rust, which already
//! ships `invocations`, plus C/C++/Scala, which ship neither) so the "exactly
//! nine violations" assertion below stays meaningful under any feature
//! selection rather than silently drifting if the plugin set changes shape.
//!
//! [S-341]: ../../docs/planning/journal.md#s-341-java-http-client-call-capture
//! [S-348]: ../../docs/planning/journal.md#s-348-php-http-client-call-capture
#![cfg(all(
    feature = "lang-rust",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-scala",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]

use logos_core::plugin::{LanguagePlugin, LanguageRegistry};
use logos_core::Engine;

/// A loaded plugin's `capabilities()` snapshot as an owned, sorted-for-display
/// helper — kept local to this file since it is a test-only convenience.
fn has_capability(plugin: &dyn LanguagePlugin, capability: &str) -> bool {
    plugin.capabilities().iter().any(|c| c == capability)
}

// ── FR-WS-08 AC (capability vocabulary): `invocations` is reported per
// language, and reported ABSENT — never empty-but-present — for c/cpp/scala
// (they ship no `frameworks` and therefore no client side). ──────────────────

#[test]
fn invocations_is_reported_present_for_rust_and_absent_for_c_cpp_scala() {
    let tmp = tempfile::tempdir().unwrap();
    let info = Engine::open(tmp.path()).languages();
    assert!(
        info.load_error.is_none(),
        "the embedded plugin set must load cleanly, got {:?}",
        info.load_error
    );

    let rust = info
        .languages
        .iter()
        .find(|d| d.name == "rust")
        .expect("rust is compiled in by default");
    assert!(
        rust.capabilities.iter().any(|c| c == "invocations"),
        "rust already ships an invocations.scm and must report the capability: {:?}",
        rust.capabilities
    );

    for name in ["c", "cpp", "scala"] {
        let d = info
            .languages
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} is compiled in by default"));
        assert!(
            !d.capabilities.iter().any(|c| c == "frameworks"),
            "{name} ships no frameworks capability: {:?}",
            d.capabilities
        );
        assert!(
            !d.capabilities.iter().any(|c| c == "invocations"),
            "{name} must report `invocations` absent (never empty-but-present) \
             since it ships no client side: {:?}",
            d.capabilities
        );
    }
}

/// Each language story's own AC — "both plugins carry the capability" — pinned
/// by a test that is **green today** (S-341: java; S-343: typescript + tsx;
/// S-345: go).
///
/// The invariant below is the sprint-wide guard and stays red by design until
/// the last of S-341..S-348 lands, so it cannot serve as any single story's
/// evidence. This row grows by one entry per story instead, which also makes a
/// later regression attributable to a language rather than to "the invariant".
#[test]
fn each_landed_language_reports_the_invocations_capability() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    // Rust (pre-CR-108), then S-341 (java), S-343 (typescript, tsx), S-345 (go),
    // S-342 (kotlin), S-346 (c-sharp), S-347 (ruby). Subsequent stories append here.
    //
    // Rust belongs in this row even though the test above already reports its
    // capability: that one reads the `languages()` descriptor summary and never
    // calls `plugin.query("invocations")`, so the stronger assertion — the
    // declared query actually LOADS — was not applied to the language that has
    // shipped the arm longest.
    for name in ["rust", "java", "typescript", "tsx", "go", "kotlin", "c-sharp", "ruby"] {
        let plugin = reg
            .iter()
            .find(|p| p.name() == name)
            .unwrap_or_else(|| panic!("{name} is compiled in by default"));
        assert!(
            has_capability(plugin, "invocations"),
            "{name} ships an invocations.scm and must declare the capability: {:?}",
            plugin.capabilities()
        );
        assert!(
            plugin.query("invocations").is_some(),
            "{name}'s declared invocations query must actually load — a declared \
             but unloadable query is the silent no-capture failure S-340 closed"
        );
    }
}

// ── FR-WS-08 AC5 / CR-108: every plugin declaring `frameworks` also declares
// `invocations` — a language shipping a provider side (a route) with no
// consumer side (a client-call capture) is a conformance failure, not an
// honest absence. ─────────────────────────────────────────────────────────
//
// RED BY DESIGN (S-340): today nine languages ship `frameworks` without
// `invocations` — csharp, go, java, kotlin, php, python, ruby, tsx, typescript
// — exactly the CR-108 gap this sprint closes across Iterations 2-3 ([S-341]
// through [S-348]). This assertion is written for the *end state*
// (`violations.is_empty()`) rather than pinned to "nine" so it turns green on
// its own, with no edit here, as each language story lands its
// `invocations.scm` — the guard that stops a **tenth** language ever shipping
// a provider side with no consumer side again.
#[test]
fn every_plugin_declaring_frameworks_also_declares_invocations() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    let mut violations: Vec<&str> = reg
        .iter()
        .filter(|p| has_capability(*p, "frameworks") && !has_capability(*p, "invocations"))
        .map(|p| p.name())
        .collect();
    violations.sort_unstable();

    assert!(
        violations.is_empty(),
        "every plugin declaring `frameworks` must also declare `invocations` \
         (FR-WS-08 AC5, CR-108) — missing: {violations:?}. On today's tree this \
         is exactly ['c-sharp', 'go', 'java', 'kotlin', 'php', 'python', 'ruby', \
         'tsx', 'typescript'] (nine languages) — S-341 through S-348 each remove \
         one entry by shipping that language's invocations.scm, with no edit to \
         this test required; it turns green on its own once the last lands."
    );
}

// ── The structural guard S-342 wishes had existed ──────────────────────────

/// **Every pattern in every `invocations` query must capture something.**
///
/// A capture-less pattern is not a harmless no-op — it is the signature of a
/// **detached predicate**. Tree-sitter opens a new pattern for each top-level
/// s-expression, and a `(#match? …)` written at column 0 after a completed
/// pattern (rather than inside its parentheses) becomes its own step-less,
/// capture-less pattern. The predicate then constrains nothing, and the pattern
/// it was meant to guard runs wide open.
///
/// S-342 shipped exactly that. Kotlin's pattern 4 — the broad
/// `<receiver>.<method>(<arg>)` anchor — carried its lower-case-receiver rule
/// and its string-template guard outside the alternation, so both were inert:
/// the arm captured `RequestPredicates.GET("/users")` (a Spring WebFlux **route
/// declaration**, which would have bound another workspace member's real
/// `GET /users` and fabricated a cross-service edge), `Paths.get("/etc/hosts")`
/// (a filesystem path) and `restTemplate.delete("/carts/$id")` (a
/// runtime-composed template bound as though static) — the [NFR-RA-05]
/// never-fabricate breach the whole arm exists to avoid. Every per-language test
/// stayed green, because a rule with no test cannot notice that it stopped
/// applying.
///
/// This guard is language-agnostic and needs no per-language knowledge, so it
/// covers the four CR-108 arms still landing as well as the four already merged.
/// It is deliberately weaker than "assert the intended pattern count": a count
/// has to be maintained per language and per edit, whereas "no pattern captures
/// nothing" is true of every correct query by construction and cannot drift.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn no_invocations_query_contains_a_capture_less_pattern() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    let mut checked = 0usize;
    for plugin in reg.iter() {
        let Some(query) = plugin.query("invocations") else {
            continue;
        };
        checked += 1;
        for index in 0..query.pattern_count() {
            let captures_something = query
                .capture_quantifiers(index)
                .iter()
                .any(|q| *q != tree_sitter::CaptureQuantifier::Zero);
            assert!(
                captures_something,
                "{}'s invocations.scm pattern #{index} (source byte {}) captures \
                 nothing. That is almost always a predicate written at column 0 \
                 after a closing `)` or `]` instead of inside it — in which case \
                 the pattern it was meant to guard is running unguarded.",
                plugin.name(),
                query.start_byte_for_pattern(index)
            );
        }
    }
    assert!(
        checked >= 5,
        "expected the landed invocations arms to be compiled in, saw {checked}"
    );
}
