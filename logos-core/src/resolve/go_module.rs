//! The Go modules a tree declares — the anchor a Go import path binds against
//! (S-439, [CR-142] D1, [FR-RS-01]).
//!
//! A Go import path (`"github.com/org/repo/internal/admin"`) names a package
//! **directory** by its module path plus the directory's position under the
//! module root. Nothing in the path itself says where the module path ends: the
//! same text could be an external package whose last segments happen to spell a
//! workspace directory (`github.com/golang/protobuf/proto` against a repo's own
//! generated `proto/`). A suffix match on directories would therefore bind
//! external imports to workspace code, which [NFR-RA-05] forbids. The one fact
//! that settles it is the `module` directive of the `go.mod` the Go toolchain
//! itself reads, so that is what this module reads — and nothing else.
//!
//! Only the `go.mod` files that are an ancestor of an indexed `.go` file are
//! opened, so a tree with no Go code reads no file at all. What is read is the
//! `module` line; a `go.mod` without one declares no module and contributes
//! nothing.
//!
//! [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// One Go module: where its `go.mod` sits and the module path it declares.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GoModule {
    /// The project-relative directory holding the `go.mod` (`""` at the root).
    pub root: String,
    /// The declared module path (`github.com/sourcesense/desk-picker`).
    pub path: String,
}

/// The module path a `go.mod`'s text declares, or `None` if it declares none.
///
/// Reads the first `module` directive: `module example.com/m`, optionally
/// quoted, with a trailing `//` comment ignored.
pub(crate) fn module_path(go_mod: &str) -> Option<String> {
    go_mod.lines().find_map(|line| {
        let line = line.split("//").next().unwrap_or_default().trim();
        let rest = line.strip_prefix("module")?;
        // `module` must be the whole directive keyword, not a prefix of one.
        if !rest.starts_with([' ', '\t']) {
            return None;
        }
        let path = rest.trim().trim_matches(|c| c == '"' || c == '`');
        (!path.is_empty() && !path.contains(char::is_whitespace)).then(|| path.to_string())
    })
}

/// Every Go module declared by the nearest `go.mod` — in the file's own
/// directory or an ancestor of it — of one of `go_files` under `root`, sorted longest module path first so the
/// first prefix match is the most specific one; ties break on the path text,
/// then the root, so the order is deterministic ([NFR-RA-06]).
///
/// Each directory is probed at most once. An unreadable `go.mod` is treated as
/// absent — losing an anchor only leaves an import unbound, never mis-bound.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn discover<'p>(
    root: &Path,
    go_files: impl IntoIterator<Item = &'p str>,
) -> Vec<GoModule> {
    let mut probed: HashMap<String, Option<String>> = HashMap::new();
    let mut found: BTreeSet<GoModule> = BTreeSet::new();
    for file in go_files {
        let mut dir = parent_of(file);
        loop {
            let declared = probed
                .entry(dir.to_string())
                .or_insert_with(|| {
                    let at = if dir.is_empty() {
                        root.join("go.mod")
                    } else {
                        root.join(dir).join("go.mod")
                    };
                    std::fs::read_to_string(at)
                        .ok()
                        .and_then(|text| module_path(&text))
                })
                .clone();
            if let Some(path) = declared {
                found.insert(GoModule {
                    root: dir.to_string(),
                    path,
                });
                break; // the nearest go.mod owns the file
            }
            if dir.is_empty() {
                break;
            }
            dir = parent_of(dir);
        }
    }
    let mut modules: Vec<GoModule> = found.into_iter().collect();
    modules.sort_by(|a, b| {
        b.path
            .len()
            .cmp(&a.path.len())
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.root.cmp(&b.root))
    });
    modules
}

/// The directory part of a project-relative path (`""` for a root-level one).
fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_path_reads_the_module_directive_only() {
        assert_eq!(
            module_path("module github.com/sourcesense/desk-picker\n\ngo 1.26.1\n").as_deref(),
            Some("github.com/sourcesense/desk-picker")
        );
        assert_eq!(
            module_path("// a comment\nmodule \"example.com/m\" // trailing\n").as_deref(),
            Some("example.com/m")
        );
        assert_eq!(
            module_path("module\tlocal-tool\n").as_deref(),
            Some("local-tool")
        );
        // No directive, or a keyword that only starts with `module`.
        assert_eq!(module_path("go 1.22\nrequire x v1\n"), None);
        assert_eq!(module_path("modules x\n"), None);
        assert_eq!(module_path("module \n"), None);
    }

    #[test]
    fn discover_finds_the_nearest_go_mod_and_orders_longest_path_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("go.mod"), "module example.com/shop\n").unwrap();
        std::fs::create_dir_all(root.join("tools/gen")).unwrap();
        std::fs::write(root.join("tools/go.mod"), "module example.com/shop/tools\n").unwrap();
        let files = ["cmd/main.go", "internal/admin/admin.go", "tools/gen/gen.go"];
        assert_eq!(
            discover(root, files),
            vec![
                GoModule {
                    root: "tools".into(),
                    path: "example.com/shop/tools".into()
                },
                GoModule {
                    root: String::new(),
                    path: "example.com/shop".into()
                },
            ]
        );
    }

    #[test]
    fn discover_stops_at_the_nearest_go_mod_so_an_ancestor_owning_no_file_is_absent() {
        // Every Go file sits under `svc/`, which has its own go.mod. The root
        // go.mod owns none of them, so it declares no module an import could be
        // anchored on — else `example.com/shop/svc/x` would bind through it.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("go.mod"), "module example.com/shop\n").unwrap();
        std::fs::create_dir_all(root.join("svc/x")).unwrap();
        std::fs::write(root.join("svc/go.mod"), "module example.com/svc\n").unwrap();
        assert_eq!(
            discover(root, ["svc/main.go", "svc/x/x.go"]),
            vec![GoModule {
                root: "svc".into(),
                path: "example.com/svc".into()
            }]
        );
    }

    #[test]
    fn discover_reads_nothing_without_go_files_and_finds_nothing_without_go_mod() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("go.mod"), "module example.com/shop\n").unwrap();
        assert!(discover(tmp.path(), std::iter::empty()).is_empty());
        let bare = tempfile::tempdir().unwrap();
        assert!(discover(bare.path(), ["main.go"]).is_empty());
    }
}
