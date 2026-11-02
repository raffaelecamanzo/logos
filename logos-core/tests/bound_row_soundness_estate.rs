//! The estate reading of the name test's soundness (S-835, CR-262 N2,
//! FR-RS-49): over every indexed store under a root, per language, the bound
//! rows judged, the bound rows the name test would call provably external
//! ("named nowhere"), and the call rows carrying an alias.
//!
//! The same check `bound_rows_are_never_named_nowhere` runs on the reach
//! fixtures in every gate ([`bound_row_soundness`]), here over real graphs —
//! the only place a shape the fixtures lack can break the invariant.
//!
//! Estate-gated: set `LOGOS_REF_WORKSPACE` to a root (a workspace, or a
//! directory of indexed clones such as `~/source/.lang-inspect`). Without it
//! the test prints `SKIPPED` and passes — which is why an estate figure only
//! counts when the run that produced it is shown (command, date,
//! denominators). Run it by hand, beside the other `LOGOS_REF_WORKSPACE`
//! harnesses:
//!
//! ```text
//! LOGOS_REF_WORKSPACE=~/source/pec-services RAYON_NUM_THREADS=2 \
//!   cargo test -p logos-core --release --test bound_row_soundness_estate -- --nocapture
//! ```
//!
//! # Copies only
//!
//! Each store is copied, with its `-wal` and `-shm` files, into a temporary
//! directory and opened there; the copy is migrated to this binary's schema
//! and dropped after its reading. The estate itself is never opened, so a
//! migration never touches it.
//!
//! # A report, and one invariant
//!
//! No count is a floor ([census-figure-as-acceptance-floor]). The test fails
//! when a bound row is named nowhere — listing the first rows with their
//! store, language, kind and target — and when it read no store at all, so a
//! root that holds none never reads green. A store that cannot be read is
//! named unread with its reason, and does not fail the run: the readings of the
//! others still stand. A store whose call rows carry no alias was indexed
//! before calls recorded one: its reading is real, but it cannot show whether
//! a bound row needs a call row's alias, and the report says so.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::graph_store::{GraphStore, SqliteGraphStore};
use logos_core::plugin::LanguageRegistry;
use logos_core::resolve::package_key::PackageLayout;
use logos_core::resolve::{bound_row_soundness, BoundRowSoundness, LanguageSoundness};

/// How deep under the root a store's repository may sit: a member of a
/// workspace (1), a clone under a story directory (2), a before/after pair
/// (3), with room for one more level.
const MAX_DEPTH: usize = 4;

/// How many named-nowhere rows the failure lists.
const LISTED: usize = 40;

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

