//! A historical security finding's code scope ([FR-SC-28] as amended by
//! [CR-265], S-843): the scope of the code it was found in, not of its path.
//!
//! A historical finding is a secret found only in history (`historical`,
//! anchored to its commit, S-767): its file is gone, or its line holds other
//! text now, so no node of today's graph encloses it at its reported line. Its
//! anchor commit's version of the file still holds the line, and so decides,
//! in this order:
//!
//! 1. **Re-anchored.** The lines the finding spans — its start line, or every
//!    line to its end line for a multi-line secret such as a PEM key, whose
//!    `BEGIN` line every key shares — are looked up in the file's current
//!    version, as one consecutive run, each line compared exactly after
//!    trimming whitespace. When every match lies in a code node of today's
//!    graph and all of those nodes are of one scope, the finding takes that
//!    scope.
//! 2. **At its own commit.** When the file or the line is gone today, or the
//!    matches disagree, or one lies in no node, the anchor commit's version of
//!    the file is extracted in memory by its language plugin — nothing is
//!    written to the store — and the node enclosing the line there decides,
//!    by the very rule the graph's `is_test` is computed with
//!    ([`is_test_declaration`]).
//! 3. **Otherwise** — the commit's file cannot be read, no plugin reads its
//!    language, its parse is damaged, no node encloses the line, or the
//!    project's configuration does not load, so no test markers can be read
//!    (a warning says so) — this
//!    module answers nothing, and the path rule decides
//!    ([`super::scope`]).
//!
//! A node of production code decides too: a key committed in production code
//! and then deleted stays `production` (labelled historical by its binding),
//! where the path rule would have said the same. The answer is the graph's
//! half of the scope only; the path rule still reads a production node's
//! file, exactly as it does for a bound finding.
//!
//! # Secrets stay in memory ([FR-SC-03])
//! The compared line holds the secret. It is read from git in memory, compared
//! and dropped with this value: nothing here stores, logs or returns it, and
//! the answer is one `bool` per finding.
//!
//! [FR-SC-03]: ../../../docs/specs/requirements/FR-SC-03.md
//! [FR-SC-28]: ../../../docs/specs/requirements/FR-SC-28.md
//! [CR-265]: ../../../docs/requests/CR-265-security-findings-without-fixture-and-audit-noise.md

use std::collections::HashMap;
use std::path::Path;

use crate::annotate::is_test_declaration;
use crate::extract::{extract, FileInput, SymbolContext};
use crate::graph_store::NodeRow;
use crate::history::security::{commit_source, project_prefix, project_relative, tree_source};
use crate::history::{FindingBinding, SecurityFinding};
use crate::model::NodeId;
use crate::plugin::LanguageRegistry;

/// Where one historical finding's line is, at its anchor commit and today.
struct Anchored {
    /// Its anchor commit and its path there, relative to the git root: the
    /// key of its file's version in [`Reanchoring::sources`].
    version: (String, String),
    /// The same file spelled relative to the project — today's file, and the
    /// path its plugin and the test path rule read.
    project_path: String,
    /// Its 1-based start line at the anchor commit.
    line: u32,
    /// The lines of today's file where the anchor commit's run of lines
    /// starts again, each line's exact trimmed text; empty when the file or
    /// the run is gone.
    today: Vec<u32>,
}

/// The historical findings of one read, re-anchored ([module docs](self)).
pub(super) struct Reanchoring {
    /// One entry per finding read, in order; `None` for a finding that is not
    /// historical, or whose anchor commit's line cannot be read.
    anchored: Vec<Option<Anchored>>,
    /// Each anchor commit's version of a file, read once — `None` when git
    /// could not read it, so a version is never asked for twice; held in
    /// memory for [`AtItsCommit::extract`] and dropped with this value.
    sources: HashMap<(String, String), Option<Vec<u8>>>,
    /// Each version's extraction, made once whichever of its lines is asked;
    /// `None` when it decides nothing ([`AtItsCommit::extract`]).
    extracted: HashMap<(String, String), Option<AtItsCommit>>,
}

impl Reanchoring {
    /// Read every historical finding's line at its anchor commit and look it
    /// up in today's file under `root`. Reads git only when a finding is
    /// historical.
    pub(super) fn read(root: &Path, findings: &[&SecurityFinding]) -> Self {
        let mut this = Self { anchored: Vec::with_capacity(findings.len()), sources: HashMap::new(), extracted: HashMap::new() };
        let mut prefix: Option<String> = None;
        let mut today: HashMap<String, Option<Vec<u8>>> = HashMap::new();
        for finding in findings {
            let anchored = anchor_of(finding).and_then(|(commit, git_path, line, end)| {
                let prefix = prefix.get_or_insert_with(|| project_prefix(root).unwrap_or_default());
                let version = (commit.to_string(), git_path.to_string());
                let source = this.sources.entry(version.clone()).or_insert_with(|| commit_source(root, commit, git_path));
                let run = region_lines(source.as_deref()?, line, end)?;
                let project_path = project_relative(root, prefix, git_path);
                let current = today.entry(project_path.clone()).or_insert_with(|| tree_source(root, &project_path));
                let today = current.as_deref().map(|current| matching_runs(current, &run)).unwrap_or_default();
                Some(Anchored { version, project_path, line, today })
            });
            this.anchored.push(anchored);
        }
        this
    }

