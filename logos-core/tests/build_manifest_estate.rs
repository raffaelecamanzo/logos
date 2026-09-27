//! The reference-workspace census of build-manifest facts (S-462, CR-148 §2.1,
//! ADR-69 decision point 1).
//!
//! Estate-gated: set `LOGOS_REF_WORKSPACE` to a workspace root. Without it the
//! test prints `SKIPPED` and passes — which is why an estate figure only counts
//! when the run that produced it is shown (command, date, denominators).
//!
//! # Read-only, and the product's own code
//!
//! For each workspace member this walks the member exactly as the indexing
//! pipeline does — its own `logos.toml`, then [`config::discover`] — keeps the
//! build manifests that walk admits, reads them, and hands them to
//! [`member_facts`], the function the pipeline persists from. It opens **no
//! member database** and writes nothing, so it can never migrate a store; the
//! persisted twin of these figures is read off a private copy of the estate
//! (see the S-462 implementation notes).
//!
//! # A census, never a floor
//!
//! CR-148 §2.1's hand census is an upper bound to reconcile against by name.
//! Nothing here asserts a count ([census-figure-as-acceptance-floor]); the
//! assertions are structural — every refusal carries its reason, every
//! manifest found is either read or says why not.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use logos_core::config;
use logos_core::extract::build_manifest::{
    manifest_format, member_facts, ArtifactRole, ManifestFacts, ManifestStatus, ReferenceKind,
    Resolution,
};
use logos_core::federation;

fn workspace_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(&home).join(rest),
        None => PathBuf::from(&raw),
    };
    assert!(
        expanded.is_dir(),
        "LOGOS_REF_WORKSPACE={raw} is not a directory — refusing to report a green run that \
         measured nothing"
    );
    Some(expanded)
}

/// One member's manifests, read through the pipeline's own walk.
fn member_manifests(root: &std::path::Path) -> (usize, Vec<ManifestFacts>) {
    let cfg = config::load_config_from_root(root).expect("the member's logos.toml loads");
    let report = config::discover(root, &cfg).expect("the member walks");
    let canon = root.canonicalize().expect("canonical member root");
    let mut texts: Vec<(String, String)> = Vec::new();
    let mut unreadable: Vec<ManifestFacts> = Vec::new();
    for abs in &report.files {
        let rel = abs
            .strip_prefix(&canon)
            .expect("the walk stays in the root")
            .to_string_lossy()
            .replace('\\', "/");
        let Some(format) = manifest_format(&rel) else {
            continue;
        };
        match std::fs::read_to_string(abs) {
            Ok(text) => texts.push((rel, text)),
            Err(e) => unreadable.push(ManifestFacts::unreadable(&rel, format, e.to_string())),
        }
    }
    let found = texts.len() + unreadable.len();
    let pairs: Vec<(&str, &str)> = texts.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let mut facts = member_facts(&pairs);
    facts.extend(unreadable);
    (found, facts)
}

