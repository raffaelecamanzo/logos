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
//! `corpus_root` is duplicated from `broker_topic_corpus.rs` /
//! `operand_resolvability.rs` rather than shared, and the reason is **editorial,
//! not technical** — this file states the tradeoff those two leave implicit. A
//! `#[path]`-included support module would work (`logos-core/tests` already uses
//! that mechanism), and it would compile the module separately into each test
//! binary, so there is no shared build unit to couple. What it would couple is
//! three **published figures** to one file's edits: a change to the shared reader
//! would silently alter the corpus three separate recorded measurements were taken
//! over. Nineteen lines with no judgement in them is the cheaper side of that
//! trade. Revisit if a fourth measurement arrives.
//!
//! # Recorded finding (2026-09-09, `~/source/pec-services`, 84 members)
//!
//! ```text
//! references            929
//!                       bound  ambiguous  unbound  no-provider
//! contract-surface         81        146        1          646
//! invocation                0          0       54            1
//! headline                 81        146       55          647
//! 0.287 (81 of 282 measured; 647 excluded as no-provider-in-workspace)
//! ```
//!
//! **[CR-120] §6's criterion is met: 81 contract-surface / 0 invocation bound
//! rows.** Reconciled against the baseline sprint 66 §7 names for the purpose,
//! `logos-docs/ws-status-2026-09-08-v1.4.7.json`: all five figures above are
//! reproduced from it exactly, and it settles the 81/0 pair from the pre-change
//! artifact rather than from anything this change asserts — of its 929 rows, 81
//! carry `intake` and all 81 read `contract-surface`, while the other **848 carry
//! no `intake` at all**. Those 848 rows are what AC1 changes, and their absence
//! is [CR-120] §3.1's defect visible in a shipped payload.
//!
//! The invocation column's 55 rows are **not** S-374's ~115 HTTP client-call
//! refusals: those are written at *index* time and this store was cold-indexed on
//! 2026-09-08 with 1.4.7, before S-374 merged. They are the 54 broker
//! `topic-not-literal` refusals (S-370/[CR-117]) plus the one workspace-wide
//! `http-client-call` reference, which is bucketed `no-provider-in-workspace`
//! rather than `unbound`. Expect the invocation `unbound` column to rise by ~115
//! on the first re-index that carries S-374; `bound` is not expected to move,
//! because a refusal never binds.
//!
//! The full record, including the verified read-only proof, is the durable
//! artifact `coverage_intake_split/intake_split_finding.txt`.
//!
//! [CR-117]: ../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
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
/// 1. the `bound` count splits as **81 contract-surface / 0 invocation**;
/// 2. every row carries an intake, so the split is auditable from the rows rather
///    than taken on trust — the property AC1 adds and the one that makes (1)
///    reproducible by a reader with the same `--json`;
/// 3. the two populations sum to the headline counters, so the split cannot
///    under-report what it sits beside.
///
/// **The 81/0 pair is asserted, and a moved corpus fails this harness
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

    let cov = cross_service_coverage(&registry);
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
        cov.bound_ratio_summary,
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
        (81, 0),
        "CR-120's criterion: 81 contract-surface / 0 invocation bound rows. Measured \
         {} / {} over {} references. If the corpus has been re-indexed or re-enrolled \
         since 2026-09-08, RECORD the measured figure — do not bend the classifier to \
         reproduce this one.",
        cs.bound,
        inv.bound,
        cov.references.len()
    );
}
