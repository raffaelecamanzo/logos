//! A call through an imported binding resolves to the imported definition
//! (S-440, CR-142 D2, FR-RS-03, NFR-RA-05) — exercised end-to-end through the
//! public [`Engine`] façade against real temp-directory fixtures.
//!
//! [FR-RS-03] binds by scope rules *local → imported → module → global*. Before
//! S-440 the **imported** rung contributed nothing outside Rust: a TypeScript
//! file's `import { navItemsFor } from "./nav"` bound its `Imports` edge
//! (S-439) and its `navItemsFor()` call still bound nowhere, so `callers` and
//! `impact` answered *zero* for every non-Rust symbol.
//!
//! Every fixture here asserts its **import edge already binds** before it
//! asserts the call: the two defects must be provable separately, since that
//! is how one of them stayed hidden behind the other. A fixture whose import
//! binds and whose call does not is exactly the pre-S-440 state.
//!
//! [FR-RS-03]: ../../docs/specs/requirements/FR-RS-03.md

#![cfg(all(feature = "lang-typescript", feature = "lang-go"))]

use std::collections::{HashMap, HashSet};
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

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

/// Every edge of `kind` as `(source file:name, target file:name)`, sorted.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}", n.name))
            })
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

/// The `Calls` edges whose target is the node `file:name`, as source labels.
fn callers_of(rt: &Runtime, target: &str) -> Vec<String> {
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t == target)
        .map(|(s, _)| s)
        .collect()
}

/// The files the `Imports` edges out of `rel` reach, sorted — the S-439
/// precondition every call fixture below asserts first.
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

/// Whether some still-unresolved `Calls` ledger row's target ends with
/// `name` — the never-fabricate outcome: the reference is kept, not dropped.
fn call_row_unresolved(rt: &Runtime, name: &str) -> bool {
    let wanted = name.to_string();
    rt.submit_read(move |store| {
        Ok(store.unresolved_refs()?.into_iter().any(|r| {
            r.kind == EdgeKind::Calls
                && !r.resolved
                && (r.target == wanted || r.target.ends_with(&format!("::{wanted}")))
        }))
    })
    .expect("read runs")
}

const NAV: &str = "export function navItemsFor(role: string): string[] { return [role]; }\n\
export function isAppLevelPath(p: string): boolean { return p === '/'; }\n";

#[test]
fn a_typescript_named_import_call_binds_to_the_imported_definition() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/nav.ts", NAV);
    write(
        tmp.path(),
        "src/menu.ts",
        "import { navItemsFor } from './nav';\nexport function menu() { return navItemsFor('admin'); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/menu.ts"), ["src/nav.ts"], "precondition: the import binds");
    assert_eq!(callers_of(rt, "src/nav.ts:navItemsFor"), ["src/menu.ts:menu"]);
}

#[test]
fn a_tsx_named_import_call_binds_to_the_imported_definition() {
    // This repository's own spelling: an explicit `.ts` extension from a `.tsx`
    // importer, and the call site CR-142 recorded as answering zero.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "web/ui/src/nav.ts", NAV);
    write(
        tmp.path(),
        "web/ui/src/App.tsx",
        "import { isAppLevelPath } from \"./nav.ts\";\n\
export function App({ pathname }: { pathname: string }) {\n  const viewKey = isAppLevelPath(pathname) ? 'app' : 'x';\n  return <div>{viewKey}</div>;\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "web/ui/src/App.tsx"), ["web/ui/src/nav.ts"]);
    assert_eq!(
        callers_of(rt, "web/ui/src/nav.ts:isAppLevelPath"),
        ["web/ui/src/App.tsx:App"]
    );
}

#[test]
fn a_javascript_named_import_call_binds_to_the_imported_definition() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "lib/format.js",
        "export function formatBytes(n) { return `${n} B`; }\n",
    );
    write(
        tmp.path(),
        "lib/report.js",
        "import { formatBytes } from './format.js';\nexport function report(n) { return formatBytes(n); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "lib/report.js"), ["lib/format.js"]);
    assert_eq!(callers_of(rt, "lib/format.js:formatBytes"), ["lib/report.js:report"]);
}

