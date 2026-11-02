//! A plugin query may not put more captures on one pattern node than tree-sitter
//! keeps (S-841, [CR-264](../../docs/requests/CR-264-sprint-109-review-follow-ups.md),
//! [FR-PL-02](../../docs/specs/requirements/FR-PL-02.md)).
//!
//! tree-sitter keeps a fixed number of capture ids per query step
//! (`MAX_STEP_CAPTURE_COUNT`, 3 on the pinned 0.25.10) and **silently drops the
//! rest**: the query compiles, the match simply never carries them. It still
//! records the dropped capture's quantifier, so `Query::capture_quantifiers`
//! cannot reveal the loss either. The Sprint 109 review lost
//! `@fw.route.prefix.within` in a Ruby pattern this way, and a Rails test caught
//! it only by chance. This file is the guard that does not depend on chance.
//!
//! * The limit is **measured**, never written down: [`measured_step_capture_limit`]
//!   matches a probe pattern carrying many captures on one node and counts what
//!   the match returns.
//! * [`over_limit_runs`] reads query source and reports every step carrying more
//!   captures than that limit, with the line and the captures dropped. Captures
//!   written after a group or an alternation stack onto its first step's own.
//! * The embedded set is `grammars::compiled()` — the exact text tree-sitter
//!   compiles, layered queries included — so a build that ships a query is a
//!   build that is checked.
//!
//! A line number is the line in the *embedded* source. For a query layered over a
//! shared one (the label names both files, `a + b`) that is the shared text
//! followed by the layer, so the failure also quotes the offending line.
#![cfg(feature = "lang-all")]

use logos_core::plugin::grammars::{self, EmbeddedQuery};
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

/// The captures tree-sitter keeps on one step, read off the pinned runtime.
///
/// A pattern with far more captures than any plausible limit sits on one named
/// node of a tiny Rust source; the match returns the captures the step kept.
fn measured_step_capture_limit() -> usize {
    const PROBE_CAPTURES: usize = 12;
    let pattern = (0..PROBE_CAPTURES).fold(String::from("(source_file)"), |p, i| {
        format!("{p} @probe{i}")
    });
    let kept = kept_captures(&pattern).len();
    // A probe that kept every capture measured nothing: the limit is above it.
    assert!(
        kept < PROBE_CAPTURES,
        "the probe's {PROBE_CAPTURES} captures were all kept; raise PROBE_CAPTURES"
    );
    kept
}

/// The language the probe and the planted cases run on: the Rust grammar, which
/// `lang-all` always links.
fn probe_language() -> Language {
    let entry = grammars::compiled()
        .into_iter()
        .find(|e| e.manifest_label.starts_with("rust/"))
        .expect("lang-all links the rust grammar");
    Language::from(entry.language)
}

/// Compile `pattern` against the probe grammar, match it on a one-line source,
/// and return the capture names the first match carries, in match order.
fn kept_captures(pattern: &str) -> Vec<String> {
    let language = probe_language();
    let query = Query::new(&language, pattern)
        .unwrap_or_else(|e| panic!("probe pattern must compile: {e}"));
    let mut parser = Parser::new();
    parser.set_language(&language).expect("probe grammar loads");
    let source = "fn probe() {}";
    let tree = parser.parse(source, None).expect("probe source parses");
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    let first = matches
        .next()
        .expect("the probe pattern matches the probe source");
    first
        .captures
        .iter()
        .map(|c| query.capture_names()[c.index as usize].to_string())
        .collect()
}

/// One node carrying more captures than tree-sitter keeps.
#[derive(Debug, PartialEq, Eq)]
struct OverLimit {
    /// 1-based line of the step's first capture.
    line: usize,
    /// Every capture on the step, in source order, `@` included.
    captures: Vec<String>,
    /// The ones past the limit — the ones tree-sitter silently drops.
    dropped: Vec<String>,
}

/// A capture and its 1-based line.
type Capture = (usize, String);

