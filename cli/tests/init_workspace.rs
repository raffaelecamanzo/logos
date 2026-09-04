//! End-to-end tests for `logos init --workspace` (S-246, FR-WS-02), exercised
//! through the real binary over a multi-repo fixture:
//!
//! - `--yes` returns promptly without blocking on indexing;
//! - the bounded warm supervisor (S-321, FR-WS-14) is hidden from help,
//!   warms a 2-member fixture end to end, holds no store lock afterwards,
//!   drains past a failing member, and re-runs only the newly approved delta;
//! - an existing member's `.logos/config.toml` is never overwritten;
//! - stdout stays machine-clean while the approval gate goes to stderr;
//! - `--exclude` drops a candidate member;
//! - a second run injects no duplicate workspace MCP entry (idempotent);
//! - the `[workspace.warm] concurrency` key (S-322, FR-WS-01) is accepted, is
//!   preserved across a re-run, and is rejected with an actionable exit-2
//!   message when out of range.
//!
//! …and, at the other end of the same feature, the **plain** `logos init`
//! nudge that points a user at all of the above (S-319, FR-IN-08): at a
//! parent-of-repos root a non-TTY run never prompts and never reads stdin,
//! stdout stays byte-for-byte as before, an ordinary repository root gets
//! nothing, and a re-run stays non-clobbering.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Instant;

use tempfile::TempDir;

fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("no signal termination")
}

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

/// A committed git repo at `dir` with one file.
fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("f.txt"), "x\n").unwrap();
    // A real source file, so a completed index admits something and
    // `graph_revision` can distinguish warmed from merely scaffolded.
    fs::write(dir.join("lib.rs"), "pub fn member_entry() -> u32 { 7 }\n").unwrap();
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// A parent folder with two sibling repos: `api` and `web`.
fn two_member_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    init_repo(&tmp.path().join("api"));
    init_repo(&tmp.path().join("web"));
    tmp
}

/// A member's `graph_revision` — `0` before its first index, advanced by each
/// completed one.
///
/// This, not the existence of `.logos/logos.db`, is what distinguishes "the
/// warm ran" from "enablement scaffolded the store": `enable` calls
/// `Engine::init_with` per member, which creates the db file itself, so a
/// file-existence assertion is satisfied before any supervisor starts and
/// cannot fail.
fn graph_revision(member_root: &Path) -> u64 {
    let out = logos(member_root, &["--json", "status"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("status emits JSON")["graph_revision"]
        .as_u64()
        .expect("status carries graph_revision")
}

/// `--yes` returns promptly and reports both members initialised.
///
/// Scope note: this is a smoke test, NOT a proof of the FR-WS-02 non-blocking
/// contract. On a fixture this small a fully blocking implementation would also
/// finish well inside the bound, so the wall clock cannot separate "returned
/// before indexing" from "indexing was fast". The non-blocking property is held
/// structurally (`spawn_supervisor` calls `Command::spawn` and never `wait`)
/// and the *decision* — one warm invocation, not a synchronous per-member walk
/// — is asserted deterministically in `workspace_init`'s unit tests. A stronger
/// assertion here would need either wall-clock ratios (flaky under the parallel
/// load this suite already runs) or a many-member fixture, which S-321's own
/// criteria forbid in the suite.
#[test]
fn yes_returns_promptly_without_blocking_on_indexing() {
    let tmp = two_member_fixture();
    let start = Instant::now();
    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    let elapsed = start.elapsed();

    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        elapsed.as_secs() < 10,
        "init --workspace must return promptly, not block on indexing N members: {elapsed:?}"
    );

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON on stdout");
    let members = report["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    for m in members {
        assert_eq!(m["status"], "ready", "{m:?}");
    }
}

/// An existing member's hand-edited `.logos/config.toml` is left byte-for-byte
/// untouched (FR-IN-01 non-clobber, inherited verbatim).
#[test]
fn existing_member_config_is_never_overwritten() {
    let tmp = two_member_fixture();
    let api_config_dir = tmp.path().join("api").join(".logos");
    fs::create_dir_all(&api_config_dir).unwrap();
    fs::write(api_config_dir.join("config.toml"), "# hand-edited by the user\n").unwrap();

    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    assert_eq!(
        fs::read_to_string(api_config_dir.join("config.toml")).unwrap(),
        "# hand-edited by the user\n",
        "a pre-existing member config must never be overwritten"
    );
}

/// stdout carries only the machine-readable report; the approval gate (when
/// it would prompt) writes to stderr, never stdout (FR-CL-02). Without a TTY
/// on stdin, `ask()` degrades to its default without printing at all — so
/// even the non-`--yes` path here produces no stdout noise beyond the report.
#[test]
fn stdout_stays_machine_clean() {
    let tmp = two_member_fixture();
    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0);

    // Every line of stdout must be the single JSON report — no prompt text.
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("stdout is exactly one JSON document, nothing else");
}

