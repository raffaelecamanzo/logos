//! Integration tests for the CR-005 structural extraction facts (S-042),
//! driving the public extraction API and the [`Engine`] façade against real
//! fixtures.
//!
//! Coverage by acceptance criterion:
//! - per-function **maximum nesting depth** from the declarative
//!   `nesting_block_kinds`, hand-verifiable in each of the five languages and
//!   byte-identical across runs ([FR-EX-07](../../docs/specs/requirements/FR-EX-07.md));
//! - **member-access `Accesses` edges** (Method → Field) bound under the
//!   exactly-one-candidate rule, with an unmatched access staying honestly in
//!   `unresolved_refs` ([FR-EX-08](../../docs/specs/requirements/FR-EX-08.md),
//!   [NFR-RA-05](../../docs/specs/requirements/NFR-RA-05.md));
//! - **winnowed near-clone shingle** sets that are rename-invariant and
//!   deterministic ([FR-EX-09](../../docs/specs/requirements/FR-EX-09.md)).
//!
//! Gated on `lang-rust` (the crate's baseline feature); each per-language case
//! is additionally gated on its own `lang-*` feature, exactly as the grammars
//! are, so a narrower build excludes the cases it cannot run.
#![cfg(feature = "lang-rust")]

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::plugin::LanguageRegistry;

/// Load the registry with the embedded grammars, no on-disk overrides.
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

/// Extract one in-memory source string, picking the plugin by `path`'s extension.
fn facts_for(reg: &LanguageRegistry, path: &str, src: &str) -> Facts {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .expect("path has an extension");
    let plugin = reg
        .for_extension(ext)
        .unwrap_or_else(|| panic!("no grammar for .{ext}"));
    extract(
        &FileInput::new(path, src),
        plugin,
        &SymbolContext::cargo("logos-core", "0.1.0"),
    )
}

/// The `max_nesting_depth` of the (first) callable named `name`.
fn depth_of(facts: &Facts, name: &str) -> Option<u32> {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .and_then(|n| n.max_nesting_depth)
}

// ── FR-EX-07: per-function max nesting depth, hand-verifiable per language ────

/// A fixture's nested function is depth 3, its flat function is depth 0, and the
/// values are byte-identical across two independent extractions. Run once per
/// language so the same structural shape scores identically everywhere
/// (NFR-RA-06).
fn assert_nesting(path: &str, src: &str, nested: &str, flat: &str) {
    let reg = registry();
    let facts = facts_for(&reg, path, src);
    assert_eq!(
        depth_of(&facts, nested),
        Some(3),
        "{path}: `{nested}` has a hand-computed nesting depth of 3"
    );
    assert_eq!(
        depth_of(&facts, flat),
        Some(0),
        "{path}: `{flat}` is a flat body (depth 0)"
    );
    // The nested function's non-trivial body yields a shingle set through the
    // full extract() path (FR-EX-09), and the set is itself deterministic —
    // pipeline wiring coverage, not just the unit-level `shingles()`.
    let nested_shingles: Vec<u64> = facts
        .nodes
        .iter()
        .find(|n| n.name == nested)
        .map(|n| n.shingles.clone())
        .unwrap_or_default();
    assert!(
        !nested_shingles.is_empty(),
        "{path}: a non-trivial body yields shingles through the pipeline"
    );
    // Byte-identical across runs (deterministic extraction, NFR-RA-06).
    let again = facts_for(&reg, path, src);
    assert_eq!(
        facts.nodes, again.nodes,
        "{path}: extraction is deterministic across runs"
    );
}

#[test]
fn rust_nesting_depth_is_hand_verifiable() {
    // if (1) → while (2) → match (3); a flat sibling stays 0.
    assert_nesting(
        "src/n.rs",
        "\
fn nested(a: bool, n: u32) {
    if a {
        while a {
            match n { _ => {} }
        }
    }
}

fn flat() {
    let _x = 1;
}
",
        "nested",
        "flat",
    );
}

