//! Discovery (Pass 0) — the contained, gitignore-aware project walk ([FR-IX-02]).
//!
//! [`discover`] composes the **union of three exclusion sources** ([FR-CF-02]):
//!
//! 1. **gitignore** — via [`ignore::WalkBuilder`] (`.gitignore`, `.git/info/exclude`,
//!    and `.ignore` files), honoured even outside a git repo (`require_git(false)`).
//! 2. **`ignored_dirs`** — directory *names* pruned anywhere in the tree, applied
//!    as a `filter_entry` so whole subtrees (`target/`, `node_modules/`) are never
//!    descended.
//! 3. **config `exclude` globs** — applied as a post-filter on the root-relative path.
//!
//! A positive **`include`** glob set then keeps only matching files (default
//! `**` = everything), and files above `max_file_size` are skipped with a notice
//! ([FR-CF-04]).
//!
//! # Containment ([NFR-SE-04])
//! The walk is rooted at the canonicalised project root with `follow_links(false)`,
//! so it never follows a symlink out of the tree, and every yielded path is
//! re-checked with `starts_with(root)` as defence in depth. This is the
//! enforcement point documented in [`docs/security/trusted-input-boundary.md`].
//!
//! [FR-IX-02]: ../../../../docs/specs/requirements/FR-IX-02.md
//! [FR-CF-02]: ../../../../docs/specs/requirements/FR-CF-02.md
//! [FR-CF-04]: ../../../../docs/specs/requirements/FR-CF-04.md
//! [NFR-SE-04]: ../../../../docs/specs/requirements/NFR-SE-04.md
//! [`docs/security/trusted-input-boundary.md`]: ../../../../docs/security/trusted-input-boundary.md

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use ignore::{WalkBuilder, WalkState};

use super::error::ConfigError;
use super::globs;
use super::settings::Config;

/// A file skipped during discovery because it exceeds `max_file_size` ([FR-CF-04]).
///
/// Carries the data for the logged notice; [`fmt::Display`] renders the notice
/// line so the surface can emit it verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OversizeSkip {
    /// The skipped file (absolute path within the project root).
    pub path: PathBuf,
    /// The file's size in bytes.
    pub size: u64,
    /// The configured `max_file_size` it exceeded.
    pub max: u64,
}

impl fmt::Display for OversizeSkip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "skipping {}: {} bytes exceeds max_file_size {} bytes",
            self.path.display(),
            self.size,
            self.max
        )
    }
}

/// The result of a discovery walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Files to index, sorted, each an absolute path within the project root.
    pub files: Vec<PathBuf>,
    /// Files skipped for exceeding `max_file_size`, sorted by path ([FR-CF-04]).
    pub skipped_oversize: Vec<OversizeSkip>,
}

impl DiscoveryReport {
    /// The human-readable oversize notices, one per skipped file ([FR-CF-04]).
    ///
    /// The surface logs these to stderr; keeping the text here (not at the
    /// emission site) lets the core own and test the notice contract.
    pub fn notices(&self) -> impl Iterator<Item = String> + '_ {
        self.skipped_oversize.iter().map(ToString::to_string)
    }
}

/// Walk `root` and return the files to index under `config` ([FR-IX-02]).
///
/// The walk runs in parallel across cores ([FR-IX-09], [NFR-PE-08]) via
/// [`ignore`]'s `build_parallel`, using an auto-sized worker count. Output is
/// order-deterministic regardless of that count ([NFR-RA-06]) — see
/// [`discover_with_threads`].
///
/// # Errors
/// - [`ConfigError::InvalidRoot`] if `root` is missing or not a directory.
/// - [`ConfigError::EscapingPattern`] / [`ConfigError::BadGlob`] if an
///   include/exclude glob escapes the root or fails to compile (exit 2).
pub fn discover(root: &Path, config: &Config) -> Result<DiscoveryReport, ConfigError> {
    // 0 lets `ignore` pick the worker count from available parallelism — the
    // core-scaling default (NFR-PE-08). Tests pin an explicit count to prove the
    // output is thread-count-independent (NFR-RA-06).
    discover_with_threads(root, config, 0)
}

