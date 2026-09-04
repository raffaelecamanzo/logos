//! The durable warm-outcome record, end to end ([FR-WS-17], [BR-47], [CR-102]).
//!
//! The unit tests in `federation::warm_state` pin the schema, the atomic write
//! and the derivation truth table; the ones in `federation::warm` pin the
//! producer. This binary proves the thing neither can: that the two halves
//! **join through the filesystem**, so a warm outcome survives the process that
//! produced it.
//!
//! # Why "cold process" is the load-bearing word
//! The defect this requirement corrects was not that the supervisor failed to
//! notice a failed member — it noticed perfectly. It recorded the fact in an
//! in-process `WarmSummary`, printed it to a stderr the real detached spawn
//! sends to `/dev/null`, and keyed it on absolute roots rather than the member
//! names read-models join on. Every one of those is invisible to a later
//! `workspace status`, so three shipped acceptance criteria ([FR-WS-14] AC3,
//! [FR-WS-02], [FR-WS-15] AC4) promised a `degraded` state the code could not
//! produce.
//!
//! A test that produced the outcome and read it back through a value held in
//! memory would pass against exactly that broken design. So the assertion here
//! is deliberately shaped to fail against it: the record is produced, **every
//! in-process value from the producing side is dropped**, and a registry built
//! from nothing but the manifest and the filesystem must still report the
//! member `degraded` with the recorded reason. The only channel left between
//! the two is the sidecar on disk.
//!
//! [FR-WS-02]: ../../docs/specs/requirements/FR-WS-02.md
//! [FR-WS-14]: ../../docs/specs/requirements/FR-WS-14.md
//! [FR-WS-15]: ../../docs/specs/requirements/FR-WS-15.md
//! [FR-WS-17]: ../../docs/specs/requirements/FR-WS-17.md
//! [CR-102]: ../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
//! [BR-47]: ../../docs/specs/software-spec.md#327-workspace-federation

use std::path::{Path, PathBuf};

use logos_core::federation::warm::{record_outcomes, MemberWarm, WarmSummary};
use logos_core::federation::warm_state::{
    read_outcomes, WarmOutcome, WarmOutcomes, OUTCOME_FILENAME,
};
use logos_core::federation::{
    workspace_status, EngineRegistry, Federation, Member, MemberWarmState, RegistryMode,
    WorkspaceStatus, MANIFEST_FILENAME,
};
use logos_core::Engine;

/// A workspace root carrying a manifest and one directory per member.
///
/// `indexed` members get a real source file **and a real index**, so their
/// `warm` label comes from a graph that genuinely holds files rather than from
/// a stubbed status — index presence is the fact that outranks the record, and
/// faking it would leave the rule it is being weighed against untested.
fn workspace(members: &[(&str, bool)]) -> (tempfile::TempDir, PathBuf, Vec<Member>) {
    let dir = tempfile::tempdir().expect("workspace dir");
    let root = dir.path().canonicalize().expect("canonical workspace root");
    std::fs::write(
        root.join(MANIFEST_FILENAME),
        "[workspace]\nname = \"w\"\n",
    )
    .expect("manifest");

    let resolved = members
        .iter()
        .map(|(name, indexed)| {
            let member_root = root.join(name);
            std::fs::create_dir_all(&member_root).expect("member dir");
            if *indexed {
                std::fs::write(member_root.join("lib.rs"), "pub fn f() {}\n").expect("source");
                // `start`, not `open`: it is the constructor that actually creates
                // the member store, and a graph holding real files is the whole
                // point of the `indexed` members here.
                Engine::start(&member_root).expect("member engine").index();
            }
            Member {
                name: (*name).to_string(),
                root: member_root.canonicalize().expect("canonical member"),
            }
        })
        .collect();
    (dir, root, resolved)
}

/// The registry a `logos workspace status` one-shot builds — and nothing else.
/// Everything it knows comes from the manifest's member set and the filesystem.
fn status_from_a_cold_start(root: &Path, members: &[Member]) -> WorkspaceStatus {
    let federation = Federation {
        name: "w".to_string(),
        root: root.to_path_buf(),
        members: members.to_vec(),
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    };
    workspace_status(&EngineRegistry::new(federation, RegistryMode::Lazy))
}

