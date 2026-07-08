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
//! # Containment ([NFR-SE-04], amended by [FR-IX-10] / [ADR-59])
//! The walk is rooted at the canonicalised project root with `follow_links(false)`,
//! so source-code discovery never follows a symlink out of the tree, and every
//! yielded path is re-checked with `starts_with(root)` as defence in depth. This
//! is the enforcement point documented in [`docs/security/trusted-input-boundary.md`].
//!
//! The one **sanctioned carve-out** ([FR-IX-10], [ADR-59]) restores documentation
//! indexing under the swe-skills external-docs layout, where `docs/specs`,
//! `docs/planning`, and `docs/requests` are directory symlinks into a sibling
//! repo. Discovery resolves a sanctioned external docs root from a project-root
//! `.swe-skills` file and follows a **directory** symlink **iff** its canonical
//! target is contained within the project root or that sanctioned root — and only
//! when documentation discovery is on, yielding only documentation files. Every
//! other symlink is still skipped; a target escaping both roots, a broken link,
//! or an absent/misconfigured `.swe-skills` all fail closed to the skip-all
//! posture. Source-code discovery is untouched. See [`contained_dir_target`] and
//! [`discover_followed_symlink`].
//!
//! [FR-IX-02]: ../../../../docs/specs/requirements/FR-IX-02.md
//! [FR-IX-10]: ../../../../docs/specs/requirements/FR-IX-10.md
//! [FR-CF-02]: ../../../../docs/specs/requirements/FR-CF-02.md
//! [FR-CF-04]: ../../../../docs/specs/requirements/FR-CF-04.md
//! [NFR-SE-04]: ../../../../docs/specs/requirements/NFR-SE-04.md
//! [ADR-59]: ../../../../docs/specs/architecture/decisions/ADR-59.md
//! [`docs/security/trusted-input-boundary.md`]: ../../../../docs/security/trusted-input-boundary.md

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use globset::GlobSet;
use ignore::{DirEntry, WalkBuilder, WalkState};

use super::error::ConfigError;
use super::globs::{self, DocGlobs};
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

/// Why a documentation directory-symlink that exists under the documentation-
/// include set ended up **unindexed** ([FR-IX-11]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocSymlinkDrop {
    /// No sanctioned docs root is resolved (`.swe-skills` absent, empty, or
    /// misconfigured) and the symlink's canonical target escapes the project
    /// root — so the [FR-IX-10] follow-branch refuses it.
    NoSanctionedRoot,
    /// A sanctioned root *is* resolved, but the symlink's canonical target
    /// escapes **both** the project root and that sanctioned root — the
    /// no-arbitrary-escape invariant ([NFR-SE-04]) refuses it.
    EscapesContainment,
}

impl DocSymlinkDrop {
    /// The human-readable reason phrase ([FR-IX-11]: name the path *and* reason).
    fn reason(self) -> &'static str {
        match self {
            DocSymlinkDrop::NoSanctionedRoot => {
                "its canonical target escapes the project root and no sanctioned \
                 `.swe-skills` docs root is resolved"
            }
            DocSymlinkDrop::EscapesContainment => {
                "its canonical target escapes both the project root and the \
                 sanctioned `.swe-skills` docs root"
            }
        }
    }
}

/// A documentation **directory-symlink** that exists under the documentation-
/// include set but ended up unindexed ([FR-IX-11]): the dropped in-tree path and
/// the reason. Surfaced as an `index`/`doctor` warning so a silent doc-drop —
/// the failure mode [CR-071] closes — becomes a visible, actionable signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnindexedDocSymlink {
    /// The symlink's project-root-relative in-tree path (e.g. `docs/specs`).
    pub link: PathBuf,
    /// Why it was not followed and therefore not indexed.
    pub reason: DocSymlinkDrop,
}

