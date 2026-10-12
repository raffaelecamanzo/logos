//! Go packages as DSM rows ([S-527], [FR-GV-07]).
//!
//! The module rollup ([`crate::hydrate`]) keys a node on its nearest enclosing
//! `Module` node, and every source file carries one synthetic `Module` node of
//! its own. For Rust that file module is nested under the real `mod` tree, so a
//! DSM row is a module. For Go the file module is all there is, and a Go
//! package is a *directory plus its `package` clause*: a Go DSM listed one row
//! per file.
//!
//! [`package_rows`] maps each Go file-module vertex key to the key of its
//! package, and nothing else. A vertex whose key it does not return keeps its
//! own key, so a project without Go files is untouched.
//!
//! The `package` clause is not in the graph, so it is read from the file the
//! module node is bound to, at report time: it is in step with the indexed graph
//! only when the report reconciled first (not under `--no-reconcile`). A file
//! that cannot be read, or whose clause cannot be found, falls back to its
//! directory alone, which makes a second row beside its siblings' package row;
//! [`GoPackages::directory_only`] counts those so the report can say so.
//!
//! [S-527]: ../../../docs/planning/journal.md#s-527-the-go-dsm-lists-packages
//! [FR-GV-07]: ../../../docs/specs/requirements/FR-GV-07.md

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::engine::Engine;
use crate::model::NodeKind;

use super::quality_runtime;

/// A file-module vertex key ends in the file's name: ``…/`a.go`/``.
const GO_FILE_MODULE_SUFFIX: &str = ".go`/";

/// The Go file-module vertices of a DSM and the package rows they roll up into.
#[derive(Debug, Default)]
pub(super) struct GoPackages {
    /// Go file-module vertex key → its package row key.
    pub(super) row_of: HashMap<String, String>,
    /// How many of those files had no readable `package` clause and were keyed
    /// by directory alone.
    pub(super) directory_only: usize,
}

/// For each Go file-module vertex key among `keys`, the key of the package row
/// it rolls up into. The read of the node rows happens only when some key looks
/// like a Go file module, so a project without Go pays nothing.
pub(super) fn package_rows<'a>(
    engine: &Engine,
    keys: impl Iterator<Item = &'a str>,
) -> Result<GoPackages> {
    let candidates: Vec<&str> = keys
        .filter(|k| k.starts_with("module:") && k.ends_with(GO_FILE_MODULE_SUFFIX))
        .collect();
    if candidates.is_empty() {
        return Ok(GoPackages::default());
    }

    let nodes = quality_runtime(engine)?.submit_read(|store| store.all_nodes())?;
    let go_files: HashMap<String, &str> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module)
        .filter_map(|n| {
            let path = n.file_path.as_deref().filter(|p| p.ends_with(".go"))?;
            Some((format!("module:{}", n.symbol.as_str()), path))
        })
        .collect();

    let root = engine.root();
    let mut packages = GoPackages::default();
    for key in candidates {
        let Some(path) = go_files.get(key) else {
            continue;
        };
        let clause = read_package_clause(&root.join(path));
        if clause.is_none() {
            packages.directory_only += 1;
        }
        packages
            .row_of
            .insert(key.to_string(), package_key(path, clause.as_deref()));
    }
    Ok(packages)
}

/// The package row key: `go:<directory>:<package>`, `<directory>` being `.` at
/// the project root. Without a clause it is `go:<directory>`.
fn package_key(file_path: &str, clause: Option<&str>) -> String {
    let dir = match file_path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir,
        _ => ".",
    };
    match clause {
        Some(name) => format!("go:{dir}:{name}"),
        None => format!("go:{dir}"),
    }
}

/// The `package` clause of the Go file at `path`, when it can be read and has one.
/// The whole file is read: a clause follows only comments, but a generated or
/// licensed header can be arbitrarily long, and a clause missed here puts one
/// package on two rows.
fn read_package_clause(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    package_clause(&String::from_utf8_lossy(&bytes)).map(str::to_string)
}

/// `s` without the whitespace and comments (`//` to end of line, `/* */`) that
/// lead it; `None` when a block comment never closes.
fn skip_trivia(mut s: &str) -> Option<&str> {
    loop {
        s = s.trim_start();
        if let Some(after) = s.strip_prefix("//") {
            s = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = s.strip_prefix("/*") {
            s = after.split_once("*/")?.1;
        } else {
            return Some(s);
        }
    }
}

/// The identifier of the first `package` clause in Go source `src`: the first
/// token that is not whitespace or a comment, when that token is `package`.
fn package_clause(src: &str) -> Option<&str> {
    let rest = skip_trivia(src.trim_start_matches('\u{feff}'))?.strip_prefix("package")?;
    // `package` must end at a boundary: `packages := 1` is not a clause.
    if !rest.starts_with(|c: char| c.is_whitespace() || c == '/') {
        return None;
    }
    let rest = skip_trivia(rest)?;
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clause_follows_any_amount_of_comments() {
        let src = "// Copyright\n// notice\n\n//go:build linux\n\n/* block\n package fake */\n// Package x does y.\npackage x\n\nfunc f() {}\n";
        assert_eq!(package_clause(src), Some("x"));
        assert_eq!(package_clause("package main"), Some("main"));
        assert_eq!(
            package_clause("\u{feff}package x_test // trailing\n"),
            Some("x_test")
        );
        assert_eq!(package_clause("package /* c */ y\n"), Some("y"));
        // A line comment may sit between the keyword and the name.
        assert_eq!(package_clause("package // c\nx\n"), Some("x"));
        assert_eq!(package_clause("package//c\nx\n"), Some("x"));
    }

    #[test]
    fn a_clause_far_past_a_long_header_is_still_found() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.go");
        std::fs::write(
            &file,
            format!("/*\n{}\n*/\npackage b\n", "x".repeat(200 * 1024)),
        )
        .unwrap();
        assert_eq!(read_package_clause(&file), Some("b".to_string()));
        assert_eq!(read_package_clause(&dir.path().join("missing.go")), None);
    }

    #[test]
    fn a_near_miss_is_not_a_clause() {
        assert_eq!(package_clause("packages := 1\n"), None);
        assert_eq!(package_clause("package\n"), None);
        assert_eq!(package_clause("import \"fmt\"\npackage x\n"), None);
        assert_eq!(package_clause("/* never closed\npackage x\n"), None);
        assert_eq!(package_clause(""), None);
    }

    #[test]
    fn a_package_key_is_directory_plus_clause() {
        assert_eq!(
            package_key("server/a.go", Some("server")),
            "go:server:server"
        );
        assert_eq!(
            package_key("server/a_test.go", Some("server_test")),
            "go:server:server_test"
        );
        assert_eq!(package_key("a.go", Some("zap")), "go:.:zap");
        assert_eq!(package_key("x/y/a.go", None), "go:x/y");
    }
}
