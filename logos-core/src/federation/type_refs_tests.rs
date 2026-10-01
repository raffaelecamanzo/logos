//! Unit tests for the cross-member type-reference overlay (S-473,
//! [FR-WS-35], [ADR-70]) — the pure [`build_index`] over fixture facts, and
//! the stamp-cached [`TypeReferences`] holder over fake engines.
//!
//! The fixture workspace is the reference estate's shapes in miniature:
//!
//! - `lib` produces `com.acme:lib` and declares `com.acme.lib.Dto` in its main
//!   tree, `com.acme.lib.Fixture` in its test tree, and one refused fact;
//! - `models` produces `com.acme:models` and declares the Avro record
//!   `com.acme.events.Evt`;
//! - `app` depends on both and declares `com.acme.app.Own`;
//! - `fork-a` and `fork-b` both produce `com.acme:dup` — a build collision —
//!   and both declare `com.acme.dup.Thing`; only `fork-a` declares
//!   `com.acme.dup.Only`;
//! - `user` depends on `com.acme:dup`;
//! - `stray` builds against nothing.
//!
//! [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
//! [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::*;
use crate::federation::bridge::{ContractNode, MemberContracts};
use crate::federation::build_deps::{join, MemberBuildFacts};
use crate::federation::manifest::MemberKind;
use crate::federation::registry::RegistryMode;
use crate::federation::Federation;
use crate::graph_store::{BuildArtifactRow, BuildManifestRow};

// ── fixture builders ──────────────────────────────────────────────────────

fn fed(names: &[&str], kinds: &[(&str, MemberKind)]) -> Federation {
    let root = PathBuf::from("/ws");
    Federation {
        name: "w".to_string(),
        members: names
            .iter()
            .map(|name| Member {
                name: (*name).to_string(),
                root: root.join(name),
            })
            .collect(),
        root,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
        member_kinds: kinds.iter().map(|(m, k)| ((*m).to_string(), *k)).collect(),
    }
}

/// A main-tree source declaration of `fqn` in `path`, its node `sym:<fqn>`.
fn source(fqn: &str, path: &str) -> DeclaredTypeRow {
    DeclaredTypeRow {
        origin: "source".to_string(),
        path: path.to_string(),
        name: fqn.rsplit('.').next().unwrap().to_string(),
        fqn: Some(fqn.to_string()),
        kind: "class".to_string(),
        symbol: Some(format!("sym:{fqn}")),
        tree: Some("main".to_string()),
        resolution: "resolved".to_string(),
        reason: None,
    }
}

fn test_tree(fqn: &str, path: &str) -> DeclaredTypeRow {
    DeclaredTypeRow {
        tree: Some("test".to_string()),
        ..source(fqn, path)
    }
}

fn refused(name: &str, path: &str) -> DeclaredTypeRow {
    DeclaredTypeRow {
        name: name.to_string(),
        fqn: None,
        resolution: "refused".to_string(),
        reason: Some("package `x` disagrees with its directory".to_string()),
        ..source("x.Refused", path)
    }
}

fn avro(fqn: &str, schema: &str) -> DeclaredTypeRow {
    DeclaredTypeRow {
        origin: "avro".to_string(),
        kind: "record".to_string(),
        symbol: None,
        tree: None,
        ..source(fqn, schema)
    }
}

fn schema(path: &str) -> AvroSchemaRow {
    AvroSchemaRow {
        path: path.to_string(),
        content_hash: Some("h".to_string()),
        status: "read".to_string(),
        detail: None,
    }
}

/// An unresolved import of `target` (ledger spelling) at `path:line`.
fn import(target: &str, path: &str, line: i64) -> TypeRefRow {
    TypeRefRow {
        source_symbol: format!("file:{path}"),
        path: path.to_string(),
        line: Some(line),
        target: target.to_string(),
        kind: EdgeKind::Imports,
    }
}

fn type_use(target: &str, path: &str, line: i64) -> TypeRefRow {
    TypeRefRow {
        kind: EdgeKind::TypeUses,
        ..import(target, path, line)
    }
}

fn artifact(role: &str, kind: Option<&str>, a: &str) -> BuildArtifactRow {
    BuildArtifactRow {
        role: role.to_string(),
        kind: kind.map(str::to_string),
        group_id: Some("com.acme".to_string()),
        artifact_id: Some(a.to_string()),
        version: Some("1".to_string()),
        scope: None,
        project_path: None,
        resolution: "resolved".to_string(),
        reason: None,
    }
}

/// A pom producing `produces` and depending on each of `depends`.
fn pom(produces: Option<&str>, depends: &[&str]) -> Vec<BuildManifestRow> {
    let mut artifacts: Vec<BuildArtifactRow> =
        produces.map(|p| artifact("produced", None, p)).into_iter().collect();
    artifacts.extend(depends.iter().map(|d| artifact("referenced", Some("dependency"), d)));
    vec![BuildManifestRow {
        path: "pom.xml".to_string(),
        format: "maven".to_string(),
        content_hash: Some("h".to_string()),
        status: "read".to_string(),
        detail: None,
        artifacts,
    }]
}

