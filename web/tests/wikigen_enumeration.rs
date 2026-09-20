//! **Every engine call in `web/src/wikigen/` is enumerated and classified**
//! ([FR-OB-13], [CR-139] §2.3, [BR-42]).
//!
//! # Why this module is enumerated rather than patched
//!
//! [CR-139] §2.3 records what made the misattributed `wiki_materialize` call
//! invisible to [S-316]'s build-failure census: it carries no `Surface::` token,
//! makes no `bridge` call, and sits in a file outside the two sources that
//! census reads. Every one of those three is a property of the *census*, not of
//! the call — so a fix that classified this one call would leave exactly the
//! same hole open for the next one. This walks the module instead and compares
//! what it finds against a declared table, so an undeclared path to the engine
//! is a test failure rather than a silent inclusion.
//!
//! # Not feature-gated
//!
//! The module it scans compiles only under `agents` (CR-078/ADR-60), but the
//! *files* are on disk in every build and this reads them as text. So the
//! enumeration holds in a listen-only `--features ui` build too, where nothing
//! else in this crate looks at `wikigen` at all.
//!
//! # Why the scanner lives here and not in `wikigen`'s own test module
//!
//! It scans a directory by walking it. A scanner inside that directory finds
//! its own markers — its `MARKERS` list, its fixtures, its declared table — and
//! reports them as sites in whatever `fn` happens to precede them. The first
//! draft was written in `web/src/wikigen/mod.rs` and did exactly that, adding
//! eight phantom sites attributed to `sse_body` and to the fixtures' own
//! helpers. Parsing string literals out would work and is what `lib.rs`'s
//! `fn_body` does for braces, but raw strings and escapes make that a second
//! thing to get right; keeping the scanner outside the scanned tree removes the
//! self-reference by construction instead. ([`strip_comments`] does now track
//! quotes, but for a different defect review found: a `//` inside a string
//! literal was truncating real markers away.)
//!
//! [BR-42]: ../../docs/specs/software-spec.md#316-observability--telemetry
//! [CR-139]: ../../docs/requests/CR-139-the-wiki-generation-pass-names-its-own-surface.md
//! [FR-OB-13]: ../../docs/specs/requirements/FR-OB-13.md
//! [S-316]: ../../docs/planning/journal.md#s-316-classify-the-shells-status-read-as-non-usage-telemetry

/// Every `.rs` file this module is made of. Compared for **equality** with the
/// directory walk, so adding a file to `web/src/wikigen/` fails here until it is
/// declared and its sites classified.
const DECLARED_FILES: [&str; 2] = ["configured.rs", "mod.rs"];

/// Every engine-reaching site in `web/src/wikigen/`, with its classification:
/// `(file, enclosing fn, marker, occurrences, classification)`.
///
/// `mod.rs` declares none, and that is a finding rather than an omission: the
/// run-state/SSE half of this module holds the single-run lock, the broadcast
/// fan-out and the frame mapping, and never touches the `Engine` at all.
const DECLARED_SITES: [(&str, &str, &str, usize, &str); 5] = [
    (
        "configured.rs",
        "start_run",
        "spawn_blocking",
        2,
        "the module's two ADR-03 bridge hops. (1) the deterministic presented \
         tier — classified, it carries the `Surface::WikiGen` scope below; \
         (2) the config/secrets read, which calls no Engine method and emits no \
         telemetry (`load_config_from_root` / `load_secrets_from_root` are free \
         functions that bypass the Engine chokepoint), so it has nothing to \
         attribute",
    ),
    (
        "configured.rs",
        "start_run",
        "engine.root",
        1,
        "a plain path accessor, not a chokepoint method: it is not `traced` and \
         emits no telemetry event, so it carries no surface to get wrong",
    ),
    (
        "configured.rs",
        "start_run",
        "engine.wiki_materialize",
        1,
        "CLASSIFIED — `Surface::WikiGen`. The CR-139 site: a `traced` chokepoint \
         call (`Tool::WikiMaterialize`) that would otherwise inherit the \
         `serve --ui` process surface and book Logos's own generator as a \
         developer browsing the dashboard",
    ),
    (
        "configured.rs",
        "start_run",
        "in_surface",
        1,
        "the classification itself, installed inside the materialize \
         `spawn_blocking`. Declared as a site so that removing it — the mutation \
         that restores the defect — changes this table too, and not only a \
         behavioural test",
    ),
    (
        "configured.rs",
        "start_run",
        "run_configured",
        1,
        "OUT OF MODULE, and the one reach this scope does NOT cover. The engine \
         is handed to `wiki-agent`, which owns its own `spawn_blocking` hops on \
         its own threads (`wiki-agent/src/agent.rs` — `wiki_generate`, \
         `wiki_read`, and the per-page grounding and write calls); `in_surface` \
         is thread-scoped, so nothing installed here reaches them and they are \
         emitted under the process surface today. Recorded rather than fixed: \
         `wiki-agent/**` is outside S-435's stated ownership and CR-139 §3.2 \
         scopes the change to the materialize call. Stated so the next audit \
         starts from a known reach rather than an assumption",
    ),
];

