//! A module specifier binds as a path, not as a member expression (S-439,
//! CR-142 D1, FR-RS-01, NFR-RA-05) — exercised end-to-end through the public
//! [`Engine`] façade against real temp-directory fixtures.
//!
//! Before S-439 every TypeScript/TSX/JavaScript/Go import was canonicalised
//! with the member-path grammar: `"./nav.ts"` was recorded `nav::ts` and
//! `"github.com/org/repo/internal/admin"` had its host split, so no import in
//! those languages could ever reach a file node. These fixtures pin the
//! corrected contract:
//!
//! - a relative import **with** an explicit extension and one **without** bind
//!   to the same target file — two separate fixtures, because that spelling
//!   difference is what produced 0 bound imports in one corpus and 9 in
//!   another, so neither may be inferred from the other;
//! - a Go intra-module import binds to its package — every non-test `.go` file
//!   of the directory the `go.mod`-anchored path names;
//! - an external package import (`react`, `context`, `github.com/lib/pq`)
//!   binds nothing and stays in `unresolved_refs`, even beside a workspace
//!   directory that shares its last segment.

#![cfg(all(feature = "lang-typescript", feature = "lang-go"))]

use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The file-root module node of the file at `rel`.
fn file_module(rt: &Runtime, rel: &str) -> NodeId {
    let wanted = rel.to_string();
    rt.submit_read(move |store| {
        let parented: std::collections::HashSet<NodeId> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Contains)
            .map(|e| e.target)
            .collect();
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|n| {
                n.kind == NodeKind::Module
                    && !parented.contains(&n.id)
                    && n.file_path.as_deref() == Some(wanted.as_str())
            })
            .map(|n| n.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no file module for {rel}"))
}

/// The file paths the `Imports` edges out of `rel`'s file module reach, sorted.
fn imported_files(rt: &Runtime, rel: &str) -> Vec<String> {
    let source = file_module(rt, rel);
    rt.submit_read(move |store| {
        let path_of: std::collections::HashMap<NodeId, Option<String>> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.file_path))
            .collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Imports && e.source == source)
            .filter_map(|e| path_of.get(&e.target).cloned().flatten())
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// Whether the `Imports` ledger row with `target` is still unresolved.
fn import_row_unresolved(rt: &Runtime, target: &str) -> bool {
    let wanted = target.to_string();
    let rows: Vec<bool> = rt
        .submit_read(move |store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Imports && r.target == wanted)
                .map(|r| r.resolved)
                .collect())
        })
        .expect("read runs");
    assert!(!rows.is_empty(), "no ledger row for import {target:?}");
    rows.iter().all(|resolved| !resolved)
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

const NAV: &str = "export function navItemsFor(role: string): string[] { return [role]; }\n";
const HEADER: &str = "export default function Header() { return <header />; }\n";

#[test]
fn a_relative_import_with_an_explicit_extension_binds_to_its_file() {
    // This repository's SPA spelling: all 408 of its relative imports carry the
    // extension, and before S-439 exactly 0 of them bound.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "web/ui/src/nav.ts", NAV);
    write(tmp.path(), "web/ui/src/shell/Header.tsx", HEADER);
    write(
        tmp.path(),
        "web/ui/src/App.tsx",
        "import { navItemsFor } from \"./nav.ts\";\nimport Header from \"./shell/Header.tsx\";\nexport const App = () => navItemsFor(\"x\");\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "web/ui/src/App.tsx"),
        ["web/ui/src/nav.ts", "web/ui/src/shell/Header.tsx"]
    );
}

#[test]
fn a_relative_import_without_an_extension_binds_to_the_same_file() {
    // desk-picker's spelling, on its own fixture: `./nav` reaches the same
    // `nav.ts` the explicit-extension fixture reaches, and a nested
    // extension-less path reaches a `.tsx`.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "web/ui/src/nav.ts", NAV);
    write(
        tmp.path(),
        "web/ui/src/auth/AuthContext.tsx",
        "export function useAuth() { return null; }\n",
    );
    write(
        tmp.path(),
        "web/ui/src/App.tsx",
        "import { navItemsFor } from './nav';\nimport { useAuth } from './auth/AuthContext';\nexport const App = () => navItemsFor('x');\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "web/ui/src/App.tsx"),
        ["web/ui/src/auth/AuthContext.tsx", "web/ui/src/nav.ts"]
    );
}

