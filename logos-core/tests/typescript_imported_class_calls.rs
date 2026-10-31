//! A TypeScript call through an imported class binds among that class's
//! methods (S-536, FR-RS-20, NFR-RA-05) — exercised end-to-end through the
//! public [`Engine`] façade against real temp-directory fixtures.
//!
//! `UserMapper.toDomain(raw)` after `import { UserMapper } from
//! '../mappers/user.mapper'` is recorded as
//! `<module>::UserMapper::toDomain` and binds to the one callable of that
//! name among the members of the one class the import reaches. A receiver
//! naming an external package's class, an imported function or value, or a
//! name that reaches two classes stays unbound; a namespace import keeps its
//! `<module>::<name>` qualification.
//!
//! Every call fixture asserts its import edge binds first, as
//! `typescript_reexports.rs` does: an import that binds and a call that does
//! not is exactly the state before this story.
//!
//! [FR-RS-20]: ../../docs/specs/requirements/FR-RS-20.md

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

/// The `Calls` edges out of the node `file:name`, as target labels.
fn callees_of(rt: &Runtime, source: &str) -> Vec<String> {
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(s, _)| s == source)
        .map(|(_, t)| t)
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

/// The kind of every `Calls` edge's target, in edge order — what tells a
/// class's method from a module function of the same name and file.
fn call_target_kinds(rt: &Runtime) -> Vec<String> {
    rt.submit_read(|store| {
        let kind: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{:?}", n.kind)))
            .collect();
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| kind[&e.target].clone())
            .collect())
    })
    .expect("read runs")
}

/// Whether a still-unresolved `Calls` ledger row's target is `target`.
fn call_row_unresolved(rt: &Runtime, target: &str) -> bool {
    let target = target.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .unresolved_refs()?
            .into_iter()
            .any(|r| r.kind == EdgeKind::Calls && !r.resolved && r.target == target))
    })
    .expect("read runs")
}

/// nestjs-boilerplate's relational user mapper, trimmed: two static methods,
/// one of which calls another imported mapper.
const USER_MAPPER: &str = "import { FileMapper } from '../../../../files/mappers/file.mapper';\n\
export class UserMapper {\n\
  static toDomain(raw: any): any {\n\
    return { photo: FileMapper.toDomain(raw.photo) };\n\
  }\n\
  static toPersistence(domainEntity: any): any {\n\
    return { id: domainEntity.id };\n\
  }\n\
}\n";

const FILE_MAPPER: &str = "export class FileMapper {\n\
  static toDomain(raw: any): any {\n\
    return raw;\n\
  }\n\
}\n";

/// The repository calling both mapper methods, as nest's does.
const USER_REPOSITORY: &str = "import { UserMapper } from '../mappers/user.mapper';\n\
export class UsersRelationalRepository {\n\
  async create(data: any): Promise<any> {\n\
    const persistenceModel = UserMapper.toPersistence(data);\n\
    return UserMapper.toDomain(persistenceModel);\n\
  }\n\
  async findById(id: any): Promise<any> {\n\
    return UserMapper.toDomain(id);\n\
  }\n\
}\n";

const MAPPER_REL: &str = "src/users/persistence/relational/mappers/user.mapper.ts";
const REPOSITORY_REL: &str = "src/users/persistence/relational/repositories/user.repository.ts";
const FILE_MAPPER_REL: &str = "src/files/mappers/file.mapper.ts";

fn nest_fixture(root: &Path) {
    write(root, MAPPER_REL, USER_MAPPER);
    write(root, FILE_MAPPER_REL, FILE_MAPPER);
    write(root, REPOSITORY_REL, USER_REPOSITORY);
}