/// An open bracket, and what a capture written after it lands on.
enum Frame {
    /// `(name …)`: the node is one step; its children are steps of their own.
    Node,
    /// `( … )` over several patterns: tree-sitter puts a capture written after
    /// it on the group's *first* step, which already holds that element's own
    /// captures. Holds them once the first element has settled.
    Group(Option<Vec<Capture>>),
    /// `[ … ]`: a capture written after it lands on the first step of *every*
    /// branch. Holds the fullest branch's captures so far.
    Alternation(Vec<Capture>),
    /// `(#eq? …)`, `(#set! …)`: its `@names` are arguments, not captures.
    Predicate,
}

/// The state of one pass over a query source.
struct Scan {
    limit: usize,
    found: Vec<OverLimit>,
    frames: Vec<Frame>,
    /// The step of the pattern just written, still taking captures.
    open: Option<Vec<Capture>>,
}

impl Scan {
    fn in_predicate(&self) -> bool {
        self.frames.iter().any(|f| matches!(f, Frame::Predicate))
    }

    /// The pattern just written takes no more captures: report its step if it
    /// holds too many, and hand it to the group or alternation it sits in.
    fn settle(&mut self) {
        let Some(step) = self.open.take() else { return };
        if step.len() > self.limit {
            self.found.push(OverLimit {
                line: step[0].0,
                captures: step.iter().map(|(_, c)| c.clone()).collect(),
                dropped: step[self.limit..].iter().map(|(_, c)| c.clone()).collect(),
            });
        }
        match self.frames.last_mut() {
            Some(Frame::Group(first @ None)) => *first = Some(step),
            Some(Frame::Alternation(fullest)) if step.len() > fullest.len() => *fullest = step,
            _ => {}
        }
    }

    /// A pattern with no captures of its own yet (a string, `_`, a closed node).
    fn start(&mut self, step: Vec<Capture>) {
        if !self.in_predicate() {
            self.open = Some(step);
        }
    }

    fn capture(&mut self, capture: Capture) {
        if !self.in_predicate() {
            self.open.get_or_insert_with(Vec::new).push(capture);
        }
    }
}

/// Every step in `source` carrying more than `limit` captures.
///
/// A step's captures are the run of `@name`s written right after its pattern,
/// with only whitespace, comments and quantifiers (`?`, `*`, `+`) between them.
/// Anything else ends the run: the next pattern, a field name, an anchor. A
/// capture after a group or an alternation stacks onto the captures its first
/// step (every branch's, for an alternation) already carries, so
/// `((a) @x) @y @z @w` drops `@w` — that is counted too. Captures inside a
/// predicate or directive (`(#eq? @a @b …)`) are arguments, not captures on a
/// step, and a `@` inside a string or a comment is text.
fn over_limit_runs(source: &str, limit: usize) -> Vec<OverLimit> {
    let chars: Vec<char> = source.chars().collect();
    let mut scan = Scan {
        limit,
        found: Vec::new(),
        frames: Vec::new(),
        open: None,
    };
    let mut line = 1usize;
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            ';' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                scan.settle();
                scan.start(Vec::new());
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    match chars[i] {
                        '\\' => i += 1,
                        '\n' => line += 1,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            '(' => {
                scan.settle();
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let word_end = (j..chars.len())
                    .find(|&k| !(chars[k].is_alphanumeric() || matches!(chars[k], '_' | '-')))
                    .unwrap_or(chars.len());
                let names_a_node =
                    word_end > j && !chars[j].is_ascii_digit() && chars.get(word_end) != Some(&':');
                scan.frames
                    .push(if scan.in_predicate() || chars.get(j) == Some(&'#') {
                        Frame::Predicate
                    } else if names_a_node {
                        Frame::Node
                    } else {
                        Frame::Group(None)
                    });
                i += 1;
            }
            ')' => {
                scan.settle();
                match scan.frames.pop() {
                    Some(Frame::Node) => scan.start(Vec::new()),
                    Some(Frame::Group(first)) => scan.start(first.unwrap_or_default()),
                    Some(Frame::Alternation(fullest)) => scan.start(fullest),
                    Some(Frame::Predicate) | None => {}
                }
                i += 1;
            }
            '[' => {
                scan.settle();
                scan.frames.push(if scan.in_predicate() {
                    Frame::Predicate
                } else {
                    Frame::Alternation(Vec::new())
                });
                i += 1;
            }
            ']' => {
                scan.settle();
                match scan.frames.pop() {
                    Some(Frame::Alternation(fullest)) => scan.start(fullest),
                    Some(Frame::Group(first)) => scan.start(first.unwrap_or_default()),
                    Some(Frame::Node) | Some(Frame::Predicate) | None => {}
                }
                i += 1;
            }
            '@' => {
                let start = i;
                i += 1;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | '-'))
                {
                    i += 1;
                }
                scan.capture((line, chars[start..i].iter().collect()));
            }
            '?' | '*' | '+' => i += 1,
            c if c.is_alphanumeric() || c == '_' => {
                // A bare word: `_` is a wildcard pattern, anything else is a
                // field name or node name that only ends the run before it.
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '-'))
                {
                    i += 1;
                }
                scan.settle();
                if i - start == 1 && c == '_' {
                    scan.start(Vec::new());
                }
            }
            _ => {
                scan.settle();
                i += 1;
            }
        }
    }
    scan.settle();
    scan.found
}