/// The summary a supervisor pass produces: absolute roots, one optional reason.
fn summary(outcomes: &[(&Member, Option<&str>)]) -> WarmSummary {
    WarmSummary {
        concurrency: 2,
        members: outcomes
            .iter()
            .map(|(member, degraded)| MemberWarm {
                root: member.root.display().to_string(),
                degraded: degraded.map(str::to_string),
            })
            .collect(),
    }
}

fn warm_state_of<'a>(status: &'a WorkspaceStatus, member: &str) -> &'a MemberWarmState {
    &status
        .members
        .iter()
        .find(|row| row.status.member == member)
        .unwrap_or_else(|| panic!("no row for {member}"))
        .warm
}

/// [FR-WS-17] AC1 at the **filesystem** join: after a warm in which one member's
/// index failed, a `workspace status` sharing nothing with the producer but the
/// disk reports that member `degraded` **carrying the recorded reason**.
///
/// The supervisor's side is confined to its own scope and dropped before the
/// reading side exists, so nothing but the sidecar can carry the fact across.
/// Deleting the `record_outcomes` call, keying the record on absolute roots, or
/// writing it anywhere a `workspace status` does not look all fail here.
///
/// Named for a cold *registry*, not a cold process, because that is what it is:
/// both halves run in one process, so a hypothetical in-process memo of the
/// record would still pass. The genuine process-boundary proof — two real
/// `logos` binaries, the producer provably exited — is
/// `cli/tests/init_workspace.rs::a_failed_warm_is_degraded_in_a_later_cold_process`.
#[test]
fn a_failed_warm_reports_degraded_from_a_cold_registry_with_its_reason() {
    let (_dir, root, members) = workspace(&[("api", false), ("web", false)]);

    // ── the supervisor process ────────────────────────────────────────────
    {
        let pass = summary(&[
            (&members[0], None),
            (&members[1], Some("index failed: exit status: 2")),
        ]);
        record_outcomes(&pass);
        // …and it exits. `pass` dies with this scope.
    }

    // ── a cold `workspace status`, sharing nothing but the filesystem ─────
    let status = status_from_a_cold_start(&root, &members);

    assert_eq!(
        warm_state_of(&status, "web"),
        &MemberWarmState::Degraded {
            reason: "index failed: exit status: 2".to_string()
        },
        "the recorded reason must survive the supervisor, verbatim"
    );
    // Its neighbour succeeded with nothing indexable: `warm`, not `deferred`
    // ([FR-WS-17] AC2) — the half a failure-only marker could never express.
    assert_eq!(warm_state_of(&status, "api"), &MemberWarmState::Warm);

    assert_eq!(status.warm_rollup.degraded, 1);
    assert_eq!(status.warm_rollup.warm, 1);
    assert_eq!(status.warm_rollup.deferred, 0);
}