/// Discovery with an explicit walker thread count ([FR-IX-09]).
///
/// `threads == 0` auto-sizes to the host's parallelism (the production path);
/// `threads == 1` is the serial-equivalent walk. The candidate set, the
/// per-file oversize skips, and their sorted order are **identical for every
/// `threads` value** ([NFR-RA-06]) — the parallel walk visits the same entries
/// as the serial one and applies byte-identical admission logic; only the
/// visitation order differs, and the terminal `sort` erases that. Exposed for
/// the determinism/stress tests that assert this across several worker counts.
pub(crate) fn discover_with_threads(
    root: &Path,
    config: &Config,
    threads: usize,
) -> Result<DiscoveryReport, ConfigError> {
    // Canonicalise once: the anchor for every containment check (NFR-SE-04).
    let root = root.canonicalize().map_err(|_| ConfigError::InvalidRoot {
        path: root.to_path_buf(),
    })?;
    if !root.is_dir() {
        return Err(ConfigError::InvalidRoot { path: root });
    }

    let include = globs::compile(&config.include)?;
    let exclude = globs::compile(&config.exclude)?;
    let ignored_dirs: HashSet<String> = config.semantics.ignored_dirs.iter().cloned().collect();
    let max_file_size = config.max_file_size;

    let walker = WalkBuilder::new(&root)
        // Honour ignore files even outside a git repo, so discovery is
        // deterministic on any tree (worktrees are git-backed; fixtures need not be).
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .ignore(true)
        .hidden(false) // dotfiles are visible unless gitignored — config decides.
        .parents(false) // don't read ignore files above the root (containment).
        .follow_links(false) // never leave the tree via a symlink (NFR-SE-04).
        .threads(threads) // 0 = auto-size to cores (NFR-PE-08); tests pin a count.
        .filter_entry(move |entry| {
            // Directory pruning — never the root itself (depth 0), so a project dir
            // named e.g. "build", or the root's own `.git`, still walks.
            if entry.depth() > 0 && entry.file_type().is_some_and(|ft| ft.is_dir()) {
                // Prune any nested git boundary — a linked worktree (`.git` gitlink
                // file), a vendored repository (`.git` directory), or a submodule.
                // Logos indexes the one root project; a nested git tree is a
                // separate working tree the root `.gitignore` cannot reliably mask
                // (the `ignore` crate treats it as its own repo boundary and applies
                // *its* ignore rules), so folding it in double-counts symbols. This
                // is what let a `.worktrees/<sprint>/…` checkout be indexed despite
                // `.worktrees/` being gitignored.
                if entry.path().join(".git").exists() {
                    return false;
                }
                // Prune directories whose name is in `ignored_dirs`.
                if let Some(name) = entry.file_name().to_str() {
                    return !ignored_dirs.contains(name);
                }
            }
            true
        })
        .build_parallel();

    // Each walker thread owns a cloned `Sender`; admitted entries and oversize
    // skips flow back over the channel and are merged (and sorted) on this
    // thread. The per-entry admission logic below is identical to the serial
    // walk — parallelism only changes *when* each entry is seen, never *whether*
    // it is admitted (NFR-RA-06).
    let (tx, rx) = mpsc::channel::<Found>();
    let root_ref: &Path = &root;
    let include_ref = &include;
    let exclude_ref = &exclude;
    walker.run(|| {
        let tx = tx.clone();
        Box::new(move |result| {
            // A per-entry error (e.g. an unreadable directory) skips that entry
            // without aborting the walk — discovery is best-effort over the tree.
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => return WalkState::Continue,
            };

            let file_type = match entry.file_type() {
                Some(ft) => ft,
                None => return WalkState::Continue, // stdin — never from a path walk.
            };
            // Skip symlinks explicitly (NFR-SE-04): with follow_links(false) a
            // symlink is yielded as-is, never traversed. Asserting `!is_symlink()`
            // here makes the containment guarantee local and auditable rather than
            // implicit in the walker's type resolution — a symlinked file is never
            // indexed.
            if file_type.is_symlink() || !file_type.is_file() {
                return WalkState::Continue;
            }

            let path = entry.path();
            // Defence in depth: with follow_links(false) this always holds, but a
            // belt-and-braces check guarantees no path escaped the root (NFR-SE-04).
            let rel = match path.strip_prefix(root_ref) {
                Ok(rel) => rel,
                Err(_) => return WalkState::Continue,
            };

            if exclude_ref.is_match(rel) || !include_ref.is_match(rel) {
                return WalkState::Continue;
            }

            // A file we cannot stat is skipped (same best-effort policy as a walk
            // error) rather than treated as size 0 — never index an unreadable path.
            let size = match entry.metadata() {
                Ok(meta) => meta.len(),
                Err(_) => return WalkState::Continue,
            };
            let found = if size > max_file_size {
                Found::Oversize(OversizeSkip {
                    path: path.to_path_buf(),
                    size,
                    max: max_file_size,
                })
            } else {
                Found::File(path.to_path_buf())
            };
            // The receiver lives until every visitor drops, so a send failure is
            // impossible here; ignore the result rather than unwrap in a hot loop.
            let _ = tx.send(found);
            WalkState::Continue
        })
    });
    // Drop the template sender so `rx` terminates once every thread's clone is
    // gone (`run` has already joined the threads by the time it returns).
    drop(tx);

    let mut files = Vec::new();
    let mut skipped_oversize = Vec::new();
    for found in rx {
        match found {
            Found::File(path) => files.push(path),
            Found::Oversize(skip) => skipped_oversize.push(skip),
        }
    }

    // Deterministic ordering — the parallel walk order is not stable, so the sort
    // is what makes the report thread-count-independent (NFR-RA-06).
    files.sort();
    skipped_oversize.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(DiscoveryReport {
        files,
        skipped_oversize,
    })
}