    /// Whether no finding read is historical with a readable anchor line.
    pub(super) fn is_empty(&self) -> bool {
        self.anchored.iter().all(Option::is_none)
    }

    /// Today's files a re-anchored line was found in: what the graph's span
    /// read must cover.
    pub(super) fn today_paths(&self) -> impl Iterator<Item = &str> {
        self.anchored.iter().flatten().filter(|a| !a.today.is_empty()).map(|a| a.project_path.as_str())
    }

    /// Whether finding `i` sits in test code ([module docs](self)): `Some`
    /// when a node decides, `None` when it is not historical or neither rule
    /// decides, so the path rule does. `today` is the graph's answer for one
    /// line of one of [`today_paths`](Self::today_paths): `Some(is_test)` of
    /// the code node enclosing it, `None` when no code node does.
    /// `test_markers` is `[semantics].test_markers`; `None` — the
    /// configuration does not load — skips the anchor commit's read.
    pub(super) fn in_test(
        &mut self,
        i: usize,
        today: impl Fn(&str, u32) -> Option<bool>,
        registry: Option<&LanguageRegistry>,
        test_markers: Option<&[String]>,
    ) -> Option<bool> {
        let anchored = self.anchored.get(i)?.as_ref()?;
        if let Some(scope) = one_scope(anchored.today.iter().map(|&line| today(&anchored.project_path, line))) {
            return Some(scope);
        }
        let sources = &self.sources;
        let extracted = self.extracted.entry(anchored.version.clone()).or_insert_with(|| {
            let source = sources.get(&anchored.version)?.as_deref()?;
            AtItsCommit::extract(source, &anchored.project_path, registry?, test_markers?)
        });
        extracted.as_ref()?.in_test(anchored.line)
    }
}

/// A historical finding's anchor commit, its path there and its 1-based start
/// and end lines — the end its start when the report gave none, or one before
/// it; `None` for any other finding, or one whose report named no commit or
/// line.
fn anchor_of(finding: &SecurityFinding) -> Option<(&str, &str, u32, u32)> {
    if finding.binding != FindingBinding::Historical {
        return None;
    }
    let anchor = finding.anchor.as_ref()?;
    let line = finding.start_line.and_then(|l| u32::try_from(l).ok()).filter(|&l| l > 0)?;
    let end = finding.end_line.and_then(|l| u32::try_from(l).ok()).filter(|&l| l >= line).unwrap_or(line);
    Some((anchor.commit.as_str(), anchor.path.as_str(), line, end))
}

/// The scope every match agrees on: `Some` when there is at least one match,
/// every one lies in a code node, and all those nodes are of one scope.
fn one_scope(matches: impl IntoIterator<Item = Option<bool>>) -> Option<bool> {
    let mut agreed = None;
    for scope in matches {
        let scope = scope?;
        if agreed.is_some_and(|agreed| agreed != scope) {
            return None;
        }
        agreed = Some(scope);
    }
    agreed
}

/// Lines `start..=end` (1-based) of `source`, each trimmed; `None` when the
/// run passes the end of the text, or when its first line holds nothing but
/// whitespace — an empty line matches anything and so proves nothing.
fn region_lines(source: &[u8], start: u32, end: u32) -> Option<Vec<&[u8]>> {
    let (start, end) = (usize::try_from(start).ok()?, usize::try_from(end).ok()?);
    let run: Vec<&[u8]> = source.split(|&b| b == b'\n').skip(start - 1).take(end - start + 1).map(<[u8]>::trim_ascii).collect();
    (run.len() == end - start + 1 && !run[0].is_empty()).then_some(run)
}

/// The 1-based lines of `source` where `run` starts: the consecutive lines
/// whose trimmed text is exactly `run`'s, line for line.
fn matching_runs(source: &[u8], run: &[&[u8]]) -> Vec<u32> {
    let lines: Vec<&[u8]> = source.split(|&b| b == b'\n').map(<[u8]>::trim_ascii).collect();
    lines.windows(run.len()).zip(1u32..).filter(|(window, _)| *window == run).map(|(_, number)| number).collect()
}

/// A file's version at its anchor commit, extracted in memory by its plugin:
/// its nodes, each with whether it is test code by the rule the graph's
/// `is_test` is computed with. A row's `id` is its index here, never a store
/// id.
struct AtItsCommit {
    rows: Vec<NodeRow>,
    is_test: Vec<bool>,
}

