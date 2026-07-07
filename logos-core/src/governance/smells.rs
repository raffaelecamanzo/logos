//! On-demand test-quality smell detection ([FR-CV-08], [CR-007]).
//!
//! This is the advisory appendix the `test_gaps` report grows ([FR-GV-08] as
//! modified): three smells — **assertion-free**, **empty-body**, and
//! **sleeping** tests — detected over the test population via the plugins'
//! optional fourth `.scm` query ([`SMELL_QUERY`]).
//!
//! # Two-tier rule ([BR-28])
//!
//! Everything here is computed **on demand from the current tree** (the files
//! are re-read and re-parsed at call time) and is **advisory only**: it writes
//! nothing to the canonical store and is architecturally forbidden from moving
//! the `gate`. `test_gaps` is already off the gate path, so attaching this
//! appendix cannot change `gate` output.
//!
//! # Single source of truth for "what is a test" ([FR-AN-05])
//!
//! The smell query captures **candidate** functions broadly; each candidate is
//! then post-filtered through the canonical [`test_evidence`] classifier — the
//! same logic the persisted `is_test` annotation uses. So a `#[test]` attribute,
//! a `*_test.go` filename, a `@Test` annotation, or an enclosing `it`/`test`
//! call (a gate a tree-sitter query cannot fully express) decides what counts,
//! never the query alone.
//!
//! # `n/a`, never "clean" ([NFR-RA-05])
//!
//! A language whose plugin ships no smell query is reported in
//! [`TestSmellAppendix::not_analyzed`] — never silently treated as having no
//! smells.
//!
//! [FR-CV-08]: ../../../docs/specs/requirements/FR-CV-08.md
//! [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [CR-007]: ../../../docs/requests/CR-007-coverage-ingestion.md

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::config::Config;
use crate::extract::testmarker::test_evidence;
use crate::model::NodeKind;
use crate::models::quality::{TestSmell, TestSmellAppendix, TestSmellKind};
use crate::plugin::{LanguagePlugin, LanguageRegistry, TestConvention, SMELL_QUERY};

/// The advisory label every smell appendix carries ([FR-CV-08], [BR-28]) so a
/// reader never mistakes an advisory finding for a binding gate signal.
const SMELL_LABEL: &str =
    "test-quality smells (advisory — static AST evidence; never affects gate)";

/// Build the advisory smell appendix by re-parsing the **current tree**
/// ([FR-CV-08], [CR-007]).
///
/// The file set is the project's discovered source files (the same
/// gitignore-aware, root-contained walk indexing uses, [FR-IX-02]) — not the
/// persisted graph, so an idiomatic anonymous `it('…', () => …)` test file that
/// declares no captured symbol is still analyzed. For each file:
///
/// - its plugin ships the smell query → parse and detect smells;
/// - its plugin ships none but declares a test idiom
///   ([`TestConvention`] ≠ `None`) → record the language as `n/a`, never
///   "clean" ([NFR-RA-05], [FR-CV-08]);
/// - its plugin is a non-testable grammar (documentation; no test idiom) → it is
///   neither analyzed nor reported.
///
/// This is an on-demand diagnostic off every hot path ([BR-28]); it re-parses
/// the discovered files once per call (acceptable for `test_gaps`, never run on
/// the gate/sync/navigation paths). A discovery failure degrades to an empty
/// appendix — advisory findings are never an error.
pub(crate) fn detect_test_smells(
    root: &Path,
    registry: &LanguageRegistry,
    config: &Config,
) -> TestSmellAppendix {
    let mut findings: Vec<TestSmell> = Vec::new();
    let mut not_analyzed: BTreeSet<String> = BTreeSet::new();

    let Ok(discovery) = crate::config::discover(root, config) else {
        // Advisory: a discovery fault must not fail `test_gaps`.
        return TestSmellAppendix {
            label: SMELL_LABEL.to_string(),
            ..Default::default()
        };
    };
    // `discover` canonicalises the root and returns paths beneath it; canonicalise
    // here too so `strip_prefix` yields the repo-relative path.
    let croot = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    for abs in &discovery.files {
        let rel = abs
            .strip_prefix(&croot)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/");
        let Some(ext) = extension_of(&rel) else {
            continue;
        };
        let Some(plugin) = registry.for_extension(ext) else {
            continue; // no plugin claims this extension
        };
        match plugin.query(SMELL_QUERY) {
            Some(query) => {
                let Ok(source) = std::fs::read(abs) else {
                    continue; // unreadable file — advisory, skip
                };
                detect_in_file(plugin, query, &rel, &source, &mut findings);
            }
            None if plugin.semantics().test_convention != TestConvention::None => {
                // A language with a test idiom but no smell query → n/a.
                not_analyzed.insert(plugin.name().to_string());
            }
            None => {} // documentation / non-testable grammar — not reported
        }
    }

    // Deterministic appendix: a given tree + queries always yields the same
    // bytes (NFR-RA-06). `TestSmell`'s Ord is (file, line, name, kind).
    findings.sort();
    TestSmellAppendix {
        label: SMELL_LABEL.to_string(),
        findings,
        not_analyzed: not_analyzed.into_iter().collect(),
    }
}

