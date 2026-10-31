//! A bare TypeScript specifier binds through the importing file's nearest
//! `tsconfig.json`/`jsconfig.json` and the workspace packages (S-531, CR-173
//! §3.2 (1), FR-RS-02, NFR-RA-05) — exercised end-to-end through the public
//! [`Engine`] façade against a real temp-directory monorepo.
//!
//! Before S-531 every non-relative specifier was decided unbound, and a call
//! through one kept its bare name: `@/lib/format` (0 of 200 on
//! bulletproof-react's `apps/react-vite`) and `@acme/ui` reached nothing. These
//! fixtures pin the corrected contract:
//!
//! - each app's `@/` resolves through its **own** config, so two apps' `@/lib`
//!   reach two different files, and a call through the alias binds;
//! - a workspace package name binds to the package's entry, and a call through
//!   it binds to the entry's export;
//! - an external package (`react`) stays unbound — its import, and a call
//!   through it, which never reaches a workspace function of the same name;
//! - a `tsconfig.json` edit re-binds on the sync it arrives in, and the synced
//!   graph equals a cold index of the same tree (sync ≡ reindex).

#![cfg(feature = "lang-typescript")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine
}

/// Every edge of `kind` as `(source file:name, target file:name)`, sorted.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The files the `Imports` edges out of `rel` reach, sorted.
fn imported_files(rt: &Runtime, rel: &str) -> Vec<String> {
    let prefix = format!("{rel}:");
    let mut out: Vec<String> = edges_of(rt, EdgeKind::Imports)
        .into_iter()
        .filter(|(s, _)| s.starts_with(&prefix))
        .map(|(_, t)| t.split(':').next().unwrap().to_string())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    out.sort();
    out
}

/// The `Calls` edges whose target is the node `file:name`, as source labels.
fn callers_of(rt: &Runtime, target: &str) -> Vec<String> {
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t == target)
        .map(|(s, _)| s)
        .collect()
}

/// Whether the ledger rows of `kind` with exactly `target` exist and are all
/// unresolved — the never-fabricate outcome: the reference is kept.
fn unresolved(rt: &Runtime, kind: EdgeKind, target: &str) -> bool {
    let wanted = target.to_string();
    let rows: Vec<bool> = rt
        .submit_read(move |store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == kind && r.target == wanted)
                .map(|r| r.resolved)
                .collect())
        })
        .expect("read runs");
    assert!(!rows.is_empty(), "no {kind:?} ledger row for {target:?}");
    rows.iter().all(|resolved| !resolved)
}

const ALIAS: &str = r#"{
  // JSONC, as tsconfig files are written
  "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"], }, },
}"#;

/// Two apps, each with its own `@/`, and a workspace package `@acme/ui`
/// whose `exports` entry is its source and whose `main` is an unbuilt `dist/`.
/// `packages/ui/src/hooks.ts` declares a `useState` — the decoy an external
/// `react` import must never reach.
fn monorepo(root: &Path) {
    write(root, "package.json", r#"{ "private": true, "workspaces": ["apps/*", "packages/*"] }"#);
    write(root, "apps/web/tsconfig.json", ALIAS);
    write(root, "apps/admin/jsconfig.json", ALIAS);
    write(
        root,
        "apps/web/src/lib/format.ts",
        "export function formatDate(d: string): string { return d; }\n",
    );
    write(
        root,
        "apps/admin/src/lib/format.ts",
        "export function formatDate(d: string): string { return d; }\n",
    );
    write(
        root,
        "apps/web/src/app.tsx",
        "import { formatDate } from '@/lib/format';\n\
         import { Button } from '@acme/ui';\n\
         import { useState } from 'react';\n\
         export function App() { useState(0); Button(); return formatDate('x'); }\n",
    );
    write(
        root,
        "apps/admin/src/page.ts",
        "import { formatDate as fmt } from '@/lib/format';\nexport function page() { return fmt('y'); }\n",
    );
    write(
        root,
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui", "exports": { ".": { "import": "./src/index.ts" } }, "main": "./dist/index.js" }"#,
    );
    write(root, "packages/ui/src/index.ts", "export function Button() { return 1; }\n");
    write(root, "packages/ui/src/hooks.ts", "export function useState(v: number) { return v; }\n");
}

#[test]
fn each_app_resolves_its_alias_through_its_own_config_and_calls_through_it_bind() {
    let tmp = TempDir::new().unwrap();
    monorepo(tmp.path());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        imported_files(rt, "apps/web/src/app.tsx").contains(&"apps/web/src/lib/format.ts".to_string()),
        "web's @/ is web's src: {:?}",
        imported_files(rt, "apps/web/src/app.tsx")
    );
    assert_eq!(imported_files(rt, "apps/admin/src/page.ts"), ["apps/admin/src/lib/format.ts"]);
    assert_eq!(callers_of(rt, "apps/web/src/lib/format.ts:formatDate"), ["apps/web/src/app.tsx:App"]);
    assert_eq!(callers_of(rt, "apps/admin/src/lib/format.ts:formatDate"), ["apps/admin/src/page.ts:page"]);
}

