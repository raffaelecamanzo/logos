//! The fixed repository coordinate a test fixture pins (S-703, [ADR-85]) — the
//! logos-core integration suite's one definition of it, included by every test
//! file whose assertions see a symbol.
//!
//! Every node symbol carries the repository coordinate in its package slot. A
//! fixture that does not pin `[project] id` derives one from its temp
//! directory's name, which differs on every run and every machine: two stores
//! built from one tree would then differ in every symbol, and a golden would
//! differ between CI targets. Pinning it makes both byte-identical.
//!
//! [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md

#![allow(dead_code)] // each including binary uses a different subset

use std::fs;
use std::path::Path;

/// The coordinate a pinned fixture carries.
pub const FIXTURE_COORDINATE: &str = "example.com/fixture";

/// The `[project]` table pinning `id` — for a fixture that writes its own
/// `.logos/config.toml` and must carry the pin inside it.
pub fn project_table(id: &str) -> String {
    format!("[project]\nid = \"{id}\"\n")
}

/// The canonical symbol a pinned fixture's declaration carries:
/// `descriptors` (path and scope, e.g. ``src/`lib.rs`/run().``) under
/// [`FIXTURE_COORDINATE`].
pub fn symbol(descriptors: &str) -> String {
    format!("logos . {FIXTURE_COORDINATE} . {descriptors}")
}

/// Pin [`FIXTURE_COORDINATE`] as `root`'s `[project] id`.
pub fn pin(root: &Path) {
    pin_as(root, FIXTURE_COORDINATE);
}

/// Pin `id` as `root`'s `[project] id`: appended to the `.logos/config.toml`
/// the fixture already wrote (a TOML table may follow any other), or written as
/// the file's only table. A config that already names a `[project]` is left
/// as it is.
pub fn pin_as(root: &Path, id: &str) {
    let path = root.join(".logos/config.toml");
    let mut body = fs::read_to_string(&path).unwrap_or_default();
    if body.contains("[project]") {
        return;
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&project_table(id));
    fs::create_dir_all(path.parent().expect("a config path has a parent")).expect("create .logos/");
    fs::write(&path, body).expect("write .logos/config.toml");
}

/// Give an **already indexed** member a different `[project] id` — for a
/// fixture whose members were indexed under one shared coordinate and must be
/// distinct identities in a workspace, which degrades the later of two members
/// resolving to one coordinate (S-706, [FR-WS-39]).
///
/// The store is not touched here; it migrates to the new coordinate the next
/// time it is opened (S-704, [FR-EX-35]), so no member ever answers under the
/// old one. A fixture whose members must answer to the same symbol queries it
/// written without a coordinate ([FR-EX-36]), never through this.
///
/// [FR-WS-39]: ../../../docs/specs/requirements/FR-WS-39.md
/// [FR-EX-35]: ../../../docs/specs/requirements/FR-EX-35.md
/// [FR-EX-36]: ../../../docs/specs/requirements/FR-EX-36.md
pub fn detach(root: &Path, id: &str) {
    let path = root.join(".logos/config.toml");
    let body = fs::read_to_string(&path).expect("a pinned fixture has a config");
    let detached: Vec<String> = body
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("id = ") {
                format!("id = \"{id}\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    assert_ne!(body.lines().collect::<Vec<_>>(), detached, "{} pins an id to replace", path.display());
    fs::write(&path, detached.join("\n") + "\n").expect("write .logos/config.toml");
}
