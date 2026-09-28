//! The reference-workspace report of the build-dependency relation (S-463,
//! CR-148 §2.1, ADR-69 decision points 2–3).
//!
//! Estate-gated: set `LOGOS_REF_WORKSPACE` to a workspace root. Without it the
//! test prints `SKIPPED` and passes — which is why an estate figure only counts
//! when the run that produced it is shown (command, date, denominators).
//!
//! # A private copy only — and the harness refuses anything else
//!
//! The relation is read through each member's engine, the product path, so this
//! harness **opens member databases**, and opening one below the binary's schema
//! version migrates it. Before any engine starts, every member store is opened
//! read-only and its `PRAGMA user_version` compared with the version a fresh
//! store gets from this binary; one lower store fails the run naming it, so
//! pointing the harness at a live estate can never advance its graphs. Populate
//! a private copy first (a `logos health` per member with this binary).
//!
//! # A report, never a floor
//!
//! CR-148 §2.1's hand census is an upper bound to reconcile against by name.
//! Nothing here asserts a count ([census-figure-as-acceptance-floor]); the
//! assertions are structural — every reference is filed in exactly one bucket,
//! and every member is either read or named unread.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use logos_core::federation::build_deps::join;
use logos_core::federation::{
    self, BuildDependencies, BuildDependencyRelation, BuildEdgeKind, EngineRegistry, MemberContracts,
    MemberKind, RegistryMode,
};
use logos_core::Engine;

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

fn user_version(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap_or_else(|e| panic!("open {} read-only: {e}", db.display()));
    conn.query_row("PRAGMA user_version", [], |r| r.get(0)).expect("read user_version")
}

/// The schema version this binary gives a fresh store — derived, never pinned.
fn latest_schema_version() -> i64 {
    let tmp = tempfile::tempdir().expect("tempdir");
    drop(Engine::start(tmp.path()).expect("a fresh engine starts"));
    user_version(&tmp.path().join(".logos").join("logos.db"))
}

/// Refuse before any engine starts if a member store would migrate.
fn refuse_a_store_that_would_migrate(members: &[federation::Member]) {
    let latest = latest_schema_version();
    let behind: Vec<String> = members
        .iter()
        .filter_map(|m| {
            let db = m.root.join(".logos").join("logos.db");
            let v = db.is_file().then(|| user_version(&db))?;
            (v < latest).then(|| format!("{} (v{v})", m.name))
        })
        .collect();
    assert!(
        behind.is_empty(),
        "{} member store(s) are below schema v{latest} and would be MIGRATED by this run: {}. \
         Run on a private copy whose stores this binary has already reconciled.",
        behind.len(),
        behind.join(", ")
    );
}

const PLATFORM_HUBS: [&str; 2] = ["poste-pec-starter", "poste-pec-common"];

fn relation_over(root: &Path, kinds: &[(&str, MemberKind)]) -> std::sync::Arc<BuildDependencyRelation> {
    let mut fed = federation::discover(root).expect("manifest parses").expect("a workspace");
    for (member, kind) in kinds {
        fed.member_kinds.insert((*member).to_string(), *kind);
    }
    let registry = EngineRegistry::<Engine>::new(fed, RegistryMode::Lazy);
    BuildDependencies::new().relation(&registry)
}

fn assert_accounted(relation: &BuildDependencyRelation) {
    let r = relation.headline.references;
    assert_eq!(
        r.to_member + r.to_platform + r.in_member + r.to_collision + r.external + r.refused
            + r.project_reference + r.build_plugin,
        r.references,
        "every reference is filed exactly once"
    );
    let m = &relation.headline.members;
    assert_eq!(m.read + m.unread.len() as u64, m.members, "every member read or named unread");
}

