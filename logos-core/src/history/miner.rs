//! The incremental, HEAD-anchored git miner ([FR-GH-02], [ADR-22], [Git]).
//!
//! Parses `git log --numstat -M -z` into per-commit per-file change facts (the
//! `-z`/`NUL` path framing makes rename detection exact — see [`NUL`]). The
//! design honours three boundaries:
//!
//! - **HEAD-anchored window** ([BR-27]): "now" is the HEAD commit's committer
//!   timestamp; the cutoff is `HEAD − window_months` computed in UTC by
//!   [`window_cutoff`] — a pure function of that timestamp. This module never
//!   calls `Utc::now`; the determinism is enforced here and by the
//!   byte-identical golden, not by the `chrono` feature graph.
//! - **Incremental** ([FR-GH-02]): after the first mine only `mined_through..HEAD`
//!   is read; an unchanged HEAD reads **zero** commits without spawning `git`.
//! - **Local & read-only** ([NFR-SE-01]): every call is a local `git`
//!   subprocess; no `fetch`, no remote, only the object database is read.
//!
//! Author identities respect `.mailmap` for free: the `%aN`/`%aE` format
//! placeholders are the mailmapped name/email ([FR-GH-02] note) — a local,
//! deterministic input, part of the [ADR-22] boundary.
//!
//! [FR-GH-02]: ../../../docs/specs/requirements/FR-GH-02.md
//! [BR-27]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
//! [Git]: ../../../docs/specs/architecture/integrations/git.md

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};
use chrono::{DateTime, Months, Utc};

use crate::config::EffectiveHistory;

/// `git log` record separator (ASCII RS, `0x1e`) — prefixes every commit's
/// format line so the output splits unambiguously regardless of how `-z` frames
/// the numstat body.
const RS: char = '\u{1e}';
/// `git log` field separator (ASCII US, `0x1f`) — separates the five header
/// fields. RS/US are control bytes that cannot occur in a sha or committer
/// timestamp and, in practice, are never emitted by any tooling into a path, a
/// git author identity, or a commit subject — so the header split is unambiguous
/// for real-world input.
const US: char = '\u{1f}';
/// The `git log --numstat -z` path separator (`NUL`, `0x00`). With `-z` git
/// terminates each numstat pathname with `NUL` and renders a rename as an
/// **empty** path field followed by `NUL old NUL new NUL` — so a path that
/// *literally* contains ` => ` is no longer misread as a rename (the [S-046]
/// deferred finding the temporal metrics depend on, [CR-006] CRA-02). `NUL`
/// cannot occur in a pathname, so the framing is exact and git-plumbing-stable.
///
/// [S-046]: ../../../docs/planning/journal.md#s-046-history-store-and-incremental-git-miner
/// [CR-006]: ../../../docs/requests/CR-006-git-history-analytics.md
const NUL: char = '\u{0}';

/// One mined commit's metadata ([FR-GH-02]); the `commits` row shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitFact {
    /// Full 40-hex commit id (`%H`).
    pub(crate) sha: String,
    /// Committer unix timestamp (`%ct`) — the determinism clock ([BR-27]).
    pub(crate) committed_at: i64,
    /// `.mailmap`-coalesced author name (`%aN`).
    pub(crate) author_name: String,
    /// `.mailmap`-coalesced author email (`%aE`).
    pub(crate) author_email: String,
    /// The commit subject (`%s`, the first message line) — the input the
    /// defect-history heuristic ([S-047], [FR-GH-05]) matches `defect_patterns`
    /// against. Single-line by construction (git's `%s` strips the body), so it
    /// never breaks the line/`\0` framing of the log parse.
    ///
    /// [S-047]: ../../../docs/planning/journal.md#s-047-temporal-metrics-co-change-and-defect-heuristic
    /// [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
    pub(crate) subject: String,
    /// Distinct files this commit touched — the co-change mega-commit cap input
    /// ([FR-GH-04]); a capped commit still counts toward churn.
    pub(crate) file_count: usize,
}

