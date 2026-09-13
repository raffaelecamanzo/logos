//! **S-397 T2 — what the SHIPPED pipeline admits on the reference estate**
//! ([FR-WS-19], [NFR-CC-04], [CR-122] §6).
//!
//! [S-397] T1 wired the accessor capture hop. Its acceptance criterion 2 is a
//! figure, and it names the surface the figure must be read from: *"on the
//! reference workspace the shipped pipeline admits at least 79 accessor-based
//! client-call sites carrying `config-bound` provenance, measured through
//! `logos workspace status --json` and **not** through the harness, against the 0
//! it admits today"*.
//!
//! That distinction is the whole reason this file is not part of the
//! `operand_resolvability` harness. The harness resolves the estate's accessors
//! with its own tree-sitter reader and its own filesystem corpus walk; it proves
//! what is *resolvable*. This file reads what the **product** emits, through the
//! same call the CLI makes — `federation::discover` →
//! `EngineRegistry<Engine>(RegistryMode::Lazy)` → [`workspace_status`] → the serde
//! serialization `Output::print` applies for `--json` — and counts rows in the
//! resulting payload. A figure the harness proves is not a figure the product
//! emits, and on this estate the two differ by a factor of nearly two.
//!
//! # The measured finding (2026-09-13, `~/source/pec-services`, 84 members)
//!
//! **The criterion is NOT met. The shipped pipeline admits 44, against a floor of
//! 79.** Recorded rather than rounded, and the shortfall is diagnosed rather than
//! absorbed:
//!
//! ```text
//! rows carrying `config-bound` provenance          44
//!   of which bound                                  5
//!   of which ambiguous                              9
//!   of which no-provider-in-workspace              30
//! rows carrying `config-unresolved` provenance      0
//! the accessor denominator (S-382 reading)         108
//! the harness's upper bound (79 agreed + 2 divergent) 81
//! the criterion's floor                            79
//! the figure the same payload carried on 1.4.9      0
//! ```
//!
//! So: **44 of 108** accessor-denominator sites, **44 of 81** of what the harness
//! proves resolvable, against a floor of **79**. The move from 0 is real and it is
//! the whole of what T1 bought; the floor is not met.
//!
//! # The shortfall is one mechanism, and T1 predicted it
//!
//! Diffing the harness census against the members' invocation ledgers site by
//! site, the gap is **37 sites and every one of them spells `this.`**:
//! `this.mailboxConfigurationApi.getUriGetMailbox()`. `extract::config::accessor`
//! refuses a **qualified receiver** by design — [NFR-RA-05], never guess — where
//! the harness's `operand_name` trims the expression to its last segment and
//! resolves it. T1's own implementation notes record this ceiling in as many
//! words: *"the census can resolve a site the product will not"*. The 79 the
//! criterion was written against is a census figure, and 37 of those 79 sites are
//! only reachable by the trim.
//!
//! Arithmetic, for a reader who wants to reconstruct it: the harness resolves 79
//! agreed + 2 divergent = 81 Java sites. 37 of the 79 agreed are `this.`-qualified
//! and the product refuses them, leaving 42; the 2 divergent sites are captured
//! (their receiver is unqualified), giving **44**. No site is lost anywhere else,
//! and nothing is lost in the coverage tier: the members' ledgers hold exactly 44
//! `http-client-call` rows carrying a `${…}` target, and all 44 reach the payload.
//!
//! # The `.properties` gap is NOT the cause, and that is measured too
//!
//! [S-397] AC5 requires the `.properties` admission residue stated as a numerator
//! over the accessor denominator, so the delivered figure is not read as full
//! coverage. It is measured in
//! `operand_resolvability/configuration_agreement.rs::properties_residue` and is
//! **0 of 108**: this estate commits every key its accessors read in yaml, and the
//! 3 `.properties` sources the corpus admits define none of them. The gap is still
//! open and still unowned — a residue of zero says what it costs today, not that
//! it is closed — but it accounts for none of the 44-against-79 shortfall.
//!
//! # VOID is not zero ([NFR-CC-04])
//!
//! A run that cannot see the estate corpus must report **VOID**, never 0 —
//! "measured zero" and "measured nothing" are different findings, and [S-392]'s
//! forwarding gate already tests that distinction. Three gates enforce it here,
//! each failing loudly rather than passing with a zero:
//!
//! * `LOGOS_REF_WORKSPACE` unset → the test **skips** and says so on stdout and
//!   stderr. A skip that reads as a pass is a false green, so it is named.
//! * `LOGOS_REF_WORKSPACE` set but not a directory, or not a Logos workspace →
//!   **panic**. A typo'd path would otherwise report a green run over nothing.
//! * The workspace opens but no member could be read, or the roster is empty →
//!   **panic with VOID**. This is the case the two above do not cover and the one
//!   [NFR-CC-04] is actually about: the payload is well-formed, every counter
//!   reads 0, and nothing whatsoever was measured.
//!
//! # Read-only
//!
//! It opens each member's existing store and reads it. It indexes nothing and
//! enrols nothing — the re-index this story's figure required was performed once,
//! by hand, before the run, and is recorded in the durable artifact beside this
//! file. A harness that quietly re-indexed would make its own figure
//! unattributable.
//!
//! # `corpus_root` is duplicated, and the fourth measurement has now arrived
//!
//! `coverage_intake_split.rs` states the trade and ends "Revisit if a fourth
//! measurement arrives". This is it, so it was revisited rather than copied
//! silently. The verdict is unchanged, and for this file it is stronger than for
//! the other three: AC2 requires this arm to be independent of the
//! `operand_resolvability` harness, and a `#[path]`-included reader shared with it
//! would couple the two populations' entry point — the one coupling this
//! measurement exists to avoid. Nineteen lines with no judgement in them remain
//! the cheaper side of the trade.
//!
//! The full record is the durable artifact
//! `config_bound_admission/config_bound_admission_finding.txt`.
//!
//! [CR-122]: ../../docs/requests/CR-122-the-configuration-substrate-reaches-the-product.md
//! [FR-WS-19]: ../../docs/specs/requirements/FR-WS-19.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [S-392]: ../../docs/planning/journal.md#s-392-measure-the-one-hop-parameter-forwarding-residue
//! [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm

