//! Single admission authority ([FR-SY-11], [ADR-48]) — the one predicate that
//! answers "would a fresh [`index`] admit path P?".
//!
//! The full walk ([`discover`](super::discover), [FR-IX-02]) and the incremental
//! path ([`sync`], the [filesystem-watcher] `classify`) must agree on what is
//! indexable, but historically diverged: the walk honours `.gitignore` and the
//! nested-`.git` boundary; the incremental path did not. [`AdmissionAuthority`]
//! encapsulates the **discovery-side gates** — nested-`.git`-boundary walk-up,
//! an `ignore`-crate gitignore matcher, `ignored_dirs`, include/exclude globs,
//! and the size cap — so both paths can consult one authority. The
//! extension/doc/config predicate (`admits_file`) is composed **at the call
//! site** ([ADR-48]); this predicate deliberately stops at the walk-level gates
//! so it can live in the config layer without depending on the language
//! registry.
//!
//! # Parity with the walk
//! Built from the **same [`Config`]** the walk uses, with the same matcher flags
//! ([`discover`](super::discovery)): gitignore honoured even outside a git repo,
//! no global gitignore, and no ignore files read above the root. On a tree with
//! only root-level ignore files, [`AdmissionAuthority::admits_path`] returns the
//! walk's per-file verdict exactly (proven by the parity unit test).
//!
//! # v1 limitation ([ADR-48])
//! The gitignore matcher is **root-anchored**: it reads `<root>/.gitignore`,
//! `<root>/.ignore`, and `<root>/.git/info/exclude`, but **not** nested
//! `.gitignore` files in subdirectories. This is sufficient for the root-level
//! scratch the CR targets (`.worktrees`, `.playwright-mcp`); exact nested parity
//! is a tracked follow-up. A per-path check against a nested `.gitignore` rule
//! therefore admits where the full walk would exclude — pinned by
//! [`tests::nested_gitignore_is_not_read_v1_limitation`].
//!
//! [FR-SY-11]: ../../../docs/specs/requirements/FR-SY-11.md
//! [FR-IX-02]: ../../../docs/specs/requirements/FR-IX-02.md
//! [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
//! [`index`]: ../../../docs/specs/requirements/FR-IX-01.md
//! [`sync`]: ../../../docs/specs/requirements/FR-SY-01.md
//! [filesystem-watcher]: ../../../docs/specs/architecture/integrations/filesystem-watcher.md

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::GlobSet;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use super::error::ConfigError;
use super::globs;
use super::settings::Config;

/// A reusable, `Config`-derived predicate mirroring the full walk's admission
/// ([FR-SY-11], [ADR-48]).
///
/// Build once (per `sync`, or `Arc`-cached in the watcher — the boundary walk-up
/// is a few `stat`s and the gitignore matcher compiles once), then query many
/// paths with [`admits_path`](Self::admits_path). It composes with `admits_file`
/// at the call site to decide final indexability.
#[derive(Debug, Clone)]
pub struct AdmissionAuthority {
    /// The canonicalised project root — the anchor for relativisation, the
    /// boundary walk-up, and the gitignore matcher ([NFR-SE-04]).
    root: PathBuf,
    /// Compiled include globs (default `**`); a file must match one.
    include: GlobSet,
    /// Compiled exclude globs; a file matching any is rejected.
    exclude: GlobSet,
    /// Directory *names* pruned anywhere in the tree.
    ignored_dirs: HashSet<String>,
    /// Files strictly larger than this (bytes) are rejected ([FR-CF-04]).
    max_file_size: u64,
    /// Root-anchored gitignore matcher (`.gitignore` ∪ `.ignore` ∪
    /// `.git/info/exclude`). See the module-level v1 limitation.
    gitignore: Gitignore,
    /// The sanctioned external docs root resolved from `.swe-skills`, or `None`
    /// when absent/misconfigured ([FR-IX-10]). Mirrors the walk's carve-out so a
    /// git-ignored sanctioned doc reached through a directory-symlink is admitted
    /// exactly as [`discover`](super::discover) yields it ([CR-071]).
    sanctioned_docs_root: Option<PathBuf>,
    /// The compiled documentation globs, or `None` when documentation is disabled
    /// ([FR-DG-01]). Gates the sanctioned-symlink carve-out to documentation only,
    /// as the walk's sub-walk does.
    doc_globs: Option<globs::DocGlobs>,
}

