//! Integration tests for the history store + miner over **real** git fixtures
//! ([S-046] acceptance criteria, [FR-GH-01], [FR-GH-02], [ADR-22]).
//!
//! The pure parser/cutoff logic is unit-tested inside [`super::miner`]; the
//! store round-trip inside [`super::db`]. These exercise the end-to-end seam
//! against an actual `git` subprocess: incrementality, `.mailmap` coalescing,
//! re-index survival, the independent migration track, and the load-bearing
//! "gate never mines" boundary.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use crate::config::EffectiveHistory;

/// Run a git command in `cwd` with a fixed identity (no reliance on the host
/// gitconfig), panicking on failure — fixtures only.
fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Commit `relpath` with `contents`, authored by a specific identity (so the
/// mailmap fixture can coalesce two emails).
fn commit_as(cwd: &Path, relpath: &str, contents: &str, name: &str, email: &str, msg: &str) {
    std::fs::write(cwd.join(relpath), contents).unwrap();
    sh_git(cwd, &["add", relpath]);
    sh_git(
        cwd,
        &[
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-q",
            "-m",
            msg,
        ],
    );
}

/// A git repo at `<tmp>/repo` with two commits over two files.
fn repo_fixture() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    commit_as(
        &repo,
        "src/lib.rs",
        "pub fn one() {}\n",
        "Dev",
        "dev@logos",
        "first",
    );
    commit_as(
        &repo,
        "src/util.rs",
        "pub fn two() {}\n",
        "Dev",
        "dev@logos",
        "second",
    );
    (tmp, repo)
}

/// The default effective config (12-month window, etc.) — the fixtures live in
/// "now", so the window always covers them.
fn cfg() -> EffectiveHistory {
    EffectiveHistory {
        window_months: EffectiveHistory::DEFAULT_WINDOW_MONTHS,
        co_change_max_commit_files: EffectiveHistory::DEFAULT_CO_CHANGE_MAX_COMMIT_FILES,
        defect_patterns: EffectiveHistory::default_defect_patterns(),
    }
}

