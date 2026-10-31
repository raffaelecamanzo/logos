//! TypeScript type relations are captured and bound (S-535, [FR-EX-17],
//! [FR-RS-15], [NFR-RA-05], [NFR-RA-06]) — end to end through the public
//! [`Engine`], on a fixture shaped like the nestjs-boilerplate inspection
//! repository: abstract-class ports a relational and a document repository
//! `implements`, entities extending a helper that extends a library base, DTOs
//! and services named by annotations, imported by name, through a barrel, an
//! alias and a default import.
//!
//! Before S-535 TypeScript captured no `extends`, `implements`, `new` or
//! annotation at all, so `precedent`, type impact and `super.m()` had nothing
//! to read. Each is now a path-form row, qualified by the import it was named
//! through, and binds exactly-one; a type parameter and a primitive record
//! nothing, and a library type stays in the ledger.
//!
//! [FR-EX-17]: ../../docs/specs/requirements/FR-EX-17.md
//! [FR-RS-15]: ../../docs/specs/requirements/FR-RS-15.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(feature = "lang-typescript")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

type Fixture = &'static [(&'static str, &'static str)];

/// nestjs-boilerplate's shapes, file for file where it has them.
const NEST: Fixture = &[
    (
        "tsconfig.json",
        "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@app/*\": [\"src/*\"] } } }\n",
    ),
    (
        "src/users/domain/user.ts",
        "export class User {\n  id: number;\n  email: string | null;\n}\n",
    ),
    ("src/users/domain/index.ts", "export * from './user';\n"),
    (
        "src/utils/types/nullable.type.ts",
        "export type NullableType<T> = T | null;\n",
    ),
    (
        "src/utils/relational-entity-helper.ts",
        "import { BaseEntity } from 'typeorm';\n\nexport class EntityRelationalHelper extends BaseEntity {\n  save(): void {}\n\n  reload(): void {}\n}\n",
    ),
    (
        "src/users/infrastructure/persistence/user.repository.ts",
        "import { User } from '../../domain';\nimport { NullableType } from '../../../utils/types/nullable.type';\n\nexport abstract class UserRepository {\n  abstract findById(id: User['id']): Promise<NullableType<User>>;\n}\n",
    ),
    (
        "src/users/infrastructure/persistence/relational/user.entity.ts",
        "import { EntityRelationalHelper } from '../../../../utils/relational-entity-helper';\n\nexport class UserEntity extends EntityRelationalHelper {\n  save(): void {\n    super.save();\n  }\n\n  refresh(): void {\n    this.reload();\n  }\n}\n",
    ),
    (
        "src/users/admin.entity.ts",
        "import { EntityRelationalHelper } from '@app/utils/relational-entity-helper';\nimport { IPage } from '@app/utils/types/pagination-options';\nimport { UserEntity } from '@app/users/infrastructure/persistence/relational/user.entity';\n\nexport class AdminEntity extends EntityRelationalHelper implements IPage {\n  page = 1;\n\n  save(): void {\n    super.save();\n  }\n\n  spawn(): void {\n    const made = new UserEntity();\n  }\n}\n",
    ),
    (
        "src/users/infrastructure/persistence/relational/user.repository.ts",
        "import { Injectable } from '@nestjs/common';\nimport { User } from '../../../domain';\nimport { NullableType } from '../../../../utils/types/nullable.type';\nimport { UserRepository } from '../user.repository';\nimport { UserEntity } from './user.entity';\n\n@Injectable()\nexport class UsersRelationalRepository implements UserRepository {\n  async findById(id: User['id']): Promise<NullableType<User>> {\n    const entity = new UserEntity();\n    return null;\n  }\n}\n",
    ),
    (
        "src/users/infrastructure/persistence/document/user.repository.ts",
        "import { User } from '../../../domain';\nimport { NullableType } from '../../../../utils/types/nullable.type';\nimport { UserRepository } from '../user.repository';\n\nexport class UsersDocumentRepository implements UserRepository {\n  async findById(id: User['id']): Promise<NullableType<User>> {\n    return null;\n  }\n\n  async remove(id: User['id']): Promise<void> {}\n}\n",
    ),
    (
        "src/users/ports.ts",
        "import { User } from './domain/user';\n\nexport type Shape = { id: number };\n\nexport interface Persisted extends User {}\n\nexport interface Shaped extends Shape {}\n",
    ),
    (
        "src/users/dto/create-user.dto.ts",
        "export class CreateUserDto {\n  email: string | null;\n}\n",
    ),
    (
        "src/users/dto/update-user.dto.ts",
        "import { PartialType } from '@nestjs/swagger';\nimport { CreateUserDto } from './create-user.dto';\n\nexport class UpdateUserDto extends PartialType(CreateUserDto) {}\n",
    ),
    (
        "src/users/users.service.ts",
        "import { CreateUserDto } from './dto/create-user.dto';\nimport { UserRepository } from './infrastructure/persistence/user.repository';\nimport { User } from '@app/users/domain/user';\n\nexport class UsersService {\n  constructor(private readonly usersRepository: UserRepository) {}\n\n  create(createUserDto: CreateUserDto): Promise<User> {\n    return null;\n  }\n\n  find({\n    filter,\n  }: {\n    filter?: CreateUserDto | null;\n  }): void {}\n}\n",
    ),
    (
        "src/users/users.controller.ts",
        "import { Body, Controller, Post } from '@nestjs/common';\nimport { CreateUserDto } from './dto/create-user.dto';\nimport { UpdateUserDto } from './dto/update-user.dto';\nimport { UsersService } from './users.service';\n\n@Controller('users')\nexport class UsersController {\n  constructor(private readonly usersService: UsersService) {}\n\n  @Post()\n  create(@Body() createUserDto: CreateUserDto) {\n    return this.usersService.create(createUserDto);\n  }\n\n  update(dto: UpdateUserDto | null, ids: CreateUserDto[]): void {}\n}\n",
    ),
    (
        "src/roles/roles.guard.ts",
        "import { CanActivate } from '@nestjs/common';\n\nexport class RolesGuard implements CanActivate {\n  canActivate(): boolean {\n    return true;\n  }\n}\n",
    ),
    (
        "src/utils/types/pagination-options.ts",
        "export interface IPage {\n  page: number;\n}\n\nexport interface IPaginationOptions extends IPage {\n  limit: number;\n}\n",
    ),
    (
        "src/app/base.ts",
        "export default class AppBase {}\n",
    ),
    (
        "src/app/app.ts",
        "import AppBase from './base';\nimport { AppConfig } from './config.type';\n\nexport class App extends AppBase {\n  config: AppConfig;\n\n  fresh(): void {\n    const made = new AppConfig();\n  }\n}\n",
    ),
    (
        "src/app/config.type.ts",
        "export interface AppConfig {\n  port: number;\n}\n\nexport const AppConfig = { port: 3000 };\n",
    ),
    (
        "src/utils/operands.ts",
        "export class IntA {}\nexport class IntB {}\nexport class TupC {}\nexport class TupD {}\nexport class KeyE {}\nexport class RoF {}\nexport class ParG {}\n\nexport function shapes(i: IntA & IntB, t: [TupC, TupD], k: keyof KeyE, r: readonly RoF[], p: (ParG)): void {}\n",
    ),
    (
        "src/utils/flags.ts",
        "export class P {}\n\nexport type Flags<T> = { [P in keyof T]: P };\n",
    ),
    (
        "src/utils/box.ts",
        "export class Key {}\n\nexport class Box<Key> {\n  held: Key;\n\n  put<V>(value: V): V {\n    return value;\n  }\n}\n\nclass Local {}\n\nexport function build(): void {\n  const local: Local = new Local();\n}\n",
    ),
    (
        "src/utils/ghost.ts",
        "export class Ghost {}\n",
    ),
    (
        "src/utils/mixins.ts",
        "import { Ghost } from './ghost';\nimport { IPage } from './types/pagination-options';\n\nexport const Mixed = class extends Ghost {};\n\nexport function make() {\n  return class implements IPage {\n    page = 1;\n  };\n}\n",
    ),
    (
        "src/utils/unimported.ts",
        "import * as dto from '../users/dto/create-user.dto';\n\nexport function haunt(ghost: Ghost, named: dto.CreateUserDto): void {}\n\nexport class Widget extends dto.CreateUserDto {}\n",
    ),
];

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn indexed(fixture: Fixture) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, source) in fixture {
        write(tmp.path(), rel, source);
    }
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

