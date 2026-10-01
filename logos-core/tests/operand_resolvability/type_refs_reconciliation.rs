//! **S-473's reconciliation** — the shipped cross-member type overlay
//! ([`build_index`], [CR-152] §3.2 C) against [S-471]'s gate, **by name**.
//!
//! The product's bound figures are read off the shipped index, built from each
//! member's own declared-type facts and ledger rows, through the shipped read
//! API, over member stores opened read-only — no engine is started. The gate's
//! figures are [S-471]'s own rule ([`cross_member_type_refs`]), in two forms:
//!
//! 1. **as recorded** in `cross_member_type_refs_finding.txt`, over the estate
//!    as logos 1.5.0 indexed it — the Reach by provider, the pair split, the
//!    precision rows and the named non-build pairs are parsed or taken from its
//!    constants;
//! 2. **re-run** over the very stores the product reads (this branch's
//!    re-index), so a product-vs-gate difference can never be a ledger
//!    difference.
//!
//! Recorded → re-run is the ledger's drift between the two indexes, stated per
//! provider. Re-run → product is compared triple by triple, and every
//! difference is filed under a named [`Mechanism`]; an unattributed one fails
//! the run.
//!
//! Estate-gated: skips without `LOGOS_REF_WORKSPACE`, so it is invisible to
//! `gate.sh` and CI, and a figure from it counts only with its run shown.
//!
//! [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
//! [S-471]: ../../../docs/planning/journal.md#s-471-measure-cross-member-type-references-over-the-reference-estate

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::federation::{build_index, MemberTypeFacts, PairEvidence, TypeRefForm, TypeReferenceIndex};
use logos_core::graph_store::GraphStore;

use super::cross_member_type_refs::{
    consumer_tree, judge, open_member_store, read_estate, Estate, OwnerIndex, PairClass, Rule,
    Tree, Triple, CENSUS_NON_BUILD_PAIRS, RECORDED_FINDING, RECORDED_MAIN_TRIPLE_PAIRS,
    RECORDED_PRECISION, RECORDED_REACH, RECORDED_ROWS,
};

/// Why a triple is in one of the gate's Reach and the product's bound set and
/// not the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mechanism {
    /// The gate's path rule names a type the product holds no declared-type
    /// fact for: a Java annotation type (`@interface`), which the symbols
    /// query captures no node for — the declaring file named.
    AnnotationTypeHasNoFact(String),
    /// The same, for a declaring file that is not an annotation type — named.
    NoFact(String),
    /// The product declares the type in a file the gate's path rule does not
    /// derive it from (a second top-level type, or a file not named for it) —
    /// the file named.
    DeclaredBeyondThePathRule(String),
    /// The two rules see different owner sets for the type.
    OwnersDiffer { gate: Vec<String>, product: Vec<String> },
    /// The pair restriction disagrees: the product's evidence, named.
    PairDiffers(String),
    /// No mechanism this reconciliation names; printed, never absorbed.
    Unattributed,
}

/// Every member store's overlay inputs, read the way the shipped
/// `MemberContracts::type_facts` reads them — marker first, `None` when not
/// extracted.
fn read_type_facts(e: &Estate, root: &Path) -> (Vec<(String, MemberTypeFacts)>, Vec<String>) {
    let (mut facts, mut not_extracted) = (Vec::new(), Vec::new());
    for member in &e.members {
        let store = open_member_store(&root.join(member).join(".logos").join("logos.db"))
            .unwrap_or_else(|err| panic!("{member}: {err}"));
        let read = || -> anyhow::Result<Option<MemberTypeFacts>> {
            if !store.declared_types_extracted()? {
                return Ok(None);
            }
            Ok(Some(MemberTypeFacts {
                declared: store.declared_types()?,
                schemas: store.avro_schemas()?,
                rows: store.unresolved_type_refs()?,
            }))
        };
        match read().unwrap_or_else(|err| panic!("{member}: {err:#}")) {
            Some(read) => facts.push((member.clone(), read)),
            None => not_extracted.push(member.clone()),
        }
    }
    (facts, not_extracted)
}

/// The product's figures, on the gate's grain: import rows only.
#[derive(Debug, Default)]
pub struct ProductFigures {
    /// Main-tree bound import triples, with their pair class on the gate's
    /// vocabulary.
    pub main: BTreeMap<Triple, PairClass>,
    /// Exactly-one main-tree import triples of a type-only (or pair-unread)
    /// pair.
    pub main_unadmitted: BTreeMap<Triple, String>,
    /// `(ambiguous import rows, cross-member import rows)`, every tree.
    pub precision: (usize, usize),
}