impl fmt::Display for UnindexedDocSymlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "documentation directory-symlink {} exists but is unindexed: {}",
            to_forward_slash(&self.link),
            self.reason.reason()
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
    /// Documentation directory-symlinks that exist under the documentation-
    /// include set but ended up unindexed, sorted by path ([FR-IX-11]). Empty
    /// when documentation is disabled, when no doc symlink exists, or when every
    /// doc symlink was followed and indexed.
    pub unindexed_doc_symlinks: Vec<UnindexedDocSymlink>,
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

    // Sanctioned documentation-symlink carve-out ([FR-IX-10], [ADR-59]).
    // `doc_globs` is `None` when documentation discovery is disabled — the walk
    // then follows no symlink at all (there is no documentation-include set to
    // scope the follow to). A resolvable `.swe-skills` widens the allowlist of
    // canonical destinations from `{root}` to `{root, sanctioned_root}`; an
    // absent/empty/misconfigured file leaves it at `{root}` and fails closed.
    let doc_globs = config.documentation.compile()?;
    let sanctioned_root = resolve_sanctioned_docs_root(&root);
    let follow_enabled = doc_globs.is_some();

    // The main walker's `filter_entry` takes ownership of the ignored-dir set; the
    // post-pass sub-walk needs it too, so clone one for the closure.
    let walk_ignored_dirs = ignored_dirs.clone();
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
        .filter_entry(move |entry| keep_dir(entry, &walk_ignored_dirs))
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
    let sanctioned_ref = sanctioned_root.as_deref();
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
            // Symlinks (NFR-SE-04): with follow_links(false) a symlink is yielded
            // as-is, never traversed. Handling it explicitly here makes the
            // containment guarantee local and auditable rather than implicit in
            // the walker's type resolution.
            //
            // Sanctioned carve-out ([FR-IX-10], [ADR-59]): when documentation
            // discovery is on, a *directory* symlink whose canonical target is
            // contained within the project root or the sanctioned docs root is
            // handed to the post-pass sub-walk (`discover_followed_symlink`) for
            // documentation-only following. Every other symlink — a symlinked
            // file, a directory symlink escaping both roots, a broken link, or
            // any symlink at all when documentation is off — is skipped, so
            // source-code discovery still follows no symlink.
            if file_type.is_symlink() {
                if follow_enabled {
                    if let Some(target) =
                        contained_dir_target(entry.path(), root_ref, sanctioned_ref)
                    {
                        let _ = tx.send(Found::FollowDir {
                            link: entry.path().to_path_buf(),
                            target,
                        });
                    }
                }
                return WalkState::Continue;
            }
            if !file_type.is_file() {
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
    let mut follow_dirs = Vec::new();
    for found in rx {
        match found {
            Found::File(path) => files.push(path),
            Found::Oversize(skip) => skipped_oversize.push(skip),
            Found::FollowDir { link, target } => follow_dirs.push((link, target)),
        }
    }

    // Git-ignore-bypassing detection pass ([FR-IX-10] amended by [CR-071],
    // [FR-IX-11]). The main walk above runs `git_ignore(true)`/`git_exclude(true)`,
    // so a git-ignored documentation directory-symlink (this repo's own layout:
    // `.gitignore` lists `/docs/specs`, `/docs/planning`, `/docs/requests`) is
    // pruned by the `ignore` crate *before* the visitor's `Found::FollowDir`
    // branch can see it — making the follow inert on git-ignoring checkouts. This
    // pass re-enumerates directory-symlinks under the documentation prefixes with
    // git-ignore/exclude disabled — scoped **only** to that doc subtree, never a
    // global relaxation of the main walk — and feeds contained targets into the
    // same one-hop sub-walk. A doc-scoped symlink whose target escapes both roots
    // (or resolves no sanctioned root) is recorded as an unindexed drop for the
    // `index`/`doctor` warning ([FR-IX-11]) instead. Documentation off ⇒ no doc-
    // include set ⇒ this whole pass is skipped.
    let mut unindexed_doc_symlinks = Vec::new();
    if follow_enabled {
        let doc_prefixes = doc_dir_prefixes(&config.documentation.include);
        let (extra_follow, dropped) =
            classify_doc_dir_symlinks(root_ref, &doc_prefixes, &ignored_dirs, sanctioned_ref);
        follow_dirs.extend(extra_follow);
        unindexed_doc_symlinks = dropped;
    }

    // Follow each sanctioned directory symlink exactly once ([FR-IX-10]). Sort +
    // dedup makes the follow set order-independent (the parallel visitor observes
    // symlinks in an arbitrary order); each sub-walk is serial and its output is
    // merged then globally sorted below, so the report stays thread-count-
    // independent (NFR-RA-06).
    if let Some(doc_globs) = doc_globs.as_ref() {
        follow_dirs.sort();
        follow_dirs.dedup();
        if !follow_dirs.is_empty() {
            let scope = DocFollow {
                root: root_ref,
                include: include_ref,
                exclude: exclude_ref,
                ignored_dirs: &ignored_dirs,
                max_file_size,
                doc_globs,
            };
            // Dedup followed files by their CANONICAL (symlink-resolved) path,
            // against the real paths the main pass already yielded and against one
            // another. [FR-IX-10] permits following a directory symlink whose
            // target is within the project root, but such an in-tree symlink only
            // aliases content the main walk already indexed under its real path —
            // without this, an aliasing symlink (`docs/latest -> specs`, or the
            // degenerate `docs/all -> .`) would emit duplicate doc nodes under a
            // second root-relative key. The main pass's paths are real (the walk
            // never follows a link), so they are their own canonical form; seeding
            // `seen` with them makes an alias collide and drop. `follow_dirs` is
            // sorted, so which alias wins is deterministic (NFR-RA-06).
            let mut seen: HashSet<PathBuf> = files.iter().cloned().collect();
            for (link, target) in &follow_dirs {
                let (sub_files, sub_oversize) = discover_followed_symlink(&scope, link, target);
                for f in sub_files {
                    let canonical = f.canonicalize().unwrap_or_else(|_| f.clone());
                    if seen.insert(canonical) {
                        files.push(f);
                    }
                }
                for o in sub_oversize {
                    let canonical = o.path.canonicalize().unwrap_or_else(|_| o.path.clone());
                    if seen.insert(canonical) {
                        skipped_oversize.push(o);
                    }
                }
            }
        }
    }

    // Deterministic ordering — the parallel walk order is not stable, so the sort
    // is what makes the report thread-count-independent (NFR-RA-06). The detection
    // pass is serial, but sort its output too so the report is stable regardless
    // of directory-read order.
    files.sort();
    skipped_oversize.sort_by(|a, b| a.path.cmp(&b.path));
    unindexed_doc_symlinks.sort_by(|a, b| a.link.cmp(&b.link));

    Ok(DiscoveryReport {
        files,
        skipped_oversize,
        unindexed_doc_symlinks,
    })
}

/// An admitted entry observed by a walker thread, sent back for merging.
enum Found {
    /// A file to index (within `max_file_size`).
    File(PathBuf),
    /// A file skipped for exceeding `max_file_size` ([FR-CF-04]).
    Oversize(OversizeSkip),
    /// A sanctioned directory symlink to sub-walk after the main pass
    /// ([FR-IX-10]): `link` is its in-tree path (e.g. `<root>/docs/specs`),
    /// `target` its canonical destination (already proven contained by
    /// [`contained_dir_target`]).
    FollowDir { link: PathBuf, target: PathBuf },
}

/// Resolve the sanctioned external docs root from a project-root `.swe-skills`
/// file ([FR-IX-10], [ADR-59]).
///
/// The first non-empty, non-`#`-comment line is a path — accepted absolute,
/// `~`-relative (home-anchored), or project-root-relative — whose resolved,
/// canonicalised, **existing directory** target is the sanctioned root. Any
/// fault (file absent/unreadable, no usable line, `~` with no `$HOME`, or a
/// target that does not exist so canonicalisation fails) yields `None`:
/// discovery then follows no symlink out of the project root, i.e. today's
/// skip-all posture ("fail closed", matching swe-skills' own resolver).
///
/// Shared with [`AdmissionAuthority`](super::admission::AdmissionAuthority) so
/// the walk-parity predicate resolves the *same* sanctioned root as the walk
/// ([FR-SY-11], [ADR-48]).
pub(crate) fn resolve_sanctioned_docs_root(root: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(root.join(".swe-skills")).ok()?;
    let line = contents
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;

    let expanded: PathBuf = if line == "~" {
        PathBuf::from(std::env::var_os("HOME")?)
    } else if let Some(rest) = line.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME")?).join(rest)
    } else {
        let p = Path::new(line);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };

    let canonical = expanded.canonicalize().ok()?;
    canonical.is_dir().then_some(canonical)
}

