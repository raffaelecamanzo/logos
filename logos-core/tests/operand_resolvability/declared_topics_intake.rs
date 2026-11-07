//! **S-489's estate arm** — the shipped declared-topic intake
//! ([`DeclaredTopics`]) over a private copy of the reference estate.
//!
//! Two acceptance criteria are measured here, and only here:
//!
//! - **The net-new pairs reproduce [S-488]'s by name.** S-488's recorded
//!   finding (`declared_topics_finding.txt`, embedded by its harness as
//!   [`RECORDED_FINDING`]) names 8 net-new `(member, topic)` pairs with their
//!   key and site split. [`recorded_net_new`] parses them out of it — the
//!   record is the source, never a restatement — and the run compares the
//!   product's net-new pairs against them, each difference printed with its
//!   mechanism.
//! - **Byte-identity with the intake on and off.** The topic inventory, the
//!   coverage payload (`resolved_cross_service_edges` and `egress_resolution`
//!   inside it) and the bridge edge set are serialized from a registry that
//!   never built the intake, and again from one that did; the two must be
//!   equal byte for byte. Member `scan`/`gate` are member-local commands that
//!   read no federation module; they are compared before and after this run by
//!   the shell procedure recorded in the implementation notes.
//!
//! Every other figure (the already-observed and declared-only pairs, the
//! refusals, the denominators) is **printed, never asserted** — figures from a
//! reference repository are records, not floors.
//!
//! Like every `LOGOS_REF_WORKSPACE` harness this arm is invisible to `gate.sh`
//! and to CI; without the variable it prints VOID. The parser of the record is
//! pinned by an always-run test.
//!
//! [S-488]: ../../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges

use std::collections::BTreeSet;
use std::path::Path;

use logos_core::federation::topics::workspace_topics;
use logos_core::federation::{
    cross_service_coverage, discover, xservice_declared_topics, ContractBridge, DeclarationStatus,
    DeclaredTopicRelation, DeclaredTopics, EngineRegistry, RegistryMode, SiteState,
};
use logos_core::Engine;

use super::declared_topics::RECORDED_FINDING;
use super::vendored_spec_contracts::refuse_a_store_that_would_migrate;

/// One net-new pair as S-488 recorded it: `(member, topic, key, split)`, the
/// split in the product's wire words (`config-only`, `site-refused`,
/// `site-uncaptured`).
pub type RecordedPair = (String, String, String, String);

/// The net-new pairs S-488's finding names under its headline, parsed out of
/// the record: each `member -> topic key` line, under the split heading it
/// sits beneath.
pub fn recorded_net_new(finding: &str) -> BTreeSet<RecordedPair> {
    let mut out = BTreeSet::new();
    let mut split: Option<&str> = None;
    let mut in_headline = false;
    for line in finding.lines() {
        if line.starts_with("THE HEADLINE") {
            in_headline = true;
            continue;
        }
        if line.starts_with("BESIDE THE HEADLINE") {
            break;
        }
        if !in_headline {
            continue;
        }
        let trimmed = line.trim_start();
        for (heading, word) in [
            ("CONFIG-ONLY", "config-only"),
            ("SITE-REFUSED", "site-refused"),
            ("SITE-UNCAPTURED", "site-uncaptured"),
        ] {
            if trimmed.starts_with(heading) {
                split = Some(word);
            }
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if let ([member, "->", topic, key], Some(word)) = (fields.as_slice(), split) {
            out.insert((
                member.to_string(),
                topic.to_string(),
                key.to_string(),
                word.to_string(),
            ));
        }
    }
    out
}

/// The split word of a site state — the record's vocabulary.
fn split_word(site: Option<&SiteState>) -> &'static str {
    match site {
        Some(SiteState::ConfigOnly) => "config-only",
        Some(SiteState::SiteRefused { .. }) => "site-refused",
        Some(SiteState::SiteUncaptured) => "site-uncaptured",
        Some(SiteState::SiteUnread) => "site-unread",
        None => "no site split",
    }
}

/// The product's net-new pairs in the record's shape. A pair declared under
/// two keys or files is one entry per key, as the record lists it.
pub fn product_net_new(r: &DeclaredTopicRelation) -> BTreeSet<RecordedPair> {
    r.declarations
        .iter()
        .filter(|d| d.status == DeclarationStatus::NetNew)
        .map(|d| {
            (
                d.member.clone(),
                d.topic.clone(),
                d.key.clone(),
                split_word(d.site.as_ref()).to_string(),
            )
        })
        .collect()
}

/// The observed read-models over `registry`, serialized: the topic inventory,
/// the coverage payload and the bridge edge set.
fn observed_payloads(registry: &EngineRegistry<Engine>) -> [String; 3] {
    let answer = registry.answer();
    [
        serde_json::to_string(&workspace_topics(&answer)).unwrap(),
        serde_json::to_string(&cross_service_coverage(&answer)).unwrap(),
        serde_json::to_string(&*ContractBridge::new().edges(registry)).unwrap(),
    ]
}