impl ProductFigures {
    pub fn of(index: &TypeReferenceIndex) -> Self {
        let mut out = Self::default();
        let key = |r: &logos_core::federation::TypeReference| {
            (r.importer.member.clone(), r.owner.member.clone(), r.fqn.clone())
        };
        let imports = |list: &'_ [logos_core::federation::TypeReference]| {
            list.iter().filter(|r| r.form == TypeRefForm::Import).cloned().collect::<Vec<_>>()
        };
        let (bound, type_only, unread) =
            (imports(&index.references), imports(&index.type_only), imports(&index.pair_unread));
        for r in bound.iter().filter(|r| consumer_tree(&r.importer.file) == Tree::Main) {
            let class = match &r.evidence {
                PairEvidence::Build { platform } => PairClass::Build { platform: *platform },
                PairEvidence::Collision { .. } => PairClass::CollisionBacked,
                PairEvidence::TypeOnly | PairEvidence::PairUnread => unreachable!("never bound"),
            };
            out.main.insert(key(r), class);
        }
        for r in type_only.iter().chain(&unread).filter(|r| consumer_tree(&r.importer.file) == Tree::Main) {
            out.main_unadmitted.insert(key(r), format!("{:?}", r.evidence));
        }
        let ambiguous = index.ambiguous.iter().filter(|a| a.form == TypeRefForm::Import).count();
        out.precision = (ambiguous, ambiguous + bound.len() + type_only.len() + unread.len());
        out
    }

    /// `(build, build into a platform, collision-backed, type-only)` — the
    /// gate's `main_triple_pairs` on the product.
    pub fn main_triple_pairs(&self) -> (usize, usize, usize, usize) {
        let n = |want| self.main.values().filter(|p| **p == want).count();
        (
            n(PairClass::Build { platform: false }),
            n(PairClass::Build { platform: true }),
            n(PairClass::CollisionBacked),
            self.main_unadmitted.len(),
        )
    }
}

/// The finding's "Reach by provider" list, parsed by name.
pub fn recorded_reach_by_provider(finding: &str) -> BTreeMap<String, usize> {
    let start = finding.find("Reach by provider:").expect("the finding lists Reach by provider");
    let rest = &finding[start + "Reach by provider:".len()..];
    let end = rest.find("Test-tree triples").expect("the list ends before the test-tree line");
    rest[..end]
        .split('·')
        .filter_map(|item| {
            let mut words = item.split_whitespace();
            let (name, count) = (words.next()?, words.next()?);
            Some((name.to_string(), count.parse().ok()?))
        })
        .collect()
}

fn by_provider<'a>(triples: impl Iterator<Item = &'a Triple>) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for (_, provider, _) in triples {
        *out.entry(provider.clone()).or_default() += 1;
    }
    out
}

/// Whether the declaring source file declares `name` as an annotation type:
/// an `@interface` whose next identifier is exactly `name`.
fn is_annotation_type(file: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else { return false };
    text.split("@interface").skip(1).any(|after| {
        let ident: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
            .collect();
        ident == name
    })
}

/// Triples on one side only, each with the mechanism that put it there.
type Differences = Vec<(Triple, Mechanism)>;

