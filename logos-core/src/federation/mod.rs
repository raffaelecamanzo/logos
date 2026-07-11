//! Workspace **federation** — the in-memory overlay that turns a parent folder
//! of sibling repositories into one queryable workspace ([federation component],
//! [FR-WS-01], [ADR-52]).
//!
//! Federation is an **overlay, never a union** ([ADR-52]): each member keeps its
//! own `.logos/logos.db` and its single-root behaviour byte-for-byte unchanged.
//! This module is the foundation the rest of the overlay is built on. Here we
//! own:
//!
//! - the manifest schema + parse/write ([`manifest`]);
//! - [`discover`] — the up-tree walk that locates the manifest, resolves and
//!   validates each member, and returns the [`Federation`] member set;
//! - the [`registry`] — the `root → Engine` registry over the member set, with
//!   the repo-qualified fan-out helper and the `Backing::Single | Federated`
//!   serve-layer choice ([FR-WS-03], [NFR-PE-10]);
//! - [`discover_candidates`] — the child-directory scan `logos init
//!   --workspace` proposes for approval before any manifest exists
//!   ([`enable`], [FR-WS-02]);
//! - [`enable`] — the `logos init --workspace` orchestration: per-member
//!   non-clobber `init`, the incremental manifest write, and the workspace
//!   MCP injection ([FR-WS-02]).
//! - the [`bridge`] — the in-memory cross-service contract bridge: reads each
//!   member's contract surface through its read pool, matches portable keys
//!   across members exactly-one, and emits ephemeral `BridgeEdge` values cached
//!   on member sync-stamps; never persisted, never `ATTACH`-ed ([FR-WS-04]).
//!
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//!
//! # Single-root invariant
//! When **no** manifest is found anywhere up-tree, [`discover`] returns
//! `Ok(None)` and touches nothing — behaviour is identical to today and no file
//! is created ([FR-WS-01]). Federation is purely additive and read-only
//! ([ADR-52] Reversibility).
//!
//! # Naming
//! The internal module is `federation` (not `workspace`) to avoid collision with
//! git-worktree root resolution ([`crate::workspace`]) and `TargetClass::Workspace`;
//! the user-facing term is "workspace" ([ADR-52] Notes).
//!
//! [federation component]: ../../../docs/specs/architecture/components/federation.md
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

pub mod bridge;
pub mod enable;
pub mod manifest;
pub mod registry;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::ConfigError;
use crate::workspace::{is_git_root, resolve_root};

pub use bridge::{BridgeEdge, BridgeEndpoint, ContractBridge, ContractNode, MemberContracts};
pub use manifest::{Link, MANIFEST_FILENAME};
pub use registry::{Backing, EngineRegistry, MemberEngine, MemberScoped, RegistryMode};

/// One resolved, validated member repository of a [`Federation`] ([FR-WS-01]).
///
/// A member is always a **distinct git root contained within the workspace**;
/// [`discover`] drops anything that is not. The [`name`](Self::name) is the
/// member's path relative to the workspace root — the stable label the registry
/// and cross-service read-models use to *repo-qualify* results ([FR-WS-03]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Member {
    /// The member's path relative to the workspace root (e.g. `"api"`,
    /// `"services/web"`) — the stable, human-facing label for the member.
    pub name: String,
    /// The member repository's resolved (symlink-canonical) working-tree root.
    pub root: PathBuf,
}

/// A discovered application workspace — the resolved member set the overlay
/// federates ([FR-WS-01], [ADR-52]).
///
/// Produced by [`discover`] from a [`manifest::Manifest`]. Every [`Member`] here
/// has already been resolved to a distinct git root and validated for
/// containment; [`default`](Self::default), when present, is guaranteed to name
/// one of [`members`](Self::members).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Federation {
    /// The workspace name (`[workspace] name`).
    pub name: String,
    /// The workspace root — the directory containing the manifest, and the
    /// containment boundary every member must sit within.
    pub root: PathBuf,
    /// The resolved, de-duplicated member set: explicit `members` (in manifest
    /// order) unioned with autodiscovered child repositories (sorted).
    pub members: Vec<Member>,
    /// The default member's [`name`](Member::name), if `[workspace] default`
    /// named a member that survived resolution; otherwise `None`.
    pub default: Option<String>,
    /// User-asserted cross-service links (`[[links]]`), carried through verbatim
    /// for the contract bridge ([FR-WS-04]).
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    pub links: Vec<Link>,
}