#[cfg(feature = "lang-python")]
#[test]
fn python_nesting_depth_is_hand_verifiable() {
    // if (1) → for (2) → while (3).
    assert_nesting(
        "n.py",
        "\
def nested(a):
    if a:
        for _ in a:
            while a:
                pass

def flat():
    return 1
",
        "nested",
        "flat",
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_nesting_depth_is_hand_verifiable() {
    // if (1) → for (2) → while (3).
    assert_nesting(
        "n.ts",
        "\
function nested(a: boolean) {
    if (a) {
        for (;;) {
            while (a) {}
        }
    }
}

function flat() {
    return 1;
}
",
        "nested",
        "flat",
    );
}

#[cfg(feature = "lang-go")]
#[test]
fn go_nesting_depth_is_hand_verifiable() {
    // if (1) → for (2) → switch (3, expression_switch_statement).
    assert_nesting(
        "n.go",
        "\
package p

func nested(a bool) {
    if a {
        for {
            switch {
            default:
            }
        }
    }
}

func flat() int {
    return 1
}
",
        "nested",
        "flat",
    );
}

#[cfg(feature = "lang-java")]
#[test]
fn java_nesting_depth_is_hand_verifiable() {
    // if (1) → for (2) → while (3); the methods nest in the class body.
    assert_nesting(
        "N.java",
        "\
class N {
    void nested(boolean a) {
        if (a) {
            for (;;) {
                while (a) {}
            }
        }
    }

    void flat() {
        int x = 1;
    }
}
",
        "nested",
        "flat",
    );
}

#[cfg(feature = "lang-c")]
#[test]
fn c_nesting_depth_is_hand_verifiable() {
    // if (1) → for (2) → while (3). Exercises the engine's `declarator`-field
    // ascent: the captured name nests in a `function_declarator`, so a depth of
    // 3 proves metrics see the whole `function_definition` body (S-056), not the
    // declarator.
    assert_nesting(
        "n.c",
        "\
int nested(int a) {
    if (a) {
        for (;;) {
            while (a) {
            }
        }
    }
    return 0;
}

int flat(void) {
    return 1;
}
",
        "nested",
        "flat",
    );
}

#[cfg(feature = "lang-php")]
#[test]
fn php_nesting_depth_is_hand_verifiable() {
    // if (1) → for (2) → while (3); the functions are free (file scope).
    assert_nesting(
        "n.php",
        "<?php
function nested($a) {
    if ($a) {
        for (;;) {
            while ($a) {}
        }
    }
}

function flat() {
    $x = 1;
}
",
        "nested",
        "flat",
    );
}

// ── FR-EX-09: rename-invariant, deterministic shingle fingerprints ───────────

#[cfg(feature = "lang-python")]
#[test]
fn rename_equivalent_functions_share_their_shingle_set() {
    use std::collections::BTreeSet;
    let reg = registry();
    let original = facts_for(
        &reg,
        "a.py",
        "\
def compute(values):
    total = 0
    for value in values:
        if value > 10:
            total = total + value
    return total
",
    );
    let renamed = facts_for(
        &reg,
        "b.py",
        "\
def tally(items):
    sum = 0
    for item in items:
        if item > 10:
            sum = sum + item
    return sum
",
    );
    let a: BTreeSet<u64> = original
        .nodes
        .iter()
        .find(|n| n.name == "compute")
        .map(|n| n.shingles.iter().copied().collect())
        .expect("compute node");
    let b: BTreeSet<u64> = renamed
        .nodes
        .iter()
        .find(|n| n.name == "tally")
        .map(|n| n.shingles.iter().copied().collect())
        .expect("tally node");
    assert!(!a.is_empty(), "a non-trivial body produces shingles");
    assert_eq!(
        a, b,
        "renaming identifiers must not change the shingle set (FR-EX-09)"
    );
}

#[test]
fn rust_rename_equivalent_functions_share_their_shingle_set() {
    use std::collections::BTreeSet;
    let reg = registry();
    let original = facts_for(
        &reg,
        "a.rs",
        "\
fn compute(values: &[u32]) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > 10 {
            total += value * 2;
        }
    }
    total
}
",
    );
    let renamed = facts_for(
        &reg,
        "b.rs",
        "\
fn tally(items: &[u32]) -> u32 {
    let mut sum = 0;
    for item in items {
        if *item > 10 {
            sum += item * 2;
        }
    }
    sum
}
",
    );
    let set = |facts: &Facts, name: &str| -> BTreeSet<u64> {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.shingles.iter().copied().collect())
            .unwrap_or_else(|| panic!("{name} node"))
    };
    let a = set(&original, "compute");
    let b = set(&renamed, "tally");
    assert!(!a.is_empty(), "a non-trivial body produces shingles");
    assert_eq!(
        a, b,
        "renaming identifiers must not change the shingle set, through the full \
         Rust extract() path (FR-EX-09)"
    );
}

// ── FR-EX-08 / NFR-RA-05: member-access Accesses-edge binding (Java) ─────────

