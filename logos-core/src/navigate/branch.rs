//! Branch and merge symbol overlap ([FR-NV-13], CR-114) — the integration
//! boundary's question, asked of git and the graph together.
//!
//! Two halves, one traversal of the same evidence:
//!
//! - **Before the merge**: given N refs, which symbols does more than one of
//!   them modify? Those refs contend, and the contention is named rather than
//!   discovered by a conflict marker.
//! - **After the merge**: given a stated merge result, which symbols did a ref
//!   modify that the result does not carry? A clean merge is not a complete
//!   merge — when several branches contribute to one append point, git resolves
//!   them without a conflict and nothing signals what never arrived.
//!
//! # How a git hunk becomes a symbol
//!
//! `git diff --unified=0 <base> <ref>` gives, per file, the line ranges the ref
//! changed on its own side. Those ranges are mapped onto the **indexed** symbol
//! spans of the same file ([`span_nodes_in_files`](crate::graph_store::GraphStore::span_nodes_in_files)), keeping the
//! *innermost* symbols a range lands in, so a hunk inside a function is
//! attributed to that function and not also to its enclosing module.
//!
//! That join has one horizon per source, and both are stated on the payload
//! ([NFR-CC-04], [`OverlapCoverage`]): git sees every byte of every ref, the
//! graph sees one indexed snapshot. A symbol outside the indexed set therefore
//! cannot be reported at all, and a symbol whose span moved between a ref and
//! the snapshot may be attributed to its neighbour. Neither is papered over:
//! the files that drifted, the files the index holds nothing for, and the hunks
//! that landed in no span are all counted back to the caller.
//!
//! Local and read-only throughout ([NFR-SE-01]): every git call is a local
//! subprocess against the object database — no fetch, no remote, no write.
//!
//! [FR-NV-13]: ../../../docs/specs/requirements/FR-NV-13.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Output};

use anyhow::Result;

use crate::engine::Engine;
use crate::graph_store::NodeRow;
use crate::model::NodeId;
use crate::models::navigation::{
    BranchOverlapResult, ContendedSymbol, LostFile, LostSymbol, MergeCheck, OverlapCoverage,
    RefChangeSummary,
};

use super::symbol_ref;

/// Bounded symbol lists. A merge whose refs contend on hundreds of symbols has
/// already answered the integration question; the tail is weight, so it is
/// elided **and counted** ([NFR-CC-04]) rather than shipped.
const MAX_SYMBOLS_LISTED: usize = 100;

/// Bounded path lists (the coverage limits and `lost_files`). Same rule, lower
/// bound: a path list is a pointer to where to look, not the answer itself.
const MAX_PATHS_LISTED: usize = 50;

/// The standing coverage-limits statement carried by every overlap answer
/// ([FR-NV-13] AC 4, [NFR-CC-04]). Fixed prose, not a computed claim: what
/// varies between answers is the counted evidence listed beside it.
const OVERLAP_COVERAGE: &str = "changed line ranges are read from git and attributed to the \
symbol spans of the INDEXED snapshot. A symbol outside the indexed set cannot be reported — an \
unindexed language, an excluded path, or a symbol a ref adds that never reached the index is \
invisible to the symbol half of this answer, and only its file can be reported. Spans come from \
the snapshot, so a symbol that moved between a ref and the snapshot may be attributed to its \
neighbour; the drifted files are listed. `absent_from` marks refs that do not touch a symbol \
their siblings share — the silent-drop shape, and a smell rather than a proof; it is drawn only from \
the refs that were actually diffed, so a ref listed under `unresolved_refs` or `refs_not_diffed` is \
never reported as absent from anything.";

