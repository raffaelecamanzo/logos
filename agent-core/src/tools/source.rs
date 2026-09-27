//! The **source** tool domain (S-167): net-new, path-sandboxed `read` / `grep`
//! / `glob` — the Source-Reader subagent's least-privilege set (S-174).
//!
//! These are the only agent tools that touch the filesystem directly (the
//! graph/governance tools go through the [`Engine`](logos_core::Engine)). Every
//! path is confined to the project root and `ignored_dirs` are skipped, the
//! same containment the indexer's discovery walk enforces ([NFR-SE-04]):
//!
//! - a caller-supplied path is **project-relative**; an absolute path or a `..`
//!   component is refused before any filesystem access ([`Sandbox::resolve`]);
//! - a path naming an `ignored_dirs` segment is refused;
//! - the resolved (canonicalised) path is re-checked with `starts_with(root)`,
//!   so a symlink pointing outside the tree cannot escape;
//! - the `grep`/`glob` walks use [`ignore::WalkBuilder`] with
//!   `follow_links(false)`, mirroring [`logos_core::config::discovery`].
//!
//! # Declared read roots (`[chat] read_roots`, sprint-79 HF-1)
//! A project may declare extra directories ([`Sandbox::with_read_roots`]) — the
//! sibling repo its `docs/` symlinks point into. Default: none, and everything
//! above holds byte-for-byte. When declared, a resolved canonical path is also
//! admitted under a read root, but a read root is reachable **only through an
//! in-tree symlink**: the lexical refusals (absolute, `..`) still run first, so
//! the agent never names a read root itself, and a symlink to any *undeclared*
//! directory is still an [`SandboxError::Escape`]. The walks follow a symlink
//! only when its canonical target lies under a read root — found by a per-
//! directory scan, because such a link is commonly git-ignored (this repo's
//! `/docs/planning`) and the gitignore-aware walker would prune it unseen —
//! skipping a link whose target is, or encloses, a directory already on the
//! chain of links that reached it (the cycle guard), and reporting every hit
//! under its in-tree path, so two links to one directory are both listed.
//!
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md

use std::collections::{HashSet, VecDeque};
// Anonymous import: brings `Read::{take, read_to_end}` into scope for the
// bounded file read without binding the name `Read` (which is also this
// module's `read` tool struct).
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use globset::GlobBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use rig_core::completion::ToolDefinition;
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The default read cap: 256 KiB. A single source file rarely exceeds this, and
/// the cap keeps a `read` of an accidental large artifact bounded.
const DEFAULT_MAX_READ_BYTES: usize = 256 * 1024;

/// The default cap on `grep` matches / `glob` paths returned in one call.
const DEFAULT_MATCH_LIMIT: usize = 200;

/// The cap on symlinks one `grep`/`glob` call follows into read roots. The
/// ancestry cycle guard makes every walk finite, but a link graph with fan-out
/// can still multiply the walks; past this cap no further link is followed and
/// the call reports `truncated`. A real docs layout follows a handful.
pub const MAX_FOLLOWED_LINKS: usize = 256;