/// The embedded queries of every compiled grammar, each text once. A shared
/// entry (`typescript` ← `tsx`) repeats its queries under several languages.
fn embedded_queries() -> Vec<EmbeddedQuery> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for entry in grammars::compiled() {
        for query in entry.embedded_queries {
            if seen.insert((query.label, query.source)) {
                out.push(*query);
            }
        }
    }
    out
}

/// The sweep's denominator, counted two other ways: the compiled grammars'
/// distinct queries by sorting (not by the set `embedded_queries` dedupes
/// with), and the `queries/*.scm` files on disk, each of which must be named
/// by some embedded label. A dedupe that loses queries, or a query file no
/// registry row embeds, would otherwise shrink the sweep and stay green.
fn assert_every_query_was_read(queries: &[EmbeddedQuery]) {
    let mut every: Vec<(&str, &str)> = grammars::compiled()
        .iter()
        .flat_map(|e| e.embedded_queries.iter().map(|q| (q.label, q.source)))
        .collect();
    every.sort_unstable();
    every.dedup();
    assert!(
        !every.is_empty(),
        "lang-all embeds plugin queries; none were read"
    );
    assert_eq!(
        queries.len(),
        every.len(),
        "the sweep read {} embedded queries; the compiled grammars hold {} distinct ones",
        queries.len(),
        every.len()
    );

    let plugins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
    let mut on_disk = Vec::new();
    for plugin in std::fs::read_dir(&plugins)
        .expect("read plugins/")
        .flatten()
    {
        let Ok(files) = std::fs::read_dir(plugin.path().join("queries")) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) == Some("scm") {
                on_disk.push(format!(
                    "{}/queries/{}",
                    plugin.file_name().to_string_lossy(),
                    file.file_name().to_string_lossy()
                ));
            }
        }
    }
    assert!(!on_disk.is_empty(), "no plugins/*/queries/*.scm on disk");
    let unread: Vec<&String> = on_disk
        .iter()
        .filter(|rel| !queries.iter().any(|q| q.label.contains(rel.as_str())))
        .collect();
    assert!(
        unread.is_empty(),
        "{} of {} query files on disk are in no embedded label, so the sweep never read them: {unread:?}",
        unread.len(),
        on_disk.len()
    );
}

/// One failure line per offending node: file, line, the quoted source line, and
/// the captures tree-sitter would drop.
fn describe(label: &str, source: &str, limit: usize, hit: &OverLimit) -> String {
    let text = source.lines().nth(hit.line - 1).unwrap_or("").trim();
    format!(
        "{label}:{}: {} captures on one node, tree-sitter keeps {limit}; dropped {} — `{text}`",
        hit.line,
        hit.captures.len(),
        hit.dropped.join(" "),
    )
}