/// Java is the v1 language whose fields are extracted as `Field` nodes and whose
/// methods lexically nest in the class, so an own-field access binds end-to-end
/// through the pipeline. The positive access (`this.count`) becomes one
/// `Accesses` edge Method → Field; an unmatched access (`this.missing`, no such
/// field) stays honestly in `unresolved_refs` with `resolved = 0` — never
/// fabricated ([FR-EX-08], [NFR-RA-05]).
#[cfg(feature = "lang-java")]
mod accesses {
    use logos_core::model::{EdgeKind, NodeId, NodeKind};
    use logos_core::{Engine, Runtime};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
        let wanted = name.to_string();
        rt.submit_read(move |store| {
            let rows = store.search(&wanted, Some(kind), 16)?;
            Ok(rows.into_iter().find(|r| r.name == wanted).map(|r| r.id))
        })
        .expect("read runs")
        .unwrap_or_else(|| panic!("no {kind:?} node named {name}"))
    }

    fn accesses_edges(rt: &Runtime) -> Vec<(NodeId, NodeId)> {
        rt.submit_read(|store| {
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Accesses)
                .map(|e| (e.source, e.target))
                .collect())
        })
        .expect("read runs")
    }

    #[test]
    fn member_access_binds_to_one_field_and_unmatched_stays_unresolved() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "Repo.java",
            "\
class Repo {
    private int count;
    int total;

    int get() {
        return this.count;
    }

    int bad() {
        return this.missing;
    }
}
",
        );

        let engine = Engine::start(tmp.path()).expect("engine starts");
        let rt = engine.runtime().unwrap();
        engine.index();

        let get = node_id(rt, "get", NodeKind::Method);
        let count = node_id(rt, "count", NodeKind::Field);

        // The matched own-field access is exactly one Accesses edge: get → count.
        let edges = accesses_edges(rt);
        assert!(
            edges.contains(&(get, count)),
            "this.count binds to the one Field of the enclosing class (FR-EX-08)"
        );
        assert_eq!(
            edges.len(),
            1,
            "only the matched access is an edge — the unmatched one is not fabricated"
        );

        // The unmatched access (`this.missing`, no such field) stays in the
        // ledger, unresolved — never invented (NFR-RA-05).
        let refs = rt
            .submit_read(|store| store.unresolved_refs())
            .expect("read runs");
        let unmatched: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Accesses && r.target == "missing")
            .collect();
        assert_eq!(
            unmatched.len(),
            1,
            "the unmatched member access is recorded once in the ledger"
        );
        assert!(
            !unmatched[0].resolved,
            "an unmatched access stays resolved = 0 and retries on sync (NFR-RA-05)"
        );
    }

    /// Two classes each declare a field of the **same name**; each method's
    /// `this.count` must bind to *its own* class's field, never the other's.
    /// This exercises the class-like-container scoping of `resolve_member_access`:
    /// the candidate set is the enclosing class's fields, so a same-named field in
    /// a different class is never a candidate. Were scoping broken (workspace-wide
    /// `count` lookup), the access would see two candidates, fail the exactly-one
    /// rule, and bind nothing — so two distinct-target edges prove the isolation
    /// ([FR-EX-08], [NFR-RA-05]).
    #[test]
    fn same_named_fields_in_different_classes_do_not_cross_bind() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "Two.java",
            "\
class A {
    private int count;
    int getA() { return this.count; }
}

class B {
    private int count;
    int getB() { return this.count; }
}
",
        );

        let engine = Engine::start(tmp.path()).expect("engine starts");
        let rt = engine.runtime().unwrap();
        engine.index();

        let edges = accesses_edges(rt);
        assert_eq!(
            edges.len(),
            2,
            "each method binds to exactly one own-class field — scoping isolates \
             the same-named candidate in the other class (not an ambiguity)"
        );
        let targets: std::collections::BTreeSet<NodeId> = edges.iter().map(|(_, t)| *t).collect();
        assert_eq!(
            targets.len(),
            2,
            "the two accesses bind to two distinct fields — never cross-bound"
        );
    }
}

// ── FR-EX-08 / NFR-RA-05: member-access Accesses-edge binding (C#) ───────────