/// First mine reads every in-window commit and flags `first_mine`; a second mine
/// at the unchanged HEAD reads **zero** ([FR-GH-02] acceptance).
#[test]
fn first_mine_reads_all_then_second_reads_zero() {
    let (_tmp, repo) = repo_fixture();

    let first = super::mine(&repo, &cfg()).expect("first mine");
    assert!(first.degraded.is_none(), "a normal repo does not degrade");
    assert!(first.first_mine, "the first mine is flagged");
    assert_eq!(first.commits_read, 2, "both in-window commits are mined");
    assert!(first.head_sha.is_some());

    let second = super::mine(&repo, &cfg()).expect("second mine");
    assert!(!second.first_mine, "the second mine is not a first mine");
    assert_eq!(
        second.commits_read, 0,
        "an unchanged HEAD reads zero commits (FR-GH-02)"
    );
    assert_eq!(
        second.head_sha, first.head_sha,
        "the cursor still tracks HEAD"
    );

    // The facts are persisted: two commits, the two file changes.
    let conn = super::open(&repo).unwrap();
    let commits: i64 = conn
        .query_row("SELECT count(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    let changes: i64 = conn
        .query_row("SELECT count(*) FROM file_changes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commits, 2);
    assert_eq!(changes, 2, "src/lib.rs + src/util.rs");
}

/// A new commit after the first mine is picked up incrementally; the prior
/// commits are not re-read into duplicate rows ([FR-GH-02] incremental).
#[test]
fn incremental_mine_reads_only_new_commits() {
    let (_tmp, repo) = repo_fixture();
    let first = super::mine(&repo, &cfg()).expect("first mine");
    assert_eq!(first.commits_read, 2);

    commit_as(
        &repo,
        "src/lib.rs",
        "pub fn one_v2() {}\n",
        "Dev",
        "dev@logos",
        "third",
    );
    let inc = super::mine(&repo, &cfg()).expect("incremental mine");
    assert_eq!(inc.commits_read, 1, "only the new commit is read");
    assert!(!inc.first_mine);

    let conn = super::open(&repo).unwrap();
    let commits: i64 = conn
        .query_row("SELECT count(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commits, 3, "no duplicate rows — three distinct commits");
}

/// When the prior `mined_through` is no longer an ancestor of HEAD (a rebase /
/// force-push / orphan history), the incremental range is abandoned for a full
/// windowed re-mine — and the idempotent upserts keep that safe (no duplicate
/// rows). Exercises the `is_ancestor`-false branch of `mine_incremental`.
#[test]
fn non_ancestor_head_triggers_idempotent_full_remine() {
    let (_tmp, repo) = repo_fixture();
    assert_eq!(
        super::mine(&repo, &cfg()).expect("first mine").commits_read,
        2
    );

    // Orphan branch: a brand-new root commit whose ancestry shares nothing with
    // the previously-mined HEAD, so `merge-base --is-ancestor` returns false.
    sh_git(&repo, &["checkout", "-q", "--orphan", "rebased"]);
    commit_as(
        &repo,
        "src/fresh.rs",
        "pub fn fresh() {}\n",
        "Dev",
        "dev@logos",
        "orphan root",
    );

    let fallback = super::mine(&repo, &cfg()).expect("fallback mine");
    assert!(
        fallback.degraded.is_none(),
        "a rewritten history is not a degraded state"
    );
    assert!(
        fallback.commits_read >= 1,
        "the non-ancestor HEAD forces a full re-mine, not a zero read"
    );

    // The orphan commit is recorded; the re-mine did not duplicate any rows.
    let conn = super::open(&repo).unwrap();
    let fresh_rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM file_changes WHERE path = 'src/fresh.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        fresh_rows, 1,
        "the orphan commit's file change is recorded once"
    );
}

/// Degraded mode: a shallow clone reports `Shallow` and writes no numbers
/// ([FR-GH-08], [NFR-RA-05] never-fabricate) — git history is truncated, so any
/// window figure would be a fabrication.
#[test]
fn shallow_clone_degrades_without_fabricating() {
    let (_tmp, source) = repo_fixture();
    // One more commit so a depth-1 clone is genuinely shallow (truncated).
    commit_as(
        &source,
        "src/more.rs",
        "pub fn more() {}\n",
        "Dev",
        "dev@logos",
        "third",
    );

    let tmp2 = TempDir::new().unwrap();
    let shallow = tmp2.path().join("shallow");
    // A `file://` URL is required for `--depth` to take effect — git ignores it
    // for plain local-path clones (which hardlink the full object store).
    let out = Command::new("git")
        .args([
            "clone",
            "--depth=1",
            &format!("file://{}", source.display()),
            shallow.to_str().unwrap(),
        ])
        .output()
        .expect("git clone");
    assert!(
        out.status.success(),
        "shallow clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let outcome = super::mine(&shallow, &cfg()).expect("mine on a shallow clone");
    assert_eq!(
        outcome.degraded,
        Some(super::DegradedReason::Shallow),
        "a shallow clone degrades to Shallow"
    );
    assert_eq!(outcome.commits_read, 0, "nothing is mined");
    // The store was never populated — never-fabricate.
    let conn = super::open(&shallow).unwrap();
    let commits: i64 = conn
        .query_row("SELECT count(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commits, 0, "no fabricated facts from a shallow clone");
}

/// `.mailmap` coalesces two author emails to one canonical identity ([FR-GH-02]
/// note): `%aE` is the mailmapped email, so both commits land under the
/// canonical address.
#[test]
fn mailmap_coalesces_author_identity() {
    let (_tmp, repo) = repo_fixture();
    // Map the alias email to the canonical one.
    std::fs::write(
        repo.join(".mailmap"),
        "Ada Lovelace <ada@canonical.example> <ada@alias.example>\n",
    )
    .unwrap();
    // One commit authored under the alias, one under the canonical email.
    commit_as(
        &repo,
        "src/a.rs",
        "pub fn a() {}\n",
        "Ada",
        "ada@alias.example",
        "alias commit",
    );
    commit_as(
        &repo,
        "src/b.rs",
        "pub fn b() {}\n",
        "Ada Lovelace",
        "ada@canonical.example",
        "canonical commit",
    );

    super::mine(&repo, &cfg()).expect("mine");

    let conn = super::open(&repo).unwrap();
    // The two Ada commits coalesce to a single canonical email.
    let ada_aliases: i64 = conn
        .query_row(
            "SELECT count(*) FROM commits WHERE author_email = 'ada@alias.example'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let ada_canonical: i64 = conn
        .query_row(
            "SELECT count(*) FROM commits WHERE author_email = 'ada@canonical.example'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ada_aliases, 0, "the alias email is mailmapped away");
    assert_eq!(ada_canonical, 2, "both Ada commits coalesce to canonical");
}

/// A shorter window excludes older commits, so two evaluations differ only by
/// the window — and wall-clock never enters (the fixtures are HEAD-anchored).
/// Here both fixture commits are seconds apart, so a 12-month window keeps both;
/// the assertion pins the HEAD-anchored bound's *shape* rather than a calendar
/// edge (the calendar math itself is unit-tested in `miner`).
#[test]
fn window_is_head_anchored_not_wall_clock() {
    let (_tmp, repo) = repo_fixture();
    let a = super::mine(&repo, &cfg()).expect("mine A");
    // A second mine at the same HEAD with the same config is a byte-for-byte
    // no-op regardless of how much wall-clock time passed between the two.
    let b = super::mine(&repo, &cfg()).expect("mine B");
    assert_eq!(a.head_sha, b.head_sha);
    assert_eq!(b.commits_read, 0, "same HEAD + config → nothing re-read");
}

/// The store reports its **own** migration version, independent of `logos.db`'s
/// track ([FR-GH-01]).
#[test]
fn migration_track_is_independent() {
    let (_tmp, repo) = repo_fixture();
    super::mine(&repo, &cfg()).expect("mine");
    assert_eq!(
        super::schema_version(&repo).unwrap(),
        super::latest_schema_version(),
        "history.db is fully migrated on its own track"
    );
    assert!(
        super::latest_schema_version() >= 1,
        "the history ledger starts at version 1 (forward-only)"
    );
}

/// Degraded mode: outside a git repo the miner reports `NotGit` and writes no
/// numbers ([FR-GH-08], [NFR-RA-05] never-fabricate).
#[test]
fn non_git_directory_degrades_without_fabricating() {
    let tmp = TempDir::new().unwrap();
    let plain = tmp.path().join("not-a-repo");
    std::fs::create_dir_all(&plain).unwrap();

    let outcome = super::mine(&plain, &cfg()).expect("mine returns, never errors");
    assert_eq!(outcome.degraded, Some(super::DegradedReason::NotGit));
    assert_eq!(outcome.commits_read, 0);
    assert!(outcome.head_sha.is_none());
}

// ── Engine-level boundaries (the load-bearing invariants) ───────────────────

/// A full `index` leaves `history.db` (mined facts + migration version) intact
/// ([FR-GH-01] acceptance): the store is a separate file the graph rebuild never
/// touches. Driven through the [`Engine`](crate::Engine) seam end to end.
#[cfg(feature = "lang-rust")]
#[test]
fn history_db_survives_a_full_index() {
    let (_tmp, repo) = repo_fixture();
    let engine = crate::Engine::start(&repo).expect("engine starts");

    // Mine via the lazy seam, then capture the state.
    let mined = engine.ensure_history_mined().expect("mine");
    assert_eq!(mined.commits_read, 2);
    let before: i64 = {
        let conn = super::open(&repo).unwrap();
        conn.query_row("SELECT count(*) FROM commits", [], |r| r.get(0))
            .unwrap()
    };
    let version_before = super::schema_version(&repo).unwrap();

    // A full index rebuilds logos.db wholesale.
    let _ = engine.index();

    // history.db — a separate file — is untouched: same facts, same version.
    let after: i64 = {
        let conn = super::open(&repo).unwrap();
        conn.query_row("SELECT count(*) FROM commits", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(after, before, "mined facts survive a full index (FR-GH-01)");
    assert_eq!(
        super::schema_version(&repo).unwrap(),
        version_before,
        "the history migration version is unaffected by a logos.db rebuild"
    );

    // And mining stays incremental afterwards — the cursor survived too.
    let again = engine.ensure_history_mined().expect("re-mine");
    assert_eq!(
        again.commits_read, 0,
        "post-index mine at the same HEAD reads zero (FR-GH-01: 'mines only new commits')"
    );
}

/// The load-bearing boundary ([BR-26], [FR-GH-02]): `gate` runs **no** history
/// mining — `history.db` does not even exist after a gate, and only appears once
/// a temporal read (the lazy seam) is invoked.
#[cfg(feature = "lang-rust")]
#[test]
fn gate_never_mines_history() {
    let (_tmp, repo) = repo_fixture();
    let engine = crate::Engine::start(&repo).expect("engine starts");

    // A gate (reconcile-then-score) must not create or touch history.db.
    engine.gate(None, false, true).expect("gate runs");
    assert!(
        !super::db_path(&repo).exists(),
        "gate must not mine — history.db is absent (BR-26, FR-GH-02)"
    );

    // The lazy temporal seam is the ONLY path that mines.
    engine.ensure_history_mined().expect("mine");
    assert!(
        super::db_path(&repo).exists(),
        "the temporal read seam creates history.db"
    );
}

// ── Temporal metrics, co-change, defect heuristic, snapshots ([S-047]) ──────

/// Commit already-staged changes with a **fixed** author+committer date, so the
/// HEAD-anchored window and every age are pinned regardless of when/where the
/// test runs — the determinism the golden relies on ([BR-27], [UAT-GH-01]).
fn commit_staged_dated(cwd: &Path, name: &str, email: &str, msg: &str, date: &str) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
        ])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .args(["commit", "-q", "-m", msg])
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "dated commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Stage a write of `relpath` = `contents`.
fn write_add(cwd: &Path, relpath: &str, contents: &str) {
    let path = cwd.join(relpath);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, contents).unwrap();
    sh_git(cwd, &["add", relpath]);
}

/// The scripted-history golden fixture: a fixed four-commit sequence with pinned
/// dates and identities — two authors, a fix-commit, a co-change, and a rename.
fn scripted_history() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);

    // c1 — Alice adds a.rs.
    write_add(&repo, "a.rs", "a1\n");
    commit_staged_dated(
        &repo,
        "Alice",
        "alice@x",
        "initial A",
        "2025-01-01T12:00:00Z",
    );
    // c2 — Bob fixes a.rs (defect-pattern match).
    write_add(&repo, "a.rs", "a1\na2\n");
    commit_staged_dated(
        &repo,
        "Bob",
        "bob@x",
        "fix: bug in A",
        "2025-01-02T12:00:00Z",
    );
    // c3 — Alice tweaks a.rs and adds b.rs (a co-change pair).
    write_add(&repo, "a.rs", "a1\na2\na3\n");
    write_add(&repo, "b.rs", "b1\n");
    commit_staged_dated(
        &repo,
        "Alice",
        "alice@x",
        "add B and tweak A",
        "2025-01-03T12:00:00Z",
    );
    // c4 (HEAD) — Alice renames b.rs → c.rs (content unchanged → 100% rename).
    sh_git(&repo, &["mv", "b.rs", "c.rs"]);
    commit_staged_dated(
        &repo,
        "Alice",
        "alice@x",
        "rename B to C",
        "2025-01-04T12:00:00Z",
    );

    (tmp, repo)
}

fn file<'a>(report: &'a super::TemporalReport, path: &str) -> &'a super::FileTemporal {
    report
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("file {path} present in the temporal report"))
}