/// One captured candidate test unit: the declaration node, its body block (for
/// empty-body detection), and its display name.
struct Unit<'tree> {
    node: Node<'tree>,
    body: Option<Node<'tree>>,
    name: String,
}

/// Parse `source` and append every confirmed smell in `file` to `findings`.
fn detect_in_file(
    plugin: &dyn LanguagePlugin,
    query: &Query,
    file: &str,
    source: &[u8],
    findings: &mut Vec<TestSmell>,
) {
    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return; // grammar failed to bind (ABI skew) — advisory, skip
    }
    let Some(tree) = parser.parse(source, None) else {
        return;
    };

    let capture_names = query.capture_names();
    let mut units: Vec<Unit<'_>> = Vec::new();
    // Assertion / sleep evidence as byte ranges, associated to a unit by
    // containment. Deduped by range so overlapping query patterns can't
    // double-count.
    let mut asserts: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut sleeps: BTreeSet<(usize, usize)> = BTreeSet::new();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source);
    while let Some(m) = matches.next() {
        let mut test: Option<Node<'_>> = None;
        let mut body: Option<Node<'_>> = None;
        let mut name: Option<String> = None;
        for cap in m.captures {
            match capture_names[cap.index as usize] {
                "smell.test" => test = Some(cap.node),
                "smell.body" => body = Some(cap.node),
                "smell.name" => {
                    name = cap.node.utf8_text(source).ok().map(strip_quotes);
                }
                "smell.assertion" => {
                    asserts.insert((cap.node.start_byte(), cap.node.end_byte()));
                }
                "smell.sleep" => {
                    sleeps.insert((cap.node.start_byte(), cap.node.end_byte()));
                }
                _ => {} // a predicate-helper capture (`@_a`, `@_s`, …)
            }
        }
        if let Some(node) = test {
            units.push(Unit {
                node,
                body,
                name: name.unwrap_or_default(),
            });
        }
    }

    let convention = plugin.semantics().test_convention;
    let mut seen: HashSet<usize> = HashSet::new();
    for unit in &units {
        // One candidate node is classified once even if two patterns matched it.
        if !seen.insert(unit.node.id()) {
            continue;
        }
        // Single source of truth: only candidates the canonical test-marker
        // logic confirms are tests (FR-AN-05). `test_evidence` gates on a
        // callable kind; Function is accepted for every shape we capture.
        if !test_evidence(
            unit.node,
            &unit.name,
            NodeKind::Function,
            file,
            convention,
            source,
        ) {
            continue;
        }

        let line = Some(unit.node.start_position().row as i64 + 1);
        let span = (unit.node.start_byte(), unit.node.end_byte());
        let empty = unit.body.is_some_and(|b| body_is_empty(b, source));
        let has_assertion = asserts.iter().any(|r| contained(*r, span));
        let has_sleep = sleeps.iter().any(|r| contained(*r, span));

        let mut push = |kind: TestSmellKind| {
            findings.push(TestSmell {
                file: file.to_string(),
                line,
                name: unit.name.clone(),
                kind,
            });
        };
        // Empty-body subsumes assertion-free (an empty test trivially has no
        // assertion); report the more specific smell only.
        if empty {
            push(TestSmellKind::EmptyBody);
        } else if !has_assertion {
            push(TestSmellKind::AssertionFree);
        }
        // Sleeping is orthogonal: a test can sleep while also asserting.
        if has_sleep {
            push(TestSmellKind::Sleeping);
        }
    }
}

/// `true` when `body` holds no meaningful statement — only placeholders
/// (`pass`, `...`, a lone docstring, an empty `;`) or comments. Universal
/// placeholder detection, not a per-language idiom.
fn body_is_empty(body: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = body.walk();
    // Bound to a local so the cursor's borrow ends before return (the codebase
    // idiom — see `testmarker::java_annotations`).
    let empty = body
        .named_children(&mut cursor)
        .all(|child| is_trivial(child, source));
    empty
}