/// Every edge of `kind`, as `source name -> target file:name:kind`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<String> {
    rt.submit_read(move |store| {
        let nodes: HashMap<NodeId, _> = store.all_nodes()?.into_iter().map(|n| (n.id, n)).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let (s, t) = (&nodes[&e.source], &nodes[&e.target]);
                let file = t.file_path.as_deref().unwrap_or_default();
                format!("{} -> {file}:{}:{}", s.name, t.name, t.kind.as_str())
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The `Calls` edges out of methods named `caller`, as `target file:name`.
fn calls_from(rt: &Runtime, caller: &str) -> Vec<String> {
    let caller = caller.to_string();
    rt.submit_read(move |store| {
        let nodes: HashMap<NodeId, _> = store.all_nodes()?.into_iter().map(|n| (n.id, n)).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls && nodes[&e.source].name == caller)
            .map(|e| {
                let t = &nodes[&e.target];
                format!("{}:{}", t.file_path.as_deref().unwrap_or_default(), t.name)
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The ledger's type-relation rows, as `kind target`, sorted and deduplicated —
/// the unbound ones when `unbound`, else every one.
fn relation_rows(rt: &Runtime, unbound: bool) -> Vec<String> {
    let mut rows: Vec<String> = rt
        .submit_read(move |store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| {
                    matches!(
                        r.kind,
                        EdgeKind::Extends | EdgeKind::Implements | EdgeKind::Instantiates | EdgeKind::TypeUses
                    )
                })
                .filter(|r| r.form != RefForm::Symbol && (!unbound || !r.resolved))
                .map(|r| format!("{} {}", r.kind.as_str(), r.target))
                .collect())
        })
        .expect("read runs");
    rows.sort();
    rows.dedup();
    rows
}

/// Every edge as `(source symbol, target symbol, kind)` and every textual ledger
/// row, sorted — what a cold index and a sync must agree on.
fn binding_facts(rt: &Runtime) -> (Vec<(String, String, String)>, Vec<String>) {
    rt.submit_read(|store| {
        let sym: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.symbol.as_str().to_string()))
            .collect();
        let mut edges: Vec<(String, String, String)> = store
            .all_edges()?
            .into_iter()
            .map(|e| (sym[&e.source].clone(), sym[&e.target].clone(), e.kind.as_str().to_string()))
            .collect();
        edges.sort();
        let mut refs: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .map(|r| format!("{} {} {:?} {:?} {}", r.source_symbol, r.target, r.form, r.kind, r.resolved))
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

/// The facts a cold index of `fixture`, with `tmp`'s current contents, records.
fn cold(fixture: Fixture, tmp: &TempDir) -> (Vec<(String, String, String)>, Vec<String>) {
    let fresh = TempDir::new().unwrap();
    for (rel, _) in fixture {
        write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    coordinate::pin(fresh.path());
    let engine = Engine::start(fresh.path()).expect("engine starts");
    engine.index();
    binding_facts(engine.runtime().unwrap())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// FR-EX-17 AC (nestjs-boilerplate's 13 `extends`): an entity extends the
/// helper it imports by a relative path, an interface the interface beside it
/// — or, TypeScript's types being structural, a class it imports or a type
/// alias —
/// a class the default export it imports, a second entity the helper it
/// imports through the `@app` alias. The helper's own `BaseEntity` is
/// typeorm's and stays unbound, and `extends PartialType(CreateUserDto)` — a
/// call, naming no type — records no `Extends` at all.
#[test]
fn a_class_and_an_interface_extend_the_one_type_their_imports_name() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "AdminEntity -> src/utils/relational-entity-helper.ts:EntityRelationalHelper:class",
            "App -> src/app/base.ts:AppBase:class",
            "IPaginationOptions -> src/utils/types/pagination-options.ts:IPage:interface",
            "Persisted -> src/users/domain/user.ts:User:class",
            "Shaped -> src/users/ports.ts:Shape:type_alias",
            "UserEntity -> src/utils/relational-entity-helper.ts:EntityRelationalHelper:class",
        ])
    );
    let unbound = relation_rows(rt, true);
    assert!(unbound.contains(&"extends typeorm::BaseEntity".to_string()), "{unbound:?}");
    assert!(
        !relation_rows(rt, false).iter().any(|r| r.starts_with("extends") && r.contains("PartialType")),
        "a call names no supertype: {:?}",
        relation_rows(rt, false)
    );
}

/// Each type a TypeScript type is built from is a use: an intersection's and a
/// tuple's members, `keyof`'s operand, a `readonly` array's element and a
/// parenthesised type.
#[test]
fn every_operand_of_a_composite_type_is_a_use() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    let shapes: Vec<String> = edges(rt, EdgeKind::TypeUses)
        .into_iter()
        .filter(|e| e.starts_with("shapes -> "))
        .collect();
    assert_eq!(
        shapes,
        strings(&[
            "shapes -> src/utils/operands.ts:IntA:class",
            "shapes -> src/utils/operands.ts:IntB:class",
            "shapes -> src/utils/operands.ts:KeyE:class",
            "shapes -> src/utils/operands.ts:ParG:class",
            "shapes -> src/utils/operands.ts:RoF:class",
            "shapes -> src/utils/operands.ts:TupC:class",
            "shapes -> src/utils/operands.ts:TupD:class",
        ])
    );
}

/// FR-EX-17 AC (nestjs-boilerplate's 11 `implements`): both repositories
/// implement the abstract-class port they import — an `implements` names a
/// class as readily as an interface in TypeScript — and `CanActivate`, a
/// library interface, stays unbound.
#[test]
fn a_class_implements_the_abstract_class_port_it_imports() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&[
            "AdminEntity -> src/utils/types/pagination-options.ts:IPage:interface",
            "UsersDocumentRepository -> src/users/infrastructure/persistence/user.repository.ts:UserRepository:class",
            "UsersRelationalRepository -> src/users/infrastructure/persistence/user.repository.ts:UserRepository:class",
        ])
    );
    assert!(relation_rows(rt, true).contains(&"implements @nestjs::common::CanActivate".to_string()));
}

