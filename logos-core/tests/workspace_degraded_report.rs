//! Fitness function for **quiet** degraded-member reporting: one unopenable
//! member in a `workspace status` answer costs exactly one open attempt and
//! exactly one diagnostic line, however many all-member walks the read-model
//! makes — on **both** the CLI one-shot and the long-lived serve registry
//! ([CR-102] §4, [CR-105], [FR-WS-16], [NFR-CC-04], [NFR-PE-10]).
//!
//! The unit for "once" is one **answer**, not one registry. S-332 delivered the
//! guarantee by gating the suppression on `RegistryMode::Lazy`, which held for
//! the one-shot and missed the one production `RegistryMode::Serve` registry —
//! the federated web surface, where four walks × 63 degraded members was 252
//! open attempts and ~100 KB of near-identical warning text on every
//! `GET /api/v1/workspace/status`. S-336 replaced the mode gate with a
//! per-answer scope, so the serve assertions below run the identical shape as
//! the one-shot ones, and the *second request* assertions are what keep the
//! scope from outliving its answer ([CR-105]).
//!
//! The unit tests in `federation::registry` prove the single-attempt seam against
//! spy engines. This proves it against the **real read-model and a real
//! `Engine`**: `workspace_status` walks every member four times through
//! `federation::query`, `federation::coverage` (twice) and `federation::topics`,
//! and three of those walks previously re-attempted a member whose engine had
//! already failed to start — so one broken member in the measured 84-member
//! workspace produced three identical `WARN` lines and three wasted opens,
//! growing as `3 × N`.
//!
//! The degraded member is made degraded the way a filesystem can guarantee: a
//! **directory** at the member's canonical store path `<root>/.logos/logos.db`,
//! which nothing can open as a database. No descriptor limit is lowered and no
//! permission bit is flipped, so the fixture is deterministic on every platform
//! and under every user — including a CI runner that is root, for whom a
//! `chmod 000` store would open perfectly well.
//!
//! # What must NOT move
//! Everything a consumer reads. The exit-code predicate, the `degraded_rollup`
//! and **both** completeness markers — the roll-up's and the coverage tier's
//! own — are asserted on the degraded side *and* on a healthy workspace in the
//! same binary, so a change in what the suppression costs the payload fails
//! here rather than in a release.
//!
//! The walk count itself is deliberately **not** asserted here: the whole point
//! of the fix is that one attempt and one line are paid whatever the walk count
//! is. `workspace_connection_budget.rs`'s `WALKS_PER_STATUS` is what pins the
//! count, and it is the test that fails if a fan-out is added.
//!
//! [CR-102]: ../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
//! [CR-105]: ../../docs/requests/CR-105-report-a-failed-member-open-once-per-answer.md
//! [FR-WS-16]: ../../docs/specs/requirements/FR-WS-16.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md

use std::path::Path;
use std::sync::{Arc, Mutex, Once};

use logos_core::federation::{
    workspace_status, EngineRegistry, Federation, Member, RegistryMode, WorkspaceStatus,
};
use logos_core::Engine;
use tracing_subscriber::layer::SubscriberExt;

/// A minimal member repo: one tiny source file, no index. The report is a
/// property of the *open attempt*, so the fixture needs a real store path, not a
/// large one.
fn member_repo(root: &Path, name: &str) -> Member {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).expect("member dir");
    std::fs::write(repo.join("lib.rs"), "pub fn f() {}\n").expect("member source");
    Member {
        name: name.to_string(),
        root: repo,
    }
}

/// Make `member`'s store **present but unopenable** by putting a directory where
/// its database file belongs.
///
/// Deterministic in a way a permission bit is not: `open(2)` on a directory
/// cannot succeed for any user, so the fixture means the same thing on a
/// developer laptop and on a root CI runner.
fn obstruct_store(member: &Member) {
    let store = member.root.join(".logos").join("logos.db");
    std::fs::create_dir_all(&store).expect("a directory where the store belongs");
}

fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "w".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    }
}