/// Why a sandboxed path or pattern was refused.
///
/// The first four arms are the [NFR-SE-04] containment refusals; the rest are
/// ordinary I/O / argument faults. All are surfaced to the model so it can
/// correct the call rather than silently getting nothing.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The path is absolute; only project-relative paths are allowed.
    #[error("path {0:?} is absolute; only project-root-relative paths are allowed")]
    AbsolutePath(String),

    /// The path contains a `..` component that would climb above the root.
    #[error("path {0:?} escapes the project root via a `..` component")]
    Traversal(String),

    /// The path names an `ignored_dirs` segment.
    #[error("path {0:?} lies under the ignored directory {1:?}")]
    Ignored(String, String),

    /// The resolved (canonical) path lies outside the project root — e.g. a
    /// symlink pointing out of the tree — and outside every declared read root.
    /// The message is the pre-read-roots text, byte-for-byte, so a project that
    /// declares none sees exactly what it saw before.
    #[error("path {0:?} resolves outside the project root")]
    Escape(String),

    /// The path does not exist within the project.
    #[error("path {0:?} was not found within the project root")]
    NotFound(String),

    /// The path was found but is not a regular file (e.g. a directory passed to
    /// `read`).
    #[error("path {0:?} is not a regular file")]
    NotAFile(String),

    /// A filesystem error while resolving or reading a path.
    #[error("i/o error for path {path:?}: {source}")]
    Io {
        /// The offending path.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The caller's glob pattern failed to compile.
    #[error("invalid glob pattern {0:?}: {1}")]
    BadGlob(String, String),

    /// The caller's regex pattern failed to compile.
    #[error("invalid regex {0:?}: {1}")]
    BadRegex(String, String),

    /// A declared read root (`[chat] read_roots`) cannot be used — it does not
    /// exist, is not a directory, or cannot be canonicalised. Raised when the
    /// sandbox is built, never by a tool call: a declared entry is reported,
    /// never silently dropped.
    #[error("[chat] read_roots entry {entry:?} (resolved to {resolved:?}) {reason}")]
    BadReadRoot {
        /// The entry as declared.
        entry: String,
        /// The path it resolved to against its declaring root.
        resolved: String,
        /// Why it was refused.
        reason: String,
    },
}

impl SandboxError {
    /// Whether this is a **security-sandbox / containment refusal** — a path
    /// escaping the project root (an absolute path, a `..` traversal, an
    /// `ignored_dirs` segment, or a symlink resolving outside the tree) — as
    /// opposed to a benign I/O or argument fault (a missing file, a non-file
    /// target, an I/O error, a bad glob/regex).
    ///
    /// This is the single structural predicate the dispatch seam keys on to make
    /// a containment violation **turn-fatal** rather than a recoverable
    /// route-around fault ([NFR-SE-04], [NFR-CC-04], CR-063): every source tool
    /// that consults the [`Sandbox`] surfaces its refusals through this one enum,
    /// so classifying here means they all inherit the behavior. A new containment
    /// arm added later must be listed here too.
    pub fn is_containment_refusal(&self) -> bool {
        matches!(
            self,
            SandboxError::AbsolutePath(_)
                | SandboxError::Traversal(_)
                | SandboxError::Ignored(_, _)
                | SandboxError::Escape(_)
        )
    }
}

/// A project-root-confined filesystem view shared by the source tools.
///
/// Construct once per worktree root (cheap; canonicalises the root) and share
/// behind an `Arc` across the three tools.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// The canonicalised project root — the anchor for every containment check.
    root: PathBuf,
    /// Directory *names* pruned anywhere in the tree (config `ignored_dirs`).
    ///
    /// Behind an `Arc` so the per-call `grep`/`glob` walk captures a pointer
    /// copy into its `'static` `filter_entry` closure rather than re-cloning the
    /// whole set on every invocation.
    ignored_dirs: Arc<HashSet<String>>,
    /// The byte cap a single `read` returns — and the per-file cap the `grep`
    /// walk applies, so neither tool can be steered into an unbounded allocation
    /// by a large file in the tree.
    max_read_bytes: usize,
    /// The canonicalised declared read roots ([`with_read_roots`](Self::with_read_roots));
    /// empty by default, which keeps every check and walk exactly the
    /// root-only containment.
    read_roots: Arc<Vec<PathBuf>>,
}

