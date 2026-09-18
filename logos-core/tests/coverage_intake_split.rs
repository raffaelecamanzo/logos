//! **S-377 — the intake split, measured over the reference workspace**
//! ([CR-120] §6, [FR-WS-05], [NFR-CC-04]).
//!
//! [CR-120]'s acceptance criterion is a number, not a shape: *"splitting the
//! reference workspace's rows by intake shows **81 contract-surface** and **0
//! invocation** bound rows"*. Its CRA-02 records that as measured on 2026-09-08 —
//! but measured by reading the ledger and the reference count *around* the
//! coverage tier, because no payload carried the split. This harness measures it
//! **through the tier itself**, over the same registry construction the
//! `logos workspace status` command uses ([`discover`] → [`EngineRegistry`] in
//! [`RegistryMode::Lazy`] → [`cross_service_coverage`]), so the figure a reader
//! reconciles against is the figure the shipped command prints.
//!
//! # Read-only, and why that matters here
//!
//! It opens each member's existing store and reads its contract surface and
//! invocation ledger. It indexes nothing and enrols nothing: re-enrolling the
//! reference workspace is a human-gated sprint-review step in this sprint's risk
//! register, and a harness that quietly re-indexed would make its own figure
//! unattributable — a changed number could be the change or the re-index.
//!
//! # Skips when unconfigured, and the limit of that
//!
//! Set `LOGOS_REF_WORKSPACE=<path>` to run it (`~` is expanded), following the
//! same contract as the S-355/S-365/S-374 measurements. With no corpus configured
//! the test prints a `SKIPPED:` line and passes — it must not fail a machine that
//! has no corpus. Be exact about how weak that signal is: libtest captures
//! `eprintln!` for a test that PASSES, so a default `cargo test` run shows only
//! `... ok` and the `SKIPPED:` line is invisible. **A green run here does not
//! establish that anything was measured** — look for the printed figures, or run
//! with `--nocapture`. The test name carries `when_one_is_configured` for exactly
//! that reason.
//!
//! `corpus_root` is duplicated rather than shared. This file used to argue that
//! sharing would couple the **published figures** of the harnesses that copy it,
//! and to arm a revisit "if a fourth measurement arrives". **Both halves were
//! wrong and are withdrawn**, and the correction is left here rather than silently
//! deleted because the argument was inherited from this file by the copies that
//! followed it:
//!
//! * The coupling argument does not survive examination —
//!   `coverage_headline_baseline.rs`'s module docs give the rebuttal in full:
//!   `corpus_root` reads an environment variable, expands `~` and asserts the path
//!   is a directory. It performs no measurement, so a shared locator cannot move a
//!   recorded number; it can only change whether a corpus is *found*, and that
//!   failure is loud.
//! * The trigger fired long ago and nobody noticed, which is the more useful
//!   lesson. There are now **six** byte-identical copies —
//!   `operand_resolvability.rs`, `config_corpus.rs`, `broker_topic_corpus.rs`,
//!   this file, `coverage_headline_baseline.rs` and `config_bound_admission.rs` —
//!   so a count-based trigger was the wrong instrument for a debt nobody owns.
//!
//! The standing verdict is `coverage_headline_baseline.rs`'s: the real cost is
//! drift across N copies, a `tests/common/mod.rs` is the idiomatic answer and costs
//! almost nothing, and it is **accepted debt** until a story has legitimate
//! business in those files. No count is armed here any more.
//!
//! # Recorded finding (2026-09-18, `~/source/pec-services`, 84 members)
//!
//! Re-recorded by S-420 T2 over the estate **re-enrolled at logos 1.4.12 on
//! 2026-09-17**. The two previous generations are kept beside it, because the
//! movement is the finding:
//!
//! ```text
//! references           1036                       (was 1034, and 1060 before that)
//!                       bound  ambiguous  unbound  no-provider
//! contract-surface         81        146        1          646   (unchanged, all three)
//! invocation               45         29       43           45   (was 15/23/79/43)
//! headline                126        175       44          691   (was 96/169/80/689)
//! 0.365 (126 of 345 measured; 691 excluded as no-provider-in-workspace)
//! ```
//!
//! **The invocation column's `bound` 15 → 45 is two movements and is never
//! summed.** **+27** are broker publishes, a population this payload did not carry
//! at all before: the committed-topic-value resolution of S-409 / S-410 reaching a
//! store-backed readout for the first time. **+3** are HTTP client calls, S-399's
//! and S-405's accessor widenings arriving with the re-enrolment. Read as one
//! figure it says outbound HTTP resolution tripled; it did not — that arm moved
//! 15 → 18.
//!
//! **S-420 moves nothing in this file, and that is measured rather than assumed.**
//! The same estate was read on 2026-09-18 with and without its bridge arm and the
//! two coverage payloads are byte-identical on every counter here. What it moves
//! is `coverage.bridge_invocation_edges` (33 → 51), which is not a figure this
//! harness takes; its single home is `config_bound_admission.rs`.
//!
//! **No floor is asserted on any figure in this file.**
//!
//! The 2026-09-13 reading, and every attribution stated against it below, is left
//! as the dated record it is.
//!
//! **[CR-120] §6's criterion was met, on the generation it was written over: 81
//! contract-surface / 0 invocation bound rows, measured 2026-09-09 over a store
//! cold-indexed with 1.4.7.** That measurement reconciled cell for cell against
//! the baseline sprint 66 §7 names for the purpose,
//! `logos-docs/ws-status-2026-09-08-v1.4.7.json`, whose 929 rows carried `intake`
//! on 81 and none at all on the other 848 — [CR-120] §3.1's defect visible in a
//! shipped payload, and what AC1 changed.
//!
//! The figures above are a **later index generation** of the same estate, not a
//! retraction of that. [S-397] T2 re-indexed all 84 members on 2026-09-13 from a
//! binary carrying [S-374] and [S-397] T1, and [S-398] T2 re-indexed them again
//! the same day from a binary carrying the whole of Sprint 69 Iteration 1. The
//! contract-surface column is byte-identical across **all four** generations;
//! every cell that has ever moved is in the invocation column.
//!
//! Against the 1.4.7 generation it moved for two reasons worth keeping apart:
//!
//! * **`unbound` 54 → 141.** The 54 were the broker `topic-not-literal` refusals
//!   (S-370/[CR-117]) that every generation carries. The 87 added are [S-374]'s
//!   HTTP client-call refusals, which are written at *index* time and which the
//!   1.4.7 store predated. This doc used to predict "~115 on the first re-index";
//!   the re-index wrote **131** client-call rows, of which 87 landed here.
//! * **`bound` 0 → 5, `ambiguous` 0 → 9, `no-provider` 1 → 31.** The other 44 of
//!   those 131 rows carry `config-bound` provenance — [S-397] T1's accessor hop.
//!   This doc also used to say "`bound` is not expected to move, because a refusal
//!   never binds"; that reasoning was sound and its conclusion was still wrong,
//!   because a story landed between the prediction and the re-index that turned 44
//!   of the refusals into admissions. The admitted figure and its shortfall
//!   against [S-397] AC2's floor are `config_bound_admission.rs`'s subject, not
//!   this file's.
//!
//! Against the [S-397] generation — the 1060/5/9/141/31 column above — it moved
//! for two more, and those two must NOT be summed either:
//!
//! * **[S-398] T1, an admission.** `config-bound` rows rose 44 → 81, exactly +37,
//!   as the qualified-receiver accessor shape became reachable. Those 37 rows left
//!   the `base-url-runtime` refusals — 37 fewer of them in exactly the four Java
//!   members that gained 37 `config-bound` rows, per member — and landed across
//!   `bound` (+10), `ambiguous` (+14),
//!   `no-provider-in-workspace` (+12) and one `path-not-composed` row (+1). The
//!   invocation `unbound` column therefore reads 141 → 79: the 54
//!   `topic-not-literal` broker refusals are untouched, 24 `base-url-runtime`
//!   remain, and the new `path-not-composed` row is the +1.
//! * **[S-402] T1, a population correction.** `references` fell 1060 → 1034 and
//!   the whole invocation intake fell 186 → 160, all 26 of them in
//!   `hermodr-mirror`, the estate's one Go member, whose captured client-call
//!   sites went 37 → 11 when the candidacy gate became receiver-grained. All 26
//!   were keyless refusals, so nothing that bound stopped binding — but this is a
//!   smaller population, not better coverage, and the two halves are reported
//!   separately for exactly that reason.
//!
//! The full record, including the verified read-only proof, is the durable
//! artifact `coverage_intake_split/intake_split_finding.txt`.
//!
//! [CR-117]: ../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
//! [S-374]: ../../docs/planning/journal.md#s-374-the-http-client-call-arm-records-its-refusals
//! [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm
//! [S-398]: ../../docs/planning/journal.md#s-398-the-accessor-hop-reaches-a-qualified-receiver
//! [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained
//!
//! [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::path::PathBuf;