/// Re-run → product, triple by triple: `(missing, extra)` — gate Reach triples
/// the product does not bind, and product triples outside the gate's Reach.
fn differences(
    root: &Path,
    e: &Estate,
    gate_reach: &BTreeSet<Triple>,
    gate_index: &OwnerIndex,
    product: &ProductFigures,
    index: &TypeReferenceIndex,
) -> (Differences, Differences) {
    let gate_owners = |t: &str| -> Vec<String> {
        gate_index.owners(t).map(|o| o.keys().cloned().collect()).unwrap_or_default()
    };
    let product_owners = |t: &str| -> Vec<String> {
        let mut owners: Vec<String> = index.owners(t).iter().map(|o| o.member.clone()).collect();
        owners.dedup();
        owners
    };
    let declaring_file = |member: &str, fqn: &str| {
        e.decl.sources.iter().find(|s| s.member == member && s.path_fqn() == fqn).map(|s| s.path.clone())
    };

    let mut missing = Vec::new();
    for triple in gate_reach.iter().filter(|t| !product.main.contains_key(*t)) {
        let (_, provider, fqn) = triple;
        let (gate, mine) = (gate_owners(fqn), product_owners(fqn));
        let why = if mine.is_empty() {
            match declaring_file(provider, fqn) {
                Some(file) => {
                    let name = fqn.rsplit('.').next().unwrap_or(fqn);
                    let path = format!("{provider}/{file}");
                    if is_annotation_type(&root.join(&path), name) {
                        Mechanism::AnnotationTypeHasNoFact(path)
                    } else {
                        Mechanism::NoFact(path)
                    }
                }
                None => Mechanism::Unattributed,
            }
        } else if gate != mine {
            Mechanism::OwnersDiffer { gate, product: mine }
        } else if let Some(evidence) = product.main_unadmitted.get(triple) {
            Mechanism::PairDiffers(evidence.clone())
        } else {
            Mechanism::Unattributed
        };
        missing.push((triple.clone(), why));
    }

    let mut extra = Vec::new();
    for triple in product.main.keys().filter(|t| !gate_reach.contains(*t)) {
        let (consumer, provider, fqn) = triple;
        let (gate, mine) = (gate_owners(fqn), product_owners(fqn));
        let why = if gate.is_empty() {
            let file = index
                .owners(fqn)
                .iter()
                .find(|o| &o.member == provider)
                .map(|o| format!("{provider}/{}", o.declared_in))
                .unwrap_or_default();
            Mechanism::DeclaredBeyondThePathRule(file)
        } else if gate != mine {
            Mechanism::OwnersDiffer { gate, product: mine }
        } else if !e.pairs.class(consumer, provider).admitted() {
            Mechanism::PairDiffers(format!("gate {:?}", e.pairs.class(consumer, provider)))
        } else {
            Mechanism::Unattributed
        };
        extra.push((triple.clone(), why));
    }
    (missing, extra)
}

fn mechanism_label(m: &Mechanism) -> &'static str {
    match m {
        Mechanism::AnnotationTypeHasNoFact(_) => "annotation type, no declared-type fact",
        Mechanism::NoFact(_) => "no declared-type fact",
        Mechanism::DeclaredBeyondThePathRule(_) => "declared in a file the path rule does not name",
        Mechanism::OwnersDiffer { .. } => "owner sets differ",
        Mechanism::PairDiffers(_) => "pair restriction differs",
        Mechanism::Unattributed => "UNATTRIBUTED",
    }
}

fn render_diffs(title: &str, diffs: &[(Triple, Mechanism)]) {
    println!("  {title}: {}", diffs.len());
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, m) in diffs {
        *by.entry(mechanism_label(m)).or_default() += 1;
    }
    for (label, n) in &by {
        println!("      {n:>4}  {label}");
    }
    for ((a, b, t), m) in diffs {
        println!("        {a} → {b}  {t}  [{m:?}]");
    }
}