/// The pairs a colliding coordinate would have produced had either producer
/// been the sole one — the mechanism behind any gap to a census that did not
/// apply the collision rule. Read off the member stores directly (read-only),
/// because the relation by design keeps no edge through a collision.
fn pairs_through_collisions(root: &Path, relation: &BuildDependencyRelation) {
    let fed = federation::discover(root).unwrap().unwrap();
    let colliding: BTreeMap<&str, &Vec<String>> = relation
        .headline
        .collisions
        .iter()
        .map(|c| (c.artifact.as_str(), &c.producers))
        .collect();
    let existing: BTreeSet<(&str, &str, BuildEdgeKind)> = relation
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str(), e.kind))
        .collect();
    let mut lost: BTreeMap<(String, String, String), BTreeSet<String>> = BTreeMap::new();
    for member in &fed.members {
        let db = member.root.join(".logos").join("logos.db");
        if !db.is_file() {
            continue;
        }
        let conn =
            rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT kind, group_id || ':' || artifact_id FROM build_artifacts \
                 WHERE role = 'referenced' AND resolution != 'refused' \
                 AND group_id IS NOT NULL AND artifact_id IS NOT NULL",
            )
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for (kind, artifact) in rows {
            let Some(producers) = colliding.get(artifact.as_str()) else { continue };
            for to in producers.iter().filter(|p| **p != member.name) {
                lost.entry((member.name.clone(), to.clone(), kind.clone()))
                    .or_default()
                    .insert(artifact.clone());
            }
        }
    }
    println!("\n## Pairs a colliding coordinate would have produced (one row per from→to, kind)");
    let mut new_pairs: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for ((from, to, kind), artifacts) in &lost {
        let already = BuildEdgeKind::from_fact(kind)
            .is_some_and(|k| existing.contains(&(from.as_str(), to.as_str(), k)));
        println!(
            "  {from} → {to}  {kind}  via {:?}{}",
            artifacts,
            if already { "  (pair+kind already present by another artifact)" } else { "" }
        );
        if !already {
            new_pairs.entry(kind.clone()).or_default().insert((from.clone(), to.clone()));
        }
    }
    let all_pairs: BTreeSet<(String, String)> = new_pairs.values().flatten().cloned().collect();
    let existing_pairs: BTreeSet<(&str, &str)> =
        relation.edges.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect();
    let fresh = all_pairs
        .iter()
        .filter(|(f, t)| !existing_pairs.contains(&(f.as_str(), t.as_str())))
        .count();
    println!(
        "  → {} (from,to,kind) not otherwise present: {:?}; {} member pairs not otherwise present",
        new_pairs.values().map(BTreeSet::len).sum::<usize>(),
        new_pairs.iter().map(|(k, v)| (k.as_str(), v.len())).collect::<Vec<_>>(),
        fresh
    );
}