/// The containment verdict for a directory symlink ([FR-IX-10], [ADR-59],
/// [FR-IX-11]) — the shared classification the follow-branch and the unindexed-
/// symlink warning both key on.
pub(crate) enum DirSymlinkTarget {
    /// A directory target contained within the project root or the sanctioned
    /// docs root — the symlink is followed one hop.
    Contained(PathBuf),
    /// A directory target escaping **both** roots — refused ([NFR-SE-04]); if it
    /// is a documentation symlink this is an [FR-IX-11] unindexed drop.
    Escapes,
    /// Not a directory symlink at all — a broken link or a link to a file — so it
    /// is neither followed nor reported.
    NotDir,
}

/// Classify a symlink's canonical target for the containment gate ([FR-IX-10]).
///
/// `canonicalize` fully resolves the link (so a chain of links is evaluated by
/// its final destination); a broken or non-directory target is [`NotDir`]; a
/// directory target is [`Contained`] iff it is within `root` or `sanctioned`
/// (when present), else [`Escapes`] — the no-arbitrary-escape invariant
/// [NFR-SE-04] preserves as amended.
///
/// [`NotDir`]: DirSymlinkTarget::NotDir
/// [`Contained`]: DirSymlinkTarget::Contained
/// [`Escapes`]: DirSymlinkTarget::Escapes
pub(crate) fn classify_dir_symlink(
    path: &Path,
    root: &Path,
    sanctioned: Option<&Path>,
) -> DirSymlinkTarget {
    let Ok(target) = path.canonicalize() else {
        return DirSymlinkTarget::NotDir;
    };
    if !target.is_dir() {
        return DirSymlinkTarget::NotDir;
    }
    if target.starts_with(root) || sanctioned.is_some_and(|s| target.starts_with(s)) {
        DirSymlinkTarget::Contained(target)
    } else {
        DirSymlinkTarget::Escapes
    }
}

/// If `path` is a symlink to a directory whose canonical target is contained
/// within `root` or `sanctioned` (when present), return that canonical target;
/// otherwise `None` ([FR-IX-10], [ADR-59]).
///
/// The follow-branch's view of [`classify_dir_symlink`]: only a [`Contained`]
/// target is followed.
///
/// [`Contained`]: DirSymlinkTarget::Contained
pub(crate) fn contained_dir_target(
    path: &Path,
    root: &Path,
    sanctioned: Option<&Path>,
) -> Option<PathBuf> {
    match classify_dir_symlink(path, root, sanctioned) {
        DirSymlinkTarget::Contained(target) => Some(target),
        DirSymlinkTarget::Escapes | DirSymlinkTarget::NotDir => None,
    }
}

/// The admission policy a sanctioned-symlink sub-walk applies, borrowed from the
/// main walk so the two passes agree on what is indexable ([FR-IX-10]).
struct DocFollow<'a> {
    /// The canonical project root — the anchor every result relativises under.
    root: &'a Path,
    /// The code include globs (a file must match one), mirroring the main pass.
    include: &'a GlobSet,
    /// The code exclude globs (a match rejects), mirroring the main pass.
    exclude: &'a GlobSet,
    /// Directory names pruned anywhere in the sub-walk.
    ignored_dirs: &'a HashSet<String>,
    /// Files strictly larger than this (bytes) become oversize skips ([FR-CF-04]).
    max_file_size: u64,
    /// The documentation globs — the gate that scopes the follow to docs.
    doc_globs: &'a DocGlobs,
}

