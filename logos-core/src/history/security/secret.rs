//! The keyed digest of a Gitleaks secret — what a Gitleaks finding's identity
//! folds in instead of the report's per-commit fingerprint ([FR-SC-02] as
//! amended by [CR-238] Amendment 1, [FR-SC-03], [ADR-79]).
//!
//! # Where the secret comes from
//! The registry runs Gitleaks with `--redact`, so its report holds no secret:
//! the snippet reads `REDACTED`, and the snippet is never read anyway
//! ([`super::parse`]). The matched text is read from the source instead — from
//! the working tree for a scoped (`--no-git`) result, from the blob at the
//! result's commit for a history-mode one — at the region the report gives,
//! digested in memory and dropped. Neither it nor its digest is stored: the
//! stored fingerprint is a hash over the rule, the path and this digest.
//!
//! # The region Gitleaks 8.18.4 reports
//! Its columns are byte offsets into the line, but a line that is not the
//! first of the text Gitleaks matched in reads one column late at both ends.
//! That text is the whole file in a scoped run, and the lines a commit added
//! (one `-U0` hunk) in history mode, so one secret reads `1..41` in one mode
//! and `2..42` in the other (recorded in
//! `tests/fixtures/sarif/gitleaks-8.18.4/`). Which reading applies is
//! decided, never guessed ([`SecretDigests::digest`]): late on any line but
//! the file's first in a scoped run, and on any line but its hunk's first in
//! the commit's own `-U0` diff in history mode. [`match_text`] then slices the
//! exact match, so the digest is the same in both modes, at every commit and
//! on any line, whatever bytes stand next to the match (`MY_API_KEY`,
//! `"…".strip()`, recorded in `gitleaks-8.18.4/neighbours/`). A history
//! result on a line no hunk of its commit added has no known reading: it reads
//! nothing.
//!
//! # The key
//! The digest is a keyed blake3 hash. The key is derived from the
//! repository's first commit — the root its first-parent history reaches — so
//! every clone of one repository keys a secret alike: a committed ignore entry
//! ([FR-SC-11]) and a checked-in baseline ([FR-SC-25]) name the same finding on
//! every machine and in CI, and no stored value is an unkeyed hash of a secret.
//! A shallow clone's oldest commit is not the repository's first, so it keys
//! differently: Gitleaks' history mode needs the full history in any case.
//!
//! [FR-SC-02]: ../../../../docs/specs/requirements/FR-SC-02.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
//! [FR-SC-25]: ../../../../docs/specs/requirements/FR-SC-25.md
//! [CR-238]: ../../../../docs/requests/CR-238-sprint-106-review-follow-ups.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use super::Region;

/// The blake3 key-derivation context of the repository key. A new context is a
/// new identity for every Gitleaks finding.
pub(super) const KEY_CONTEXT: &str = "logos 2026-10-18 gitleaks secret digest v1";

/// The hunks a commit's `-U0` diff adds, per new path, as `(first line, line
/// count)` in the new file.
type AddedHunks = HashMap<String, Vec<(usize, usize)>>;

/// Reads and digests Gitleaks secrets for one ingest, reading each source file
/// or blob once.
pub(crate) struct SecretDigests {
    root: PathBuf,
    key: [u8; 32],
    /// Source text by `(commit, path)`; `None` commit for the working tree.
    /// `None` text when it could not be read.
    sources: HashMap<(Option<String>, String), Option<Vec<u8>>>,
    /// Per commit, the hunks its `-U0` diff adds; `None` when the diff could
    /// not be read.
    hunks: HashMap<String, Option<AddedHunks>>,
}

impl SecretDigests {
    /// The digests for the repository at `root`; `None` when it has no first
    /// commit to key from.
    pub(crate) fn open(root: &Path) -> Option<Self> {
        Some(Self { root: root.to_path_buf(), key: repository_key(root)?, sources: HashMap::new(), hunks: HashMap::new() })
    }

