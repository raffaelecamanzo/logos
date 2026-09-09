//! **S-376 — the post-[CR-120] coverage baseline, measured over the reference
//! workspace** ([CR-120] §4.4 and its AC "a post-change measured baseline … is
//! recorded as a durable artifact", [FR-WS-05], [NFR-CC-04]).
//!
//! [CR-120] retires `bound_ratio` and makes the headline a **count of resolved
//! cross-service edges** plus the **egress resolution rate**. Every later delta —
//! [CR-121]'s whole programme begins by moving these numbers — is stated against
//! this artifact, so it exists as a committed file rather than as a figure in a
//! commit message.
//!
//! The harness is [`coverage_intake_split`]'s, deliberately: the same
//! [`discover`] → [`EngineRegistry`] in [`RegistryMode::Lazy`] →
//! [`cross_service_coverage`] construction the `logos workspace status` command
//! uses, so the figures recorded here are the figures the shipped command prints.
//!
//! # Read-only, and what that costs this baseline
//!
//! It opens each member's existing store and reads it. It indexes nothing and
//! enrols nothing: re-enrolling the reference workspace is an 84-member operation
//! that sprint 66's risk register makes a **human-gated step at sprint review**.
//!
//! That has a consequence this file states rather than buries. **The store was
//! cold-indexed on 2026-09-08 with logos 1.4.7, before [S-374] merged**, so it
//! does not contain [S-374]'s ~115 HTTP client-call refusal rows — those are
//! written at *index* time. So the artifact is a post-change **payload shape** over
//! a pre-[S-374] **index generation**, and it is labelled that way in its own
//! `generation` block so no reader can mistake it for a post-S-374 measurement.
//! The refresh procedure that closes the gap is in the artifact itself.
//!
//! # Recorded finding (2026-09-09, `~/source/pec-services`, 84 members)
//!
//! ```text
//! resolved_cross_service_edges   0
//! egress_resolution              0.000 (0 of 54 egress sites resolved)
//! spec_conformance_ratio         0.287 (81 of 282 measured; 647 excluded)
//! ```
//!
//! **Zero.** Not one cross-service edge in the estate is resolved from a captured
//! call site, over 54 captured egress sites — while the retired `bound_ratio`
//! reported `0.287` on the same data and the pooled `bound` reads 81. That
//! contrast is [CR-120]'s entire case, and it is now the number the command
//! prints first.
//!
//! The full record — the per-story delta attribution, the read-only proof and the
//! human-gated refresh procedure — is the durable artifact
//! `coverage_headline_baseline/headline_baseline_finding.txt`, beside the payload
//! capture `coverage_headline_baseline/ws-coverage-2026-09-09-post-cr120.json`.
//!
//! # Skips when unconfigured, and the limit of that
//!
//! Set `LOGOS_REF_WORKSPACE=<path>` to run it (`~` is expanded), the same contract
//! the S-355/S-365/S-374/S-377 measurements read. With no corpus configured it
//! prints a `SKIPPED:` line and passes — it must not fail a machine with no
//! corpus. Be exact about how weak that signal is: libtest captures `eprintln!`
//! for a PASSING test, so a default `cargo test` run shows only `... ok`. **A green
//! run here does not establish that anything was measured**; the test name carries
//! `when_one_is_configured` for that reason.
//!
//! `corpus_root` is duplicated from `coverage_intake_split.rs` /
//! `broker_topic_corpus.rs` / `operand_resolvability.rs`, and this file is the
//! fourth copy — so it is the one that owes an honest reason rather than an
//! inherited one.
//!
//! **The inherited reason does not survive examination, and is not repeated.**
//! Those files argue that sharing would couple four separately-recorded published
//! figures to one file's edits. It would not: `corpus_root` reads an environment
//! variable, expands `~` and asserts the path is a directory. It performs no
//! measurement, and each harness asserts its own figures against its own artifact,
//! so a shared locator cannot move a recorded number — it can only change whether
//! a corpus is *found*, and that failure is loud (a panic) or a printed skip.
//!
//! What is true is the plainer thing: four byte-identical copies can drift, and a
//! fix applied to one (a `$HOME` fallback, symlink resolution) leaves three wrong
//! with nothing to catch it. A `tests/common/mod.rs` is the idiomatic answer and
//! costs almost nothing. It is **not** done here only because this story is a
//! payload rename, and lifting a helper out of three unrelated measurement
//! harnesses touches files it has no other business in — the kind of drive-by that
//! makes a diff harder to review than the thing it fixes. Recorded as accepted
//! debt with its real cost named, which is what the inherited rationale was not.
//!
//! [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
//! [CR-121]: ../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [S-374]: ../../docs/planning/journal.md#s-374-the-http-client-call-arm-records-its-refusals

use std::path::{Path, PathBuf};

use logos_core::federation::{
    cross_service_coverage, discover, CrossServiceCoverage, EngineRegistry, RegistryMode,
    SpecConformanceReading,
};
use logos_core::Engine;