/// Every `.logos/logos.db` under `root`, at most [`MAX_DEPTH`] directories
/// down, sorted. Hidden directories, `node_modules` and `target` are not
/// entered: no repository keeps its store there, and the estate's clones hold
/// millions of files below them. A symbolic link is not followed either — an
/// estate links to live checkouts, whose stores are not the estate's.
fn stores_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let db = dir.join(".logos").join("logos.db");
        if db.is_file() {
            found.push(db);
        }
        if depth == MAX_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && !name.starts_with('.') && name != "node_modules" && name != "target" {
                pending.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// Copy `db` and its `-wal`/`-shm` sidecars into `into`, returning the copy's
/// path. A sidecar the store does not have is skipped; the WAL is what holds a
/// store's latest commits until a checkpoint, so it is never left behind.
fn copy_store(db: &Path, into: &Path) -> std::io::Result<PathBuf> {
    let copy = into.join("logos.db");
    fs::copy(db, &copy)?;
    for suffix in ["-wal", "-shm"] {
        let mut side = db.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.is_file() {
            let mut to = copy.as_os_str().to_owned();
            to.push(suffix);
            fs::copy(&side, PathBuf::from(to))?;
        }
    }
    Ok(copy)
}

/// The schema version a store was written at, read from the copy before this
/// binary migrates it.
fn schema_version(copy: &Path) -> Option<i64> {
    let conn = rusqlite::Connection::open_with_flags(copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row("PRAGMA user_version", [], |row| row.get(0)).ok()
}

/// Read one store's copy: its schema version, and the check's reading.
fn read_store(db: &Path, layout: &PackageLayout) -> Result<(Option<i64>, BoundRowSoundness), String> {
    let tmp = tempfile::tempdir().map_err(|e| format!("no temporary directory: {e}"))?;
    let copy = copy_store(db, tmp.path()).map_err(|e| format!("copy failed: {e}"))?;
    let version = schema_version(&copy);
    let store = SqliteGraphStore::open(&copy).map_err(|e| format!("did not open: {e:#}"))?;
    // A copy that holds no file is no reading: its commits stayed in a `-wal`
    // the copy lacks, or the store was never indexed.
    if store.indexed_files().map_err(|e| format!("did not read: {e:#}"))?.is_empty() {
        return Err("the copy holds no indexed file".to_string());
    }
    let reading = bound_row_soundness(&store, layout).map_err(|e| format!("did not read: {e:#}"))?;
    Ok((version, reading))
}

#[test]
fn every_bound_row_of_every_store_under_the_root_is_named_by_the_universe() {
    let Some(root) = workspace_root() else {
        println!("SKIPPED: LOGOS_REF_WORKSPACE is not set");
        return;
    };
    let registry = {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    };
    let layout = PackageLayout::from_registry(&registry);
    let stores = stores_under(&root);

    let mut by_language: BTreeMap<String, LanguageSoundness> = BTreeMap::new();
    let mut unread: Vec<(String, String)> = Vec::new();
    let mut without_call_aliases: Vec<String> = Vec::new();
    let mut nowhere: Vec<String> = Vec::new();
    let mut read = 0usize;
    println!("\n## Bound-row soundness per store (S-835) — {}", root.display());
    println!("{:<64} {:>6} {:>12} {:>8} {:>12}", "store", "schema", "bound judged", "nowhere", "call aliases");
    for db in &stores {
        let repo = db.parent().and_then(Path::parent).unwrap_or(db);
        let name = repo.strip_prefix(&root).unwrap_or(repo).display().to_string();
        let name = if name.is_empty() { ".".to_string() } else { name };
        let (version, reading) = match read_store(db, &layout) {
            Ok(reading) => reading,
            Err(reason) => {
                unread.push((name, reason));
                continue;
            }
        };
        read += 1;
        let total = reading.total();
        let version = version.map_or_else(|| "?".to_string(), |v| v.to_string());
        println!(
            "{name:<64} {version:>6} {:>12} {:>8} {:>12}",
            total.bound_judged, total.named_nowhere, total.call_aliases
        );
        if total.call_aliases == 0 {
            without_call_aliases.push(name.clone());
        }
        for (language, row) in &reading.by_language {
            let language = language.clone().unwrap_or_else(|| "(no language)".to_string());
            *by_language.entry(language).or_default() += row;
        }
        nowhere.extend(reading.named_nowhere.iter().map(|r| {
            let language = r.language.as_deref().unwrap_or("(no language)");
            format!("{name}: {language} {:?} {}", r.kind, r.target)
        }));
    }

    println!("\n## Per language, every store read");
    println!("{:<20} {:>12} {:>8} {:>12}", "language", "bound judged", "nowhere", "call aliases");
    let mut total = LanguageSoundness::default();
    for (language, row) in &by_language {
        if *row == LanguageSoundness::default() {
            continue;
        }
        println!(
            "{language:<20} {:>12} {:>8} {:>12}",
            row.bound_judged, row.named_nowhere, row.call_aliases
        );
        total += row;
    }
    println!(
        "{:<20} {:>12} {:>8} {:>12}",
        "TOTAL", total.bound_judged, total.named_nowhere, total.call_aliases
    );
    println!("\nstores: {read} read of {} found; unread: {}", stores.len(), unread.len());
    for (name, reason) in &unread {
        println!("  unread {name}: {reason}");
    }
    println!(
        "stores whose call rows carry no alias (indexed before calls recorded one, or with no \
         qualified call): {} of {read}",
        without_call_aliases.len()
    );
    for name in &without_call_aliases {
        println!("  no call alias: {name}");
    }

    assert!(read > 0, "no store under {} was read — a run that measured nothing", root.display());
    assert!(
        nowhere.is_empty(),
        "{} bound rows named nowhere (first {LISTED}): {:#?}",
        nowhere.len(),
        nowhere.iter().take(LISTED).collect::<Vec<_>>()
    );
}