impl Sandbox {
    /// Build a sandbox rooted at `root`, pruning the given `ignored_dirs`.
    ///
    /// # Errors
    /// [`SandboxError::Io`] / [`SandboxError::NotFound`] if `root` cannot be
    /// canonicalised (missing or unreadable).
    pub fn new(
        root: impl AsRef<Path>,
        ignored_dirs: impl IntoIterator<Item = String>,
    ) -> Result<Self, SandboxError> {
        let root_ref = root.as_ref();
        let canon = root_ref.canonicalize().map_err(|source| {
            let path = root_ref.display().to_string();
            if source.kind() == std::io::ErrorKind::NotFound {
                SandboxError::NotFound(path)
            } else {
                SandboxError::Io { path, source }
            }
        })?;
        Ok(Self {
            root: canon,
            ignored_dirs: Arc::new(ignored_dirs.into_iter().collect()),
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            read_roots: Arc::new(Vec::new()),
        })
    }

    /// Build a sandbox for `root` using the project's configured `ignored_dirs`
    /// (the [config](logos_core::config) `[semantics]` table, or its defaults
    /// when no `config.toml` is present) — the constructor the chat/wiki
    /// surfaces use.
    ///
    /// # Errors
    /// Propagates a config-load failure or a root-canonicalisation failure.
    pub fn from_root(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let config = logos_core::config::load_config_from_root(root.as_ref())?;
        Ok(Self::new(root, config.semantics.ignored_dirs)?)
    }

    /// Override the per-`read` byte cap (returns `self` for chaining).
    pub fn with_max_read_bytes(mut self, max_read_bytes: usize) -> Self {
        self.max_read_bytes = max_read_bytes;
        self
    }