#[test]
fn a_relative_import_resolves_parent_hops_and_directory_index_files() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "web/ui/src/api.ts",
        "export const get = () => 1;\n",
    );
    write(
        tmp.path(),
        "web/ui/src/components/index.ts",
        "export const Button = 1;\n",
    );
    write(
        tmp.path(),
        "web/ui/src/pages/Home.tsx",
        "import { get } from '../api.js';\nimport { Button } from '../components';\nexport const Home = () => get();\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // `../api.js` is the ESM spelling of `api.ts`: `.js` is a declared
    // specifier extension, so it names the file by its stem.
    assert_eq!(
        imported_files(rt, "web/ui/src/pages/Home.tsx"),
        ["web/ui/src/api.ts", "web/ui/src/components/index.ts"]
    );
}

#[test]
fn a_javascript_relative_require_binds_to_its_file() {
    // `.js` rides the typescript grammar; neither CR-142 corpus holds a relative
    // JavaScript import, so this arm is evidenced here and nowhere else.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "server/lib/util.js", "module.exports = { f() {} };\n");
    write(
        tmp.path(),
        "server/app.js",
        "const util = require('./lib/util.js');\nconst express = require('express');\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "server/app.js"), ["server/lib/util.js"]);
    assert!(import_row_unresolved(rt, "express"));
}

#[test]
fn an_ambiguous_or_non_code_relative_import_binds_nothing() {
    let tmp = TempDir::new().unwrap();
    // Two files share the stem `nav`: `./nav` could mean either, so it is an
    // ambiguity, never a pick.
    write(tmp.path(), "src/nav.ts", NAV);
    write(tmp.path(), "src/nav.tsx", HEADER);
    // `styles.ts` exists, but `./styles.css` names the stylesheet — an
    // undeclared extension is kept and never read as the code file.
    write(tmp.path(), "src/styles.ts", "export const s = 1;\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import { navItemsFor } from './nav';\nimport './styles.css';\nimport x from '../../outside';\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "src/main.ts").is_empty());
    assert!(import_row_unresolved(rt, ".::nav"));
    assert!(import_row_unresolved(rt, ".::styles.css"));
    assert!(import_row_unresolved(rt, "..::..::outside"));
}

/// A Go module whose package `internal/admin` has two source files, a test file
/// and a TypeScript file, plus decoys: directories whose names equal the last segment of external
/// imports (`pq/`, `context/`) — the shape a directory-suffix matcher would bind
/// — and a root `context.go`, the shape the member-path hierarchy would bind.
fn go_module_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "go.mod",
        "module github.com/acme/desk-picker\n\ngo 1.22\n",
    );
    write(
        tmp.path(),
        "internal/admin/admin.go",
        "package admin\n\nfunc Register() {}\n",
    );
    write(
        tmp.path(),
        "internal/admin/routes.go",
        "package admin\n\nfunc routes() {}\n",
    );
    write(
        tmp.path(),
        "internal/admin/admin_test.go",
        "package admin\n\nfunc helper() {}\n",
    );
    // A non-Go code file in the package directory is not part of the package.
    write(
        tmp.path(),
        "internal/admin/helper.ts",
        "export const h = 1;\n",
    );
    write(tmp.path(), "pq/pq.go", "package pq\n\nfunc Open() {}\n");
    write(
        tmp.path(),
        "context/context.go",
        "package context\n\nfunc Background() {}\n",
    );
    // A root-level file whose stem is an external import's whole path: the
    // member-path hierarchy reads `context` as a name and would bind it here.
    write(
        tmp.path(),
        "context.go",
        "package shop\n\nfunc withTimeout() {}\n",
    );
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport (\n\t\"context\"\n\t\"net/http\"\n\n\t\"github.com/lib/pq\"\n\t\"github.com/acme/desk-picker/internal/admin\"\n)\n\nfunc main() {\n\tadmin.Register()\n}\n",
    );
    tmp
}