#[test]
fn a_renamed_named_import_binds_to_the_name_it_was_exported_under() {
    // `import { a as b }`: the call spells `b`, the definition is `a`. The
    // imported module also defines an unrelated `b`, which must never be the
    // target — binding the local spelling would fabricate that edge.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/util.ts",
        "export function slugify(s: string) { return s; }\nexport function slug(s: string) { return s; }\n",
    );
    write(
        tmp.path(),
        "src/page.ts",
        "import { slugify as slug } from './util';\nexport function page() { return slug('x'); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/page.ts"), ["src/util.ts"]);
    assert_eq!(callers_of(rt, "src/util.ts:slugify"), ["src/page.ts:page"]);
    assert!(callers_of(rt, "src/util.ts:slug").is_empty());
}

#[test]
fn a_namespace_import_member_call_binds_to_the_imported_definition() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/nav.ts", NAV);
    write(
        tmp.path(),
        "src/menu.ts",
        "import * as nav from './nav';\nexport function menu() { return nav.navItemsFor('admin'); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/menu.ts"), ["src/nav.ts"]);
    assert_eq!(callers_of(rt, "src/nav.ts:navItemsFor"), ["src/menu.ts:menu"]);
}

#[test]
fn a_jsx_component_usage_is_a_call_of_the_component() {
    // CR-142 names `RuleFindingsCard`: its one real caller renders it as JSX in
    // the same file, and a JSX element is how a TSX file calls a component. A
    // component imported from another file is called across the file boundary
    // the same way; an intrinsic element (`<div>`) names no workspace symbol.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Badge.tsx",
        "export function Badge({ tone }: { tone: string }) { return <span>{tone}</span>; }\n",
    );
    write(
        tmp.path(),
        "src/Dashboard.tsx",
        "import { Badge } from './Badge';\n\
export function DashboardView() {\n  return <div><RuleFindingsCard /><Badge tone=\"ok\"></Badge></div>;\n}\n\
function RuleFindingsCard() { return <p>none</p>; }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/Dashboard.tsx"), ["src/Badge.tsx"]);
    assert_eq!(
        callers_of(rt, "src/Dashboard.tsx:RuleFindingsCard"),
        ["src/Dashboard.tsx:DashboardView"]
    );
    assert_eq!(callers_of(rt, "src/Badge.tsx:Badge"), ["src/Dashboard.tsx:DashboardView"]);
}

#[test]
fn a_same_named_export_in_two_modules_binds_only_to_the_one_imported() {
    // `a.ts` and `b.ts` both export `helper`; the importer imports `helper`
    // from `a` and something else from `b`. The import names `a`, so `a` is the
    // one target — never `b`, and never both (NFR-RA-05).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.ts", "export function helper() { return 1; }\n");
    write(
        tmp.path(),
        "src/b.ts",
        "export function helper() { return 2; }\nexport function other() { return 3; }\n",
    );
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './a';\nimport { other } from './b';\nexport function main() { return helper() + other(); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/main.ts"), ["src/a.ts", "src/b.ts"]);
    assert_eq!(callers_of(rt, "src/a.ts:helper"), ["src/main.ts:main"]);
    assert!(callers_of(rt, "src/b.ts:helper").is_empty(), "never the module it was not imported from");
    assert_eq!(callers_of(rt, "src/b.ts:other"), ["src/main.ts:main"]);
}

#[test]
fn an_import_whose_module_does_not_define_the_name_stays_in_the_ledger() {
    // A barrel: `components/index.ts` re-exports `helper` from one of two
    // same-named definitions. The import binds (to the barrel), but the barrel
    // defines no `helper` of its own, so nothing narrows the call to `a` or
    // `b`: it stays unresolved rather than binding to either, or to both.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/components/a.ts", "export function helper() { return 1; }\n");
    write(tmp.path(), "src/components/b.ts", "export function helper() { return 2; }\n");
    write(
        tmp.path(),
        "src/components/index.ts",
        "export { helper } from './a';\n",
    );
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './components';\nexport function main() { return helper(); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/main.ts"), ["src/components/index.ts"]);
    assert!(callers_of(rt, "src/components/a.ts:helper").is_empty());
    assert!(callers_of(rt, "src/components/b.ts:helper").is_empty());
    assert!(call_row_unresolved(rt, "helper"), "the call must stay in unresolved_refs");
}

#[test]
fn a_named_import_from_an_external_package_binds_nothing() {
    // `useState` comes from `react`, which is not in the workspace. A workspace
    // function of the same name in a module the file ALSO imports is not it.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/hooks.ts",
        "export function useState() { return 0; }\nexport function useThing() { return 1; }\n",
    );
    write(
        tmp.path(),
        "src/view.ts",
        "import { useState } from 'react';\nimport { useThing } from './hooks';\nexport function view() { return useState() + useThing(); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/view.ts"), ["src/hooks.ts"]);
    assert_eq!(callers_of(rt, "src/hooks.ts:useThing"), ["src/view.ts:view"]);
    assert!(callers_of(rt, "src/hooks.ts:useState").is_empty());
    assert!(call_row_unresolved(rt, "useState"));
}

