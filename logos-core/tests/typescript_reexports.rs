//! TypeScript re-exports are followed, default exports bind, and a literal
//! dynamic import is an import (S-534, FR-RS-19, NFR-RA-05) — exercised
//! end-to-end through the public [`Engine`] façade against real
//! temp-directory fixtures.
//!
//! A name imported from a barrel is followed through `export * from` and
//! `export { a as b } from` transitively, at most 8 hops and stopping at
//! cycles, and binds only when exactly one definition is reached. A default
//! import binds through the module's default-export alias (`export default X`,
//! `export default function X`, `export { X as default }`), or to the
//! `default` symbol of `export default <expression>`.
//!
//! Every call fixture asserts its import edge binds first, as
//! `imported_binding_calls.rs` does: an import that binds and a call that does
//! not is exactly the state before this story.
//!
//! [FR-RS-19]: ../../docs/specs/requirements/FR-RS-19.md

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

/// Whether a still-unresolved `Calls` ledger row's target ends with `::name`.
fn call_row_unresolved(rt: &Runtime, name: &str) -> bool {
    let suffix = format!("::{name}");
    rt.submit_read(move |store| {
        Ok(store
            .unresolved_refs()?
            .into_iter()
            .any(|r| r.kind == EdgeKind::Calls && !r.resolved && r.target.ends_with(&suffix)))
    })
    .expect("read runs")
}

const SPINNER: &str = "export type SpinnerProps = { size?: string };\n\
export const Spinner = ({ size }: SpinnerProps) => <div className={size} />;\n";

#[test]
fn a_component_used_through_an_export_star_barrel_lists_its_users() {
    // bulletproof-react's `components/ui/spinner`: the barrel is one line,
    // `export * from './spinner'`, and every user imports the directory.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/components/ui/spinner/spinner.tsx", SPINNER);
    write(tmp.path(), "src/components/ui/spinner/index.ts", "export * from './spinner';\n");
    write(
        tmp.path(),
        "src/features/list.tsx",
        "import { Spinner } from '../components/ui/spinner';\n\
export const List = () => <Spinner size=\"lg\" />;\n",
    );
    write(
        tmp.path(),
        "src/features/page.tsx",
        "import { Spinner } from '../components/ui/spinner';\n\
export function Page() { return <section><Spinner /></section>; }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "src/features/list.tsx"),
        ["src/components/ui/spinner/index.ts"],
        "precondition: the import binds to the barrel"
    );
    assert_eq!(
        imported_files(rt, "src/components/ui/spinner/index.ts"),
        ["src/components/ui/spinner/spinner.tsx"],
        "the re-export is an import edge of the barrel"
    );
    assert_eq!(
        callers_of(rt, "src/components/ui/spinner/spinner.tsx:Spinner"),
        ["src/features/list.tsx:List", "src/features/page.tsx:Page"]
    );
}

#[test]
fn a_renamed_re_export_binds_to_the_name_it_was_exported_from() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.ts", "export function helper() { return 1; }\n");
    write(tmp.path(), "src/b.ts", "export function aid() { return 2; }\n");
    write(tmp.path(), "src/barrel.ts", "export { helper as aid } from './a';\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import { aid } from './barrel';\nexport function main() { return aid(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/main.ts"), ["src/barrel.ts"]);
    assert_eq!(callers_of(rt, "src/a.ts:helper"), ["src/main.ts:main"]);
    assert!(callers_of(rt, "src/b.ts:aid").is_empty(), "a same-named function the barrel never names");
}

#[test]
fn a_re_export_is_followed_through_several_barrels() {
    // `export *` → `export { x } from` → the definition: two hops.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib/leaf.ts", "export function format() { return ''; }\n");
    write(tmp.path(), "src/lib/inner.ts", "export { format } from './leaf';\n");
    write(tmp.path(), "src/lib/index.ts", "export * from './inner';\n");
    write(
        tmp.path(),
        "src/app.ts",
        "import { format } from './lib';\nexport function app() { return format(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/app.ts"), ["src/lib/index.ts"]);
    assert_eq!(callers_of(rt, "src/lib/leaf.ts:format"), ["src/app.ts:app"]);
}