#[test]
fn a_go_intra_module_import_binds_to_its_package_files() {
    let tmp = go_module_fixture();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // The dotted host is compared whole against the go.mod module path; the
    // package is every non-test file of `internal/admin` — not the test file,
    // and not the decoys.
    assert_eq!(
        imported_files(rt, "cmd/server/main.go"),
        ["internal/admin/admin.go", "internal/admin/routes.go"]
    );
}

#[test]
fn an_external_package_import_binds_nothing_and_stays_in_the_ledger() {
    // The never-fabricate rule is not relaxed (NFR-RA-05). Go: `pq/` and
    // `context/` are workspace directories named exactly like the external
    // packages' last segments, and still nothing binds to them.
    let tmp = go_module_fixture();
    // TypeScript: `react` beside a workspace `react.ts` file.
    write(tmp.path(), "web/src/react.ts", "export const x = 1;\n");
    write(
        tmp.path(),
        "web/src/App.tsx",
        "import React from 'react';\nexport const App = () => React;\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    for external in ["github.com::lib::pq", "context", "net::http", "react"] {
        assert!(
            import_row_unresolved(rt, external),
            "the external import {external:?} must stay in unresolved_refs"
        );
    }
    let pq = file_module(rt, "pq/pq.go");
    let ctx = file_module(rt, "context/context.go");
    let root_ctx = file_module(rt, "context.go");
    let inbound: Vec<(NodeId, NodeId)> = rt
        .submit_read(|store| {
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Imports)
                .map(|e| (e.source, e.target))
                .collect())
        })
        .expect("read runs");
    assert!(
        !inbound.iter().any(|&(_, t)| t == pq || t == ctx || t == root_ctx),
        "a decoy directory sharing an external import's last segment bound: {inbound:?}"
    );
    assert!(imported_files(rt, "web/src/App.tsx").is_empty());
}

#[test]
fn a_go_import_outside_any_declared_module_binds_nothing() {
    // No go.mod: nothing anchors the module path, so an import path that reads
    // like an intra-module one is not bound on a guess.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "internal/admin/admin.go",
        "package admin\n\nfunc Register() {}\n",
    );
    write(
        tmp.path(),
        "cmd/main.go",
        "package main\n\nimport \"github.com/acme/desk-picker/internal/admin\"\n\nfunc main() { admin.Register() }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "cmd/main.go").is_empty());
    assert!(import_row_unresolved(
        rt,
        "github.com::acme::desk-picker::internal::admin"
    ));
}

#[test]
fn an_import_binds_on_the_sync_that_indexes_its_target_and_matches_a_full_index() {
    // The incremental re-bind selects a row by the tokens of its target; a
    // relative specifier (`.::nav`) and a Go path (`…::internal::admin`) must
    // both be picked up when their target file arrives, and the synced graph
    // must equal a cold index of the same tree (the CR-015 equivalence).
    let tmp = go_module_fixture();
    write(
        tmp.path(),
        "web/src/App.tsx",
        "import { navItemsFor } from './nav';\nexport const App = () => navItemsFor('x');\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(
        imported_files(rt, "web/src/App.tsx").is_empty(),
        "no nav.ts yet"
    );

    write(tmp.path(), "web/src/nav.ts", NAV);
    write(
        tmp.path(),
        "internal/admin/audit.go",
        "package admin\n\nfunc audit() {}\n",
    );
    engine.sync(&["web/src/nav.ts".into(), "internal/admin/audit.go".into()]);
    let synced_ts = imported_files(rt, "web/src/App.tsx");
    let synced_go = imported_files(rt, "cmd/server/main.go");
    assert_eq!(synced_ts, ["web/src/nav.ts"]);
    assert_eq!(
        synced_go,
        [
            "internal/admin/admin.go",
            "internal/admin/audit.go",
            "internal/admin/routes.go"
        ]
    );
    drop(engine);

    let cold = TempDir::new().unwrap();
    for rel in [
        "go.mod",
        "internal/admin/admin.go",
        "internal/admin/routes.go",
        "internal/admin/admin_test.go",
        "internal/admin/audit.go",
        "pq/pq.go",
        "context/context.go",
        "context.go",
        "cmd/server/main.go",
        "web/src/App.tsx",
        "web/src/nav.ts",
    ] {
        write(
            cold.path(),
            rel,
            &fs::read_to_string(tmp.path().join(rel)).unwrap(),
        );
    }
    let cold_engine = index(&cold);
    let cold_rt = cold_engine.runtime().unwrap();
    assert_eq!(imported_files(cold_rt, "web/src/App.tsx"), synced_ts);
    assert_eq!(imported_files(cold_rt, "cmd/server/main.go"), synced_go);
}