/// An admitted entry observed by a walker thread, sent back for merging.
enum Found {
    /// A file to index (within `max_file_size`).
    File(PathBuf),
    /// A file skipped for exceeding `max_file_size` ([FR-CF-04]).
    Oversize(OversizeSkip),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The worker counts every determinism assertion sweeps: `1` is the
    /// serial-equivalent walk, `0` is the production auto-sized default, and
    /// `2/4/8` force multi-threaded traversal (FR-IX-09, NFR-RA-06).
    const WORKER_COUNTS: &[usize] = &[1, 2, 4, 8, 0];

    /// A permissive config: include everything, no excludes, and only the two
    /// `ignored_dirs` names the fixtures use — so a test admits its whole tree
    /// except what it deliberately gates.
    fn test_config(max_file_size: u64) -> Config {
        let mut config = Config {
            exclude: vec![],
            max_file_size,
            ..Config::default()
        };
        config.semantics.ignored_dirs = vec!["target".to_string(), ".git".to_string()];
        config
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    /// Build a multi-file, multi-directory fixture large enough that a parallel
    /// walk genuinely spreads across threads. Returns the temp guard (kept alive
    /// by the caller) and the canonical root.
    fn build_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        // A spread of small source files across nested directories.
        for d in 0..8 {
            for f in 0..16 {
                write(&root.join(format!("src/d{d}/f{f}.rs")), "pub fn f() {}\n");
            }
        }
        // A file pruned by `ignored_dirs` (never admitted).
        write(&root.join("target/generated.rs"), "pub fn gen() {}\n");
        // A file pruned by a nested git boundary (never admitted).
        write(&root.join("vendored/.git"), "gitdir: /elsewhere\n");
        write(&root.join("vendored/lib.rs"), "pub fn v() {}\n");
        (tmp, root)
    }