/// The markers that locate a site where this module reaches the `Engine`.
///
/// `spawn_blocking` is the load-bearing one: [ADR-03] makes it the only way a
/// surface touches the synchronous core, so a call reached through a binding the
/// `engine.` matcher does not name by sight is still caught by the hop it has to
/// cross. `in_surface` is here because the classification is itself part of what
/// the table records, and `run_configured` because handing the engine to another
/// crate is a reach even though no method is called on it here.
const MARKERS: [&str; 3] = ["spawn_blocking(", "in_surface(", "run_configured("];

/// A synthetic module whose second engine call sits **after** a `#[cfg(test)]`
/// test module — the exact shape that made the first draft of a comparable
/// census report clean by construction (Sprint 72 review, appendix 5.9: a
/// helper that truncates at the first `#[cfg(test)] mod tests {` kept nothing
/// after it, so a marker past that point passed a guard it should have failed).
const FIXTURE_WITH_A_SITE_AFTER_THE_TEST_MODULE: &str = "\
fn already_declared() {
    tokio::task::spawn_blocking(move || engine.wiki_materialize());
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        // engine.wiki_generate() named in a comment is prose, not a site
    }
}

fn a_second_unclassified_path() {
    tokio::task::spawn_blocking(move || engine.wiki_generate());
}
";

/// Is the match at `at` a whole identifier, or the tail of a longer one? Same
/// rule, and the same reason, as `lib.rs`'s own `whole_identifier`:
/// `my_engine.` ends with the word this scan looks for and is not it.
fn whole_identifier(code: &str, at: usize) -> bool {
    code[..at]
        .chars()
        .next_back()
        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
}

/// Comments stripped — prose *about* a marker is not a marker — and **every line
/// kept**, test module included.
///
/// Deliberately not the `production_code` shape used elsewhere in this crate,
/// for the reason the module header gives: truncating at the first
/// `#[cfg(test)] mod tests {` is what made a comparable guard false-green, and
/// [`a_new_site_is_detected_even_after_a_test_module`] proves this scan does not
/// have that defect rather than asserting it.
///
/// # `//` is only a comment outside a string literal
///
/// The obvious one-liner — `line.split_once("//")` — is **quote-blind**, and
/// review reproduced what that costs: a line carrying a URL in a string,
/// `let u = "https://example.com"; spawn_blocking(move || engine.foo());`,
/// truncates at the URL's own `//` and the two markers after it vanish. That is
/// a **silent miss**, and an earlier version of this very doc claimed the
/// opposite — "a loud false positive, never a silent miss". It was wrong, and
/// the direction it was wrong in is the one a census must never be wrong in:
/// the whole point of this file is that an unclassified site fails loudly.
///
/// So the scan tracks whether it is inside a `"…"` (honouring `\` escapes) and
/// treats `//` as a comment start only outside one. A marker written inside a
/// string literal is still reported as a site — that false positive is kept
/// deliberately, since it fails loudly, and a census is allowed to be wrong
/// only in that direction.
///
/// Every line is kept, test module included — deliberately not the
/// `production_code` shape used elsewhere in this crate, which truncates at the
/// first `#[cfg(test)] mod tests {`. Sprint 72 appendix 5.9 records what that
/// costs, and [`a_new_site_is_detected_even_after_a_test_module`] proves this
/// scan does not have that defect rather than asserting it.
fn strip_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let (mut in_str, mut i) = (false, 0usize);
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if in_str => i += 1,
                    b'"' => in_str = !in_str,
                    b'/' if !in_str && bytes.get(i + 1) == Some(&b'/') => return &line[..i],
                    _ => {}
                }
                i += 1;
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The name of the `fn` enclosing `at` — `fn`, `async fn` and `pub fn` alike, so
/// an occurrence is attributed without depending on line layout.
fn enclosing_fn(code: &str, at: usize) -> String {
    let mut cursor = &code[..at];
    loop {
        let Some(start) = cursor.rfind("fn ") else {
            return "<no enclosing fn>".to_string();
        };
        if whole_identifier(cursor, start) {
            let rest = &code[start + "fn ".len()..];
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            return rest[..end].to_string();
        }
        cursor = &cursor[..start];
    }
}