/// Reconcile the shipped overlay against S-471's gate, by name, over the
/// reference estate — skipping without `LOGOS_REF_WORKSPACE`, so this run must
/// be shown explicitly: it is invisible to `gate.sh` and CI.
#[test]
fn reconcile_the_type_reference_overlay_with_the_s471_gate_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference workspace, \
             every member re-indexed by this binary> to reconcile S-473's type-reference overlay \
             with the S-471 gate."
        );
        return;
    };
    let e = read_estate(&root);
    let gate_index = OwnerIndex::build(&e.decl, Rule::Product);
    let gate = judge(&e.rows, &gate_index, &e.pairs);
    let gate_reach = gate.reach();

    let (facts, not_extracted) = read_type_facts(&e, &root);
    let federation = logos_core::federation::discover(&root).expect("parses").expect("a workspace");
    let index = build_index(&federation.members, &facts, &not_extracted, &e.relation);
    assert!(
        index.headline.members.unread.is_empty(),
        "members unread — re-index every member with this binary first: {:?}",
        index.headline.members.unread_reasons
    );
    let product = ProductFigures::of(&index);
    let product_reach: BTreeSet<Triple> = product.main.keys().cloned().collect();

    println!("S-473 RECONCILIATION — THE OVERLAY AGAINST THE S-471 GATE, BY NAME");
    println!("root: {}", root.display());
    println!("\nPRODUCT HEADLINE\n  {}", index.headline.summary);
    println!("  rows: {:?}", index.headline.rows);
    println!(
        "  members: {} of {} read; {} Java/Kotlin/Avro; owned declarations {}, test-tree {}, refused {}; \
         schemas {} read of {}",
        index.headline.members.read,
        index.headline.members.members,
        index.headline.members.java_kotlin_avro,
        index.headline.members.owned_declarations,
        index.headline.members.test_tree_declarations,
        index.headline.members.refused_declarations,
        index.headline.members.schemas_read,
        index.headline.members.schemas,
    );
    println!("  collision-backed: {:?}", index.headline.collision_backed);
    println!("  type-only: {:?}", index.headline.type_only);
    println!("  ambiguous-owner: {:?}", index.headline.ambiguous_owner);

    println!("\nTHREE FIGURES — recorded (1.5.0 stores) · gate rule re-run (these stores) · product");
    println!("  import rows:          {RECORDED_ROWS} · {} · (same rows)", e.rows.len());
    println!("  Reach (main triples): {RECORDED_REACH} · {} · {}", gate_reach.len(), product_reach.len());
    println!(
        "  main triples by pair (build, platform, collision, type-only): {RECORDED_MAIN_TRIPLE_PAIRS:?} · {:?} · {:?}",
        gate.main_triple_pairs(),
        product.main_triple_pairs()
    );
    println!("  precision (ambiguous, cross): {RECORDED_PRECISION:?} · {:?} · {:?}", gate.precision(), product.precision);

    println!("\nREACH BY PROVIDER — recorded · re-run · product");
    let recorded = recorded_reach_by_provider(RECORDED_FINDING);
    let rerun = by_provider(gate_reach.iter());
    let mine = by_provider(product_reach.iter());
    let providers: BTreeSet<&String> = recorded.keys().chain(rerun.keys()).chain(mine.keys()).collect();
    for p in providers {
        let n = |m: &BTreeMap<String, usize>| m.get(p).copied().unwrap_or(0);
        println!("  {p:<36} {:>4} · {:>4} · {:>4}", n(&recorded), n(&rerun), n(&mine));
    }

    println!("\nTHE NAMED NON-BUILD PAIRS (CR-152 §2.1) on the product");
    for (from, to) in CENSUS_NON_BUILD_PAIRS {
        let collision = index.headline.collision_backed.iter().find(|c| c.from == from && c.to == to);
        let only = index.headline.type_only.iter().find(|c| c.from == from && c.to == to);
        let in_reach = product_reach.iter().filter(|(a, b, _)| a == from && b == to).count();
        println!(
            "  {from} → {to}: {}",
            match (collision, only) {
                (Some(c), _) => format!("collision-backed via {:?}, {in_reach} main triples", c.artifacts),
                (None, Some(t)) => format!("type-only, {} row(s), types {:?}", t.references, t.types),
                (None, None) => "absent".to_string(),
            }
        );
    }

    println!("\nRE-RUN → PRODUCT, BY NAME");
    let (missing, extra) = differences(&root, &e, &gate_reach, &gate_index, &product, &index);
    render_diffs("gate Reach triples the product does not bind", &missing);
    render_diffs("product triples outside the gate's Reach", &extra);

    assert!(
        missing.iter().chain(&extra).all(|(_, m)| *m != Mechanism::Unattributed),
        "a difference between the overlay and the gate has no named mechanism"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The finding's provider list parses by name, every provider and count.
    #[test]
    fn the_recorded_reach_by_provider_parses_by_name() {
        let recorded = recorded_reach_by_provider(RECORDED_FINDING);
        assert_eq!(recorded.len(), 15, "{recorded:?}");
        assert_eq!(recorded["mailbox-kafka-models"], 145);
        assert_eq!(recorded["mailserver-common"], 4);
        assert_eq!(recorded.values().sum::<usize>(), RECORDED_REACH);
    }

    /// The annotation-type check reads the declaration, never the file name.
    #[test]
    fn an_annotation_type_is_recognised_by_its_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("NotEmpty.java");
        std::fs::write(&file, "package x;\n\npublic @interface\n  NotEmpty {}\n").unwrap();
        assert!(is_annotation_type(&file, "NotEmpty"));
        assert!(!is_annotation_type(&file, "NotEmptyObject"), "the near miss is not a match");
        std::fs::write(&file, "package x;\n\npublic @interface NotEmptyObject {}\n").unwrap();
        assert!(!is_annotation_type(&file, "NotEmpty"), "nor is a prefix of another annotation");
        std::fs::write(&file, "package x;\n\npublic interface NotEmpty {}\n").unwrap();
        assert!(!is_annotation_type(&file, "NotEmpty"));
    }
}
