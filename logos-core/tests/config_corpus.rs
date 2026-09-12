//! The committed-configuration corpus, end to end (S-380, [CR-121], [FR-WS-19]).
//!
//! The unit half of this mechanism moved into
//! `logos_core::extract::config::corpus` with the 22 tests it was validated by
//! — those prove the flattener. This file proves the two things a unit test
//! cannot:
//!
//! 1. **Ingestion.** A committed `application*.{yml,yaml}` file reaches the store
//!    through the real discover → extract → persist pipeline, with its value and
//!    its profile, at a nesting depth the `ConfigSection` walk's fixed depth
//!    bound ([BR-30]) never reaches — and a project with no configuration corpus
//!    writes not one row ([FR-WS-19] AC7).
//! 2. **The reference census.** The figures the rest of [CR-121] is planned on —
//!    174 sources, 872 distinct keys, 44 unprofiled and 130 profiled across 5
//!    profiles — reproduce from the estate SOURCE.
//!
//! # The census reads the source tree, not an index
//!
//! `ConfigCorpus::discover` walks the corpus read-only. It never opens an
//! [`Engine`](logos_core::Engine), never indexes and never writes a `.logos`
//! store, so it can run against a reference workspace whose enrolment state is
//! being held for another purpose — which is exactly the situation this story
//! landed in.
//!
//! # Skips when unconfigured, and says so — but only under `--nocapture`
//!
//! Set `LOGOS_REF_WORKSPACE=<path>` to run the census (`~` is expanded), the same
//! contract the S-339/S-355/S-365 measurements read. With no corpus configured
//! the test prints a `SKIPPED:` line and passes: it must not fail a machine that
//! has no corpus. Be exact about the limit of that — libtest captures `eprintln!`
//! for a test that PASSES, so a default `cargo test` run shows only `... ok` and
//! the `SKIPPED:` line is invisible. A green summary here does **not** establish
//! that the census ran: look for the printed figures, or run with `--nocapture`.
//! The test name carries `when_one_is_configured` for that reason.
//!
//! [BR-30]: ../../docs/specs/software-spec.md
//! [CR-121]: ../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [FR-WS-19]: ../../docs/specs/requirements/FR-WS-19.md

#![cfg(feature = "lang-yaml")]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::extract::config::corpus::ConfigCorpus;
use logos_core::graph_store::ConfigDefinition;
use logos_core::{Engine, Runtime};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index `root` and hand back the live runtime.
fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine
}

/// Every committed definition of one canonical key, as the store answers it.
fn definitions(rt: &Runtime, key: &str) -> Vec<ConfigDefinition> {
    let key = key.to_string();
    rt.submit_read(move |store| store.config_definitions(&key))
        .expect("read runs")
}

/// The corpus row counts straight off the member store: `(sources, values)`.
///
/// Read as raw SQL, deliberately. "No corpus row at all" is a statement about
/// the TABLES, and a key-by-key probe through the public query can only ever
/// show that the keys it happened to name are absent.
fn corpus_row_counts(root: &Path) -> (i64, i64) {
    let db = root.join(".logos").join("logos.db");
    assert!(db.is_file(), "the index wrote no store at {}", db.display());
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the member store read-only");
    conn.query_row(
        "SELECT (SELECT count(*) FROM config_sources), (SELECT count(*) FROM config_values)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("the migration-19 tables exist")
}

/// The reference workspace, or `None` when none is configured.
fn corpus_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(&home).join(rest),
        None if raw == "~" => PathBuf::from(&home),
        None => PathBuf::from(&raw),
    };
    assert!(
        expanded.is_dir(),
        "LOGOS_REF_WORKSPACE={raw} does not resolve to a directory (expanded: {}) — \
         refusing to report a green run that measured nothing",
        expanded.display(),
    );
    Some(expanded)
}

// ── AC1: the value, the profile and the defining file become facts ───────────

#[test]
fn a_committed_configuration_value_reaches_the_store_with_its_profile_and_its_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Depth FOUR. The ConfigSection walk stops at two (BR-30), so before this
    // story `mailserver` and `api` were nodes and `uri-get-mailbox` was not
    // indexed at all — and none of the three ever carried a value.
    write(
        root,
        "svc/src/main/resources/application.yml",
        "mailserver:\n  api:\n    uri-get-mailbox: /mailbox/{id}\n    timeout: 30\n",
    );
    write(
        root,
        "svc/src/main/resources/application-dev.yml",
        "mailserver:\n  api:\n    uri-get-mailbox: /dev/mailbox/{id}\n",
    );

    let engine = index(root);
    drop(engine);

    // A third overlay, in a second module, added by a later index pass — so the
    // "every overlay is retained" claim below is made over three sources across
    // two modules rather than two in one.
    //
    // It does NOT pin the `ORDER BY f.path, v.value` in `config_definitions`, and
    // nothing here does. That was attempted twice and neither attempt
    // discriminated: the pipeline walks files in sorted path order on every full
    // index, so config_values row ids track path order structurally, and deleting
    // the `ORDER BY` leaves this test green (measured, three consecutive runs).
    // Pinning it needs a store whose row order and path order genuinely diverge,
    // which a full re-index cannot produce. Recorded rather than papered over.
    write(
        root,
        "aaa/src/main/resources/application.yml",
        "mailserver:\n  api:\n    uri-get-mailbox: /aaa/mailbox/{id}\n",
    );
    let engine = index(root);
    let rt = engine.runtime().expect("runtime present");

    let defs = definitions(rt, "mailserver.api.urigetmailbox");
    assert_eq!(
        defs,
        vec![
            ConfigDefinition {
                path: "aaa/src/main/resources/application.yml".to_string(),
                profile: None,
                value: "/aaa/mailbox/{id}".to_string(),
            },
            ConfigDefinition {
                path: "svc/src/main/resources/application-dev.yml".to_string(),
                profile: Some("dev".to_string()),
                value: "/dev/mailbox/{id}".to_string(),
            },
            ConfigDefinition {
                path: "svc/src/main/resources/application.yml".to_string(),
                profile: None,
                value: "/mailbox/{id}".to_string(),
            },
        ],
        "the value, its defining file and its profile are all facts, ordered by path \
         (FR-WS-19 AC1)",
    );
    // Every overlay is retained: disagreement is represented, never averaged and
    // never refused at this layer (FR-WS-19 AC2).
    assert_eq!(defs.len(), 3, "every overlay's value survives");

    // The relaxed binding is what the key is stored under, so the source
    // spelling and the camelCase accessor spelling find the same row.
    assert!(
        definitions(rt, "mailserver.api.uri-get-mailbox").is_empty(),
        "the store holds CANONICAL keys — a caller canonicalises first",
    );
    assert_eq!(
        definitions(rt, "mailserver.api.timeout").len(),
        1,
        "a sibling key at the same depth is indexed independently",
    );
}