/// **The census rule, replayed.** A hand census that does not apply the
/// collision rule gives each colliding coordinate to *one* of its producers.
/// Re-join the product's own facts with every collision resolved to its
/// first producer (sorted), then to its last, and report what each yields —
/// so a gap to the census is explained by which producer it picked, not by
/// arithmetic.
fn replay_collisions_resolved_to_one_producer(root: &Path, relation: &BuildDependencyRelation) {
    let fed = federation::discover(root).unwrap().unwrap();
    let registry = EngineRegistry::<Engine>::new(fed.clone(), RegistryMode::Lazy);
    let facts: Vec<(String, Vec<logos_core::graph_store::BuildManifestRow>)> = fed
        .members
        .iter()
        .map(|m| {
            let engine = registry.engine_for(&m.name).expect("member opens");
            let rows = engine
                .build_manifests()
                .expect("facts read")
                .unwrap_or_else(|| {
                    panic!("{}: build facts not yet extracted — run `logos health` in it", m.name)
                });
            (m.name.clone(), rows)
        })
        .collect();
    for (label, pick_last) in [("first", false), ("last", true)] {
        let keep: BTreeMap<&str, &str> = relation
            .headline
            .collisions
            .iter()
            .map(|c| {
                let p = if pick_last { c.producers.last() } else { c.producers.first() };
                (c.artifact.as_str(), p.unwrap().as_str())
            })
            .collect();
        let replayed: Vec<(String, Vec<logos_core::graph_store::BuildManifestRow>)> = facts
            .iter()
            .map(|(member, rows)| {
                let rows = rows
                    .iter()
                    .map(|row| {
                        let mut row = row.clone();
                        row.artifacts.retain(|a| {
                            let key = format!(
                                "{}:{}",
                                a.group_id.as_deref().unwrap_or(""),
                                a.artifact_id.as_deref().unwrap_or("")
                            );
                            a.role != "produced"
                                || keep.get(key.as_str()).is_none_or(|p| *p == member)
                        });
                        row
                    })
                    .collect();
                (member.clone(), rows)
            })
            .collect();
        let hubs: BTreeMap<String, MemberKind> =
            PLATFORM_HUBS.iter().map(|m| ((*m).to_string(), MemberKind::Platform)).collect();
        let all = join(&fed.members, &BTreeMap::new(), &replayed, &[]);
        let apart = join(&fed.members, &hubs, &replayed, &[]);
        let non_hub_dep: BTreeSet<(&str, &str)> = apart
            .edges
            .iter()
            .filter(|e| !e.platform && e.kind == BuildEdgeKind::Dependency)
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        let members: BTreeSet<&str> = non_hub_dep.iter().flat_map(|(f, t)| [*f, *t]).collect();
        let gained: Vec<String> = all
            .edges
            .iter()
            .filter(|e| {
                !relation.edges.iter().any(|x| {
                    x.from == e.from && x.to == e.to && x.kind == e.kind && x.artifact == e.artifact
                })
            })
            .map(|e| format!("{} → {} {}", e.from, e.to, e.kind.as_str()))
            .collect();
        println!(
            "\n## Replay: every collision resolved to its {label} producer {:?}\n  {:?}\n  \
             non-hub dependency pairs {} over {} members; edges gained: {:?}",
            keep,
            all.headline.build_dependency_pairs,
            non_hub_dep.len(),
            members.len(),
            gained
        );
    }
}

fn per_member_table(relation: &BuildDependencyRelation) {
    println!("\n## Per member (out = builds against, in = built against by; distinct members, by kind)");
    println!("| member | out par | out dep | out mgd | out bom | in par | in dep | in mgd | in bom |");
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for view in relation.per_member() {
        let count = |edges: &[federation::BuildsAgainst], kind, outbound: bool| {
            edges
                .iter()
                .filter(|e| e.kind == kind)
                .map(|e| if outbound { e.to.as_str() } else { e.from.as_str() })
                .collect::<BTreeSet<_>>()
                .len()
        };
        let kinds = BuildEdgeKind::ALL;
        let out: Vec<String> =
            kinds.iter().map(|k| count(&view.builds_against, *k, true).to_string()).collect();
        let inb: Vec<String> =
            kinds.iter().map(|k| count(&view.built_against_by, *k, false).to_string()).collect();
        if view.builds_against.is_empty() && view.built_against_by.is_empty() {
            continue;
        }
        println!("| {} | {} | {} |", view.member, out.join(" | "), inb.join(" | "));
    }
    let isolated: Vec<String> = relation
        .per_member()
        .into_iter()
        .filter(|v| v.builds_against.is_empty() && v.built_against_by.is_empty())
        .map(|v| v.member)
        .collect();
    println!("\nmembers read with no build edge ({}): {:?}", isolated.len(), isolated);
}