const MEMBERS: [&str; 8] = ["lib", "models", "app", "fork-a", "fork-b", "user", "stray", "plain"];

fn build_facts() -> Vec<MemberBuildFacts> {
    [
        ("lib", pom(Some("lib"), &[])),
        ("models", pom(Some("models"), &[])),
        ("app", pom(Some("app"), &["lib", "models"])),
        ("fork-a", pom(Some("dup"), &[])),
        ("fork-b", pom(Some("dup"), &[])),
        ("user", pom(Some("user"), &["dup"])),
        ("stray", pom(Some("stray"), &[])),
        ("plain", Vec::new()),
    ]
    .into_iter()
    .map(|(m, rows)| (m.to_string(), rows))
    .collect()
}

const APP: &str = "src/main/java/com/acme/app/App.java";

fn type_facts() -> Vec<MemberTypeFactsRead> {
    let facts = |declared: Vec<DeclaredTypeRow>, schemas: Vec<AvroSchemaRow>, rows: Vec<TypeRefRow>| {
        MemberTypeFacts { declared, schemas, rows }
    };
    vec![
        (
            "lib".to_string(),
            facts(
                vec![
                    source("com.acme.lib.Dto", "src/main/java/com/acme/lib/Dto.java"),
                    test_tree("com.acme.lib.Fixture", "src/test/java/com/acme/lib/Fixture.java"),
                    refused("Odd", "src/main/java/elsewhere/Odd.java"),
                ],
                Vec::new(),
                Vec::new(),
            ),
        ),
        (
            "models".to_string(),
            facts(
                vec![avro("com.acme.events.Evt", "src/main/avro/evt.avsc")],
                vec![schema("src/main/avro/evt.avsc")],
                Vec::new(),
            ),
        ),
        (
            "app".to_string(),
            facts(
                vec![source("com.acme.app.Own", "src/main/java/com/acme/app/Own.java")],
                Vec::new(),
                vec![
                    import("com::acme::lib::Dto", APP, 3),
                    import("com::acme::events::Evt", APP, 4),
                    import("com::acme::lib::Dto::CONSTANT", APP, 5),
                    import("java::util::List", APP, 6),
                    import("com::acme::app::Own::Inner", APP, 7),
                    import("com::acme::lib::Fixture", APP, 8),
                    type_use("Dto", APP, 12),
                    type_use("com.acme.lib.Dto", APP, 13),
                ],
            ),
        ),
        (
            "fork-a".to_string(),
            facts(
                vec![
                    source("com.acme.dup.Thing", "src/main/java/com/acme/dup/Thing.java"),
                    source("com.acme.dup.Only", "src/main/java/com/acme/dup/Only.java"),
                ],
                Vec::new(),
                vec![import("com::acme::dup::Thing", "src/main/java/com/acme/dup/Use.java", 2)],
            ),
        ),
        (
            "fork-b".to_string(),
            facts(
                vec![source("com.acme.dup.Thing", "src/main/java/com/acme/dup/Thing.java")],
                Vec::new(),
                Vec::new(),
            ),
        ),
        (
            "user".to_string(),
            facts(
                Vec::new(),
                Vec::new(),
                vec![
                    import("com::acme::dup::Thing", "src/main/kotlin/U.kt", 1),
                    import("com::acme::dup::Only", "src/main/kotlin/U.kt", 2),
                ],
            ),
        ),
        (
            "stray".to_string(),
            facts(
                Vec::new(),
                Vec::new(),
                vec![
                    import("com::acme::lib::Dto", "src/main/java/S.java", 9),
                    import("com::acme::dup::Only", "src/main/java/S.java", 10),
                ],
            ),
        ),
        ("plain".to_string(), MemberTypeFacts::default()),
    ]
}

fn index_with(kinds: &[(&str, MemberKind)]) -> TypeReferenceIndex {
    let federation = fed(&MEMBERS, kinds);
    let build = join(&federation.members, &federation.member_kinds, &build_facts(), &[]);
    build_index(&federation.members, &type_facts(), &[], &build)
}

fn index() -> TypeReferenceIndex {
    index_with(&[])
}

/// `(importer, line, fqn, owner)` per reference — the projection most
/// assertions compare.
fn keys(references: &[TypeReference]) -> Vec<(&str, Option<i64>, &str, &str)> {
    references
        .iter()
        .map(|r| (r.importer.member.as_str(), r.importer.line, r.fqn.as_str(), r.owner.member.as_str()))
        .collect()
}

// ── the owner match ───────────────────────────────────────────────────────