#[test]
fn a_module_s_own_declaration_outranks_a_name_its_export_star_brings_in() {
    // An explicit export shadows `export *` (ECMAScript): the barrel's own
    // `helper` is the one, never ambiguous with the star's.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.ts", "export function helper() { return 1; }\n");
    write(
        tmp.path(),
        "src/barrel.ts",
        "export * from './a';\nexport function helper() { return 2; }\n",
    );
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './barrel';\nexport function main() { return helper(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/barrel.ts:helper"), ["src/main.ts:main"]);
    assert!(callers_of(rt, "src/a.ts:helper").is_empty());
}

#[test]
fn an_explicit_export_shadows_a_star_even_when_it_reaches_no_call_target() {
    // The barrel exports `X` itself — re-exported from an external package, or
    // its own class — so its `export *` never brings in another `X`, though
    // nothing the barrel's own `X` names is a call target here.
    for barrel in [
        "export { X } from 'some-external-pkg';\nexport * from './custom';\n",
        "export class X {}\nexport * from './custom';\n",
    ] {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "src/custom.ts", "export function X() { return 2; }\n");
        write(tmp.path(), "src/ui.ts", barrel);
        write(
            tmp.path(),
            "src/main.ts",
            "import { X } from './ui';\nexport function main() { return X(); }\n",
        );
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert_eq!(imported_files(rt, "src/main.ts"), ["src/ui.ts"], "{barrel}");
        assert!(callers_of(rt, "src/custom.ts:X").is_empty(), "{barrel}");
        assert!(call_row_unresolved(rt, "X"), "{barrel}");
    }
}

#[test]
fn a_definition_reached_again_through_another_barrel_binds_whatever_the_file_order() {
    // `m` re-exports `x` from `a` and stars `b`, which declares another `x`;
    // `k` re-exports `a`'s `x` too. However the importer's stars are ordered
    // — by file name — `m`'s explicit `x` shadows its star, and the one `x`
    // reached is `a`'s.
    for (first, second) in [("k", "m"), ("m", "z")] {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "src/a.ts", "export function x() { return 1; }\n");
        write(tmp.path(), "src/b.ts", "export function x() { return 2; }\n");
        let (m, k) = if first == "m" { (first, second) } else { (second, first) };
        write(
            tmp.path(),
            &format!("src/{m}.ts"),
            "export { x } from './a';\nexport * from './b';\n",
        );
        write(tmp.path(), &format!("src/{k}.ts"), "export { x } from './a';\n");
        write(
            tmp.path(),
            "src/r.ts",
            &format!("export * from './{first}';\nexport * from './{second}';\n"),
        );
        write(
            tmp.path(),
            "src/use.ts",
            "import { x } from './r';\nexport function use() { return x(); }\n",
        );
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert_eq!(callers_of(rt, "src/a.ts:x"), ["src/use.ts:use"], "{first} before {second}");
        assert!(callers_of(rt, "src/b.ts:x").is_empty(), "{first} before {second}");
    }
}

#[test]
fn a_name_two_export_stars_reach_stays_unbound() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.ts", "export function helper() { return 1; }\n");
    write(tmp.path(), "src/b.ts", "export function helper() { return 2; }\n");
    write(tmp.path(), "src/barrel.ts", "export * from './a';\nexport * from './b';\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './barrel';\nexport function main() { return helper(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/main.ts"), ["src/barrel.ts"]);
    assert!(callers_of(rt, "src/a.ts:helper").is_empty());
    assert!(callers_of(rt, "src/b.ts:helper").is_empty());
    assert!(call_row_unresolved(rt, "helper"), "two definitions reached: the call stays in the ledger");
}

#[test]
fn one_definition_reached_by_two_routes_is_one_definition() {
    // A diamond: both stars re-export the one `helper` of `c`, the first
    // through one barrel more — so `c` is read again from the shorter route,
    // which can reach further, and its `helper` arrives twice.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/c.ts", "export function helper() { return 1; }\n");
    write(tmp.path(), "src/a.ts", "export * from './d';\n");
    write(tmp.path(), "src/d.ts", "export * from './c';\n");
    write(tmp.path(), "src/b.ts", "export { helper } from './c';\n");
    write(tmp.path(), "src/barrel.ts", "export * from './a';\nexport * from './b';\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './barrel';\nexport function main() { return helper(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/c.ts:helper"), ["src/main.ts:main"]);
}

