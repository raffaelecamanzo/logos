//! S-706 fitness function ([FR-WS-39], [BR-74], [ADR-85]): the symbol sets of a
//! workspace's member stores **union without collision or elision**.
//!
//! Before the repository coordinate was part of a symbol, two members that
//! shared a repo-relative path — the normal case for archetype-generated
//! services, which all carry `src/index.ts` — built the *same* symbol string for
//! the same declaration, so any join keyed on a symbol alone merged two
//! repositories into one. Sprint 103 answered that at the answer layer by
//! keying cross-member joins on `(member, symbol)`; the coordinate is the root
//! fix, and this test is what proves it holds on the graphs themselves.
//!
//! The fixture: three services generated from one template — identical files at
//! identical paths — each with its own `[project] id`. Each member's store is
//! indexed, and every node symbol is read back through the registry's own
//! member walk, the way a workspace answer reads them.
//!
//! The same check is then run over the same symbols with the coordinate forced
//! empty (the slot [ADR-85]'s reversibility path renders), and **must** report
//! collisions. A fitness function that cannot fail proves nothing: that second
//! assertion is what shows this one detects the defect it was written for.
//!
//! [FR-WS-39]: ../../docs/specs/requirements/FR-WS-39.md
//! [BR-74]: ../../docs/specs/software-spec.md#35-extraction--symbol-ids
//! [ADR-85]: ../../docs/specs/architecture/decisions/ADR-85.md

#![cfg(feature = "lang-typescript")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use logos_core::federation::{EngineRegistry, Federation, Member, RegistryMode};
use logos_core::model::LogosSymbol;
use logos_core::Engine;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

/// The services the template generated, and the coordinate each one carries.
const SERVICES: [(&str, &str); 3] = [
    ("users-service", "example.com/users-service"),
    ("orders-service", "example.com/orders-service"),
    ("billing-service", "example.com/billing-service"),
];

/// What the template wrote into every service: the same files, the same
/// declarations, at the same repo-relative paths.
const TEMPLATE: [(&str, &str); 3] = [
    (
        "src/index.ts",
        "import { route } from \"./routes\";\n\
         export function start(port: number): void {\n  route(port);\n}\n\
         export class Service {\n  run(): void {\n    start(8080);\n  }\n}\n",
    ),
    (
        "src/routes.ts",
        "export function route(port: number): number {\n  return port;\n}\n",
    ),
    (
        "src/health.ts",
        "export const healthy = (): boolean => true;\n",
    ),
];