// ── The measured limit ───────────────────────────────────────────────────────

#[test]
fn the_probe_measures_three_captures_kept_on_the_pinned_tree_sitter() {
    // Pinned on purpose: a tree-sitter upgrade that moves the limit must be read
    // here, with the planted cases below, before the embedded set is trusted.
    assert_eq!(
        measured_step_capture_limit(),
        3,
        "tree-sitter 0.25.10 keeps three captures per step; if the runtime moved, \
         re-read the planted cases below against the new limit"
    );
}

#[test]
fn a_match_of_the_four_capture_probe_returns_no_fourth_capture() {
    let kept = kept_captures(
        "(source_file) @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within @x",
    );
    assert_eq!(
        kept,
        [
            "fw.route.prefix.scope",
            "fw.route.prefix.nest",
            "fw.route.prefix.within"
        ],
        "the query compiles with four captures on one node, and the match drops the fourth"
    );
    // The same three without `@x` lose nothing.
    let three = kept_captures(
        "(source_file) @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within",
    );
    assert_eq!(kept, three);
}

// ── The scanner, on planted queries ──────────────────────────────────────────

#[test]
fn a_planted_four_capture_node_fails_and_names_the_dropped_capture() {
    let limit = measured_step_capture_limit();
    let planted = "(call\n  method: (identifier))\n  @fw.route.prefix.scope @fw.route.prefix.nest\n  @fw.route.prefix.within @x\n";
    let hits = over_limit_runs(planted, limit);
    assert_eq!(
        hits,
        [OverLimit {
            line: 3,
            captures: [
                "@fw.route.prefix.scope",
                "@fw.route.prefix.nest",
                "@fw.route.prefix.within",
                "@x"
            ]
            .map(String::from)
            .to_vec(),
            dropped: vec!["@x".to_string()],
        }]
    );
    assert_eq!(
        describe("ruby/queries/frameworks.scm", planted, limit, &hits[0]),
        "ruby/queries/frameworks.scm:3: 4 captures on one node, tree-sitter keeps 3; dropped @x — \
         `@fw.route.prefix.scope @fw.route.prefix.nest`"
    );
}

#[test]
fn a_planted_three_capture_node_passes() {
    let limit = measured_step_capture_limit();
    let planted = "(call method: (identifier)) @fw.route.prefix.scope @fw.route.prefix.nest @fw.route.prefix.within\n";
    assert_eq!(over_limit_runs(planted, limit), []);
}

#[test]
fn quantifiers_and_comments_between_captures_do_not_end_the_run() {
    let limit = measured_step_capture_limit();
    let planted = "(a)? @p ; note\n  * @q @r\n  @s\n";
    let hits = over_limit_runs(planted, limit);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].dropped, ["@s"]);
}