/// Make `member`'s store present but unopenable — a directory where its
/// database file belongs. Deterministic on every platform and every user,
/// including a root CI runner for whom a permission bit would open fine.
///
/// Duplicated from `workspace_degraded_report.rs` rather than shared: each
/// `tests/*.rs` file is its own binary, and this whole file already accepts
/// its own copies of comparable fixtures (`workspace`, `summary`) for the
/// same reason.
fn obstruct_store(member: &Member) {
    let store = member.root.join(".logos").join("logos.db");
    std::fs::create_dir_all(&store).expect("a directory where the store belongs");
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

/// [FR-WS-16] and [FR-WS-17] together, through the real read-model rather than
/// `derive_state`'s pure truth table: a member whose store is **live-broken**
/// this run ([`obstruct_store`], S-332's fixture) AND whose **durable record**
/// says a past warm failed, for a *different* reason.
///
/// `derive_state`'s own precedence table says the durable record outranks the
/// transient open error for `warm_state`'s `reason` — pinned in
/// `federation::warm_state`'s unit tests — but nothing before this test built
/// that combination through a real obstructed store and a real on-disk record
/// at once, so a regression that let the live error leak into `reason`, or the
/// record leak into `degraded_reason`, could ship unnoticed: every existing
/// fixture in this file leaves `open_state` healthy, and every one in
/// `workspace_degraded_report.rs` leaves no outcome record on disk.
#[test]
fn a_member_can_be_degraded_on_both_axes_with_two_different_reasons() {
    let (_dir, root, members) = workspace(&[("api", true), ("web", false)]);
    let web = &members[1];

    obstruct_store(web);
    record_outcomes(&summary(&[(web, Some("index failed: exit status: 2"))]));

    let status = status_from_a_cold_start(&root, &members);

    assert_eq!(
        warm_state_of(&status, "web"),
        &MemberWarmState::Degraded {
            reason: "index failed: exit status: 2".to_string()
        },
        "the WARM axis reads the durable record, not this run's own open failure"
    );

    let row = member_row(&status, "web");
    assert_eq!(row["warm_state"], "degraded");
    assert_eq!(row["open_state"], "degraded");
    assert_eq!(
        row["reason"], "index failed: exit status: 2",
        "the warm axis' reason is the durable record's, verbatim"
    );
    let live_error = row["degraded_reason"]
        .as_str()
        .expect("the open axis carries its own live diagnostic");
    assert!(
        live_error.contains("not a regular file"),
        "the OPEN axis' reason is this run's own obstruction, not the record: {live_error}"
    );
    assert_ne!(
        row["reason"], row["degraded_reason"],
        "two independent facts about the same member — a recorded warm failure \
         and a live open failure — must not collapse into one string just \
         because they happen to be reported on the same row"
    );
}

/// [BR-47]'s truth table over the **real** read-model, not the pure derivation:
/// four members, four states, one `workspace status`.
///
/// The `indexed` member is genuinely indexed, so the "index presence beats a
/// stale record" cell is decided by an actual graph.
#[test]
fn the_truth_table_holds_through_workspace_status() {
    let (_dir, root, members) = workspace(&[
        ("succeeded-empty", false),
        ("no-record", false),
        ("indexed-stale-failure", true),
        ("failed-no-index", false),
    ]);

    record_outcomes(&summary(&[
        (&members[0], None),
        (&members[2], Some("index failed: killed")),
        (&members[3], Some("spawn failed: No such file or directory")),
    ]));

    let status = status_from_a_cold_start(&root, &members);

    // Warm succeeded, member holds no supported-language file ⇒ `warm`.
    assert_eq!(
        warm_state_of(&status, "succeeded-empty"),
        &MemberWarmState::Warm
    );
    // Neither record nor index ⇒ `deferred`.
    assert_eq!(warm_state_of(&status, "no-record"), &MemberWarmState::Deferred);
    // Graph holds indexed files ⇒ `warm`, EVEN AGAINST a record saying it
    // failed. Nothing had to clear the record for this to be true.
    assert_eq!(
        warm_state_of(&status, "indexed-stale-failure"),
        &MemberWarmState::Warm
    );
    // Recorded failure, no index ⇒ `degraded` with the recorded reason.
    assert_eq!(
        warm_state_of(&status, "failed-no-index"),
        &MemberWarmState::Degraded {
            reason: "spawn failed: No such file or directory".to_string()
        }
    );

    // The roll-up still partitions the member set, and `warming` is still
    // OMITTED rather than zeroed — a finished outcome is not a live signal
    // ([FR-WS-17] AC7, [NFR-CC-04]).
    let rollup = &status.warm_rollup;
    assert_eq!(rollup.warming, None);
    assert_eq!(
        rollup.warm + rollup.warming.unwrap_or(0) + rollup.deferred + rollup.degraded,
        rollup.members,
        "no member may fall out of the partition"
    );
    let value = serde_json::to_value(rollup).expect("rollup json");
    assert!(
        value.get("warming").is_none(),
        "the key must be absent, not null or 0: {value}"
    );
}

/// [FR-WS-17] AC5: a workspace with **no** outcome record produces `workspace
/// status` output identical to the pre-[CR-102] behaviour.
///
/// Asserted as byte equality between the same workspace read with the record
/// absent and read again after a record is written and removed, so the claim is
/// "identical output", not merely "the same labels".
#[test]
fn a_workspace_with_no_record_is_byte_identical_to_before() {
    let (_dir, root, members) = workspace(&[("api", true), ("web", false)]);

    let before = serde_json::to_value(status_from_a_cold_start(&root, &members)).expect("json");

    record_outcomes(&summary(&[(&members[1], Some("index failed"))]));
    assert!(
        read_outcomes(&root).members.contains_key("web"),
        "the record must actually have been written, or this proves nothing"
    );
    // The differential: while the record IS there the payload must DIFFER.
    // Without it, a build that never read the sidecar would pass the equality
    // below just as happily.
    let with_record = serde_json::to_value(status_from_a_cold_start(&root, &members)).expect("json");
    assert_ne!(
        with_record, before,
        "the record must actually be consumed, or the equality below proves nothing"
    );

    std::fs::remove_file(root.join(OUTCOME_FILENAME)).expect("remove the record");

    let after = serde_json::to_value(status_from_a_cold_start(&root, &members)).expect("json");

    assert_eq!(before, after, "a workspace with no record reads exactly as before");
    assert_eq!(before["warm_rollup"]["warm"], 1);
    assert_eq!(before["warm_rollup"]["deferred"], 1);
    assert!(before["warm_rollup"].get("warming").is_none());
}

/// [FR-WS-17] AC6: a malformed, truncated or unreadable record degrades to
/// index-presence derivation and **never fails the command** — asserted against
/// the whole read-model, whose output must equal the no-record output exactly.
#[test]
fn a_corrupt_record_degrades_the_read_model_without_failing_it() {
    let (_dir, root, members) = workspace(&[("api", true), ("web", false)]);
    let healthy = serde_json::to_value(status_from_a_cold_start(&root, &members)).expect("json");

    let good = serde_json::to_string(&WarmOutcomes {
        version: 1,
        members: [(
            "web".to_string(),
            WarmOutcome::Failed {
                reason: "index failed".to_string(),
            },
        )]
        .into_iter()
        .collect(),
    })
    .expect("json");

    // The `good` record is in the table as a CONTROL: it must NOT read as
    // healthy. Without it, every row below is satisfied by a build that ignores
    // the sidecar entirely, and the whole test is vacuous.
    for (case, bytes) in [
        ("a well-formed record (control)", good.clone()),
        ("malformed", "{ not json".to_string()),
        ("truncated", good[..good.len() / 2].to_string()),
        ("empty", String::new()),
        ("a directory where the file belongs", String::new()),
    ] {
        let path = root.join(OUTCOME_FILENAME);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&path);
        if case == "a directory where the file belongs" {
            // Unreadable in a way no permission bit can fake on a root CI
            // runner: `read(2)` on a directory cannot succeed for any user.
            std::fs::create_dir(&path).expect("obstruct the sidecar");
        } else {
            std::fs::write(&path, &bytes).expect("corrupt sidecar");
        }

        let status = serde_json::to_value(status_from_a_cold_start(&root, &members)).expect("json");

        if case == "a well-formed record (control)" {
            assert_ne!(
                status, healthy,
                "the control must be consumed — otherwise every row below is vacuous"
            );
            continue;
        }
        assert_eq!(status, healthy, "{case} must read exactly as no record at all");
    }
}