/// `branch_overlap` — which refs collide, and what a merge did not carry
/// ([FR-NV-13], CR-114).
///
/// Diffs every ref against a common base (the octopus merge-base, or `base`
/// when given), attributes each ref's changed line ranges to indexed symbols,
/// and reports the symbols more than one ref modifies. When `merge` names a
/// merge result, also reports the symbols and files a ref changed that the
/// result does not.
///
/// Deterministic throughout: refs keep the caller's order, contended symbols
/// sort most-contended first then by canonical symbol ([NFR-RA-06]). Costs one
/// pooled read for the whole attribution — the changed files' spans are
/// materialised once, never one lookup per hunk.
///
/// [FR-NV-13]: ../../../docs/specs/requirements/FR-NV-13.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn branch_overlap(
    engine: &Engine,
    refs: &[String],
    base: Option<&str>,
    merge: Option<&str>,
) -> Result<BranchOverlapResult> {
    let root = engine.root().to_path_buf();
    let mut warnings = Vec::new();
    if refs.is_empty() {
        warnings.push("no refs supplied; nothing to compare".to_string());
    } else if refs.len() < 2 && merge.is_none() {
        warnings.push(
            "an overlap needs at least two refs, or one ref and a stated merge result".to_string(),
        );
    }

    // ── resolve every ref, then the comparison point ────────────────────────
    let mut coverage = OverlapCoverage {
        statement: OVERLAP_COVERAGE.to_string(),
        indexed_snapshot: rev_parse(&root, "HEAD"),
        ..OverlapCoverage::default()
    };
    let resolved: Vec<(String, Option<String>)> = refs
        .iter()
        .map(|r| (r.clone(), rev_parse(&root, r)))
        .collect();
    for (name, commit) in &resolved {
        if commit.is_none() {
            coverage.unresolved_refs.push(name.clone());
            warnings.push(format!("{name} does not resolve to a commit in this repository"));
        }
    }
    let merge_commit = merge.and_then(|m| {
        let commit = rev_parse(&root, m);
        if commit.is_none() {
            warnings.push(format!(
                "the stated merge result {m} does not resolve to a commit in this repository"
            ));
        }
        commit
    });

    let ref_names: Vec<String> = resolved.iter().map(|(name, _)| name.clone()).collect();
    let live: Vec<(usize, &str, &str)> = resolved
        .iter()
        .enumerate()
        .filter_map(|(i, (name, commit))| Some((i, name.as_str(), commit.as_deref()?)))
        .collect();
    let (base_commit, base_origin) = resolve_base(&root, base, &live, merge_commit.as_deref());
    if base_commit.is_none() && !live.is_empty() {
        warnings.push(
            "no common ancestor for the supplied refs; pass --base to state the comparison point"
                .to_string(),
        );
    }

    let mut result = BranchOverlapResult {
        base: base_commit.clone(),
        base_origin,
        refs: resolved
            .iter()
            .map(|(name, commit)| RefChangeSummary {
                git_ref: name.clone(),
                commit: commit.clone(),
                ..RefChangeSummary::default()
            })
            .collect(),
        coverage,
        warnings,
        ..BranchOverlapResult::default()
    };
    let Some(base_commit) = base_commit else {
        return Ok(result);
    };

    // ── one diff per ref (plus one for the merge result) ────────────────────
    let mut per_ref: Vec<(usize, FileRanges)> = Vec::with_capacity(live.len());
    for (index, name, commit) in &live {
        match changed_ranges(&root, &base_commit, commit) {
            Ok(changes) => {
                result.refs[*index].files_changed = changes.len() as u32;
                per_ref.push((*index, changes));
            }
            Err(err) => {
                // The ref resolved but we have no data for it. It must NOT fall
                // through to `absent_from` below: "we did not look" and "this
                // ref did not touch it" are opposite claims, and `absent_from`
                // is the field a reader acts on.
                result.coverage.refs_not_diffed.push((*name).to_string());
                result
                    .warnings
                    .push(format!("diffing {name} against the base failed: {err}"));
            }
        }
    }
    let merge_changes = match (&merge_commit, merge) {
        (Some(commit), Some(name)) => match changed_ranges(&root, &base_commit, commit) {
            Ok(changes) => Some(changes),
            Err(err) => {
                result.warnings.push(format!(
                    "diffing the stated merge result {name} against the base failed: {err}"
                ));
                None
            }
        },
        _ => None,
    };

    // ── one pooled read materialises every changed file's symbol spans ──────
    let mut paths: BTreeSet<String> = per_ref
        .iter()
        .flat_map(|(_, changes)| changes.keys().cloned())
        .collect();
    if let Some(changes) = &merge_changes {
        paths.extend(changes.keys().cloned());
    }
    let path_list: Vec<String> = paths.iter().cloned().collect();
    let runtime = engine.nav_runtime()?;
    let spans = runtime.submit_read(move |store| store.span_nodes_in_files(&path_list))?;
    let mut by_path: BTreeMap<&str, Vec<&NodeRow>> = BTreeMap::new();
    for row in &spans {
        if let Some(path) = row.file_path.as_deref() {
            by_path.entry(path).or_default().push(row);
        }
    }
    result.coverage.files_without_indexed_symbols = paths
        .iter()
        .filter(|path| !by_path.contains_key(path.as_str()))
        .take(MAX_PATHS_LISTED)
        .cloned()
        .collect();

    // ── attribute each ref's ranges to symbols ──────────────────────────────
    let mut rows: BTreeMap<NodeId, &NodeRow> = BTreeMap::new();
    let mut modified: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
    for (index, changes) in &per_ref {
        let mut touched: BTreeSet<NodeId> = BTreeSet::new();
        for (path, ranges) in changes {
            let Some(nodes) = by_path.get(path.as_str()) else {
                continue;
            };
            for range in ranges {
                let hit = innermost(nodes, *range);
                if hit.is_empty() {
                    result.coverage.unattributed_hunks += 1;
                }
                for row in hit {
                    rows.insert(row.id, row);
                    touched.insert(row.id);
                }
            }
        }
        result.refs[*index].symbols_modified = touched.len() as u32;
        for id in touched {
            modified.entry(id).or_default().push(*index);
        }
    }
    for entry in modified.values_mut() {
        entry.sort_unstable();
    }

    // Which changed files differ from the indexed snapshot: those are the ones
    // whose spans may have moved under the attribution above ([NFR-CC-04]).
    let mut drifted: BTreeSet<String> = BTreeSet::new();
    for (_, _, commit) in &live {
        if let Some(against_head) = changed_paths(&root, "HEAD", commit) {
            drifted.extend(against_head.intersection(&paths).cloned());
        }
    }
    result.coverage.files_with_drifted_spans =
        drifted.into_iter().take(MAX_PATHS_LISTED).collect();

    // ── the collision half ([FR-NV-13] AC 1) ────────────────────────────────
    // `absent_from` is drawn from the refs actually DIFFED, never merely from
    // the refs that resolved: a ref whose diff failed is an unknown, and naming
    // it as absent would manufacture exactly the silent-drop signal this query
    // exists to make trustworthy.
    let diffed: Vec<usize> = per_ref.iter().map(|(index, _)| *index).collect();
    let mut contended: Vec<ContendedSymbol> = modified
        .iter()
        .filter(|(_, by)| by.len() > 1)
        .filter_map(|(id, by)| {
            let row = rows.get(id)?;
            Some(ContendedSymbol {
                symbol: symbol_ref(row),
                modified_by: by.iter().map(|i| ref_names[*i].clone()).collect(),
                absent_from: diffed
                    .iter()
                    .copied()
                    .filter(|index| !by.contains(index))
                    .map(|index| ref_names[index].clone())
                    .collect(),
            })
        })
        .collect();
    // Most-contended first, then canonical symbol — so the truncation below can
    // never drop a five-way collision in favour of a two-way one.
    contended.sort_by(|a, b| {
        b.modified_by
            .len()
            .cmp(&a.modified_by.len())
            .then_with(|| a.symbol.symbol.cmp(&b.symbol.symbol))
    });
    result.contended_total = contended.len() as u32;
    contended.truncate(MAX_SYMBOLS_LISTED);
    result.contended_elided = result.contended_total - contended.len() as u32;
    result.contended = contended;

    // ── the silent-drop half ([FR-NV-13] AC 2) ──────────────────────────────
    if let (Some(name), Some(changes)) = (merge, &merge_changes) {
        let mut merged_symbols: BTreeSet<NodeId> = BTreeSet::new();
        for (path, ranges) in changes {
            let Some(nodes) = by_path.get(path.as_str()) else {
                continue;
            };
            for range in ranges {
                merged_symbols.extend(innermost(nodes, *range).into_iter().map(|row| row.id));
            }
        }
        let mut lost_symbols: Vec<LostSymbol> = modified
            .iter()
            .filter(|(id, _)| !merged_symbols.contains(id))
            .filter_map(|(id, by)| {
                let row = rows.get(id)?;
                Some(LostSymbol {
                    symbol: symbol_ref(row),
                    modified_by: by.iter().map(|i| ref_names[*i].clone()).collect(),
                    merge_changed_the_file: row
                        .file_path
                        .as_deref()
                        .is_some_and(|path| changes.contains_key(path)),
                })
            })
            .collect();
        // The merge took some of the file and not this: the stronger signal
        // leads, so truncation cannot drop it for a whole-file omission.
        lost_symbols.sort_by(|a, b| {
            b.merge_changed_the_file
                .cmp(&a.merge_changed_the_file)
                .then_with(|| a.symbol.symbol.cmp(&b.symbol.symbol))
        });
        let lost_symbols_total = lost_symbols.len() as u32;
        lost_symbols.truncate(MAX_SYMBOLS_LISTED);

        let mut by_file: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (index, ref_changes) in &per_ref {
            for path in ref_changes.keys() {
                if !changes.contains_key(path) {
                    by_file.entry(path.as_str()).or_default().push(*index);
                }
            }
        }
        result.merge = Some(MergeCheck {
            git_ref: name.to_string(),
            commit: merge_commit,
            files_changed: changes.len() as u32,
            lost_symbols_elided: lost_symbols_total - lost_symbols.len() as u32,
            lost_symbols_total,
            lost_symbols,
            lost_files: by_file
                .into_iter()
                .take(MAX_PATHS_LISTED)
                .map(|(path, by)| LostFile {
                    path: path.to_string(),
                    modified_by: by.into_iter().map(|i| ref_names[i].clone()).collect(),
                })
                .collect(),
        });
    }
    Ok(result)
}