fn go_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "go.mod", "module github.com/acme/desk-picker\n\ngo 1.22\n");
    write(
        tmp.path(),
        "internal/admin/admin.go",
        "package admin\n\nfunc Register() {}\n\ntype Server struct{}\n\nfunc (s *Server) Start() {}\n",
    );
    write(
        tmp.path(),
        "internal/admin/routes.go",
        "package admin\n\nfunc Routes() {}\n",
    );
    // A workspace directory named exactly like an external package's last
    // segment, defining the function the external call names.
    write(tmp.path(), "pq/pq.go", "package pq\n\nfunc Open() {}\n");
    tmp
}

#[test]
fn a_go_package_qualified_call_binds_to_the_package_function() {
    let tmp = go_fixture();
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport \"github.com/acme/desk-picker/internal/admin\"\n\nfunc main() {\n\tadmin.Register()\n\tadmin.Routes()\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "cmd/server/main.go"),
        ["internal/admin/admin.go", "internal/admin/routes.go"]
    );
    assert_eq!(callers_of(rt, "internal/admin/admin.go:Register"), ["cmd/server/main.go:main"]);
    assert_eq!(callers_of(rt, "internal/admin/routes.go:Routes"), ["cmd/server/main.go:main"]);
}

#[test]
fn a_go_import_alias_qualifies_the_call() {
    let tmp = go_fixture();
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport adm \"github.com/acme/desk-picker/internal/admin\"\n\nfunc main() {\n\tadm.Register()\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "cmd/server/main.go").len(), 2);
    assert_eq!(callers_of(rt, "internal/admin/admin.go:Register"), ["cmd/server/main.go:main"]);
}