fn dependency_scopes(relation: &BuildDependencyRelation) {
    let mut by_scope: BTreeMap<String, BTreeSet<(&str, &str)>> = BTreeMap::new();
    for e in relation.edges.iter().filter(|e| e.kind == BuildEdgeKind::Dependency) {
        by_scope
            .entry(e.scope.clone().unwrap_or_else(|| "(undeclared)".into()))
            .or_default()
            .insert((e.from.as_str(), e.to.as_str()));
    }
    let compile_or_undeclared: BTreeSet<(&str, &str)> = relation
        .edges
        .iter()
        .filter(|e| {
            e.kind == BuildEdgeKind::Dependency
                && matches!(e.scope.as_deref(), None | Some("compile"))
        })
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    let any: BTreeSet<(&str, &str)> = relation
        .edges
        .iter()
        .filter(|e| e.kind == BuildEdgeKind::Dependency)
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    println!(
        "\n## Dependency pairs by scope: {:?}; compile-or-undeclared {} of {}; test-scope-only {}",
        by_scope.iter().map(|(k, v)| (k.as_str(), v.len())).collect::<Vec<_>>(),
        compile_or_undeclared.len(),
        any.len(),
        any.difference(&compile_or_undeclared).count()
    );
}

fn headline(label: &str, relation: &BuildDependencyRelation) {
    println!("\n## {label}");
    println!("{}", serde_json::to_string_pretty(&relation.headline).unwrap());
}

#[test]
fn estate_build_dependency_relation_report() {
    let Some(root) = workspace_root() else {
        println!("SKIPPED — set LOGOS_REF_WORKSPACE to a PRIVATE COPY of the reference workspace");
        return;
    };
    let fed = federation::discover(&root).expect("manifest parses").expect("a workspace");
    refuse_a_store_that_would_migrate(&fed.members);
    println!("# build-dependency relation over {} ({} members)", root.display(), fed.members.len());

    let undeclared = relation_over(&root, &[]);
    assert_accounted(&undeclared);
    headline("Headline, no kind declared", &undeclared);
    per_member_table(&undeclared);
    dependency_scopes(&undeclared);
    for kind in BuildEdgeKind::ALL {
        let mut inbound: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for e in undeclared.edges.iter().filter(|e| e.kind == kind) {
            inbound.entry(e.to.as_str()).or_default().insert(e.from.as_str());
        }
        let mut top: Vec<(usize, &str)> = inbound.iter().map(|(t, f)| (f.len(), *t)).collect();
        top.sort_by(|a, b| b.cmp(a));
        println!("top in-degree, {}: {:?}", kind.as_str(), &top[..top.len().min(8)]);
    }
    let mut any_kind: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &undeclared.edges {
        any_kind.entry(e.to.as_str()).or_default().insert(e.from.as_str());
    }
    println!(
        "in-degree (any kind) of the *-kafka-models members: {:?}",
        any_kind
            .iter()
            .filter(|(m, _)| m.ends_with("-kafka-models"))
            .map(|(m, f)| (*m, f.len()))
            .collect::<Vec<_>>()
    );
    pairs_through_collisions(&root, &undeclared);
    replay_collisions_resolved_to_one_producer(&root, &undeclared);

    let hubs: Vec<(&str, MemberKind)> =
        PLATFORM_HUBS.iter().map(|m| (*m, MemberKind::Platform)).collect();
    let declared = relation_over(&root, &hubs);
    assert_accounted(&declared);
    headline("Headline, both hubs declared platform", &declared);
    let non_hub_dep: BTreeSet<(&str, &str)> = declared
        .edges
        .iter()
        .filter(|e| !e.platform && e.kind == BuildEdgeKind::Dependency)
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    let members: BTreeSet<&str> = non_hub_dep.iter().flat_map(|(f, t)| [*f, *t]).collect();
    println!(
        "\nnon-hub dependency pairs: {} over {} members",
        non_hub_dep.len(),
        members.len()
    );
    println!(
        "deprecated-mailbox-core's non-hub dependency pairs: {:?}",
        non_hub_dep
            .iter()
            .filter(|(f, t)| *f == "deprecated-mailbox-core" || *t == "deprecated-mailbox-core")
            .collect::<Vec<_>>()
    );
}