/// Every engine-reaching site in one comment-stripped source, as
/// `(enclosing fn, marker)`, sorted.
///
/// Two matchers. The [`MARKERS`] list, matched as whole identifiers; and
/// `engine.<method>(`, which yields the method name so two different calls on
/// the same binding are two distinct sites rather than one count of two. The
/// second matcher's reach is exactly the token `engine`, so a binding named
/// `wiki_engine` is not caught by sight — which is why `spawn_blocking` is in
/// the list above.
fn engine_sites(code: &str) -> Vec<(String, String)> {
    let mut sites: Vec<(String, String)> = Vec::new();
    for marker in MARKERS {
        for (at, _) in code.match_indices(marker) {
            if whole_identifier(code, at) {
                sites.push((
                    enclosing_fn(code, at),
                    marker.trim_end_matches('(').to_string(),
                ));
            }
        }
    }
    for (at, _) in code.match_indices("engine.") {
        if !whole_identifier(code, at) {
            continue;
        }
        let rest = &code[at + "engine.".len()..];
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let (method, tail) = rest.split_at(end);
        // A field read (`engine.root`) is not a call, and only a call can emit.
        if method.is_empty() || !tail.trim_start().starts_with('(') {
            continue;
        }
        sites.push((enclosing_fn(code, at), format!("engine.{method}")));
    }
    sites.sort();
    sites
}

/// The matcher, probed with the near misses it must reject — run, not read.
#[test]
fn the_matcher_rejects_its_near_misses() {
    let decoy = "fn f() { my_engine.wiki_materialize(); engine.root; engine.root(); }";
    assert_eq!(
        engine_sites(decoy),
        vec![("f".to_string(), "engine.root".to_string())],
        "`my_engine.` ends with the token this scan looks for and is not it; \
         `engine.root` without parentheses is a field read and cannot emit"
    );

    // A `//` INSIDE a string literal is not a comment start. Quote-blind
    // stripping truncates the line there and drops both markers after it — a
    // silent miss, the one direction this census must never fail in.
    // Reproduced by review before it was fixed; pinned here so it cannot return.
    let url_line = "fn h() {\n    let u = \"https://example.com\"; tokio::task::spawn_blocking(move || engine.wiki_materialize());\n}";
    assert_eq!(
        engine_sites(&strip_comments(url_line)),
        vec![
            ("h".to_string(), "engine.wiki_materialize".to_string()),
            ("h".to_string(), "spawn_blocking".to_string()),
        ],
        "a URL's `//` inside a string literal must not truncate the line and \
         hide the markers after it"
    );
    assert!(
        engine_sites(&strip_comments("fn i() { } // engine.wiki_read( named in a comment"))
            .is_empty(),
        "…while a genuine trailing comment is still prose, not a site"
    );

    let wrapped = "fn g() {\n    tokio::task::spawn_blocking(\n        move || engine.wiki_read(&slug),\n    );\n}";
    assert_eq!(
        engine_sites(wrapped),
        vec![
            ("g".to_string(), "engine.wiki_read".to_string()),
            ("g".to_string(), "spawn_blocking".to_string()),
        ],
        "a call rustfmt wrapped across lines is still one site in one fn — the \
         evasion a per-physical-line matcher admits"
    );
}

