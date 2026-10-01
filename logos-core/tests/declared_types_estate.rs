//! The reference-workspace report of the declared-type facts (S-472, CR-152
//! §3.2 B): per member, the types it declares from source and from Avro, each
//! beside its denominator — files read, of files found.
//!
//! Estate-gated: set `LOGOS_REF_WORKSPACE` to a workspace root. Without it the
//! test prints `SKIPPED` and passes — which is why an estate figure only counts
//! when the run that produced it is shown (command, date, denominators).
//!
//! # A private copy only — and the harness refuses anything else
//!
//! Member stores are opened **read-only** through the shipped
//! [`SqliteGraphStore::open_readonly`], which refuses a store at any schema
//! version other than this binary's rather than migrating it, so pointing the
//! harness at a live estate can never advance its graphs. Populate a private
//! copy first (`logos index` per member with this binary).
//!
//! # A report, never a floor
//!
//! Nothing here asserts a count ([census-figure-as-acceptance-floor]). The
//! assertions are structural: every member is read or named unread, every
//! found schema is read or refused with its reason, and every declared fact is
//! resolved with a name or refused with a reason. S-471's gate figures
//! (`operand_resolvability/cross_member_type_refs_finding.txt`: 1,891 main-tree
//! declaring files, 70 `.avsc` declaring 77 records/enums, 0 package refusals)
//! are printed beside the product's for reconciliation by name, never compared
//! by an assertion.

use std::collections::BTreeMap;
use std::path::PathBuf;

use logos_core::config::{discover, load_config_from_root};
use logos_core::extract::declared_types::is_avro_schema;
use logos_core::federation;
use logos_core::graph_store::{GraphStore, SqliteGraphStore};
use logos_core::plugin::LanguageRegistry;
use logos_core::resolve::package_key::PackageLayout;

fn workspace_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(&home).join(rest),
        None => PathBuf::from(&raw),
    };
    assert!(
        expanded.is_dir(),
        "LOGOS_REF_WORKSPACE={raw} is not a directory — refusing to report a green run that \
         measured nothing"
    );
    Some(expanded)
}

/// One member's row of the report.
#[derive(Default, Debug)]
struct Row {
    /// Package-shaped source files the product's walk admits.
    source_found: usize,
    /// …of which the index holds a `files` row for.
    source_read: usize,
    main: usize,
    test: usize,
    refused: usize,
    /// Main-tree files declaring at least one resolved type.
    main_declaring_files: usize,
    schemas_found: usize,
    schemas_read: usize,
    avro: usize,
}