/// The degraded [ADR-14] payload: an answer with no data behind it is the one
/// that most needs its limits stated, so it carries the standing coverage
/// statement rather than an empty string.
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
pub(crate) fn overlap_degraded(message: String) -> BranchOverlapResult {
    BranchOverlapResult {
        base_origin: "not determined — the query degraded before diffing".to_string(),
        coverage: OverlapCoverage {
            statement: OVERLAP_COVERAGE.to_string(),
            ..OverlapCoverage::default()
        },
        warnings: vec![message],
        ..BranchOverlapResult::default()
    }
}

/// The comparison point, and the words that explain it.
///
/// A caller-supplied `--base` always wins — stating it is how a caller says
/// "compare against *this*", and second-guessing that would make the answer
/// depend on repository shape. Otherwise the octopus merge-base of the resolved
/// refs (and the merge result, when one is stated, so a merge commit's own
/// parentage cannot move the base under its branches).
fn resolve_base(
    root: &Path,
    base: Option<&str>,
    live: &[(usize, &str, &str)],
    merge: Option<&str>,
) -> (Option<String>, String) {
    if let Some(base) = base {
        return match rev_parse(root, base) {
            Some(commit) => (Some(commit), format!("stated by the caller as {base}")),
            None => (
                None,
                format!("{base} was stated as the base but does not resolve"),
            ),
        };
    }
    if live.is_empty() {
        return (None, "no ref resolved, so no base could be chosen".to_string());
    }
    let mut args = vec!["merge-base", "--octopus"];
    args.extend(live.iter().map(|(_, _, commit)| *commit));
    args.extend(merge);
    let commit = git(root, &args).ok().filter(|o| o.status.success()).map(|o| {
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    });
    match commit.filter(|c| !c.is_empty()) {
        Some(commit) => (
            Some(commit),
            "the common ancestor (git merge-base) of the supplied refs".to_string(),
        ),
        None => (
            None,
            "the supplied refs have no common ancestor".to_string(),
        ),
    }
}