/// One numstat fact ([FR-GH-02]); the `file_changes` row shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileChange {
    /// The owning commit's sha.
    pub(crate) commit_sha: String,
    /// Current (post-rename) repo-relative path.
    pub(crate) path: String,
    /// Lines added, or `None` for a binary file (numstat `-`).
    pub(crate) added: Option<i64>,
    /// Lines deleted, or `None` for a binary file.
    pub(crate) deleted: Option<i64>,
    /// Pre-rename path when `-M` followed a rename, else `None`.
    pub(crate) old_path: Option<String>,
}

/// Why the temporal tier degraded to `n/a` instead of mining ([FR-GH-08]). The
/// surface ([S-048]) maps each to a one-line notice with exit 0.
///
/// [FR-GH-08]: ../../../docs/specs/requirements/FR-GH-08.md
/// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DegradedReason {
    /// The root is not inside a git working tree.
    NotGit,
    /// The `git` binary is not on `PATH`.
    GitAbsent,
    /// A shallow clone — history is truncated, so any window number would be a
    /// fabrication ([NFR-RA-05]). Never partial numbers.
    Shallow,
}

impl DegradedReason {
    /// A one-line, user-facing reason ([FR-GH-08] notice).
    pub fn message(self) -> &'static str {
        match self {
            DegradedReason::NotGit => "not a git repository — temporal metrics unavailable",
            DegradedReason::GitAbsent => "`git` not found on PATH — temporal metrics unavailable",
            DegradedReason::Shallow => {
                "shallow clone — temporal metrics unavailable (history is truncated)"
            }
        }
    }
}

/// The outcome of a mine attempt ([FR-GH-02]). A `Serialize` read-model so the
/// `pub` [`Engine::ensure_history_mined`](crate::Engine::ensure_history_mined)
/// seam conforms to the [ADR-01] "every public Engine method returns a
/// serializable read-model" convention.
///
/// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MineOutcome {
    /// Commits parsed and persisted this mine. `0` on an unchanged HEAD (the
    /// zero-cost path that spawns no `git log`) or when degraded.
    pub commits_read: usize,
    /// `true` when this mine populated a previously-empty store — the surface
    /// renders the [FR-GH-02] one-line first-mine notice from this flag.
    pub first_mine: bool,
    /// The resolved HEAD commit (`None` only when degraded before resolving it).
    pub head_sha: Option<String>,
    /// The newest commit incorporated into the store after this mine.
    pub mined_through: Option<String>,
    /// `Some` when the tier degraded instead of mining ([FR-GH-08]).
    pub degraded: Option<DegradedReason>,
}

impl MineOutcome {
    /// A degraded outcome carrying `reason` — nothing was mined.
    fn degraded(reason: DegradedReason) -> Self {
        Self {
            commits_read: 0,
            first_mine: false,
            head_sha: None,
            mined_through: None,
            degraded: Some(reason),
        }
    }
}

/// What `git rev-parse HEAD` told us about the repository state.
enum HeadState {
    /// HEAD resolves to this commit id.
    Resolved(String),
    /// Inside a repo, but HEAD is unresolvable (unborn branch / empty repo) —
    /// nothing to mine, but not an error.
    Unborn,
    /// Not a git repository (git ran, exited non-zero).
    NotGit,
    /// The `git` binary is absent from `PATH`.
    GitAbsent,
}