/// `--exclude` drops a matching candidate from the proposed member set.
#[test]
fn exclude_drops_a_matching_member() {
    let tmp = two_member_fixture();
    let out = logos(
        tmp.path(),
        &["--json", "init", "--workspace", "--yes", "--exclude", "web"],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = report["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["api"], "web is excluded, api remains");

    let manifest = fs::read_to_string(tmp.path().join("logos.workspace.toml")).unwrap();
    assert!(manifest.contains("api"));
    assert!(!manifest.contains("web"));
}

/// A second run injects no duplicate workspace MCP entry — exactly one
/// `logos-workspace` key, idempotently.
#[test]
fn second_run_does_not_duplicate_the_workspace_mcp_entry() {
    let tmp = two_member_fixture();
    let first = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&first), 0);

    let second = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&second), 0, "{}", String::from_utf8_lossy(&second.stderr));

    let mcp: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(tmp.path().join(".mcp.json")).unwrap()).unwrap();
    let servers = mcp["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1, "still exactly one server entry: {servers:?}");
    assert!(servers.contains_key("logos-workspace"));

    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["mcp"]["action"], "unchanged");
    assert_eq!(report["manifest"]["action"], "unchanged");
}

/// A folder with no sibling git repos is a no-op, not a fabricated
/// zero-member workspace.
#[test]
fn no_candidates_is_a_noop() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0);
    assert!(!tmp.path().join("logos.workspace.toml").exists());
}

/// The default workspace name derives from the real directory name even when
/// invoked with no `--project` (root resolves to the literal `"."`, whose
/// `file_name()` is `None` unless canonicalised first) — the primary intended
/// usage: `cd` into the parent folder and run `logos init --workspace`.
#[test]
fn default_name_is_the_real_directory_name_not_the_literal_dot() {
    let tmp = two_member_fixture();
    let canonical_dir_name = tmp
        .path()
        .canonicalize()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .current_dir(tmp.path())
        .args(["--json", "init", "--workspace", "--yes"])
        .output()
        .expect("the logos binary runs");
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["workspace"], canonical_dir_name,
        "must not fall back to the literal \"workspace\""
    );

    let manifest = fs::read_to_string(tmp.path().join("logos.workspace.toml")).unwrap();
    assert!(manifest.contains(&format!("name = \"{canonical_dir_name}\"")));
}

// ── The bounded warm supervisor (S-321, FR-WS-14, BR-44) ───────────────────

/// The supervisor's entry point is internal (FR-CL-01): it must not appear in
/// top-level help output, so nobody can discover and script against it.
#[test]
fn the_supervisor_entry_point_is_hidden_from_help() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["--help"]);
    assert_eq!(exit_code(&out), 0);

    let help = String::from_utf8_lossy(&out.stdout);
    // Positive control first: an assertion about what help LACKS is vacuous if
    // the capture is empty or help ever moves to stderr.
    assert!(help.contains("index"), "help output was captured: {help}");
    assert!(
        !help.contains("internal-warm"),
        "the warm supervisor must not be listed in help output: {help}"
    );
}