#[test]
fn the_reference_workspace_reports_its_build_manifest_facts_when_one_is_configured() {
    let Some(root) = workspace_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-462 build-manifest census."
        );
        return;
    };
    let workspace = federation::discover(&root)
        .expect("the workspace manifest parses")
        .expect("LOGOS_REF_WORKSPACE holds a logos.workspace.toml");

    println!("S-462 build-manifest census — workspace `{}`, {} members", workspace.name, workspace.members.len());
    println!(
        "{:<48} {:>5} {:>4} {:>4} {:>4} | {:>4} {:>4} {:>4} {:>4} | {:>4} {:>4} {:>4}",
        "member", "found", "read", "prod", "p-rf", "par", "dep", "mgd", "bom", "res", "v-rf", "ref"
    );

    let mut totals = BTreeMap::<&str, usize>::new();
    // coordinate → producing members, and member → referenced coordinates by kind.
    let mut producers: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut references: Vec<(String, &'static str, String, String)> = Vec::new();
    let mut members_with_manifests = 0usize;
    // Every fact that did not fully resolve, with its reason — the refusals are
    // few enough to read one by one, and reading them is the point.
    let mut unresolved: Vec<String> = Vec::new();

    for member in &workspace.members {
        let (found, facts) = member_manifests(&member.root);
        let read = facts.iter().filter(|m| m.status == ManifestStatus::Read).count();
        let mut row = BTreeMap::<&str, usize>::new();
        for m in &facts {
            for a in &m.artifacts {
                assert_eq!(
                    a.reason.is_some(),
                    a.resolution != Resolution::Resolved,
                    "{}: {}: a refusal carries its reason and a resolution does not",
                    member.name,
                    m.path
                );
                if let Some(reason) = &a.reason {
                    unresolved.push(format!(
                        "  {} {} {}:{} [{}] — {reason}",
                        member.name,
                        m.path,
                        a.group_id.as_deref().unwrap_or("?"),
                        a.artifact_id.as_deref().unwrap_or("?"),
                        a.resolution.as_str(),
                    ));
                }
                match a.role {
                    ArtifactRole::Produced => {
                        *row.entry("produced").or_default() += 1;
                        if a.resolution == Resolution::Refused {
                            *row.entry("produced_refused").or_default() += 1;
                        } else if let (Some(g), Some(art)) = (&a.group_id, &a.artifact_id) {
                            producers.entry((g.clone(), art.clone())).or_default().insert(member.name.clone());
                        }
                    }
                    ArtifactRole::Referenced => {
                        let kind = a.kind.expect("a reference has a kind");
                        *row.entry(kind.as_str()).or_default() += 1;
                        *row.entry(a.resolution.as_str()).or_default() += 1;
                        if a.resolution != Resolution::Refused {
                            if let (Some(g), Some(art)) = (&a.group_id, &a.artifact_id) {
                                references.push((member.name.clone(), kind.as_str(), g.clone(), art.clone()));
                            }
                        }
                    }
                }
            }
        }
        assert!(read <= found, "{}: files read never exceed manifests found", member.name);
        if found > 0 {
            members_with_manifests += 1;
        }
        let g = |k: &str| row.get(k).copied().unwrap_or(0);
        println!(
            "{:<48} {:>5} {:>4} {:>4} {:>4} | {:>4} {:>4} {:>4} {:>4} | {:>4} {:>4} {:>4}",
            member.name,
            found,
            read,
            g("produced"),
            g("produced_refused"),
            g(ReferenceKind::Parent.as_str()),
            g(ReferenceKind::Dependency.as_str()),
            g(ReferenceKind::Managed.as_str()),
            g(ReferenceKind::BomImport.as_str()),
            g("resolved"),
            g("version-refused"),
            g("refused"),
        );
        *totals.entry("found").or_default() += found;
        *totals.entry("read").or_default() += read;
        for (k, v) in row {
            *totals.entry(k).or_default() += v;
        }
    }
    let t = |k: &str| totals.get(k).copied().unwrap_or(0);
    println!(
        "TOTAL: manifests read {} of {} found, over {} of {} members; produced {} ({} refused); \
         references parent {} / dependency {} / managed {} / bom-import {}; \
         resolved {} / version-refused {} / refused {}",
        t("read"),
        t("found"),
        members_with_manifests,
        workspace.members.len(),
        t("produced"),
        t("produced_refused"),
        t("parent"),
        t("dependency"),
        t("managed"),
        t("bom-import"),
        t("resolved"),
        t("version-refused"),
        t("refused"),
    );

    println!("facts that did not fully resolve: {}", unresolved.len());
    for line in &unresolved {
        println!("{line}");
    }

    // ── Reconciliation by name to CR-148 §2.1 (a report, never an assertion) ──
    let distinct_produced = producers.len();
    let collisions: Vec<_> = producers.iter().filter(|(_, m)| m.len() > 1).collect();
    println!("distinct produced coordinates: {distinct_produced}; produced by >1 member: {}", collisions.len());
    for ((g, a), members) in &collisions {
        println!("  collision {g}:{a} <- {members:?}");
    }
    // Member-level pairs by kind, joined by name only for this reconciliation —
    // the product relation is S-463's.
    let mut pairs: BTreeMap<&str, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut inbound: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (from, kind, g, a) in &references {
        let Some(owners) = producers.get(&(g.clone(), a.clone())) else {
            continue;
        };
        if owners.len() != 1 {
            continue;
        }
        let to = owners.iter().next().unwrap().clone();
        if &to == from {
            continue;
        }
        pairs.entry(kind).or_default().insert((from.clone(), to.clone()));
        inbound.entry(to).or_default().insert(from.clone());
    }
    let any: BTreeSet<_> = pairs.values().flatten().cloned().collect();
    println!(
        "member pairs with any build edge: {} (parent {} / dependency {} / managed {} / bom-import {})",
        any.len(),
        pairs.get("parent").map_or(0, BTreeSet::len),
        pairs.get("dependency").map_or(0, BTreeSet::len),
        pairs.get("managed").map_or(0, BTreeSet::len),
        pairs.get("bom-import").map_or(0, BTreeSet::len),
    );
    let mut by_in_degree: Vec<_> = inbound.iter().map(|(m, from)| (from.len(), m)).collect();
    by_in_degree.sort_by(|a, b| b.cmp(a));
    for (n, m) in by_in_degree.iter().take(14) {
        println!("  in-degree {n:>3}  {m}");
    }
}