/// A CLI one-shot registry — the shape `logos workspace status` builds, and
/// dropped again when the command ends.
fn one_shot(federation: Federation) -> EngineRegistry<Engine> {
    EngineRegistry::new(federation, RegistryMode::Lazy)
}

/// A **serve** registry — the shape the federated web surface builds once and
/// holds for the process lifetime, answering many requests from it.
///
/// The eager warm it performs is itself one open attempt per member, so every
/// assertion below measures a *delta* from the count after construction rather
/// than an absolute.
fn serving(federation: Federation) -> EngineRegistry<Engine> {
    EngineRegistry::new(federation, RegistryMode::Serve)
}

/// The parts of a `workspace status` payload [CR-105] promises are unchanged:
/// the degraded roll-up, the per-member rows, and both completeness markers.
fn payload_contract(status: &WorkspaceStatus) -> serde_json::Value {
    let value = serde_json::to_value(status).expect("status serialises");
    serde_json::json!({
        "degraded_rollup": value["degraded_rollup"],
        "members": value["members"],
        "coverage_covers_all_members": status.coverage.covers_all_members,
        "coverage_read": [status.coverage.members_read, status.coverage.members_total],
    })
}

/// Raise the process-wide max level once, for the same reason
/// `tests/config_apply.rs` does: a scoped `with_default` subscriber does not move
/// the global level gate the `warn!` macro checks first, so without a permissive
/// global default the events under test are dropped before reaching the scoped
/// layer.
fn ensure_global_level() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    });
}

/// Captures the message of every `WARN` (or worse) event emitted on the calling
/// thread — the human diagnostic channel, which is exactly what a duplicate
/// costs an operator.
#[derive(Default)]
struct WarnRecorder {
    lines: Arc<Mutex<Vec<String>>>,
}

struct MessageVisitor<'a>(&'a mut Option<String>);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = Some(format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnRecorder {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() > tracing::Level::WARN {
            return;
        }
        let mut message = None;
        event.record(&mut MessageVisitor(&mut message));
        if let Some(message) = message {
            self.lines.lock().expect("warn lines").push(message);
        }
    }
}

/// Run `f`, returning every `WARN` message it emitted on this thread.
fn captured_warnings<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    ensure_global_level();
    let recorder = WarnRecorder::default();
    let lines = Arc::clone(&recorder.lines);
    let subscriber = tracing_subscriber::registry().with(recorder);
    let value = tracing::subscriber::with_default(subscriber, f);
    let captured = lines.lock().expect("warn lines").clone();
    (value, captured)
}

/// Warnings that report a member's **engine start** failing — the per-member,
/// subject-independent diagnostic that used to repeat once per walk. A read
/// failure is a different fact on a different schedule and is not counted here.
fn start_failure_lines(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|line| line.contains("engine failed to start"))
        .collect()
}