#[test]
fn a_go_call_into_an_external_package_never_binds_a_same_named_workspace_function() {
    // `pq.Open()` is `github.com/lib/pq`'s `Open`. Neither the workspace
    // `pq/pq.go` `Open` nor this file's own `Open` is it: before S-440 the
    // receiver was discarded and the bare `Open` bound the local function.
    let tmp = go_fixture();
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport (\n\t\"context\"\n\n\t\"github.com/lib/pq\"\n)\n\nfunc Open() {}\n\nfunc main() {\n\tpq.Open()\n\tcontext.Background()\n}\n",
    );
    // A workspace directory named like the standard library's `context`: the
    // name hierarchy's suffix match would read `context::Background` as a
    // member of it. An import that binds nothing decides its calls unbound.
    write(
        tmp.path(),
        "context/context.go",
        "package context\n\nfunc Background() {}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "cmd/server/main.go").is_empty(), "an external import binds nothing");
    assert!(callers_of(rt, "cmd/server/main.go:Open").is_empty());
    assert!(callers_of(rt, "pq/pq.go:Open").is_empty());
    assert!(callers_of(rt, "context/context.go:Background").is_empty());
    assert!(call_row_unresolved(rt, "Open"));
    assert!(call_row_unresolved(rt, "Background"));
}

#[test]
fn a_go_method_call_on_a_value_is_not_read_as_a_package_call() {
    // `s.Start()` calls a method on a value; `s` is no import. The receiver
    // discipline of FR-RS-06 is unchanged for it: the package's function-only
    // rung never sees it, and no package method is bound by name.
    let tmp = go_fixture();
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport \"github.com/acme/desk-picker/internal/admin\"\n\nfunc main() {\n\ts := admin.Server{}\n\ts.Start()\n\tadmin.Register()\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "internal/admin/admin.go:Register"), ["cmd/server/main.go:main"]);
    assert!(callers_of(rt, "internal/admin/admin.go:Start").is_empty());
}

#[test]
fn a_cross_file_call_binds_on_the_sync_that_indexes_its_target_and_matches_a_full_index() {
    let tmp = go_fixture();
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport \"github.com/acme/desk-picker/internal/admin\"\n\nfunc main() {\n\tadmin.Audit()\n}\n",
    );
    write(
        tmp.path(),
        "web/src/App.tsx",
        "import { navItemsFor } from './nav';\nexport const App = () => navItemsFor('x');\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(callers_of(rt, "web/src/nav.ts:navItemsFor").is_empty(), "no nav.ts yet");

    write(tmp.path(), "web/src/nav.ts", NAV);
    write(
        tmp.path(),
        "internal/admin/audit.go",
        "package admin\n\nfunc Audit() {}\n",
    );
    engine.sync(&["web/src/nav.ts".into(), "internal/admin/audit.go".into()]);
    let synced = edges_of(rt, EdgeKind::Calls);
    assert!(synced.contains(&("web/src/App.tsx:App".into(), "web/src/nav.ts:navItemsFor".into())));
    assert!(synced.contains(&(
        "cmd/server/main.go:main".into(),
        "internal/admin/audit.go:Audit".into()
    )));
    drop(engine);

    let cold = TempDir::new().unwrap();
    for rel in [
        "go.mod",
        "internal/admin/admin.go",
        "internal/admin/routes.go",
        "internal/admin/audit.go",
        "pq/pq.go",
        "cmd/server/main.go",
        "web/src/App.tsx",
        "web/src/nav.ts",
    ] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(&cold);
    assert_eq!(edges_of(cold_engine.runtime().unwrap(), EdgeKind::Calls), synced);
}

#[test]
fn a_go_mod_edit_rebinds_the_calls_through_its_imports_on_sync() {
    // A `go.mod` carries no node, so no name a sync dirties points at the calls
    // in untouched files that bind through its module path. Renaming the module
    // makes `…/desk-picker/internal/admin` an external path: the import stops
    // binding (S-439), and the call through it must stop binding with it.
    let tmp = go_fixture();
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport \"github.com/acme/desk-picker/internal/admin\"\n\nfunc main() {\n\tadmin.Register()\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "internal/admin/admin.go:Register"), ["cmd/server/main.go:main"]);
    assert!(!call_row_unresolved(rt, "Register"));
    write(tmp.path(), "go.mod", "module github.com/acme/renamed\n\ngo 1.22\n");
    engine.sync(&["go.mod".into()]);
    assert!(
        call_row_unresolved(rt, "Register"),
        "the call through an import that no longer binds must flip to unresolved"
    );
}

#[test]
fn a_removed_definition_unbinds_the_cross_file_call_on_sync() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/nav.ts", NAV);
    write(
        tmp.path(),
        "src/menu.ts",
        "import { navItemsFor } from './nav';\nexport function menu() { return navItemsFor('admin'); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/nav.ts:navItemsFor"), ["src/menu.ts:menu"]);
    write(
        tmp.path(),
        "src/nav.ts",
        "export function isAppLevelPath(p: string): boolean { return p === '/'; }\n",
    );
    engine.sync(&["src/nav.ts".into()]);
    assert!(call_row_unresolved(rt, "navItemsFor"), "the ledger never lies");
}