#[test]
fn a_module_first_reached_at_the_cap_is_read_again_from_a_shorter_route() {
    // `x` is reached first through seven barrels, with no hop left to follow
    // its own star to the leaf, then again through one barrel: the second read
    // reaches further and finds `helper`.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/leaf.ts", "export function helper() { return 1; }\n");
    write(tmp.path(), "src/x.ts", "export * from './leaf';\n");
    for i in 1..=7 {
        let next = if i == 7 { "./x".to_string() } else { format!("./a{}", i + 1) };
        write(tmp.path(), &format!("src/a{i}.ts"), &format!("export * from '{next}';\n"));
    }
    write(tmp.path(), "src/b.ts", "export * from './x';\n");
    write(tmp.path(), "src/barrel.ts", "export * from './a1';\nexport * from './b';\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './barrel';\nexport function main() { return helper(); }\n",
    );
    let engine = index(tmp.path());
    assert_eq!(callers_of(engine.runtime().unwrap(), "src/leaf.ts:helper"), ["src/main.ts:main"]);
}

#[test]
fn a_re_export_cycle_stays_unbound_and_ends() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.ts", "export * from './b';\n");
    write(tmp.path(), "src/b.ts", "export * from './a';\nexport { helper } from './a';\n");
    write(tmp.path(), "src/other.ts", "export function helper() { return 1; }\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './a';\nexport function main() { return helper(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/main.ts"), ["src/a.ts"]);
    assert!(callers_of(rt, "src/other.ts:helper").is_empty());
    assert!(call_row_unresolved(rt, "helper"));
}

/// A chain of `hops` barrels in front of `src/leaf.ts`'s `helper`: the
/// importer reaches `barrel0`, which re-exports `barrel1`, …, the last of which
/// re-exports the leaf — each by `export *`, or each by `export { helper }
/// from` when `named`.
fn barrel_chain(hops: usize, named: bool) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/leaf.ts", "export function helper() { return 1; }\n");
    let clause = if named { "{ helper }" } else { "*" };
    for i in 0..hops {
        let next = if i + 1 == hops { "./leaf".to_string() } else { format!("./barrel{}", i + 1) };
        write(tmp.path(), &format!("src/barrel{i}.ts"), &format!("export {clause} from '{next}';\n"));
    }
    write(
        tmp.path(),
        "src/main.ts",
        "import { helper } from './barrel0';\nexport function main() { return helper(); }\n",
    );
    tmp
}

#[test]
fn a_re_export_is_followed_eight_hops_and_no_further() {
    for named in [false, true] {
        let eight = barrel_chain(8, named);
        let engine = index(eight.path());
        assert_eq!(
            callers_of(engine.runtime().unwrap(), "src/leaf.ts:helper"),
            ["src/main.ts:main"],
            "named: {named}"
        );
        drop(engine);

        let nine = barrel_chain(9, named);
        let engine = index(nine.path());
        let rt = engine.runtime().unwrap();
        assert!(callers_of(rt, "src/leaf.ts:helper").is_empty(), "nine hops is past the cap (named: {named})");
        assert!(call_row_unresolved(rt, "helper"), "named: {named}");
    }
}

#[test]
fn a_default_import_binds_through_the_default_export_alias() {
    // bulletproof-react's route modules: `const AppRoot = …; export default
    // AppRoot;`, imported both as `{ default as AppRoot }` and as a default.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/routes/root.tsx",
        "export const AppRootErrorBoundary = () => <div />;\n\
const AppRoot = () => <main />;\n\
export default AppRoot;\n",
    );
    write(
        tmp.path(),
        "src/router.tsx",
        "import { default as AppRoot, AppRootErrorBoundary } from './routes/root';\n\
export const Router = () => <AppRoot />;\n",
    );
    write(
        tmp.path(),
        "src/shell.tsx",
        "import Root from './routes/root';\nexport function Shell() { return Root(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/router.tsx"), ["src/routes/root.tsx"]);
    assert_eq!(
        callers_of(rt, "src/routes/root.tsx:AppRoot"),
        ["src/router.tsx:Router", "src/shell.tsx:Shell"]
    );
}

#[test]
fn a_local_export_alias_is_an_import_edge_to_the_declaration_it_names() {
    // `export default X`, `export default function X` and `export { x as
    // default }` each bind their row to the declaration in their own file.
    for (src, target) in [
        ("const App = () => 1;\nexport default App;\n", "App"),
        ("export default function greet() { return 1; }\n", "greet"),
        ("function quiet() { return 0; }\nexport { quiet as default };\n", "quiet"),
    ] {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "src/m.ts", src);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        let edges: Vec<(String, String)> = edges_of(rt, EdgeKind::Imports);
        assert_eq!(edges, [("src/m.ts:m".to_string(), format!("src/m.ts:{target}"))], "{src}");
    }
}