#[test]
fn the_reference_workspace_reports_each_members_declared_types_with_their_denominators() {
    let Some(root) = workspace_root() else {
        println!("SKIPPED: LOGOS_REF_WORKSPACE is not set");
        return;
    };
    let fed = federation::discover(&root).expect("manifest parses").expect("a workspace");
    let registry = {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    };
    let layout = PackageLayout::from_registry(&registry);

    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut unread: Vec<String> = Vec::new();
    let mut refusals: Vec<(String, String, String)> = Vec::new();
    let mut schema_refusals: Vec<(String, String, String)> = Vec::new();
    for member in &fed.members {
        let db = member.root.join(".logos").join("logos.db");
        let store = match db.is_file().then(|| SqliteGraphStore::open_readonly(&db)) {
            Some(Ok(store)) => store,
            Some(Err(e)) => panic!(
                "{}: the store did not open read-only at this binary's version ({e:#}) — run on a \
                 private copy this binary has indexed",
                member.name
            ),
            None => {
                unread.push(format!("{} (no store)", member.name));
                continue;
            }
        };
        if !store.declared_types_extracted().expect("marker reads") {
            unread.push(format!("{} (declared types not yet extracted)", member.name));
            continue;
        }
        let config = load_config_from_root(&member.root).expect("member config loads");
        let walk = discover(&member.root, &config).expect("member walk");
        let canon = member.root.canonicalize().expect("canonical member root");
        let rels: Vec<String> = walk
            .files
            .iter()
            .filter_map(|abs| abs.strip_prefix(&canon).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();

        let mut row = Row {
            source_found: rels.iter().filter(|r| layout.is_package_shaped(r)).count(),
            schemas_found: rels.iter().filter(|r| is_avro_schema(r)).count(),
            ..Row::default()
        };
        row.source_read = store
            .indexed_files()
            .expect("files read")
            .iter()
            .filter(|f| layout.is_package_shaped(&f.path))
            .count();
        let schemas = store.avro_schemas().expect("schemas read");
        for s in &schemas {
            match s.status.as_str() {
                "read" => row.schemas_read += 1,
                _ => {
                    let detail = s.detail.clone().expect("a refused schema carries its reason");
                    schema_refusals.push((member.name.clone(), s.path.clone(), detail));
                }
            }
        }
        assert_eq!(schemas.len(), row.schemas_found, "{}: every found schema is recorded", member.name);
        let mut declaring: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let types = store.declared_types().expect("declared types read");
        for t in &types {
            assert_eq!(t.fqn.is_some(), t.reason.is_none(), "{}: {t:?}", member.name);
            match (t.origin.as_str(), t.fqn.is_some(), t.tree.as_deref()) {
                ("avro", _, _) => row.avro += 1,
                ("source", false, _) => {
                    row.refused += 1;
                    refusals.push((member.name.clone(), t.path.clone(), t.reason.clone().unwrap()));
                }
                ("source", true, Some("main")) => {
                    row.main += 1;
                    declaring.insert(&t.path);
                }
                ("source", true, Some("test")) => row.test += 1,
                other => panic!("{}: an unaccounted fact {other:?}", member.name),
            }
        }
        row.main_declaring_files = declaring.len();
        rows.insert(member.name.clone(), row);
    }

    println!("\n## Declared types per member (S-472) — {}", root.display());
    println!(
        "{:<44} {:>13} {:>6} {:>6} {:>7} {:>13} {:>6}",
        "member", "src read/found", "main", "test", "refused", "avsc read/found", "avro"
    );
    let mut total = Row::default();
    for (name, r) in &rows {
        if r.source_found + r.schemas_found == 0 {
            continue;
        }
        println!(
            "{name:<44} {:>6}/{:<6} {:>6} {:>6} {:>7} {:>6}/{:<6} {:>6}",
            r.source_read, r.source_found, r.main, r.test, r.refused, r.schemas_read, r.schemas_found, r.avro
        );
        total.source_found += r.source_found;
        total.source_read += r.source_read;
        total.main += r.main;
        total.test += r.test;
        total.refused += r.refused;
        total.main_declaring_files += r.main_declaring_files;
        total.schemas_found += r.schemas_found;
        total.schemas_read += r.schemas_read;
        total.avro += r.avro;
    }
    let silent = rows.values().filter(|r| r.source_found + r.schemas_found == 0).count();
    println!(
        "{:<44} {:>6}/{:<6} {:>6} {:>6} {:>7} {:>6}/{:<6} {:>6}",
        "TOTAL",
        total.source_read,
        total.source_found,
        total.main,
        total.test,
        total.refused,
        total.schemas_read,
        total.schemas_found,
        total.avro
    );
    println!(
        "\nmembers: {} read of {} ({} with no Java/Kotlin/Avro file); unread: {:?}",
        rows.len(),
        fed.members.len(),
        silent,
        unread
    );
    println!(
        "main-tree files declaring a resolved type: {} (S-471 gate: 1,891 main-tree declaring files)",
        total.main_declaring_files
    );
    println!(
        "Avro: {} schemas read of {} found, {} records/enums (S-471 gate: 70 .avsc, 77 records/enums)",
        total.schemas_read, total.schemas_found, total.avro
    );
    println!("source refusals: {} (S-471 gate: 0 package refusals)", total.refused);
    for (member, path, reason) in &refusals {
        println!("  refused {member}: {path} — {reason}");
    }
    for (member, path, detail) in &schema_refusals {
        println!("  schema refused {member}: {path} — {detail}");
    }
    assert_eq!(rows.len() + unread.len(), fed.members.len(), "every member read or named unread");
}