/// [UAT-GH-01] — the scripted-history golden: every per-file metric matches a
/// hand-computed expected value, and two evaluations at the same HEAD + config
/// serialise byte-for-byte identically (the across-runs / across-CI-targets
/// determinism, [NFR-RA-06]).
#[test]
fn scripted_history_golden_is_byte_identical() {
    let (_tmp, repo) = scripted_history();

    let r1 = super::temporal_report(&repo, &cfg()).expect("temporal report");
    assert!(r1.degraded.is_none());
    assert_eq!(r1.files.len(), 3, "a.rs, b.rs, c.rs");
    // The determinism "now" is pinned to HEAD's committer timestamp
    // (2025-01-04T12:00:00Z = 1735992000), never the wall clock ([BR-27]).
    assert_eq!(r1.head_committed_at, Some(1_735_992_000));

    // a.rs — 3 commits, two authors (Alice 2 / Bob 1), one fix-commit, co-changed
    // with b.rs once. Ages (HEAD − commit): [3, 2, 1] days.
    let a = file(&r1, "a.rs");
    assert_eq!(a.commit_count, 3);
    assert_eq!(a.lines_added, 3);
    assert_eq!(a.lines_deleted, 0);
    assert_eq!(a.last_change_age_days, 1);
    assert_eq!(a.age_dispersion_days, 1); // std-dev of [3,2,1] ≈ 0.82 → 1
    assert_eq!(a.ownership_dispersion_bp, 3333); // 1 − 2/3
    assert_eq!(a.change_entropy_bp, 9183); // H(2/3,1/3)/log2(2)
    assert_eq!(a.co_change_count, 1); // b.rs
    assert_eq!(a.defect_commits, 1); // "fix: bug in A"

    // b.rs — added in c3 (1 day before HEAD), single author, paired with a.rs.
    let b = file(&r1, "b.rs");
    assert_eq!(b.commit_count, 1);
    assert_eq!(b.lines_added, 1);
    assert_eq!(b.lines_deleted, 0);
    assert_eq!(b.last_change_age_days, 1);
    assert_eq!(
        b.age_dispersion_days, 0,
        "a single change has no dispersion"
    );
    assert_eq!(b.ownership_dispersion_bp, 0);
    assert_eq!(b.change_entropy_bp, 0);
    assert_eq!(b.co_change_count, 1); // a.rs
    assert_eq!(b.defect_commits, 0);

    // c.rs — the rename target (0/0), HEAD-touched, single author, no partner.
    let c = file(&r1, "c.rs");
    assert_eq!(c.commit_count, 1);
    assert_eq!(c.lines_added, 0);
    assert_eq!(c.lines_deleted, 0);
    assert_eq!(c.last_change_age_days, 0, "HEAD itself touched it");
    assert_eq!(c.age_dispersion_days, 0);
    assert_eq!(c.ownership_dispersion_bp, 0);
    assert_eq!(c.change_entropy_bp, 0);
    assert_eq!(c.co_change_count, 0);
    assert_eq!(c.defect_commits, 0);

    // The full serialised per-file payload is pinned as a golden literal — any
    // field drift or serialization change (not just a within-run nondeterminism)
    // is caught here and on every CI target ([UAT-GH-01], [NFR-RA-06]).
    let golden = r#"[{"path":"a.rs","commit_count":3,"lines_added":3,"lines_deleted":0,"last_change_age_days":1,"age_dispersion_days":1,"ownership_dispersion_bp":3333,"change_entropy_bp":9183,"co_change_count":1,"defect_commits":1},{"path":"b.rs","commit_count":1,"lines_added":1,"lines_deleted":0,"last_change_age_days":1,"age_dispersion_days":0,"ownership_dispersion_bp":0,"change_entropy_bp":0,"co_change_count":1,"defect_commits":0},{"path":"c.rs","commit_count":1,"lines_added":0,"lines_deleted":0,"last_change_age_days":0,"age_dispersion_days":0,"ownership_dispersion_bp":0,"change_entropy_bp":0,"co_change_count":0,"defect_commits":0}]"#;
    assert_eq!(
        serde_json::to_string(&r1.files).unwrap(),
        golden,
        "the per-file payload matches the committed golden literal byte-for-byte"
    );

    // And byte-identical across a second evaluation at the same HEAD + config.
    let r2 = super::temporal_report(&repo, &cfg()).expect("second temporal report");
    assert_eq!(
        serde_json::to_string(&r1.files).unwrap(),
        serde_json::to_string(&r2.files).unwrap(),
        "two evaluations at the same HEAD + config are byte-identical (NFR-RA-06)"
    );
}