    /// Declare extra read roots (`[chat] read_roots`), each resolved against
    /// `base` — the root whose `config.toml` declared them — or taken as given
    /// when absolute, then canonicalised **once**, here.
    ///
    /// A read root widens only what a resolved path may *land* in; callers still
    /// pass project-relative paths, so one is reached through an in-tree symlink
    /// or not at all (see the module docs).
    ///
    /// # Errors
    /// [`SandboxError::BadReadRoot`] naming the first entry that does not exist,
    /// is not a directory, or cannot be canonicalised.
    pub fn with_read_roots<I, S>(mut self, base: &Path, entries: I) -> Result<Self, SandboxError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut roots = Vec::new();
        for entry in entries {
            let entry = entry.as_ref();
            let joined = base.join(entry);
            let bad = |reason: String| SandboxError::BadReadRoot {
                entry: entry.to_string(),
                resolved: joined.display().to_string(),
                reason,
            };
            let canonical = joined.canonicalize().map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    bad("does not exist".to_string())
                } else {
                    bad(format!("cannot be resolved: {e}"))
                }
            })?;
            if !canonical.is_dir() {
                return Err(bad("is not a directory".to_string()));
            }
            roots.push(canonical);
        }
        self.read_roots = Arc::new(roots);
        Ok(self)
    }

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonicalised declared read roots (empty unless declared).
    pub fn read_roots(&self) -> &[PathBuf] {
        &self.read_roots
    }

    /// The anchor a canonical path is contained by — the project root, else the
    /// first declared read root containing it — or `None` for an escape.
    /// Containment is by path component ([`Path::starts_with`]), so a sibling
    /// sharing a read root's name as a string prefix is not contained.
    fn anchor_of(&self, canonical: &Path) -> Option<&Path> {
        if canonical.starts_with(&self.root) {
            return Some(&self.root);
        }
        self.read_roots
            .iter()
            .find(|read_root| canonical.starts_with(read_root))
            .map(PathBuf::as_path)
    }

    /// Whether any segment of `relative` names an `ignored_dirs` entry.
    fn ignored_segment(&self, relative: &Path) -> Option<String> {
        relative.components().find_map(|component| match component {
            Component::Normal(name) => name
                .to_str()
                .filter(|name| self.ignored_dirs.contains(*name))
                .map(str::to_string),
            _ => None,
        })
    }

    /// Resolve a caller-supplied project-relative path to a canonical path
    /// confined to the root ([NFR-SE-04]).
    ///
    /// Refuses, **before** touching the filesystem, an absolute path, a `..`
    /// component, or any segment named in `ignored_dirs`; then canonicalises and
    /// re-checks `starts_with(root)` — or `starts_with` a declared read root — so
    /// a symlink cannot escape, and re-scans the canonical path's segments below
    /// that anchor for ignored directories (catching a symlink *into* an ignored
    /// subtree).
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, SandboxError> {
        let requested = Path::new(rel);

        // Lexical checks first — cheap, and they never touch the filesystem, so a
        // traversal attempt is rejected without a stat (NFR-SE-04).
        for component in requested.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    return Err(SandboxError::AbsolutePath(rel.to_string()));
                }
                Component::ParentDir => {
                    return Err(SandboxError::Traversal(rel.to_string()));
                }
                Component::Normal(name) => {
                    if let Some(name) = name.to_str() {
                        if self.ignored_dirs.contains(name) {
                            return Err(SandboxError::Ignored(rel.to_string(), name.to_string()));
                        }
                    }
                }
                Component::CurDir => {}
            }
        }

        let joined = self.root.join(requested);
        let canonical = joined.canonicalize().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                SandboxError::NotFound(rel.to_string())
            } else {
                SandboxError::Io {
                    path: rel.to_string(),
                    source,
                }
            }
        })?;

        // Defence in depth: the canonical path must still live under the root or
        // a declared read root — this is what stops a symlink whose target is
        // outside the tree.
        let anchor = self
            .anchor_of(&canonical)
            .ok_or_else(|| SandboxError::Escape(rel.to_string()))?;
        let relative = canonical
            .strip_prefix(anchor)
            .map_err(|_| SandboxError::Escape(rel.to_string()))?;

        // A symlink could resolve to a path *inside* the root but under an ignored
        // subtree; re-scan the canonical segments to refuse that too.
        if let Some(name) = self.ignored_segment(relative) {
            return Err(SandboxError::Ignored(rel.to_string(), name));
        }

        Ok(canonical)
    }

    /// Resolve a `grep` scope to the pair its walk needs: the physical
    /// (canonical) directory to walk, and the in-tree path its hits are reported
    /// under. A scope inside the tree reports under its canonical relative path,
    /// exactly as before; a scope that resolved into a read root reports under
    /// the path the caller named (normalised), since that is its in-tree identity.
    fn resolve_scope(&self, rel: &str) -> Result<(PathBuf, PathBuf), SandboxError> {
        let canonical = self.resolve(rel)?;
        let logical = match canonical.strip_prefix(&self.root) {
            Ok(inside) => inside.to_path_buf(),
            Err(_) => Path::new(rel)
                .components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .collect(),
        };
        Ok((canonical, logical))
    }

    /// The canonical target of `link` when it lies under a declared read root
    /// and not under an ignored segment there — the only symlinks a walk
    /// follows. Every other symlink (in-tree alias, undeclared target, broken
    /// link) is `None` and stays skipped; an in-tree alias stays skipped even
    /// when a read root encloses the project, since it only re-reaches content
    /// the walk already visits under its real path.
    fn read_root_target(&self, link: &Path) -> Option<PathBuf> {
        let target = link.canonicalize().ok()?;
        if target.starts_with(&self.root) {
            return None;
        }
        let read_root = self
            .read_roots
            .iter()
            .find(|read_root| target.starts_with(read_root))?;
        let below = target.strip_prefix(read_root).ok()?;
        self.ignored_segment(below).is_none().then_some(target)
    }

    /// Read a confined file, capped at the sandbox's read budget.
    fn read_file(&self, rel: &str) -> Result<ReadOutput, SandboxError> {
        let path = self.resolve(rel)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|source| SandboxError::Io {
            path: rel.to_string(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(SandboxError::NotAFile(rel.to_string()));
        }

        // Bound the read at the I/O boundary: take one byte past the cap to
        // detect truncation without ever allocating the whole file (a large
        // file in the tree must not be loaded in full just to return a slice).
        let file = std::fs::File::open(&path).map_err(|source| SandboxError::Io {
            path: rel.to_string(),
            source,
        })?;
        let mut bytes = Vec::new();
        file.take(self.max_read_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| SandboxError::Io {
                path: rel.to_string(),
                source,
            })?;
        let truncated = bytes.len() > self.max_read_bytes;
        bytes.truncate(self.max_read_bytes);
        Ok(ReadOutput {
            path: rel.to_string(),
            bytes_read: bytes.len(),
            truncated,
            content: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }

    /// Walk the regular files under `start` (an absolute, canonical path within
    /// the root or a read root) whose hits are reported under the in-tree path
    /// `logical`, honoring gitignore + `ignored_dirs` and never following a
    /// symlink out of the tree, invoking `visit(absolute, relative)` until it
    /// asks to stop.
    ///
    /// Mirrors [`logos_core::config::discovery`] — the canonical [NFR-SE-04]
    /// containment walk. With read roots declared, each walked directory is also
    /// scanned for symlinks into them ([`read_root_target`](Self::read_root_target)):
    /// a directory target is queued for its own walk under the link's in-tree
    /// path, and a file target is visited directly. Every link is followed —
    /// two links to one directory are both walked, each under its own in-tree
    /// path — **except** one whose target is, or encloses, a directory already
    /// on the chain of walks that reached it: following that would re-enter
    /// where the walk already is, so skipping it is what makes a cycle
    /// terminate. At most [`MAX_FOLLOWED_LINKS`] links are followed per call.
    ///
    /// Returns `false` when the cap left a link unfollowed — the caller then
    /// reports `truncated` — and `true` otherwise (including when `visit`
    /// asked to stop). Past the cap the walks already queued still run.
    fn walk_files(
        &self,
        start: &Path,
        logical: &Path,
        mut visit: impl FnMut(&Path, &Path) -> std::ops::ControlFlow<()>,
    ) -> bool {
        // Each queued walk carries its chain: the canonical roots of the walks
        // that led to it, itself included.
        let mut queue = VecDeque::from([(
            start.to_path_buf(),
            logical.to_path_buf(),
            vec![start.to_path_buf()],
        )]);
        let mut followed = 0usize;
        let mut capped = false;
        while let Some((physical, logical, chain)) = queue.pop_front() {
            let mut links = Vec::new();
            if self
                .walk_one(&physical, &logical, &mut links, &mut visit)
                .is_break()
            {
                return !capped;
            }
            for (target, link_logical) in links {
                if chain.iter().any(|walked| walked.starts_with(&target)) {
                    continue; // a cycle: the target is, or encloses, this chain.
                }
                // Past the cap no new link is queued, but the walks already
                // queued still run, so the answer holds everything reached.
                if followed == MAX_FOLLOWED_LINKS {
                    capped = true;
                    break;
                }
                followed += 1;
                let mut link_chain = chain.clone();
                link_chain.push(target.clone());
                queue.push_back((target, link_logical, link_chain));
            }
        }
        !capped
    }

    /// One contained walk of `start` (see [`walk_files`](Self::walk_files)):
    /// visits its regular files under `logical`, and — only with read roots
    /// declared — pushes each directory symlink into a read root onto `links`
    /// as `(canonical target, in-tree path)`.
    fn walk_one(
        &self,
        start: &Path,
        logical: &Path,
        links: &mut Vec<(PathBuf, PathBuf)>,
        visit: &mut impl FnMut(&Path, &Path) -> std::ops::ControlFlow<()>,
    ) -> std::ops::ControlFlow<()> {
        let ignored_dirs = self.ignored_dirs.clone();
        let walker = WalkBuilder::new(start)
            .require_git(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .ignore(true)
            .hidden(false)
            .parents(false)
            .follow_links(false) // never leave the tree via a symlink (NFR-SE-04).
            .filter_entry(move |entry| {
                if entry.depth() > 0 && entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    if entry.path().join(".git").exists() {
                        return false;
                    }
                    if let Some(name) = entry.file_name().to_str() {
                        return !ignored_dirs.contains(name);
                    }
                }
                true
            })
            .build();

        for result in walker {
            let Ok(entry) = result else { continue };
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            let Ok(within) = path.strip_prefix(start) else {
                continue;
            };
            // `start` itself (depth 0) reports as `logical`, never `logical/`.
            let reported = if within.as_os_str().is_empty() {
                logical.to_path_buf()
            } else {
                logical.join(within)
            };
            if file_type.is_dir() && !self.read_roots.is_empty() {
                if self
                    .scan_read_root_links(path, &reported, links, visit)
                    .is_break()
                {
                    return std::ops::ControlFlow::Break(());
                }
                continue;
            }
            // With follow_links(false) a symlink is yielded as-is; skip it
            // explicitly so a symlinked file is never read (a link into a read
            // root is picked up by the directory scan above instead).
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            // Belt-and-braces: confirm the path is under the root or a read root.
            if self.anchor_of(path).is_none() {
                continue;
            }
            if visit(path, &reported).is_break() {
                return std::ops::ControlFlow::Break(());
            }
        }
        std::ops::ControlFlow::Continue(())
    }

    /// Scan one walked directory's entries for symlinks into a read root. The
    /// walker cannot be asked: a git-ignored link (`/docs/planning` here) is
    /// pruned before it is yielded, so the directory is read directly. A link
    /// named in `ignored_dirs` is skipped; a directory target is pushed onto
    /// `links`, a file target visited under its in-tree path. Sorted, so which
    /// link reaches a shared target first is deterministic.
    fn scan_read_root_links(
        &self,
        dir: &Path,
        logical: &Path,
        links: &mut Vec<(PathBuf, PathBuf)>,
        visit: &mut impl FnMut(&Path, &Path) -> std::ops::ControlFlow<()>,
    ) -> std::ops::ControlFlow<()> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return std::ops::ControlFlow::Continue(());
        };
        let mut found: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|ft| ft.is_symlink()))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| !self.ignored_dirs.contains(name))
            })
            .filter_map(|entry| {
                let target = self.read_root_target(&entry.path())?;
                Some((target, logical.join(entry.file_name())))
            })
            .collect();
        found.sort();
        for (target, link_logical) in found {
            if target.is_dir() {
                links.push((target, link_logical));
            } else if target.is_file() && visit(&target, &link_logical).is_break() {
                return std::ops::ControlFlow::Break(());
            }
        }
        std::ops::ControlFlow::Continue(())
    }
}