impl AtItsCommit {
    /// `source` extracted as `path`; `None` when no plugin reads `path`, the
    /// text is not UTF-8, or its parse is damaged.
    fn extract(source: &[u8], path: &str, registry: &LanguageRegistry, test_markers: &[String]) -> Option<Self> {
        let plugin = registry.for_path(path)?;
        let text = std::str::from_utf8(source).ok()?;
        let facts = extract(&FileInput::new(path, text), plugin, &SymbolContext::default());
        if facts.parse_damaged {
            return None;
        }
        let is_test = facts
            .nodes
            .iter()
            .map(|node| is_test_declaration(node.test_evidence, Some(path), node.kind, &node.name, test_markers))
            .collect();
        let rows = facts
            .nodes
            .into_iter()
            .zip(0i64..)
            .map(|(node, i)| NodeRow {
                id: NodeId(i),
                symbol: node.symbol,
                kind: node.kind,
                name: node.name,
                file_path: Some(path.to_string()),
                start_line: Some(i64::from(node.start_line)),
                end_line: Some(i64::from(node.end_line)),
            })
            .collect();
        Some(Self { rows, is_test })
    }

    /// Whether the code node enclosing `line` is test code; `None` when no
    /// code node encloses it.
    fn in_test(&self, line: u32) -> Option<bool> {
        let row = super::context::enclosing_code(&self.rows.iter().collect::<Vec<_>>(), line)?;
        self.is_test.get(usize::try_from(row.id.0).ok()?).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every match must lie in a node, and every node must be of one scope;
    /// no match decides nothing.
    #[test]
    fn the_matches_decide_only_when_every_one_lies_in_a_node_of_one_scope() {
        assert_eq!(one_scope([Some(true), Some(true)]), Some(true));
        assert_eq!(one_scope([Some(false)]), Some(false));
        assert_eq!(one_scope([Some(true), Some(false)]), None, "two scopes");
        assert_eq!(one_scope([Some(true), None]), None, "a match in no node");
        assert_eq!(one_scope(std::iter::empty()), None, "the line is gone");
    }

    /// A line is compared exactly after trimming, CRLF included; a blank line
    /// or one past the end is no anchor.
    #[test]
    fn a_line_matches_exactly_after_trimming_and_a_blank_one_is_no_anchor() {
        let source = b"fn a() {\n    let k = \"X\";\r\n}\n\tlet k = \"X\";\nlet k = \"X\"; // no\n\n";
        let run = region_lines(source, 2, 2).unwrap();
        assert_eq!(run, [&b"let k = \"X\";"[..]]);
        assert_eq!(matching_runs(source, &run), [2, 4], "a trailing comment is another line");
        assert_eq!(region_lines(source, 6, 6), None, "a blank line");
        assert_eq!(region_lines(source, 99, 99), None, "past the end");
        assert_eq!(region_lines(source, 5, 99), None, "a run past the end");
    }

    /// A file's version at its commit is judged by the graph's own `is_test`
    /// rule: test evidence, a test path, or a test marker in the node's name
    /// (with the project's markers, never others). Text no plugin reads, text
    /// that is not UTF-8 and a damaged parse decide nothing.
    #[test]
    fn a_commits_version_is_judged_by_the_is_test_rule_and_an_unreadable_one_decides_nothing() {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let markers = ["test".to_string(), "tests".to_string(), "spec".to_string()];
        let source = b"pub fn key() {\n    let k = 1;\n}\n\n#[test]\nfn masks() {\n    let k = 2;\n}\n\nfn helper_test() {\n    let k = 3;\n}\n";
        let at = |path: &str, markers: &[String]| AtItsCommit::extract(source, path, &registry, markers).expect("a Rust file extracts");
        let read = at("src/lib.rs", &markers);
        assert_eq!([2, 7, 11].map(|line| read.in_test(line)), [Some(false), Some(true), Some(true)], "production, #[test], a marker");
        assert_eq!(at("src/lib.rs", &[]).in_test(11), Some(false), "a marker the project does not name is none");
        assert_eq!(at("tests/helpers.rs", &markers).in_test(2), Some(true), "a test path");

        assert!(AtItsCommit::extract(source, "deploy/creds.cfg", &registry, &markers).is_none(), "no plugin");
        let not_utf8 = b"fn a() {\n    let k = \"\xff\";\n}\n";
        assert!(AtItsCommit::extract(not_utf8, "src/lib.rs", &registry, &markers).is_none(), "not UTF-8");
        let damaged = b"#[test]\nfn masks() {\n    let k = 2;\n    if {\n";
        assert!(AtItsCommit::extract(damaged, "src/lib.rs", &registry, &markers).is_none(), "a damaged parse");
    }

    /// A multi-line secret matches only where every one of its lines follows
    /// in order: two PEM keys share their `BEGIN` and `END` lines, never their
    /// body.
    #[test]
    fn a_multi_line_secret_matches_only_where_its_whole_run_does() {
        let old = b"x\n-----BEGIN KEY-----\nAAAA\n-----END KEY-----\n";
        let today = b"-----BEGIN KEY-----\nBBBB\n-----END KEY-----\n\n  -----BEGIN KEY-----\n  AAAA\n  -----END KEY-----\n";
        let run = region_lines(old, 2, 4).unwrap();
        assert_eq!(matching_runs(today, &run), [5], "the other key's header is not this key");
        assert_eq!(matching_runs(today, &region_lines(old, 2, 2).unwrap()), [1, 5], "the header alone matches both");
    }

}
