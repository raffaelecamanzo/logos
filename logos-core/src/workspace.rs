//! Worktree-aware root resolution and seed-from-main discovery ([ADR-15],
//! S-021, [Git integration]).
//!
//! Logos follows the agent into a `git worktree`: `.logos/` resolves against
//! the **working-tree root** (`git rev-parse --show-toplevel` — in a linked
//! worktree that is *that worktree's* root, [FR-WT-01]), so each worktree owns
//! its own `logos.db` and the graph always describes the code the agent is
//! standing in ([NFR-CC-02]). Outside a git repo (or with no `git` on PATH)
//! resolution falls back to the caller's hint — cwd or `--project` — and
//! worktree features degrade gracefully (the [Git integration] failure table).
//!
//! On first use in a DB-less linked worktree, [`seed_source`] locates the
//! primary checkout's DB via `git rev-parse --git-common-dir` so the engine
//! can copy it as a seed and reconcile only the main↔branch diff
//! ([`diff_from_primary`]) — O(diff-from-main), not O(repo) ([FR-WT-03]).
//! No seed found → the caller falls back to a full index.
//!
//! Everything here shells out to the `git` CLI (sub-ms local subprocess, the
//! [Git integration] contract); no libgit2-style dependency enters the binary.
//!
//! [ADR-15]: ../../docs/specs/architecture/decisions/ADR-15.md
//! [Git integration]: ../../docs/specs/architecture/integrations/git.md
//! [FR-WT-01]: ../../docs/specs/requirements/FR-WT-01.md
//! [FR-WT-03]: ../../docs/specs/requirements/FR-WT-03.md
//! [NFR-CC-02]: ../../docs/specs/requirements/NFR-CC-02.md

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// The primary checkout a DB-less linked worktree can seed from ([FR-WT-03]).
///
/// Produced by [`seed_source`] only when all the preconditions hold: the root
/// is a *linked* worktree (not the primary checkout itself), the primary's
/// `.logos/logos.db` exists, and the primary's HEAD is resolvable (the
/// diff-reconcile base).
///
/// [FR-WT-03]: ../../docs/specs/requirements/FR-WT-03.md
#[derive(Debug, Clone)]
pub struct SeedSource {
    /// The primary checkout's working-tree root.
    pub primary_root: PathBuf,
    /// The primary checkout's `.logos/logos.db` (verified to exist).
    pub db_path: PathBuf,
    /// The primary checkout's HEAD commit — the base the seeded graph
    /// describes, and therefore the base of the diff-reconcile.
    pub head: String,
}

/// Resolve `hint` to the working-tree root via `git rev-parse --show-toplevel`
/// ([FR-WT-01], [NFR-CC-02]).
///
/// In a linked worktree this returns *that worktree's* root — the property the
/// whole per-worktree DB model relies on ([ADR-15]). When `hint` is not inside
/// a git repository, or `git` is absent from PATH, the hint is returned
/// verbatim (cwd / `--project` fallback — worktree features degrade, nothing
/// fails).
///
/// [ADR-15]: ../../docs/specs/architecture/decisions/ADR-15.md
/// [FR-WT-01]: ../../docs/specs/requirements/FR-WT-01.md
/// [NFR-CC-02]: ../../docs/specs/requirements/NFR-CC-02.md
pub fn resolve_root(hint: &Path) -> PathBuf {
    match git(hint, &["rev-parse", "--show-toplevel"]) {
        Ok(top) if !top.is_empty() => PathBuf::from(top),
        // Not a repo (non-zero exit) falls back silently; a MISSING git binary
        // degrades with a notice, per the Git-integration failure table.
        Err(err) if git_is_missing(&err) => {
            tracing::warn!(
                "`git` is not on PATH; worktree features degrade — using {} as the \
                 project root",
                hint.display()
            );
            hint.to_path_buf()
        }
        _ => hint.to_path_buf(),
    }
}