/// Even hidden, the entry point has to *work* — the detached spawn's argv is
/// nulled on all three streams, so a usage error there would be invisible.
#[test]
fn the_hidden_supervisor_entry_point_still_routes() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["internal-warm", "--concurrency", "2"]);
    assert_eq!(
        exit_code(&out),
        0,
        "an empty queue drains immediately: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// End to end on a **2-member** fixture only (never a many-member one — the
/// bound itself is asserted against a stubbed spawn in
/// `logos_core::federation::warm`, so this suite never oversubscribes the host,
/// NFR-PE-08): the supervisor indexes both members, exits when the queue
/// drains, and holds no `.logos` store lock afterwards — a subsequent command
/// that opens each member's store succeeds immediately.
#[test]
fn the_supervisor_warms_a_two_member_fixture_and_releases_every_store() {
    let tmp = two_member_fixture();
    // Both members need their `.logos/` scaffolding, which enablement writes.
    let enabled = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&enabled), 0, "{}", String::from_utf8_lossy(&enabled.stderr));

    // Run the supervisor in the FOREGROUND (the detached spawn is what
    // `init --workspace` does; here we want to observe its completion).
    let out = logos(
        tmp.path(),
        &[
            "internal-warm",
            "--concurrency",
            "2",
            tmp.path().join("api").to_str().unwrap(),
            tmp.path().join("web").to_str().unwrap(),
        ],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    for member in ["api", "web"] {
        let root = tmp.path().join(member);
        // `graph_revision >= 1`, not file existence: `enable` already created
        // the db, so only a completed index moves this.
        assert!(
            graph_revision(&root) >= 1,
            "{member} was actually indexed by the supervisor, not merely scaffolded"
        );
        // No lingering lock — and a *write* is the half a WAL reader cannot
        // prove: readers never see SQLITE_BUSY, so `status` alone would pass
        // even against a live writer.
        let sync = logos(&root, &["--json", "sync"]);
        assert_eq!(
            exit_code(&sync),
            0,
            "{member}'s store accepts a writer after the queue drained: {}",
            String::from_utf8_lossy(&sync.stderr)
        );
    }
    // The supervisor opens no store of its own: it ran with `--project <parent>`,
    // which has no index at all. Had it gone through the Engine-guarded dispatch
    // path it would have exited 3 (CoreError::NoIndex) instead of 0.
    assert!(
        !tmp.path().join(".logos").exists(),
        "the supervisor must not create a store at the workspace root"
    );
}

/// A member whose index genuinely fails (here: a malformed `config.toml`, the
/// loud usage fault of FR-CF-03 — the same shape `enable` already reports as
/// `Degraded`) is recorded degraded on stderr and the queue continues to the
/// next member — it neither stalls nor aborts, and the supervisor still exits
/// 0 because the warm is advisory (FR-IX-07 carries correctness).
#[test]
fn a_failing_member_degrades_without_stalling_or_aborting_the_queue() {
    let tmp = two_member_fixture();
    let enabled = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&enabled), 0);

    let bad = tmp.path().join("broken");
    fs::create_dir_all(bad.join(".logos")).unwrap();
    fs::write(bad.join(".logos/config.toml"), "this is not [[[ toml\n").unwrap();

    let good = tmp.path().join("api");
    let last = tmp.path().join("web");
    let out = logos(
        tmp.path(),
        &[
            "internal-warm",
            "--concurrency",
            "1",
            bad.to_str().unwrap(),
            good.to_str().unwrap(),
            last.to_str().unwrap(),
        ],
    );

    assert_eq!(
        exit_code(&out),
        0,
        "one bad member is never fatal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("degraded") && stderr.contains("broken"),
        "the failing member is recorded degraded: {stderr}"
    );
    // K = 1, so the bad member was first in a strictly serial queue: both
    // members behind it must have been reached AND indexed. `graph_revision`,
    // not file existence — the latter is true before the supervisor starts.
    for member in ["api", "web"] {
        assert!(
            graph_revision(&tmp.path().join(member)) >= 1,
            "{member} was indexed after the failing member, not skipped"
        );
    }
}