/// Run an incremental mine of the local git history at `root` into the
/// `history.db` connection, bounded by the HEAD-anchored window in `cfg`
/// ([FR-GH-02], [ADR-22]).
///
/// Lazy by contract: this is only ever reached from a temporal-surface read
/// ([S-048]), never from `gate`/`sync`/navigation ([BR-26]). The caller opens
/// the store; this function mines and persists.
///
/// # Errors
/// Returns an error only on an unexpected git/parse/persist failure. The
/// *expected* degraded states (non-git, `git` absent, shallow, unborn HEAD)
/// resolve to a [`MineOutcome`] with `degraded`/zero commits, never an error —
/// mining must never fail a caller ([NFR-RA-05]).
pub(crate) fn mine_incremental(
    conn: &mut rusqlite::Connection,
    root: &Path,
    cfg: &EffectiveHistory,
) -> Result<MineOutcome> {
    // 1. Resolve HEAD — also our non-git / git-absent probe.
    let head = match resolve_head(root) {
        HeadState::Resolved(sha) => sha,
        HeadState::Unborn => {
            // A repo with no commits: nothing to mine, but not degraded.
            return Ok(MineOutcome {
                commits_read: 0,
                first_mine: false,
                head_sha: None,
                mined_through: None,
                degraded: None,
            });
        }
        HeadState::NotGit => return Ok(MineOutcome::degraded(DegradedReason::NotGit)),
        HeadState::GitAbsent => return Ok(MineOutcome::degraded(DegradedReason::GitAbsent)),
    };

    // 2. Shallow clones cannot yield honest window numbers ([FR-GH-08]).
    if is_shallow(root) {
        return Ok(MineOutcome::degraded(DegradedReason::Shallow));
    }

    // 3. Unchanged-HEAD short circuit: the second mine at the same HEAD reads
    //    zero commits and spawns no `git log` ([FR-GH-02] acceptance).
    let cursor = super::db::mine_cursor(conn)?;
    let store_was_empty = cursor.is_none();
    if let Some(c) = &cursor {
        if c.head_sha == head {
            return Ok(MineOutcome {
                commits_read: 0,
                first_mine: false,
                head_sha: Some(head),
                mined_through: Some(c.mined_through.clone()),
                degraded: None,
            });
        }
    }

    // 4. HEAD-anchored window cutoff (UTC, wall-clock-free).
    let head_committed_at = head_committed_at(root, &head)?;
    let cutoff = window_cutoff(head_committed_at, cfg.window_months);

    // 5. Decide the commit range. Incremental when the prior `mined_through` is
    //    an ancestor of HEAD; otherwise (history rewritten/rebased) fall back to
    //    a full windowed re-mine — idempotent upserts make that safe.
    let range = cursor.as_ref().and_then(|c| {
        if is_ancestor(root, &c.mined_through, &head) {
            Some(format!("{}..{}", c.mined_through, head))
        } else {
            None
        }
    });

    // 6. Mine.
    let raw = run_log(root, range.as_deref(), cutoff)?;
    let mut commits = Vec::new();
    let mut changes = Vec::new();
    for (commit, files) in parse_numstat_log(&raw) {
        // Exact cutoff enforcement in Rust — `--since` bounded the walk for
        // cost; this owns correctness regardless of git's date fuzz ([BR-27]).
        if let Some(min) = cutoff {
            if commit.committed_at < min {
                continue;
            }
        }
        changes.extend(files);
        commits.push(commit);
    }

    // The newest commit incorporated. On an incremental mine that read nothing
    // new, keep the prior `mined_through`; HEAD still advances the cursor so the
    // next call short-circuits.
    let mined_through = newest(&commits)
        .map(str::to_string)
        .or_else(|| cursor.as_ref().map(|c| c.mined_through.clone()))
        .unwrap_or_else(|| head.clone());

    let git_version = git_version(root);
    super::db::persist_mine(
        conn,
        &commits,
        &changes,
        &head,
        &mined_through,
        &git_version,
    )?;

    // `first_mine` means "this populated a previously-empty store" — only true
    // when commits were actually persisted. A store that was empty but whose
    // every commit predates the window yields zero commits and is NOT a first
    // mine, so the surface ([S-048]) never shows the notice for an empty mine.
    let first_mine = store_was_empty && !commits.is_empty();
    if first_mine {
        // The [FR-GH-02] first-mine notice. Emitted through the single tracing
        // seam ([ADR-13]) on the default target so the stderr fmt layer renders
        // it (never the telemetry-only target, which is filtered off stderr).
        // The user-facing surface ([S-048]) also renders it from `first_mine`.
        tracing::info!(
            commits = commits.len(),
            window_months = cfg.window_months,
            "Logos mined git history for the first time"
        );
    }

    Ok(MineOutcome {
        commits_read: commits.len(),
        first_mine,
        head_sha: Some(head),
        mined_through: Some(mined_through),
        degraded: None,
    })
}