/// Per-file changed line ranges on the **new** side of a diff.
type FileRanges = BTreeMap<String, Vec<(u32, u32)>>;

/// The line ranges `commit` changed relative to `base`, per file.
///
/// `--unified=0` so a range is the change itself and not three lines of
/// courtesy context either side; `--no-renames` so a rename reads as the
/// delete-and-add it is for attribution purposes; `--no-prefix` so no `a/`/`b/`
/// convention has to be stripped (which would corrupt a real path beginning
/// `b/`). The header path still needs [`header_path`] — `--no-prefix` does not
/// make the rest of the line a bare path.
fn changed_ranges(root: &Path, base: &str, commit: &str) -> Result<FileRanges> {
    let out = git(
        root,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            "--no-prefix",
            "--unified=0",
            base,
            commit,
        ],
    )?;
    anyhow::ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(parse_diff(&String::from_utf8_lossy(&out.stdout)))
}

/// The files `commit` differs from `other` in — the drift probe, and cheaper
/// than a content diff. `None` when git could not answer.
fn changed_paths(root: &Path, other: &str, commit: &str) -> Option<BTreeSet<String>> {
    let out = git(root, &["diff", "--name-only", "--no-renames", other, commit]).ok()?;
    out.status.success().then(|| {
        // `--name-only` C-quotes a path containing `"` or a control character
        // exactly as the diff headers do (there is no trailing TAB here), so it
        // has to come back through the same unquoting or the intersection with
        // the diffed paths silently misses those files.
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(unquote_c_style)
            .collect()
    })
}