/// [FR-GH-04] — a synthetic mega-commit (more files than the cap) inflates churn
/// but never pairs; lowering the cap below a commit's file count excludes THAT
/// commit from pairing on the next evaluation, and the config hash changes.
#[test]
fn mega_commit_isolated_from_co_change() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);

    // c1 — a 2-file commit pairs hot.rs ↔ near.rs.
    write_add(&repo, "hot.rs", "h1\n");
    write_add(&repo, "near.rs", "n1\n");
    commit_staged_dated(&repo, "Dev", "dev@x", "pair", "2025-02-01T12:00:00Z");
    // c2 — a mega-commit touches hot.rs + 60 others (> the default cap of 50).
    write_add(&repo, "hot.rs", "h1\nh2\n");
    for i in 0..60 {
        write_add(&repo, &format!("mega/f{i}.rs"), "x\n");
    }
    commit_staged_dated(
        &repo,
        "Dev",
        "dev@x",
        "mass refactor",
        "2025-02-02T12:00:00Z",
    );
    // c3 — a 3-file commit pairs hot.rs with x.rs and y.rs.
    write_add(&repo, "hot.rs", "h1\nh2\nh3\n");
    write_add(&repo, "x.rs", "x1\n");
    write_add(&repo, "y.rs", "y1\n");
    commit_staged_dated(&repo, "Dev", "dev@x", "trio", "2025-02-03T12:00:00Z");

    // Default cap 50: c1 (2) and c3 (3) pair; the 61-file c2 never does.
    let report = super::temporal_report(&repo, &cfg()).expect("report");
    let hot = file(&report, "hot.rs");
    assert_eq!(
        hot.commit_count, 3,
        "every commit (mega included) counts toward churn"
    );
    assert_eq!(
        hot.co_change_count, 3,
        "co-change sees near.rs (c1) + x.rs,y.rs (c3) — never the 61-file mega-commit"
    );
    // A file touched only by the mega-commit has no co-change partners at all.
    assert_eq!(
        file(&report, "mega/f0.rs").co_change_count,
        0,
        "a mega-commit-only file never pairs"
    );

    // Lowering the cap to 2 excludes c3 (3 files) from pairing on the NEXT
    // evaluation — hot now pairs only via c1 — while churn is unchanged. The
    // config hash also changes ([FR-GH-09]).
    let lowered = EffectiveHistory {
        co_change_max_commit_files: 2,
        ..cfg()
    };
    assert_ne!(
        cfg().hash(),
        lowered.hash(),
        "lowering the co-change cap changes the config hash"
    );
    let lo = super::temporal_report(&repo, &lowered).expect("lowered-cap report");
    let hot_lo = file(&lo, "hot.rs");
    assert_eq!(hot_lo.commit_count, 3, "churn is unaffected by the cap");
    assert_eq!(
        hot_lo.co_change_count, 1,
        "cap=2 excludes the 3-file c3; only near.rs (c1) still pairs"
    );
}