use std::path::PathBuf;

use logos_core::federation::{discover, workspace_status, EngineRegistry, RegistryMode};
use logos_core::Engine;

/// The recorded verdict, reproduced by the run and pinned by its assertions.
const RECORDED_FINDING: &str = include_str!("config_bound_admission/config_bound_admission_finding.txt");

/// [S-397] AC2's floor: the criterion asks for **at least** this many.
const CRITERION_FLOOR: usize = 79;

/// What the shipped pipeline actually admits, measured 2026-09-13.
const RECORDED_ADMITTED: usize = 44;

/// The accessor denominator the figure is stated over — the S-382 reading's
/// production client-call population, measured on the same corpus by
/// `operand_resolvability/configuration_agreement.rs`.
const ACCESSOR_DENOMINATOR: usize = 108;

/// The three buckets the 44 admitted rows fall into, in the order
/// [`admission`] tallies them — a `BTreeMap`, so alphabetical by bucket name
/// rather than by count. `unbound` here is entirely `no-provider-in-workspace`.
const RECORDED_BUCKETS: [(&str, usize); 3] =
    [("ambiguous", 9), ("bound", 5), ("unbound", 30)];

/// The reference workspace, or `None` when none is configured — the same
/// `LOGOS_REF_WORKSPACE` contract the S-355/S-365/S-374/S-377 measurements read.
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

/// One reference row's three discriminators, read off the `--json` payload the
/// way a consumer reads them rather than off the Rust types.
///
/// Deliberately from `serde_json::Value` and not from `CrossServiceCoverage`:
/// AC2 names `logos workspace status --json`, and a count taken off the Rust
/// struct would pass even if the field never reached the wire. `provenance` is
/// `#[serde(flatten)]`ed onto the row, so its absence here is a serialization
/// defect, not a missing value — which is why it is read as an `Option` and
/// asserted present rather than defaulted.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    provenance: Option<String>,
    intake: String,
    relation: String,
    bucket: String,
}

/// What [S-397] AC2 asks the payload for.
#[derive(Debug, Default)]
struct Admission {
    /// Every reference row in the payload.
    references: usize,
    /// Rows carrying `config-bound` provenance — **the figure**.
    config_bound: usize,
    /// Rows carrying `config-unresolved` provenance: an accessor whose key the
    /// committed sources do not admit. Counted because zero of them is itself a
    /// finding — the shortfall is not keys going missing.
    config_unresolved: usize,
    /// The `config-bound` rows split by display bucket.
    by_bucket: Vec<(String, usize)>,
    /// Rows carrying no `provenance` key at all. Must be zero: the field is not
    /// optional ([FR-WS-19] AC6).
    missing_provenance: usize,
    /// Members the coverage walk actually read, and the roster size.
    members_read: u64,
    members_total: u64,
    covers_all: bool,
}