/// The range standing for "every line of this file" — what a deleted file
/// contributes, since it has no new side to carry hunks. A ref that deletes a
/// file has modified every symbol the file held, and that is exactly the kind of
/// contention this query exists to surface before a merge.
const WHOLE_FILE: (u32, u32) = (1, u32::MAX);

/// Parse `git diff --unified=0 --no-prefix` into per-file new-side ranges.
fn parse_diff(diff: &str) -> FileRanges {
    let mut files: FileRanges = BTreeMap::new();
    let mut header = FileHeader::start();
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            header = FileHeader::start();
        } else if header.in_header && line.starts_with("--- ") {
            header.old_path = header_path(&line[4..]);
        } else if header.in_header && line.starts_with("+++ ") {
            header.open(&line[4..], &mut files);
        } else if line.starts_with("@@ ") {
            header.hunk(line, &mut files);
        }
    }
    files
}

/// The per-file state a `--unified=0` diff parse carries between lines.
///
/// It exists for [`in_header`](Self::in_header). The `---`/`+++` lines are only
/// read while inside a file header — between `diff --git` and that file's first
/// `@@` — because with zero context an added line whose own content begins
/// `++ ` is indistinguishable from a path header by prefix alone. That is the
/// classic way a hand-rolled diff parser goes wrong.
struct FileHeader {
    /// Whether the parse is inside a file header.
    in_header: bool,
    /// The `---` path, kept so a deletion can be recorded under its old name.
    old_path: Option<String>,
    /// The file the following hunks belong to.
    current: Option<String>,
    /// `false` once `+++ /dev/null` says this file has no new side.
    new_side: bool,
}

impl FileHeader {
    /// The state at the start of a file's header (and of the whole diff).
    fn start() -> Self {
        FileHeader {
            in_header: true,
            old_path: None,
            current: None,
            new_side: true,
        }
    }