/// **Exactly one other owner binds, with provenance** ([FR-WS-35]): `app`'s
/// import of `com.acme.lib.Dto` names the importing file and line and the
/// declaring file and node.
///
/// [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
#[test]
fn an_import_of_a_type_exactly_one_other_member_declares_binds_with_its_provenance() {
    let index = index();
    let dto = index
        .references
        .iter()
        .find(|r| r.importer.line == Some(3))
        .expect("app's import of Dto binds");
    assert_eq!(
        *dto,
        TypeReference {
            fqn: "com.acme.lib.Dto".to_string(),
            naming: TypeNaming::Exact,
            form: TypeRefForm::Import,
            importer: TypeImporter {
                member: "app".to_string(),
                file: APP.to_string(),
                line: Some(3),
                symbol: format!("file:{APP}"),
            },
            owner: TypeOwner {
                member: "lib".to_string(),
                origin: TypeOrigin::Source,
                declared_in: "src/main/java/com/acme/lib/Dto.java".to_string(),
                symbol: Some("sym:com.acme.lib.Dto".to_string()),
                kind: "class".to_string(),
            },
            evidence: PairEvidence::Build { platform: false },
        }
    );
}

/// An Avro-declared type binds to its schema: the owner names the `.avsc`
/// and carries no symbol, since no member store holds a node for it.
#[test]
fn an_avro_owner_names_its_schema_and_has_no_symbol() {
    let index = index();
    let evt = index.importers("com.acme.events.Evt").next().expect("bound");
    assert_eq!(evt.owner.origin, TypeOrigin::Avro);
    assert_eq!(evt.owner.declared_in, "src/main/avro/evt.avsc");
    assert_eq!(evt.owner.symbol, None);
    let json = serde_json::to_value(evt).unwrap();
    assert!(json["owner"].get("symbol").is_none(), "{json}");
    assert_eq!(json["owner"]["origin"], "avro");
}

/// A static-member import binds its enclosing type; a qualified type use binds
/// as a `type-use`; a bare type-use name is unqualified, never looked up.
#[test]
fn a_static_member_binds_through_its_enclosing_type_and_a_type_use_binds_too() {
    let index = index();
    let constant = index.references.iter().find(|r| r.importer.line == Some(5)).expect("bound");
    assert_eq!((constant.fqn.as_str(), constant.naming), ("com.acme.lib.Dto", TypeNaming::Enclosing));
    let used = index.references.iter().find(|r| r.importer.line == Some(13)).expect("bound");
    assert_eq!((used.form, used.naming), (TypeRefForm::TypeUse, TypeNaming::Exact));
    assert!(index.references.iter().all(|r| r.importer.line != Some(12)), "a bare name names no type");
}

/// **Several owners stay unbound** ([NFR-RA-05]): `user`'s import of
/// `com.acme.dup.Thing`, which `fork-a` and `fork-b` both declare, binds
/// neither and names both with reason `ambiguous-owner`. This is the test the
/// exactly-one → at-least-one mutation fails.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_type_several_other_members_declare_stays_unbound_naming_its_owners() {
    let index = index();
    assert!(
        index.references.iter().all(|r| r.fqn != "com.acme.dup.Thing"),
        "an ambiguous type is never bound: {:?}",
        keys(&index.references)
    );
    assert_eq!(index.ambiguous.len(), 1, "{:?}", index.ambiguous);
    let thing = &index.ambiguous[0];
    assert_eq!(thing.fqn, "com.acme.dup.Thing");
    assert_eq!(thing.importer.member, "user");
    assert_eq!(thing.owners, ["fork-a", "fork-b"]);
    assert_eq!(thing.reason, AMBIGUOUS_OWNER);
    assert_eq!(index.headline.rows.ambiguous_owner, 1);
    assert_eq!(
        index.headline.ambiguous_owner,
        [AmbiguousType {
            fqn: "com.acme.dup.Thing".to_string(),
            owners: vec!["fork-a".to_string(), "fork-b".to_string()],
            references: 1,
        }]
    );
}

/// **An importer that is an owner is out of the tier**: `fork-a` importing
/// `com.acme.dup.Thing` — which it and `fork-b` declare — is self-owned, not
/// ambiguous and not bound to `fork-b`; `app`'s import nested under its own
/// `Own` is self-owned too.
#[test]
fn a_row_whose_importer_is_an_owner_is_out_of_the_tier() {
    let index = index();
    assert_eq!(index.headline.rows.self_owned, 2);
    assert!(index.ambiguous.iter().all(|a| a.importer.member != "fork-a"));
    for list in [&index.references, &index.type_only, &index.pair_unread] {
        assert!(
            list.iter().all(|r| r.importer.member != r.owner.member && r.importer.member != "fork-a"),
            "{:?}",
            keys(list)
        );
    }
}

/// A test-tree declaration is never an owner, and a refused fact names no
/// type; both are counted.
#[test]
fn a_test_tree_type_owns_nothing_and_a_refused_fact_names_nothing() {
    let index = index();
    assert!(index.owners("com.acme.lib.Fixture").is_empty());
    assert!(index.references.iter().all(|r| r.importer.line != Some(8)));
    let members = &index.headline.members;
    assert_eq!((members.test_tree_declarations, members.refused_declarations), (1, 1));
    assert_eq!(members.owned_declarations, 6, "Dto, Evt, Own, Only, and Thing in each fork");
}