#[test]
fn captures_after_a_group_or_an_alternation_are_counted() {
    let limit = measured_step_capture_limit();
    let planted = "[(a) (b)] @p @q @r @s\n((c) (d)) @t @u @v @w @z\n";
    let hits = over_limit_runs(planted, limit);
    let dropped: Vec<_> = hits.iter().map(|h| h.dropped.clone()).collect();
    assert_eq!(
        dropped,
        [
            vec!["@s".to_string()],
            vec!["@w".to_string(), "@z".to_string()]
        ]
    );
    assert_eq!(hits.iter().map(|h| h.line).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn captures_after_a_group_or_an_alternation_stack_onto_its_first_step() {
    let limit = measured_step_capture_limit();
    // Each of these keeps `@a @b @c` and silently drops `@d` on the pinned
    // runtime: the outer captures join the ones the first step already holds.
    let stacked = [
        "((source_file) @a) @b @c @d",
        "[(source_file) @a (identifier)] @b @c @d",
        "((source_file) @a (#not-eq? @a \"y\")) @b @c @d",
        "[((source_file) @a) (identifier)] @b @c @d",
    ];
    for query in stacked {
        assert_eq!(
            kept_captures(query),
            ["a", "b", "c"],
            "the runtime drops the fourth: {query}"
        );
        let hits = over_limit_runs(query, limit);
        assert_eq!(hits.len(), 1, "{query}: {hits:?}");
        assert_eq!(hits[0].dropped, ["@d"], "{query}");
    }
}

#[test]
fn a_captured_group_that_stays_within_the_limit_passes() {
    let limit = measured_step_capture_limit();
    for query in [
        // The first element's captures plus the group's: three.
        "((source_file) @a) @b @c",
        // Only the first element's step takes the group's captures.
        "((source_file) @a (identifier) @x @y @z) @b @c",
        // A node's children are steps of their own.
        "(source_file (identifier) @a @b @c) @d @e @f",
        // The fullest branch decides, and it is within the limit.
        "[(source_file) @a (identifier) @b] @c @d",
    ] {
        assert_eq!(over_limit_runs(query, limit), [], "{query}");
    }
}

#[test]
fn a_violation_after_a_predicate_is_still_found() {
    let limit = measured_step_capture_limit();
    let planted = "(a) @p (#eq? @p \"x\")\n(b) @q @r @s @t\n";
    let hits = over_limit_runs(planted, limit);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(
        (hits[0].line, hits[0].dropped.clone()),
        (2, vec!["@t".to_string()])
    );
}

#[test]
fn a_string_with_an_escaped_quote_or_a_newline_keeps_the_scan_aligned() {
    let limit = measured_step_capture_limit();
    // An escaped quote does not end the string: the node after it is still seen.
    let escaped = "(x \"a\\\"b\") @a @b @c @d\n";
    let hits = over_limit_runs(escaped, limit);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(
        (hits[0].line, hits[0].dropped.clone()),
        (1, vec!["@d".to_string()])
    );
    // A string spanning lines still counts them: the violation is on line 3.
    let spanning = "(x \"a\nb\") @p\n(y) @a @b @c @d\n";
    let hits = over_limit_runs(spanning, limit);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].line, 3, "{hits:?}");
}

#[test]
fn a_capture_name_may_carry_dots_dashes_and_underscores() {
    let limit = measured_step_capture_limit();
    let hits = over_limit_runs("(a) @a-b @c.d @e_f @g-h.i_j\n", limit);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].dropped, ["@g-h.i_j"]);
}

#[test]
fn near_misses_are_not_counted_as_captures_on_one_node() {
    let limit = measured_step_capture_limit();
    let near_misses = [
        // Three patterns, one capture each — a pattern between every capture.
        "(a) @p (b) @q (c) @r (d) @s",
        // Captures named in a predicate or a directive are arguments.
        "(a) @p (#any-of? @p @q @r @s @t)\n(b) @u (#set! @u @v @w @x @y)",
        // Whitespace after the `(` still opens a predicate.
        "(a) @p ( #eq? @p @q @r @s @t)",
        // `@` in a string and in a comment is text.
        "(a \"@p @q @r @s\") @t ; @u @v @w @x\n",
        // A field name ends the run: the capture after it belongs to the next node.
        "(a (b) @p @q @r\n  name: (c) @s)",
        // An anonymous node with a captured sibling.
        "\"def\" @p \"end\" @q \"if\" @r \"x\" @s",
    ];
    for source in near_misses {
        assert_eq!(over_limit_runs(source, limit), [], "{source}");
    }
}

// ── The embedded set ─────────────────────────────────────────────────────────

#[test]
fn every_embedded_plugin_query_keeps_every_capture_it_declares() {
    let limit = measured_step_capture_limit();
    let queries = embedded_queries();
    assert_every_query_was_read(&queries);

    let failures: Vec<String> = queries
        .iter()
        .flat_map(|q| {
            over_limit_runs(q.source, limit)
                .into_iter()
                .map(move |hit| describe(q.label, q.source, limit, &hit))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} embedded queries put more captures on one node than tree-sitter keeps \
         ({limit}); the rest are dropped without an error:\n{}",
        failures.len(),
        queries.len(),
        failures.join("\n")
    );
}