#[test]
fn a_workspace_package_import_binds_to_its_entry_and_a_call_to_the_entrys_export() {
    let tmp = TempDir::new().unwrap();
    monorepo(tmp.path());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "apps/web/src/app.tsx"),
        ["apps/web/src/lib/format.ts", "packages/ui/src/index.ts"]
    );
    assert_eq!(callers_of(rt, "packages/ui/src/index.ts:Button"), ["apps/web/src/app.tsx:App"]);
}

#[test]
fn an_external_package_and_a_call_through_it_stay_unbound_beside_a_same_named_function() {
    let tmp = TempDir::new().unwrap();
    monorepo(tmp.path());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(unresolved(rt, EdgeKind::Imports, "react"));
    assert!(unresolved(rt, EdgeKind::Calls, "react::useState"));
    assert!(
        callers_of(rt, "packages/ui/src/hooks.ts:useState").is_empty(),
        "react's useState reached the workspace decoy"
    );
}

#[test]
fn a_config_edit_rebinds_on_its_sync_and_matches_a_cold_index() {
    config_edit_rebinds_and_matches_a_cold_index("");
}

/// With the config-artifact layer off, no `.json` file is indexed, so nothing
/// but the sync's own descriptor check reports the `tsconfig.json` edit.
#[test]
fn a_config_edit_rebinds_even_when_no_json_file_is_indexed() {
    config_edit_rebinds_and_matches_a_cold_index("[config_artifacts]\nenabled = false\n");
}

fn config_edit_rebinds_and_matches_a_cold_index(logos_config: &str) {
    let fixture = |root: &Path| {
        monorepo(root);
        write(
            root,
            "apps/web/src2/lib/format.ts",
            "export function formatDate(d: string): string { return d; }\n",
        );
        if !logos_config.is_empty() {
            write(root, ".logos/config.toml", logos_config);
        }
    };
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "apps/web/src/lib/format.ts:formatDate"), ["apps/web/src/app.tsx:App"]);

    // Only the config changes: no source file is re-extracted.
    write(
        tmp.path(),
        "apps/web/tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src2/*"] } } }"#,
    );
    engine.sync(&["apps/web/tsconfig.json".into()]);
    let synced_imports = edges_of(rt, EdgeKind::Imports);
    let synced_calls = edges_of(rt, EdgeKind::Calls);
    assert_eq!(callers_of(rt, "apps/web/src2/lib/format.ts:formatDate"), ["apps/web/src/app.tsx:App"]);
    assert!(callers_of(rt, "apps/web/src/lib/format.ts:formatDate").is_empty());
    drop(engine);

    let cold = TempDir::new().unwrap();
    fixture(cold.path());
    write(
        cold.path(),
        "apps/web/tsconfig.json",
        &fs::read_to_string(tmp.path().join("apps/web/tsconfig.json")).unwrap(),
    );
    let cold_engine = index(cold.path());
    let cold_rt = cold_engine.runtime().unwrap();
    assert_eq!(edges_of(cold_rt, EdgeKind::Imports), synced_imports);
    assert_eq!(edges_of(cold_rt, EdgeKind::Calls), synced_calls);
}