/// **A default-package type owns nothing** (review finding A3-F2): `lib`'s
/// `src/main/java/Helper.java` has no `package`, so its fact is the dotless
/// `Helper`. `app`'s bare type use of a third-party `Helper` must not bind to
/// it — no named package can import a default-package type.
#[test]
fn a_default_package_type_is_never_an_owner() {
    let federation = fed(&["lib", "app"], &[]);
    let build = join(
        &federation.members,
        &federation.member_kinds,
        &[("lib".to_string(), pom(Some("lib"), &[])), ("app".to_string(), pom(Some("app"), &["lib"]))],
        &[],
    );
    let facts = vec![
        (
            "lib".to_string(),
            MemberTypeFacts { declared: vec![source("Helper", "src/main/java/Helper.java")], ..MemberTypeFacts::default() },
        ),
        (
            "app".to_string(),
            MemberTypeFacts {
                rows: vec![import("org::thirdparty::Helper", APP, 3), type_use("Helper", APP, 9)],
                ..MemberTypeFacts::default()
            },
        ),
    ];
    let index = build_index(&federation.members, &facts, &[], &build);
    assert!(index.references.is_empty(), "a dotless name bound: {:?}", keys(&index.references));
    assert!(index.owners("Helper").is_empty());
    assert_eq!(index.headline.members.default_package_declarations, 1);
    assert_eq!(index.headline.members.owned_declarations, 0);
}

// ── the pair restriction ──────────────────────────────────────────────────

/// **A build-unrelated pair is type-only, listed, never bound**: `stray`
/// builds against nothing, so its import of `com.acme.lib.Dto` matches `lib`
/// exactly-one and is counted `type-only` beside the pair and type. This is
/// the test dropping the pair restriction fails.
#[test]
fn a_match_between_members_the_build_relation_does_not_relate_is_type_only() {
    let index = index();
    assert!(
        index.references.iter().all(|r| r.importer.member != "stray"),
        "a type-only match is never bound: {:?}",
        keys(&index.references)
    );
    assert_eq!(
        keys(&index.type_only),
        [
            ("stray", Some(10), "com.acme.dup.Only", "fork-a"),
            ("stray", Some(9), "com.acme.lib.Dto", "lib"),
        ]
    );
    assert!(index.type_only.iter().all(|r| r.evidence == PairEvidence::TypeOnly));
    assert_eq!(index.headline.rows.type_only, 2);
    assert_eq!(
        index.headline.type_only,
        [
            TypeOnlyPair {
                from: "stray".to_string(),
                to: "fork-a".to_string(),
                types: vec!["com.acme.dup.Only".to_string()],
                references: 1,
            },
            TypeOnlyPair {
                from: "stray".to_string(),
                to: "lib".to_string(),
                types: vec!["com.acme.lib.Dto".to_string()],
                references: 1,
            },
        ]
    );
}

/// **A collision-backed match names the artifact**: `user` references
/// `com.acme:dup`, which `fork-a` and `fork-b` both produce, so the build
/// relation binds it to neither — yet `com.acme.dup.Only` has one owner,
/// `fork-a`, one of the producers. `stray`, which references no such
/// artifact, gets no collision evidence for the same type.
#[test]
fn a_collision_backed_match_binds_and_names_the_collision_artifact() {
    let index = index();
    let only = index
        .references
        .iter()
        .find(|r| r.importer.member == "user")
        .expect("user's import of Only binds");
    assert_eq!(only.owner.member, "fork-a");
    assert_eq!(only.evidence, PairEvidence::Collision { artifacts: vec!["com.acme:dup".to_string()] });
    let json = serde_json::to_value(&only.evidence).unwrap();
    assert_eq!(json, serde_json::json!({"via": "collision", "artifacts": ["com.acme:dup"]}));
    assert_eq!(
        index.headline.collision_backed,
        [CollisionBackedPair {
            from: "user".to_string(),
            to: "fork-a".to_string(),
            artifacts: vec!["com.acme:dup".to_string()],
            references: 1,
        }]
    );
    assert_eq!((index.headline.build_pairs, index.headline.collision_backed_pairs), (2, 1));
}