/// [FR-GH-05] — changing `defect_patterns` changes the per-file counts AND the
/// config hash; the heuristic flags exactly the matching commit (each commit
/// touches a distinct file, so the count cannot be coincidental) and never
/// fabricates for a non-matching message.
#[test]
fn defect_pattern_change_moves_counts_and_hash() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    // Each commit touches a DISTINCT file, so a per-file count of 1 unambiguously
    // identifies which commit the pattern matched.
    write_add(&repo, "fixed.rs", "1\n");
    commit_staged_dated(&repo, "Dev", "dev@x", "fix: a bug", "2025-03-01T12:00:00Z");
    write_add(&repo, "feature.rs", "1\n");
    commit_staged_dated(
        &repo,
        "Dev",
        "dev@x",
        "add a shiny feature",
        "2025-03-02T12:00:00Z",
    );

    // Default patterns match the "fix" commit → only fixed.rs is flagged.
    let default = super::temporal_report(&repo, &cfg()).expect("default");
    assert_eq!(file(&default, "fixed.rs").defect_commits, 1);
    assert_eq!(
        file(&default, "feature.rs").defect_commits,
        0,
        "the feature commit is not a fix under the default patterns"
    );

    // A "feature" pattern flips the match to the OTHER commit/file, and the hash
    // differs.
    let feature_cfg = EffectiveHistory {
        defect_patterns: vec!["(?i)\\bfeature\\b".to_string()],
        ..cfg()
    };
    assert_ne!(
        cfg().hash(),
        feature_cfg.hash(),
        "patterns hash into the config"
    );
    let feat = super::temporal_report(&repo, &feature_cfg).expect("feature patterns");
    assert_eq!(
        file(&feat, "feature.rs").defect_commits,
        1,
        "the 'feature' commit now matches"
    );
    assert_eq!(
        file(&feat, "fixed.rs").defect_commits,
        0,
        "the fix commit no longer matches under the feature pattern"
    );

    // A pattern matching neither message fabricates nothing.
    let none_cfg = EffectiveHistory {
        defect_patterns: vec!["(?i)\\bnevermatch\\b".to_string()],
        ..cfg()
    };
    let none = super::temporal_report(&repo, &none_cfg).expect("no-match patterns");
    assert_eq!(
        file(&none, "fixed.rs").defect_commits,
        0,
        "never fabricated"
    );
    assert_eq!(
        file(&none, "feature.rs").defect_commits,
        0,
        "never fabricated"
    );
}