/// Compute the HEAD-anchored window cutoff: `head_committed_at` minus
/// `window_months` **calendar** months, in UTC ([BR-27], [FR-GH-03]).
///
/// Calendar-aware (the ratified choice): subtracting 12 months from a June-11
/// HEAD lands on the previous June 11, not "365 days ago", so the window tracks
/// human intuition while staying a pure function of the HEAD timestamp — no
/// wall clock, byte-identical across the CI matrix ([NFR-RA-06]). Returns `None`
/// only if the timestamp is out of `chrono`'s representable range (treated as
/// "no lower bound" by the caller — every commit is in-window).
///
/// [FR-GH-03]: ../../../docs/specs/requirements/FR-GH-03.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn window_cutoff(head_committed_at: i64, window_months: u32) -> Option<i64> {
    let head: DateTime<Utc> = DateTime::from_timestamp(head_committed_at, 0)?;
    let cutoff = head.checked_sub_months(Months::new(window_months))?;
    Some(cutoff.timestamp())
}

/// The newest commit id in a mined batch — the batch is in `git log` order
/// (newest first), so the first element is `mined_through`.
fn newest(commits: &[CommitFact]) -> Option<&str> {
    commits.first().map(|c| c.sha.as_str())
}

/// Parse `git log --numstat -M -z` output emitted with the RS/US header format
/// (see [`run_log`]). Pure and total — the unit-test seam for the parser.
///
/// Each commit block (split on [`RS`]) is a single header line
/// `sha US ct US name US email US subject`, a `\n`, then the `-z` numstat body.
/// In the body each entry is `added \t deleted \t` followed by either
/// `path NUL` (a non-rename) or `NUL old NUL new NUL` (a rename — git emits an
/// **empty** path field, then the two `NUL`-terminated paths). `-` counts mark a
/// binary file (`None`).
///
/// Splitting the body on [`NUL`] yields, per entry, one `"added\tdeleted\tpath"`
/// cell for a non-rename, or a `"added\tdeleted\t"` cell (empty trailing path)
/// **plus the next two cells** (old, new) for a rename — so a cursor that
/// consumes 1 cell normally and 3 on a rename reconstructs every change exactly.
/// This is what fixes the [S-046] deferred finding: a path literally containing
/// ` => ` is just a non-rename `path` cell, never split as a phantom rename.
///
/// [S-046]: ../../../docs/planning/journal.md#s-046-history-store-and-incremental-git-miner
fn parse_numstat_log(raw: &str) -> Vec<(CommitFact, Vec<FileChange>)> {
    let mut out = Vec::new();
    for block in raw.split(RS) {
        // Strip framing at both ends: a leading NUL/newline from the prior
        // commit's terminator, and the trailing commit-terminator NUL. Trimming
        // the tail matters for a no-diff commit (a merge) whose block is just
        // `header\0` with no `\n` — otherwise the terminator would cling to the
        // subject. NUL-terminated body paths are recovered by the `\0` split
        // regardless, so dropping trailing framing is safe.
        let block = block.trim_matches(['\n', NUL]);
        if block.is_empty() {
            continue;
        }
        // Header line is everything up to the first `\n`; the `-z` numstat body
        // (NUL-framed) is the remainder. A commit with no diff (a merge) has no
        // `\n` at all — `split_once` returns `None` and the body is empty.
        let (header, body) = match block.split_once('\n') {
            Some((h, b)) => (h, b),
            None => (block, ""),
        };

        let mut fields = header.splitn(5, US);
        let (Some(sha), Some(ct), Some(name), Some(email)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue; // a malformed header line is skipped, never fabricated
        };
        // `%s` is empty only for an empty subject; an absent field (4-field
        // legacy header) defaults to "" rather than dropping the commit.
        let subject = fields.next().unwrap_or("");
        let Ok(committed_at) = ct.parse::<i64>() else {
            continue;
        };

        let files = parse_numstat_body(sha, body);
        out.push((
            CommitFact {
                sha: sha.to_string(),
                committed_at,
                author_name: name.to_string(),
                author_email: email.to_string(),
                subject: subject.to_string(),
                file_count: files.len(),
            },
            files,
        ));
    }
    out
}