/// **Collision evidence names the owner among the producers** (review finding
/// A4-F1): `user` references the colliding `com.acme:dup`, but `lib` — the
/// one owner of `com.acme.lib.Dto` — produces no part of it, so `user`'s
/// import of `Dto` is type-only, never collision-backed.
#[test]
fn a_referenced_collision_admits_only_an_owner_among_its_producers() {
    let federation = fed(&MEMBERS, &[]);
    let build = join(&federation.members, &federation.member_kinds, &build_facts(), &[]);
    let mut facts = type_facts();
    let user = facts.iter_mut().find(|(m, _)| m == "user").expect("user");
    user.1.rows.push(import("com::acme::lib::Dto", "src/main/kotlin/U.kt", 3));
    let index = build_index(&federation.members, &facts, &[], &build);
    let dto = index
        .type_only
        .iter()
        .find(|r| r.importer.member == "user")
        .unwrap_or_else(|| panic!("user's import of Dto is type-only: {:?}", keys(&index.references)));
    assert_eq!((dto.owner.member.as_str(), &dto.evidence), ("lib", &PairEvidence::TypeOnly));
    assert!(index.references.iter().all(|r| !(r.importer.member == "user" && r.owner.member == "lib")));
}

/// A declared `platform` owner still binds — its build edge is counted apart
/// in the build headline, and the reference says so.
#[test]
fn a_platform_owner_binds_with_its_build_edge_flagged() {
    let index = index_with(&[("lib", MemberKind::Platform)]);
    let dto = index.references.iter().find(|r| r.importer.line == Some(3)).expect("bound");
    assert_eq!(dto.evidence, PairEvidence::Build { platform: true });
    assert_eq!(serde_json::to_value(&dto.evidence).unwrap(), serde_json::json!({"via": "build", "platform": true}));
    let unflagged = index.references.iter().find(|r| r.owner.member == "models").expect("bound");
    assert_eq!(serde_json::to_value(&unflagged.evidence).unwrap(), serde_json::json!({"via": "build"}));
}

/// A pair whose build facts are unread is not judged: never bound, never
/// read as unrelated.
#[test]
fn a_pair_with_unread_build_facts_is_never_bound_and_never_type_only() {
    let federation = fed(&MEMBERS, &[]);
    let mut facts = build_facts();
    facts.retain(|(m, _)| m != "app");
    let build = join(&federation.members, &federation.member_kinds, &facts, &["app".to_string()]);
    let index = build_index(&federation.members, &type_facts(), &[], &build);
    assert!(index.references.iter().all(|r| r.importer.member != "app"));
    assert!(index.type_only.iter().all(|r| r.importer.member != "app"));
    assert_eq!(index.headline.rows.pair_unread, 4, "Dto, Evt, Dto::CONSTANT, the qualified type-use");
    assert!(index.pair_unread.iter().all(|r| r.evidence == PairEvidence::PairUnread));
}

/// The owner's side too: with `lib`'s build facts unread, `stray`'s import of
/// `Dto` is pair-unread — nobody read whether `stray` builds against `lib` —
/// never type-only.
#[test]
fn a_pair_whose_owner_has_unread_build_facts_is_pair_unread() {
    let federation = fed(&MEMBERS, &[]);
    let mut facts = build_facts();
    facts.retain(|(m, _)| m != "lib");
    let build = join(&federation.members, &federation.member_kinds, &facts, &["lib".to_string()]);
    let index = build_index(&federation.members, &type_facts(), &[], &build);
    assert!(
        index.pair_unread.iter().any(|r| r.importer.member == "stray" && r.owner.member == "lib"),
        "{:?}",
        keys(&index.pair_unread)
    );
    assert!(index.type_only.iter().all(|r| r.owner.member != "lib"), "{:?}", keys(&index.type_only));
}

// ── the headline ──────────────────────────────────────────────────────────

/// **Every row in exactly one bucket**, the headline beside them, in one line.
#[test]
fn every_row_considered_is_filed_in_exactly_one_bucket_beside_the_headline() {
    let headline = index().headline;
    let rows = headline.rows;
    assert_eq!(rows.considered, 13);
    assert_eq!((rows.imports, rows.type_uses), (11, 2));
    assert_eq!(
        rows.bound
            + rows.type_only
            + rows.pair_unread
            + rows.ambiguous_owner
            + rows.self_owned
            + rows.unqualified
            + rows.no_owner,
        rows.considered,
        "{rows:?}"
    );
    assert_eq!(
        (rows.bound, rows.type_only, rows.ambiguous_owner, rows.self_owned, rows.unqualified, rows.no_owner),
        (5, 2, 1, 2, 1, 2),
        "the bare `Dto` type use is unqualified, never \"no owner\""
    );
    assert_eq!(headline.type_reference_pairs, 3, "app→lib, app→models, user→fork-a");
    assert_eq!(headline.triples, 3);
    assert_eq!((headline.members.read, headline.members.members), (8, 8));
    assert_eq!(headline.members.java_kotlin_avro, 7, "plain holds nothing");
    assert_eq!((headline.members.schemas, headline.members.schemas_read), (1, 1));
    assert_eq!(
        headline.summary,
        "3 member pairs (2 build · 1 collision-backed) bind 5 of 13 unresolved Java/Kotlin import \
         and type-use rows to a type another member declares (2 type-only, 0 pair unread, 1 \
         ambiguous-owner, 2 self-owned, 1 unqualified, 2 no owner in the workspace), over 8 of 8 \
         members read; an advisory type reference, never a coupling"
    );
}