impl AdmissionAuthority {
    /// Build the authority from the project `root` and the `config` the walk uses.
    ///
    /// Canonicalises `root` once (the containment anchor), compiles the
    /// include/exclude globs through the same [`globs::compile`] the walk uses,
    /// and builds the root-anchored gitignore matcher with the walk's flags.
    ///
    /// # Errors
    /// - [`ConfigError::InvalidRoot`] if `root` is missing or not a directory.
    /// - [`ConfigError::EscapingPattern`] / [`ConfigError::BadGlob`] if an
    ///   include/exclude glob escapes the root or fails to compile (exit 2) —
    ///   the same failures [`discover`](super::discover) raises.
    pub fn from_config(root: &Path, config: &Config) -> Result<Self, ConfigError> {
        let root = root.canonicalize().map_err(|_| ConfigError::InvalidRoot {
            path: root.to_path_buf(),
        })?;
        if !root.is_dir() {
            return Err(ConfigError::InvalidRoot { path: root });
        }

        let include = globs::compile(&config.include)?;
        let exclude = globs::compile(&config.exclude)?;
        let ignored_dirs: HashSet<String> =
            config.semantics.ignored_dirs.iter().cloned().collect();
        let gitignore = build_root_gitignore(&root);
        // Resolve the sanctioned docs root and doc globs from the SAME config the
        // walk uses, so the carve-out below stays in parity with `discover`
        // ([FR-IX-10]/[CR-071], [FR-SY-11]).
        let sanctioned_docs_root = super::discovery::resolve_sanctioned_docs_root(&root);
        let doc_globs = config.documentation.compile()?;

        Ok(Self {
            root,
            include,
            exclude,
            ignored_dirs,
            max_file_size: config.max_file_size,
            gitignore,
            sanctioned_docs_root,
            doc_globs,
        })
    }