#[test]
fn a_default_export_of_a_value_beside_a_same_named_type_aliases_the_value() {
    // TypeScript's one name in two declaration spaces: `type Button` and
    // `const Button`. The export alias, and the call through it, are the value.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/button.tsx",
        "type Button = { a: number };\nconst Button = () => <b />;\nexport default Button;\n",
    );
    write(
        tmp.path(),
        "src/page.tsx",
        "import Button from './button';\nexport const Page = () => <Button />;\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let alias: Vec<(String, String)> = edges_of(rt, EdgeKind::Imports)
        .into_iter()
        .filter(|(s, _)| s.starts_with("src/button.tsx:"))
        .collect();
    assert_eq!(alias, [("src/button.tsx:button".to_string(), "src/button.tsx:Button".to_string())]);
    assert_eq!(callers_of(rt, "src/button.tsx:Button"), ["src/page.tsx:Page"]);
}

#[test]
fn a_default_import_binds_to_a_default_exported_declaration() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/greet.ts", "export default function greet() { return 'hi'; }\n");
    write(tmp.path(), "src/rename.ts", "function quiet() { return ''; }\nexport { quiet as default };\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import hello from './greet';\nimport hush from './rename';\n\
export function main() { return hello() + hush(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/greet.ts:greet"), ["src/main.ts:main"]);
    assert_eq!(callers_of(rt, "src/rename.ts:quiet"), ["src/main.ts:main"]);
}

#[test]
fn a_default_import_of_an_exported_expression_binds_to_its_default_symbol() {
    // nestjs-boilerplate's config modules: `export default registerAs(…)`,
    // called as `databaseConfig()` through a default import.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/database/config/database.config.ts",
        "import { registerAs } from '@nestjs/config';\n\
export default registerAs('database', () => ({ host: 'h' }));\n",
    );
    write(
        tmp.path(),
        "src/app.module.ts",
        "import databaseConfig from './database/config/database.config';\n\
export function load() { return [databaseConfig()]; }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/app.module.ts"), ["src/database/config/database.config.ts"]);
    assert_eq!(
        callers_of(rt, "src/database/config/database.config.ts:default"),
        ["src/app.module.ts:load"]
    );
}

#[test]
fn a_default_re_exported_by_a_barrel_binds() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/ui/button.tsx", "const Button = () => <button />;\nexport default Button;\n");
    write(tmp.path(), "src/ui/card.tsx", "export default function Card() { return <div />; }\n");
    write(
        tmp.path(),
        "src/ui/index.ts",
        "export { default } from './button';\nexport { default as Card } from './card';\n",
    );
    write(
        tmp.path(),
        "src/page.tsx",
        "import Button, { Card } from './ui';\nexport const Page = () => <Card><Button /></Card>;\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/page.tsx"), ["src/ui/index.ts"]);
    assert_eq!(callers_of(rt, "src/ui/button.tsx:Button"), ["src/page.tsx:Page"]);
    assert_eq!(callers_of(rt, "src/ui/card.tsx:Card"), ["src/page.tsx:Page"]);
}

#[test]
fn an_export_star_never_brings_in_a_default() {
    // `export *` re-exports every name but `default` (ECMAScript).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.ts", "export default function helper() { return 1; }\n");
    write(tmp.path(), "src/barrel.ts", "export * from './a';\n");
    write(
        tmp.path(),
        "src/main.ts",
        "import helper from './barrel';\nexport function main() { return helper(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(callers_of(rt, "src/a.ts:helper").is_empty());
    assert!(call_row_unresolved(rt, "default"));
}

#[test]
fn an_imported_name_a_module_exports_again_is_followed() {
    // `import { a } from './x'; export { a };` and `export default` of an
    // imported name are re-exports too.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/x.ts", "export function a() { return 1; }\nexport function b() { return 2; }\n");
    write(
        tmp.path(),
        "src/hub.ts",
        "import { a, b } from './x';\nexport { a };\nexport default b;\n",
    );
    write(
        tmp.path(),
        "src/main.ts",
        "import second, { a } from './hub';\nexport function main() { return a() + second(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/x.ts:a"), ["src/main.ts:main"]);
    assert_eq!(callers_of(rt, "src/x.ts:b"), ["src/main.ts:main"]);
}