/// [FR-WS-17] AC4 / [FR-WS-14]'s no-member-store property, asserted where it can
/// actually regress: producing the record for a real member set writes nothing
/// inside any member's `.logos`.
///
/// The un-indexed members have no `.logos` at all, so the assertion is that the
/// directory is still absent; the indexed one has a real store, so the
/// assertion is that its contents are byte-identical before and after.
#[test]
fn recording_touches_no_member_store() {
    let (_dir, _root, members) = workspace(&[("api", true), ("web", false)]);

    let store = members[0].root.join(".logos");
    let before = listing(&store);
    assert!(!before.is_empty(), "the indexed member must have a real store");
    assert!(!members[1].root.join(".logos").exists());

    record_outcomes(&summary(&[
        (&members[0], Some("index failed")),
        (&members[1], None),
    ]));

    assert_eq!(listing(&store), before, "the member store must be untouched");
    assert!(
        !members[1].root.join(".logos").exists(),
        "no member store may be created by recording an outcome"
    );
}

/// Every entry under `dir`, with its byte length — enough to catch a file
/// added, removed or rewritten.
fn listing(dir: &Path) -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut listing: Vec<(String, u64)> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let len = entry.metadata().map(|m| m.len()).unwrap_or_default();
            (entry.file_name().to_string_lossy().into_owned(), len)
        })
        .collect();
    listing.sort();
    listing
}