/// The status section is omitted only when every member was read and none
/// is Java/Kotlin/Avro; an unread member keeps it.
#[test]
fn the_section_is_omitted_only_for_a_fully_read_workspace_with_no_java_kotlin_or_avro() {
    let federation = fed(&["a", "b"], &[]);
    let build = join(&federation.members, &federation.member_kinds, &[], &[]);
    let none = vec![
        ("a".to_string(), MemberTypeFacts::default()),
        ("b".to_string(), MemberTypeFacts::default()),
    ];
    assert!(build_index(&federation.members, &none, &[], &build).section().is_none());

    let only_a = &none[..1];
    let index = build_index(&federation.members, only_a, &["b".to_string()], &build);
    let section = index.section().expect("an unread member keeps the section");
    assert_eq!(section.members.unread, ["b"]);
    assert_eq!(section.members.unread_reasons["b"], UNREAD_NOT_EXTRACTED);

    let schema_only = vec![
        ("a".to_string(), MemberTypeFacts { schemas: vec![schema("x.avsc")], ..MemberTypeFacts::default() }),
        ("b".to_string(), MemberTypeFacts::default()),
    ];
    assert!(build_index(&federation.members, &schema_only, &[], &build).section().is_some());
}

/// An unread member contributes neither types nor rows, and is named with
/// its reason: not extracted, or failed.
#[test]
fn an_unread_member_owns_nothing_and_is_named_with_its_reason() {
    let federation = fed(&MEMBERS, &[]);
    let build = join(&federation.members, &federation.member_kinds, &build_facts(), &[]);
    let mut facts = type_facts();
    facts.retain(|(m, _)| m != "lib" && m != "stray");
    let index = build_index(&federation.members, &facts, &["lib".to_string()], &build);
    let members = &index.headline.members;
    assert_eq!(members.unread, ["lib", "stray"]);
    assert_eq!(members.unread_reasons["lib"], UNREAD_NOT_EXTRACTED);
    assert_eq!(members.unread_reasons["stray"], UNREAD_FAILED);
    assert!(index.owners("com.acme.lib.Dto").is_empty());
    assert!(index.references.iter().all(|r| r.owner.member != "lib" && r.importer.member != "stray"));
    assert!(index.headline.summary.contains("over 6 of 8 members read"));
}

// ── the headline past cardinality one (review findings A4-F3..F6) ─────────

/// The base fixture with every aggregate the headline folds holding two:
/// `lib` declares a second type `app` imports (two triples on one pair);
/// `stray` imports `Dto` twice and `Thing` once; `user` imports `Only` twice;
/// `fork-a` and `fork-b` also both produce `com.acme:dup2`, which `user`
/// references too (a collision pair backed by two artifacts); `models` holds a
/// second, malformed schema, and `Evt`'s generated class is committed beside its
/// schema.
fn rich_index() -> TypeReferenceIndex {
    let federation = fed(&MEMBERS, &[]);
    let mut build = build_facts();
    for (member, rows) in build.iter_mut() {
        match member.as_str() {
            "fork-a" | "fork-b" => rows.extend(pom(Some("dup2"), &[])),
            "user" => *rows = pom(Some("user"), &["dup", "dup2"]),
            _ => {}
        }
    }
    let build = join(&federation.members, &federation.member_kinds, &build, &[]);
    let mut facts = type_facts();
    for (member, f) in facts.iter_mut() {
        match member.as_str() {
            "lib" => f.declared.push(source("com.acme.lib.Other", "src/main/java/com/acme/lib/Other.java")),
            "app" => f.rows.push(import("com::acme::lib::Other", APP, 20)),
            "stray" => f.rows.extend([
                import("com::acme::lib::Dto", "src/main/java/S2.java", 1),
                import("com::acme::dup::Thing", "src/main/java/S2.java", 2),
            ]),
            "user" => f.rows.push(import("com::acme::dup::Only", "src/main/kotlin/V.kt", 1)),
            "models" => {
                f.declared.push(source("com.acme.events.Evt", "src/main/java/com/acme/events/Evt.java"));
                f.schemas.push(AvroSchemaRow {
                    status: "malformed".to_string(),
                    detail: Some("invalid JSON".to_string()),
                    ..schema("src/main/avro/broken.avsc")
                });
            }
            _ => {}
        }
    }
    build_index(&federation.members, &facts, &[], &build)
}

/// `triples` counts distinct `(importer, owner, type)`: two types on one pair
/// are two triples, one pair.
#[test]
fn triples_count_each_type_a_pair_carries() {
    let headline = rich_index().headline;
    assert_eq!(headline.type_reference_pairs, 3, "app→lib, app→models, user→fork-a");
    assert_eq!(headline.triples, 4, "app→lib carries Dto and Other");
}

