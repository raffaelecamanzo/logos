//! [`Project`] — the repository's identity coordinate (`[project]`, [FR-CF-10],
//! [ADR-85], S-702).
//!
//! `[project] id` names the repository every node symbol will carry in its SCIP
//! package slot ([FR-EX-01]). It is a core-owned table ([FR-CF-09]): the core's
//! `deny_unknown_fields` parse reads it, and a malformed value is a
//! configuration error (exit 2) naming `project.id`.
//!
//! With the key absent the same coordinate is **derived**, never from where a
//! checkout is mounted, so the main checkout, a linked worktree, a clone at
//! another path and CI agree ([`resolve_project_id`]):
//!
//! 1. the normalised `origin` remote ([`normalise_remote`]) — scheme,
//!    credentials, port, query, fragment and `.git` suffix stripped, host
//!    lowercased, so
//!    `git@h:o/r.git` and `https://h/o/r` both yield `h/o/r`;
//! 2. else the directory name of the repository's **main** worktree, read
//!    through the git common directory ([`crate::workspace::main_worktree_dir`]);
//! 3. else (no git at all) the project root's own directory name.
//!
//! [FR-CF-09]: ../../../../docs/specs/requirements/FR-CF-09.md
//! [FR-CF-10]: ../../../../docs/specs/requirements/FR-CF-10.md
//! [FR-EX-01]: ../../../../docs/specs/requirements/FR-EX-01.md
//! [ADR-85]: ../../../../docs/specs/architecture/decisions/ADR-85.md

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::ConfigError;
use super::Config;

/// The coordinate used when nothing else yields one — a root with no nameable
/// directory (`/`) outside any repository. Valid, so a derivation never fails.
const FALLBACK_COORDINATE: &str = "project";

/// The `[project]` table ([FR-CF-10]).
///
/// [FR-CF-10]: ../../../../docs/specs/requirements/FR-CF-10.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// The repository's identity coordinate. Absent → derived at run time
    /// ([`resolve_project_id`]); `logos init` writes the derived value when the
    /// key is missing and never overwrites one that is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl Project {
    /// No key set — the table serialises to nothing, so a config without
    /// `[project]` keeps its read-model unchanged.
    pub(crate) fn is_empty(&self) -> bool {
        self.id.is_none()
    }

    /// A present `id` must be a coordinate ([`is_valid_coordinate`]); a space,
    /// an empty value or a disallowed character fails the load (exit 2).
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        match &self.id {
            Some(id) if !is_valid_coordinate(id) => Err(ConfigError::InvalidValue {
                key: "project.id".to_string(),
                message: format!(
                    "{id:?} is not a project coordinate: it must be non-empty, not `.`, and \
                     use only letters, digits and `._/@:-` (no spaces)"
                ),
            }),
            _ => Ok(()),
        }
    }
}

/// Is `value` a coordinate — non-empty, every character in `[A-Za-z0-9._/@:-]`,
/// and not the bare `.`, which in a symbol's package slot is the SCIP marker
/// for an empty field and would read back as no coordinate at all (S-703)?
pub fn is_valid_coordinate(value: &str) -> bool {
    !value.is_empty() && value != "." && value.chars().all(is_coordinate_char)
}

fn is_coordinate_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '@' | ':' | '-')
}

/// Normalise a git remote URL to `host/owner/repo`.
///
/// URL forms (`https://`, `ssh://`, `git://`, …) and the scp-like
/// `[user@]host:path` form drop their scheme, credentials, port, query,
/// fragment and `.git` suffix; the host is lowercased, the path keeps its case.
/// A local remote — an absolute or relative path, or a `file://` URL — keeps
/// only its final component (a checkout's `.git` directory names the
/// checkout), which is what a remote-less repository derives for itself.
/// [`derive_project_id`] follows a local remote that exists instead, so this
/// name is the answer only for one that is gone. `None` when nothing nameable
/// remains.
pub fn normalise_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return local_remote_name(rest);
        }
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        (strip_port(strip_credentials(authority)), path)
    } else if let Some((authority, path)) = scp_like(url) {
        (strip_credentials(authority), path)
    } else {
        return local_remote_name(url);
    };
    // A query or fragment names no part of the repository, and can carry a
    // token: cut it before anything is kept.
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let path = strip_git_suffix(path.trim_matches('/'));
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