/// A member the supervisor never reached must still index correctly on first
/// query (FR-IX-07) — the lazy `ensure_indexed` fallback is what makes a
/// *bounded*, therefore deferred, warm safe at all. This is exactly the state a
/// killed supervisor leaves behind for its unreached members.
///
/// Scaffolded with a plain per-member `init` rather than `init --workspace`
/// on purpose: the latter spawns a real detached supervisor that would race in
/// and warm the member, so the test would assert against an already-warm store
/// and prove nothing. Here the member is provably cold (`graph_revision == 0`)
/// before the query.
#[test]
fn a_cold_member_indexes_itself_on_its_first_navigation_call() {
    let tmp = two_member_fixture();
    let web = tmp.path().join("web");
    assert_eq!(exit_code(&logos(&web, &["init"])), 0);
    assert_eq!(graph_revision(&web), 0, "the member starts genuinely cold");

    // `search` is infallible by contract (it degrades to warnings, exit 0), so
    // the exit code proves nothing on its own — assert the hit and the index.
    let out = logos(&web, &["--json", "search", "member_entry"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let found = String::from_utf8_lossy(&out.stdout).contains("member_entry");
    assert!(
        graph_revision(&web) >= 1,
        "the first navigation call indexed the cold member (FR-IX-07)"
    );
    assert!(found, "and returned the hit from the freshly built index");
}

/// A re-run carries the already-manifested members forward and adds the new
/// one. (Which members the re-run *warms* is asserted where it is observable —
/// `workspace_init::tests::a_rerun_hands_the_warm_only_the_new_delta`; from out
/// here the warm set is not visible, so this covers the manifest half only.)
#[test]
fn a_rerun_carries_the_manifested_members_forward() {
    let tmp = two_member_fixture();
    let first = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&first), 0);

    // A third member appears only for the second run.
    init_repo(&tmp.path().join("batch"));
    let second = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&second), 0, "{}", String::from_utf8_lossy(&second.stderr));

    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let names: Vec<&str> = report["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["api", "web", "batch"],
        "the already-manifested pair is carried forward; only `batch` — the \
         newly approved delta — is what the second run's supervisor warms"
    );
}

/// `--concurrency 0` must still drain. Both floors that guarantee it
/// (`effective_concurrency`'s and `warm_queue`'s) are unit-tested, but this is
/// the boundary that ships: had either been missing, the queue would spin
/// forever inside a detached, stdio-nulled child — a hang with no diagnostic,
/// which is strictly worse than a crash.
#[test]
fn a_zero_concurrency_bound_still_drains_the_queue() {
    let tmp = two_member_fixture();
    let enabled = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&enabled), 0);

    let out = logos(
        tmp.path(),
        &[
            "internal-warm",
            "--concurrency",
            "0",
            "--",
            tmp.path().join("api").to_str().unwrap(),
        ],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        graph_revision(&tmp.path().join("api")) >= 1,
        "a zero bound is floored to one worker and the member is warmed"
    );
}