    /// Read a `+++` line: register the changed file, and give a deletion the
    /// whole-file range it has no hunks to express.
    fn open(&mut self, raw: &str, files: &mut FileRanges) {
        let path = header_path(raw);
        self.new_side = path.is_some();
        self.current = path.or_else(|| self.old_path.clone());
        let Some(current) = self.current.clone() else {
            return;
        };
        let ranges = files.entry(current).or_default();
        if !self.new_side {
            ranges.push(WHOLE_FILE);
        }
    }

    /// Read an `@@` line, which also ends the header. A file with no new side
    /// has only old-side hunks — they describe lines that no longer exist, and
    /// its range already came from [`open`](Self::open).
    fn hunk(&mut self, line: &str, files: &mut FileRanges) {
        self.in_header = false;
        let (Some(current), Some(range)) = (self.current.clone(), hunk_range(line)) else {
            return;
        };
        if self.new_side {
            files.entry(current).or_default().push(range);
        }
    }
}

/// The project-relative path a `---`/`+++` header names, or `None` for git's
/// `/dev/null` absent-side marker.
///
/// The rest of the header line is **not** the path. Git appends a literal TAB
/// whenever the path contains a space, and C-quotes the whole path in double
/// quotes whenever it contains `"`, a TAB, or another control character —
/// neither of which `core.quotePath=false` suppresses (that setting governs
/// only octal-escaping of non-ASCII bytes). Taking the raw remainder would key
/// the file map on `"src/My Handler.ts\t"`, which matches no indexed path, so a
/// collision in any file with a space in its name would be silently missed.
fn header_path(raw: &str) -> Option<String> {
    let path = raw.strip_suffix('\t').unwrap_or(raw);
    (path != "/dev/null").then(|| unquote_c_style(path))
}

/// Undo git's C-style quoting if `path` carries it, else return it unchanged.
///
/// Git quotes with a leading and trailing `"` and escapes `\\`, `\"`, the usual
/// control-character letters, and any other byte as three octal digits. Bytes
/// are collected and decoded once at the end so a multi-byte character split
/// across escapes still reassembles.
fn unquote_c_style(path: &str) -> String {
    let Some(inner) = path
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|_| path.len() >= 2)
    else {
        return path.to_string();
    };
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut bytes = inner.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            out.push(byte);
            continue;
        }
        match bytes.next() {
            Some(b'a') => out.push(0x07),
            Some(b'b') => out.push(0x08),
            Some(b'f') => out.push(0x0c),
            Some(b'n') => out.push(b'\n'),
            Some(b'r') => out.push(b'\r'),
            Some(b't') => out.push(b'\t'),
            Some(b'v') => out.push(0x0b),
            // `\NNN` octal: git always emits exactly three digits.
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    match bytes.next() {
                        Some(next @ b'0'..=b'7') => value = value * 8 + u32::from(next - b'0'),
                        Some(other) => {
                            out.push(value as u8);
                            out.push(other);
                            value = u32::MAX;
                            break;
                        }
                        None => break,
                    }
                }
                if value != u32::MAX {
                    out.push(value as u8);
                }
            }
            // `\\`, `\"`, and anything git did not mean as an escape.
            Some(other) => out.push(other),
            None => out.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The inclusive new-side line range of a `@@ -a,b +c,d @@` hunk header.
///
/// A pure deletion (`+c,0`) has no new-side lines: git reports the line the
/// removal sat after, so the range is that line and the one following it —
/// whichever symbol the removed text belonged to is one of those two.
fn hunk_range(line: &str) -> Option<(u32, u32)> {
    let plus = line
        .split_whitespace()
        .find(|token| token.starts_with('+'))?
        .trim_start_matches('+');
    let (start, count) = match plus.split_once(',') {
        Some((start, count)) => (start.parse::<u32>().ok()?, count.parse::<u32>().ok()?),
        None => (plus.parse::<u32>().ok()?, 1),
    };
    Some(match count {
        0 => (start.max(1), start.max(1) + 1),
        count => (start, start + count - 1),
    })
}