/// Parse the `-z` numstat body of one commit into its [`FileChange`] facts.
///
/// The body is a flat `NUL`-separated cell list; a rename spends three cells
/// (the empty-path counts cell + old + new), every other change one. A trailing
/// empty cell (the commit's `NUL` terminator) is skipped.
fn parse_numstat_body(sha: &str, body: &str) -> Vec<FileChange> {
    let cells: Vec<&str> = body.split(NUL).collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let cell = cells[i];
        if cell.is_empty() {
            // A blank cell is inter-commit/terminator framing, never an entry.
            i += 1;
            continue;
        }
        // `added \t deleted \t <path-or-empty>`.
        let mut parts = cell.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(rest)) = (parts.next(), parts.next(), parts.next())
        else {
            // Not a numstat cell (a stray fragment) — skip, never fabricate.
            i += 1;
            continue;
        };

        let (path, old_path, consumed) = if rest.is_empty() {
            // Rename: the next two cells are the old and new paths.
            match (cells.get(i + 1), cells.get(i + 2)) {
                (Some(old), Some(new)) => (new.to_string(), Some(old.to_string()), 3),
                // Truncated rename trailer (shouldn't happen) — skip the cell.
                _ => {
                    i += 1;
                    continue;
                }
            }
        } else {
            // Non-rename: `rest` is the verbatim path (may literally contain
            // ` => ` — no longer misread, the finding-#11 fix).
            (rest.to_string(), None, 1)
        };

        files.push(FileChange {
            commit_sha: sha.to_string(),
            path,
            added: parse_numstat_count(added),
            deleted: parse_numstat_count(deleted),
            old_path,
        });
        i += consumed;
    }
    files
}

/// A numstat count: a decimal, or `None` for the `-` binary-file marker.
fn parse_numstat_count(field: &str) -> Option<i64> {
    if field == "-" {
        None
    } else {
        field.parse::<i64>().ok()
    }
}

// ── git subprocess helpers ──────────────────────────────────────────────────

/// Spawn `git -C <root> <args…>`, returning the raw [`Output`].
///
/// `core.quotePath=false` keeps non-ASCII paths literal (no octal quoting) so
/// the parser sees real bytes; nothing else touches global config.
fn git(root: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotePath=false"])
        .args(args)
        .output()
}