/// Is `path` itself the **top level** of a git working tree — not merely a
/// directory *inside* one ([FR-WS-01])?
///
/// Unlike [`resolve_root`] (which silently returns its hint outside git, so it
/// cannot tell "is a repo root" from "is not in a repo"), this answers the
/// distinct question federation member discovery needs: *is this exact
/// directory a repository root?* It is `true` only when `git rev-parse
/// --show-toplevel` succeeds **and** resolves back to `path` — a subdirectory
/// of a repo (toplevel is an ancestor), a non-repo directory, and a missing
/// `git` binary all return `false`. Paths are compared symlink-resolved
/// (`--show-toplevel` output is canonical; the hint may not be).
///
/// [FR-WS-01]: ../../docs/specs/requirements/FR-WS-01.md
pub fn is_git_root(path: &Path) -> bool {
    match git(path, &["rev-parse", "--show-toplevel"]) {
        Ok(top) if !top.is_empty() => {
            let top = PathBuf::from(top);
            match (top.canonicalize(), path.canonicalize()) {
                (Ok(a), Ok(b)) => a == b,
                _ => top == path,
            }
        }
        _ => false,
    }
}

/// Was this [`git`] error a failure to *spawn* the binary (git absent from
/// PATH), as opposed to git running and exiting non-zero?
fn git_is_missing(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// The **primary** checkout's working-tree root, as seen from `root`
/// ([ADR-15]).
///
/// Resolves the repository's common `.git` directory via `git rev-parse
/// --git-common-dir` and returns its parent — the primary working tree that a
/// linked worktree shares. This is the anchor for any *repo-global* state that
/// must outlive an individual worktree: the seed-from-main DB ([`seed_source`])
/// and the shared telemetry store ([ADR-50], [FR-OB-07]).
///
/// Returns `None` — "no distinct primary; treat `root` as its own repo" — when:
/// `root` is not in a git repo or `git` is absent from PATH; the common dir is
/// not a `<primary>/.git` directory (a bare repo's worktree has no primary
/// working tree); or `root` **is** the primary checkout (no linked-worktree
/// indirection). Callers use `None` to fall back to the local `root`.
///
/// [ADR-15]: ../../docs/specs/architecture/decisions/ADR-15.md
/// [ADR-50]: ../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-07]: ../../docs/specs/requirements/FR-OB-07.md
pub fn primary_root(root: &Path) -> Option<PathBuf> {
    let common = git(root, &["rev-parse", "--git-common-dir"]).ok()?;
    if common.is_empty() {
        return None;
    }
    // The common dir may come back relative (".git" in the primary checkout);
    // anchor it where the git call ran.
    let common = {
        let p = PathBuf::from(common);
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    };
    // The primary working tree sits around its `.git` directory. A common dir
    // not named `.git` (a bare repo's worktree) has no primary checkout.
    if common.file_name().is_none_or(|n| n != ".git") {
        return None;
    }
    let primary_root = common.parent()?.to_path_buf();

    // The primary checkout has no distinct primary to point at. Compare real
    // paths — `--git-common-dir` output is symlink-resolved, the hint may not
    // be.
    let same_tree = match (root.canonicalize(), primary_root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => root == primary_root,
    };
    if same_tree {
        return None;
    }
    Some(primary_root)
}

/// The current branch name at `root` via `git rev-parse --abbrev-ref HEAD`
/// ([ADR-50], [FR-OB-08]).
///
/// Used to stamp the telemetry `origin` of a linked worktree with the
/// development increment it builds. Returns `None` — "no nameable branch" —
/// when `root` is not in a git repo, `git` is absent from PATH, or HEAD is
/// detached (`git` prints the literal `HEAD`); callers fall back to `"main"`.
///
/// [ADR-50]: ../../docs/specs/architecture/decisions/ADR-50.md
/// [FR-OB-08]: ../../docs/specs/requirements/FR-OB-08.md
pub fn current_branch(root: &Path) -> Option<String> {
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    // Empty (unborn branch) or the literal "HEAD" (detached) is not a usable
    // increment name — let the caller default to "main".
    if branch.is_empty() || branch == "HEAD" {
        return None;
    }
    Some(branch)
}

/// Locate the primary checkout's DB to seed a DB-less worktree from
/// ([FR-WT-03], [ADR-15]).
///
/// Returns `None` — meaning "no seed, fall back to a full index" — when any
/// link in the chain is missing: there is no distinct primary checkout
/// ([`primary_root`] is `None` — not a git repo / no `git` binary, a bare
/// repo's worktree, or `root` *is* the primary), the primary has no
/// `.logos/logos.db`, or its HEAD cannot be resolved.
///
/// [ADR-15]: ../../docs/specs/architecture/decisions/ADR-15.md
/// [FR-WT-03]: ../../docs/specs/requirements/FR-WT-03.md
pub fn seed_source(root: &Path) -> Option<SeedSource> {
    let primary_root = primary_root(root)?;

    let db_path = primary_root.join(".logos").join("logos.db");
    if !db_path.is_file() {
        return None; // no seed → full index (ADR-15 fallback)
    }
    // The primary's HEAD is the state its DB (at best) describes — the
    // diff-reconcile base. Unresolvable HEAD (unborn branch) → no seed.
    let head = git(&primary_root, &["rev-parse", "HEAD"]).ok()?;
    Some(SeedSource {
        primary_root,
        db_path,
        head,
    })
}

/// The paths that differ between the primary checkout's `base` commit and this
/// worktree's current state — the O(diff-from-main) reconcile set
/// ([FR-WT-03]).
///
/// Covers, in one set: commits on the worktree's branch since `base`,
/// uncommitted working-tree edits and deletions (`git diff --name-only
/// --no-renames` against `base`), and untracked files (`git ls-files --others
/// --exclude-standard`). Paths are root-relative, exactly as
/// [`crate::pipeline::sync`] expects. Rename detection is disabled so a rename
/// surfaces as delete + add — both sides must be re-synced.
///
/// # Errors
/// Returns an error if `git` cannot be invoked or exits non-zero (e.g. `base`
/// is unknown in this worktree) — the caller degrades to the reconcile
/// backstop rather than guessing.
///
/// [FR-WT-03]: ../../docs/specs/requirements/FR-WT-03.md
pub fn diff_from_primary(root: &Path, base: &str) -> Result<Vec<PathBuf>> {
    // `-z` (NUL-separated, unquoted) keeps non-ASCII and space-bearing paths
    // intact; `--no-renames` makes a rename surface as delete + add.
    let tracked = git(
        root,
        &["diff", "--name-only", "--no-renames", "-z", base, "--"],
    )?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;

    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for rel in tracked.split('\0').chain(untracked.split('\0')) {
        if !rel.is_empty() && seen.insert(rel.to_string()) {
            paths.push(PathBuf::from(rel));
        }
    }
    Ok(paths)
}

/// Run `git -C <cwd> <args…>` and capture trimmed stdout.
///
/// # Errors
/// Returns an error if the binary cannot be spawned (git absent) or exits
/// non-zero (not a repo, unknown revision, …), with stderr in the message.
fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("invoking `git {}` (is git on PATH?)", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`git {}` failed in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use tempfile::TempDir;

    /// Run a git command in `cwd`, panicking on failure — fixtures only.
    fn sh_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            // Identity for commits; no reliance on the host's gitconfig.
            .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A committed repo at `<tmp>/main` with one source file, plus an empty
    /// sibling slot for worktrees. Returns (tmp, primary_root).
    fn repo_fixture() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().expect("temp root");
        let main = tmp.path().join("main");
        fs::create_dir_all(main.join("src")).unwrap();
        fs::write(main.join("src/lib.rs"), "pub fn seeded() {}\n").unwrap();
        sh_git(&main, &["init", "-q", "-b", "main"]);
        sh_git(&main, &["add", "."]);
        sh_git(&main, &["commit", "-q", "-m", "initial"]);
        (tmp, main)
    }

    /// Add a linked worktree at `<tmp>/wt` on a new branch and return its root.
    fn add_worktree(tmp: &TempDir, main: &Path) -> PathBuf {
        let wt = tmp.path().join("wt");
        sh_git(
            main,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );
        wt
    }

    // ── resolve_root (FR-WT-01) ───────────────────────────────────────────

    /// Outside any git repo the hint is returned verbatim — the cwd /
    /// `--project` fallback of the Git-integration failure table.
    #[test]
    fn resolve_root_falls_back_to_the_hint_outside_git() {
        let tmp = TempDir::new().unwrap();
        let plain = tmp.path().join("not-a-repo");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(resolve_root(&plain), plain);
        // A nonexistent hint degrades the same way (git fails, hint wins).
        let ghost = tmp.path().join("ghost");
        assert_eq!(resolve_root(&ghost), ghost);
    }

    // ── is_git_root (FR-WS-01) ────────────────────────────────────────────

    /// A repository's top-level directory is a git root; a subdirectory of it
    /// is not (its toplevel is an ancestor), and neither is a plain non-repo
    /// directory or a nonexistent path.
    #[test]
    fn is_git_root_distinguishes_a_repo_root_from_everything_else() {
        let (_tmp, main) = repo_fixture();
        fs::create_dir_all(main.join("src")).unwrap();
        assert!(is_git_root(&main), "the repo toplevel is a git root");
        assert!(
            !is_git_root(&main.join("src")),
            "a subdir of a repo is not itself a root"
        );

        let plain = TempDir::new().unwrap();
        assert!(
            !is_git_root(plain.path()),
            "a directory outside any repo is not a git root"
        );
        assert!(
            !is_git_root(&plain.path().join("ghost")),
            "a nonexistent path is not a git root"
        );
    }

    /// Inside a repo, any subdirectory resolves to the repo's toplevel.
    #[test]
    fn resolve_root_finds_the_toplevel_from_a_subdirectory() {
        let (_tmp, main) = repo_fixture();
        let resolved = resolve_root(&main.join("src"));
        assert_eq!(
            resolved.canonicalize().unwrap(),
            main.canonicalize().unwrap(),
            "a subdir resolves to the working-tree root"
        );
    }

    /// In a linked worktree, resolution lands on the WORKTREE's root, not the
    /// primary checkout's — the property ADR-15 relies on (NFR-CC-02).
    #[test]
    fn resolve_root_in_a_linked_worktree_is_the_worktree_root() {
        let (tmp, main) = repo_fixture();
        let wt = add_worktree(&tmp, &main);
        let resolved = resolve_root(&wt.join("src"));
        assert_eq!(
            resolved.canonicalize().unwrap(),
            wt.canonicalize().unwrap(),
            "the worktree resolves to its own root, never main's"
        );
    }

    /// A bare repo at `<tmp>/bare.git` plus a linked worktree at `<tmp>/wt`.
    /// A bare repo has no primary working tree, so its common dir is the bare
    /// repo itself (not a `<primary>/.git`). Returns the worktree root.
    fn bare_repo_worktree(tmp: &TempDir) -> PathBuf {
        let bare = tmp.path().join("bare.git");
        sh_git(tmp.path(), &["init", "-q", "--bare", "-b", "main", "bare.git"]);
        // A bare repo needs a commit before a worktree can be added: seed one
        // through a throwaway clone, then add the worktree off the bare repo.
        let seed = tmp.path().join("seed");
        sh_git(tmp.path(), &["clone", "-q", bare.to_str().unwrap(), "seed"]);
        fs::write(seed.join("f.txt"), "x\n").unwrap();
        sh_git(&seed, &["add", "."]);
        sh_git(&seed, &["commit", "-q", "-m", "seed"]);
        sh_git(&seed, &["push", "-q", "origin", "main"]);

        let wt = tmp.path().join("wt");
        sh_git(
            &bare,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "main"],
        );
        wt
    }

    // ── primary_root (ADR-15, ADR-50) ─────────────────────────────────────

    /// From a linked worktree, `primary_root` resolves the PRIMARY checkout's
    /// root — the anchor the shared telemetry store (ADR-50) and the seed
    /// (ADR-15) both hang off.
    #[test]
    fn primary_root_from_a_worktree_is_the_primary_checkout() {
        let (tmp, main) = repo_fixture();
        let wt = add_worktree(&tmp, &main);
        let primary = primary_root(&wt).expect("a linked worktree has a primary");
        assert_eq!(
            primary.canonicalize().unwrap(),
            main.canonicalize().unwrap(),
            "the worktree points back at main, never at itself"
        );
    }

    /// The primary checkout has no distinct primary to point at — `None`, so
    /// callers fall back to the local root (identity resolution).
    #[test]
    fn primary_root_is_none_in_the_primary_checkout() {
        let (_tmp, main) = repo_fixture();
        assert!(primary_root(&main).is_none());
    }

    /// A bare repo's worktree has no primary working tree (its common dir is
    /// the bare repo, not a `<primary>/.git`) — `None`.
    #[test]
    fn primary_root_is_none_for_a_bare_repo_worktree() {
        let tmp = TempDir::new().unwrap();
        let wt = bare_repo_worktree(&tmp);
        assert!(primary_root(&wt).is_none());
    }

    /// Outside any git repo there is no primary (and no panic).
    #[test]
    fn primary_root_is_none_outside_git() {
        let tmp = TempDir::new().unwrap();
        assert!(primary_root(tmp.path()).is_none());
    }

    // ── current_branch (ADR-50, FR-OB-08) ─────────────────────────────────

    /// The primary checkout is on `main` (the fixture inits `-b main`) — the
    /// branch that names the telemetry `origin` there.
    #[test]
    fn current_branch_in_the_primary_is_main() {
        let (_tmp, main) = repo_fixture();
        assert_eq!(current_branch(&main).as_deref(), Some("main"));
    }

    /// A linked worktree reports its own branch — the increment its events are
    /// attributed to (FR-OB-08). `add_worktree` creates it on `feature`.
    #[test]
    fn current_branch_in_a_worktree_is_its_branch() {
        let (tmp, main) = repo_fixture();
        let wt = add_worktree(&tmp, &main);
        assert_eq!(current_branch(&wt).as_deref(), Some("feature"));
    }

    /// A detached HEAD has no nameable branch — `None`, so the caller defaults
    /// to `"main"` rather than recording the literal `HEAD`.
    #[test]
    fn current_branch_is_none_when_detached() {
        let (_tmp, main) = repo_fixture();
        let head = resolve_head(&main);
        sh_git(&main, &["checkout", "-q", &head]);
        assert!(current_branch(&main).is_none());
    }

    /// Outside any git repo there is no branch (and no panic).
    #[test]
    fn current_branch_is_none_outside_git() {
        let tmp = TempDir::new().unwrap();
        assert!(current_branch(tmp.path()).is_none());
    }

    // ── seed_source (FR-WT-03) ────────────────────────────────────────────

    /// The primary checkout never seeds from itself.
    #[test]
    fn seed_source_is_none_in_the_primary_checkout() {
        let (_tmp, main) = repo_fixture();
        fs::create_dir_all(main.join(".logos")).unwrap();
        fs::write(main.join(".logos/logos.db"), b"db").unwrap();
        assert!(seed_source(&main).is_none());
    }

    /// A linked worktree with no primary DB has nothing to seed from — the
    /// caller falls back to a full index.
    #[test]
    fn seed_source_is_none_without_a_primary_db() {
        let (tmp, main) = repo_fixture();
        let wt = add_worktree(&tmp, &main);
        assert!(seed_source(&wt).is_none());
    }

    /// A linked worktree with a primary DB resolves the seed: the primary's
    /// DB path and its HEAD as the diff base.
    #[test]
    fn seed_source_finds_the_primary_db_from_a_worktree() {
        let (tmp, main) = repo_fixture();
        fs::create_dir_all(main.join(".logos")).unwrap();
        fs::write(main.join(".logos/logos.db"), b"db").unwrap();
        let wt = add_worktree(&tmp, &main);

        let seed = seed_source(&wt).expect("worktree + primary DB → a seed");
        assert_eq!(
            seed.primary_root.canonicalize().unwrap(),
            main.canonicalize().unwrap()
        );
        assert_eq!(
            seed.db_path.canonicalize().unwrap(),
            main.join(".logos/logos.db").canonicalize().unwrap()
        );
        assert_eq!(seed.head.len(), 40, "HEAD is a full commit id");
    }

    /// Outside git there is no seed (and no panic).
    #[test]
    fn seed_source_is_none_outside_git() {
        let tmp = TempDir::new().unwrap();
        assert!(seed_source(tmp.path()).is_none());
    }

    // ── diff_from_primary (FR-WT-03) ──────────────────────────────────────

    /// The diff set covers the three change shapes in one list: a committed
    /// branch edit, an uncommitted working-tree edit, and an untracked file —
    /// while untouched files stay out of it.
    #[test]
    fn diff_from_primary_lists_committed_dirty_and_untracked_changes() {
        let (tmp, main) = repo_fixture();
        // Tracked files the worktree will NOT touch / WILL delete.
        fs::write(main.join("src/stable.rs"), "pub fn stable() {}\n").unwrap();
        fs::write(main.join("src/doomed.rs"), "pub fn doomed() {}\n").unwrap();
        sh_git(&main, &["add", "."]);
        sh_git(&main, &["commit", "-q", "-m", "stable + doomed files"]);
        let base = resolve_head(&main);
        let wt = add_worktree(&tmp, &main);

        // Committed on the branch…
        fs::write(wt.join("src/lib.rs"), "pub fn seeded_v2() {}\n").unwrap();
        sh_git(&wt, &["add", "."]);
        sh_git(&wt, &["commit", "-q", "-m", "branch work"]);
        // …dirty in the working tree…
        fs::write(wt.join("src/dirty.rs"), "pub fn dirty() {}\n").unwrap();
        sh_git(&wt, &["add", "src/dirty.rs"]);
        sh_git(&wt, &["commit", "-q", "-m", "add dirty"]);
        fs::write(wt.join("src/dirty.rs"), "pub fn dirty_v2() {}\n").unwrap();
        // …untracked…
        fs::write(wt.join("src/fresh.rs"), "pub fn fresh() {}\n").unwrap();
        // …and deleted on the branch (drives a graph removal in the sync).
        sh_git(&wt, &["rm", "-q", "src/doomed.rs"]);

        let paths = diff_from_primary(&wt, &base).expect("diff runs");
        let set: Vec<&str> = paths.iter().filter_map(|p| p.to_str()).collect();
        assert!(
            set.contains(&"src/lib.rs"),
            "committed edit listed: {set:?}"
        );
        assert!(set.contains(&"src/dirty.rs"), "dirty edit listed: {set:?}");
        assert!(set.contains(&"src/fresh.rs"), "untracked listed: {set:?}");
        assert!(
            set.contains(&"src/doomed.rs"),
            "a deleted tracked file is listed so its nodes get removed: {set:?}"
        );
        assert!(
            !set.contains(&"src/stable.rs"),
            "untouched files stay out of the O(diff) set: {set:?}"
        );
    }

    /// An unknown base is an error (never a silent empty diff) — the caller
    /// degrades to the reconcile backstop.
    #[test]
    fn diff_from_primary_rejects_an_unknown_base() {
        let (_tmp, main) = repo_fixture();
        let bogus = "0123456789abcdef0123456789abcdef01234567";
        assert!(diff_from_primary(&main, bogus).is_err());
    }

    /// Fixture helper: the repo's current HEAD commit id.
    fn resolve_head(root: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
}