/// The `[user@]host:path` form git reads as an ssh remote: a colon before any
/// slash, after more than one character — so `C:/repos/r` stays a path.
fn scp_like(url: &str) -> Option<(&str, &str)> {
    let (authority, path) = url.split_once(':')?;
    (authority.len() > 1 && !authority.contains('/')).then_some((authority, path))
}

fn strip_credentials(authority: &str) -> &str {
    authority.rsplit_once('@').map_or(authority, |(_, host)| host)
}

fn strip_port(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    }
}

fn strip_git_suffix(path: &str) -> &str {
    path.strip_suffix(".git").unwrap_or(path).trim_end_matches('/')
}

/// A local remote's name: its final path component without `.git` — or, for a
/// checkout's own `.git` directory, the checkout's name.
fn local_remote_name(path: &str) -> Option<String> {
    let mut components = path
        .split(['/', '\\'])
        .filter(|c| !c.is_empty())
        .rev();
    let last = match components.next()? {
        ".git" => components.next()?,
        last => last,
    };
    let last = strip_git_suffix(last);
    (!last.is_empty() && last != "." && last != "..").then(|| last.to_string())
}

/// The path a local `origin` names — a plain path, or a `file://` URL — or
/// `None` for a hosted remote. The same split [`normalise_remote`] makes.
fn local_remote_path(url: &str) -> Option<&str> {
    let url = url.trim();
    match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("file") => Some(rest),
        Some(_) => None,
        None if scp_like(url).is_some() => None,
        None => Some(url),
    }
}

/// The directory a local `origin` path names when it exists on this machine:
/// the repository a clone was made from. A relative path is read from the main
/// worktree, so a linked worktree finds what its main checkout finds and the
/// two derive one coordinate (CRA-04).
fn local_remote_dir(root: &Path, path: &str) -> Option<std::path::PathBuf> {
    let path = Path::new(path);
    let dir = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate::workspace::main_worktree_dir(root)
            .unwrap_or_else(|| root.to_path_buf())
            .join(path)
    };
    dir.is_dir().then_some(dir)
}

/// Replace every character a coordinate does not allow with `-`, so a derived
/// value always validates; `None` when nothing valid remains.
fn sanitise(value: &str) -> Option<String> {
    let cleaned: String = value
        .chars()
        .map(|c| if is_coordinate_char(c) { c } else { '-' })
        .collect();
    let cleaned = cleaned.trim_matches('/');
    is_valid_coordinate(cleaned).then(|| cleaned.to_string())
}

/// The project's identity coordinate: the configured `[project] id`, else the
/// derived default ([`derive_project_id`]).
pub fn resolve_project_id(root: &Path, config: &Config) -> String {
    resolve_project_id_confirmed(root, config).coordinate
}

/// A resolved coordinate, and whether this machine confirmed it.
///
/// Confirmed: a configured `[project] id`, a hosted `origin`, a local `origin`
/// that exists (followed), or — with no `origin` — the main worktree's name,
/// which git answered for. Not confirmed: the name of a local `origin` that is
/// gone, or `root`'s own name when git cannot answer for it (absent from
/// `PATH`, or refusing the repository's ownership). Such a value is the best
/// name on hand, but it moves back once the source returns or git answers
/// again — so the open-time coordinate migration never moves a recorded
/// coordinate to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCoordinate {
    pub(crate) coordinate: String,
    pub(crate) confirmed: bool,
}

/// [`resolve_project_id`], saying whether the value was confirmed.
pub(crate) fn resolve_project_id_confirmed(root: &Path, config: &Config) -> ResolvedCoordinate {
    match &config.project.id {
        Some(id) => ResolvedCoordinate { coordinate: id.clone(), confirmed: true },
        None => derive_following_local_origins(root, LOCAL_ORIGIN_HOPS),
    }
}

/// How many local `origin` hops [`derive_project_id`] follows — a clone of a
/// local mirror of a checkout is two — so a cycle of local remotes ends.
const LOCAL_ORIGIN_HOPS: usize = 4;

/// The coordinate a project derives when `[project] id` is absent — the
/// normalised `origin` remote, else the main worktree's directory name, else
/// `root`'s own directory name. Always a valid coordinate.
///
/// An `origin` that is a local path to an existing repository (a clone made
/// from a checkout, a CI cache, a mirror) is followed: the clone resolves what
/// that repository resolves, its hosted remote included.
pub fn derive_project_id(root: &Path) -> String {
    derive_following_local_origins(root, LOCAL_ORIGIN_HOPS).coordinate
}