/// The committed post-change capture, relative to this crate's manifest.
const ARTIFACT: &str = "tests/coverage_headline_baseline/ws-coverage-2026-09-09-post-cr120.json";

/// The reference workspace, or `None` when none is configured.
///
/// A variable that is **set but does not resolve to a directory** panics rather
/// than skipping: the two cases are indistinguishable to a reader of a green test
/// run, and a typo'd corpus path would otherwise report success while measuring
/// nothing.
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

/// The headline block of a coverage payload, as the artifact records it.
///
/// Deliberately the *summary* fields only, and not the 929 classified rows. The
/// rows are already on record in the pre-change capture
/// `logos-docs/ws-status-2026-09-08-v1.4.7.json` over the **same index
/// generation**, so committing a second copy of them here would add 400 KB to
/// this repository to record nothing new — while the summary block is exactly what
/// changed and exactly what every later delta is stated against.
fn headline(cov: &CrossServiceCoverage) -> serde_json::Value {
    serde_json::json!({
        "references": cov.references.len(),
        "bound": cov.bound,
        "ambiguous": cov.ambiguous,
        "unbound": cov.unbound,
        "no_provider_in_workspace": cov.no_provider_in_workspace,
        "by_intake": cov.by_intake,
        "resolved_cross_service_edges": cov.resolved_cross_service_edges,
        "egress_resolution": cov.egress_resolution,
        "egress_resolution_measured": cov.egress_resolution_measured,
        "resolved_edges_summary": cov.resolved_edges_summary,
        "spec_conformance_ratio": cov.spec_conformance_ratio,
        "spec_conformance_measured": cov.spec_conformance_measured,
        "spec_conformance_summary": cov.spec_conformance_summary,
        "members_read": cov.members_read,
        "members_total": cov.members_total,
        "covers_all_members": cov.covers_all_members,
    })
}

/// The committed artifact's recorded measurement block.
fn artifact() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(ARTIFACT);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the durable baseline {} must be readable: {e}", path.display()));
    serde_json::from_str(&text).expect("the durable baseline is valid JSON")
}

