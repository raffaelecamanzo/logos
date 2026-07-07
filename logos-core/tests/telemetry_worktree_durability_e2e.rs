//! End-to-end regression that locks the worktree-teardown telemetry
//! durability guarantee (S-232, [NFR-OO-07], [FR-OB-07], [FR-OB-08],
//! [UAT-OB-03], [observability]).
//!
//! The `observability/tests.rs` unit suite proves the store-resolution matrix
//! and the `origin` stamp in isolation (that `telemetry_logos_dir` and
//! `telemetry_origin` return the primary's `.logos/` and the worktree's branch
//! from a linked worktree). This test composes them into the single narrative
//! [UAT-OB-03] is chartered to confirm, driven through **real** `git worktree`
//! I/O and the exact public surface the CLI/MCP adapters wire — `observability::init`
//! → instrumented `Engine` calls → `telemetry.db` — with **no** sprint-coordinator,
//! hook, or skill anywhere in the tested path ([NFR-OO-07]: the durability is a
//! property of Logos itself):
//!
//! 1. a traced navigation call issued from a linked worktree writes through to
//!    the **primary** repo's `.logos/telemetry.db`, stamped with the worktree's
//!    branch as `origin` ([FR-OB-07], [FR-OB-08]);
//! 2. **no** `telemetry.db` is ever created inside the worktree — the worktree
//!    must not carry its own doomed store ([FR-OB-07]);
//! 3. after `git worktree remove` deletes the worktree, the events it recorded
//!    are still present and queryable in the primary store — the signal
//!    outlived the worktree ([NFR-OO-07], [UAT-OB-03]).
//!
//! This is the regression that keeps [NFR-OO-07] from silently regressing: were
//! the S-230 primary-root resolution reverted, step 1 would write to the
//! worktree's own `.logos/telemetry.db` — failing assertion (2) immediately and
//! leaving the primary empty, so (1) and (3) would fail too.
//!
//! Everything lives in **one** test function on purpose: `observability::init`
//! installs the *global* `tracing` subscriber (and computes the per-process
//! `origin` once), so a second test in this binary would record its own engine
//! calls into the same store under the wrong origin and perturb the counts.
//!
//! Gated on `lang-rust`: the seed/index fixture needs the Rust grammar, exactly
//! like the sibling `tests/observability.rs` and `tests/worktree_lifecycle_e2e.rs`.
//!
//! [observability]: ../../docs/specs/architecture/components/observability.md
//! [NFR-OO-07]: ../../docs/specs/requirements/NFR-OO-07.md
//! [FR-OB-07]: ../../docs/specs/requirements/FR-OB-07.md
//! [FR-OB-08]: ../../docs/specs/requirements/FR-OB-08.md
//! [UAT-OB-03]: ../../docs/specs/requirements/UAT-OB-03.md
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use logos_core::observability::{self, Surface};
use logos_core::Engine;

/// The branch the linked worktree is built on — the increment its telemetry is
/// attributed to ([UAT-OB-03] test data: `feature/x`). A slash-bearing name
/// also guards that `origin` is stored verbatim, not mangled.
const BRANCH: &str = "feature/x";

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

/// A committed primary repo at `<tmp>/main` mirroring the canonical layout: a
/// tracked source file, a tracked `.logos/` policy (so the primary's `.logos/`
/// exists — the precondition telemetry resolution keys off), and the
/// gitignored-DB posture (`*.db*` never travels through git). Returns
/// `(tmp, primary_root)`.
fn repo_fixture() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("temp root");
    let main = tmp.path().join("main");
    fs::create_dir_all(main.join("src")).expect("create src/");
    fs::write(main.join("src/lib.rs"), "pub fn seeded_fn() {}\n").expect("write source");
    fs::write(main.join(".gitignore"), ".logos/*.db\n.logos/*.db-*\n").expect("write .gitignore");
    // A tracked file under .logos/ makes the primary's `.logos/` exist from the
    // first checkout — including in the worktree — while the DBs stay ignored.
    fs::create_dir_all(main.join(".logos")).expect("create .logos/");
    fs::write(main.join(".logos/config.toml"), "").expect("write .logos policy");
    sh_git(&main, &["init", "-q", "-b", "main"]);
    sh_git(&main, &["add", "."]);
    sh_git(&main, &["commit", "-q", "-m", "initial"]);
    (tmp, main)
}

/// Add a linked worktree at `<tmp>/wt` on branch [`BRANCH`]; returns its root.
fn add_worktree(tmp: &TempDir, main: &Path) -> PathBuf {
    let wt = tmp.path().join("wt");
    sh_git(
        main,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", BRANCH],
    );
    wt
}

/// Count events in the telemetry store at `path` whose `origin` equals
/// `origin`, opening the store **read-only** (the durability query the primary
/// serves — [UAT-OB-03] step 3/5).
fn count_events_with_origin(path: &Path, origin: &str) -> i64 {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("telemetry store opens read-only");
    conn.query_row(
        "SELECT count(*) FROM events WHERE origin = ?1",
        [origin],
        |r| r.get(0),
    )
    .expect("events table readable")
}