/// The census is **demonstrated to detect an added site**, including one past a
/// `#[cfg(test)]` module — not merely observed to run clean today.
///
/// A guard that reports clean by construction is the failure class this census
/// exists to prevent, so the detection is proven on a fixture: the extractor
/// finds the added call, *and* the comparison the real test performs rejects the
/// fixture's site set against the declared table.
#[test]
fn a_new_site_is_detected_even_after_a_test_module() {
    let sites = engine_sites(&strip_comments(FIXTURE_WITH_A_SITE_AFTER_THE_TEST_MODULE));

    assert!(
        sites.contains(&(
            "a_second_unclassified_path".to_string(),
            "engine.wiki_generate".to_string()
        )),
        "a site added AFTER the test module must be found — truncating there is \
         what made the first draft of a comparable guard false-green: {sites:?}"
    );
    assert!(
        sites.contains(&("already_declared".to_string(), "spawn_blocking".to_string()))
            && sites.contains(&(
                "a_second_unclassified_path".to_string(),
                "spawn_blocking".to_string()
            )),
        "and both bridge hops, one either side of the test module: {sites:?}"
    );
    assert!(
        !sites.iter().any(|(enclosing, _)| enclosing == "t"),
        "the call named in a comment inside the test fn is prose, not a site: \
         {sites:?}"
    );

    // The comparison, not just the extractor: an undeclared site must survive
    // into the diff the real assertion makes.
    let declared: Vec<(String, String)> = DECLARED_SITES
        .iter()
        .flat_map(|(_, enclosing, marker, count, _)| {
            std::iter::repeat_n(((*enclosing).to_string(), (*marker).to_string()), *count)
        })
        .collect();
    assert_ne!(
        sites, declared,
        "the declared table must not accept a module carrying an unclassified \
         second path to the engine"
    );
}

/// The module, walked from disk, matches the declared table exactly.
#[test]
fn every_engine_call_in_the_wikigen_module_is_enumerated_and_classified() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("wikigen");
    let mut walked: Vec<String> = Vec::new();
    let mut found: Vec<(String, String, String)> = Vec::new();
    for entry in
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
    {
        let path = entry.expect("a readable directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let file = path
            .file_name()
            .expect("a named file")
            .to_string_lossy()
            .to_string();
        walked.push(file.clone());
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (enclosing, marker) in engine_sites(&strip_comments(&source)) {
            found.push((file.clone(), enclosing, marker));
        }
    }

    walked.sort();
    let mut expected_files: Vec<String> = DECLARED_FILES.iter().map(|f| (*f).to_string()).collect();
    expected_files.sort();
    assert_eq!(
        walked, expected_files,
        "`web/src/wikigen/` is exactly these files. A new one carries new paths \
         to the engine, so it is declared here and its sites classified in \
         DECLARED_SITES (FR-OB-13, CR-139 §2.3)"
    );

    // Anti-vacuity, and it is not the same claim as the equality below: a walk
    // rooted at the wrong directory, or a scan that matched nothing, reports an
    // empty set — which would compare equal to an empty declared table and mean
    // nothing.
    assert!(
        !found.is_empty(),
        "the walk found no engine-reaching site in {} — it cannot have read the \
         module",
        dir.display()
    );

    let mut declared: Vec<(String, String, String)> = Vec::new();
    for (file, enclosing, marker, count, classification) in DECLARED_SITES {
        assert!(
            !classification.is_empty(),
            "{file}/{enclosing}/{marker} is declared without a classification"
        );
        for _ in 0..count {
            declared.push((
                file.to_string(),
                enclosing.to_string(),
                marker.to_string(),
            ));
        }
    }
    found.sort();
    declared.sort();
    assert_eq!(
        found, declared,
        "every engine call in `web/src/wikigen/` is enumerated and classified. \
         An undeclared site is a path to the engine that no surface classifies \
         — the CR-139 defect, one call along (FR-OB-13, BR-42). Add it to \
         DECLARED_SITES with what attributes it"
    );
}