/// Discover the workspace by walking **up** from `hint`'s resolved root
/// ([FR-WS-01], [ADR-52]).
///
/// `hint` is resolved to its working-tree root ([`resolve_root`]); the walk then
/// climbs its ancestors for a [`MANIFEST_FILENAME`]. On finding one, each
/// declared member is resolved through [`resolve_root`] and **kept only if** it
/// is a distinct git root contained within the workspace directory; duplicates
/// and path-escapes are dropped. `[workspace.autodiscover]`, when enabled,
/// unions immediate child directories that are git roots (or already carry
/// `.logos/logos.db`) with the explicit members.
///
/// # Returns
/// - `Ok(None)` — **no manifest anywhere up-tree**: single-root mode, byte-for-byte
///   unchanged, nothing created ([FR-WS-01]).
/// - `Ok(Some(federation))` — a manifest was found and parsed; the member set is
///   resolved (possibly empty if every declared member was dropped).
///
/// # Errors
/// Returns a [`ConfigError`] (exit code 2) when a manifest **is** found but is
/// unreadable ([`ConfigError::Io`]) or malformed / has an unknown key
/// ([`ConfigError::Parse`]) — a discovered-but-broken manifest fails loud rather
/// than silently degrading to single-root ([ADR-14]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub fn discover(hint: &Path) -> Result<Option<Federation>, ConfigError> {
    let start = resolve_root(hint);
    let Some(manifest_path) = find_manifest_uptree(&start) else {
        // No manifest up-tree → single-root, unchanged, nothing created.
        return Ok(None);
    };

    let manifest = manifest::parse(&manifest_path)?;

    // Surface the adopted manifest so an *unexpected* federation is visible
    // rather than silent: the walk deliberately climbs above the resolved git
    // root (the manifest lives at the parent folder, outside any member repo),
    // so a stray `logos.workspace.toml` in an ancestor would otherwise flip
    // single-root to federated with no trace ([FR-WS-01]).
    tracing::info!(
        manifest = %manifest_path.display(),
        "workspace manifest adopted — operating in federated mode"
    );

    // The workspace root is the manifest's directory — the containment boundary
    // for every member. Canonicalise so escape checks compare like-for-like.
    let root_raw = manifest_path
        .parent()
        .expect("a located manifest file has a parent directory")
        .to_path_buf();
    let root = root_raw.canonicalize().unwrap_or(root_raw);

    let members = resolve_members(&root, &manifest.workspace);

    // Keep `default` only when it names a member that survived resolution, so
    // downstream can trust it points at a real member. Match by the SAME
    // normalisation used to derive member names (resolve → canonicalise →
    // relativise), so a `default` written non-canonically (`"api/"`, `"./api"`)
    // still binds its member, and the stored value is the canonical member name.
    let default = manifest
        .workspace
        .default
        .as_deref()
        .and_then(|spec| member_name(&root, &root.join(spec)))
        .filter(|name| members.iter().any(|m| &m.name == name));

    Ok(Some(Federation {
        name: manifest.workspace.name,
        root,
        members,
        default,
        links: manifest.links,
    }))
}

/// Walk `start` and its ancestors for a [`MANIFEST_FILENAME`]; the first hit
/// (nearest ancestor) wins, or `None` at the filesystem root.
fn find_manifest_uptree(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(MANIFEST_FILENAME))
        .find(|candidate| candidate.is_file())
}