/// [FR-GH-03] / [NFR-RA-05] — a file whose only commit predates the window is
/// reported `n/a` (absent), never a fabricated zero-risk score; and a
/// wall-clock-only advance (no new commit) changes nothing.
#[test]
fn out_of_window_file_is_na_and_window_is_head_anchored() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    // An OLD file, then a much later HEAD — with a 1-month window the old file
    // falls outside it.
    write_add(&repo, "old.rs", "old\n");
    commit_staged_dated(&repo, "Dev", "dev@x", "old work", "2024-01-01T12:00:00Z");
    write_add(&repo, "new.rs", "new\n");
    commit_staged_dated(&repo, "Dev", "dev@x", "recent work", "2025-06-01T12:00:00Z");

    let narrow = EffectiveHistory {
        window_months: 1,
        ..cfg()
    };
    let report = super::temporal_report(&repo, &narrow).expect("report");
    assert!(
        report.files.iter().any(|f| f.path == "new.rs"),
        "the in-window file is present"
    );
    assert!(
        !report.files.iter().any(|f| f.path == "old.rs"),
        "the out-of-window file is n/a (absent), never a fabricated zero"
    );

    // Re-evaluating at the same HEAD + config is a byte-for-byte no-op regardless
    // of how much wall-clock time elapsed between the calls.
    let again = super::temporal_report(&repo, &narrow).expect("re-eval");
    assert_eq!(
        serde_json::to_string(&report.files).unwrap(),
        serde_json::to_string(&again.files).unwrap(),
        "a wall-clock-only advance changes nothing (BR-27)"
    );
}