fn registry(root: &Path) -> EngineRegistry<Engine> {
    let federation = discover(root)
        .expect("the workspace manifest parses")
        .unwrap_or_else(|| panic!("{} is not a Logos workspace", root.display()));
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

fn report(
    r: &DeclaredTopicRelation,
    recorded: &BTreeSet<RecordedPair>,
    product: &BTreeSet<RecordedPair>,
) {
    println!("\n══ S-489 — THE SHIPPED DECLARED-TOPIC INTAKE OVER THE ESTATE COPY ══");
    println!("  {}", r.headline.summary);
    println!("\n  NET-NEW, by name (product) — {} entries", product.len());
    for d in r
        .declarations
        .iter()
        .filter(|d| d.status == DeclarationStatus::NetNew)
    {
        println!(
            "    {} -> {}   [{}]  key {}  file {}  profile {:?}",
            d.member,
            d.topic,
            split_word(d.site.as_ref()),
            d.key,
            d.file,
            d.profile
        );
        if let Some(SiteState::SiteRefused { refusals }) = &d.site {
            for x in refusals {
                println!("        refused site: {} at {}", x.reason, x.symbol);
            }
        }
    }
    println!("\n  RECORDED BY S-488 — {} entries", recorded.len());
    for x in recorded.difference(product) {
        println!("    in S-488, not here: {x:?}");
    }
    for x in product.difference(recorded) {
        println!("    here, not in S-488: {x:?}");
    }
    let listing = xservice_declared_topics(r, None);
    println!(
        "\n  ALREADY OBSERVED / NET-NEW, per topic ({} topics)",
        listing.topics.len()
    );
    for t in &listing.topics {
        let members: Vec<String> = t
            .declarations
            .iter()
            .map(|d| {
                format!(
                    "{}:{}",
                    d.member,
                    if d.status == DeclarationStatus::NetNew {
                        "net-new"
                    } else {
                        "observed"
                    }
                )
            })
            .collect();
        println!("    {}  {}", t.topic, members.join(" "));
    }
    println!("\n  DECLARED-ONLY — {}", listing.declared_only.len());
    for d in &listing.declared_only {
        println!("    {}: {:?} via {} ({})", d.member, d.topic, d.key, d.file);
    }
    println!("\n  REFUSED — {}", listing.refused.len());
    for x in &listing.refused {
        println!(
            "    {} {} = {:?} ({}) {:?}",
            x.member, x.key, x.value, x.file, x.reason
        );
    }
}

/// **S-489's estate criteria.** VOID without `LOGOS_REF_WORKSPACE`.
#[test]
fn the_shipped_intake_reproduces_the_recorded_net_new_pairs_and_moves_no_observed_figure() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "VOID: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference workspace> to \
             run S-489's estate arm. A run that sees no estate reports VOID, never zero."
        );
        return;
    };
    refuse_a_store_that_would_migrate(&registry(&root).federation().members);

    // OFF: a registry that never built the intake.
    let off = observed_payloads(&registry(&root));
    // ON: the intake built first — its site-split coverage walk included —
    // then the same read-models from the same registry.
    let on_registry = registry(&root);
    let relation = DeclaredTopics::new().relation(&on_registry);
    let on = observed_payloads(&on_registry);

    let recorded = recorded_net_new(RECORDED_FINDING);
    let product = product_net_new(&relation);
    report(&relation, &recorded, &product);

    for (label, (a, b)) in ["topic inventory", "coverage payload", "bridge edge set"]
        .iter()
        .zip(off.iter().zip(&on))
    {
        assert_eq!(a, b, "the {label} moved with the intake on");
    }
    println!(
        "\n  BYTE-IDENTITY: topic inventory {} B, coverage payload {} B, bridge edge set {} B — \
         equal with the intake off and on",
        on[0].len(),
        on[1].len(),
        on[2].len()
    );
    let h = &relation.headline;
    assert_eq!(
        h.members.read, h.members.members,
        "every member read: {:?}",
        h.members.unread
    );
    assert_eq!(
        h.declared_topic_pairs,
        h.already_observed.pairs + h.net_new.pairs
    );
    assert_eq!(
        h.net_new.pairs,
        h.net_new.config_only
            + h.net_new.site_refused
            + h.net_new.site_uncaptured
            + h.net_new.site_unread
    );
    assert_eq!(
        product, recorded,
        "the product's net-new pairs differ from S-488's recorded ones — explain each by mechanism"
    );
}

/// The record parser reads S-488's 8 pairs, each under its split — always run,
/// so a reworded record cannot silently turn the estate comparison vacuous.
#[test]
fn the_recorded_finding_names_eight_net_new_pairs_under_their_split() {
    let recorded = recorded_net_new(RECORDED_FINDING);
    assert_eq!(recorded.len(), 8, "{recorded:#?}");
    let split = |word: &str| recorded.iter().filter(|x| x.3 == word).count();
    assert_eq!(
        (
            split("config-only"),
            split("site-refused"),
            split("site-uncaptured")
        ),
        (3, 5, 0)
    );
    assert!(recorded.contains(&(
        "mailbox-unread-mail-batch".to_string(),
        "unread-mails-requests".to_string(),
        "spring.kafka.template.defaulttopic".to_string(),
        "config-only".to_string(),
    )));
    assert!(recorded
        .iter()
        .any(|x| x.0 == "deprecated-mailbox-core" && x.1 == "pec-server-commands"));
}