#[test]
fn an_imported_class_is_never_a_call_target() {
    // A class is reached through `new` or its members (S-535, S-536), never
    // called: a named or default import of one used as `<K />` binds nothing.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/k.tsx", "export class K { render() { return <i />; } }\n");
    write(tmp.path(), "src/d.tsx", "export default class D { render() { return <i />; } }\n");
    write(
        tmp.path(),
        "src/page.tsx",
        "import { K } from './k';\nimport D from './d';\nexport const Page = () => <K><D /></K>;\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/page.tsx"), ["src/d.tsx", "src/k.tsx"]);
    assert!(callers_of(rt, "src/k.tsx:K").is_empty());
    assert!(callers_of(rt, "src/d.tsx:D").is_empty());
}

#[test]
fn a_literal_dynamic_import_records_an_import_edge() {
    // bulletproof-react's lazy routes: `lazy: () => import('./routes/landing')`.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/routes/landing.tsx", "export default function Landing() { return <div />; }\n");
    write(tmp.path(), "src/routes/login.tsx", "export default function Login() { return <div />; }\n");
    write(
        tmp.path(),
        "src/router.tsx",
        "const name = 'login';\n\
export const routes = [\n  { lazy: () => import('./routes/landing') },\n  { lazy: () => import(`./routes/${name}`) },\n];\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "src/router.tsx"),
        ["src/routes/landing.tsx"],
        "a literal specifier is an import; a template is not"
    );
}

#[test]
fn a_definition_added_behind_a_barrel_binds_on_sync_and_matches_a_full_index() {
    // Only the leaf changes: the barrel and the importer are untouched, so
    // nothing they spell is dirty — the call must still re-bind (sync ≡
    // reindex).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib/leaf.ts", "export function other() { return 0; }\n");
    write(tmp.path(), "src/lib/index.ts", "export * from './leaf';\n");
    write(
        tmp.path(),
        "src/app.ts",
        "import { format } from './lib';\nexport function app() { return format(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(callers_of(rt, "src/lib/leaf.ts:format").is_empty(), "no format yet");

    write(
        tmp.path(),
        "src/lib/leaf.ts",
        "export function other() { return 0; }\nexport function format() { return ''; }\n",
    );
    engine.sync(&["src/lib/leaf.ts".into()]);
    let synced = (edges_of(rt, EdgeKind::Calls), edges_of(rt, EdgeKind::Imports));
    assert_eq!(callers_of(rt, "src/lib/leaf.ts:format"), ["src/app.ts:app"]);
    drop(engine);

    let cold = TempDir::new().unwrap();
    for rel in ["src/lib/leaf.ts", "src/lib/index.ts", "src/app.ts"] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(cold.path());
    let cold_rt = cold_engine.runtime().unwrap();
    assert_eq!((edges_of(cold_rt, EdgeKind::Calls), edges_of(cold_rt, EdgeKind::Imports)), synced);
}

#[test]
fn a_default_alias_edit_rebinds_the_default_import_on_sync_and_matches_a_full_index() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/root.tsx",
        "const First = () => <main />;\nconst Second = () => <aside />;\nexport default First;\n",
    );
    write(
        tmp.path(),
        "src/router.tsx",
        "import Root from './root';\nexport const Router = () => <Root />;\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/root.tsx:First"), ["src/router.tsx:Router"]);

    write(
        tmp.path(),
        "src/root.tsx",
        "const First = () => <main />;\nconst Second = () => <aside />;\nexport default Second;\n",
    );
    engine.sync(&["src/root.tsx".into()]);
    assert_eq!(callers_of(rt, "src/root.tsx:Second"), ["src/router.tsx:Router"]);
    assert!(callers_of(rt, "src/root.tsx:First").is_empty());
    let synced = (edges_of(rt, EdgeKind::Calls), edges_of(rt, EdgeKind::Imports));
    drop(engine);

    let cold = TempDir::new().unwrap();
    for rel in ["src/root.tsx", "src/router.tsx"] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(cold.path());
    let cold_rt = cold_engine.runtime().unwrap();
    assert_eq!((edges_of(cold_rt, EdgeKind::Calls), edges_of(cold_rt, EdgeKind::Imports)), synced);
}