/// The **innermost** indexed symbols a changed range lands in.
///
/// A file's spans nest — a function sits inside an impl block inside a module,
/// and all three overlap the same hunk. Reporting all three would make every
/// answer claim the file's outermost node is contended, which is true and
/// useless. So an overlapping node is kept only when it contains no other
/// overlapping node: a hunk inside one function yields that function, and a
/// range spanning several siblings yields each sibling.
fn innermost<'a>(nodes: &[&'a NodeRow], (lo, hi): (u32, u32)) -> Vec<&'a NodeRow> {
    let span = |row: &NodeRow| {
        let start = row.start_line.unwrap_or(0).max(0) as u32;
        let end = row.end_line.unwrap_or(0).max(0) as u32;
        (start, end.max(start))
    };
    let overlapping: Vec<&NodeRow> = nodes
        .iter()
        .copied()
        .filter(|row| {
            let (start, end) = span(row);
            start > 0 && start <= hi && end >= lo
        })
        .collect();
    overlapping
        .iter()
        .copied()
        .filter(|outer| {
            let (os, oe) = span(outer);
            !overlapping.iter().any(|inner| {
                let (is, ie) = span(inner);
                inner.id != outer.id && os <= is && oe >= ie && (os < is || oe > ie)
            })
        })
        .collect()
}

/// Resolve a ref to a commit id, or `None` when it is not one (or `git` is not
/// on `PATH`, or this is not a repository — all of which the caller reports as
/// an unresolved ref rather than an error, [ADR-14]).
fn rev_parse(root: &Path, reference: &str) -> Option<String> {
    let spec = format!("{reference}^{{commit}}");
    let out = git(root, &["rev-parse", "--verify", "--quiet", &spec]).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
}