/// [FR-GH-09] — every temporal evaluation appends an immutable snapshot row
/// carrying the config hash + provenance; the series is append-only, and the
/// `logos.db` schema is untouched (the temporal tier lives only in `history.db`).
#[test]
fn temporal_snapshots_are_append_only_with_the_config_hash() {
    let (_tmp, repo) = scripted_history();

    let report = super::temporal_report(&repo, &cfg()).expect("first eval");
    super::temporal_report(&repo, &cfg()).expect("second eval");

    let conn = super::open(&repo).unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM temporal_snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2, "each evaluation appends one snapshot row");

    // The first snapshot pins every FR-GH-09 provenance field + the aggregates.
    let (
        config_hash,
        head_sha,
        mined_through,
        git_version,
        head_committed_at,
        total_added,
        max_churn,
        mean_ownership_bp,
    ): (String, String, String, String, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT config_hash, head_sha, mined_through, git_version, head_committed_at,
                    total_added, max_churn_commits, mean_ownership_dispersion_bp
               FROM temporal_snapshots ORDER BY id LIMIT 1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(
        config_hash,
        cfg().hash(),
        "the snapshot pins the config hash"
    );
    // Provenance fields match the report (mined-through SHA, HEAD SHA, git version).
    assert_eq!(Some(head_sha), report.head_sha, "HEAD SHA pinned");
    assert_eq!(
        Some(mined_through),
        report.mined_through,
        "mined-through SHA pinned"
    );
    assert_eq!(Some(git_version), report.git_version, "git version pinned");
    assert!(
        report.git_version.as_deref().is_some_and(|v| !v.is_empty()),
        "the git version is recorded, not empty"
    );
    assert_eq!(head_committed_at, 1_735_992_000, "the determinism clock");
    // Aggregates: a.rs(3)+b.rs(1)+c.rs(0) added = 4; max churn 3; mean ownership
    // dispersion (3333 + 0 + 0)/3 = 1111.
    assert_eq!(total_added, 4);
    assert_eq!(max_churn, 3);
    assert_eq!(mean_ownership_bp, 1111);
}