#[test]
fn a_static_mapper_call_through_a_named_import_binds_to_the_mappers_method() {
    let tmp = TempDir::new().unwrap();
    nest_fixture(tmp.path());
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, REPOSITORY_REL),
        [MAPPER_REL],
        "precondition: the import binds to the mapper"
    );
    assert_eq!(
        callers_of(rt, &format!("{MAPPER_REL}:toDomain")),
        [format!("{REPOSITORY_REL}:create"), format!("{REPOSITORY_REL}:findById")]
    );
    assert_eq!(
        callers_of(rt, &format!("{MAPPER_REL}:toPersistence")),
        [format!("{REPOSITORY_REL}:create")]
    );
    // A mapper calling another mapper binds the other's method, never its own
    // method of the same name.
    assert_eq!(
        callees_of(rt, &format!("{MAPPER_REL}:toDomain")),
        [format!("{FILE_MAPPER_REL}:toDomain")]
    );
}

#[test]
fn a_call_through_an_aliased_or_default_class_import_binds_its_method() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), FILE_MAPPER_REL, FILE_MAPPER);
    write(
        tmp.path(),
        "src/sessions/session.mapper.ts",
        "export default class SessionMapper {\n  static toDomain(raw: any): any { return raw; }\n}\n",
    );
    write(
        tmp.path(),
        "src/sessions/session.repository.ts",
        "import Sessions from './session.mapper';\n\
import { FileMapper as Files } from '../files/mappers/file.mapper';\n\
export function load(raw: any) { Files.toDomain(raw); return Sessions.toDomain(raw); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "src/sessions/session.repository.ts"),
        [FILE_MAPPER_REL, "src/sessions/session.mapper.ts"],
        "precondition: both imports bind"
    );
    assert_eq!(
        callees_of(rt, "src/sessions/session.repository.ts:load"),
        [format!("{FILE_MAPPER_REL}:toDomain"), "src/sessions/session.mapper.ts:toDomain".to_string()]
    );
}

#[test]
fn a_call_through_an_external_class_import_stays_unbound() {
    // A workspace class of the same name the file did not import is never a
    // candidate: the evidence is the import the call went through.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/logger.ts",
        "export class Logger {\n  static log(message: string): void {}\n}\n",
    );
    write(
        tmp.path(),
        "src/app.service.ts",
        "import { Logger } from '@nestjs/common';\nexport function boot() { Logger.log('up'); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(callees_of(rt, "src/app.service.ts:boot").is_empty());
    assert!(callers_of(rt, "src/logger.ts:log").is_empty());
    assert!(call_row_unresolved(rt, "@nestjs::common::Logger::log"));
}

#[test]
fn a_namespace_import_keeps_its_qualification() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/utils.ts",
        "export function format(v: string) { return v; }\nexport class Fmt {\n  static format(v: string) { return v; }\n}\n",
    );
    write(
        tmp.path(),
        "src/app.ts",
        "import * as utils from './utils';\nexport function app() { return utils.format('x'); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/app.ts"), ["src/utils.ts"], "precondition");
    assert_eq!(callees_of(rt, "src/app.ts:app"), ["src/utils.ts:format"]);
    assert_eq!(
        call_target_kinds(rt),
        ["Function"],
        "the module's function, never the class method of that name"
    );
}

#[test]
fn a_call_binds_the_classs_method_never_the_module_function_beside_it() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/mapper.ts",
        "export function toDomain(raw: any) { return raw; }\nexport function label() { return ''; }\nexport class Mapper {\n  static label: string = 'x';\n  toDomain(raw: any) { return raw; }\n}\n",
    );
    // `Mapper.label()` names a field, which is no call target, and never the
    // module function `label` beside the class.
    write(
        tmp.path(),
        "src/repo.ts",
        "import { Mapper } from './mapper';\nexport function find(raw: any) { Mapper.label(); return Mapper.toDomain(raw); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/repo.ts"), ["src/mapper.ts"], "precondition");
    // Both nodes are named `toDomain`; the class's method is told apart by
    // its kind.
    assert_eq!(
        call_target_kinds(rt),
        ["Method"],
        "the class's method, never the module function or the field"
    );
}