fn service(root: &Path, name: &str, coordinate: &str) -> Member {
    let dir = root.join(name);
    for (rel, body) in TEMPLATE {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    coordinate::pin_as(&dir, coordinate);
    // Index with a throwaway engine so the registry's walk below finds a graph.
    let engine = Engine::start(&dir).expect("start the member engine");
    let result = engine.index();
    assert!(result.persist_failures.is_empty(), "{name}: {:?}", result.persist_failures);
    Member {
        name: name.to_string(),
        root: dir.canonicalize().unwrap(),
    }
}

fn workspace(root: &Path) -> EngineRegistry<Engine> {
    let members = SERVICES
        .iter()
        .map(|(name, coordinate)| service(root, name, coordinate))
        .collect();
    let federation = Federation {
        name: "template".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: Default::default(),
        exclusion: Default::default(),
    };
    EngineRegistry::new(federation, RegistryMode::Lazy)
}

/// Every node symbol of every member store, by member, read through the
/// registry's own all-member walk.
fn symbols_by_member(registry: &EngineRegistry<Engine>) -> BTreeMap<String, Vec<LogosSymbol>> {
    registry
        .answer()
        .fan_out(|_member, engine| {
            engine
                .runtime()
                .unwrap()
                .submit_read(|store| Ok(store.all_nodes()?.into_iter().map(|n| n.symbol).collect()))
                .unwrap()
        })
        .into_iter()
        .map(|scoped| (scoped.member, scoped.value.expect("every member opens")))
        .collect()
}

/// The symbols two or more members hold, each with the members that hold it —
/// the union's collisions. Empty exactly when the union is collision-free.
fn collisions(sets: &BTreeMap<String, Vec<LogosSymbol>>) -> BTreeMap<String, BTreeSet<String>> {
    let mut holders: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (member, symbols) in sets {
        for symbol in symbols {
            holders
                .entry(symbol.as_str().to_string())
                .or_default()
                .insert(member.clone());
        }
    }
    holders.retain(|_, members| members.len() > 1);
    holders
}

/// The size of the union of every member's symbol set.
fn union_size(sets: &BTreeMap<String, Vec<LogosSymbol>>) -> usize {
    sets.values()
        .flatten()
        .map(LogosSymbol::as_str)
        .collect::<BTreeSet<_>>()
        .len()
}

/// The same symbols with the coordinate forced to the empty slot.
fn coordinate_less(sets: &BTreeMap<String, Vec<LogosSymbol>>) -> BTreeMap<String, Vec<LogosSymbol>> {
    sets.iter()
        .map(|(member, symbols)| {
            let emptied = symbols
                .iter()
                .map(|symbol| symbol.with_coordinate(None).expect("the empty slot renders"))
                .collect();
            (member.clone(), emptied)
        })
        .collect()
}

#[test]
fn the_symbol_sets_of_members_sharing_every_path_union_without_collision() {
    let tmp = TempDir::new().unwrap();
    let registry = workspace(tmp.path());
    let sets = symbols_by_member(&registry);

    // Not vacuous: every member holds a graph, of the same shape, and the
    // fixture really does give them the same repo-relative declarations.
    let sizes: Vec<usize> = sets.values().map(Vec::len).collect();
    assert_eq!(sets.len(), SERVICES.len(), "{sets:?}");
    assert!(sizes.iter().all(|&n| n >= 6), "each member holds a real graph: {sizes:?}");
    assert!(sizes.windows(2).all(|w| w[0] == w[1]), "template-identical members: {sizes:?}");
    let tails: Vec<BTreeSet<&str>> = sets
        .values()
        .map(|symbols| symbols.iter().filter_map(LogosSymbol::descriptor_tail).collect())
        .collect();
    assert!(tails.windows(2).all(|w| w[0] == w[1]), "identical descriptor tails: {tails:?}");

    // The fitness function: no symbol is held by two members, and the union is
    // exactly as large as the members put together — nothing collided, nothing
    // was elided.
    assert_eq!(collisions(&sets), BTreeMap::new(), "no symbol is held by two members");
    let total: usize = sizes.iter().sum();
    assert_eq!(union_size(&sets), total, "the union equals the sum of the member node counts");

    // Each member's symbols carry that member's own coordinate and no other.
    for (member, coordinate) in SERVICES {
        let held = &sets[member];
        assert!(
            held.iter().all(|symbol| symbol.coordinate() == Some(coordinate)),
            "{member}: every symbol carries {coordinate}"
        );
    }
}

/// The negative that makes the fitness function mean something: the same
/// symbols with the coordinate forced empty collide, and the union shrinks below
/// the sum — exactly the defect the coordinate removes.
#[test]
fn forcing_the_coordinate_empty_makes_the_same_check_fail() {
    let tmp = TempDir::new().unwrap();
    let registry = workspace(tmp.path());
    let sets = symbols_by_member(&registry);
    let emptied = coordinate_less(&sets);

    assert!(
        emptied.values().flatten().all(LogosSymbol::is_coordinate_less),
        "every symbol was rendered in the empty slot"
    );
    let found = collisions(&emptied);
    assert!(!found.is_empty(), "coordinate-less members collide on shared paths");
    assert!(
        found.values().all(|members| members.len() == SERVICES.len()),
        "every shared declaration is held by all three members: {found:?}"
    );
    let total: usize = emptied.values().map(Vec::len).sum();
    assert!(
        union_size(&emptied) < total,
        "the union ({}) is smaller than the sum ({total}): symbols were folded",
        union_size(&emptied)
    );
    // And the collision is the whole of the loss: one distinct symbol per
    // declaration the template wrote.
    assert_eq!(union_size(&emptied), total / SERVICES.len());
}