/// The finding-#11 fix end-to-end: a committed file whose path *literally*
/// contains ` => ` is attributed to that exact path, never split into a phantom
/// rename — proven through a real `git` subprocess with the `-z` miner.
#[test]
fn literal_arrow_filename_attributed_correctly() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh_git(&repo, &["init", "-q", "-b", "main"]);
    write_add(&repo, "weird => name.txt", "v1\n");
    commit_staged_dated(
        &repo,
        "Dev",
        "dev@x",
        "add the weird file",
        "2025-04-01T12:00:00Z",
    );
    write_add(&repo, "weird => name.txt", "v1\nv2\n");
    commit_staged_dated(
        &repo,
        "Dev",
        "dev@x",
        "edit the weird file",
        "2025-04-02T12:00:00Z",
    );

    let report = super::temporal_report(&repo, &cfg()).expect("report");
    let weird = file(&report, "weird => name.txt");
    assert_eq!(
        weird.commit_count, 2,
        "both edits attribute to the literal-arrow path, not a phantom rename"
    );
    assert_eq!(weird.lines_added, 2);
    // No phantom 'name.txt' file was fabricated by a bad rename split.
    assert!(
        !report.files.iter().any(|f| f.path == "name.txt"),
        "no phantom rename target leaks into the report"
    );
}