/// FR-EX-17 AC: `CreateUserDto` and `UsersService` record their users as
/// incoming `TypeUses` — the methods whose parameters name them, a union and an
/// array included, and the field a constructor parameter property declares —
/// and `new UserEntity()` instantiates the entity it imports. (`impact` walks
/// the dependency view, which fences every structural fact out in every
/// language, ADR-21, so these users are read off the edges.)
#[test]
fn dto_and_service_annotations_record_their_users() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    let type_uses = edges(rt, EdgeKind::TypeUses);
    let users_of = |target: &str| -> Vec<String> {
        let mut users: Vec<String> = type_uses
            .iter()
            .filter(|e| e.ends_with(target))
            .map(|e| e.split(" -> ").next().unwrap().to_string())
            .collect();
        users.dedup();
        users
    };
    assert_eq!(
        users_of("src/users/dto/create-user.dto.ts:CreateUserDto:class"),
        strings(&["create", "find", "update"])
    );
    // `dto: UpdateUserDto | null`: a union's member is a use.
    assert_eq!(
        users_of("src/users/dto/update-user.dto.ts:UpdateUserDto:class"),
        strings(&["update"])
    );
    assert_eq!(
        users_of("src/users/users.service.ts:UsersService:class"),
        strings(&["usersService"])
    );
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        strings(&[
            "build -> src/utils/box.ts:Local:class",
            "findById -> src/users/infrastructure/persistence/relational/user.entity.ts:UserEntity:class",
            "spawn -> src/users/infrastructure/persistence/relational/user.entity.ts:UserEntity:class",
        ])
    );
}

