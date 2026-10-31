//! Which plugin reads a shared extension in this repository (S-582,
//! [FR-PL-07]): `.h` is C's in a repository whose C sources outnumber its C++
//! ones, C++'s otherwise ([`LanguageRegistry::shared_owners_for`]).
//!
//! The decision is a function of the admitted file set, so index and sync
//! reach the same one over the same files ([NFR-RA-06]). Discovery makes it
//! before admitting a file, the store records it with the graph
//! ([`SHARED_OWNERS_KEY`]), and a sync whose files move it re-reads every
//! stored header under the new reader — the same headers a fresh index of the
//! post-sync tree would read so.
//!
//! [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use super::{hash_source, supported_extension, LoadedFile};
use crate::graph_store::SHARED_OWNERS_KEY;
use crate::plugin::{LanguageRegistry, SharedOwners};
use crate::runtime::Runtime;

/// The decision over `paths`, counting only the code files the `languages`
/// allowlist admits — a source the run would not index decides nothing.
pub(super) fn decide<'p>(
    registry: &LanguageRegistry,
    langs: Option<&HashSet<String>>,
    paths: impl IntoIterator<Item = &'p str>,
) -> SharedOwners {
    registry.shared_owners_for(paths.into_iter().filter(|rel| supported_extension(registry, langs, rel)))
}

/// The decision the store's graph was built under — the default when none is
/// recorded.
pub(super) fn recorded(runtime: &Runtime) -> Result<SharedOwners> {
    let record = runtime.submit_read(|store| store.project_metadata(SHARED_OWNERS_KEY))?;
    Ok(record.as_deref().map(SharedOwners::from_record).unwrap_or_default())
}

/// Route `registry` by the decision the store's graph was built under (S-582)
/// — the open-time step an engine takes, so a process that runs no index or
/// sync still reads a `.h` as the graph does.
pub(crate) fn route_by_record(runtime: &Runtime, registry: &LanguageRegistry) -> Result<()> {
    registry.set_shared_owners(&recorded(runtime)?);
    Ok(())
}

/// Record `owners` as the decision the graph is built under; the default is
/// recorded as no row, so a repository with no shared extension in play
/// writes nothing.
pub(super) fn record(runtime: &Runtime, owners: &SharedOwners) -> Result<()> {
    let record = owners.record();
    runtime.submit_write(move |w| {
        if record.is_empty() {
            w.clear_project_metadata(SHARED_OWNERS_KEY)
        } else {
            w.set_project_metadata(SHARED_OWNERS_KEY, &record)
        }
    })
}

/// Settle a sync's decision: decide it over the post-sync file set (`stored`
/// less `removals`, with `loaded`), route the registry by it, and — when it
/// differs from the one the graph was built under — add every stored file of a
/// shared extension the batch does not already re-read: to `loaded`, read from
/// disk, when `admits` still yields its path, else to `removals`. Returns the
/// new decision when it moved, for the caller to record once the batch is
/// stored.
pub(super) fn reread_on_change(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    langs: Option<&HashSet<String>>,
    stored: &HashMap<String, Option<String>>,
    loaded: &mut Vec<LoadedFile>,
    removals: &mut Vec<String>,
    admits: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Option<SharedOwners>> {
    // The post-sync file set, each file once: a modified file is both stored
    // and loaded, and counting it twice would tip a tie.
    let removed: HashSet<&str> = removals.iter().map(String::as_str).collect();
    let post_sync: HashSet<&str> = stored
        .keys()
        .map(String::as_str)
        .filter(|rel| !removed.contains(rel))
        .chain(loaded.iter().map(|l| l.rel.as_str()))
        .collect();
    let owners = decide(registry, langs, post_sync);
    registry.set_shared_owners(&owners);
    if owners == recorded(runtime)? {
        return Ok(None);
    }
    let batch: HashSet<String> = loaded.iter().map(|l| l.rel.clone()).chain(removals.iter().cloned()).collect();
    let mut headers: Vec<&String> = stored
        .keys()
        .filter(|rel| registry.is_shared_path(rel) && !batch.contains(*rel))
        .collect();
    headers.sort();
    for rel in headers {
        match admits(rel).and_then(|abs| fs::read_to_string(abs).ok()) {
            Some(source) => loaded.push(LoadedFile {
                rel: rel.clone(),
                hash: hash_source(&source),
                source,
            }),
            None => removals.push(rel.clone()),
        }
    }
    Ok(Some(owners))
}