/// Resolve HEAD, classifying the three "no mine" repository states apart from a
/// real commit id.
fn resolve_head(root: &Path) -> HeadState {
    match git(root, &["rev-parse", "HEAD"]) {
        Ok(out) if out.status.success() => {
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if sha.is_empty() {
                HeadState::Unborn
            } else {
                HeadState::Resolved(sha)
            }
        }
        // git ran and failed: either not a repo, or HEAD is unborn. Distinguish
        // by probing for the work tree.
        Ok(_) => {
            if is_inside_work_tree(root) {
                HeadState::Unborn
            } else {
                HeadState::NotGit
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HeadState::GitAbsent,
        // Any other spawn error: treat as non-git (degrade, never fail).
        Err(_) => HeadState::NotGit,
    }
}

/// The resolved HEAD commit id, or `None` if `root` is not a git work tree with
/// a resolvable HEAD (no commits, `git` absent, or non-repo). The coverage ingest
/// ([S-049]) anchors every snapshot to this ([FR-CV-02]); without it there is
/// nothing to anchor to, so the caller fails loud rather than fabricating a
/// snapshot ([NFR-RA-05]).
///
/// [S-049]: ../../../docs/planning/journal.md#s-049-coverage-store-parsers-and-ingest-pipeline
/// [FR-CV-02]: ../../../docs/specs/requirements/FR-CV-02.md
pub(crate) fn head_sha(root: &Path) -> Option<String> {
    match resolve_head(root) {
        HeadState::Resolved(sha) => Some(sha),
        HeadState::Unborn | HeadState::NotGit | HeadState::GitAbsent => None,
    }
}

/// `true` if `root` is inside a git work tree (used to tell an unborn HEAD apart
/// from a non-repo when `rev-parse HEAD` fails).
fn is_inside_work_tree(root: &Path) -> bool {
    matches!(
        git(root, &["rev-parse", "--is-inside-work-tree"]),
        Ok(out) if out.status.success()
            && String::from_utf8_lossy(&out.stdout).trim() == "true"
    )
}

/// `true` for a shallow clone ([FR-GH-08] degraded mode).
fn is_shallow(root: &Path) -> bool {
    matches!(
        git(root, &["rev-parse", "--is-shallow-repository"]),
        Ok(out) if out.status.success()
            && String::from_utf8_lossy(&out.stdout).trim() == "true"
    )
}

/// `true` if `ancestor` is an ancestor of `descendant` (drives the incremental
/// range vs full-re-mine decision). `git merge-base --is-ancestor` exits 0 when
/// true, 1 when false.
fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> bool {
    matches!(
        git(root, &["merge-base", "--is-ancestor", ancestor, descendant]),
        Ok(out) if out.status.success()
    )
}

/// HEAD's committer timestamp (`%ct`) — the window anchor ([BR-27]).
///
/// # Errors
/// Returns an error if `git show` fails or the output is not an integer (an
/// unexpected git failure, distinct from the degraded states already handled).
fn head_committed_at(root: &Path, head: &str) -> Result<i64> {
    let out = git(root, &["show", "-s", "--format=%ct", head])
        .with_context(|| format!("reading committer timestamp of {head}"))?;
    anyhow::ensure!(
        out.status.success(),
        "`git show -s --format=%ct {head}` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<i64>()
        .with_context(|| format!("parsing committer timestamp of {head}"))
}

/// The local `git --version` string, recorded in the cursor for golden
/// reproducibility ([ADR-22]). Best-effort: an unreadable version is stored as
/// `"unknown"` rather than failing the mine.
fn git_version(root: &Path) -> String {
    match git(root, &["--version"]) {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

/// Run the windowed `git log --numstat -M -z`, optionally restricted to a
/// `mined_through..HEAD` range, and return raw stdout for [`parse_numstat_log`].
///
/// `-z` frames numstat pathnames with `NUL`, making rename detection exact (see
/// [`NUL`]); `%s` adds the commit subject for the defect heuristic ([FR-GH-05]).
/// `--since=<ISO>` bounds the walk for cost ([FR-GH-02] "never walks history
/// older than the window"); the Rust-side cutoff filter in [`mine_incremental`]
/// owns exact correctness.
///
/// # Errors
/// Returns an error if `git log` cannot be spawned or exits non-zero.
fn run_log(root: &Path, range: Option<&str>, cutoff: Option<i64>) -> Result<String> {
    // The two owned strings are held in locals so the `&str` args can borrow
    // them; everything else is a `&str` literal — one vector, no re-collect.
    let format = format!("--format=format:{RS}%H{US}%ct{US}%aN{US}%aE{US}%s");
    let since = cutoff
        .and_then(|min| DateTime::from_timestamp(min, 0))
        // Exact ISO 8601 UTC — git parses it precisely (not approxidate fuzz).
        .map(|dt| format!("--since={}", dt.format("%Y-%m-%dT%H:%M:%S+00:00")));

    let mut args: Vec<&str> = vec!["log", "--numstat", "-M", "-z", &format];
    if let Some(since) = since.as_deref() {
        args.push(since);
    }
    if let Some(r) = range {
        args.push(r);
    }

    let out = git(root, &args).context("running `git log --numstat -M -z`")?;
    anyhow::ensure!(
        out.status.success(),
        "`git log --numstat -M -z` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calendar-aware cutoff lands on the same day-of-month one window back,
    /// in UTC — and is a pure function of the HEAD timestamp (no wall clock).
    #[test]
    fn window_cutoff_is_calendar_aware_and_pure() {
        // 2026-06-11T00:00:00Z.
        let head = DateTime::parse_from_rfc3339("2026-06-11T00:00:00Z")
            .unwrap()
            .timestamp();
        let cutoff = window_cutoff(head, 12).expect("representable");
        let expected = DateTime::parse_from_rfc3339("2025-06-11T00:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(cutoff, expected, "12 calendar months back, not 365 days");

        // Determinism: same inputs, same output, every time.
        assert_eq!(window_cutoff(head, 12), window_cutoff(head, 12));
        // A one-month window off a 31st clamps to the shorter month's end
        // (chrono's documented saturating behaviour) — still deterministic.
        let jan31 = DateTime::parse_from_rfc3339("2026-01-31T00:00:00Z")
            .unwrap()
            .timestamp();
        let back = window_cutoff(jan31, 1).unwrap();
        let dec_back = DateTime::from_timestamp(back, 0).unwrap();
        assert_eq!(
            dec_back.format("%Y-%m-%d").to_string(),
            "2025-12-31",
            "Jan 31 − 1mo clamps deterministically to Dec 31"
        );
    }

    /// Boundary behaviour the caller relies on: an out-of-range timestamp yields
    /// `None` (the caller treats `None` as "no lower bound"), and a zero-month
    /// window is a no-op (the config validator forbids 0, but the pure function
    /// must never panic if reached).
    #[test]
    fn window_cutoff_edge_cases() {
        assert!(
            window_cutoff(i64::MIN, 12).is_none(),
            "an unrepresentable timestamp returns None, never panics"
        );
        let ts = DateTime::parse_from_rfc3339("2026-06-11T00:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            window_cutoff(ts, 0),
            Some(ts),
            "a zero-month window subtracts nothing (no panic)"
        );
    }

    /// A non-rename `-z` block parses into a commit plus its file facts, with the
    /// binary `-` marker mapped to `None` and the subject captured. In `-z` each
    /// numstat path is `NUL`-terminated (no inter-entry newlines).
    #[test]
    fn parse_plain_block() {
        let raw = format!(
            "{RS}{sha}{US}1700000000{US}Ada Lovelace{US}ada@example.com{US}fix: tidy lib\n\
             10\t2\tsrc/lib.rs{NUL}-\t-\tassets/logo.png{NUL}{NUL}",
            sha = "a".repeat(40),
        );
        let parsed = parse_numstat_log(&raw);
        assert_eq!(parsed.len(), 1);
        let (commit, files) = &parsed[0];
        assert_eq!(commit.sha, "a".repeat(40));
        assert_eq!(commit.committed_at, 1_700_000_000);
        assert_eq!(commit.author_name, "Ada Lovelace");
        assert_eq!(commit.author_email, "ada@example.com");
        assert_eq!(
            commit.subject, "fix: tidy lib",
            "the %s subject is captured"
        );
        assert_eq!(commit.file_count, 2);
        assert_eq!(files[0].added, Some(10));
        assert_eq!(files[0].deleted, Some(2));
        assert_eq!(files[0].path, "src/lib.rs");
        assert_eq!(files[1].added, None, "binary file → None");
        assert_eq!(files[1].deleted, None);
        assert_eq!(files[1].path, "assets/logo.png");
    }

    /// Multiple commits, including one with no file changes (a merge), parse in
    /// order; the empty-diff commit yields `file_count = 0`.
    #[test]
    fn parse_multiple_blocks_including_empty() {
        let raw = format!(
            "{RS}{a}{US}200{US}Bo{US}bo@x{US}work\n\
             1\t1\ta.rs{NUL}{NUL}\
             {RS}{b}{US}100{US}Cy{US}cy@x{US}merge\n",
            a = "a".repeat(40),
            b = "b".repeat(40),
        );
        let parsed = parse_numstat_log(&raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0.file_count, 1);
        assert_eq!(
            parsed[1].0.file_count, 0,
            "an empty-diff commit has no files"
        );
        assert_eq!(parsed[1].1.len(), 0);
    }

    /// A `-z` rename entry — empty path field then `NUL old NUL new NUL` — threads
    /// the old path through to the file fact, alongside an ordinary change in the
    /// same commit (the rename consumes three `NUL` cells, the change one).
    #[test]
    fn parse_block_with_rename() {
        let raw = format!(
            "{RS}{sha}{US}300{US}Di{US}di@x{US}refactor\n\
             1\t0\tsrc/keep.rs{NUL}\
             4\t1\t{NUL}src/old.rs{NUL}src/new.rs{NUL}{NUL}",
            sha = "c".repeat(40),
        );
        let parsed = parse_numstat_log(&raw);
        let (commit, files) = &parsed[0];
        assert_eq!(
            files.len(),
            2,
            "the normal change and the rename both parse"
        );
        assert_eq!(
            commit.file_count, 2,
            "a rename counts as one touched file toward file_count (the cap input)"
        );
        assert_eq!(files[0].path, "src/keep.rs");
        assert_eq!(files[0].old_path, None);
        assert_eq!(files[1].path, "src/new.rs");
        assert_eq!(files[1].old_path, Some("src/old.rs".to_string()));
        assert_eq!(files[1].added, Some(4));
    }

    /// A truncated rename trailer (the empty-path rename cell is the last cell,
    /// with no following old/new cells) is skipped — never a panic, never a
    /// fabricated half-rename. Exercises the defensive branch in
    /// [`parse_numstat_body`].
    #[test]
    fn parse_truncated_rename_trailer_is_skipped() {
        let raw = format!(
            "{RS}{sha}{US}300{US}Di{US}di@x{US}truncated\n\
             1\t0\tok.rs{NUL}\
             4\t1\t{NUL}",
            sha = "c".repeat(40),
        );
        let parsed = parse_numstat_log(&raw);
        let (_c, files) = &parsed[0];
        assert_eq!(files.len(), 1, "only the well-formed change survives");
        assert_eq!(files[0].path, "ok.rs");
    }

    /// Regression for the [S-046] deferred finding: a file whose path *literally*
    /// contains ` => ` is a plain (`NUL`-terminated) numstat path under `-z`, so
    /// it parses verbatim — never split into a phantom rename. This is the whole
    /// reason for the `-z` rewrite.
    ///
    /// [S-046]: ../../../docs/planning/journal.md#s-046-history-store-and-incremental-git-miner
    #[test]
    fn parse_literal_arrow_path_is_not_a_rename() {
        let raw = format!(
            "{RS}{sha}{US}400{US}Eve{US}eve@x{US}edit weird file\n\
             1\t1\tweird => name.txt{NUL}{NUL}",
            sha = "d".repeat(40),
        );
        let parsed = parse_numstat_log(&raw);
        let (_c, files) = &parsed[0];
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].path, "weird => name.txt",
            "a literal ` => ` in a filename is preserved verbatim"
        );
        assert_eq!(
            files[0].old_path, None,
            "it is NOT misread as a rename (the finding-#11 fix)"
        );
        assert_eq!(files[0].added, Some(1));
    }

    /// A no-diff commit (a merge) in `-z` ends with the commit-terminator `NUL`
    /// and no `\n` — the subject must not absorb that trailing `NUL`, and the
    /// commit parses with zero files.
    #[test]
    fn parse_merge_commit_header_only_with_trailing_nul() {
        let raw = format!(
            "{RS}{a}{US}500{US}Mo{US}mo@x{US}Merge branch 'topic'{NUL}\
             {RS}{b}{US}400{US}Mo{US}mo@x{US}work\n\
             2\t0\tfile.rs{NUL}{NUL}",
            a = "e".repeat(40),
            b = "f".repeat(40),
        );
        let parsed = parse_numstat_log(&raw);
        assert_eq!(parsed.len(), 2);
        // The merge: header-only, subject clean (no trailing NUL), zero files.
        assert_eq!(parsed[0].0.subject, "Merge branch 'topic'");
        assert_eq!(parsed[0].0.file_count, 0);
        assert!(parsed[0].1.is_empty());
        // The following ordinary commit still parses.
        assert_eq!(parsed[1].0.subject, "work");
        assert_eq!(parsed[1].1[0].path, "file.rs");
    }

    /// Empty git output parses to nothing (a degraded/empty mine, never a panic).
    #[test]
    fn parse_empty_output() {
        assert!(parse_numstat_log("").is_empty());
        assert!(parse_numstat_log("\n\n").is_empty());
    }

    /// A degraded outcome carries the reason and mines nothing.
    #[test]
    fn degraded_outcome_shape() {
        let o = MineOutcome::degraded(DegradedReason::Shallow);
        assert_eq!(o.commits_read, 0);
        assert!(!o.first_mine);
        assert_eq!(o.degraded, Some(DegradedReason::Shallow));
        assert!(o.degraded.unwrap().message().contains("shallow"));
    }
}