/// Read the `--json` payload and count what AC2 asks for.
fn admission(payload: &serde_json::Value) -> Admission {
    let coverage = &payload["coverage"];
    let references = coverage["references"].as_array().expect("`references` is an array");
    let mut out = Admission {
        references: references.len(),
        members_read: coverage["members_read"].as_u64().expect("`members_read` is a number"),
        members_total: coverage["members_total"].as_u64().expect("`members_total` is a number"),
        covers_all: coverage["covers_all_members"].as_bool().expect("`covers_all_members` is a bool"),
        ..Admission::default()
    };
    let mut buckets: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for reference in references {
        let row = Row {
            provenance: reference["provenance"].as_str().map(str::to_string),
            intake: reference["intake"].as_str().unwrap_or_default().to_string(),
            relation: reference["relation"].as_str().unwrap_or_default().to_string(),
            bucket: reference["bucket"].as_str().unwrap_or_default().to_string(),
        };
        match row.provenance.as_deref() {
            None => out.missing_provenance += 1,
            Some("config-bound") => {
                out.config_bound += 1;
                *buckets.entry(row.bucket.clone()).or_default() += 1;
                // Every admitted row is a captured call site on the HTTP key.
                // Asserted per row rather than in aggregate, so a payload that
                // admitted a contract-surface row under this provenance names
                // the row that did it.
                assert_eq!(
                    (row.intake.as_str(), row.relation.as_str()),
                    ("invocation", "route"),
                    "a `config-bound` row must be a captured HTTP client call; this one is \
                     {row:?}",
                );
            }
            Some("config-unresolved") => out.config_unresolved += 1,
            Some(_) => {}
        }
    }
    out.by_bucket = buckets.into_iter().collect();
    out
}