/// Sub-walk a followed sanctioned directory symlink and return the
/// **documentation** files beneath it, each expressed under the symlink's
/// in-tree path so every result stays project-root-relative ([FR-IX-10],
/// [ADR-59]).
///
/// `link` is the symlink's path inside the project (e.g. `<root>/docs/specs`);
/// `target` is its canonicalised, already-proven-contained destination. The
/// sub-walk is the same gitignore-aware, contained walk as the main pass —
/// honouring `ignored_dirs`, nested-`.git` boundaries, the include/exclude
/// globs, and the size cap — but rooted at `target` with `follow_links(false)`,
/// so it cannot chain through a second symlink out of the sanctioned tree (the
/// carve-out is exactly one hop deep). A hit is emitted **only** when it is
/// admitted by both the code include/exclude globs (mirroring the main pass) and
/// the documentation globs: that dual gate is what scopes following to
/// documentation and keeps any source/config file reachable *only* through the
/// symlink out of the graph — source-code discovery still follows no symlink.
///
/// Each physical path under `target` is re-expressed as `link / <rest>` before
/// admission, because the whole pipeline keys on the project-root-relative path
/// (`docs/specs/…`), not the physical location; a path that fails to relativise
/// under `root` is dropped as defence in depth.
fn discover_followed_symlink(
    scope: &DocFollow<'_>,
    link: &Path,
    target: &Path,
) -> (Vec<PathBuf>, Vec<OversizeSkip>) {
    let mut files = Vec::new();
    let mut oversize = Vec::new();

    let pruned: HashSet<String> = scope.ignored_dirs.clone();
    let walker = WalkBuilder::new(target)
        .require_git(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .ignore(true)
        .hidden(false)
        .parents(false) // don't read ignore files above the sanctioned target.
        .follow_links(false) // never chain out of the sanctioned tree via a nested symlink.
        .filter_entry(move |entry| keep_dir(entry, &pruned)) // same pruning as the main pass.
        .build();

    for result in walker {
        let Ok(entry) = result else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        // A nested symlink or non-regular file inside the sanctioned tree is
        // never followed or indexed.
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        // Re-express the physical path under the in-tree symlink path so the
        // result is project-root-relative — the identity every downstream
        // consumer keys on.
        let Ok(within) = entry.path().strip_prefix(target) else {
            continue;
        };
        let logical = link.join(within);
        let Ok(rel) = logical.strip_prefix(scope.root) else {
            continue; // defence in depth — should always relativise under `root`.
        };
        // Dual gate: the code include/exclude globs (as the main pass applies) AND
        // the documentation globs. The doc gate is what scopes the follow to
        // documentation; a source/config file passing include/exclude but not a
        // doc glob is dropped, so it is never indexed via the symlink.
        if scope.exclude.is_match(rel) || !scope.include.is_match(rel) {
            continue;
        }
        let rel_str = to_forward_slash(rel);
        if !scope.doc_globs.admits(&rel_str) {
            continue;
        }
        let size = match entry.metadata() {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        if size > scope.max_file_size {
            oversize.push(OversizeSkip {
                path: logical,
                size,
                max: scope.max_file_size,
            });
        } else {
            files.push(logical);
        }
    }

    (files, oversize)
}

/// Whether the walker should descend into (keep) `entry` — the two directory-
/// pruning rules shared by the main pass and the sanctioned-symlink sub-walk.
///
/// Never prunes the walk root (depth 0), so a project dir named e.g. `build`, or
/// the root's own `.git`, still walks. Otherwise prunes:
/// - a **nested git boundary** — a linked worktree (`.git` gitlink file), a
///   vendored repository (`.git` directory), or a submodule. Logos indexes the
///   one root project; a nested git tree is a separate working tree the root
///   `.gitignore` cannot reliably mask (the `ignore` crate treats it as its own
///   repo boundary and applies *its* ignore rules), so folding it in
///   double-counts symbols — this is what let a `.worktrees/<sprint>/…` checkout
///   be indexed despite `.worktrees/` being gitignored;
/// - any directory whose name is in `ignored_dirs`.
///
/// Factored out of both `filter_entry` closures so the two passes' pruning is
/// provably identical rather than a comment asserting it ([NFR-SE-04] nested-
/// boundary containment).
fn keep_dir(entry: &DirEntry, ignored_dirs: &HashSet<String>) -> bool {
    if entry.depth() > 0 && entry.file_type().is_some_and(|ft| ft.is_dir()) {
        if entry.path().join(".git").exists() {
            return false;
        }
        if let Some(name) = entry.file_name().to_str() {
            return !ignored_dirs.contains(name);
        }
    }
    true
}

/// A path's components joined with `/` — the root-relative, forward-slashed form
/// the documentation globs match against (mirrors the pipeline's `to_forward_slash`).
///
/// Shared with [`AdmissionAuthority`](super::admission::AdmissionAuthority) so the
/// walk-parity predicate matches doc globs against the identical string form.
pub(crate) fn to_forward_slash(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// The literal directory **prefixes** of the documentation-include globs — the
/// scope the git-ignore-bypassing detection pass is confined to ([FR-IX-11]).
///
/// Each include pattern contributes its leading run of wildcard-free path
/// components: `docs/**/*.md` → `docs`, `docs/specs/**/*.md` → `docs/specs`. A
/// pattern whose first component is a wildcard (`*.md`, `README*`) contributes
/// nothing — a top-level file glob cannot be satisfied by a directory-symlink's
/// subtree — and neither does a fully-literal single-file pattern. The result is
/// the set of directory roots under which a documentation directory-symlink could
/// live, so the detection walk never descends the whole (possibly git-ignored)
/// tree, only that doc subtree (the scoping [CR-071] mandates).
fn doc_dir_prefixes(include: &[String]) -> Vec<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    for pattern in include {
        let mut base = PathBuf::new();
        for component in pattern.split('/') {
            if component.is_empty() || component_has_wildcard(component) {
                break;
            }
            base.push(component);
        }
        // A usable prefix has ≥1 literal directory component AND leaves room for
        // files to nest beneath it (the pattern continues past the base) — so a
        // bare `*.md`/`README*` (empty base) and a fully-literal single-file
        // pattern (`docs/guide.md`, base == pattern) both contribute nothing.
        if base.components().next().is_some() && base.as_path() != Path::new(pattern) {
            prefixes.push(base);
        }
    }
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

/// Whether a glob path-component contains a wildcard metacharacter, marking the
/// end of a pattern's literal directory prefix.
fn component_has_wildcard(component: &str) -> bool {
    component.contains(['*', '?', '[', ']', '{', '}'])
}

/// Whether `rel` lies at or beneath one of the documentation `prefixes` — the
/// test for "is this a documentation directory-symlink?" (component-wise, so
/// `documentation` is not under `docs`).
fn within_doc_prefix(rel: &Path, prefixes: &[PathBuf]) -> bool {
    prefixes.iter().any(|prefix| rel.starts_with(prefix))
}

/// Whether the detection walk should descend into directory `rel`: it must sit on
/// a documentation lineage — an ancestor of a prefix (to reach it) or at/under a
/// prefix (within the doc subtree).
fn on_doc_lineage(rel: &Path, prefixes: &[PathBuf]) -> bool {
    prefixes
        .iter()
        .any(|prefix| rel.starts_with(prefix) || prefix.starts_with(rel))
}

/// The detection walk's `filter_entry`: prune exactly as the main pass
/// ([`keep_dir`]) **and** confine descent to the documentation lineage so the
/// git-ignore-bypassing pass never walks the whole tree ([FR-IX-11], [CR-071]).
///
/// A real directory is descended only when it is on a doc lineage; a symlink or
/// file is kept only when it falls under a doc prefix (a doc symlink to inspect).
/// The root (depth 0) is always kept.
fn keep_detection(
    entry: &DirEntry,
    ignored_dirs: &HashSet<String>,
    prefixes: &[PathBuf],
    root: &Path,
) -> bool {
    if !keep_dir(entry, ignored_dirs) {
        return false;
    }
    if entry.depth() == 0 {
        return true;
    }
    let Ok(rel) = entry.path().strip_prefix(root) else {
        return false;
    };
    if entry.file_type().is_some_and(|ft| ft.is_dir()) {
        on_doc_lineage(rel, prefixes)
    } else {
        within_doc_prefix(rel, prefixes)
    }
}

/// Enumerate documentation directory-symlinks under `doc_prefixes`, bypassing
/// git-ignore/exclude ([FR-IX-11], [CR-071]).
///
/// A dedicated **serial** walk with `git_ignore(false)`/`git_exclude(false)` —
/// the one relaxation, and it is confined to the documentation subtree by
/// [`keep_detection`], never applied to the main walk. `follow_links(false)`
/// keeps it from chaining through any symlink (the one-hop follow is the sub-walk
/// [`discover_followed_symlink`]'s job). Returns the in-tree absolute paths of
/// every directory-symlink whose root-relative path falls under a doc prefix,
/// sorted+deduped for determinism.
fn detect_doc_dir_symlinks(
    root: &Path,
    doc_prefixes: &[PathBuf],
    ignored_dirs: &HashSet<String>,
) -> Vec<PathBuf> {
    if doc_prefixes.is_empty() {
        return Vec::new();
    }
    let prefixes = doc_prefixes.to_vec();
    let pruned = ignored_dirs.clone();
    let root_owned = root.to_path_buf();
    let walker = WalkBuilder::new(root)
        .require_git(false)
        .git_ignore(false) // the whole point: see git-ignored doc symlinks ([CR-071]).
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .hidden(false)
        .parents(false)
        .follow_links(false) // never chain through a symlink — the follow is one hop.
        .filter_entry(move |entry| keep_detection(entry, &pruned, &prefixes, &root_owned))
        .build();

    let mut links = Vec::new();
    for result in walker {
        let Ok(entry) = result else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_symlink()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        if within_doc_prefix(rel, doc_prefixes) {
            links.push(entry.path().to_path_buf());
        }
    }
    links.sort();
    links.dedup();
    links
}

/// Detect and classify the documentation directory-symlinks under `doc_prefixes`
/// ([FR-IX-10] amended by [CR-071], [FR-IX-11]).
///
/// Returns `(follow, unindexed)`: contained doc symlinks to feed into the one-hop
/// follow set (union-ed with the main pass's, deduped by the caller), and the
/// doc symlinks whose target escapes containment — the [FR-IX-11] unindexed drops
/// to warn about. A broken/non-directory link is neither. Shared with
/// [`unindexed_doc_symlinks`] so `discover` and `doctor` classify identically.
fn classify_doc_dir_symlinks(
    root: &Path,
    doc_prefixes: &[PathBuf],
    ignored_dirs: &HashSet<String>,
    sanctioned: Option<&Path>,
) -> (Vec<(PathBuf, PathBuf)>, Vec<UnindexedDocSymlink>) {
    let mut follow = Vec::new();
    let mut unindexed = Vec::new();
    for link in detect_doc_dir_symlinks(root, doc_prefixes, ignored_dirs) {
        match classify_dir_symlink(&link, root, sanctioned) {
            DirSymlinkTarget::Contained(target) => follow.push((link, target)),
            DirSymlinkTarget::Escapes => {
                if let Ok(rel) = link.strip_prefix(root) {
                    let reason = if sanctioned.is_some() {
                        DocSymlinkDrop::EscapesContainment
                    } else {
                        DocSymlinkDrop::NoSanctionedRoot
                    };
                    unindexed.push(UnindexedDocSymlink {
                        link: rel.to_path_buf(),
                        reason,
                    });
                }
            }
            DirSymlinkTarget::NotDir => {}
        }
    }
    (follow, unindexed)
}

/// The documentation directory-symlinks that exist under the documentation-
/// include set but end up **unindexed** ([FR-IX-11]) — the `doctor`-side twin of
/// the drops [`discover`] folds into its report, computed without a full index
/// walk so `doctor` stays a cheap diagnostic.
///
/// Returns an empty list when documentation is disabled (no doc-include set to
/// scope the follow to, mirroring [`discover`]). Fails only on a bad root or a
/// malformed/escaping doc glob — the same faults [`discover`] raises.
///
/// # Errors
/// - [`ConfigError::InvalidRoot`] if `root` is missing or not a directory.
/// - [`ConfigError::EscapingPattern`] / [`ConfigError::BadGlob`] on a bad doc glob.
pub fn unindexed_doc_symlinks(
    root: &Path,
    config: &Config,
) -> Result<Vec<UnindexedDocSymlink>, ConfigError> {
    let root = root.canonicalize().map_err(|_| ConfigError::InvalidRoot {
        path: root.to_path_buf(),
    })?;
    if !root.is_dir() {
        return Err(ConfigError::InvalidRoot { path: root });
    }
    // Documentation off ⇒ no doc-include set ⇒ nothing to detect (parity with
    // `discover`'s `follow_enabled` gate). `compile` also fails loud on a bad glob.
    if config.documentation.compile()?.is_none() {
        return Ok(Vec::new());
    }
    let sanctioned = resolve_sanctioned_docs_root(&root);
    let ignored_dirs: HashSet<String> = config.semantics.ignored_dirs.iter().cloned().collect();
    let doc_prefixes = doc_dir_prefixes(&config.documentation.include);
    let (_follow, mut unindexed) =
        classify_doc_dir_symlinks(&root, &doc_prefixes, &ignored_dirs, sanctioned.as_deref());
    unindexed.sort_by(|a, b| a.link.cmp(&b.link));
    Ok(unindexed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
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

    // ── Sanctioned docs-symlink carve-out ([FR-IX-10], [ADR-59]) ──────────────

    #[test]
    fn resolve_sanctioned_docs_root_relative_absolute_and_fails_closed() {
        // A sibling directory that will be the sanctioned target.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let sibling = base.join("logos-docs");
        fs::create_dir_all(&sibling).unwrap();
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();

        // Relative (repo-root-relative) form: `../logos-docs`.
        write(&root.join(".swe-skills"), "../logos-docs\n");
        assert_eq!(
            resolve_sanctioned_docs_root(&root),
            Some(sibling.clone()),
            "a repo-root-relative path resolves and canonicalises"
        );

        // Absolute form resolves to the same canonical dir; leading comments and
        // blank lines are skipped, the first usable line wins.
        write(
            &root.join(".swe-skills"),
            &format!("# swe-skills docs root\n\n{}\n", sibling.display()),
        );
        assert_eq!(
            resolve_sanctioned_docs_root(&root),
            Some(sibling.clone()),
            "an absolute path (after comments/blank lines) resolves"
        );

        // Fail closed: absent file → None.
        fs::remove_file(root.join(".swe-skills")).unwrap();
        assert_eq!(resolve_sanctioned_docs_root(&root), None, "absent .swe-skills → None");

        // Fail closed: empty / comment-only file → None.
        write(&root.join(".swe-skills"), "# only a comment\n\n");
        assert_eq!(resolve_sanctioned_docs_root(&root), None, "no usable line → None");

        // Fail closed: a target that does not exist → None (canonicalisation fails).
        write(&root.join(".swe-skills"), "../does-not-exist\n");
        assert_eq!(
            resolve_sanctioned_docs_root(&root),
            None,
            "a missing target fails closed"
        );

        // Fail closed: a target that is a file, not a directory → None.
        write(&sibling.join("afile"), "x");
        write(&root.join(".swe-skills"), "../logos-docs/afile\n");
        assert_eq!(
            resolve_sanctioned_docs_root(&root),
            None,
            "a non-directory target is not a sanctioned root"
        );
    }

    #[test]
    #[cfg(unix)]
    fn contained_dir_target_follows_sanctioned_and_refuses_escape() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        let sanctioned = base.join("logos-docs");
        fs::create_dir_all(sanctioned.join("specs")).unwrap();
        let escape = base.join("elsewhere");
        fs::create_dir_all(&escape).unwrap();

        // Target inside the sanctioned root → followed.
        let into_sanctioned = root.join("docs_specs");
        symlink(sanctioned.join("specs"), &into_sanctioned).unwrap();
        assert_eq!(
            contained_dir_target(&into_sanctioned, &root, Some(&sanctioned)),
            Some(sanctioned.join("specs").canonicalize().unwrap()),
            "a target within the sanctioned root is followed"
        );
        // …but not once the sanctioned root is withdrawn (escapes both roots).
        assert_eq!(
            contained_dir_target(&into_sanctioned, &root, None),
            None,
            "without a sanctioned root the same link escapes and is refused"
        );

        // Target inside the project root → followed even with no sanctioned root.
        fs::create_dir_all(root.join("real")).unwrap();
        let into_project = root.join("selfref");
        symlink(root.join("real"), &into_project).unwrap();
        assert_eq!(
            contained_dir_target(&into_project, &root, None),
            Some(root.join("real").canonicalize().unwrap()),
            "a target within the project root is followed"
        );

        // Target escaping both roots → refused.
        let out = root.join("escape");
        symlink(&escape, &out).unwrap();
        assert_eq!(
            contained_dir_target(&out, &root, Some(&sanctioned)),
            None,
            "a target escaping both roots is refused"
        );

        // A symlink to a *file* (not a directory) → refused (only dirs are followed).
        write(&sanctioned.join("specs/ADR.md"), "# doc");
        let file_link = root.join("filelink");
        symlink(sanctioned.join("specs/ADR.md"), &file_link).unwrap();
        assert_eq!(
            contained_dir_target(&file_link, &root, Some(&sanctioned)),
            None,
            "a symlink to a file is not a directory target"
        );

        // A broken symlink → refused (canonicalisation fails).
        let broken = root.join("broken");
        symlink(base.join("nope"), &broken).unwrap();
        assert_eq!(contained_dir_target(&broken, &root, Some(&sanctioned)), None);
    }

    /// Build a project whose `docs/specs` is a directory symlink into a sibling
    /// "sanctioned" repo declared in `.swe-skills`, with a markdown doc and a
    /// source file living behind the symlink. Returns (temp guard, canonical
    /// project root, canonical sanctioned root).
    #[cfg(unix)]
    fn build_symlink_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("project");
        fs::create_dir_all(root.join("docs")).unwrap();
        // A real, in-repo source file (always discovered by the main pass).
        write(&root.join("src/main.rs"), "fn main() {}\n");

        // The sanctioned sibling repo with a doc and a source file under specs/.
        let sanctioned = base.join("logos-docs");
        write(&sanctioned.join("specs/ADR-46.md"), "# ADR-46\n");
        write(&sanctioned.join("specs/deep/FR-IX-10.md"), "# FR-IX-10\n");
        write(&sanctioned.join("specs/embedded.rs"), "pub fn x() {}\n");
        let sanctioned = sanctioned.canonicalize().unwrap();

        // `.swe-skills` points at the sibling; `docs/specs` is a symlink into it.
        write(&root.join(".swe-skills"), &format!("{}\n", sanctioned.display()));
        symlink(sanctioned.join("specs"), root.join("docs/specs")).unwrap();

        (tmp, root, sanctioned)
    }

    #[test]
    #[cfg(unix)]
    fn follows_sanctioned_docs_symlink_and_indexes_only_docs() {
        let (_tmp, root, _sanctioned) = build_symlink_fixture();
        // Default config: documentation on, default doc globs (`docs/**/*.md`).
        let config = Config::default();

        let baseline = discover_with_threads(&root, &config, 1).unwrap();
        let rels: BTreeSet<String> = baseline
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains("src/main.rs"), "the real source file is discovered: {rels:?}");
        assert!(
            rels.contains("docs/specs/ADR-46.md"),
            "a doc behind the sanctioned symlink is discovered under its in-tree path: {rels:?}"
        );
        assert!(
            rels.contains("docs/specs/deep/FR-IX-10.md"),
            "a nested doc behind the symlink is discovered: {rels:?}"
        );
        assert!(
            !rels.iter().any(|r| r.ends_with("embedded.rs")),
            "a source file behind the symlink is NOT indexed (source skips symlinks): {rels:?}"
        );

        // Idempotent + thread-count-independent (NFR-RA-06): every worker count
        // yields the identical report, and a re-run is byte-identical.
        for &n in WORKER_COUNTS {
            let report = discover_with_threads(&root, &config, n).unwrap();
            assert_eq!(report, baseline, "worker count {n} diverged");
        }
        assert_eq!(
            discover_with_threads(&root, &config, 0).unwrap(),
            baseline,
            "a second discovery is idempotent"
        );
    }

    #[test]
    #[cfg(unix)]
    fn escaping_symlink_not_followed_even_with_swe_skills() {
        use std::os::unix::fs::symlink;

        let (_tmp, root, _sanctioned) = build_symlink_fixture();
        // A second symlink escaping BOTH roots, into an unrelated outside dir
        // that itself contains a markdown file — it must never be followed.
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        write(&outside.join("secret.md"), "# secret\n");
        symlink(&outside, root.join("docs/leak")).unwrap();

        let report = discover_with_threads(&root, &Config::default(), 0).unwrap();
        let rels: BTreeSet<String> = report
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains("docs/specs/ADR-46.md"), "the sanctioned doc is still followed");
        assert!(
            !rels.iter().any(|r| r.contains("secret.md")),
            "a symlink escaping both roots is not followed: {rels:?}"
        );
        // Defence in depth: every discovered path stays under the project root.
        for f in &report.files {
            assert!(f.starts_with(&root), "{f:?} escaped {root:?}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn absent_swe_skills_reproduces_skip_all_symlinks() {
        let (_tmp, root, _sanctioned) = build_symlink_fixture();
        // Remove `.swe-skills`: no sanctioned root, so the out-of-root docs
        // symlink escapes and is skipped — the prior skip-all behaviour.
        fs::remove_file(root.join(".swe-skills")).unwrap();

        let report = discover_with_threads(&root, &Config::default(), 0).unwrap();
        let rels: BTreeSet<String> = report
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains("src/main.rs"), "real source is still discovered");
        assert!(
            !rels.iter().any(|r| r.starts_with("docs/specs")),
            "with no sanctioned root the out-of-root docs symlink is skipped: {rels:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn in_tree_alias_symlink_does_not_duplicate_docs() {
        // An in-tree directory symlink (target within the project root) is
        // followable per FR-IX-10, but its content is already indexed by the main
        // pass under the real path — so the followed alias must NOT produce a
        // duplicate doc node under a second key (canonical-path dedup).
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("docs/real/guide.md"), "# Guide\n");
        // `docs/alias -> docs/real` (target within the project root) and the
        // degenerate `docs/all -> .` (target = the root itself).
        symlink(root.join("docs/real"), root.join("docs/alias")).unwrap();
        symlink(&root, root.join("docs/all")).unwrap();

        let report = discover_with_threads(&root, &Config::default(), 0).unwrap();
        let rels: Vec<String> = report
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains(&"docs/real/guide.md".to_string()), "real doc indexed: {rels:?}");
        // Exactly one occurrence of the physical doc — no aliased duplicates under
        // `docs/alias/…` or `docs/all/…`.
        assert_eq!(
            rels.iter().filter(|r| r.ends_with("guide.md")).count(),
            1,
            "the aliased doc must not be indexed twice: {rels:?}"
        );
        assert!(
            !rels.iter().any(|r| r.starts_with("docs/alias") || r.starts_with("docs/all")),
            "aliased in-tree paths are deduped away: {rels:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn nested_symlink_inside_sanctioned_tree_is_not_followed() {
        // The carve-out is exactly one hop deep: a nested symlink INSIDE the
        // followed sanctioned tree, pointing back out, must not be chained through
        // (NFR-SE-04 as amended — no escape via a second hop).
        use std::os::unix::fs::symlink;

        let (_tmp, root, sanctioned) = build_symlink_fixture();
        // An escaping directory holding an admissible doc, linked from *inside*
        // the sanctioned tree that `docs/specs` points at.
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        write(&outside.join("secret.md"), "# secret\n");
        symlink(&outside, sanctioned.join("specs/nested_leak")).unwrap();

        let report = discover_with_threads(&root, &Config::default(), 0).unwrap();
        let rels: BTreeSet<String> = report
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains("docs/specs/ADR-46.md"), "the sanctioned doc is followed");
        assert!(
            !rels.iter().any(|r| r.contains("secret.md")),
            "a nested symlink inside the sanctioned tree is not chained through: {rels:?}"
        );
        for f in &report.files {
            assert!(f.starts_with(&root), "{f:?} escaped {root:?}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn documentation_disabled_follows_no_symlink() {
        let (_tmp, root, _sanctioned) = build_symlink_fixture();
        let mut config = Config::default();
        config.documentation.enabled = false;

        let report = discover_with_threads(&root, &config, 0).unwrap();
        let rels: BTreeSet<String> = report
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(
            !rels.iter().any(|r| r.starts_with("docs/specs")),
            "with documentation off there is no include set to scope the follow to: {rels:?}"
        );
    }

    // ── Git-ignored sanctioned doc-symlink bypass + unindexed warning ─────────
    // ([FR-IX-10] amended by [CR-071], [FR-IX-11]).

    /// [`build_symlink_fixture`] plus a `.gitignore` that ignores the `docs/specs`
    /// symlink — the exact condition [S-277]'s fixtures missed and [CR-071] fixes:
    /// on `main` the walk's `git_ignore(true)` prunes this symlink before the
    /// follow-branch, so the feature was inert. Returns (guard, root, sanctioned).
    #[cfg(unix)]
    fn build_gitignored_symlink_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (tmp, root, sanctioned) = build_symlink_fixture();
        // Git-ignore the doc symlink exactly as this project's own `.gitignore`
        // does (`/docs/specs`) — the machine-local relocation kept out of git.
        write(&root.join(".gitignore"), "/docs/specs\n");
        (tmp, root, sanctioned)
    }

    #[test]
    #[cfg(unix)]
    fn follows_git_ignored_sanctioned_docs_symlink() {
        // The core [CR-071] regression: a git-ignored sanctioned doc symlink must
        // still be followed and its docs indexed — the detection pass bypasses the
        // main walk's git-ignore filtering for exactly the sanctioned doc subtree.
        let (_tmp, root, _sanctioned) = build_gitignored_symlink_fixture();
        let config = Config::default();

        let baseline = discover_with_threads(&root, &config, 1).unwrap();
        let rels: BTreeSet<String> = baseline
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains("src/main.rs"), "the real source file is discovered: {rels:?}");
        assert!(
            rels.contains("docs/specs/ADR-46.md"),
            "a git-ignored sanctioned doc is followed and indexed ([CR-071]): {rels:?}"
        );
        assert!(
            rels.contains("docs/specs/deep/FR-IX-10.md"),
            "a nested git-ignored sanctioned doc is followed: {rels:?}"
        );
        assert!(
            !rels.iter().any(|r| r.ends_with("embedded.rs")),
            "source behind the symlink is still NOT indexed (source skips symlinks): {rels:?}"
        );
        // Docs indexed correctly ⇒ no unindexed-doc-symlink warning ([FR-IX-11]).
        assert!(
            baseline.unindexed_doc_symlinks.is_empty(),
            "no warning when the git-ignored sanctioned symlink indexes correctly: {:?}",
            baseline.unindexed_doc_symlinks
        );

        // Idempotent + thread-count-independent (NFR-RA-06), including the new
        // detection pass and report field.
        for &n in WORKER_COUNTS {
            assert_eq!(discover_with_threads(&root, &config, n).unwrap(), baseline, "count {n}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn git_ignored_non_sanctioned_path_stays_skipped() {
        // The bypass is scoped to the sanctioned DOC subtree only: a git-ignored
        // ordinary source tree is still pruned (no global relaxation of the walk).
        let (_tmp, root, _sanctioned) = build_gitignored_symlink_fixture();
        write(&root.join("generated/out.rs"), "pub fn g() {}\n");
        write(&root.join(".gitignore"), "/docs/specs\n/generated\n");

        let rels: BTreeSet<String> = discover_with_threads(&root, &Config::default(), 0)
            .unwrap()
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(rels.contains("docs/specs/ADR-46.md"), "sanctioned doc followed: {rels:?}");
        assert!(
            !rels.iter().any(|r| r.starts_with("generated/")),
            "a git-ignored non-sanctioned source tree stays skipped: {rels:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn escaping_git_ignored_doc_symlink_is_reported_unindexed() {
        // A doc-scoped directory symlink whose target escapes BOTH roots is not
        // followed (unchanged), and — even git-ignored — is now surfaced as an
        // unindexed drop naming the path + reason ([FR-IX-11]).
        use std::os::unix::fs::symlink;

        let (_tmp, root, _sanctioned) = build_gitignored_symlink_fixture();
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        write(&outside.join("secret.md"), "# secret\n");
        symlink(&outside, root.join("docs/leak")).unwrap();
        // Git-ignore the escaping doc symlink too — it must still be detected.
        write(&root.join(".gitignore"), "/docs/specs\n/docs/leak\n");

        let report = discover_with_threads(&root, &Config::default(), 0).unwrap();
        let rels: BTreeSet<String> = report
            .files
            .iter()
            .map(|p| to_forward_slash(p.strip_prefix(&root).unwrap()))
            .collect();

        assert!(!rels.iter().any(|r| r.contains("secret.md")), "escaping symlink not followed");
        let dropped: Vec<&UnindexedDocSymlink> = report
            .unindexed_doc_symlinks
            .iter()
            .filter(|d| d.link.as_path() == Path::new("docs/leak"))
            .collect();
        assert_eq!(dropped.len(), 1, "docs/leak reported unindexed: {:?}", report.unindexed_doc_symlinks);
        assert_eq!(dropped[0].reason, DocSymlinkDrop::EscapesContainment);
        // The rendered warning names the path and the reason ([FR-IX-11]).
        let msg = dropped[0].to_string();
        assert!(msg.contains("docs/leak"), "warning names the path: {msg}");
        assert!(msg.contains("escapes"), "warning names the reason: {msg}");
        // The sanctioned doc still indexes — the drop is scoped to the escaper.
        assert!(rels.contains("docs/specs/ADR-46.md"), "sanctioned doc still followed: {rels:?}");
    }

    #[test]
    #[cfg(unix)]
    fn absent_swe_skills_reports_doc_symlink_unindexed() {
        // With `.swe-skills` absent, an out-of-root doc symlink resolves no
        // sanctioned root and is skipped (unchanged) — and is reported unindexed
        // with the NoSanctionedRoot reason ([FR-IX-11]), turning the previously
        // silent doc-drop into a signal.
        let (_tmp, root, _sanctioned) = build_gitignored_symlink_fixture();
        fs::remove_file(root.join(".swe-skills")).unwrap();

        let report = discover_with_threads(&root, &Config::default(), 0).unwrap();

        assert!(
            !report.files.iter().any(|p| p.strip_prefix(&root).unwrap().starts_with("docs/specs")),
            "with no sanctioned root the doc symlink is skipped"
        );
        let dropped: Vec<&UnindexedDocSymlink> = report
            .unindexed_doc_symlinks
            .iter()
            .filter(|d| d.link.as_path() == Path::new("docs/specs"))
            .collect();
        assert_eq!(dropped.len(), 1, "docs/specs reported unindexed: {:?}", report.unindexed_doc_symlinks);
        assert_eq!(dropped[0].reason, DocSymlinkDrop::NoSanctionedRoot);
    }

    #[test]
    #[cfg(unix)]
    fn documentation_disabled_reports_no_unindexed_doc_symlinks() {
        // Documentation off ⇒ no doc-include set ⇒ no detection and no warning,
        // even with a git-ignored escaping doc symlink present.
        let (_tmp, root, _sanctioned) = build_gitignored_symlink_fixture();
        fs::remove_file(root.join(".swe-skills")).unwrap();
        let mut config = Config::default();
        config.documentation.enabled = false;

        let report = discover_with_threads(&root, &config, 0).unwrap();
        assert!(
            report.unindexed_doc_symlinks.is_empty(),
            "no doc-include set ⇒ no unindexed warning: {:?}",
            report.unindexed_doc_symlinks
        );
    }

    #[test]
    #[cfg(unix)]
    fn unindexed_doc_symlinks_fn_matches_discover_for_doctor() {
        // The `doctor`-side twin computes the same drops as `discover` folds into
        // its report — without a full index walk.
        let (_tmp, root, _sanctioned) = build_gitignored_symlink_fixture();
        fs::remove_file(root.join(".swe-skills")).unwrap();
        let config = Config::default();

        let from_discover = discover_with_threads(&root, &config, 0).unwrap().unindexed_doc_symlinks;
        let from_doctor = unindexed_doc_symlinks(&root, &config).unwrap();
        assert_eq!(from_discover, from_doctor, "doctor twin agrees with discover");
        assert!(
            from_doctor.iter().any(|d| d.link.as_path() == Path::new("docs/specs")),
            "the git-ignored, unsanctioned doc symlink is reported: {from_doctor:?}"
        );

        // When docs index correctly (`.swe-skills` present), the twin is empty.
        write(&root.join(".swe-skills"), &format!("{}\n", _sanctioned.display()));
        assert!(
            unindexed_doc_symlinks(&root, &config).unwrap().is_empty(),
            "no warning once the sanctioned symlink resolves and indexes"
        );
    }

    #[test]
    fn doc_dir_prefixes_extracts_literal_directory_bases() {
        // The detection scope: only the literal directory prefix of a glob that
        // can nest files under a directory contributes; top-level file globs and
        // single-file literals contribute nothing.
        assert_eq!(
            doc_dir_prefixes(&["docs/**/*.md".into(), "*.md".into(), "README*".into()]),
            vec![PathBuf::from("docs")],
            "only `docs/**/*.md` yields a directory prefix"
        );
        assert_eq!(
            doc_dir_prefixes(&["docs/specs/**/*.md".into(), "handbook/**/*.md".into()]),
            vec![PathBuf::from("docs/specs"), PathBuf::from("handbook")],
            "a deeper base and a custom doc root are both captured (sorted)"
        );
        assert!(
            doc_dir_prefixes(&["*.md".into(), "README*".into(), "guide.md".into()]).is_empty(),
            "top-level file globs and single-file literals contribute no directory prefix"
        );
    }
}