#[test]
fn a_node_kind_the_rung_binds_is_a_function() {
    // Sanity for the fixtures above: the definitions they bind are functions
    // (an exported arrow `const` included), the kind the rung accepts.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/k.ts", "export const k = () => 1;\n");
    write(
        tmp.path(),
        "src/use.ts",
        "import { k } from './k';\nexport function use() { return k(); }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    let kind = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .find(|n| n.name == "k" && n.kind != NodeKind::Module)
                .map(|n| n.kind))
        })
        .unwrap();
    assert_eq!(kind, Some(NodeKind::Function));
    assert_eq!(callers_of(rt, "src/k.ts:k"), ["src/use.ts:use"]);
}

#[test]
fn a_local_binding_that_shadows_an_import_is_not_a_call_through_it() {
    // A parameter, a destructured prop, or a plain local that reuses the
    // import's name is a value of its own: a call on it names the local, not
    // the import, so no edge to the imported definition may come of it
    // (NFR-RA-05). A call outside that scope still goes through the import.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/nav.ts", NAV);
    write(tmp.path(), "src/Row.tsx", "export function Row() { return <tr />; }\n");
    write(
        tmp.path(),
        "src/menu.ts",
        "import { navItemsFor } from './nav';\n\
export function byParam(navItemsFor: (r: string) => string[]) { return navItemsFor('a'); }\n\
export function byConst() { const navItemsFor = pick(); return navItemsFor('b'); }\n\
export function real() { return navItemsFor('c'); }\n",
    );
    write(
        tmp.path(),
        "src/Table.tsx",
        "import { Row } from './Row';\n\
export function Table({ Row }: { Row: () => null }) { return <table><Row /></table>; }\n\
export function Plain() { return <table><Row /></table>; }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/menu.ts"), ["src/nav.ts"]);
    assert_eq!(callers_of(rt, "src/nav.ts:navItemsFor"), ["src/menu.ts:real"]);
    assert_eq!(callers_of(rt, "src/Row.tsx:Row"), ["src/Table.tsx:Plain"]);
}

#[test]
fn a_go_value_named_like_an_imported_package_is_not_the_package() {
    // `func handle(admin *admin.Server) { admin.Reload() }` calls a method on
    // the parameter; `admin := …; admin.Reload()` on a local. Neither is the
    // package's `Reload`, which `other()` really calls.
    let tmp = go_fixture();
    // The package-level `Reload` and the method `(*Server).Reload` live in two
    // files: one Go file declaring both aborts the index on a duplicate
    // `Contains` edge today, a defect that predates this story.
    write(tmp.path(), "internal/admin/reload.go", "package admin\n\nfunc Reload() {}\n");
    write(
        tmp.path(),
        "internal/admin/server.go",
        "package admin\n\nfunc (s *Server) Reload() {}\n",
    );
    write(
        tmp.path(),
        "cmd/server/main.go",
        "package main\n\nimport \"github.com/acme/desk-picker/internal/admin\"\n\n\
func handle(peer, admin *admin.Server) {\n\tadmin.Reload()\n}\n\n\
func local() {\n\tadmin := &admin.Server{}\n\tadmin.Reload()\n}\n\n\
func other() {\n\tadmin.Reload()\n}\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "internal/admin/reload.go:Reload"), ["cmd/server/main.go:other"]);
}

#[test]
fn a_jsx_tag_naming_a_local_value_is_not_a_call_of_a_same_named_component() {
    // The dynamic-component idiom: `const Icon = icons[name]; <Icon />` renders
    // a value, not the file's own top-level `Icon` component.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Icons.tsx",
        "export function Icon() { return <i />; }\n\
export function Named({ name }: { name: string }) { const Icon = icons[name]; return <Icon />; }\n\
export function Plain() { return <Icon />; }\n",
    );
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/Icons.tsx:Icon"), ["src/Icons.tsx:Plain"]);
}