/// An `extends` base may be named anything. With no `.json` file indexed, an
/// edit to `configs/base.json` still re-binds on its sync, and the synced graph
/// equals a cold index.
#[test]
fn an_extends_base_edit_rebinds_even_when_no_json_file_is_indexed() {
    let fixture = |root: &Path, base_paths: &str| {
        write(root, ".logos/config.toml", "[config_artifacts]\nenabled = false\n");
        write(root, "tsconfig.json", r#"{ "extends": "./configs/base.json" }"#);
        write(
            root,
            "configs/base.json",
            &format!(r#"{{ "compilerOptions": {{ "baseUrl": "..", "paths": {{ "@/*": ["{base_paths}"] }} }} }}"#),
        );
        for dir in ["src", "src2"] {
            write(
                root,
                &format!("{dir}/lib/format.ts"),
                "export function formatDate(d: string): string { return d; }\n",
            );
        }
        write(
            root,
            "src/app.ts",
            "import { formatDate } from '@/lib/format';\nexport function App() { return formatDate('x'); }\n",
        );
    };
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path(), "./src/*");
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/lib/format.ts:formatDate"), ["src/app.ts:App"]);

    fixture(tmp.path(), "./src2/*");
    engine.sync(&["configs/base.json".into()]);
    let synced = edges_of(rt, EdgeKind::Calls);
    assert_eq!(callers_of(rt, "src2/lib/format.ts:formatDate"), ["src/app.ts:App"]);
    drop(engine);

    let cold = TempDir::new().unwrap();
    fixture(cold.path(), "./src2/*");
    let cold_engine = index(cold.path());
    assert_eq!(edges_of(cold_engine.runtime().unwrap(), EdgeKind::Calls), synced);
}

/// The first rewrite that names anything decides (NFR-RA-05): `./src/*` names
/// both `format.ts` and `format.tsx`, so the import stays unbound rather than
/// falling through to `./src2/*`. A non-code alias (`theme.css`) is never read
/// as the `theme.ts` beside it.
#[test]
fn an_ambiguous_or_non_code_alias_binds_nothing() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "tsconfig.json",
        r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*", "./src2/*"] } } }"#,
    );
    let format = "export function formatDate(d: string): string { return d; }\n";
    for rel in ["src/lib/format.ts", "src/lib/format.tsx", "src2/lib/format.ts"] {
        write(tmp.path(), rel, format);
    }
    write(tmp.path(), "src/styles/theme.ts", "export const theme = 1;\n");
    write(
        tmp.path(),
        "src/app.ts",
        "import { formatDate } from '@/lib/format';\nimport '@/styles/theme.css';\n\
         export function App() { return formatDate('x'); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "src/app.ts").is_empty(), "{:?}", imported_files(rt, "src/app.ts"));
    assert!(unresolved(rt, EdgeKind::Imports, "@::lib::format"));
    assert!(unresolved(rt, EdgeKind::Imports, "@::styles::theme.css"));
    assert!(callers_of(rt, "src2/lib/format.ts:formatDate").is_empty());
}

/// An alias target arriving on a sync binds on that sync, and a workspace
/// package's `package.json` edit moves its entry with no `.json` indexed —
/// each equal to a cold index of the same tree.
#[test]
fn an_alias_target_or_a_manifest_arriving_by_sync_matches_a_cold_index() {
    let logos_config = "[config_artifacts]\nenabled = false\n";
    let tmp = TempDir::new().unwrap();
    monorepo(tmp.path());
    write(tmp.path(), ".logos/config.toml", logos_config);
    fs::remove_file(tmp.path().join("apps/web/src/lib/format.ts")).unwrap();
    write(tmp.path(), "packages/ui/src/next.ts", "export function Button() { return 2; }\n");
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(callers_of(rt, "apps/web/src/lib/format.ts:formatDate").is_empty(), "no target yet");
    assert_eq!(callers_of(rt, "packages/ui/src/index.ts:Button"), ["apps/web/src/app.tsx:App"]);

    write(
        tmp.path(),
        "apps/web/src/lib/format.ts",
        "export function formatDate(d: string): string { return d; }\n",
    );
    write(
        tmp.path(),
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui", "exports": { ".": { "import": "./src/next.ts" } } }"#,
    );
    engine.sync(&["apps/web/src/lib/format.ts".into(), "packages/ui/package.json".into()]);
    let synced = edges_of(rt, EdgeKind::Calls);
    assert_eq!(callers_of(rt, "apps/web/src/lib/format.ts:formatDate"), ["apps/web/src/app.tsx:App"]);
    assert_eq!(callers_of(rt, "packages/ui/src/next.ts:Button"), ["apps/web/src/app.tsx:App"]);
    drop(engine);

    let cold = TempDir::new().unwrap();
    monorepo(cold.path());
    write(cold.path(), ".logos/config.toml", logos_config);
    for rel in ["packages/ui/src/next.ts", "packages/ui/package.json"] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(cold.path());
    assert_eq!(edges_of(cold_engine.runtime().unwrap(), EdgeKind::Calls), synced);
}