/// `true` for a node that carries no test logic: a comment, a `pass`, a bare
/// `...`/`;`, or an expression statement wrapping only a string (docstring) or
/// an ellipsis.
fn is_trivial(node: Node<'_>, source: &[u8]) -> bool {
    let kind = node.kind();
    if kind.contains("comment") {
        return true;
    }
    match kind {
        "pass_statement" | "empty_statement" | "ellipsis" => true,
        "expression_statement" => {
            let mut cursor = node.walk();
            let kids: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
            !kids.is_empty()
                && kids
                    .iter()
                    .all(|k| matches!(k.kind(), "string" | "ellipsis"))
        }
        _ => {
            // Defensive: a whitespace-only/other unnamed node is trivial; a
            // node whose own text is empty carries nothing.
            node.utf8_text(source).is_ok_and(str::is_empty)
        }
    }
}

/// `true` when the `inner` byte range is contained within `outer`.
const fn contained(inner: (usize, usize), outer: (usize, usize)) -> bool {
    inner.0 >= outer.0 && inner.1 <= outer.1
}

/// Strip one layer of surrounding quotes from a captured name (the `it`/`test`
/// description string arrives as `'desc'`/`"desc"`/`` `desc` ``); leave bare
/// identifiers untouched.
fn strip_quotes(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if first == last && matches!(first, b'\'' | b'"' | b'`') {
            return text[1..text.len() - 1].to_string();
        }
    }
    text.to_string()
}

/// The extension of a path, without the dot; `None` when there is none. Case is
/// preserved here — [`LanguageRegistry::for_extension`] lowercases at lookup. A
/// leading-dot file (`.gitignore`) has no extension (matching `std::path`),
/// never a stem-less `"gitignore"`.
fn extension_of(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some(("", _)) => None, // a dotfile (`.gitignore`) is not an extension
        Some((_, ext)) => Some(ext),
        None => None,
    }
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;
    use crate::plugin::grammars::{EmbeddedQuery, GrammarEntry};
    use crate::plugin::AbiRange;

    /// FR-CV-08 / NFR-RA-05 `n/a` posture: a language whose plugin declares a
    /// test idiom but ships **no** smell query is reported as `not_analyzed`,
    /// never silently "clean". Built from a data-only grammar (no smell query)
    /// claiming `.noq`, so no real plugin needs its query removed.
    #[test]
    fn a_query_less_test_language_reports_na_never_clean() {
        const NOQ: &str = r#"
            name = "noqlang"
            extensions = ["noq"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            test_convention = "rust-attributes"
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let entry = GrammarEntry {
            manifest_label: "noqlang/plugin.toml",
            manifest_toml: NOQ,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "noqlang/queries/symbols.scm",
                source: "(function_item name: (identifier) @symbol.function)",
            }],
        };
        let entries = vec![entry];
        let registry =
            LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
                .expect("the data-only grammar loads");

        let tmp = tempfile::tempdir().unwrap();
        // A file of the query-less language must exist for discovery to find it.
        std::fs::write(tmp.path().join("a.noq"), "#[test]\nfn t() {}\n").unwrap();

        let appendix = detect_test_smells(tmp.path(), &registry, &Config::default());

        assert_eq!(
            appendix.not_analyzed,
            vec!["noqlang".to_string()],
            "a test-capable language without a smell query is n/a"
        );
        assert!(
            appendix.findings.is_empty(),
            "nothing is analyzed for the query-less language"
        );
        assert!(
            appendix.label.contains("advisory"),
            "the appendix always carries the advisory label"
        );
    }

    #[test]
    fn trivial_bodies_are_detected_as_empty() {
        // Universal placeholder detection (no per-language idiom): the helpers
        // are exercised directly so the empty-body rule is pinned.
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let src = b"fn empty() {}\nfn full() { let x = 1; }\n";
        let tree = parser.parse(&src[..], None).unwrap();
        let root = tree.root_node();
        let mut cursor = root.walk();
        let fns: Vec<_> = root
            .named_children(&mut cursor)
            .filter(|n| n.kind() == "function_item")
            .collect();
        let body0 = fns[0].child_by_field_name("body").unwrap();
        let body1 = fns[1].child_by_field_name("body").unwrap();
        assert!(body_is_empty(body0, &src[..]), "{{}} is empty");
        assert!(
            !body_is_empty(body1, &src[..]),
            "a body with a statement is not empty"
        );
    }
}
