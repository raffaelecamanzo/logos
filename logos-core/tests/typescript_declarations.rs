//! TypeScript declares abstract classes, member signatures, wrapped components,
//! top-level constants and default-exported expressions (S-532, CR-173,
//! FR-EX-16, NFR-RA-06) — exercised end to end through the public [`Engine`]
//! façade against temp-directory fixtures.
//!
//! The per-construct extraction facts live beside the extractor
//! (`extract/tests.rs`); this suite pins what only the persisted graph shows:
//! the kinds and the required-signature marker the store holds, that no call
//! binds to a bodyless member, and that a one-file edit re-derives every new
//! declaration through sync to exactly what a fresh index of the edited tree
//! records.

#![cfg(feature = "lang-typescript")]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every node as `(name, kind, is a required signature)`.
fn nodes(rt: &Runtime) -> Vec<(String, NodeKind, bool)> {
    rt.submit_read(|store| {
        let signatures: HashSet<NodeId> =
            store.node_item_facts()?.into_iter().filter(|f| f.signature).map(|f| f.id).collect();
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.name, n.kind, signatures.contains(&n.id)))
            .collect())
    })
    .expect("read runs")
}

const DECLARATIONS: &str = "\
export abstract class UserRepository {
  abstract findById(id: string): string;
  save(id: string): void {}
}
export interface Port { send(m: string): void; }
export const Button = React.forwardRef((props, ref) => null);
export const Dialog = DialogPrimitive.Root;
export default registerAs('db', () => ({}));
";

#[test]
fn the_store_holds_the_declarations_and_marks_every_bodyless_member_a_signature() {
    let tmp = tree(&[("src/ui.tsx", DECLARATIONS)]);
    let engine = index(tmp.path());
    let by_name: HashMap<String, (NodeKind, bool)> =
        nodes(engine.runtime().unwrap()).into_iter().map(|(n, k, s)| (n, (k, s))).collect();
    let got = |name: &str| *by_name.get(name).unwrap_or_else(|| panic!("{name} is persisted: {by_name:?}"));
    assert_eq!(got("UserRepository"), (NodeKind::Class, false));
    assert_eq!(got("findById"), (NodeKind::Method, true));
    assert_eq!(got("save"), (NodeKind::Method, false));
    assert_eq!(got("Port"), (NodeKind::Interface, false));
    assert_eq!(got("send"), (NodeKind::Method, true));
    assert_eq!(got("Button"), (NodeKind::Function, false));
    assert_eq!(got("Dialog"), (NodeKind::Constant, false));
    assert_eq!(got("default"), (NodeKind::Constant, false));
}

/// A bodyless member is never a call target (S-606): every binding run leaves
/// a required signature out. `this.step()` in an abstract class names its own
/// abstract `step` — without the marker it would bind to the signature — while
/// `this.send()` in a class binds to its implemented method as before.
#[test]
fn no_call_binds_to_a_bodyless_member() {
    let tmp = tree(&[(
        "src/steps.ts",
        "export interface Port { send(m: string): void; }\n\
export abstract class Base {\n  abstract step(): void;\n  run(): void { this.step(); }\n}\n\
export class Wire {\n  send(m: string): void {}\n  flush(): void { this.send('x'); }\n}\n",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let signatures = nodes(rt).into_iter().filter(|(_, _, signature)| *signature).count();
    assert_eq!(signatures, 2, "the interface's `send` and the abstract class's `step`");
    let calls: Vec<(String, String)> = rt
        .submit_read(|store| {
            let names: HashMap<NodeId, String> = store.all_nodes()?.into_iter().map(|n| (n.id, n.name)).collect();
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
                .collect())
        })
        .unwrap();
    assert_eq!(calls, [("flush".to_string(), "send".to_string())], "only the implemented method is a call target");
}

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical; the synced graph's
/// nodes are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<(String, NodeKind, bool)> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        fs::write(tmp_a.path().join(rel), text).unwrap();
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    nodes(engine_a.runtime().unwrap())
}

#[test]
fn a_declaration_added_or_removed_in_one_file_syncs_as_a_fresh_index() {
    const USE: &str = "import { keep } from './ui';\nkeep();\n";
    let grown = format!("export function keep() {{}}\n{DECLARATIONS}");
    let base = [("src/ui.tsx", "export function keep() {}\n"), ("src/use.ts", USE)];
    let added = synced_equals_reindexed(&base, &[("src/ui.tsx", &grown)]);
    assert!(added.contains(&("UserRepository".to_string(), NodeKind::Class, false)), "the synced graph holds the new class: {added:?}");
    assert!(added.contains(&("send".to_string(), NodeKind::Method, true)), "and the new signature: {added:?}");
    let removed = synced_equals_reindexed(&[("src/ui.tsx", &grown), ("src/use.ts", USE)], &[("src/ui.tsx", "export function keep() {}\n")]);
    assert!(removed.iter().all(|(n, _, _)| n != "UserRepository" && n != "default"), "the synced graph drops them: {removed:?}");
}