// ── read ────────────────────────────────────────────────────────────────────

/// `read` arguments.
#[derive(Debug, Deserialize)]
pub struct ReadArgs {
    /// Project-relative path of the file to read.
    pub path: String,
}

/// A confined file read.
#[derive(Debug, Serialize)]
pub struct ReadOutput {
    /// The project-relative path read.
    pub path: String,
    /// Number of bytes returned (≤ the read cap).
    pub bytes_read: usize,
    /// Whether the file was longer than the cap and the content was truncated.
    pub truncated: bool,
    /// The file content (UTF-8 lossy), capped at the read budget.
    pub content: String,
}

/// Read a project-relative file, sandboxed to the project root.
#[derive(Clone)]
pub struct Read {
    sandbox: Arc<Sandbox>,
}

impl Read {
    /// Wrap a shared sandbox.
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        Self { sandbox }
    }
}

impl Tool for Read {
    const NAME: &'static str = "read";
    type Error = SandboxError;
    type Args = ReadArgs;
    type Output = ReadOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read a UTF-8 source file by its project-relative path. \
                 Confined to the project root; absolute paths, `..` traversal, and \
                 ignored directories are refused. Long files are truncated."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Project-relative path of the file to read." }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: ReadArgs) -> Result<ReadOutput, SandboxError> {
        self.sandbox.read_file(&args.path)
    }
}