/// The headline lists count every row behind them — two per type-only pair,
/// collision pair and ambiguous type here — and a collision pair names every
/// artifact backing it.
#[test]
fn headline_lists_count_every_row_and_name_every_collision_artifact() {
    let headline = rich_index().headline;
    assert_eq!(
        headline.collision_backed,
        [CollisionBackedPair {
            from: "user".to_string(),
            to: "fork-a".to_string(),
            artifacts: vec!["com.acme:dup".to_string(), "com.acme:dup2".to_string()],
            references: 2,
        }]
    );
    assert_eq!(
        headline.type_only.iter().map(|p| (p.to.as_str(), p.references)).collect::<Vec<_>>(),
        [("fork-a", 1), ("lib", 2)]
    );
    assert_eq!(
        headline.ambiguous_owner,
        [AmbiguousType {
            fqn: "com.acme.dup.Thing".to_string(),
            owners: vec!["fork-a".to_string(), "fork-b".to_string()],
            references: 2,
        }]
    );
}

/// `schemas_read` counts only the schemas read, beside every schema found.
#[test]
fn schemas_read_counts_only_the_schemas_that_parsed() {
    let members = rich_index().headline.members;
    assert_eq!((members.schemas, members.schemas_read), (2, 1), "one of `models`' two schemas is malformed");
}

/// A member declaring one name twice — `Evt` in its schema and its committed
/// generated class — is one owner: the first declaration in `(origin, path,
/// symbol)` order, the source class with its node, so the reference keeps a
/// symbol to stitch through. Both declarations are counted.
#[test]
fn a_name_declared_twice_in_one_member_keeps_its_source_declaration() {
    let index = rich_index();
    let owners = index.owners("com.acme.events.Evt");
    assert_eq!(owners.len(), 1, "one owner per member: {owners:?}");
    assert_eq!(
        (owners[0].origin, owners[0].declared_in.as_str(), owners[0].symbol.as_deref()),
        (TypeOrigin::Source, "src/main/java/com/acme/events/Evt.java", Some("sym:com.acme.events.Evt"))
    );
    let evt = index.importers("com.acme.events.Evt").next().expect("app's import binds");
    assert_eq!(evt.owner, owners[0]);
    assert_eq!(index.headline.members.owned_declarations, 8, "the base six, Other, and Evt's class");
}

// ── the API S-474 reads ───────────────────────────────────────────────────

/// The index, the per-type importers, the per-symbol references and the
/// per-member view all answer over the same bound references.
#[test]
fn the_api_answers_owners_importers_symbols_and_members() {
    let index = index();
    assert_eq!(
        index.owners("com.acme.dup.Thing").iter().map(|o| o.member.as_str()).collect::<Vec<_>>(),
        ["fork-a", "fork-b"]
    );
    assert_eq!(index.importers("com.acme.lib.Dto").count(), 3, "lines 3, 5 and 13");
    assert_eq!(index.references_to("lib", "sym:com.acme.lib.Dto").count(), 3);
    assert_eq!(index.references_to("lib", "sym:nope").count(), 0);
    assert_eq!(index.references_to("app", "sym:com.acme.lib.Dto").count(), 0, "the member is part of the key");

    let lib = index.member("lib").expect("read");
    assert_eq!((lib.imports.len(), lib.imported_by.len(), lib.type_only.len()), (0, 3, 1));
    let stray = index.member("stray").expect("read");
    assert_eq!(stray.type_only.len(), 2, "the importer's end lists its type-only matches too");
    let app = index.member("app").expect("read");
    assert_eq!((app.imports.len(), app.imported_by.len()), (4, 0));
    assert!(index.member("nope").is_none());
    assert_eq!(index.per_member().len(), MEMBERS.len());
}

/// The serialized index carries the matches and the headline, never the
/// private FQN map or roster.
#[test]
fn the_serialized_index_carries_no_private_state() {
    let json = serde_json::to_value(index()).unwrap();
    let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["ambiguous", "headline", "pair_unread", "references", "type_only"]);
}

/// The build relation's collision referencers ride beside it unserialized:
/// the build headline's bytes do not move.
#[test]
fn the_collision_referencers_never_reach_the_build_relations_bytes() {
    let federation = fed(&MEMBERS, &[]);
    let build = join(&federation.members, &federation.member_kinds, &build_facts(), &[]);
    let text = serde_json::to_string(&build).unwrap();
    assert!(!text.contains("referencers"), "{text}");
    assert_eq!(
        build.collisions_referenced_by("user").map(|c| c.artifact.as_str()).collect::<Vec<_>>(),
        ["com.acme:dup"]
    );
    assert_eq!(build.collisions_referenced_by("stray").count(), 0);
}

// ── the stamp-cached holder over fake engines ─────────────────────────────

thread_local! {
    static TYPES: RefCell<HashMap<String, MemberTypeFacts>> = RefCell::new(HashMap::new());
    static STAMPS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// Every `type_facts` read served, by member.
    static READS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// Every `contract_stamp` read served — one per member per stamp walk.
    static STAMP_READS: RefCell<u64> = const { RefCell::new(0) };
}