/// The `[workspace.warm] concurrency` key survives the real binary end to end
/// (S-322, FR-WS-01): a manifest declaring it enables without complaint and
/// still carries the key afterwards.
///
/// Worth an end-to-end test on top of the unit coverage for one reason: the
/// manifest parses under `deny_unknown_fields`, so an unregistered key does not
/// merely lose the warm bound — it fails the whole manifest and takes the
/// workspace back to single-root. The failure mode is total, so the acceptance
/// is checked against the real parser through the real command.
#[test]
fn a_declared_warm_concurrency_survives_enablement_and_a_rerun() {
    let tmp = two_member_fixture();
    let manifest = tmp.path().join("logos.workspace.toml");
    fs::write(
        &manifest,
        "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\n\n[workspace.warm]\nconcurrency = 2\n",
    )
    .unwrap();

    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(
        exit_code(&out),
        0,
        "a registered key must not fail the manifest: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let text = fs::read_to_string(&manifest).unwrap();
    assert!(
        text.contains("[workspace.warm]") && text.contains("concurrency = 2"),
        "the operator's tuned bound survives the incremental re-write: {text}"
    );
    assert!(
        text.contains("\"web\""),
        "and `members` was still upserted: {text}"
    );
}

/// An out-of-range value is rejected with an actionable message and exit 2
/// (FR-CF-01, NFR-UX-02) — never silently floored or clamped by the bound
/// resolution downstream.
#[test]
fn an_out_of_range_warm_concurrency_is_an_actionable_exit_2() {
    let tmp = two_member_fixture();
    fs::write(
        tmp.path().join("logos.workspace.toml"),
        "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\n\n[workspace.warm]\nconcurrency = 0\n",
    )
    .unwrap();

    let out = logos(tmp.path(), &["init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 2, "a config fault is exit 2");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("workspace.warm.concurrency"),
        "the message names the offending key: {stderr}"
    );
    let range = format!(
        "1..={}",
        logos_core::federation::warm::MANIFEST_CONCURRENCY_MAX
    );
    assert!(
        stderr.contains(&range),
        "and names the whole legal range ({range}), not just the bound breached: {stderr}"
    );
    assert!(
        stderr.contains("Omit the key"),
        "and how to get the core-derived default instead: {stderr}"
    );
}

// ── the working-tree footprint (S-333, FR-WS-02, FR-IN-04) ──────────────────

/// Enabling N members leaves N repositories git-dirty; the report says so, in
/// **both** renderings. `--json` and the human form print the same read-model
/// through the same printer, so a footprint present in one but not the other
/// would mean a rendering path composed its own — which is exactly what
/// NFR-MA-02 forbids the adapter to do.
#[test]
fn both_output_forms_carry_the_enablement_footprint() {
    let tmp = two_member_fixture();

    let machine = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&machine), 0, "{}", String::from_utf8_lossy(&machine.stderr));
    let json: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(json["footprint"]["fresh"], 2, "both members are newly dirtied");
    assert_eq!(json["footprint"]["unchanged"], 0);
    let committed: Vec<&str> = json["footprint"]["committed"]
        .as_array()
        .expect("the committed half of FR-IN-04 is stated")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(committed.contains(&".logos/config.toml"), "{committed:?}");
    assert!(
        json["footprint"]["ignored"]
            .as_array()
            .expect("the ignored half too")
            .iter()
            .any(|v| v == "logos.db*"),
        "{json}"
    );

    // The human form of a *second*, third-member run: same field, same shape.
    let batch = tmp.path().join("batch");
    init_repo(&batch);
    let human = logos(tmp.path(), &["init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&human), 0, "{}", String::from_utf8_lossy(&human.stderr));
    let pretty: serde_json::Value = serde_json::from_slice(&human.stdout)
        .expect("the human rendering is pretty JSON, not prose");
    assert_eq!(pretty["footprint"]["fresh"], 1, "only the new member is fresh");
    assert_eq!(pretty["footprint"]["unchanged"], 2, "the enabled pair already had one");
}

/// The prose half goes to stderr: stdout stays exactly one machine document
/// (FR-CL-02), and `--quiet` silences the note without emptying the report.
#[test]
fn the_footprint_note_is_advisory_on_stderr_and_quiet_suppresses_it() {
    let tmp = two_member_fixture();
    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0);

    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("stdout is exactly one JSON document, the note is not on it");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("fresh untracked `.logos/`") && stderr.contains("FR-IN-04"),
        "the operator is told what to commit and why: {stderr}"
    );

    // A fresh workspace under --quiet: the note is suppressed, and `--json`
    // still emits the essential report (FR-CL-02).
    let quiet_tmp = two_member_fixture();
    let quiet = logos(quiet_tmp.path(), &["--json", "--quiet", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&quiet), 0);
    let stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(!stderr.contains("fresh untracked"), "--quiet silences the advisory: {stderr}");
    let report: serde_json::Value = serde_json::from_slice(&quiet.stdout).unwrap();
    assert_eq!(report["footprint"]["fresh"], 2, "the machine payload is unaffected");
}

// ── the plain-`logos init` parent-of-repos nudge (S-319, FR-IN-08) ─────────