// ── grep ────────────────────────────────────────────────────────────────────

/// `grep` arguments.
#[derive(Debug, Deserialize)]
pub struct GrepArgs {
    /// The regular expression to search for.
    pub pattern: String,
    /// Optional project-relative subdirectory to scope the search (default: root).
    #[serde(default)]
    pub path: Option<String>,
    /// Case-insensitive match (default false).
    #[serde(default)]
    pub case_insensitive: Option<bool>,
    /// Maximum matches to return (default 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One `grep` hit.
#[derive(Debug, Serialize)]
pub struct GrepMatch {
    /// Project-relative path of the matching file.
    pub path: String,
    /// 1-based line number.
    pub line: usize,
    /// The matching line, trimmed of trailing newline.
    pub text: String,
}

/// `grep` results.
#[derive(Debug, Serialize)]
pub struct GrepOutput {
    /// The pattern searched for.
    pub pattern: String,
    /// The matches, in walk order, capped at `limit`.
    pub matches: Vec<GrepMatch>,
    /// Whether the cap was reached (more matches may exist).
    pub truncated: bool,
}

/// Regex search across confined source files.
#[derive(Clone)]
pub struct Grep {
    sandbox: Arc<Sandbox>,
}

impl Grep {
    /// Wrap a shared sandbox.
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        Self { sandbox }
    }
}

