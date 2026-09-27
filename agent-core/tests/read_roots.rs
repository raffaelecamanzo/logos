//! Declared extra read roots — `[chat] read_roots` (sprint-79 HF-1, [NFR-SE-04]
//! amended).
//!
//! A project that keeps its docs in a sibling repo reaches them through in-tree
//! symlinks (`docs/planning -> ../logos-docs/planning`). With no read roots
//! declared those symlinks are an [`SandboxError::Escape`], exactly as before;
//! once a directory is declared, a canonical path under it is admitted — but
//! only when reached **through** the project tree: an absolute path or a `..`
//! component is still refused before any filesystem access, and a symlink to any
//! undeclared directory is still a turn-fatal escape. The `grep`/`glob` walks
//! follow a symlink only when its canonical target lies under a declared root,
//! and report every hit under its in-tree path.
//!
//! [NFR-SE-04]: filesystem walk is contained.

#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{source_toolset, Sandbox, SandboxError};

/// The fixture estate, under one temp `base`:
/// ```text
/// project/                          ← the sandbox root
///   .gitignore                      "/docs/planning\n/docs/sprawl\n" (this repo's shape)
///   src/lib.rs                      "pub fn f() { needle(); }"
///   docs/notes.md                   "needle in a real doc"
///   docs/planning -> docs-repo/planning      (gitignored, into the read root)
///   log.md -> docs-repo/planning/sprint-log.md   (a FILE symlink into it)
///   stray -> elsewhere                        (an undeclared directory)
///   stray-file.md -> elsewhere/secret.md      (an undeclared file)
///   near -> docs-repo-private                 (one character from the read root)
///   docs/buried -> docs-repo/planning/target  (a link INTO an ignored dir there)
///   docs/target -> docs-repo/other            (a link NAMED like an ignored dir)
///   target/junk.md                  (an ignored dir, in-tree)
/// docs-repo/                        ← the declared read root
///   planning/sprint-log.md          "needle in the sprint log"
///   planning/target/junk.md         (an ignored dir INSIDE the read root)
///   planning/loop -> docs-repo/planning   (a cycle onto itself)
///   planning/out -> elsewhere             (a nested link out of the read root)
///   planning/sneaky.md -> planning/target/junk.md  (reaches the ignored dir by link)
///   other/x.md                      "needle behind an ignored-name link"
/// docs-repo-private/secret.md       "needle near-miss secret"
/// elsewhere/secret.md               "needle top secret"
/// ```
struct Estate {
    _base: tempfile::TempDir,
    base: PathBuf,
    project: PathBuf,
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write");
}

fn estate() -> Estate {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().canonicalize().expect("canonical base");
    let project = base.join("project");
    let docs_repo = base.join("docs-repo");
    let elsewhere = base.join("elsewhere");

    write(&project.join(".gitignore"), "/docs/planning\n/docs/sprawl\n");
    write(&project.join("src/lib.rs"), "pub fn f() { needle(); }\n");
    write(&project.join("docs/notes.md"), "needle in a real doc\n");
    write(&project.join("target/junk.md"), "needle buried in-tree\n");
    write(&docs_repo.join("planning/sprint-log.md"), "needle in the sprint log\n");
    write(&docs_repo.join("planning/target/junk.md"), "needle buried in the read root\n");
    write(&docs_repo.join("other/x.md"), "needle behind an ignored-name link\n");
    write(&base.join("docs-repo-private/secret.md"), "needle near-miss secret\n");
    write(&elsewhere.join("secret.md"), "needle top secret\n");

    symlink(docs_repo.join("planning"), project.join("docs/planning")).expect("docs link");
    symlink(docs_repo.join("planning/sprint-log.md"), project.join("log.md")).expect("file link");
    symlink(&elsewhere, project.join("stray")).expect("stray link");
    symlink(elsewhere.join("secret.md"), project.join("stray-file.md")).expect("stray file");
    symlink(base.join("docs-repo-private"), project.join("near")).expect("near link");
    symlink(docs_repo.join("planning"), docs_repo.join("planning/loop")).expect("cycle");
    symlink(&elsewhere, docs_repo.join("planning/out")).expect("nested out");
    symlink(docs_repo.join("planning/target"), project.join("docs/buried")).expect("buried");
    symlink(docs_repo.join("other"), project.join("docs/target")).expect("ignored name");
    symlink(
        docs_repo.join("planning/target/junk.md"),
        docs_repo.join("planning/sneaky.md"),
    )
    .expect("sneaky");

    Estate {
        _base: dir,
        base,
        project,
    }
}

impl Estate {
    /// The default sandbox — no read roots declared.
    fn contained(&self) -> Sandbox {
        Sandbox::new(&self.project, ["target".to_string()]).expect("sandbox")
    }

    /// The sandbox with `../docs-repo` declared, resolved against the project.
    fn with_docs(&self) -> Sandbox {
        self.contained()
            .with_read_roots(&self.project, ["../docs-repo"])
            .expect("the declared read root exists")
    }
}