/// Resolve and validate the member set: explicit members (manifest order) then
/// autodiscovered children (sorted), de-duplicated by canonical root.
fn resolve_members(root: &Path, workspace: &manifest::WorkspaceSection) -> Vec<Member> {
    let mut members: Vec<Member> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    let mut push = |members: &mut Vec<Member>, member: Member| {
        if seen.insert(member.root.clone()) {
            members.push(member);
        }
    };

    // Explicit members, in manifest order — resolved to a distinct git root.
    for spec in &workspace.members {
        if let Some(member) = resolve_explicit_member(root, spec) {
            push(&mut members, member);
        }
    }

    // Autodiscovered children (git root OR carries a .logos DB), when enabled.
    if workspace.autodiscover.as_ref().is_some_and(|a| a.enabled) {
        for member in discover_candidates(root) {
            push(&mut members, member);
        }
    }

    members
}

/// Resolve one explicit `members` entry (a path relative to the workspace root)
/// to a validated [`Member`], or `None` if it is not a distinct git root or
/// escapes the workspace.
fn resolve_explicit_member(root: &Path, spec: &str) -> Option<Member> {
    let candidate = root.join(spec);
    let resolved = resolve_root(&candidate);
    // Must be a *distinct git root* — a non-repo directory (or a subdir of one)
    // is dropped ([FR-WS-01]).
    if !is_git_root(&resolved) {
        return None;
    }
    validate_member(root, &resolved)
}

/// Enumerate immediate child directories of `root` that qualify as members: a
/// git root, or a directory already carrying `.logos/logos.db` ([FR-WS-01]).
/// Sorted by name for deterministic ordering ([NFR-RA-06]).
///
/// Two callers: [`resolve_members`]'s `[workspace.autodiscover]` union (an
/// *existing* manifest, already canonicalised by [`discover`]), and `logos
/// init --workspace`'s candidate-approval scan
/// ([`enable::candidates_for_approval`], [FR-WS-02]) — run **before** any
/// manifest exists, so it cannot go through [`discover`], and may be handed a
/// non-canonical `root` (a raw CLI hint). `root` is canonicalised here (not
/// just relied on from the caller) so [`validate_member`]'s `strip_prefix`
/// against it always lines up with each candidate's own canonicalised path.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub fn discover_candidates(root: &Path) -> Vec<Member> {
    let root = &root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    dirs.into_iter()
        .filter_map(|dir| {
            // Compute `is_git_root` once — it spawns a `git` subprocess, so the
            // admission test and the resolution branch must share the result.
            let is_root = is_git_root(&dir);
            if !is_root && !dir.join(".logos").join("logos.db").is_file() {
                return None;
            }
            // A git-root child resolves through the same primitive; a DB-only
            // (non-git) child is its own root.
            let resolved = if is_root { resolve_root(&dir) } else { dir };
            validate_member(root, &resolved)
        })
        .collect()
}

/// Turn a resolved member root into a [`Member`], enforcing containment: the
/// (canonical) root must sit *within* the workspace and not *be* the workspace
/// itself. A member that escapes the parent — via `..` or a symlink — is dropped
/// ([FR-WS-01], [NFR-SE-04]).
fn validate_member(workspace_root: &Path, resolved: &Path) -> Option<Member> {
    let name = member_name(workspace_root, resolved)?;
    // `member_name` already proved `resolved` canonicalises, so this succeeds.
    let root = resolved.canonicalize().ok()?;
    Some(Member { name, root })
}

