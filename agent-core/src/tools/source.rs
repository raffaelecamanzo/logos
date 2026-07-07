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
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md

use std::collections::HashSet;
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
    /// symlink pointing out of the tree.
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

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a caller-supplied project-relative path to a canonical path
    /// confined to the root ([NFR-SE-04]).
    ///
    /// Refuses, **before** touching the filesystem, an absolute path, a `..`
    /// component, or any segment named in `ignored_dirs`; then canonicalises and
    /// re-checks `starts_with(root)` so a symlink cannot escape, and re-scans the
    /// canonical path's segments for ignored directories (catching a symlink
    /// *into* an ignored subtree).
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

        // Defence in depth: the canonical path must still live under the root —
        // this is what stops a symlink whose target is outside the tree.
        let relative = canonical
            .strip_prefix(&self.root)
            .map_err(|_| SandboxError::Escape(rel.to_string()))?;

        // A symlink could resolve to a path *inside* the root but under an ignored
        // subtree; re-scan the canonical segments to refuse that too.
        for component in relative.components() {
            if let Component::Normal(name) = component {
                if let Some(name) = name.to_str() {
                    if self.ignored_dirs.contains(name) {
                        return Err(SandboxError::Ignored(rel.to_string(), name.to_string()));
                    }
                }
            }
        }

        Ok(canonical)
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

    /// Walk the regular files under `start` (an absolute path within the root),
    /// honoring gitignore + `ignored_dirs` and never following a symlink out of
    /// the tree, invoking `visit(absolute, relative)` until it asks to stop.
    ///
    /// Mirrors [`logos_core::config::discovery`] — the canonical [NFR-SE-04]
    /// containment walk.
    fn walk_files(
        &self,
        start: &Path,
        mut visit: impl FnMut(&Path, &Path) -> std::ops::ControlFlow<()>,
    ) {
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
            // With follow_links(false) a symlink is yielded as-is; skip it
            // explicitly so a symlinked file is never read.
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            // Belt-and-braces: confirm the path is under the root.
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if visit(path, relative).is_break() {
                break;
            }
        }
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
        let start = match args.path.as_deref() {
            Some(sub) => self.sandbox.resolve(sub)?,
            None => self.sandbox.root().to_path_buf(),
        };
        let limit = args.limit.unwrap_or(DEFAULT_MATCH_LIMIT);

        let max_file_bytes = self.sandbox.max_read_bytes as u64;
        let mut matches = Vec::new();
        let mut truncated = false;
        self.sandbox.walk_files(&start, |abs, rel| {
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
            truncated,
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
        self.sandbox.walk_files(&root, |_abs, rel| {
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
            truncated,
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
        ] {
            assert!(
                !benign.is_containment_refusal(),
                "{benign:?} must stay a benign, recoverable fault"
            );
        }
    }
}