/// [CR-102] AC3 and AC5 together: one unopenable member yields **one** open
/// attempt and **one** diagnostic line across all the walks, and every figure a
/// consumer reads is what it was.
#[test]
fn one_unopenable_member_costs_one_attempt_and_one_diagnostic_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let healthy = member_repo(root, "api");
    let broken = member_repo(root, "web");
    obstruct_store(&broken);

    let registry = one_shot(federation(root, vec![healthy.clone(), broken.clone()]));
    let (status, warnings) = captured_warnings(|| workspace_status(&registry));

    // ── One attempt, not one per walk ────────────────────────────────────
    assert_eq!(
        registry.start_failures(),
        1,
        "the broken member was attempted ONCE across every all-member walk, \
         not once per walk — {} attempts recorded",
        registry.start_failures()
    );

    // ── One diagnostic line, not one per walk ────────────────────────────
    let announced = start_failure_lines(&warnings);
    assert_eq!(
        announced.len(),
        1,
        "the operator is told once per command, not once per walk: {warnings:?}"
    );
    assert!(
        announced[0].contains("web"),
        "and the one line names the member: {}",
        announced[0]
    );

    // ── The payload is unchanged: the member is still reported degraded, ──
    // ── still named, and still moves the exit-code predicate. ────────────
    let degraded = &status.degraded_rollup;
    assert_eq!(degraded.members, 2, "both roster members are accounted for");
    assert_eq!(degraded.opened, 1, "the healthy member opened");
    assert_eq!(
        degraded.degraded_members,
        ["web"],
        "the broken member is still named in the roll-up: {degraded:?}"
    );
    assert!(
        !degraded.all_opened(),
        "so the command still exits non-zero (FR-WS-16, FR-CL-03)"
    );
    assert!(
        !degraded.covers_all_members,
        "and the member-derived figures still declare themselves partial"
    );
    // The coverage tier derives its OWN marker from its OWN walk — one of the
    // two that now replay rather than re-attempt — so it is asserted here
    // beside the roll-up's, not assumed to follow it ([NFR-CC-04]). A replay
    // that let the coverage walk count a member it never read fails here.
    assert!(
        !status.coverage.covers_all_members,
        "the coverage tier's own marker declares itself partial too: {:?}",
        status.coverage
    );
    assert_eq!(
        (status.coverage.members_read, status.coverage.members_total),
        (1, 2),
        "and the replayed Err still drops the member from the coverage counts"
    );

    // The member row keeps its own diagnostic on its own key, which is why
    // suppressing the repeat WARN loses nothing.
    let row = member_row(&status, "web");
    assert_eq!(row["open_state"], "degraded");
    assert!(
        row["error"].as_str().is_some(),
        "the freshness walk's real attempt still reports the diagnostic on the \
         row: {row}"
    );
    // The classified cause is this fixture's own — a path occupied by something
    // that is not a regular file — and it keeps its own single, unambiguous
    // remedy. The two-candidate remedy belongs to a *present* store that cannot
    // be opened, whose only deterministic fixture is a permission bit (and so no
    // fixture at all for a root CI runner); it is asserted in
    // `federation::open_state`'s own tests, which take the store's state as an
    // explicit input rather than probing the filesystem for it.
    let reason = row["degraded_reason"].as_str().expect("a degraded reason");
    assert!(
        reason.contains("not a regular file"),
        "the row carries the classified remedy for what actually failed: {reason}"
    );
    assert_eq!(row["degraded_cause"], "store-obstructed");
}

/// [CRA-06]: a **fresh** command retries, so a transient condition clears on the
/// next invocation rather than persisting until something is restarted.
///
/// Run against the same on-disk workspace, with the obstruction cleared between
/// the two commands — which is what "the condition cleared" means for a store
/// that could not be opened. A suppression that outlived the command would leave
/// the second command reporting the member degraded over a store it can now open.
///
/// [CRA-06]: ../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
#[test]
fn a_fresh_command_retries_a_member_whose_condition_has_cleared() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let healthy = member_repo(root, "api");
    let broken = member_repo(root, "web");
    obstruct_store(&broken);
    let fed = || federation(root, vec![healthy.clone(), broken.clone()]);

    let first = one_shot(fed());
    let before = workspace_status(&first);
    assert_eq!(
        before.degraded_rollup.degraded_members,
        ["web"],
        "the first command reports the member degraded"
    );
    drop(first);

    // The condition clears: the directory squatting on the store path is gone.
    std::fs::remove_dir_all(broken.root.join(".logos")).expect("clear the obstruction");

    let second = one_shot(fed());
    let after = workspace_status(&second);
    assert_eq!(
        second.start_failures(),
        0,
        "the fresh command really re-attempted the member"
    );
    assert!(
        after.degraded_rollup.degraded_members.is_empty(),
        "and reports nothing degraded now that it opens: {:?}",
        after.degraded_rollup
    );
    assert!(
        after.degraded_rollup.all_opened(),
        "so the retry restores the zero exit code"
    );
}