    /// Would a fresh `index` admit `path` as a discovery candidate?
    ///
    /// `path` may be absolute (under the canonicalised root) or root-relative;
    /// anything escaping the root (a `..` component, or an absolute path outside
    /// the root) is rejected. Returns `false` for a path the walk would prune —
    /// under a nested-`.git` boundary, matched by the gitignore matcher, under an
    /// `ignored_dirs` name, failing the include/exclude globs, a symlink, a
    /// non-regular file, an oversize file, or a path that cannot be stat'd.
    ///
    /// This is the **walk-level** verdict only; the caller composes it with
    /// `admits_file` (the extension/doc/config claim) for final indexability
    /// ([ADR-48]).
    #[must_use]
    pub fn admits_path(&self, path: &Path) -> bool {
        let Some(rel) = self.relativize(path) else {
            return false; // escapes the root, or resolves to the root itself
        };

        // Ancestor directory components (every component but the filename). The
        // walk prunes a directory (depth > 0) that is a nested `.git` boundary or
        // whose name is in `ignored_dirs`, excluding its whole subtree — so a file
        // is rejected if any ancestor directory triggers either rule. The root
        // itself (depth 0) is never a boundary, so we never inspect it.
        let components: Vec<&std::ffi::OsStr> =
            rel.components().map(Component::as_os_str).collect();
        let mut ancestor = self.root.clone();
        for name in &components[..components.len().saturating_sub(1)] {
            ancestor.push(name);
            // Nested `.git` boundary — a linked worktree (`.git` gitlink file), a
            // vendored repo, or a submodule (`.git` directory). Mirrors the walk's
            // `entry.path().join(".git").exists()` prune.
            if ancestor.join(".git").exists() {
                return false;
            }
            // `ignored_dirs` name prune (matched anywhere in the tree).
            if let Some(name) = name.to_str() {
                if self.ignored_dirs.contains(name) {
                    return false;
                }
            }
        }

        // Include/exclude globs, matched against the root-relative path exactly as
        // the walk does (`exclude.is_match(rel) || !include.is_match(rel)`).
        let rel_path = rel.as_path();
        if self.exclude.is_match(rel_path) || !self.include.is_match(rel_path) {
            return false;
        }

        // Sanctioned documentation-symlink carve-out ([FR-IX-10] amended by
        // [CR-071]). `discover` follows a git-ignored documentation directory-
        // symlink whose canonical target is contained, yielding the docs beneath
        // it; mirror that here — else this authority (and thus the `doctor`
        // FR-GV-20 tripwire and the `sync` reconcile) would flag a legitimately-
        // indexed sanctioned doc as admission drift the moment it is git-ignored.
        //
        // The carve-out bypasses ONLY the gitignore reject below — it does NOT
        // short-circuit the leaf-type and size checks. `discover`'s sub-walk
        // ([`discover_followed_symlink`]) still skips a symlinked/non-regular leaf
        // and routes an oversize doc to `skipped_oversize` (never `files`), so
        // returning `true` here unconditionally would over-admit exactly those two
        // — an `index`≠`sync` parity break ([FR-SY-11]). Keeping the leaf/size
        // block in force for the carved path restores the "admit exactly what the
        // walk yields" invariant.
        let carved = self.admits_via_sanctioned_doc_symlink(rel_path);

        // Gitignore matcher — check the path and every parent so a gitignored
        // *directory* rule (`build/`) excludes its descendants, replicating the
        // walk's subtree prune without descending. Root-anchored (v1 limitation).
        // Skipped for a carved sanctioned doc (the walk follows it past git-ignore).
        let abs = self.root.join(rel_path);
        if !carved
            && self
                .gitignore
                .matched_path_or_any_parents(&abs, false)
                .is_ignore()
        {
            return false;
        }

        // Symlink / non-regular-file / size, from a single `lstat` (the walk never
        // follows links and skips oversize files). An unreadable path is skipped,
        // exactly as the walk's best-effort `entry.metadata()` failure. `abs`
        // resolves intermediate symlinks (the sanctioned `docs/specs` hop) and
        // lstats the leaf, so a carved doc that is itself a symlink or oversize is
        // rejected here — matching the walk's sub-walk exactly.
        let Ok(meta) = fs::symlink_metadata(&abs) else {
            return false;
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() || !file_type.is_file() {
            return false;
        }
        if meta.len() > self.max_file_size {
            return false;
        }

        true
    }

    /// Whether `rel` is a documentation file reached through a **sanctioned,
    /// contained** directory-symlink — the gitignore-bypass gate of the walk-parity
    /// carve-out ([FR-IX-10], [CR-071]).
    ///
    /// Two conditions: `rel` must be admitted as documentation by the doc globs,
    /// and some **directory** ancestor of `rel` must be a symlink whose canonical
    /// target is contained within the project root or the sanctioned docs root
    /// ([`contained_dir_target`]). When both hold the walk follows the symlink past
    /// git-ignore, so [`admits_path`](Self::admits_path) skips its gitignore reject
    /// for this path. It does **not** admit unconditionally: the caller still
    /// applies the leaf symlink/regular-file and size checks the walk's sub-walk
    /// applies, so an oversize or symlinked-leaf sanctioned doc (which the walk does
    /// not yield) is still rejected. Documentation off ⇒ no doc globs ⇒ `false`.
    ///
    /// This is a per-path predicate, so it does not replicate the walk's global
    /// canonical-path dedup ([FR-IX-10]) — it may skip the gitignore reject for an
    /// in-tree aliasing symlink's path the walk would dedup away. That is
    /// deliberately harmless: only actually-indexed paths reach the [FR-GV-20]
    /// tripwire, and those are exactly the deduped sub-walk outputs.
    ///
    /// [`contained_dir_target`]: super::discovery::contained_dir_target
    fn admits_via_sanctioned_doc_symlink(&self, rel: &Path) -> bool {
        let Some(doc_globs) = self.doc_globs.as_ref() else {
            return false; // documentation disabled — no carve-out.
        };
        if !doc_globs.admits(&super::discovery::to_forward_slash(rel)) {
            return false; // only documentation is followed through the symlink.
        }
        // Some directory ancestor (every component but the filename) must be the
        // sanctioned, contained directory-symlink hop.
        let components: Vec<&std::ffi::OsStr> =
            rel.components().map(Component::as_os_str).collect();
        let mut ancestor = self.root.clone();
        for name in &components[..components.len().saturating_sub(1)] {
            ancestor.push(name);
            let is_symlink = fs::symlink_metadata(&ancestor)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if is_symlink
                && super::discovery::contained_dir_target(
                    &ancestor,
                    &self.root,
                    self.sanctioned_docs_root.as_deref(),
                )
                .is_some()
            {
                return true;
            }
        }
        false
    }

    /// Resolve `path` to a root-relative path of `Normal` components, or `None`
    /// if it escapes the root or resolves to the root itself.
    ///
    /// Mirrors the pipeline's `relativize`: an absolute path is stripped of the
    /// canonical root prefix; a relative path is taken as-is. `.` components are
    /// dropped; any `..`/root/prefix component is a rejection ([NFR-SE-04]).
    fn relativize(&self, path: &Path) -> Option<PathBuf> {
        let rel: &Path = if path.is_absolute() {
            path.strip_prefix(&self.root).ok()?
        } else {
            path
        };
        let mut normalized = PathBuf::new();
        for component in rel.components() {
            match component {
                Component::CurDir => continue,
                Component::Normal(part) => normalized.push(part),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        if normalized.as_os_str().is_empty() {
            None
        } else {
            Some(normalized)
        }
    }
}

/// Build the root-anchored gitignore matcher with the walk's flags.
///
/// The [`discover`](super::discovery) walk uses `git_ignore(true)`,
/// `git_exclude(true)`, `ignore(true)`, `git_global(false)`, and
/// `parents(false)`. We reproduce that per-path by adding exactly the three
/// **root-level** ignore sources to one [`GitignoreBuilder`] rooted at `root`
/// (and nothing global, and nothing above the root):
///
/// - `.git/info/exclude` — lowest precedence,
/// - `.gitignore`,
/// - `.ignore` — highest precedence (added last so its patterns win a conflict,
///   matching the `ignore` crate's precedence order).
///
/// Missing files are not an error (the walk simply finds none); a total compile
/// failure degrades to an empty matcher (best-effort, matching the walk's
/// per-line tolerance) rather than poisoning admission.
fn build_root_gitignore(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    // Add in ascending precedence; the last matching glob wins, so `.ignore`
    // (added last) outranks `.gitignore`, which outranks `.git/info/exclude`.
    builder.add(root.join(".git").join("info").join("exclude"));
    builder.add(root.join(".gitignore"));
    builder.add(root.join(".ignore"));
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::config::discover;

    /// A `Config` with a permissive, walk-parity baseline: include everything, no
    /// exclude globs, and a small size cap — so a test can add exactly the one
    /// gate it exercises. `ignored_dirs` keeps only the two names the tests use
    /// (`.git` for boundary hygiene, `target` for the ignored-dirs test) to avoid
    /// the large shipped default set masking a test's intent.
    fn test_config() -> Config {
        // `include` already defaults to `["**"]`; override only `exclude` (default
        // is the planning/security/notes set) and trim `ignored_dirs`. Struct-update
        // syntax avoids clippy's `field_reassign_with_default`.
        let mut config = Config {
            exclude: vec![],
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

    /// The full-walk verdict for `path`: is it in [`discover`]'s admitted set?
    /// Both sides are compared as canonical absolute paths.
    fn walk_admits(root: &Path, config: &Config, path: &Path) -> bool {
        let report = discover(root, config).unwrap();
        let set: BTreeSet<PathBuf> = report.files.into_iter().collect();
        set.contains(&path.canonicalize().unwrap())
    }

    #[test]
    fn admits_an_ordinary_source_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let src = root.join("src/lib.rs");
        write(&src, "pub fn f() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(authority.admits_path(&src), "ordinary source file is admitted");
        // Parity: matches the full-walk verdict.
        assert_eq!(authority.admits_path(&src), walk_admits(&root, &config, &src));
    }

    #[test]
    fn rejects_path_under_a_nested_git_boundary() {
        // A nested checkout: `wt/.git` marks `wt` as a separate working tree the
        // walk prunes wholesale. `wt` is NOT in `ignored_dirs` and the file is NOT
        // gitignored, so only the boundary rule can exclude it — isolating it.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("wt/.git"), "gitdir: /elsewhere\n");
        let nested = root.join("wt/app.rs");
        write(&nested, "fn main() {}\n");
        let sibling = root.join("app.rs");
        write(&sibling, "fn main() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(
            !authority.admits_path(&nested),
            "a path under a nested `.git` boundary is rejected"
        );
        assert!(authority.admits_path(&sibling), "the sibling outside the boundary is admitted");
        // Parity with the walk on both.
        assert_eq!(authority.admits_path(&nested), walk_admits(&root, &config, &nested));
        assert_eq!(authority.admits_path(&sibling), walk_admits(&root, &config, &sibling));
    }

    #[test]
    fn rejects_a_gitignored_path() {
        // A root `.gitignore` ignoring a whole directory prunes its subtree.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join(".gitignore"), "generated/\nsecret.rs\n");
        let ignored_dir_file = root.join("generated/out.rs");
        write(&ignored_dir_file, "fn g() {}\n");
        let ignored_file = root.join("secret.rs");
        write(&ignored_file, "fn s() {}\n");
        let kept = root.join("keep.rs");
        write(&kept, "fn k() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&ignored_dir_file), "file under a gitignored dir rejected");
        assert!(!authority.admits_path(&ignored_file), "gitignored file rejected");
        assert!(authority.admits_path(&kept), "non-ignored file admitted");
        for p in [&ignored_dir_file, &ignored_file, &kept] {
            assert_eq!(authority.admits_path(p), walk_admits(&root, &config, p), "parity for {p:?}");
        }
    }

    #[test]
    fn honors_dot_ignore_and_git_info_exclude() {
        // The matcher honours all three root-level ignore sources with the walk's
        // flags: `.ignore` and `.git/info/exclude`, not just `.gitignore`.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join(".ignore"), "byignore.rs\n");
        write(&root.join(".git/info/exclude"), "byexclude.rs\n");
        let by_ignore = root.join("byignore.rs");
        write(&by_ignore, "fn a() {}\n");
        let by_exclude = root.join("byexclude.rs");
        write(&by_exclude, "fn b() {}\n");
        let kept = root.join("kept.rs");
        write(&kept, "fn c() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&by_ignore), "`.ignore` entry rejected");
        assert!(!authority.admits_path(&by_exclude), "`.git/info/exclude` entry rejected");
        assert!(authority.admits_path(&kept));
        for p in [&by_ignore, &by_exclude, &kept] {
            assert_eq!(authority.admits_path(p), walk_admits(&root, &config, p), "parity for {p:?}");
        }
    }

    #[test]
    fn rejects_path_under_an_ignored_dir_name() {
        // `target` is an `ignored_dirs` name — pruned anywhere in the tree, with
        // no `.gitignore` entry needed.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let in_target = root.join("target/debug/build.rs");
        write(&in_target, "fn t() {}\n");
        let nested_target = root.join("crate/target/x.rs");
        write(&nested_target, "fn n() {}\n");
        let kept = root.join("crate/src.rs");
        write(&kept, "fn s() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&in_target), "top-level `target/` rejected");
        assert!(!authority.admits_path(&nested_target), "nested `target/` rejected (name-anywhere)");
        assert!(authority.admits_path(&kept));
        for p in [&in_target, &nested_target, &kept] {
            assert_eq!(authority.admits_path(p), walk_admits(&root, &config, p), "parity for {p:?}");
        }
    }

    #[test]
    fn rejects_a_glob_excluded_path() {
        // An `exclude` glob prunes exactly the matched root-relative paths.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let excluded = root.join("docs/planning/notes.rs");
        write(&excluded, "fn e() {}\n");
        let kept = root.join("docs/api.rs");
        write(&kept, "fn k() {}\n");

        let mut config = test_config();
        config.exclude = vec!["docs/planning/**".to_string()];
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&excluded), "glob-excluded path rejected");
        assert!(authority.admits_path(&kept), "non-excluded path admitted");
        for p in [&excluded, &kept] {
            assert_eq!(authority.admits_path(p), walk_admits(&root, &config, p), "parity for {p:?}");
        }
    }

    #[test]
    fn rejects_an_oversize_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let big = root.join("big.rs");
        write(&big, &"x".repeat(100));
        let small = root.join("small.rs");
        write(&small, "y\n");

        let mut config = test_config();
        config.max_file_size = 10; // below `big`, above `small`
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&big), "oversize file rejected");
        assert!(authority.admits_path(&small), "under-cap file admitted");
        for p in [&big, &small] {
            assert_eq!(authority.admits_path(p), walk_admits(&root, &config, p), "parity for {p:?}");
        }
    }

    #[test]
    fn rejects_paths_escaping_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("in.rs"), "fn i() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        // A relative parent-traversal escape and an absolute path outside the root.
        assert!(!authority.admits_path(Path::new("../outside.rs")));
        assert!(!authority.admits_path(Path::new("/etc/passwd")));
        // The root itself relativises to empty → rejected (not a file candidate).
        assert!(!authority.admits_path(&root));
    }

    #[test]
    fn missing_root_is_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let err = AdmissionAuthority::from_config(&missing, &test_config()).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidRoot { .. }));
    }

    #[test]
    fn escaping_glob_is_a_config_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut config = test_config();
        config.exclude = vec!["../escape/**".to_string()];
        let err = AdmissionAuthority::from_config(&root, &config).unwrap_err();
        assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    }

    #[test]
    #[cfg(unix)]
    fn rejects_a_symlink() {
        // The walk uses `follow_links(false)` and skips symlinks; the authority
        // mirrors that via `symlink_metadata` (lstat) so a symlink is never
        // admitted — the containment guarantee ([NFR-SE-04]). Note: we do NOT use
        // the `walk_admits` parity helper here, because it canonicalises the path,
        // which would resolve the link to its (admitted) target and mask the bug.
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let target = root.join("target.rs");
        write(&target, "fn t() {}\n");
        let link = root.join("link.rs");
        symlink(&target, &link).unwrap();

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&link), "a symlink is never admitted");
        assert!(authority.admits_path(&target), "the real target is admitted");
        // The walk agrees: the symlink path is not in `discover`'s output.
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();
        assert!(!walk.contains(&link), "the walk does not yield the symlink either");
    }

    #[test]
    fn rejects_a_non_regular_file() {
        // A directory (or any non-regular file) fails the `is_file()` gate the
        // walk applies per entry — `admits_path` answers about file candidates.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let dir = root.join("subdir");
        fs::create_dir(&dir).unwrap();
        let file = root.join("subdir/f.rs");
        write(&file, "fn f() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(!authority.admits_path(&dir), "a directory is not a file candidate");
        assert!(authority.admits_path(&file), "a regular file under it is admitted");
        // Parity: the walk never yields a directory (only files), and admits the file.
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();
        assert!(!walk.contains(&dir), "the walk does not yield the directory");
        assert_eq!(authority.admits_path(&file), walk_admits(&root, &config, &file));
    }

    #[test]
    fn rejects_a_path_not_matching_the_include_globs() {
        // The include set is a positive gate: a file matching no include glob is
        // rejected even when nothing else excludes it. The default set is `["**"]`
        // (everything), so this narrows it to exercise the `!include.is_match`
        // branch directly, not only the exclude branch.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let included = root.join("main.rs");
        write(&included, "fn main() {}\n");
        let excluded = root.join("readme.txt");
        write(&excluded, "text\n");

        let mut config = test_config();
        config.include = vec!["*.rs".to_string()];
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(authority.admits_path(&included), "matches the include glob");
        assert!(!authority.admits_path(&excluded), "matches no include glob → rejected");
        for p in [&included, &excluded] {
            assert_eq!(authority.admits_path(p), walk_admits(&root, &config, p), "parity for {p:?}");
        }
    }

    #[test]
    fn matches_the_full_walk_verdict_across_a_mixed_tree() {
        // The parity anchor ([FR-SY-11], [ADR-48]): over a tree exercising every
        // gate at once, `admits_path` returns the walk's verdict for *every*
        // regular file discoverable by a naive recursive walk of the tree.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();

        // Ordinary source (admitted).
        write(&root.join("src/main.rs"), "fn main() {}\n");
        write(&root.join("src/lib.rs"), "pub fn f() {}\n");
        // Gitignored dir + file (rejected).
        write(&root.join(".gitignore"), "gen/\n*.tmp\n");
        write(&root.join("gen/derived.rs"), "fn d() {}\n");
        write(&root.join("scratch.tmp"), "junk\n");
        // Nested `.git` boundary (rejected).
        write(&root.join(".worktrees/s/.git"), "gitdir: /x\n");
        write(&root.join(".worktrees/s/copy.rs"), "fn c() {}\n");
        // `ignored_dirs` name (rejected).
        write(&root.join("target/out.rs"), "fn o() {}\n");
        // Root-level ignore sources (rejected).
        write(&root.join(".ignore"), "vendored.rs\n");
        write(&root.join("vendored.rs"), "fn v() {}\n");

        let mut config = test_config();
        // Keep `.worktrees` out of `ignored_dirs` so the *boundary* rule (not a
        // name prune) is what excludes the nested checkout — the CR's exact case.
        config.semantics.ignored_dirs = vec!["target".to_string(), ".git".to_string()];

        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();

        // Every regular file physically present in the tree must get the same
        // verdict from `admits_path` as its membership in the walk's output.
        let mut checked = 0;
        for path in walk_all_regular_files(&root) {
            let in_walk = walk.contains(&path);
            assert_eq!(
                authority.admits_path(&path),
                in_walk,
                "admits_path disagreed with the walk for {path:?} (in_walk={in_walk})"
            );
            checked += 1;
        }
        assert!(checked >= 8, "the fixture should present at least 8 files, saw {checked}");
        // And the walk must have admitted the ordinary source and nothing under
        // the excluded subtrees — a sanity check that the fixture bit.
        assert!(walk.contains(&root.join("src/main.rs")));
        assert!(!walk.contains(&root.join("gen/derived.rs")));
        assert!(!walk.contains(&root.join(".worktrees/s/copy.rs")));
    }

    #[test]
    fn nested_gitignore_is_not_read_v1_limitation() {
        // v1 LIMITATION ([ADR-48]): the root-anchored matcher does NOT read a
        // *nested* `.gitignore`. The full walk (per-directory ignore state) WOULD
        // honour `sub/.gitignore`; the authority does not — so here `admits_path`
        // ADMITS a file the walk EXCLUDES. This test pins that documented
        // divergence so a future per-directory matcher (the tracked follow-up)
        // flips it deliberately, not by accident.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("sub/.gitignore"), "hidden.rs\n");
        let nested_ignored = root.join("sub/hidden.rs");
        write(&nested_ignored, "fn h() {}\n");

        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        assert!(
            !walk_admits(&root, &config, &nested_ignored),
            "the full walk honours the nested `.gitignore` and excludes the file",
        );
        assert!(
            authority.admits_path(&nested_ignored),
            "v1 limitation: the root-anchored authority admits it (nested `.gitignore` unread)",
        );
    }

    /// Build a project whose git-ignored `docs/specs` is a directory-symlink into
    /// a sibling sanctioned repo declared in `.swe-skills` — the [CR-071] layout.
    /// Returns (guard, canonical root, canonical sanctioned root).
    #[cfg(unix)]
    fn build_gitignored_doc_symlink_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("project");
        fs::create_dir_all(root.join("docs")).unwrap();
        write(&root.join("src/main.rs"), "fn main() {}\n");

        let sanctioned = base.join("logos-docs");
        write(&sanctioned.join("specs/ADR-46.md"), "# ADR-46\n");
        write(&sanctioned.join("specs/embedded.rs"), "pub fn x() {}\n");
        let sanctioned = sanctioned.canonicalize().unwrap();

        write(&root.join(".swe-skills"), &format!("{}\n", sanctioned.display()));
        symlink(sanctioned.join("specs"), root.join("docs/specs")).unwrap();
        // Git-ignore the symlink exactly as this project's own `.gitignore` does.
        write(&root.join(".gitignore"), "/docs/specs\n");
        (tmp, root, sanctioned)
    }

    #[test]
    #[cfg(unix)]
    fn admits_git_ignored_sanctioned_doc_reached_via_symlink() {
        // Parity with the [CR-071] walk: `discover` now yields a git-ignored
        // sanctioned doc reached through the directory-symlink, so `admits_path`
        // must admit it too — else the FR-GV-20 doctor tripwire would flag the
        // freshly-indexed doc as admission drift.
        let (_tmp, root, _sanctioned) = build_gitignored_doc_symlink_fixture();
        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        let doc = root.join("docs/specs/ADR-46.md"); // logical in-tree path
        // The walk yields it (logical path) despite the gitignore.
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();
        assert!(walk.contains(&doc), "the CR-071 walk yields the git-ignored sanctioned doc");
        // And the authority agrees — the parity invariant ([FR-SY-11], [ADR-48]).
        assert!(
            authority.admits_path(&doc),
            "admits_path admits a git-ignored sanctioned doc reached via the symlink"
        );
    }

    #[test]
    #[cfg(unix)]
    fn source_behind_sanctioned_doc_symlink_is_not_admitted() {
        // The carve-out is documentation-scoped: a source file behind the same
        // symlink is NOT a doc, so it stays rejected (gitignored) — the walk never
        // yields it either, so parity holds.
        let (_tmp, root, _sanctioned) = build_gitignored_doc_symlink_fixture();
        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        let src = root.join("docs/specs/embedded.rs");
        assert!(
            !authority.admits_path(&src),
            "source behind the doc symlink is not admitted (doc-gated carve-out)"
        );
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();
        assert!(!walk.contains(&src), "the walk does not yield the source behind the symlink");
    }

    #[test]
    #[cfg(unix)]
    fn admits_path_still_rejects_git_ignored_doc_without_sanction() {
        // No `.swe-skills` ⇒ no sanctioned root ⇒ the git-ignored doc symlink is
        // not a carve-out and the doc behind it is rejected (fail-closed), matching
        // the walk which does not follow it.
        let (_tmp, root, _sanctioned) = build_gitignored_doc_symlink_fixture();
        fs::remove_file(root.join(".swe-skills")).unwrap();
        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        let doc = root.join("docs/specs/ADR-46.md");
        assert!(
            !authority.admits_path(&doc),
            "with no sanction the git-ignored doc is rejected (fail-closed)"
        );
    }

    #[test]
    #[cfg(unix)]
    fn carve_out_rejects_an_oversize_sanctioned_doc_matching_the_walk() {
        // Parity ([FR-SY-11]): `discover`'s sub-walk routes an oversize sanctioned
        // doc into `skipped_oversize`, NOT `files`. The carve-out must not admit it
        // — the leaf/size checks stay in force for the carved path.
        let (_tmp, root, _sanctioned) = build_gitignored_doc_symlink_fixture();
        // A big markdown doc behind the sanctioned symlink, above the cap.
        write(&root.join("docs/specs/HUGE.md"), &"x".repeat(200));
        let mut config = test_config();
        config.max_file_size = 64; // below HUGE.md, above the small ADR.
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        let huge = root.join("docs/specs/HUGE.md");
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();
        assert!(!walk.contains(&huge), "the walk skips the oversize sanctioned doc (not in files)");
        assert!(
            !authority.admits_path(&huge),
            "the carve-out does not admit an oversize sanctioned doc (parity with the walk)"
        );
        // …and the small sanctioned doc is still admitted.
        assert!(authority.admits_path(&root.join("docs/specs/ADR-46.md")));
    }

    #[test]
    #[cfg(unix)]
    fn carve_out_rejects_a_symlinked_leaf_doc_matching_the_walk() {
        // Parity ([FR-SY-11]): `discover`'s sub-walk skips a symlinked (non-regular)
        // leaf. The carve-out must reject a `.md` that is itself a symlink, even
        // reached through the sanctioned directory-symlink.
        use std::os::unix::fs::symlink;
        let (_tmp, root, sanctioned) = build_gitignored_doc_symlink_fixture();
        // A real target and a symlinked `.md` leaf next to the sanctioned docs.
        write(&sanctioned.join("specs/REAL.md"), "# real\n");
        symlink(sanctioned.join("specs/REAL.md"), sanctioned.join("specs/LINK.md")).unwrap();
        let config = test_config();
        let authority = AdmissionAuthority::from_config(&root, &config).unwrap();

        let linked = root.join("docs/specs/LINK.md"); // logical path to the symlinked leaf
        let walk: BTreeSet<PathBuf> = discover(&root, &config).unwrap().files.into_iter().collect();
        assert!(!walk.contains(&linked), "the walk skips the symlinked-leaf doc");
        assert!(
            !authority.admits_path(&linked),
            "the carve-out does not admit a symlinked-leaf doc (parity with the walk)"
        );
    }

    /// Naive recursive walk collecting every regular (non-symlink) file under
    /// `root`, as canonical absolute paths — the universe the parity test checks
    /// `admits_path` against. Deliberately does *not* apply any admission rule;
    /// it descends every directory, including gitignored and boundary ones, so
    /// the parity test sees the rejected files too.
    fn walk_all_regular_files(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.file_type().is_dir() {
                    stack.push(path);
                } else if meta.file_type().is_file() {
                    out.push(path.canonicalize().unwrap());
                }
            }
        }
        out
    }
}