/// Run the real binary with stdin held open as a pipe nobody writes to after
/// the initial nudge answer, polling for exit rather than waiting — so a
/// command that *does* read stdin blocks forever and this fails on the
/// deadline instead of hanging the suite.
///
/// This is the only shape that can prove the load-bearing claim. `output()`
/// hands the child an already-closed stdin, so a `read_line` there returns
/// EOF immediately and a wedging implementation would still pass.
fn run_with_stdin_held_open(project: &Path, args: &[&str], feed: &str) -> Output {
    use std::io::Write;

    let mut child = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the logos binary runs");

    // Held for the whole run: dropping it would close the pipe and hand a
    // blocked `read_line` the EOF that lets it finish, silently defeating the test.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // An answer that would enable a workspace if it were ever read. That it is
    // *not* is half the assertion.
    stdin.write_all(feed.as_bytes()).expect("the pipe accepts the answer");
    stdin.flush().ok();

    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    while child.try_wait().expect("the child is waitable").is_none() {
        assert!(
            Instant::now() < deadline,
            "`logos init` never exited with stdin open — it prompted or read stdin on a \
             non-TTY, which is exactly the wedge unattended dev-pane and CI spawns hit"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(stdin);
    child.wait_with_output().expect("the finished child yields its output")
}

/// **The mandatory non-TTY test** (FR-IN-08, S-319). At a parent-of-repos root
/// an unattended `logos init`:
/// - never prompts and never reads stdin (proved by exiting with the pipe open,
///   and by the fed `y` having no effect);
/// - explains the shape on stderr, naming `logos init --workspace`;
/// - completes the plain single-root `init`.
#[test]
fn a_non_tty_init_at_a_parent_of_repos_root_never_prompts_and_never_reads_stdin() {
    let tmp = two_member_fixture();
    let out = run_with_stdin_held_open(tmp.path(), &["--json", "init"], "y\ny\ny\n");

    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("parent folder of sibling repositories")
            && stderr.contains("logos init --workspace"),
        "the shape and the remedy are explained on stderr: {stderr}"
    );
    assert!(
        !stderr.contains("[y/N]") && !stderr.contains("[Y/n]"),
        "no prompt is rendered on a non-TTY: {stderr}"
    );
    assert!(
        !tmp.path().join("logos.workspace.toml").exists(),
        "the fed `y` was never read — a non-TTY must never auto-enable a workspace"
    );
    assert!(
        tmp.path().join(".logos").join("config.toml").is_file(),
        "the plain single-root init still completed"
    );
}

/// stdout stays byte-for-byte as before (FR-CL-02): the nudge is stderr-only,
/// and the machine document `init` emits at a parent-of-repos root is the same
/// document it emits at an ordinary repository root.
///
/// Compared field-set to field-set rather than byte to byte on the raw payloads,
/// because the report legitimately carries the differing root path — the
/// assertion that matters is that the nudge adds nothing and removes nothing.
#[test]
fn the_nudge_leaves_stdout_byte_for_byte_as_before() {
    let parent = two_member_fixture();
    let nudged = run_with_stdin_held_open(parent.path(), &["--json", "init"], "");
    assert_eq!(exit_code(&nudged), 0);

    let ordinary = TempDir::new().unwrap();
    init_repo(ordinary.path());
    let plain = logos(ordinary.path(), &["--json", "init"]);
    assert_eq!(exit_code(&plain), 0);

    let as_value = |bytes: &[u8]| -> serde_json::Value {
        serde_json::from_slice(bytes).expect("stdout is exactly one JSON document")
    };
    let nudged_doc = as_value(&nudged.stdout);
    let plain_doc = as_value(&plain.stdout);

    let keys = |v: &serde_json::Value| -> Vec<String> {
        let mut k: Vec<String> = v.as_object().expect("an object").keys().cloned().collect();
        k.sort();
        k
    };
    assert_eq!(keys(&nudged_doc), keys(&plain_doc), "the nudge adds no stdout field");

    let raw = String::from_utf8_lossy(&nudged.stdout);
    for fragment in ["parent folder", "logos init --workspace", "enable a Logos workspace"] {
        assert!(!raw.contains(fragment), "the nudge is stderr-only, not on stdout: {raw}");
    }
}

/// At an ordinary git repository root there is no nudge and no prompt — the
/// negative case, asserted on a repository that even contains a nested
/// checkout, since only its own git-top-level status separates the two shapes.
#[test]
fn an_ordinary_repository_root_gets_no_nudge_and_no_prompt() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    init_repo(&tmp.path().join("vendor"));

    let out = run_with_stdin_held_open(tmp.path(), &["--json", "init"], "y\n");
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let stderr = String::from_utf8_lossy(&out.stderr);
    for fragment in ["parent folder", "enable a Logos workspace", "[y/N]"] {
        assert!(!stderr.contains(fragment), "no nudge at a repository root: {stderr}");
    }
    assert!(!tmp.path().join("logos.workspace.toml").exists());
}