    /// The keyed digest of the secret a result reports at `region` of `path` —
    /// in `commit` for a history-mode result, in the working tree otherwise.
    /// `None` when that text cannot be read: the commit is not in this clone,
    /// the file or the line is gone, the path leaves the project, the region
    /// is incomplete, or no hunk of the commit added its first line.
    pub(crate) fn digest(&mut self, commit: Option<&str>, path: &str, region: &Region) -> Option<[u8; 32]> {
        let text = self.matched(commit, path, region)?;
        Some(*blake3::keyed_hash(&self.key, &text).as_bytes())
    }

    /// Whether the secret a history-mode result reports at `region` of `path`
    /// in `commit` is still on its line ([S-767]): the working tree's
    /// `tree_path` holds the exact text the commit matched there, on the same
    /// line or lines — anywhere along them, so an edit elsewhere on the line
    /// keeps it. `None` when the commit's text cannot be read ([`digest`]
    /// reads nothing either); `Some(false)` when the file is gone or its line
    /// holds other text now.
    ///
    /// [S-767]: ../../../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
    /// [`digest`]: Self::digest
    pub(crate) fn still_on_its_line(&mut self, commit: &str, path: &str, tree_path: &str, region: &Region) -> Option<bool> {
        let text = self.matched(Some(commit), path, region)?;
        Some(self.source(None, tree_path).is_some_and(|tree| lines_hold(tree, region, &text)))
    }

    /// The text of `path` in `commit`, or in the working tree when `commit`
    /// is `None`, read once per ingest; `None` when it cannot be read or the
    /// path could leave the project.
    fn source(&mut self, commit: Option<&str>, path: &str) -> Option<&[u8]> {
        if !is_project_relative(path) {
            return None;
        }
        let root = &self.root;
        self.sources
            .entry((commit.map(str::to_string), path.to_string()))
            .or_insert_with(|| read_source(root, commit, path))
            .as_deref()
    }

    /// The exact text a result matched — [`digest`](Self::digest)'s input,
    /// under the column reading its mode and line decide.
    fn matched(&mut self, commit: Option<&str>, path: &str, region: &Region) -> Option<Vec<u8>> {
        if commit.is_some_and(|c| c.is_empty() || !c.bytes().all(|b| b.is_ascii_hexdigit())) {
            return None;
        }
        let start_line = region.start_line.and_then(|l| usize::try_from(l).ok())?;
        let late = match commit {
            // A scoped run matched the whole file: only line 1 is its first.
            None => start_line > 1,
            Some(commit) => {
                let root = &self.root;
                let hunks = self.hunks.entry(commit.to_string()).or_insert_with(|| added_hunks(root, commit)).as_ref()?;
                let &(first, _) = hunks
                    .get(path)?
                    .iter()
                    .find(|&&(first, count)| first <= start_line && start_line < first + count)?;
                start_line > first
            }
        };
        match_text(self.source(commit, path)?, region, late)
    }
}

/// Whether the lines `region` spans in `source` hold `text` anywhere along
/// them. A region past the end of the text holds nothing.
fn lines_hold(source: &[u8], region: &Region, text: &[u8]) -> bool {
    // Line by line without a line's `\r`: a checkout may end its lines in CRLF
    // where the commit's blob ends them in LF, or the other way round.
    let text: Vec<u8> = text.split(|&b| b == b'\n').map(without_cr).collect::<Vec<_>>().join(&b'\n');
    spanned_lines(source, region).is_some_and(|(lines, start, end)| {
        let window = lines[start - 1..end].iter().map(|line| without_cr(line)).collect::<Vec<_>>().join(&b'\n');
        !text.is_empty() && window.windows(text.len()).any(|window| window == text.as_slice())
    })
}

/// A line without its CRLF ending's `\r`.
fn without_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// A 1-based line or column a report gives; `None` when it gives none, or
/// one below 1.
fn positive(v: Option<i64>) -> Option<usize> {
    v.and_then(|v| usize::try_from(v).ok()).filter(|&v| v >= 1)
}

/// `source`'s lines and the 1-based first and last line `region` spans;
/// `None` when the region names no lines, is inverted or runs past the text.
fn spanned_lines<'a>(source: &'a [u8], region: &Region) -> Option<(Vec<&'a [u8]>, usize, usize)> {
    let (start, end) = (positive(region.start_line)?, positive(region.end_line)?);
    let lines: Vec<&[u8]> = source.split(|&b| b == b'\n').collect();
    (start <= end && end <= lines.len()).then_some((lines, start, end))
}