#[test]
fn a_receiver_that_is_an_imported_function_or_value_stays_unbound() {
    // `helper.call(null)` names `Function.prototype.call`, and `config.get()`
    // a method of a value whose type the call does not state: neither is a
    // class, so neither binds — never the module's own `call` or `get`, nor
    // the `call` declared inside `helper`.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/h.ts",
        "export function helper() { function call() { return 0; } return call(); }\nexport const config = { get() { return 1; } };\nexport function call() { return 2; }\nexport function get() { return 3; }\n",
    );
    write(
        tmp.path(),
        "src/app.ts",
        "import { helper, config } from './h';\nexport function app() { helper.call(null); return config.get(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/app.ts"), ["src/h.ts"], "precondition");
    assert!(callees_of(rt, "src/app.ts:app").is_empty());
    assert!(call_row_unresolved(rt, ".::h::helper::call"));
    assert!(call_row_unresolved(rt, ".::h::config::get"));
}

/// `./mappers` reaches two classes `UserMapper`, through two export stars.
fn two_mappers(root: &Path) {
    let class = "export class UserMapper {\n  static toDomain(raw: any) { return raw; }\n}\n";
    write(root, "src/mappers/a.ts", class);
    write(root, "src/mappers/b.ts", class);
    write(root, "src/mappers/index.ts", "export * from './a';\nexport * from './b';\n");
}

#[test]
fn a_name_that_reaches_two_classes_stays_unbound() {
    let tmp = TempDir::new().unwrap();
    two_mappers(tmp.path());
    write(
        tmp.path(),
        "src/repo.ts",
        "import { UserMapper } from './mappers';\nexport function find(raw: any) { return UserMapper.toDomain(raw); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/repo.ts"), ["src/mappers/index.ts"], "precondition");
    assert!(callees_of(rt, "src/repo.ts:find").is_empty());
}

#[test]
fn an_ambiguous_class_never_falls_back_to_a_module_at_its_path() {
    // `./mappers/UserMapper` also provides a `toDomain`, and a bare call of it
    // would record the same `.::mappers::UserMapper::toDomain`: the class
    // reading being ambiguous decides the call, never that module's function.
    let tmp = TempDir::new().unwrap();
    two_mappers(tmp.path());
    write(
        tmp.path(),
        "src/mappers/UserMapper.ts",
        "export function toDomain(raw: any) { return raw; }\n",
    );
    write(
        tmp.path(),
        "src/repo.ts",
        "import { UserMapper } from './mappers';\nimport { toDomain } from './mappers/UserMapper';\n\
export function find(raw: any) { return UserMapper.toDomain(raw); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "src/repo.ts"),
        ["src/mappers/UserMapper.ts", "src/mappers/index.ts"],
        "precondition"
    );
    assert!(callees_of(rt, "src/repo.ts:find").is_empty());
}

#[test]
fn a_class_declaring_the_name_twice_binds_neither() {
    // A static and an instance `toDomain` are two members of one name: which
    // one the call means is not decided by the name alone.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/mapper.ts",
        "export class Mapper {\n  static toDomain(raw: any) { return raw; }\n  toDomain(raw: any) { return raw; }\n}\n",
    );
    write(
        tmp.path(),
        "src/repo.ts",
        "import { Mapper } from './mapper';\nexport function find(raw: any) { return Mapper.toDomain(raw); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/repo.ts"), ["src/mapper.ts"], "precondition");
    assert!(callees_of(rt, "src/repo.ts:find").is_empty());
}

#[test]
fn a_member_call_never_reads_as_a_function_of_a_module_at_its_path() {
    // `store.reset()` and `X.make()` record `.::lib::store::reset` and
    // `.::a::X::make`, the targets a bare call of `reset` imported from
    // `./lib/store` (or `make` from `./a/X`) records. The file imports both
    // modules, yet a member call is decided among the class's members alone:
    // `store` is a value, and `X` declares no `make` of its own.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib/index.ts", "export const store = { reset() { return 0; } };\n");
    write(
        tmp.path(),
        "src/lib/store.ts",
        "export function reset() { return 1; }\nexport const VERSION = 1;\n",
    );
    write(tmp.path(), "src/a/base.ts", "export class Base {\n  static make() { return 2; }\n}\n");
    write(
        tmp.path(),
        "src/a/index.ts",
        "import { Base } from './base';\nexport class X extends Base {}\n",
    );
    write(tmp.path(), "src/a/X.ts", "export function make() { return 3; }\nexport const tag = 1;\n");
    write(
        tmp.path(),
        "src/app.ts",
        "import { store } from './lib';\nimport { VERSION } from './lib/store';\n\
import { X } from './a';\nimport { tag } from './a/X';\n\
export function run() { store.reset(); return X.make(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        imported_files(rt, "src/app.ts"),
        ["src/a/X.ts", "src/a/index.ts", "src/lib/index.ts", "src/lib/store.ts"],
        "precondition: every import binds"
    );
    assert!(callees_of(rt, "src/app.ts:run").is_empty());
}