async fn call(sandbox: Sandbox, tool: &str, args: serde_json::Value) -> serde_json::Value {
    let out = source_toolset(Arc::new(sandbox))
        .call(tool, args.to_string())
        .await
        .unwrap_or_else(|e| panic!("{tool} {args}: {e}"));
    serde_json::from_str(&out).expect("json")
}

fn paths_of(value: &serde_json::Value, key: &str, field: Option<&str>) -> Vec<String> {
    value[key]
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|v| match field {
            Some(f) => v[f].as_str(),
            None => v.as_str(),
        })
        .map(str::to_string)
        .collect()
}

// ── default containment is unchanged ────────────────────────────────────────

#[test]
fn without_read_roots_a_symlink_into_the_docs_repo_is_still_an_escape() {
    let estate = estate();
    let err = estate
        .contained()
        .resolve("docs/planning/sprint-log.md")
        .expect_err("no read roots declared ⇒ today's refusal");
    assert!(matches!(err, SandboxError::Escape(_)), "got {err:?}");
    assert!(err.is_containment_refusal());
    // Byte-identical to the pre-read-roots refusal the user already knows.
    assert_eq!(
        err.to_string(),
        r#"path "docs/planning/sprint-log.md" resolves outside the project root"#
    );
}

#[tokio::test]
async fn without_read_roots_the_walks_follow_no_symlink() {
    let estate = estate();
    let globbed = call(estate.contained(), "glob", serde_json::json!({ "pattern": "**/*.md" })).await;
    assert_eq!(
        paths_of(&globbed, "paths", None),
        vec!["docs/notes.md".to_string()],
        "only the real in-tree doc is listed: {globbed}"
    );
}

// ── admission through an in-tree symlink ────────────────────────────────────

#[tokio::test]
async fn a_symlink_into_a_declared_read_root_is_admitted_and_read() {
    let estate = estate();
    let sandbox = estate.with_docs();
    let resolved = sandbox
        .resolve("docs/planning/sprint-log.md")
        .expect("a symlink into a declared read root is admitted");
    assert_eq!(resolved, estate.base.join("docs-repo/planning/sprint-log.md"));

    let read = call(
        sandbox,
        "read",
        serde_json::json!({ "path": "docs/planning/sprint-log.md" }),
    )
    .await;
    assert_eq!(read["path"], "docs/planning/sprint-log.md", "reported in-tree: {read}");
    assert!(read["content"].as_str().unwrap().contains("sprint log"), "{read}");
}

#[test]
fn a_file_symlink_into_a_declared_read_root_is_admitted() {
    let estate = estate();
    let resolved = estate.with_docs().resolve("log.md").expect("a file link is admitted too");
    assert_eq!(resolved, estate.base.join("docs-repo/planning/sprint-log.md"));
}

// ── what stays refused with read roots declared ─────────────────────────────

#[test]
fn a_symlink_to_an_undeclared_directory_is_still_a_turn_fatal_escape() {
    let estate = estate();
    let sandbox = estate.with_docs();
    for path in ["stray/secret.md", "stray-file.md", "docs/planning/out/secret.md"] {
        let err = sandbox
            .resolve(path)
            .expect_err("a link to an undeclared directory is refused");
        assert!(matches!(err, SandboxError::Escape(_)), "{path}: got {err:?}");
        assert!(err.is_containment_refusal(), "{path}: an escape stays turn-fatal");
    }
}

/// The near miss: `docs-repo-private` shares every character of `docs-repo` as a
/// string prefix. Containment is by path COMPONENT, so it is not admitted.
#[test]
fn a_sibling_sharing_the_read_roots_name_prefix_is_not_admitted() {
    let estate = estate();
    let err = estate
        .with_docs()
        .resolve("near/secret.md")
        .expect_err("a string-prefix sibling is not under the read root");
    assert!(matches!(err, SandboxError::Escape(_)), "got {err:?}");
}

#[test]
fn absolute_and_parent_paths_are_refused_even_into_a_read_root() {
    let estate = estate();
    let sandbox = estate.with_docs();

    let err = sandbox
        .resolve("../docs-repo/planning/sprint-log.md")
        .expect_err("the agent never names the read root itself");
    assert!(matches!(err, SandboxError::Traversal(_)), "got {err:?}");

    let absolute = estate.base.join("docs-repo/planning/sprint-log.md");
    let err = sandbox
        .resolve(absolute.to_str().unwrap())
        .expect_err("an absolute path into the read root is still refused");
    assert!(matches!(err, SandboxError::AbsolutePath(_)), "got {err:?}");
}