impl Tool for Grep {
    const NAME: &'static str = "grep";
    type Error = SandboxError;
    type Args = GrepArgs;
    type Output = GrepOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Regex search across project source files (gitignore- and \
                 ignored-dirs-aware). Optionally scope to a subdirectory. Returns \
                 matching lines with file path and line number."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression to search for." },
                    "path": { "type": "string", "description": "Optional project-relative subdirectory to scope the search." },
                    "case_insensitive": { "type": "boolean", "description": "Case-insensitive match (default false)." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum matches to return (default 200)." }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: GrepArgs) -> Result<GrepOutput, SandboxError> {
        let regex = RegexBuilder::new(&args.pattern)
            .case_insensitive(args.case_insensitive.unwrap_or(false))
            .build()
            .map_err(|e| SandboxError::BadRegex(args.pattern.clone(), e.to_string()))?;

        // Scope: a subdirectory must resolve within the sandbox; default to root.
        let (start, logical) = match args.path.as_deref() {
            Some(sub) => self.sandbox.resolve_scope(sub)?,
            None => (self.sandbox.root().to_path_buf(), PathBuf::new()),
        };
        let limit = args.limit.unwrap_or(DEFAULT_MATCH_LIMIT);

        let max_file_bytes = self.sandbox.max_read_bytes as u64;
        let mut matches = Vec::new();
        let mut truncated = false;
        let complete = self.sandbox.walk_files(&start, &logical, |abs, rel| {
            // Skip files larger than the read cap: a single large file in the
            // tree must not drive an unbounded `read_to_string` allocation.
            if std::fs::metadata(abs).is_ok_and(|m| m.len() > max_file_bytes) {
                return std::ops::ControlFlow::Continue(());
            }
            // Skip unreadable / binary files silently — best-effort, like discovery.
            let Ok(contents) = std::fs::read_to_string(abs) else {
                return std::ops::ControlFlow::Continue(());
            };
            for (idx, text) in contents.lines().enumerate() {
                if regex.is_match(text) {
                    if matches.len() >= limit {
                        truncated = true;
                        return std::ops::ControlFlow::Break(());
                    }
                    matches.push(GrepMatch {
                        path: rel.to_string_lossy().into_owned(),
                        line: idx + 1,
                        text: text.to_string(),
                    });
                }
            }
            std::ops::ControlFlow::Continue(())
        });

        Ok(GrepOutput {
            pattern: args.pattern,
            matches,
            truncated: truncated || !complete,
        })
    }
}

// ── glob ────────────────────────────────────────────────────────────────────