use logos_core::federation::{
    cross_service_coverage, discover, BridgeIntake, EngineRegistry, RegistryMode,
};
use logos_core::Engine;

/// The reference workspace, or `None` when none is configured — the same
/// `LOGOS_REF_WORKSPACE` contract the S-355/S-365/S-374 measurements read.
///
/// A variable that is **set but does not resolve to a directory** panics rather
/// than skipping: the two cases are indistinguishable to a reader of a green test
/// run, and a typo'd or un-checked-out corpus path would otherwise report success
/// while measuring nothing.
fn corpus_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(&home).join(rest),
        None if raw == "~" => PathBuf::from(&home),
        None => PathBuf::from(&raw),
    };
    assert!(
        expanded.is_dir(),
        "LOGOS_REF_WORKSPACE={raw} does not resolve to a directory (expanded: {}) — \
         refusing to report a green run that measured nothing",
        expanded.display(),
    );
    Some(expanded)
}

/// The split, measured through the shipped read-model.
///
/// Three things are checked, and only the first is [CR-120]'s headline:
///
/// 1. the `bound` count splits as **81 contract-surface / 15 invocation**;
/// 2. every row carries an intake, so the split is auditable from the rows rather
///    than taken on trust — the property AC1 adds and the one that makes (1)
///    reproducible by a reader with the same `--json`;
/// 3. the two populations sum to the headline counters, so the split cannot
///    under-report what it sits beside.
///
/// **The 81/45 pair is asserted, and a moved corpus fails this harness
/// deliberately.** Its failure message prints the measured figure and the split
/// beside it, because the criterion's number is a recorded measurement of a
/// specific workspace at a specific commit: the remedy for a red run here is to
/// **record the new figure** — in this doc, in the durable artifact, and in the
/// story's notes — never to bend the classifier to reproduce the old one. That
/// distinction is the whole of [CR-120]'s subject, so it is stated rather than
/// left to a reader of a red test.
///
/// (2) and (3) are properties of the code rather than of the corpus, so they hold
/// for any workspace — but they are asserted only *when this harness runs*, which
/// needs a corpus. Corpus-free, the same two properties are guarded by
/// `federation::coverage::tests::the_classification_counts_split_by_intake_and_sum_to_the_headline`.
#[test]
fn measure_the_intake_split_over_the_reference_workspace_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-377 intake-split measurement (see this file's module docs for the recorded \
             finding)."
        );
        return;
    };

    let federation = discover(&root)
        .expect("the workspace manifest parses")
        .unwrap_or_else(|| {
            panic!(
                "LOGOS_REF_WORKSPACE={} is not a Logos workspace: no logos.workspace.toml \
                 found up-tree. Enrol it with `logos init --workspace --yes` first — this \
                 harness deliberately does not.",
                root.display()
            )
        });
    let members_total = federation.members.len();
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);

    let cov = cross_service_coverage(&registry.answer());
    let cs = cov.by_intake.contract_surface;
    let inv = cov.by_intake.invocation;

    println!(
        "S-377 / CR-120 intake split over {} ({} members declared, {} read, covers_all={}):\n\
         \x20 references            {}\n\
         \x20 contract-surface      bound {:>5}  ambiguous {:>5}  unbound {:>5}  no-provider {:>5}\n\
         \x20 invocation            bound {:>5}  ambiguous {:>5}  unbound {:>5}  no-provider {:>5}\n\
         \x20 headline              bound {:>5}  ambiguous {:>5}  unbound {:>5}  no-provider {:>5}\n\
         \x20 {}",
        root.display(),
        members_total,
        cov.members_read,
        cov.covers_all_members,
        cov.references.len(),
        cs.bound,
        cs.ambiguous,
        cs.unbound,
        cs.no_provider_in_workspace,
        inv.bound,
        inv.ambiguous,
        inv.unbound,
        inv.no_provider_in_workspace,
        cov.bound,
        cov.ambiguous,
        cov.unbound,
        cov.no_provider_in_workspace,
        cov.spec_conformance_summary,
    );

    // (2) Every row carries its intake, so a reader can rebuild the split from
    // `references` — which is what makes the figures above auditable rather than
    // asserted. Rebuilt here the way a `--json` consumer would.
    let mut by_row = (0u64, 0u64);
    for row in &cov.references {
        match row.intake {
            BridgeIntake::ContractSurface => by_row.0 += 1,
            BridgeIntake::Invocation => by_row.1 += 1,
        }
    }
    let cs_total = cs.bound + cs.ambiguous + cs.unbound + cs.no_provider_in_workspace;
    let inv_total = inv.bound + inv.ambiguous + inv.unbound + inv.no_provider_in_workspace;
    assert_eq!(
        by_row,
        (cs_total, inv_total),
        "the split must be recomputable from the rows' own `intake`"
    );

    // (3) The split reconciles with the headline.
    assert_eq!(cs.bound + inv.bound, cov.bound, "bound");
    assert_eq!(cs.ambiguous + inv.ambiguous, cov.ambiguous, "ambiguous");
    assert_eq!(cs.unbound + inv.unbound, cov.unbound, "unbound");
    assert_eq!(
        cs.no_provider_in_workspace + inv.no_provider_in_workspace,
        cov.no_provider_in_workspace,
        "no-provider-in-workspace"
    );

    // (1) The criterion's own figures. Reported with the measured value in the
    // message, so a moved corpus reads as a moved corpus.
    assert_eq!(
        (cs.bound, inv.bound),
        (81, 45),
        "the recorded split is 81 contract-surface / 45 invocation bound rows, measured \
         2026-09-18 by S-420 T2 over the estate re-enrolled at logos 1.4.12 (it was \
         81 / 15 over the merged-Sprint-69-Iteration-1 index, 81 / 5 over the S-397 \
         generation, and 81 / 0 over the 1.4.7 one). Measured {} / {} over {} \
         references. If the corpus has been re-indexed or re-enrolled again, RECORD the \
         measured figure — here, and in the `by_intake` block in docs/howto/commands.md, \
         which restates this split in prose no test reads — do not bend the classifier to \
         reproduce this one. AND DECOMPOSE IT BY ARM before calling it a gain: the 45 is \
         18 HTTP client calls and 27 broker publishes, and the 15 it succeeds was HTTP \
         alone, so a reader who takes 15 -> 45 for outbound HTTP resolution tripling is \
         wrong by 27 of the 30. CR-120's own criterion was 81 / 0 and it was met on the \
         1.4.7 generation; the invocation column is the accessor hop (S-397 T1, then \
         S-398 T1, then S-399/S-405 and S-409/S-410) and is not a retraction of it.",
        cs.bound,
        inv.bound,
        cov.references.len()
    );
}