/// A type is read through a barrel, a tsconfig alias, a nested type argument
/// (`Promise<NullableType<User>>`) and an indexed access (`remove(id:
/// User['id'])`, its only mention of `User`): each binds the one `User`, and
/// `NullableType`, a type alias, binds too. A type in a nested object
/// annotation spread over several lines is one row, not one per enclosing
/// annotation.
#[test]
fn a_type_reached_through_a_barrel_an_alias_and_type_arguments_binds() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    let type_uses = edges(rt, EdgeKind::TypeUses);
    for expected in [
        "create -> src/users/domain/user.ts:User:class",
        "findById -> src/users/domain/user.ts:User:class",
        "findById -> src/utils/types/nullable.type.ts:NullableType:type_alias",
        "remove -> src/users/domain/user.ts:User:class",
    ] {
        assert!(type_uses.contains(&expected.to_string()), "{expected}: {type_uses:#?}");
    }
    let find_rows = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::TypeUses && r.source_symbol.ends_with("UsersService#find()."))
                .map(|r| format!("{}@{}", r.target, r.line.unwrap_or_default()))
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    assert_eq!(find_rows, strings(&[".::dto::create-user.dto::CreateUserDto@15"]), "{find_rows:?}");
}

/// FR-EX-17 AC: `precedent UsersRelationalRepository` lists its sibling
/// repository — both implement the one `UserRepository` port.
#[test]
fn precedent_lists_the_sibling_repository() {
    let (_tmp, engine) = indexed(NEST);
    let result = engine.precedent("UsersRelationalRepository", None);
    let names: Vec<&str> = result.precedents.iter().map(|p| p.symbol.name.as_str()).collect();
    assert!(names.contains(&"UsersDocumentRepository"), "{names:?}");
}

