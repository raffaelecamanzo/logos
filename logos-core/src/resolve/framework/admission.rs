//! Framework candidacy through a global `using` (S-554, [CR-177], [FR-FW-04],
//! [NFR-RA-05]).
//!
//! A file is a framework candidate when its own ledger names one of its
//! language's `framework_detectors`. C# 10's `global using
//! Microsoft.AspNetCore.Mvc;` puts the namespace in view in **every** file of
//! the project while appearing in one, so the files that rely on it name no
//! detector at all and were never scanned: an ASP.NET Core project that
//! centralises its usings in `GlobalUsings.cs` promoted nothing.
//!
//! The reach of a global `using` is the binder's directory-and-extension reach:
//! every file of the declaring file's extension under the declaring file's
//! directory ([`global_wildcard_governs`], S-518). The directory stands in for the
//! project the `.csproj` would name, which is not read; by .NET convention a global
//! `using` sits at the project root, and one declared deeper reaches fewer files,
//! never more — so a file is admitted by evidence its project states, and a sibling
//! project is not. (The binder additionally requires a package-shaped file; a
//! candidate here is only *scanned*, and a route still needs a query match, so the
//! wider reach admits nothing the binder would not.)
//!
//! The evidence is a ledger row, so the incremental gate's footprint probe
//! ([`crate::graph_store::GraphStore::has_framework_footprint`]) sees it through
//! the same detector prefixes: sync and a full index admit the same files.
//!
//! [CR-177]: ../../../../docs/requests/CR-177-csharp-calls-members-routes-partials-and-typed-receivers-resolve.md
//! [FR-FW-04]: ../../../../docs/specs/requirements/FR-FW-04.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{HashMap, HashSet};

use crate::graph_store::{FileRecord, UnresolvedRefRow};
use crate::model::{EdgeKind, RefForm};
use crate::plugin::Semantics;

use super::super::binder::{global_wildcard_governs, GLOBAL_WILDCARD_ALIAS};
use super::super::rel_path::extension_of;
use super::super::dir_of;
use super::names_a_detector;

/// The ids of the files a global wildcard import naming one of their language's
/// detectors governs — beyond the file that declares it, which the ledger
/// already admits. `semantics_by_file` holds the files of the languages that
/// declare framework candidacy; the extension a global import must share with a
/// file keeps any other language out.
pub(super) fn governed_by_global_usings(
    files: &[FileRecord],
    semantics_by_file: &HashMap<i64, &Semantics>,
    refs: &[UnresolvedRefRow],
) -> HashSet<i64> {
    let path_by_id: HashMap<i64, &str> = files.iter().map(|f| (f.id, f.path.as_str())).collect();
    // `(directory, extension)` of each declaring file whose import names a detector.
    let declared: HashSet<(&str, String)> = refs
        .iter()
        .filter(|r| {
            r.kind == EdgeKind::Imports
                && r.form == RefForm::Glob
                && r.alias.as_deref() == Some(GLOBAL_WILDCARD_ALIAS)
        })
        .filter_map(|r| {
            let file_id = r.file_id?;
            names_a_detector(semantics_by_file.get(&file_id)?, r).then_some(())?;
            let path = *path_by_id.get(&file_id)?;
            Some((dir_of(path), extension_of(path)))
        })
        .collect();
    if declared.is_empty() {
        return HashSet::new();
    }
    semantics_by_file
        .keys()
        .filter_map(|id| {
            let path = *path_by_id.get(id)?;
            let ext = extension_of(path);
            declared
                .iter()
                .any(|(dir, declared_ext)| *declared_ext == ext && global_wildcard_governs(dir, path))
                .then_some(*id)
        })
        .collect()
}