/// A **healthy** workspace is byte-unchanged: no diagnostic line at all, no
/// suppressed attempt, and the same roll-up and coverage marker it always had.
///
/// The pair to the degraded assertions above — a suppression that leaked into
/// the healthy path would be the worse defect, since every run pays it.
#[test]
fn a_healthy_workspace_reports_nothing_and_attempts_every_member() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let members = vec![member_repo(root, "api"), member_repo(root, "web")];

    let registry = one_shot(federation(root, members));
    let (status, warnings) = captured_warnings(|| workspace_status(&registry));

    assert_eq!(registry.start_failures(), 0, "nothing failed to open");
    assert!(
        start_failure_lines(&warnings).is_empty(),
        "a healthy workspace says nothing on the diagnostic channel: {warnings:?}"
    );
    let degraded = &status.degraded_rollup;
    assert_eq!((degraded.members, degraded.opened), (2, 2));
    assert!(degraded.degraded_members.is_empty());
    assert!(degraded.all_opened(), "and exits 0");
    assert!(degraded.covers_all_members);
    assert!(
        status.coverage.covers_all_members,
        "the coverage tier's own marker is complete too — a separate walk with a \
         separate marker (NFR-CC-04)"
    );

    // Every member was really opened, and the suppression reached none of them:
    // it is keyed on a *recorded failure*, and this workspace recorded none.
    // Two members sit far inside any host's budget, so both stay resident and
    // the later walks hit the resident map — one construction each is exactly
    // what a healthy `workspace status` pays ([NFR-PE-10], [NFR-PE-11]).
    assert_eq!(
        registry.engine_starts() as usize,
        status.members.len(),
        "{} engine starts for {} members — every member opened once and stayed \
         resident for the remaining walks",
        registry.engine_starts(),
        status.members.len()
    );
}

/// [CR-105] AC1 and AC2 on the **serve** surface: one unopenable member costs
/// one open attempt and one diagnostic line per request, and the payload a
/// consumer reads is byte-identical to the one-shot's.
///
/// The registry here outlives the answer, which is the whole difference from
/// `one_unopenable_member_costs_one_attempt_and_one_diagnostic_line` above. The
/// shipped behaviour before S-336 was four attempts and four `WARN`s, because
/// the suppression was gated on `RegistryMode::Lazy`; the attempt count is the
/// assertion rather than a proxy for it.
///
/// [CR-105]: ../../docs/requests/CR-105-report-a-failed-member-open-once-per-answer.md
#[test]
fn a_serve_registry_costs_one_attempt_and_one_line_per_request() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let healthy = member_repo(root, "api");
    let broken = member_repo(root, "web");
    obstruct_store(&broken);

    let registry = serving(federation(root, vec![healthy.clone(), broken.clone()]));
    // The eager warm is one attempt of its own, made outside any answer.
    let warmed = registry.start_failures();
    assert_eq!(warmed, 1, "the eager warm attempted the broken member once");

    let (status, warnings) = captured_warnings(|| workspace_status(&registry));

    assert_eq!(
        registry.start_failures() - warmed,
        1,
        "the request attempted the broken member ONCE across all four walks, \
         not once per walk — {} attempts recorded over the answer",
        registry.start_failures() - warmed
    );
    let announced = start_failure_lines(&warnings);
    assert_eq!(
        announced.len(),
        1,
        "the operator is told once per request, not once per walk: {warnings:?}"
    );
    assert!(
        announced[0].contains("web"),
        "and the one line names the member: {}",
        announced[0]
    );

    // The roll-up's own notice is the grouped form S-337 shipped: the cause once,
    // over the members it affects — asserted here so the serve surface is pinned
    // to the same text the CLI prints, not to a per-member vintage of it.
    let notice = status
        .degraded_rollup
        .notice(&registry.open_states())
        .expect("one member is degraded");
    assert!(
        notice.contains("affected (1): web"),
        "the notice groups its members under one cause: {notice}"
    );

    // ── The payload is exactly the one-shot's ────────────────────────────
    // Taken after the serving registry is dropped, so the two never hold the
    // healthy member's store at the same time.
    let serve_payload = payload_contract(&status);
    drop(registry);
    let cli = one_shot(federation(root, vec![healthy, broken]));
    let cli_payload = payload_contract(&workspace_status(&cli));
    assert_eq!(
        serve_payload, cli_payload,
        "same degraded_rollup, same per-member rows, same completeness markers — \
         only the attempt and diagnostic counts behind them changed"
    );
}