/// The canonical member name for a path resolved within the workspace: its
/// symlink-canonical form relativised to `workspace_root`, with `/` separators.
/// Returns `None` when the path cannot be canonicalised, escapes the workspace,
/// or *is* the workspace root (the parent folder is not one of its own members).
///
/// This is the single normalisation both member resolution ([`validate_member`])
/// and `default` matching ([`discover`]) share, so a `default` written
/// non-canonically still binds the member it names ([FR-WS-01], [NFR-SE-04]).
fn member_name(workspace_root: &Path, path: &Path) -> Option<String> {
    let canon = path.canonicalize().ok()?;
    let rel = canon.strip_prefix(workspace_root).ok()?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    Some(rel.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    /// Run a git command in `cwd`, panicking on failure — fixtures only.
    fn sh_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
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

    /// Initialise `dir` as a committed git repository.
    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        sh_git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("f.txt"), "x\n").unwrap();
        sh_git(dir, &["add", "."]);
        sh_git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// Write a manifest into `dir`.
    fn write_manifest(dir: &Path, body: &str) {
        fs::write(dir.join(MANIFEST_FILENAME), body).unwrap();
    }

    /// Member names, in the order `discover` returned them.
    fn names(fed: &Federation) -> Vec<String> {
        fed.members.iter().map(|m| m.name.clone()).collect()
    }

    /// With a manifest, explicit members resolve to distinct git roots and
    /// duplicates collapse to one.
    #[test]
    fn discovers_explicit_members_deduplicated() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("web"));
        init_repo(&root.join("api"));
        // "api" listed twice (once trailing-slashed) must de-duplicate.
        write_manifest(
            root,
            "[workspace]\nname = \"shop\"\nmembers = [\"web\", \"api\", \"api/\"]\n",
        );

        let fed = discover(root).unwrap().expect("a manifest yields a workspace");
        assert_eq!(fed.name, "shop");
        assert_eq!(names(&fed), ["web", "api"]);
        assert_eq!(
            fed.root.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    /// A member that escapes the workspace via `..` is dropped even when it is a
    /// real git root elsewhere on disk ([NFR-SE-04]).
    #[test]
    fn drops_a_member_that_escapes_the_parent() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        init_repo(&parent.join("inside"));
        // A real repo *outside* the workspace, reachable only via `..`.
        init_repo(&tmp.path().join("outside"));
        write_manifest(
            &parent,
            "[workspace]\nname = \"w\"\nmembers = [\"inside\", \"../outside\"]\n",
        );

        let fed = discover(&parent).unwrap().unwrap();
        assert_eq!(names(&fed), ["inside"], "the ../ escape is dropped");
    }

    /// A member directory that is not a git root is dropped.
    #[test]
    fn drops_a_non_git_root_member() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("real"));
        fs::create_dir_all(root.join("plain")).unwrap(); // exists, but not a repo
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"real\", \"plain\", \"ghost\"]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(
            names(&fed),
            ["real"],
            "a non-repo dir and a nonexistent path are both dropped"
        );
    }

    /// `[workspace.autodiscover]` unions child git roots (and `.logos`-carrying
    /// dirs) with the explicit members, without duplicating an overlap.
    #[test]
    fn autodiscover_unions_child_roots_with_explicit_members() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api")); // explicit AND a child root
        init_repo(&root.join("web")); // autodiscovered only
        // A non-git child that already carries a Logos DB is a member too.
        let dbonly = root.join("legacy");
        fs::create_dir_all(dbonly.join(".logos")).unwrap();
        fs::write(dbonly.join(".logos").join("logos.db"), b"db").unwrap();
        // A plain child dir that is neither is ignored.
        fs::create_dir_all(root.join("docs")).unwrap();

        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n\n[workspace.autodiscover]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        // Explicit "api" first; then autodiscovered, sorted: legacy, web.
        assert_eq!(names(&fed), ["api", "legacy", "web"]);
    }

    /// A disabled autodiscover section falls back to explicit members only.
    #[test]
    fn autodiscover_disabled_keeps_only_explicit_members() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        init_repo(&root.join("web"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n\n\
             [workspace.autodiscover]\nenabled = false\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["api"]);
    }

    /// `default` is retained only when it names a surviving member; a `default`
    /// pointing at a dropped member is nulled out.
    #[test]
    fn default_is_validated_against_the_resolved_set() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\ndefault = \"api\"\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(fed.default.as_deref(), Some("api"));

        // A default that names a non-member is dropped.
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\ndefault = \"nope\"\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(fed.default, None);
    }

    /// The discovery walks *up*: a hint deep inside a member still finds the
    /// manifest at the workspace parent.
    #[test]
    fn discover_walks_up_from_a_nested_hint() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let api = root.join("api");
        init_repo(&api);
        fs::create_dir_all(api.join("src")).unwrap();
        write_manifest(root, "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n");

        let fed = discover(&api.join("src")).unwrap().unwrap();
        assert_eq!(fed.name, "w");
        assert_eq!(names(&fed), ["api"]);
    }

    /// `[[links]]` are carried through verbatim onto the [`Federation`].
    #[test]
    fn links_are_carried_through() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n\n\
             [[links]]\nrelation = \"http_call\"\nfrom = \"web::c\"\nto = \"api::h\"\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(fed.links.len(), 1);
        assert_eq!(fed.links[0].relation, "http_call");
    }

    /// A manifest whose every declared member is dropped still yields a
    /// workspace (`Some` with an empty member set) — NOT single-root `None`.
    /// This distinction is load-bearing: an empty-but-present workspace must not
    /// be silently downgraded to the no-manifest path.
    #[test]
    fn an_all_dropped_member_set_stays_some_not_single_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("plain")).unwrap(); // not a git root
        write_manifest(
            root,
            "[workspace]\nname = \"empty\"\nmembers = [\"plain\"]\ndefault = \"plain\"\n",
        );

        let fed = discover(root)
            .unwrap()
            .expect("a present manifest yields Some, even with no valid members");
        assert_eq!(fed.name, "empty");
        assert!(fed.members.is_empty(), "the non-git member was dropped");
        assert_eq!(fed.default, None, "a default naming a dropped member is nulled");
    }

    /// A multi-segment member (a git root at `services/web`) resolves to a
    /// nested, `/`-separated name — exercising the relative-path derivation.
    #[test]
    fn discovers_a_nested_member_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("services").join("web"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"services/web\"]\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["services/web"]);
    }

    /// `default` binds its member even when written non-canonically (a trailing
    /// slash, a leading `./`), and the stored value is the canonical member name.
    #[test]
    fn default_matches_a_non_canonical_spec() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        for spec in ["api/", "./api"] {
            write_manifest(
                root,
                &format!("[workspace]\nname = \"w\"\nmembers = [\"api\"]\ndefault = \"{spec}\"\n"),
            );
            let fed = discover(root).unwrap().unwrap();
            assert_eq!(
                fed.default.as_deref(),
                Some("api"),
                "default {spec:?} binds member \"api\" as its canonical name"
            );
        }
    }

    // ── the single-root invariant (FR-WS-01) ──────────────────────────────

    /// With NO manifest anywhere up-tree, discovery yields `None` and creates
    /// nothing — the byte-for-byte-unchanged single-root case.
    #[test]
    fn no_manifest_is_single_root_and_creates_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("solo"));

        let before = dir_snapshot(root);
        let result = discover(&root.join("solo")).unwrap();
        assert!(result.is_none(), "no manifest → single-root (None)");
        let after = dir_snapshot(root);
        assert_eq!(before, after, "discovery must not create or touch any file");
    }

    /// A discovered-but-malformed manifest fails loud (exit 2), never silently
    /// degrading to single-root.
    #[test]
    fn a_malformed_manifest_fails_loud() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_manifest(root, "[workspace]\nname = \"w\"\nbogus_key = 1\n");
        let err = discover(root).expect_err("an unknown key must fail loud");
        assert_eq!(err.exit_code(), 2);
    }

    /// A recursive sorted listing of every path under `root` — used to assert
    /// discovery is side-effect-free.
    fn dir_snapshot(root: &Path) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let mut entries: Vec<PathBuf> = match fs::read_dir(dir) {
                Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
                Err(_) => return,
            };
            entries.sort();
            for p in entries {
                out.push(p.clone());
                if p.is_dir() {
                    walk(&p, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out
    }
}