    #[test]
    fn discovery_output_is_identical_across_worker_counts() {
        // FR-IX-09 / NFR-RA-06: the parallel walk yields the same candidate set —
        // and the same sorted order — for every worker count, including the serial
        // (threads = 1) and auto-sized (threads = 0) walks.
        let (_tmp, root) = build_fixture();
        let config = test_config(1 << 20);

        let baseline = discover_with_threads(&root, &config, WORKER_COUNTS[0]).unwrap();
        assert_eq!(baseline.files.len(), 8 * 16, "every source file is discovered");
        // Negative membership, not just the count: a regression that wrongly
        // admitted a pruned tree while dropping a `src/` file would preserve the
        // 128 total and slip past a count-only check. Assert the `ignored_dirs`
        // and nested-`.git` prunes directly (NFR-SE-04 containment).
        assert!(
            !baseline.files.iter().any(|p| p.starts_with(root.join("target"))),
            "the `ignored_dirs` name `target` is pruned, not merely count-compensated"
        );
        assert!(
            !baseline.files.iter().any(|p| p.starts_with(root.join("vendored"))),
            "the nested `.git` boundary `vendored/` is pruned (NFR-SE-04)"
        );
        // The candidate set is sorted regardless of worker count.
        let mut sorted = baseline.files.clone();
        sorted.sort();
        assert_eq!(baseline.files, sorted, "the candidate set is deterministically ordered");

        for &n in &WORKER_COUNTS[1..] {
            let report = discover_with_threads(&root, &config, n).unwrap();
            assert_eq!(
                report, baseline,
                "worker count {n} changed the discovery report (NFR-RA-06)"
            );
        }
    }

    #[test]
    fn oversize_skips_are_identical_across_worker_counts() {
        // The `skipped_oversize` set and its sorted order must also be
        // thread-count-independent (FR-CF-04, NFR-RA-06).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let max = 64u64;
        // Several oversize files interleaved with admitted ones.
        for i in 0..6 {
            write(&root.join(format!("small{i}.rs")), "fn s() {}\n"); // < 64 bytes
            write(
                &root.join(format!("big{i}.rs")),
                &"x".repeat((max as usize) + 1 + i), // > 64 bytes
            );
        }
        let config = test_config(max);

        let baseline = discover_with_threads(&root, &config, 1).unwrap();
        assert_eq!(baseline.files.len(), 6, "the six small files are admitted");
        assert_eq!(baseline.skipped_oversize.len(), 6, "the six big files are skipped");
        let mut sorted = baseline.skipped_oversize.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(baseline.skipped_oversize, sorted, "oversize skips are sorted");

        for &n in &[2usize, 4, 8, 0] {
            let report = discover_with_threads(&root, &config, n).unwrap();
            assert_eq!(
                report, baseline,
                "worker count {n} changed the oversize-skip report (NFR-RA-06)"
            );
        }
    }

    #[test]
    fn parallel_discovery_is_stable_under_repeated_multithreaded_runs() {
        // FR-IX-09 stress: a data race in the parallel walk would surface as a
        // flaky candidate set. Repeatedly re-run the multi-threaded walk and
        // assert every run is byte-identical to the first (Rust has no built-in
        // race detector, so repeated-run stability is the idiom — mirrors
        // `extract::tests::parallel_extraction_is_independent_of_thread_count`).
        let (_tmp, root) = build_fixture();
        let config = test_config(1 << 20);

        let first = discover_with_threads(&root, &config, 8).unwrap();
        for run in 0..40 {
            let report = discover_with_threads(&root, &config, 8).unwrap();
            assert_eq!(report, first, "run {run} diverged under 8 workers (data race?)");
        }
    }

    #[test]
    fn discovery_on_empty_tree_is_empty_for_any_worker_count() {
        // The zero-file edge case: the parallel walk's channel drain (`drop(tx)`
        // then `for found in rx`) must terminate cleanly on an empty tree — no
        // hang, no panic — and return an empty report for every worker count.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let config = test_config(1 << 20);

        for &n in WORKER_COUNTS {
            let report = discover_with_threads(&root, &config, n).unwrap();
            assert!(report.files.is_empty(), "no files discovered in an empty tree ({n}w)");
            assert!(report.skipped_oversize.is_empty(), "no oversize skips in an empty tree ({n}w)");
        }
    }
}