/// [CR-105] AC3, the positive: a **second** request re-attempts and
/// re-announces, so cross-request suppression is not introduced.
///
/// This is the criterion a scope that accidentally outlives its answer fails. A
/// registry-lifetime latch would make both deltas below zero, which is why they
/// are asserted as equalities rather than as bounds.
///
/// [CR-105]: ../../docs/requests/CR-105-report-a-failed-member-open-once-per-answer.md
#[test]
fn a_second_request_re_attempts_and_re_announces_a_still_broken_member() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let healthy = member_repo(root, "api");
    let broken = member_repo(root, "web");
    obstruct_store(&broken);

    let registry = serving(federation(root, vec![healthy, broken]));
    let (_, first) = captured_warnings(|| workspace_status(&registry));
    let after_first = registry.start_failures();
    assert_eq!(
        start_failure_lines(&first).len(),
        1,
        "the first request announces the member once: {first:?}"
    );

    let (second_status, second) = captured_warnings(|| workspace_status(&registry));

    assert_eq!(
        registry.start_failures() - after_first,
        1,
        "the second request really re-attempted the member — once, not zero and \
         not four"
    );
    assert_eq!(
        start_failure_lines(&second).len(),
        1,
        "and re-announced it, because a member that keeps failing across requests \
         must keep being reported: {second:?}"
    );
    assert_eq!(
        second_status.degraded_rollup.degraded_members,
        ["web"],
        "and it is still named in the second answer's roll-up"
    );
}

/// [CR-105] AC3's other half: a member that **recovers** between two requests is
/// not reported degraded by the later one, on the same long-lived registry.
///
/// The transient-recovery property the old `RegistryMode` gate existed to
/// protect. Run against one serving registry with the obstruction cleared
/// between the requests — a scope that outlived its answer would leave the
/// member degraded until the process restarted, which is exactly the regression
/// this asserts against.
///
/// [CR-105]: ../../docs/requests/CR-105-report-a-failed-member-open-once-per-answer.md
#[test]
fn a_member_that_recovers_between_requests_is_not_degraded_by_the_later_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let healthy = member_repo(root, "api");
    let broken = member_repo(root, "web");
    obstruct_store(&broken);

    let registry = serving(federation(root, vec![healthy, broken.clone()]));
    let before = workspace_status(&registry);
    assert_eq!(
        before.degraded_rollup.degraded_members,
        ["web"],
        "the first request reports the member degraded"
    );

    // The condition clears between the two requests: the directory squatting on
    // the store path is gone, and the same registry serves the next request.
    std::fs::remove_dir_all(broken.root.join(".logos")).expect("clear the obstruction");

    let (after, warnings) = captured_warnings(|| workspace_status(&registry));
    assert!(
        after.degraded_rollup.degraded_members.is_empty(),
        "the later request opens it and reports nothing degraded: {:?}",
        after.degraded_rollup
    );
    assert!(
        after.degraded_rollup.all_opened(),
        "so the recovered workspace reads whole again"
    );
    assert!(
        after.degraded_rollup.covers_all_members && after.coverage.covers_all_members,
        "and both completeness markers say so: {:?}",
        after.coverage
    );
    assert!(
        start_failure_lines(&warnings).is_empty(),
        "with nothing left to announce: {warnings:?}"
    );
}

/// One member row from a `workspace status`, as a consumer reads it.
fn member_row(status: &WorkspaceStatus, member: &str) -> serde_json::Value {
    let value = serde_json::to_value(status).expect("status serialises");
    value["members"]
        .as_array()
        .expect("a member table")
        .iter()
        .find(|row| row["member"] == member)
        .unwrap_or_else(|| panic!("{member} is in the roster: {value}"))
        .clone()
}