/// Re-running `init` at a parent-of-repos root stays non-clobbering
/// (FR-IN-01): the second run reports `config.toml`/`rules.toml` unchanged and
/// leaves an operator's edits in place. The nudge changes nothing about the
/// init step sequence — asserted with a real edit, because "reported unchanged"
/// and "actually unchanged" are separable failures.
#[test]
fn rerunning_init_at_a_parent_of_repos_root_stays_non_clobbering() {
    let tmp = two_member_fixture();
    let first = run_with_stdin_held_open(tmp.path(), &["--json", "init"], "");
    assert_eq!(exit_code(&first), 0, "{}", String::from_utf8_lossy(&first.stderr));

    let config = tmp.path().join(".logos").join("config.toml");
    let rules = tmp.path().join(".logos").join("rules.toml");
    let edited = format!("{}\n# operator edit\n", fs::read_to_string(&config).unwrap());
    fs::write(&config, &edited).unwrap();
    let rules_before = fs::read_to_string(&rules).unwrap();

    let second = run_with_stdin_held_open(tmp.path(), &["--json", "init"], "");
    assert_eq!(exit_code(&second), 0, "{}", String::from_utf8_lossy(&second.stderr));

    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let steps = report["steps"].as_array().expect("the init report lists its steps");
    for target in [".logos/config.toml", ".logos/rules.toml"] {
        let step = steps
            .iter()
            .find(|s| s["target"].as_str() == Some(target))
            .unwrap_or_else(|| panic!("the report names {target}: {report}"));
        assert_eq!(step["action"], "unchanged", "{target} is reported unchanged: {step}");
    }
    assert_eq!(fs::read_to_string(&config).unwrap(), edited, "the operator edit survives");
    assert_eq!(fs::read_to_string(&rules).unwrap(), rules_before, "rules.toml is untouched");

    // Still the plain single-root init on both runs — the nudge never enabled anything.
    assert!(!tmp.path().join("logos.workspace.toml").exists());
}

/// **Regression (review finding).** A missing `git` binary must *suppress* the
/// diagnosis, never invert it.
///
/// `is_git_root` collapses "git is absent" into `false` — safe for an admission
/// filter, inverting for a negative inference. Read through it, `detect`'s first
/// guard never tripped without git, so an **ordinary repository root** holding
/// one already-indexed subdirectory (someone ran `logos init` in `./frontend`)
/// was announced as "a parent folder of sibling repositories … not a repository
/// itself" and, on a TTY, offered federation — which would write
/// `logos.workspace.toml` into a real repository and enrol its own subdirectory
/// as a member. Reproduced against the built binary before the fix.
///
/// `PATH` is set on the **child** only, so nothing here mutates this process's
/// environment and the test is safe under libtest's parallel threads.
#[test]
fn a_missing_git_binary_suppresses_the_nudge_rather_than_inverting_it() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    // The child that makes the shape mis-fire: not a git root, but already
    // indexed, so `discover_candidates` still admits it as a member.
    let indexed = repo.join("frontend");
    fs::create_dir_all(indexed.join(".logos")).unwrap();
    fs::write(indexed.join(".logos/logos.db"), b"").unwrap();

    // An empty directory as PATH: `git` is unreachable, and `logos` itself is
    // invoked by absolute path so it still runs.
    let no_git = tmp.path().join("empty-path");
    fs::create_dir_all(&no_git).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(&repo)
        .args(["--json", "init"])
        .env("PATH", &no_git)
        .output()
        .expect("the logos binary runs without git on PATH");

    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("parent folder of sibling repositories"),
        "an unanswerable git question must not be read as `not a repository`: {stderr}"
    );
    assert!(
        !stderr.contains("enable a Logos workspace"),
        "and must never reach the offer: {stderr}"
    );
    assert!(
        !repo.join("logos.workspace.toml").exists(),
        "no workspace manifest may be written inside a real repository"
    );
    // The plain single-root init still completed — suppressing the nudge does
    // not suppress the command.
    assert!(repo.join(".logos").join("config.toml").is_file());
}