/// The measurement [S-397] AC2 names, through the surface it names.
///
/// The verdict is **asserted**, not printed. Without assertions a regression that
/// moved the figure — in either direction — would pass silently, and this figure
/// is what decides whether the qualified-receiver ceiling gets a story.
///
/// **The remedy for a red run here is to RECORD the new figure** — in this file's
/// docs, in the durable artifact beside it, and in the story's notes — never to
/// bend the pipeline to reproduce this one, and never to relax the floor. If the
/// figure has *risen* past 79 the criterion is met and that is the finding: say
/// so, and say what moved it.
#[test]
fn measure_config_bound_admission_over_the_reference_workspace_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        println!(
            "SKIPPED: LOGOS_REF_WORKSPACE is unset, so S-397 AC2 was NOT measured. This \
             test reports `ok` having measured NOTHING. Run `LOGOS_REF_WORKSPACE=<path> \
             cargo test -p logos-core --test config_bound_admission` to measure."
        );
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-397 AC2 admission measurement (see this file's module docs for the recorded \
             finding). This run measured nothing."
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

    // The `--json` payload, produced exactly as `Output::print` produces it for
    // `logos workspace status --json` (cli/src/main.rs): `serde_json` over the
    // value `federation::query::workspace_status` returns.
    let payload = serde_json::to_value(workspace_status(&registry)).expect("the payload serializes");
    let a = admission(&payload);

    println!(
        "S-397 AC2 — `config-bound` admission over {} ({} of {} members read, covers_all={}):\n\
         \x20 references                          {:>5}\n\
         \x20 carrying `config-bound`             {:>5}   <- the figure\n\
         \x20 carrying `config-unresolved`        {:>5}\n\
         \x20 rows with NO provenance             {:>5}\n\
         \x20 by bucket                           {:?}\n\
         \x20 criterion floor                     {:>5}\n\
         \x20 accessor denominator (S-382)        {:>5}\n\
         \x20 the same payload on 1.4.9               0",
        root.display(),
        a.members_read,
        a.members_total,
        a.covers_all,
        a.references,
        a.config_bound,
        a.config_unresolved,
        a.missing_provenance,
        a.by_bucket,
        CRITERION_FLOOR,
        ACCESSOR_DENOMINATOR,
    );
    println!("\n--- recorded finding ---\n{RECORDED_FINDING}");

    // ── VOID, not zero (NFR-CC-04) ──────────────────────────────────────
    //
    // The payload is well-formed whether or not a single member opened, and
    // every counter in it reads 0 either way. This is the case the corpus_root
    // guards above cannot reach, and the one a zero would be misreported as.
    assert!(
        a.members_total > 0 && a.members_read > 0,
        "VOID, not 0: the workspace at {} declares {} members and the coverage walk read \
         {}, so this run could not see the estate corpus at all. A zero admitted figure \
         from such a run is `measured nothing`, which is a different finding from \
         `measured zero` — do not record it as the latter (NFR-CC-04, the distinction \
         S-392's gate tests).",
        root.display(),
        a.members_total,
        a.members_read,
    );
    assert!(
        a.covers_all,
        "VOID, not 0: {} of {} members read, so the payload's counters are a partial \
         reading of the estate and no admitted figure taken over them is comparable to \
         the recorded one. Fix the unreadable members and re-run (NFR-CC-04).",
        a.members_read,
        a.members_total,
    );
    assert!(
        a.references > 0,
        "VOID, not 0: the coverage tier classified no cross-boundary reference at all \
         over {} members. Nothing was measured (NFR-CC-04).",
        a.members_read,
    );

    // ── The payload shape AC2's count rests on ──────────────────────────
    assert_eq!(
        a.missing_provenance, 0,
        "`provenance` is not optional (FR-WS-19 AC6): {} of {} rows carry none, so an \
         admitted value is indistinguishable from an observed one on exactly those rows \
         — which is the boundary ADR-64 draws, and the count below would be taken over a \
         payload that cannot answer the question.",
        a.missing_provenance, a.references,
    );

    // ── The figure, and the criterion it is measured against ────────────
    //
    // **The floor is checked FIRST, and the order is load-bearing.** Behind the
    // exact pin below it could never fail — 44 < 79 is a compile-time fact — so a
    // rise past the floor would have surfaced as `44 != 137` rather than as the
    // one thing a reader needs told. Here it is reachable: any run whose admitted
    // count clears 79 stops here, whatever the pin says.
    assert!(
        a.config_bound < CRITERION_FLOOR,
        "S-397 AC2's floor of {CRITERION_FLOOR} is now MET: {} rows carry `config-bound` \
         provenance, against the {RECORDED_ADMITTED} recorded on 2026-09-13. That is a \
         genuine change and this assertion is the one that must not be edited away — \
         record what moved the figure (the qualified-receiver ceiling is the only \
         mechanism between {RECORDED_ADMITTED} and {CRITERION_FLOOR}), mark AC2 \
         satisfied, and restate the finding in this file and in the artifact beside it.",
        a.config_bound,
    );
    assert_eq!(
        a.config_bound, RECORDED_ADMITTED,
        "S-397 T2's recorded finding is that the shipped pipeline admits \
         {RECORDED_ADMITTED} `config-bound` client-call rows on the reference estate \
         (of an accessor denominator of {ACCESSOR_DENOMINATOR}), against AC2's floor of \
         {CRITERION_FLOOR}. This run read {}. If the corpus has been re-indexed or \
         re-enrolled, or the accessor hop has widened, RECORD the new figure here and in \
         the artifact — do not bend the pipeline to reproduce this one, and do not relax \
         the floor.",
        a.config_bound,
    );
    assert_eq!(
        a.by_bucket,
        RECORDED_BUCKETS.map(|(bucket, n)| (bucket.to_string(), n)).to_vec(),
        "the admitted rows' bucket split moved. The headline can hold while the split \
         moves, and the split is the more informative half: a `config-bound` row is \
         excluded from `resolved_cross_service_edges` whatever its bucket, so `bound: 5` \
         is what the estate's own topology yields and not a coverage claim.",
    );
    assert_eq!(
        a.config_unresolved, 0,
        "the recorded finding is that NO admitted accessor names a key the committed \
         sources fail to define: the shortfall against the floor is capture, not \
         resolution. This run read {} `config-unresolved` rows, which moves the diagnosis \
         — record it, because it is the figure the `.properties` residue would show up in.",
        a.config_unresolved,
    );

    const {
        // The recorded headline must itself be a shortfall, or the finding this
        // file records is not the finding its assertions test. Nothing else
        // constrains these two constants: the test body runs only under
        // LOGOS_REF_WORKSPACE, so either could be edited to any value and a
        // default `cargo test` would stay green.
        assert!(
            RECORDED_ADMITTED < CRITERION_FLOOR,
            "the recorded figure must be below the floor, or this is not a shortfall",
        );
        assert!(
            RECORDED_ADMITTED < ACCESSOR_DENOMINATOR,
            "the admitted figure must be inside the denominator it is stated over",
        );
    }

    // Census floors, not equalities: the estate can only grow and a re-clone
    // must not redden the run. The figures above are pinned exactly; this guards
    // the order of magnitude the finding was recorded at.
    assert!(
        a.members_read >= 40 && a.references >= 900,
        "the recorded finding measured 1060 references over 84 members; this run saw {} \
         over {}. A collapsed corpus is a broken harness, not a new finding.",
        a.references,
        a.members_read,
    );
}