#[test]
fn a_bare_call_never_reads_as_a_member_of_a_class_at_its_path() {
    // `make()` imported from `./a/X` records `.::a::X::make`, as `X.make()`
    // through `{ X } from './a'` does. A bare call is decided by its module
    // alone: an external `make` stays unbound, and a module's own `create`
    // still binds beside a member call of the same target to the class.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a/index.ts", "export class X {\n  static make() { return 1; }\n}\n");
    write(tmp.path(), "src/a/X.ts", "export { make } from 'some-lib';\n");
    write(
        tmp.path(),
        "src/components/Button.ts",
        "export class Button {\n  static create() { return 1; }\n}\nexport function create() { return 2; }\n",
    );
    write(tmp.path(), "src/components/index.ts", "export * from './Button';\n");
    write(
        tmp.path(),
        "src/app.ts",
        "import { X } from './a';\nimport { make } from './a/X';\n\
import { create } from './components/Button';\nimport { Button } from './components';\n\
export function external() { return make(); }\n\
export function bare() { return create(); }\n\
export function member() { return Button.create(); }\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(callees_of(rt, "src/app.ts:external").is_empty());
    assert_eq!(callees_of(rt, "src/app.ts:bare"), ["src/components/Button.ts:create"]);
    assert_eq!(callees_of(rt, "src/app.ts:member"), ["src/components/Button.ts:create"]);
    // The two `create`s share a label: the member call's is the method.
    let kinds = rt
        .submit_read(|store| {
            let nodes = store.all_nodes()?;
            let member = nodes.iter().find(|n| n.name == "member").map(|n| n.id);
            let kind: HashMap<NodeId, String> =
                nodes.iter().map(|n| (n.id, format!("{:?}", n.kind))).collect();
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Calls && Some(e.source) == member)
                .map(|e| kind[&e.target].clone())
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    assert_eq!(kinds, ["Method"]);
}

#[test]
fn a_method_added_to_the_imported_class_binds_on_sync_and_matches_a_full_index() {
    // Only the mapper changes: the repository is untouched, so the call row
    // must be re-selected through its import (sync ≡ reindex).
    let tmp = TempDir::new().unwrap();
    nest_fixture(tmp.path());
    write(
        tmp.path(),
        MAPPER_REL,
        "export class UserMapper {\n  static toDomain(raw: any): any { return raw; }\n}\n",
    );
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(callers_of(rt, &format!("{MAPPER_REL}:toPersistence")).is_empty(), "no toPersistence yet");

    write(tmp.path(), MAPPER_REL, USER_MAPPER);
    engine.sync(&[MAPPER_REL.into()]);
    let synced = (edges_of(rt, EdgeKind::Calls), edges_of(rt, EdgeKind::Imports));
    assert_eq!(
        callers_of(rt, &format!("{MAPPER_REL}:toPersistence")),
        [format!("{REPOSITORY_REL}:create")]
    );
    drop(engine);

    let cold = TempDir::new().unwrap();
    for rel in [MAPPER_REL, FILE_MAPPER_REL, REPOSITORY_REL] {
        write(cold.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold_engine = index(cold.path());
    let cold_rt = cold_engine.runtime().unwrap();
    assert_eq!((edges_of(cold_rt, EdgeKind::Calls), edges_of(cold_rt, EdgeKind::Imports)), synced);
}