#[test]
fn a_non_go_import_never_binds_into_a_go_module_of_the_same_name() {
    // A monorepo whose Go backend is `module shared` and whose TS frontend
    // imports an npm package (or alias) also called `shared`: the TS specifier
    // must not be read as a Go import path, however exactly it matches.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "go.mod", "module shared\n");
    write(tmp.path(), "shared.go", "package shared\n\nfunc Root() {}\n");
    write(tmp.path(), "pkg/util/util.go", "package util\n\nfunc U() {}\n");
    write(
        tmp.path(),
        "web/src/App.ts",
        "import client from 'shared';\nimport u from 'shared/pkg/util';\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "web/src/App.ts").is_empty());
    assert!(import_row_unresolved(rt, "shared"));
    assert!(import_row_unresolved(rt, "shared::pkg::util"));
}

#[test]
fn a_relative_import_binds_only_within_its_own_language() {
    // A TS `./helper` beside only a `helper.py` names nothing a TypeScript
    // resolver can load; and a `util.py` beside `util.ts` must neither answer
    // `./util` nor make it ambiguous.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "web/src/helper.py", "def h():\n    pass\n");
    write(tmp.path(), "web/src/util.ts", "export const u = 1;\n");
    write(tmp.path(), "web/src/util.py", "def u():\n    pass\n");
    write(
        tmp.path(),
        "web/src/App.ts",
        "import h from './helper';\nimport { u } from './util';\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "web/src/App.ts"), ["web/src/util.ts"]);
    assert!(import_row_unresolved(rt, ".::helper"));
}

#[test]
fn a_go_import_binds_in_the_importers_own_module_and_the_longest_module_path() {
    let tmp = TempDir::new().unwrap();
    // Three example modules that all declare `module example`: only the
    // importer's own `go.mod` says which one `example/internal/x` means.
    for m in ["a", "b", "c"] {
        write(tmp.path(), &format!("examples/{m}/go.mod"), "module example\n");
    }
    write(tmp.path(), "examples/a/internal/x/x.go", "package x\n\nfunc X() {}\n");
    write(tmp.path(), "examples/c/internal/x/x.go", "package x\n\nfunc X() {}\n");
    let importer = "package main\n\nimport \"example/internal/x\"\n\nfunc main() { x.X() }\n";
    write(tmp.path(), "examples/b/main.go", importer);
    write(tmp.path(), "examples/c/main.go", importer);
    // A nested module owns its subtree: `example.com/shop/tools/gen` lives
    // under `x/tools`, not at the `tools/gen` the root module would spell.
    write(tmp.path(), "go.mod", "module example.com/shop\n");
    write(tmp.path(), "x/tools/go.mod", "module example.com/shop/tools\n");
    write(tmp.path(), "x/tools/gen/gen.go", "package gen\n\nfunc G() {}\n");
    write(tmp.path(), "tools/gen/decoy.go", "package gen\n\nfunc G() {}\n");
    write(
        tmp.path(),
        "cmd/main.go",
        "package main\n\nimport \"example.com/shop/tools/gen\"\n\nfunc main() { gen.G() }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(
        imported_files(rt, "examples/b/main.go").is_empty(),
        "b's own module has no internal/x; another module's must not stand in"
    );
    assert_eq!(
        imported_files(rt, "examples/c/main.go"),
        ["examples/c/internal/x/x.go"]
    );
    assert_eq!(imported_files(rt, "cmd/main.go"), ["x/tools/gen/gen.go"]);
}

#[test]
fn a_sync_rebinds_imports_no_token_of_theirs_names() {
    // Three shapes the name-token re-bind selection cannot see (S-439 review):
    // `'.'` spells no token; a file added to a Go module's ROOT package shares
    // no token with the module path; a `go.mod` carries no node at all. Each
    // must bind on the sync that brings its evidence, exactly as a cold index
    // of the same tree binds it (CR-015).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "web/src/App.tsx", "import { x } from '.';\nexport const A = x;\n");
    write(tmp.path(), "shop.go", "package shop\n\nfunc Open() {}\n");
    write(
        tmp.path(),
        "cmd/main.go",
        "package main\n\nimport \"github.com/acme/shop\"\n\nfunc main() { shop.Open() }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "web/src/App.tsx").is_empty(), "no index.ts yet");
    assert!(imported_files(rt, "cmd/main.go").is_empty(), "no go.mod yet");

    write(tmp.path(), "go.mod", "module github.com/acme/shop\n");
    engine.sync(&["go.mod".into()]);
    assert_eq!(imported_files(rt, "cmd/main.go"), ["shop.go"], "go.mod arrived");

    write(tmp.path(), "web/src/index.ts", "export const x = 1;\n");
    write(tmp.path(), "util.go", "package shop\n\nfunc helper() {}\n");
    engine.sync(&["web/src/index.ts".into(), "util.go".into()]);
    let synced_ts = imported_files(rt, "web/src/App.tsx");
    let synced_go = imported_files(rt, "cmd/main.go");
    assert_eq!(synced_ts, ["web/src/index.ts"]);
    assert_eq!(synced_go, ["shop.go", "util.go"]);
    drop(engine);

    let cold = TempDir::new().unwrap();
    for rel in ["web/src/App.tsx", "web/src/index.ts", "shop.go", "util.go", "cmd/main.go", "go.mod"] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(&cold);
    let cold_rt = cold_engine.runtime().unwrap();
    assert_eq!(imported_files(cold_rt, "web/src/App.tsx"), synced_ts);
    assert_eq!(imported_files(cold_rt, "cmd/main.go"), synced_go);
}

#[test]
fn a_multi_dot_file_name_keeps_every_dot_but_its_extension() {
    // `nav.test.ts` beside `nav.ts` is the shape real trees are full of
    // (`*.test.ts`, `*.module.ts`, `vite.config.ts`): the stem is everything
    // but the LAST extension, on both the ledger and the file side, or `./nav`
    // turns ambiguous and `./nav.test` names nothing.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "web/src/nav.ts", NAV);
    write(tmp.path(), "web/src/nav.test.ts", "export const t = 1;\n");
    write(
        tmp.path(),
        "web/src/a.ts",
        "import { navItemsFor } from './nav';\n",
    );
    write(
        tmp.path(),
        "web/src/b.ts",
        "import { t } from './nav.test.ts';\n",
    );
    write(tmp.path(), "web/src/c.ts", "import { t } from './nav.test';\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "web/src/a.ts"), ["web/src/nav.ts"]);
    assert_eq!(imported_files(rt, "web/src/b.ts"), ["web/src/nav.test.ts"]);
    assert_eq!(imported_files(rt, "web/src/c.ts"), ["web/src/nav.test.ts"]);
}

#[test]
fn a_go_module_path_matches_only_on_a_whole_segment() {
    // `github.com/acme/desk-pickerx/foo` shares a byte prefix with the module
    // `github.com/acme/desk-picker`, not a segment prefix. Read as a raw prefix
    // its rest would be `x/foo` — and a workspace `x/foo/` exists to catch that.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "go.mod", "module github.com/acme/desk-picker\n");
    write(tmp.path(), "x/foo/foo.go", "package foo\n\nfunc F() {}\n");
    write(
        tmp.path(),
        "cmd/main.go",
        "package main\n\nimport \"github.com/acme/desk-pickerx/foo\"\n\nfunc main() { foo.F() }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "cmd/main.go").is_empty());
    assert!(import_row_unresolved(rt, "github.com::acme::desk-pickerx::foo"));
}
