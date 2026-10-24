//! S-787 ([FR-GV-26], [FR-QM-17], [UAT-GV-16], [CR-243]): a near-clone group
//! has one id and one name in the attribution and in "where to start".
//!
//! The attribution's Uniqueness driver once carried its first member's id and
//! name (first by file and line), while the group's "where to start" unit
//! carries `clone_group:<digest>` and is named by the member first in identity
//! order. Over a real git fixture: the two agree on `review`, `gate` and the
//! read-only verdict Health reads; a file reorder and a full re-index keep the
//! id and the name while the member set is unchanged; `members` still lists
//! every member.
//!
//! The fixture pins `[project] id`, so every symbol is the same on every machine.
//!
//! [FR-GV-26]: ../../docs/specs/requirements/FR-GV-26.md
//! [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md
//! [UAT-GV-16]: ../../docs/specs/requirements/UAT-GV-16.md
//! [CR-243]: ../../docs/requests/CR-243-governance-signals-reach-every-surface.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::models::quality::{AttributionDriver, FixUnit, FixUnitKind, SignalAttribution};
use logos_core::Engine;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c", "user.email=dev@logos", "-c", "user.name=Logos Dev",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
            "-c", "maintenance.auto=false", "-c", "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

/// A body long enough to be a near-clone of its copy.
fn copy_of(name: &str) -> String {
    format!(
        "pub fn {name}(items: &[i64], limit: i64) -> i64 {{\n    let mut total = 0;\n    \
         for item in items {{\n        if *item > limit {{\n            total += item * 2 - limit;\n        \
         }} else if *item < -limit {{\n            total -= item * 3 + limit;\n        }} else {{\n            \
         total += item;\n        }}\n    }}\n    if total > limit * 10 {{\n        total / 2\n    }} else {{\n        \
         total + limit\n    }}\n}}\n"
    )
}

/// Eight shallow functions on `main`, the gate's baseline saved there.
fn repo() -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    coordinate::pin(root);
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    let source: String = (0..8)
        .map(|i| format!("pub fn shallow_{i}(x: i32) -> i32 {{\n    if x > {i} {{\n        return x;\n    }}\n    0\n}}\n"))
        .collect();
    write(root, "src/lib.rs", &source);
    write(root, ".logos/rules.toml", "[review]\n");
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let saved = engine.gate(None, true, true).expect("gate --save");
    assert!(saved.saved && saved.signal.is_some(), "{saved:?}");
    git(root, &["checkout", "-q", "-b", "copy"]);
    (tmp, engine)
}

/// The Uniqueness driver the review attributes the change to — the only one.
fn group_driver(a: &SignalAttribution) -> AttributionDriver {
    let uniqueness = a.contributions.iter().find(|c| c.dimension == "uniqueness").expect("listed");
    assert_eq!(uniqueness.driver_count, 1, "{uniqueness:#?}");
    uniqueness.drivers[0].clone()
}

/// The near-clone group unit "where to start" ranks.
fn group_unit(engine: &Engine) -> FixUnit {
    let ranking = engine.scan(true).expect("scan runs").where_to_start;
    ranking
        .units
        .into_iter()
        .find(|u| u.kind == FixUnitKind::CloneGroup)
        .expect("a near-clone group unit is ranked")
}

/// A group written with its identity-last member first by line, so the old
/// first-by-file-and-line name (`copy_zulu`) and the identity-order name
/// (`copy_alpha`) differ.
fn two_copies(root: &Path, first: &str, second: &str) {
    write(root, "src/dup.rs", &format!("{}{}", copy_of(first), copy_of(second)));
    commit(root, &format!("{first} then {second}"));
}

#[test]
fn a_group_driver_has_its_units_id_and_name_on_every_surface() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    two_copies(root, "copy_zulu", "copy_alpha");
    engine.index();

    let review = engine.review(None).expect("review runs").attribution.expect("attributed");
    let driver = group_driver(&review);
    let unit = group_unit(&engine);
    assert!(driver.node_id.as_deref().is_some_and(|id| id.starts_with("clone_group:")), "{driver:#?}");
    assert_eq!(driver.node_id, unit.node_id, "the driver's id is its unit's");
    assert_eq!(
        (driver.name.as_str(), driver.file.as_str(), driver.line),
        (unit.name.as_str(), unit.file.as_str(), unit.line),
        "named by the same member"
    );
    assert_eq!(driver.name, "copy_alpha", "first in identity order, not first by line: {driver:#?}");
    let members: Vec<&str> = driver.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["copy_zulu", "copy_alpha"], "every member, by line: {driver:#?}");

    // `gate` and the read-only verdict Health reads carry the same driver.
    let gate = engine.gate(None, false, true).expect("gate runs");
    assert_eq!(gate.attribution.as_ref(), Some(&review), "{gate:#?}");
    let latest = engine.latest_gate().expect("the read-only verdict");
    assert_eq!(latest.attribution.as_ref(), Some(&review), "{latest:#?}");
}

#[test]
fn the_id_and_name_survive_a_reorder_and_a_reindex_while_the_members_do() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    two_copies(root, "copy_zulu", "copy_alpha");
    engine.index();
    let before = group_driver(&engine.review(None).expect("review runs").attribution.expect("attributed"));

    // The same two members, in the other order in the file.
    two_copies(root, "copy_alpha", "copy_zulu");
    engine.index();
    let reordered = group_driver(&engine.review(None).expect("review runs").attribution.expect("attributed"));
    assert_eq!(reordered.node_id, before.node_id, "the id is a function of the member set");
    assert_eq!(reordered.name, before.name, "so is the naming member");
    assert_eq!(reordered.members.len(), 2, "{reordered:#?}");
    let by_line: Vec<&str> = reordered.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(by_line, ["copy_alpha", "copy_zulu"], "the members still follow the file: {reordered:#?}");

    // A different member set is another group.
    two_copies(root, "copy_alpha", "copy_yankee");
    engine.index();
    let renamed = group_driver(&engine.review(None).expect("review runs").attribution.expect("attributed"));
    assert_ne!(renamed.node_id, before.node_id, "one member renamed is another group");
    assert_eq!(renamed.node_id, group_unit(&engine).node_id);
}