/// The hunks `commit`'s `-U0` diff adds, per new path, as `(first line, line
/// count)` — the fragments Gitleaks' history mode matches in, read with the
/// same porcelain defaults its `git log -p -U0` uses. `None` when git fails.
fn added_hunks(root: &Path, commit: &str) -> Option<AddedHunks> {
    let out = crate::history::miner::git(
        root,
        &["show", "-U0", "--format=", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", commit],
    )
    .ok()?;
    out.status.success().then(|| parse_added_hunks(&out.stdout))
}

/// [`added_hunks`]' parser over a unified diff: each `+++ b/<path>` names the
/// file the following `@@ … +first[,count] @@` headers add lines to. A
/// deleted file (`+++ /dev/null`) and a combined (merge) diff's `@@@` headers
/// add nothing; a hunk adding no line (`+n,0`) is no fragment.
fn parse_added_hunks(diff: &[u8]) -> AddedHunks {
    let mut hunks = AddedHunks::new();
    let mut path: Option<String> = None;
    for line in diff.split(|&b| b == b'\n') {
        if let Some(target) = line.strip_prefix(b"+++ ") {
            let target = String::from_utf8_lossy(target);
            let target = target.trim_end_matches('\r');
            let target = target.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(target);
            path = target.strip_prefix("b/").map(str::to_string);
        } else if line.starts_with(b"@@ ") {
            let header = String::from_utf8_lossy(line);
            let added = header.split(' ').find_map(|part| part.strip_prefix('+'));
            let parsed = added.and_then(|added| {
                let mut parts = added.splitn(2, ',');
                let first = parts.next()?.parse::<usize>().ok()?;
                let count = parts.next().map_or(Some(1), |c| c.parse::<usize>().ok())?;
                Some((first, count))
            });
            if let (Some(path), Some((first, count))) = (&path, parsed) {
                if count > 0 {
                    hunks.entry(path.clone()).or_default().push((first, count));
                }
            }
        }
    }
    hunks
}

/// The repository key: derived from the root commit the first-parent history
/// of HEAD reaches. `None` outside a repository with commits.
fn repository_key(root: &Path) -> Option<[u8; 32]> {
    let out = crate::history::miner::git(root, &["rev-list", "--first-parent", "--max-parents=0", "HEAD"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(blake3::derive_key(KEY_CONTEXT, first.as_bytes()))
}

/// `path` names a file inside the project: relative, no `..`, no root or drive.
fn is_project_relative(path: &str) -> bool {
    !path.is_empty() && Path::new(path).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// The text of `path` in `commit`, or in the working tree when `commit` is
/// `None`.
fn read_source(root: &Path, commit: Option<&str>, path: &str) -> Option<Vec<u8>> {
    match commit {
        None => std::fs::read(root.join(path)).ok(),
        Some(commit) => {
            let out = crate::history::miner::git(root, &["cat-file", "blob", &format!("{commit}:{path}")]).ok()?;
            out.status.success().then_some(out.stdout)
        }
    }
}

/// The text Gitleaks matched at `region` of `source`, exactly: the reported
/// columns read as given, or — `late` — one column late at both ends of the
/// first line, as the [module docs](self) describe. A match spanning lines
/// ends on a line that is never its text's first, so its end column is always
/// late. `None` when the region is incomplete or does not fit the text.
pub(crate) fn match_text(source: &[u8], region: &Region, late: bool) -> Option<Vec<u8>> {
    let (start_column, end_column) = (positive(region.start_column)?, positive(region.end_column)?);
    let (lines, start_line, end_line) = spanned_lines(source, region)?;
    let first = lines[start_line - 1];
    let last = lines[end_line - 1];
    let from = start_column.checked_sub(if late { 2 } else { 1 })?;
    let to = if late || start_line < end_line { end_column - 1 } else { end_column };
    if start_line == end_line {
        return (from < to).then(|| first.get(from..to).map(<[u8]>::to_vec)).flatten();
    }
    let mut text = first.get(from..)?.to_vec();
    for line in &lines[start_line..end_line - 1] {
        text.push(b'\n');
        text.extend_from_slice(line);
    }
    text.push(b'\n');
    text.extend_from_slice(last.get(..to)?);
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(line: i64, start: i64, end: i64) -> Region {
        Region { start_line: Some(line), start_column: Some(start), end_line: Some(line), end_column: Some(end) }
    }

    fn text(source: &str, region: &Region, late: bool) -> Option<String> {
        match_text(source.as_bytes(), region, late).map(|t| String::from_utf8(t).unwrap())
    }

    const KEY_LINE: &str = r#"API_KEY = "lgs_pl4nted_S3cr3t_q7Vx9Rz2Kw""#;

    /// The two readings Gitleaks 8.18.4 gives one match — `1..41` on the first
    /// line of what it matched, `2..42` one column late on any later one (the
    /// recorded fixtures) — read the same exact text, whatever stands next to
    /// it.
    #[test]
    fn both_column_readings_of_one_match_read_the_same_exact_text() {
        let expected = Some(KEY_LINE.to_string());
        let on_line_3 = format!("import os\n\n{KEY_LINE}\nX_1 = 1\n");
        assert_eq!(text(&on_line_3, &region(3, 1, 41), false), expected, "the first line matched");
        assert_eq!(text(&on_line_3, &region(3, 2, 42), true), expected, "a later line, one column late");
        let commented = format!("import os\n\n{KEY_LINE}  # rotate me\n");
        assert_eq!(text(&commented, &region(3, 1, 41), false), expected);
        assert_eq!(text(&commented, &region(3, 2, 42), true), expected);
        assert_eq!(text(&format!("{KEY_LINE}\n"), &region(1, 1, 41), false), expected, "line 1, column 1");
    }

    /// The bytes next to a match are never part of it: a word character
    /// before (`MY_`, where Gitleaks' generic rule starts at the keyword) and a
    /// `.` after (`"…".strip()`) — the cases a delimiter trim got wrong.
    #[test]
    fn a_match_glued_to_its_neighbours_reads_the_same_under_both_readings() {
        let prefixed = format!("import os\n\nMY_{KEY_LINE}.strip()\n");
        let expected = Some(KEY_LINE.to_string());
        assert_eq!(text(&prefixed, &region(3, 4, 44), false), expected, "the hunk's first line");
        assert_eq!(text(&prefixed, &region(3, 5, 45), true), expected, "a later line of the file");
        assert_eq!(text(&prefixed, &region(3, 4, 44), true), Some(format!("_{}", &KEY_LINE[..KEY_LINE.len() - 1])), "the wrong reading is another text");
    }

    /// A multi-line match (a private key) reads its lines whole; its end line
    /// is never the first of the matched text, so its end column is late.
    #[test]
    fn a_multi_line_match_reads_every_line_between() {
        let source = "k = \"\"\"-----BEGIN KEY-----\nAAAA\n-----END KEY-----\"\"\"\n";
        let late = Region { start_line: Some(1), start_column: Some(9), end_line: Some(3), end_column: Some(18) };
        let first = Region { start_column: Some(8), ..late };
        let expected = Some("-----BEGIN KEY-----\nAAAA\n-----END KEY-----".to_string());
        assert_eq!(text(source, &late, true), expected);
        assert_eq!(text(source, &first, false), expected);
    }

    /// A region the text cannot hold reads nothing, never a guess: a line past
    /// the end, an incomplete, inverted or empty region, columns past the line.
    #[test]
    fn a_region_outside_the_text_reads_nothing() {
        let source = format!("{KEY_LINE}\n");
        assert_eq!(text(&source, &region(5, 1, 41), false), None, "past the last line");
        assert_eq!(text(&source, &region(3, 1, 41), false), None, "one line past the empty one after the newline");
        assert_eq!(text(&source, &Region { end_column: None, ..region(1, 1, 41) }, false), None);
        assert_eq!(text(&source, &region(0, 1, 41), false), None, "lines are 1-based");
        let inverted = Region { start_line: Some(2), end_line: Some(1), ..region(1, 1, 41) };
        assert_eq!(text(&source, &inverted, false), None);
        assert_eq!(text(&source, &region(1, 1, 41), true), None, "column 1 cannot be read late");
        assert_eq!(text(&source, &region(1, 5, 4), false), None, "an empty span");
        assert_eq!(text(&source, &region(1, 30, 60), false), None, "columns past the line's end");
    }

    /// A match is still on its line where that line holds its exact text,
    /// anywhere along it; another line, a line past the end and a text one
    /// byte off are not. A multi-line match needs every line it spans.
    #[test]
    fn a_match_is_on_its_line_only_where_the_line_holds_its_exact_text() {
        let text = KEY_LINE.as_bytes();
        let source = format!("import os\n    {KEY_LINE}  # note\n");
        assert!(lines_hold(source.as_bytes(), &region(2, 1, 41), text), "indented, with a comment after");
        assert!(!lines_hold(source.as_bytes(), &region(1, 1, 41), text), "another line");
        assert!(!lines_hold(source.as_bytes(), &region(4, 1, 41), text), "past the end");
        let off = KEY_LINE.replace("Kw\"", "Kx\"");
        assert_ne!(off, KEY_LINE);
        assert!(!lines_hold(format!("import os\n{off}\n").as_bytes(), &region(2, 1, 41), text), "one byte off");
        let key = b"-----BEGIN KEY-----\nAAAA\n-----END KEY-----";
        let block = "k = \"\"\"-----BEGIN KEY-----\nAAAA\n-----END KEY-----\"\"\"\n";
        let spans = Region { start_line: Some(1), start_column: Some(8), end_line: Some(3), end_column: Some(18) };
        assert!(lines_hold(block.as_bytes(), &spans, key));
        assert!(!lines_hold(block.as_bytes(), &Region { end_line: Some(2), ..spans }, key), "one line short");
        let crlf = block.replace('\n', "\r\n");
        assert!(lines_hold(crlf.as_bytes(), &spans, key), "a CRLF checkout of an LF blob");
        assert!(lines_hold(block.as_bytes(), &spans, b"-----BEGIN KEY-----\r\nAAAA\r\n-----END KEY-----"), "an LF checkout of a CRLF blob");
        assert!(!lines_hold(crlf.as_bytes(), &spans, b"-----BEGIN KEY-----\r\nAAAB\r\n-----END KEY-----"), "still exact past the line endings");
    }

    /// The hunk parser reads each new path's added hunks from a `-U0` diff: a
    /// count-less header is one line, a `+n,0` hunk and a deleted file add
    /// nothing, and a quoted path is unquoted.
    #[test]
    fn the_hunk_parser_reads_each_paths_added_lines() {
        let diff = b"diff --git a/conf/s.py b/conf/s.py\n--- a/conf/s.py\n+++ b/conf/s.py\n\
            @@ -3 +3 @@\n-old\n+new\n@@ -9,0 +10,2 @@\n+a\n+b\n@@ -20,2 +21,0 @@\n-x\n-y\n\
            diff --git a/gone.py b/gone.py\n--- a/gone.py\n+++ /dev/null\n@@ -1 +0,0 @@\n-z\n\
            diff --git \"a/sp ace.py\" \"b/sp ace.py\"\n+++ \"b/sp ace.py\"\n@@ -0,0 +1,3 @@\n+1\n+2\n+3\n";
        let hunks = parse_added_hunks(diff);
        assert_eq!(hunks.get("conf/s.py"), Some(&vec![(3, 1), (10, 2)]));
        assert_eq!(hunks.get("sp ace.py"), Some(&vec![(1, 3)]));
        assert_eq!(hunks.len(), 2, "the deleted file adds nothing: {hunks:?}");
    }

    /// A path that could leave the project is never read; a project path is.
    #[test]
    fn only_a_project_relative_path_is_read() {
        assert!(is_project_relative("tests/fixtures/settings.py"));
        assert!(is_project_relative("./a.py"));
        assert!(!is_project_relative("../outside.py"));
        assert!(!is_project_relative("a/../../outside.py"));
        assert!(!is_project_relative("/etc/passwd"));
        assert!(!is_project_relative(""));
    }
}