/// FR-EX-17: a type parameter and a primitive record nothing — `held: Key`
/// inside `Box<Key>` names the parameter, never the class `Key` beside it, and
/// `[P in keyof T]: P` names the mapped type's key, never the class `P` —
/// and a name of two declaration spaces binds its type: `config: AppConfig`
/// reaches the interface, not the `const` of the same name — and `new
/// AppConfig()` constructs neither, since only a class is instantiated.
#[test]
fn a_type_parameter_or_a_primitive_records_nothing() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    let rows = relation_rows(rt, false);
    for absent in ["Key", "V", "T", "P", "string", "number", "boolean", "void"] {
        assert!(
            !rows.iter().any(|r| r.ends_with(&format!(" {absent}"))),
            "`{absent}` recorded: {rows:#?}"
        );
    }
    let type_uses = edges(rt, EdgeKind::TypeUses);
    assert!(!type_uses.iter().any(|e| e.contains(":Key:") || e.contains(":P:")), "{type_uses:#?}");
    assert!(
        type_uses.contains(&"config -> src/app/config.type.ts:AppConfig:interface".to_string()),
        "{type_uses:#?}"
    );
    let instantiates = edges(rt, EdgeKind::Instantiates);
    assert!(!instantiates.iter().any(|e| e.starts_with("fresh ")), "{instantiates:#?}");
    assert!(
        relation_rows(rt, true).contains(&"instantiates .::config.type::AppConfig".to_string()),
        "the row stays in the ledger: {:#?}",
        relation_rows(rt, true)
    );
    assert!(
        type_uses.contains(&"local -> src/utils/box.ts:Local:class".to_string())
            || type_uses.contains(&"build -> src/utils/box.ts:Local:class".to_string()),
        "a same-file type binds: {type_uses:#?}"
    );
}

/// NFR-RA-05: a type the file never imports binds by no workspace guess, though
/// the repository declares exactly one of its name, and a type named through a
/// namespace import (`dto.CreateUserDto`) is a member of a value the binder
/// does not read — both stay in the ledger.
#[test]
fn an_unimported_or_namespace_qualified_type_stays_unbound() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    assert!(
        !edges(rt, EdgeKind::TypeUses).iter().any(|e| e.starts_with("haunt ")),
        "{:#?}",
        edges(rt, EdgeKind::TypeUses)
    );
    let unbound = relation_rows(rt, true);
    assert!(unbound.contains(&"type_uses Ghost".to_string()), "{unbound:#?}");
    assert!(unbound.contains(&"type_uses dto::CreateUserDto".to_string()), "{unbound:#?}");
    // `extends dto.CreateUserDto`, a member expression, names the same path.
    assert!(unbound.contains(&"extends dto::CreateUserDto".to_string()), "{unbound:#?}");
}