#[test]
fn ignored_dirs_still_apply_inside_a_read_root() {
    let estate = estate();
    let sandbox = estate.with_docs();
    // Named lexically, and — the case only the canonical re-scan below the read
    // root catches — reached through links whose own names are clean.
    for path in [
        "docs/planning/target/junk.md",
        "docs/planning/sneaky.md",
        "docs/buried/junk.md",
    ] {
        let err = sandbox
            .resolve(path)
            .expect_err("an ignored segment is refused behind the symlink too");
        assert!(
            matches!(err, SandboxError::Ignored(_, ref d) if d == "target"),
            "{path}: got {err:?}"
        );
    }
}

// ── the walks ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn glob_discovers_files_behind_a_gitignored_symlink_into_a_read_root() {
    let estate = estate();
    let globbed = call(estate.with_docs(), "glob", serde_json::json!({ "pattern": "**/*.md" })).await;
    let paths = paths_of(&globbed, "paths", None);

    for expected in ["docs/notes.md", "docs/planning/sprint-log.md", "log.md"] {
        assert!(paths.contains(&expected.to_string()), "{expected} listed: {paths:?}");
    }
    for refused in ["stray", "near", "target", "out/", "secret", "sneaky", "buried"] {
        assert!(
            !paths.iter().any(|p| p.contains(refused)),
            "nothing via {refused:?} is listed: {paths:?}"
        );
    }
    // The self-referential `loop` link is followed at most once: its canonical
    // target is the directory already walked, so the walk terminates and no
    // `loop/loop/…` chain appears.
    assert!(
        !paths.iter().any(|p| p.contains("loop")),
        "a cycle back onto a walked directory is not re-walked: {paths:?}"
    );
}

#[tokio::test]
async fn grep_finds_a_match_under_a_read_root_and_reports_its_in_tree_path() {
    let estate = estate();
    let grepped = call(estate.with_docs(), "grep", serde_json::json!({ "pattern": "needle" })).await;
    let paths = paths_of(&grepped, "matches", Some("path"));

    assert!(paths.contains(&"docs/planning/sprint-log.md".to_string()), "{paths:?}");
    assert!(paths.contains(&"src/lib.rs".to_string()), "{paths:?}");
    assert!(
        !grepped.to_string().contains("secret"),
        "an undeclared or near-miss link is never followed: {grepped}"
    );
    assert!(
        !grepped.to_string().contains("buried"),
        "ignored_dirs are pruned in-tree and inside the read root: {grepped}"
    );
    assert!(
        !grepped.to_string().contains("ignored-name"),
        "a link named like an ignored dir is not followed: {grepped}"
    );
}

#[tokio::test]
async fn grep_scoped_to_a_symlinked_directory_walks_it_under_its_in_tree_path() {
    let estate = estate();
    let grepped = call(
        estate.with_docs(),
        "grep",
        serde_json::json!({ "pattern": "needle", "path": "docs/planning" }),
    )
    .await;
    assert_eq!(
        paths_of(&grepped, "matches", Some("path")),
        vec!["docs/planning/sprint-log.md".to_string()],
        "{grepped}"
    );
}

#[tokio::test]
async fn grep_scoped_to_an_undeclared_symlink_is_a_containment_refusal() {
    let estate = estate();
    let err = source_toolset(Arc::new(estate.with_docs()))
        .call(
            "grep",
            serde_json::json!({ "pattern": "needle", "path": "stray" }).to_string(),
        )
        .await
        .expect_err("scoping into an undeclared link is refused");
    assert!(err.to_string().contains("resolves outside"), "{err}");
}

// ── declaring a read root ───────────────────────────────────────────────────

#[test]
fn a_missing_read_root_is_reported_never_dropped() {
    let estate = estate();
    let err = estate
        .contained()
        .with_read_roots(&estate.project, ["../docs-repo", "../no-such-repo"])
        .expect_err("a declared root that does not exist fails the build");
    assert!(matches!(err, SandboxError::BadReadRoot { .. }), "got {err:?}");
    let message = err.to_string();
    assert!(message.contains("../no-such-repo"), "names the declared entry: {message}");
    assert!(
        !err.is_containment_refusal(),
        "a configuration fault, not a containment refusal"
    );
}

#[test]
fn a_read_root_that_is_a_file_is_reported() {
    let estate = estate();
    let err = estate
        .contained()
        .with_read_roots(&estate.project, ["../elsewhere/secret.md"])
        .expect_err("a read root must be a directory");
    assert!(err.to_string().contains("not a directory"), "{err}");
}

#[test]
fn read_roots_resolve_relative_to_the_declaring_root_or_as_given_when_absolute() {
    let estate = estate();
    let absolute_path = estate.base.join("docs-repo");
    let relative_to_base = estate
        .contained()
        .with_read_roots(&estate.base, ["docs-repo"])
        .expect("relative to the declaring root");
    let absolute = estate
        .contained()
        .with_read_roots(&estate.project, [absolute_path.to_str().unwrap()])
        .expect("an absolute entry is taken as given");
    assert_eq!(relative_to_base.read_roots(), absolute.read_roots());
    assert_eq!(absolute.read_roots(), [estate.base.join("docs-repo")]);
}