/// `glob` arguments.
#[derive(Debug, Deserialize)]
pub struct GlobArgs {
    /// The glob pattern, matched against project-relative paths (e.g. `src/**/*.rs`).
    pub pattern: String,
    /// Maximum paths to return (default 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `glob` results.
#[derive(Debug, Serialize)]
pub struct GlobOutput {
    /// The pattern matched.
    pub pattern: String,
    /// Matching project-relative paths, sorted, capped at `limit`.
    pub paths: Vec<String>,
    /// Whether the cap was reached (more paths may exist).
    pub truncated: bool,
}

/// Glob file paths within the confined project root.
#[derive(Clone)]
pub struct Glob {
    sandbox: Arc<Sandbox>,
}

impl Glob {
    /// Wrap a shared sandbox.
    pub fn new(sandbox: Arc<Sandbox>) -> Self {
        Self { sandbox }
    }
}

impl Tool for Glob {
    const NAME: &'static str = "glob";
    type Error = SandboxError;
    type Args = GlobArgs;
    type Output = GlobOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List project files whose project-relative path matches a \
                 glob pattern (e.g. `src/**/*.rs`), gitignore- and \
                 ignored-dirs-aware."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern matched against project-relative paths." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Maximum paths to return (default 200)." }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: GlobArgs) -> Result<GlobOutput, SandboxError> {
        let glob = GlobBuilder::new(&args.pattern)
            .literal_separator(true) // `*` does not cross `/`; `**` does.
            .build()
            .map_err(|e| SandboxError::BadGlob(args.pattern.clone(), e.to_string()))?
            .compile_matcher();
        let limit = args.limit.unwrap_or(DEFAULT_MATCH_LIMIT);

        let mut paths = Vec::new();
        let mut truncated = false;
        let root = self.sandbox.root().to_path_buf();
        let complete = self.sandbox.walk_files(&root, Path::new(""), |_abs, rel| {
            if glob.is_match(rel) {
                if paths.len() >= limit {
                    truncated = true;
                    return std::ops::ControlFlow::Break(());
                }
                paths.push(rel.to_string_lossy().into_owned());
            }
            std::ops::ControlFlow::Continue(())
        });
        paths.sort();

        Ok(GlobOutput {
            pattern: args.pattern,
            paths,
            truncated: truncated || !complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The security contract hinges on `is_containment_refusal` partitioning the
    /// arms exactly: the four [NFR-SE-04] path-escape refusals are turn-fatal, the
    /// rest are benign, recoverable faults (CR-063). Pin every arm so a future
    /// arm added without a category — or mis-categorized — fails here rather than
    /// silently escaping (a containment miss) or aborting a turn (a benign hit).
    #[test]
    fn is_containment_refusal_partitions_the_arms_exactly() {
        // The four containment / path-escape refusals — turn-fatal.
        for containment in [
            SandboxError::AbsolutePath("/etc/passwd".into()),
            SandboxError::Traversal("../../etc/passwd".into()),
            SandboxError::Ignored("target/x".into(), "target".into()),
            SandboxError::Escape("link".into()),
        ] {
            assert!(
                containment.is_containment_refusal(),
                "{containment:?} must be a containment refusal"
            );
        }

        // The benign I/O / argument faults — recoverable route-around ([FR-UI-28]).
        for benign in [
            SandboxError::NotFound("missing.rs".into()),
            SandboxError::NotAFile("src".into()),
            SandboxError::Io {
                path: "x".into(),
                source: std::io::Error::other("boom"),
            },
            SandboxError::BadGlob("[".into(), "unclosed".into()),
            SandboxError::BadRegex("(".into(), "unclosed".into()),
            SandboxError::BadReadRoot {
                entry: "../docs".into(),
                resolved: "/x/docs".into(),
                reason: "does not exist".into(),
            },
        ] {
            assert!(
                !benign.is_containment_refusal(),
                "{benign:?} must stay a benign, recoverable fault"
            );
        }
    }
}