/// Spawn `git -C <root> <args…>`, returning the raw [`Output`].
///
/// `core.quotePath=false` keeps non-ASCII paths literal (no octal quoting) so
/// the diff parser sees real bytes — the same boundary the history miner sets.
fn git(root: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotePath=false"])
        .args(args)
        .output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LogosSymbol, NodeKind};

    fn node(id: i64, start: i64, end: i64) -> NodeRow {
        NodeRow {
            id: NodeId(id),
            symbol: LogosSymbol::parse(&format!("logos . . . a.rs/s{id}()."))
                .expect("fixture symbol parses"),
            kind: NodeKind::Function,
            name: format!("s{id}"),
            file_path: Some("a.rs".to_string()),
            start_line: Some(start),
            end_line: Some(end),
        }
    }

    /// An added line whose own content begins `++ ` must not be mistaken for a
    /// `+++` path header — the classic hand-rolled-diff-parser bug. The header
    /// state machine is what prevents it, so the fixture puts one inside a hunk.
    #[test]
    fn a_plus_plus_line_inside_a_hunk_is_not_read_as_a_path() {
        let diff = "diff --git one.rs one.rs\n\
                    --- one.rs\n\
                    +++ one.rs\n\
                    @@ -3,0 +4,2 @@\n\
                    +++ not/a/path\n\
                    +--- neither is this\n";
        let files = parse_diff(diff);
        assert_eq!(files.keys().collect::<Vec<_>>(), vec!["one.rs"]);
        assert_eq!(files["one.rs"], vec![(4, 5)]);
    }

    /// A deleted file has no new side, so its `@@` headers describe lines that
    /// no longer exist. It is recorded under its old path with the whole-file
    /// range instead — a ref that deletes a file has modified every symbol it
    /// held, which is precisely the collision worth knowing about before a
    /// merge. The phantom `+0,0` range must NOT also appear.
    #[test]
    fn a_deleted_file_claims_every_symbol_it_held_and_no_phantom_range() {
        let diff = "diff --git gone.rs gone.rs\n\
                    deleted file mode 100644\n\
                    --- gone.rs\n\
                    +++ /dev/null\n\
                    @@ -1,4 +0,0 @@\n";
        let files = parse_diff(diff);
        assert_eq!(files["gone.rs"], vec![WHOLE_FILE]);
    }

    /// Git does NOT put a bare path on a `---`/`+++` line. It appends a TAB when
    /// the path holds a space and C-quotes the whole thing when it holds a `"`
    /// or a control character — `core.quotePath=false` suppresses neither. The
    /// fixture lines here are verbatim `git diff --no-prefix` output.
    #[test]
    fn header_paths_are_de_tabbed_and_unquoted() {
        assert_eq!(header_path("plain.rs"), Some("plain.rs".to_string()));
        assert_eq!(header_path("my file.rs\t"), Some("my file.rs".to_string()));
        assert_eq!(
            header_path("\"say \\\"hi\\\".py\"\t"),
            Some("say \"hi\".py".to_string())
        );
        assert_eq!(header_path("/dev/null"), None);
        // A tab INSIDE the name is octal- or letter-escaped inside the quotes,
        // so the trailing-tab strip cannot eat part of a real path.
        assert_eq!(header_path("\"tab\\there.rs\"\t"), Some("tab\there.rs".to_string()));
        assert_eq!(header_path("\"oct\\303\\251.rs\""), Some("octé.rs".to_string()));
    }

    /// The whole parse, over a header carrying a space — the case that used to
    /// key the file map on a path no index could match.
    #[test]
    fn a_path_with_a_space_survives_the_parse() {
        let diff = "diff --git my file.rs my file.rs\n\
                    --- my file.rs\t\n\
                    +++ my file.rs\t\n\
                    @@ -1 +1 @@\n\
                    -a\n\
                    +b\n";
        let files = parse_diff(diff);
        assert_eq!(files.keys().collect::<Vec<_>>(), vec!["my file.rs"]);
        assert_eq!(files["my file.rs"], vec![(1, 1)]);
    }

    /// Both hunk-header spellings, and the deletion case where git reports the
    /// line the removal sat after rather than a range of its own.
    #[test]
    fn hunk_headers_parse_to_inclusive_new_side_ranges() {
        assert_eq!(hunk_range("@@ -1,3 +1,4 @@ fn f()"), Some((1, 4)));
        assert_eq!(hunk_range("@@ -9 +9 @@"), Some((9, 9)));
        assert_eq!(hunk_range("@@ -7,2 +6,0 @@"), Some((6, 7)));
        assert_eq!(hunk_range("@@ -0,0 +0,0 @@"), Some((1, 2)));
        assert_eq!(hunk_range("not a hunk"), None);
    }

    /// Nesting: a hunk inside a function must yield the function, not also the
    /// module that encloses it — otherwise every answer reports the file's
    /// outermost node as contended, which is true and useless.
    #[test]
    fn attribution_keeps_the_innermost_symbols_a_range_lands_in() {
        let (module, first, second) = (node(1, 1, 100), node(2, 10, 20), node(3, 30, 40));
        let nodes = vec![&module, &first, &second];

        let hit = innermost(&nodes, (12, 12));
        assert_eq!(hit.iter().map(|r| r.id).collect::<Vec<_>>(), vec![first.id]);

        // A range spanning two siblings yields both — and still not the module.
        let hit = innermost(&nodes, (15, 35));
        assert_eq!(
            hit.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![first.id, second.id]
        );

        // A range in the module's own body, outside every child, is the module.
        let hit = innermost(&nodes, (50, 50));
        assert_eq!(hit.iter().map(|r| r.id).collect::<Vec<_>>(), vec![module.id]);

        // Past the end of everything: nothing, which the caller counts as an
        // unattributed hunk rather than silently discarding.
        assert!(innermost(&nodes, (500, 500)).is_empty());
    }
}