fn derive_following_local_origins(root: &Path, hops: usize) -> ResolvedCoordinate {
    let from_remote = || {
        let url = crate::workspace::origin_url(root)?;
        let Some(path) = local_remote_path(&url) else {
            return normalise_remote(&url).map(|raw| (raw, true));
        };
        match local_remote_dir(root, path) {
            Some(source) if hops > 0 => {
                let source = derive_following_local_origins(&source, hops - 1);
                Some((source.coordinate, source.confirmed))
            }
            // An existing repository at the end of the hop budget names itself
            // the same way on every run.
            Some(_) => normalise_remote(&url).map(|raw| (raw, true)),
            // A local source that is gone leaves only its name.
            None => normalise_remote(&url).map(|raw| (raw, false)),
        }
    };
    let dir_name = |dir: &Path| {
        dir.file_name()
            .map(|name| strip_git_suffix(&name.to_string_lossy()).to_string())
    };
    from_remote()
        .or_else(|| {
            crate::workspace::main_worktree_dir(root)
                .as_deref()
                .and_then(dir_name)
                .map(|raw| (raw, true))
        })
        // Resolved first: a root spelled `.` or `x/..` has no final name.
        .or_else(|| {
            dir_name(&root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
                .map(|raw| (raw, false))
        })
        .and_then(|(raw, confirmed)| sanitise(&raw).map(|coordinate| (coordinate, confirmed)))
        .map_or_else(
            || ResolvedCoordinate { coordinate: FALLBACK_COORDINATE.to_string(), confirmed: false },
            |(coordinate, confirmed)| ResolvedCoordinate { coordinate, confirmed },
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_remote_shape_normalises_to_host_owner_repo() {
        for (url, want) in [
            ("git@h:o/r.git", "h/o/r"),
            ("https://h/o/r", "h/o/r"),
            ("https://user@h/o/r.git", "h/o/r"),
            ("ssh://git@h/o/r", "h/o/r"),
            // The same repository spelled the other ways a clone records it.
            ("https://h/o/r.git", "h/o/r"),
            ("https://h/o/r/", "h/o/r"),
            ("https://user:secret@h/o/r.git", "h/o/r"),
            ("ssh://git@h:2222/o/r.git", "h/o/r"),
            ("git://h/o/r.git", "h/o/r"),
            ("  git@h:o/r.git\n", "h/o/r"),
            // The host is case-insensitive; the path is not.
            ("git@GitHub.com:Owner/Repo.git", "github.com/Owner/Repo"),
            ("https://GITHUB.COM/Owner/Repo", "github.com/Owner/Repo"),
            // Deeper paths (a GitLab subgroup) keep every segment.
            ("git@gitlab.com:group/sub/r.git", "gitlab.com/group/sub/r"),
            // A query or fragment is no part of the repository's name — and may
            // carry a token, which must never reach config.toml or the store.
            ("https://h/o/r.git?private_token=SECRET", "h/o/r"),
            ("https://h/o/r#readme", "h/o/r"),
            ("git@h:o/r.git?ref=main", "h/o/r"),
            // A local remote that is gone keeps only its final component.
            ("/srv/git/r.git", "r"),
            ("file:///srv/git/r.git", "r"),
            ("../r", "r"),
            // A clone made from a checkout's `.git` directory names the checkout.
            ("/srv/app-main/.git", "app-main"),
            ("file:///srv/app-main/.git/", "app-main"),
        ] {
            assert_eq!(normalise_remote(url).as_deref(), Some(want), "{url:?}");
        }
    }

    #[test]
    fn a_remote_with_nothing_nameable_normalises_to_none() {
        for url in ["", "   ", "https://", "git@:o/r", "/", ".", "file:///"] {
            assert_eq!(normalise_remote(url), None, "{url:?}");
        }
    }

    #[test]
    fn a_windows_drive_path_is_a_local_remote_not_an_scp_host() {
        // `C:` is one letter before the colon — git reads it as a path, and so
        // do we: never the host `c`.
        assert_eq!(normalise_remote("C:/repos/r.git").as_deref(), Some("r"));
    }

    #[test]
    fn coordinates_accept_the_documented_alphabet_only() {
        for ok in ["h/o/r", "github.com/o/r", "a-b_c.d@e:f", "x"] {
            assert!(is_valid_coordinate(ok), "{ok:?}");
        }
        for bad in ["", " ", "a b", "a\tb", "a#b", "a\\b", "café", "a,b", "a\"b", "."] {
            assert!(!is_valid_coordinate(bad), "{bad:?}");
        }
        // Only the bare marker: dots elsewhere are ordinary characters.
        for ok in ["..", "...", "./a", "a."] {
            assert!(is_valid_coordinate(ok), "{ok:?}");
        }
    }

    #[test]
    fn a_derived_value_that_is_the_empty_slot_marker_falls_back() {
        // A coordinate of `.` would render the SCIP empty package slot, which
        // reads back as no coordinate at all (S-703).
        assert_eq!(sanitise("."), None);
        assert_eq!(sanitise("/./").as_deref(), None);
        assert_eq!(sanitise("..").as_deref(), Some(".."));
    }

    #[test]
    fn a_malformed_id_is_a_config_error_naming_the_key() {
        for bad in ["", "my repo", "o/r#1", "o\\r", "."] {
            let project = Project {
                id: Some(bad.to_string()),
            };
            let err = project.validate().expect_err(bad);
            assert!(
                matches!(&err, ConfigError::InvalidValue { key, .. } if key == "project.id"),
                "{bad:?}: {err}"
            );
            assert_eq!(err.exit_code(), 2);
            assert!(err.to_string().contains("project.id"), "{err}");
        }
        assert!(Project::default().validate().is_ok(), "an absent id is valid");
        assert!(Project {
            id: Some("h/o/r".into())
        }
        .validate()
        .is_ok());
    }

    /// Through the core's own load, the path `logos` exits 2 on: `[project]` is
    /// a core-owned table, its key validated and its unknown keys refused.
    #[test]
    fn the_project_table_is_parsed_and_validated_by_the_core_config_load() {
        let path = Path::new(".logos/config.toml");
        let config = super::super::parse_config("[project]\nid = \"h/o/r\"\n", path).unwrap();
        assert_eq!(config.project.id.as_deref(), Some("h/o/r"));
        let empty_table = super::super::parse_config("[project]\n", path).unwrap();
        assert_eq!(empty_table, Config::default(), "an empty table is the default");

        for (text, what) in [
            ("[project]\nid = \"\"\n", "empty"),
            ("[project]\nid = \"my repo\"\n", "a space"),
            ("[project]\nid = \"o/r#1\"\n", "a disallowed character"),
        ] {
            let err = super::super::parse_config(text, path).expect_err(what);
            assert!(
                matches!(&err, ConfigError::InvalidValue { key, .. } if key == "project.id"),
                "{what}: {err}"
            );
            assert_eq!(err.exit_code(), 2, "{what}");
        }
        for text in ["[project]\nname = \"x\"\n", "project = \"h/o/r\"\n", "[project]\nid = 7\n"] {
            let err = super::super::parse_config(text, path).expect_err(text);
            assert!(matches!(err, ConfigError::Parse { .. }), "{text:?}: {err}");
            assert_eq!(err.exit_code(), 2);
        }
    }

    #[test]
    fn a_configured_id_wins_over_the_derivation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = Config {
            project: Project {
                id: Some("example.org/team/app".into()),
            },
            ..Config::default()
        };
        assert_eq!(resolve_project_id(tmp.path(), &config), "example.org/team/app");
    }

    /// Run git in `cwd` for a fixture: no host identity, no background
    /// maintenance, and never walking up past the fixture's temp root.
    fn fixture_git(ceiling: &Path, cwd: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(args)
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .output()
            .expect("git is on PATH");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A committed repository at `<tmp>/<name>`, a linked worktree of it in an
    /// unrelated directory, and a clone of it at another path.
    struct Checkouts {
        _tmp: tempfile::TempDir,
        main: std::path::PathBuf,
        worktree: std::path::PathBuf,
        clone: std::path::PathBuf,
    }

    fn checkouts(name: &str) -> Checkouts {
        let tmp = tempfile::TempDir::new().unwrap();
        let ceiling = tmp.path();
        let main = tmp.path().join(name);
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("lib.rs"), "pub fn f() {}\n").unwrap();
        fixture_git(ceiling, &main, &["init", "-q", "-b", "main"]);
        fixture_git(ceiling, &main, &["add", "."]);
        fixture_git(ceiling, &main, &["commit", "-q", "-m", "initial"]);
        let worktree = tmp.path().join("elsewhere/feature-wt");
        fixture_git(
            ceiling,
            &main,
            &["worktree", "add", "-q", worktree.to_str().unwrap(), "-b", "feature"],
        );
        let clone = tmp.path().join("other/path/copy");
        std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
        fixture_git(
            ceiling,
            tmp.path(),
            &["clone", "-q", main.to_str().unwrap(), clone.to_str().unwrap()],
        );
        Checkouts {
            _tmp: tmp,
            main,
            worktree,
            clone,
        }
    }

    /// **One repository, one coordinate** — with an `origin`, every checkout
    /// resolves the normalised remote, wherever it is mounted.
    #[test]
    fn a_main_checkout_a_linked_worktree_and_a_clone_resolve_one_remote_coordinate() {
        let c = checkouts("app-main");
        let ceiling = c._tmp.path();
        let remote = "git@Example.com:team/app.git";
        fixture_git(ceiling, &c.main, &["remote", "add", "origin", remote]);
        // A clone of the same hosted repository records the same `origin`.
        fixture_git(ceiling, &c.clone, &["remote", "set-url", "origin", remote]);
        // A clone made from a local checkout of it — a CI cache, a mirror —
        // records that checkout's path as its `origin`, and still resolves the
        // hosted coordinate the checkout resolves.
        let local_clone = c._tmp.path().join("ci/cache-clone");
        fixture_git(
            ceiling,
            c._tmp.path(),
            &["clone", "-q", c.main.to_str().unwrap(), local_clone.to_str().unwrap()],
        );
        for root in [&c.main, &c.worktree, &c.clone, &local_clone] {
            assert_eq!(derive_project_id(root), "example.com/team/app", "{}", root.display());
        }
    }

    /// **CRA-04** — with no remote, a linked worktree names its main worktree's
    /// directory, never its own; a clone of that repository names it too,
    /// through the local `origin` the clone records.
    #[test]
    fn without_a_remote_every_checkout_resolves_the_main_worktree_directory_name() {
        let c = checkouts("app-main");
        assert_eq!(derive_project_id(&c.main), "app-main");
        assert_eq!(
            derive_project_id(&c.worktree),
            "app-main",
            "the worktree's own directory is `feature-wt`"
        );
        assert_eq!(derive_project_id(&c.clone), "app-main", "the clone's own directory is `copy`");
        let from_git_dir = c._tmp.path().join("other/from-git-dir");
        fixture_git(
            c._tmp.path(),
            c._tmp.path(),
            &["clone", "-q", c.main.join(".git").to_str().unwrap(), from_git_dir.to_str().unwrap()],
        );
        assert_eq!(derive_project_id(&from_git_dir), "app-main", "cloned from `app-main/.git`");
        assert_eq!(
            crate::workspace::main_worktree_dir(&c.worktree)
                .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                .as_deref(),
            Some("app-main")
        );
    }

    /// Two repositories whose `origin`s name each other: following local
    /// origins stops after a bounded number of hops and still yields a
    /// coordinate.
    #[test]
    fn a_cycle_of_local_origins_ends() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ceiling = tmp.path();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        for repo in [&a, &b] {
            std::fs::create_dir_all(repo).unwrap();
            fixture_git(ceiling, repo, &["init", "-q", "-b", "main"]);
        }
        fixture_git(ceiling, &a, &["remote", "add", "origin", b.to_str().unwrap()]);
        fixture_git(ceiling, &b, &["remote", "add", "origin", a.to_str().unwrap()]);
        let id = derive_project_id(&a);
        assert!(id == "a" || id == "b", "{id}");
        assert!(is_valid_coordinate(&id));
    }

    /// A relative local `origin` is read from the main worktree: a linked
    /// worktree finds the source its main checkout finds, and both resolve the
    /// source's hosted coordinate — never the bare name a missing path leaves.
    #[test]
    fn a_relative_local_origin_resolves_one_coordinate_from_a_linked_worktree() {
        let c = checkouts("app-main");
        let ceiling = c._tmp.path();
        let upstream = c._tmp.path().join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        fixture_git(ceiling, &upstream, &["init", "-q", "-b", "main"]);
        fixture_git(ceiling, &upstream, &["remote", "add", "origin", "https://example.com/acme/shop.git"]);
        fixture_git(ceiling, &c.main, &["remote", "add", "origin", "../upstream"]);
        assert!(
            !c.worktree.join("../upstream").exists(),
            "the path is relative to the main checkout, not to the worktree"
        );
        let config = Config::default();
        for root in [&c.main, &c.worktree] {
            assert_eq!(
                resolve_project_id_confirmed(root, &config),
                ResolvedCoordinate { coordinate: "example.com/acme/shop".into(), confirmed: true },
                "{}",
                root.display()
            );
        }
    }

    /// Only a value this machine confirmed is confirmed: a configured id, a
    /// hosted remote, an existing local source, or the main worktree's name.
    /// The name a gone local source leaves, and a root git cannot answer for,
    /// are fallbacks — the open-time migration never moves a store to one.
    #[test]
    fn a_derivation_is_confirmed_only_by_a_repository_this_machine_reads() {
        let c = checkouts("app-main");
        let ceiling = c._tmp.path();
        let resolve = |root: &Path| resolve_project_id_confirmed(root, &Config::default());
        let confirmed = |coordinate: &str| ResolvedCoordinate { coordinate: coordinate.into(), confirmed: true };
        let fallback = |coordinate: &str| ResolvedCoordinate { coordinate: coordinate.into(), confirmed: false };

        assert_eq!(resolve(&c.main), confirmed("app-main"), "no remote: the main worktree's name");
        assert_eq!(resolve(&c.clone), confirmed("app-main"), "an existing local source is followed");

        let away = c._tmp.path().join("app-main-away");
        std::fs::rename(&c.main, &away).unwrap();
        assert_eq!(resolve(&c.clone), fallback("app-main"), "a gone local source leaves only its name");
        std::fs::rename(&away, &c.main).unwrap();
        assert_eq!(resolve(&c.clone), confirmed("app-main"), "and is confirmed again once it is back");

        fixture_git(ceiling, &c.clone, &["remote", "set-url", "origin", "git@h:o/r.git"]);
        assert_eq!(resolve(&c.clone), confirmed("h/o/r"), "a hosted remote");

        let plain = c._tmp.path().join("plain-project");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(resolve(&plain), fallback("plain-project"), "outside git: the directory's own name");
        let pinned = Config {
            project: Project { id: Some("example.org/team/app".into()) },
            ..Config::default()
        };
        assert_eq!(
            resolve_project_id_confirmed(&plain, &pinned),
            confirmed("example.org/team/app"),
            "a configured id is confirmed wherever it is read"
        );
    }

    /// A bare repository's worktree has no main worktree; it names the bare
    /// repository, without its `.git` suffix.
    #[test]
    fn a_bare_repository_worktree_resolves_the_bare_repository_name() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ceiling = tmp.path();
        fixture_git(ceiling, tmp.path(), &["init", "-q", "--bare", "-b", "main", "store.git"]);
        let seed = tmp.path().join("seed");
        fixture_git(
            ceiling,
            tmp.path(),
            &["clone", "-q", tmp.path().join("store.git").to_str().unwrap(), "seed"],
        );
        std::fs::write(seed.join("f.txt"), "x\n").unwrap();
        fixture_git(ceiling, &seed, &["add", "."]);
        fixture_git(ceiling, &seed, &["commit", "-q", "-m", "seed"]);
        fixture_git(ceiling, &seed, &["push", "-q", "origin", "main"]);
        let wt = tmp.path().join("wt");
        fixture_git(
            ceiling,
            &tmp.path().join("store.git"),
            &["worktree", "add", "-q", wt.to_str().unwrap(), "main"],
        );
        assert_eq!(derive_project_id(&wt), "store");
    }

    #[test]
    fn outside_git_the_root_directory_name_is_the_coordinate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("plain-project");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(derive_project_id(&root), "plain-project");
        // A disallowed character in a directory name never yields an invalid
        // coordinate: it is replaced, so the derivation always validates.
        let spaced = tmp.path().join("My Project");
        std::fs::create_dir_all(&spaced).unwrap();
        let derived = derive_project_id(&spaced);
        assert_eq!(derived, "My-Project");
        assert!(is_valid_coordinate(&derived));
    }

    /// The CLI hands the engine a root spelled relative to the working directory
    /// (`.`), whose path has no final name of its own: the directory it denotes
    /// is named, never the last-resort fallback.
    #[test]
    fn a_root_spelled_without_a_final_name_resolves_the_directory_it_denotes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("plain-project");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let unnamed = root.join("sub").join("..");
        assert_eq!(unnamed.file_name(), None, "the spelling itself names nothing");
        assert_eq!(derive_project_id(&unnamed), "plain-project");
    }
}