/// [UAT-OB-03] end to end: record telemetry from a linked worktree, confirm it
/// wrote through to the primary store with the branch `origin` and left no
/// store in the worktree, then `git worktree remove` and confirm the events
/// are still present in the primary — no orchestrator involved anywhere.
#[test]
fn worktree_telemetry_writes_through_to_primary_and_survives_teardown() {
    let (tmp, main) = repo_fixture();
    // Canonicalize the primary root so the paths we read match the
    // symlink-resolved store path `primary_root` hands the writer (macOS
    // `/var` → `/private/var`); otherwise a `.is_file()` on the un-resolved
    // path could miss the very file the write path created.
    let main = main.canonicalize().expect("canonicalize primary root");

    // Index the primary FIRST, in a scope with no telemetry subscriber
    // installed, so (a) the worktree bootstraps via seed + diff-reconcile and
    // (b) these primary-side calls are NOT recorded — the store must contain
    // only the worktree's events, cleanly attributed to its branch.
    {
        let engine = Engine::start(&main).expect("primary engine starts");
        let indexed = engine.index();
        assert!(indexed.files_indexed >= 1, "the primary index ran");
    }
    let primary_store = main.join(".logos").join("telemetry.db");
    assert!(
        !primary_store.exists(),
        "indexing the primary without a subscriber records no telemetry"
    );

    let wt = add_worktree(&tmp, &main);
    let worktree_store = wt.join(".logos").join("telemetry.db");

    // ── (1) the adapter wiring, exactly as the CLI process does it ───────────
    // `init(&wt)` resolves the primary once and, being in a linked worktree,
    // (a) points the telemetry writer at the PRIMARY's `.logos/telemetry.db`
    // and (b) stamps every event this process emits with the worktree's branch.
    let guard = observability::init(Surface::Cli, &wt);
    {
        let engine = Engine::start(&wt).expect("worktree engine starts (seeded from primary)");
        // A couple of navigation calls from inside the worktree — each funnels
        // through the single traced emission point and records one event.
        let found = engine.search("seeded_fn", None, None);
        assert!(
            found.warnings.is_empty(),
            "the seeded worktree serves navigation: {:?}",
            found.warnings
        );
        let _ = engine.search("nonexistent_symbol", None, None);
        // Drop the worktree engine (end of scope) so its own `logos.db` is
        // released — leaving the worktree clean (only the gitignored `.logos/*.db*`
        // it wrote) for the plain `git worktree remove` below.
    }
    // Flush the last telemetry batch exactly as a process exit would.
    drop(guard);

    // ── (1) write-through to the PRIMARY store, with the branch `origin` ─────
    assert!(
        primary_store.is_file(),
        "the worktree's telemetry wrote through to the primary store (FR-OB-07)"
    );
    let events_before = count_events_with_origin(&primary_store, BRANCH);
    assert!(
        events_before > 0,
        "the worktree's navigation calls landed in the primary, stamped origin={BRANCH} \
         (FR-OB-07, FR-OB-08)"
    );

    // ── (2) the worktree carries NO store of its own ────────────────────────
    assert!(
        !worktree_store.exists(),
        "no telemetry.db is created inside the worktree — it would die with it (FR-OB-07)"
    );

    // ── (3) survive `git worktree remove` ───────────────────────────────────
    // A plain (non-`--force`) remove doubles as an assertion that the worktree
    // is clean: the engine left behind only the gitignored `.logos/*.db*`, never
    // a stray non-ignored artifact in another checkout ([ADR-50]). `--force`
    // would silently mask exactly that regression, so it is deliberately omitted.
    sh_git(&main, &["worktree", "remove", wt.to_str().unwrap()]);
    assert!(!wt.exists(), "the worktree directory was removed");
    assert!(
        primary_store.is_file(),
        "the primary store is untouched by the worktree's teardown (NFR-OO-07)"
    );
    let events_after = count_events_with_origin(&primary_store, BRANCH);
    assert_eq!(
        events_after, events_before,
        "every worktree-origin event is still present after teardown — nothing was lost \
         (NFR-OO-07, UAT-OB-03)"
    );

    // ── and still queryable through the read path on the primary ────────────
    // `Engine::open(&main).stats(..)` resolves the same primary `.logos/` and
    // counts the survived events — the `logos stats` path of [UAT-OB-03] step 5.
    // (The guard is already dropped, so this call's own traced event is a
    // best-effort no-op against the shut-down writer.)
    let stats = Engine::open(&main).stats(None);
    assert_eq!(
        stats.calls_total, events_before as u64,
        "the primary read path counts EXACTLY the survived worktree events after teardown — \
         the store holds only branch-origin events (nothing was recorded before init): {stats:?}"
    );
}