/// **The artifact is committed, well-formed and honestly labelled** — asserted
/// with **no corpus**, so it holds in CI and in any fresh clone.
///
/// A baseline nobody can read is not a baseline, and one that does not say which
/// index generation produced it is worse than none: it would be quoted as a
/// post-[S-374] figure, which it is not. Both are checked here rather than left to
/// a reviewer's reading.
///
/// [S-374]: ../../docs/planning/journal.md#s-374-the-http-client-call-arm-records-its-refusals
#[test]
fn the_durable_baseline_is_committed_and_states_its_index_generation() {
    let a = artifact();

    // It is labelled — and the label is the load-bearing part.
    let gen = &a["generation"];
    assert_eq!(
        gen["index_built_by"], "logos 1.4.7",
        "the artifact must name the binary that INDEXED the store, not the one that \
         read it: {gen}"
    );
    assert_eq!(gen["payload_shape"], "post-CR-120 (S-376)", "{gen}");
    assert_eq!(
        gen["contains_s374_refusal_rows"], false,
        "the store predates S-374, so the artifact must say so — a reader who takes \
         this for a post-S-374 measurement will attribute a later rise in the \
         invocation unbound column to the wrong cause: {gen}"
    );
    assert!(
        gen["refresh_procedure"].as_str().is_some_and(|s| s.contains("logos init --workspace")),
        "the artifact must carry the exact refresh procedure, so the human-gated \
         re-index is one step rather than a rediscovery: {gen}"
    );

    // The measurement block carries the headline in the post-change vocabulary.
    let m = &a["measurement"];
    for key in [
        "resolved_cross_service_edges",
        "egress_resolution",
        "egress_resolution_measured",
        "resolved_edges_summary",
        "spec_conformance_ratio",
        "spec_conformance_measured",
        "spec_conformance_summary",
        "by_intake",
    ] {
        assert!(m.get(key).is_some(), "the baseline must record `{key}`: {m}");
    }
    assert!(
        m.get("bound_ratio").is_none(),
        "…and none of the retired spellings, or the baseline would be quoted in the \
         vocabulary it exists to replace: {m}"
    );

    // The per-story attribution [CR-120]'s Testing & Verification section requires,
    // one entry per contributing story, each stating whether its delta is OBSERVED
    // here or PENDING the re-index. An entry that claimed neither would be the kind
    // of unattributable figure this artifact exists to prevent.
    let deltas = a["deltas"].as_object().expect("per-story delta attribution");
    for story in ["S-374", "S-375", "S-377"] {
        let d = deltas.get(story).unwrap_or_else(|| panic!("no delta recorded for {story}"));
        let status = d["status"].as_str().unwrap_or_default();
        assert!(
            status == "observed" || status == "pending-reindex",
            "{story}'s delta must be either observed or explicitly pending the \
             human-gated re-index, never silently omitted: {d}"
        );
        assert!(
            d["effect"].as_str().is_some_and(|s| !s.is_empty()),
            "{story} must state its effect on these figures, and a pending one must \
             state the expected direction and magnitude with its reason: {d}"
        );
    }

    // **The recorded measurement is internally consistent.** Corpus-free, so it
    // holds in CI — and it is the half a reader is most likely to break, because
    // the `deltas` and `generation` blocks beside it are hand-written and the
    // measurement is the one block that must never be. A hand-edited figure that
    // no longer reconciles with its own siblings is a baseline that would send a
    // sprint-review re-index chasing a delta that was never measured.
    let num = |k: &str| m[k].as_u64().unwrap_or_else(|| panic!("`{k}` is a number: {m}"));
    assert_eq!(
        num("bound") + num("ambiguous") + num("unbound"),
        num("spec_conformance_measured"),
        "the three counted buckets are the spec-conformance denominator: {m}"
    );
    assert_eq!(
        num("bound") + num("ambiguous") + num("unbound") + num("no_provider_in_workspace"),
        m["references"].as_u64().expect("`references` is a count"),
        "…and all four partition the reference total: {m}"
    );
    let inv = &m["by_intake"]["invocation"];
    let inv_num = |k: &str| inv[k].as_u64().unwrap_or_else(|| panic!("invocation.{k}: {inv}"));
    assert_eq!(
        inv_num("bound") + inv_num("ambiguous") + inv_num("unbound"),
        num("egress_resolution_measured"),
        "the egress denominator is the INVOCATION population's three counted \
         buckets — not the pooled ones, and not including no-provider: {m}"
    );
    let line = m["resolved_edges_summary"].as_str().expect("the composed line");
    let edges = num("resolved_cross_service_edges");
    let noun = if edges == 1 { "edge" } else { "edges" };
    assert!(
        line.starts_with(&format!("{edges} resolved cross-service {noun};")),
        "the recorded line must open with the recorded count (BR-51): {line:?}"
    );
    assert!(
        line.contains(&format!("of {} egress site", num("egress_resolution_measured"))),
        "…and name the recorded denominator: {line:?}"
    );

    // The pre-change capture this baseline is stated against, named by path so the
    // pair can be found together.
    assert!(
        a["reconciled_against"]
            .as_str()
            .is_some_and(|s| s.contains("ws-status-2026-09-08-v1.4.7.json")),
        "the baseline must name the pre-change capture it is compared with: {a}"
    );

    // And the retired vocabulary still READS, through the one-release alias — the
    // capability that makes the pre-change capture comparable at all (AC1).
    let pre_change_shaped = serde_json::json!({
        "bound_ratio": m["spec_conformance_ratio"],
        "bound_ratio_measured": m["spec_conformance_measured"],
        "bound_ratio_summary": m["spec_conformance_summary"],
    });
    let reading: SpecConformanceReading = serde_json::from_value(pre_change_shaped)
        .expect("a pre-CR-120-shaped payload reads through the deprecated alias");
    assert_eq!(
        reading.spec_conformance_summary,
        m["spec_conformance_summary"].as_str().unwrap(),
        "and yields the same figures, which is what lets the 2026-09-08 capture be \
         compared with this one in a single operation"
    );
}

/// **The live reference workspace still produces the recorded baseline** — run
/// only when a corpus is configured.
///
/// The remedy for a red run here is to **record the new figures** — in the
/// artifact, in this file's docs and in the story's notes — never to bend the
/// classifier to reproduce the old ones. That distinction is the whole of
/// [CR-120]'s subject, so it is stated rather than left to a reader of a red test.
///
/// Set `LOGOS_BASELINE_WRITE=1` to rewrite the artifact's `measurement` block from
/// the live run instead of asserting against it. That is how a re-measurement is
/// recorded; it is opt-in because a harness that silently rewrote its own baseline
/// could never fail.
///
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
#[test]
fn the_reference_workspace_reproduces_the_recorded_baseline_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-376 post-CR-120 baseline measurement (see this file's module docs for the \
             recorded finding)."
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
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let cov = cross_service_coverage(&registry);
    let measured = headline(&cov);

    println!(
        "S-376 / CR-120 post-change baseline over {} ({} members read of {}, covers_all={}):\n\
         \x20 {}\n\
         \x20 {}",
        root.display(),
        cov.members_read,
        cov.members_total,
        cov.covers_all_members,
        cov.resolved_edges_summary,
        cov.spec_conformance_summary,
    );

    if std::env::var("LOGOS_BASELINE_WRITE").is_ok_and(|v| v == "1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(ARTIFACT);
        let mut a = artifact();
        a["measurement"] = measured;
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&a).unwrap()))
            .expect("the artifact is writable");
        println!("REWROTE {} — commit it, and update the finding beside it.", path.display());
        return;
    }

    assert_eq!(
        measured,
        artifact()["measurement"],
        "the reference workspace no longer reproduces the recorded post-CR-120 \
         baseline. If the corpus has been re-indexed or re-enrolled, RECORD the new \
         figures (`LOGOS_BASELINE_WRITE=1`) and restate the per-story deltas — do \
         not bend the classifier to reproduce the old ones."
    );
}