/// A class expression declares no class node, so its `extends` and
/// `implements` record nothing — never a supertype of the variable or the
/// function around it, which `precedent` would then read as a sibling.
#[test]
fn a_class_expression_records_no_supertype() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    let rows: Vec<String> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| matches!(r.kind, EdgeKind::Extends | EdgeKind::Implements))
                .filter(|r| r.source_symbol.contains("`mixins.ts`"))
                .map(|r| format!("{} {} {}", r.source_symbol, r.kind.as_str(), r.target))
                .collect())
        })
        .expect("read runs");
    assert!(rows.is_empty(), "{rows:#?}");
}

/// With its base proven — imported, so read once the imported scope is known —
/// `super.save()` binds the helper's `save` (from both entities, one importing
/// it relatively, one through the alias) and `this.reload()` the `reload`
/// the entity inherits: the hierarchy walk no TypeScript `Extends` reached
/// before.
#[test]
fn a_super_or_this_call_climbs_the_proven_base() {
    let (_tmp, engine) = indexed(NEST);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "save"),
        strings(&[
            "src/utils/relational-entity-helper.ts:save",
            "src/utils/relational-entity-helper.ts:save",
        ])
    );
    assert_eq!(
        calls_from(rt, "refresh"),
        strings(&["src/utils/relational-entity-helper.ts:reload"])
    );
}

/// The `.tsx` grammar captures the same relations: a component's props
/// annotation binds the interface it imports.
#[test]
fn a_tsx_component_props_annotation_binds_its_imported_interface() {
    let fixture: Fixture = &[
        ("src/types.ts", "export interface ButtonProps {\n  label: string;\n}\n"),
        (
            "src/button.tsx",
            "import { ButtonProps } from './types';\n\nexport function Button(props: ButtonProps) {\n  return <button>{props.label}</button>;\n}\n",
        ),
    ];
    let (_tmp, engine) = indexed(fixture);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::TypeUses),
        strings(&["Button -> src/types.ts:ButtonProps:interface"])
    );
}

/// NFR-RA-06: sync ≡ reindex when the port a repository implements is renamed
/// and restored in its own file, when the barrel a type is read through stops
/// re-exporting it, and when the `tsconfig.json` alias a type is imported
/// through is retargeted — no edit spells the file whose row moves.
#[test]
fn sync_equals_a_full_reindex_when_an_imported_type_moves() {
    let fixture = NEST;
    let (tmp, engine) = indexed(fixture);
    let rt = engine.runtime().unwrap();
    let port = "src/users/infrastructure/persistence/user.repository.ts";
    let barrel = "src/users/domain/index.ts";
    let config = "tsconfig.json";
    let original = |rel: &str| fixture.iter().find(|(r, _)| *r == rel).unwrap().1.to_string();
    for (rel, edit) in [
        (port, "export abstract class OtherRepository {}\n".to_string()),
        (port, original(port)),
        (barrel, "export {};\n".to_string()),
        (barrel, original(barrel)),
        // The alias `@app/*` retargeted, then restored: the service's
        // `@app/users/domain/user` import moves, and so do the base, the
        // interface and the class `AdminEntity` extends, implements and news
        // through it, with the `super.save()` that climbs the base — the config
        // spells nothing any of those rows names.
        (config, "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@app/*\": [\"lib/*\"] } } }\n".to_string()),
        (config, original(config)),
    ] {
        write(tmp.path(), rel, &edit);
        engine.sync(&[rel.into()]);
        assert_eq!(binding_facts(rt), cold(fixture, &tmp), "after writing {rel}:\n{edit}");
    }
}