#[derive(Debug)]
struct FakeEngine {
    member: String,
}

impl MemberEngine for FakeEngine {
    type Watcher = ();
    fn start(root: &Path, _: usize, _: crate::SharedWorkerPool) -> Result<Arc<Self>> {
        let member = root.file_name().unwrap().to_string_lossy().into_owned();
        if member == "broken" {
            anyhow::bail!("store is corrupt");
        }
        Ok(Arc::new(FakeEngine { member }))
    }
    fn watch(self: &Arc<Self>) -> Result<Self::Watcher> {
        Ok(())
    }
}

impl MemberContracts for FakeEngine {
    fn contract_surface(&self) -> Result<Vec<ContractNode>> {
        Ok(Vec::new())
    }
    fn contract_stamp(&self) -> u64 {
        STAMP_READS.with(|n| *n.borrow_mut() += 1);
        STAMPS.with(|s| s.borrow().get(&self.member).copied().unwrap_or(0))
    }
    fn type_facts(&self) -> Result<Option<MemberTypeFacts>> {
        READS.with(|r| *r.borrow_mut().entry(self.member.clone()).or_default() += 1);
        if self.member == "upgraded" {
            return Ok(None);
        }
        Ok(Some(TYPES.with(|t| t.borrow().get(&self.member).cloned().unwrap_or_default())))
    }
}

fn reads() -> u64 {
    READS.with(|r| r.borrow().values().sum())
}

/// **Never at startup, cached on stamps.** A warmed serve registry and a
/// fresh holder read no fact; the first query reads each member once; a
/// second reads nothing; a member's re-sync makes the next one re-match.
#[test]
fn the_index_is_built_on_first_query_never_at_startup_and_cached_on_stamps() {
    for (member, facts) in type_facts() {
        TYPES.with(|t| t.borrow_mut().insert(member, facts));
    }
    let registry = EngineRegistry::<FakeEngine>::new(fed(&MEMBERS, &[]), RegistryMode::Serve);
    let (holder, build) = (TypeReferences::new(), BuildDependencies::new());
    assert!(registry.engine_starts() > 0, "the serve registry warmed at startup");
    assert_eq!(reads(), 0, "startup read no declared type");

    let first = holder.index(&registry, &build);
    assert_eq!(reads(), MEMBERS.len() as u64, "the first query reads every member once");
    // The fakes carry no build facts, so every match is type-only.
    assert_eq!(first.headline.rows.type_only, 7);

    let second = holder.index(&registry, &build);
    assert_eq!(reads(), MEMBERS.len() as u64, "no re-sync, no re-read");
    assert!(Arc::ptr_eq(&first, &second));

    STAMPS.with(|s| s.borrow_mut().insert("stray".into(), 1));
    TYPES.with(|t| t.borrow_mut().insert("stray".into(), MemberTypeFacts::default()));
    let third = holder.index(&registry, &build);
    assert_eq!(reads(), 2 * MEMBERS.len() as u64, "a stamp advance re-matches");
    assert_eq!(third.headline.rows.type_only, 5);
}

/// **One answer scope, one stamp snapshot** (review finding A2-F1): building
/// the index reads every member's stamp once — the build relation it judges
/// pairs with is built inside the same scope on the same stamps, never through
/// a second walk that could see a member re-sync in between.
#[test]
fn the_index_and_its_build_relation_share_one_stamp_walk() {
    let registry = EngineRegistry::<FakeEngine>::new(fed(&MEMBERS, &[]), RegistryMode::Lazy);
    let (holder, build) = (TypeReferences::new(), BuildDependencies::new());
    let _ = holder.index(&registry, &build);
    assert_eq!(
        STAMP_READS.with(|n| *n.borrow()),
        MEMBERS.len() as u64,
        "one stamp walk over the roster, shared with the build relation"
    );
}

/// A member that will not open, and one not yet extracted, are named unread
/// with their reasons; the rest still match.
#[test]
fn a_degraded_or_upgraded_member_is_named_unread_and_the_rest_still_match() {
    for (member, facts) in type_facts() {
        TYPES.with(|t| t.borrow_mut().insert(member, facts));
    }
    let registry = EngineRegistry::<FakeEngine>::new(
        fed(&["lib", "stray", "broken", "upgraded"], &[]),
        RegistryMode::Lazy,
    );
    let index = TypeReferences::new().index(&registry, &BuildDependencies::new());
    let members = &index.headline.members;
    assert_eq!(members.unread, ["broken", "upgraded"]);
    assert_eq!(members.unread_reasons["broken"], UNREAD_FAILED);
    assert_eq!(members.unread_reasons["upgraded"], UNREAD_NOT_EXTRACTED);
    assert_eq!(keys(&index.type_only), [("stray", Some(9), "com.acme.lib.Dto", "lib")]);
}