#[test]
fn a_key_a_re_index_removes_does_not_linger() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "application.yml", "a:\n  b: one\n");

    let engine = index(root);
    let rt = engine.runtime().expect("runtime present");
    assert_eq!(definitions(rt, "a.b").len(), 1);

    // Replace the source: the old value must not survive as a second, phantom
    // definition — the replace-wholesale contract the reference ledger has.
    write(root, "application.yml", "a:\n  b: two\n");
    let engine = index(root);
    let rt = engine.runtime().expect("runtime present");
    let defs = definitions(rt, "a.b");
    assert_eq!(defs.len(), 1, "a re-index replaces, never accumulates: {defs:?}");
    assert_eq!(defs[0].value, "two");

    // And a source DELETED from the tree leaves no corpus row behind — the
    // migration-19 FK cascade, exercised through the real removal path rather
    // than through a hand-written `DELETE FROM files`.
    fs::remove_file(root.join("application.yml")).expect("remove the source");
    let engine = index(root);
    let rt = engine.runtime().expect("runtime present");
    assert!(
        definitions(rt, "a.b").is_empty(),
        "removing the file removes what it proved",
    );
    assert_eq!(
        corpus_row_counts(root),
        (0, 0),
        "no orphaned source or value row survives the removal",
    );
}

// ── AC4: a project with no configuration corpus is unaffected ────────────────

#[test]
fn a_project_with_no_configuration_corpus_writes_no_corpus_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Every one of these is a YAML/JSON file the artifact extraction pass
    // already walks, and not one of them is a configuration source.
    write(root, "docker-compose.yml", "services:\n  api:\n    image: x\n");
    write(root, "k8s/deployment.yaml", "spec:\n  replicas: 2\n");
    write(root, "bootstrap.yml", "spring:\n  application:\n    name: svc\n");

    let engine = index(root);
    let rt = engine.runtime().expect("runtime present");

    for key in ["services.api.image", "spec.replicas", "spring.application.name"] {
        assert!(
            definitions(rt, key).is_empty(),
            "{key} must not be ingested — none of these files is a configuration source",
        );
    }
    assert_eq!(
        corpus_row_counts(root),
        (0, 0),
        "no configuration corpus means no corpus rows at all (FR-WS-19 AC7)",
    );
}

// ── AC5: the reference-workspace census ──────────────────────────────────────

/// The census [CR-121] plans on, reproduced from the estate source.
///
/// Bounds, not equalities, for the counts that can only grow as the estate gains
/// files — a tighter assertion would fail on a re-clone rather than on a
/// regression. The profile *set* is asserted exactly, because a sixth profile
/// appearing is a fact about the estate a reader must see rather than absorb.
#[test]
fn the_reference_workspace_census_reproduces_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-380 configuration-corpus census"
        );
        return;
    };

    let corpus = ConfigCorpus::discover(&root);
    let keys: BTreeSet<&str> = corpus
        .sources
        .iter()
        .flat_map(|s| s.values.keys().map(String::as_str))
        .collect();
    let unprofiled = corpus.sources.iter().filter(|s| s.profile.is_none()).count();
    let profiled = corpus.sources.iter().filter(|s| s.profile.is_some()).count();
    let profiles = corpus.profiles();

    eprintln!(
        "S-380 configuration corpus over {}:\n  \
           sources:          {}   ({unprofiled} unprofiled, {profiled} profiled)\n  \
           distinct keys:    {}\n  \
           profiles:         {} {:?}",
        root.display(),
        corpus.sources.len(),
        keys.len(),
        profiles.len(),
        profiles,
    );

    assert!(
        corpus.sources.len() >= 174,
        "expected at least 174 configuration sources, got {}",
        corpus.sources.len(),
    );
    assert!(
        keys.len() >= 872,
        "expected at least 872 distinct canonical keys, got {}",
        keys.len(),
    );
    assert!(
        unprofiled >= 44,
        "expected at least 44 unprofiled sources, got {unprofiled}",
    );
    assert!(
        profiled >= 130,
        "expected at least 130 profiled sources, got {profiled}",
    );
    assert_eq!(
        profiles.len(),
        5,
        "the estate declares exactly 5 profiles; got {profiles:?}",
    );
}