/// C# is class-bearing (S-057, CR-009): its fields extract as `Field` nodes and
/// its methods lexically nest in the type, so an own-field access binds
/// end-to-end and feeds Cohesion/LCOM4. This proves the `references.scm`
/// `this.X` capture — whose `expression: "this"` matches the anonymous `this`
/// keyword token (tree-sitter-c-sharp has no named `this` node) — actually
/// produces the bound `Accesses` edge, and that an unmatched access stays
/// honestly unresolved ([FR-EX-08], [NFR-RA-05]).
#[cfg(feature = "lang-c-sharp")]
mod csharp_accesses {
    use logos_core::model::{EdgeKind, NodeId, NodeKind};
    use logos_core::{Engine, Runtime};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
        let wanted = name.to_string();
        rt.submit_read(move |store| {
            let rows = store.search(&wanted, Some(kind), 16)?;
            Ok(rows.into_iter().find(|r| r.name == wanted).map(|r| r.id))
        })
        .expect("read runs")
        .unwrap_or_else(|| panic!("no {kind:?} node named {name}"))
    }

    fn accesses_edges(rt: &Runtime) -> Vec<(NodeId, NodeId)> {
        rt.submit_read(|store| {
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Accesses)
                .map(|e| (e.source, e.target))
                .collect())
        })
        .expect("read runs")
    }

    #[test]
    fn member_access_binds_to_one_field_and_unmatched_stays_unresolved() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "Repo.cs",
            "\
public class Repo {
    private int count;
    public int total;

    public int Get() {
        return this.count;
    }

    public int Bad() {
        return this.missing;
    }
}
",
        );

        let engine = Engine::start(tmp.path()).expect("engine starts");
        let rt = engine.runtime().unwrap();
        engine.index();

        let get = node_id(rt, "Get", NodeKind::Method);
        let count = node_id(rt, "count", NodeKind::Field);

        // The matched own-field access is exactly one Accesses edge: Get → count.
        let edges = accesses_edges(rt);
        assert!(
            edges.contains(&(get, count)),
            "this.count binds to the one Field of the enclosing class (FR-EX-08)"
        );
        assert_eq!(
            edges.len(),
            1,
            "only the matched access is an edge — the unmatched one is not fabricated"
        );

        // The unmatched access (`this.missing`, no such field) stays in the
        // ledger, unresolved — never invented (NFR-RA-05).
        let refs = rt
            .submit_read(|store| store.unresolved_refs())
            .expect("read runs");
        let unmatched: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Accesses && r.target == "missing")
            .collect();
        assert_eq!(
            unmatched.len(),
            1,
            "the unmatched member access is recorded once in the ledger"
        );
        assert!(
            !unmatched[0].resolved,
            "an unmatched access stays resolved = 0 and retries on sync (NFR-RA-05)"
        );
    }
}

// ── FR-EX-08 / NFR-RA-05: PHP own-property access + never-fabricate (S-060) ──

/// PHP properties extract as `Field` nodes and methods nest in the class, so a
/// `$this->balance` own-property access binds end-to-end: one `Accesses` edge
/// Method → Field (the bound LCOM4 input that makes the `Class` cohesion-
/// applicable, FR-QM-11). An access to a non-existent property
/// (`$this->missing`) the resolver cannot bind stays honestly in the ledger,
/// never fabricated ([FR-EX-08], [NFR-RA-05]).
#[cfg(feature = "lang-php")]
mod php_accesses {
    use logos_core::model::{EdgeKind, NodeId, NodeKind};
    use logos_core::{Engine, Runtime};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
        let wanted = name.to_string();
        rt.submit_read(move |store| {
            let rows = store.search(&wanted, Some(kind), 16)?;
            Ok(rows.into_iter().find(|r| r.name == wanted).map(|r| r.id))
        })
        .expect("read runs")
        .unwrap_or_else(|| panic!("no {kind:?} node named {name}"))
    }

    fn accesses_edges(rt: &Runtime) -> Vec<(NodeId, NodeId)> {
        rt.submit_read(|store| {
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Accesses)
                .map(|e| (e.source, e.target))
                .collect())
        })
        .expect("read runs")
    }

    #[test]
    fn own_property_access_binds_and_unbindable_stays_unresolved() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "Account.php",
            "<?php
class Account {
    private int $balance;
    public int $total;

    public function read() { return $this->balance; }

    public function bad() { return $this->missing; }
}
",
        );

        let engine = Engine::start(tmp.path()).expect("engine starts");
        let rt = engine.runtime().unwrap();
        engine.index();

        let read = node_id(rt, "read", NodeKind::Method);
        let balance = node_id(rt, "balance", NodeKind::Field);

        // The matched own-property access is exactly one Accesses edge.
        let edges = accesses_edges(rt);
        assert!(
            edges.contains(&(read, balance)),
            "$this->balance binds to the one Field of the enclosing class (FR-EX-08)"
        );
        assert_eq!(
            edges.len(),
            1,
            "only the matched access is an edge — the unbindable one is not fabricated"
        );

        // The unbindable access (`$this->missing`) stays unresolved (NFR-RA-05).
        let refs = rt
            .submit_read(|store| store.unresolved_refs())
            .expect("read runs");
        let unmatched: Vec<_> = refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Accesses && r.target == "missing")
            .collect();
        assert_eq!(
            unmatched.len(),
            1,
            "the unbindable member access is recorded once in the ledger"
        );
        assert!(
            !unmatched[0].resolved,
            "an unbindable access stays resolved = 0 and retries on sync (NFR-RA-05)"
        );
    }
}
