//! Ruby requires bind to their files (S-566, CR-179 §3.2 (2), FR-RS-34,
//! NFR-RA-05, NFR-RA-06) — exercised end-to-end through the public [`Engine`]
//! façade against real temp-directory fixtures.
//!
//! - `require_relative 'x'` binds the file beside the requiring one, and its
//!   `..` hops are kept — rack's `require_relative 'abstract/handler'` and
//!   `'../lib/rack/lint'`, the shape 243 of its 299 requires take;
//! - `require 'a/b'` binds against the load roots: each gem's `require_paths`
//!   (`lib/` unless its gemspec says otherwise) and the repository root, to
//!   exactly one file across them;
//! - a require no root holds a file for (`require 'json'`) stays unbound, and
//!   so does one two roots answer, or one naming only a directory;
//! - a sync that edits a gemspec or adds the file a require names reads the
//!   same as a cold index of the same tree.

#![cfg(feature = "lang-ruby")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

/// Every `Imports` edge as `(source file, target file)`, sorted.
fn import_edges(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(|store| {
        let file: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.file_path.unwrap_or_default()))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .map(|e| (file[&e.source].clone(), file[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The files the `Imports` edges out of `rel` reach.
fn imported_files(rt: &Runtime, rel: &str) -> Vec<String> {
    import_edges(rt)
        .into_iter()
        .filter(|(source, _)| source == rel)
        .map(|(_, target)| target)
        .collect()
}

/// One `Imports` ledger row: `(file, line, target, resolved)`.
type ImportRow = (String, Option<i64>, String, bool);

/// Every Ruby `Imports` ledger row, sorted.
fn import_rows(rt: &Runtime) -> Vec<ImportRow> {
    rt.submit_read(|store| {
        let path_of: HashMap<i64, String> = store.indexed_files()?.into_iter().map(|f| (f.id, f.path)).collect();
        let mut out: Vec<ImportRow> = store
            .unresolved_refs()?
            .into_iter()
            .filter(|r| r.kind == EdgeKind::Imports)
            .map(|r| {
                let path = r.file_id.and_then(|id| path_of.get(&id).cloned()).unwrap_or_default();
                (path, r.line, r.target, r.resolved)
            })
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// Whether the one `Imports` row of `rel` targeting `target` is bound.
fn row_bound(rt: &Runtime, rel: &str, target: &str) -> bool {
    let rows: Vec<bool> = import_rows(rt)
        .into_iter()
        .filter(|(path, _, t, _)| path == rel && t == target)
        .map(|(_, _, _, resolved)| resolved)
        .collect();
    assert_eq!(rows.len(), 1, "one ledger row for {rel} → {target:?}: {:?}", import_rows(rt));
    rows[0]
}

/// The file module nodes the store holds, by path — a guard that a fixture
/// file was indexed at all, so an unbound row is the binder's verdict.
fn indexed(rt: &Runtime, rel: &str) -> bool {
    let wanted = rel.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .any(|n| n.kind == NodeKind::Module && n.file_path.as_deref() == Some(wanted.as_str())))
    })
    .expect("read runs")
}

const GEMSPEC: &str = "require_relative 'lib/rack/version'\n\nGem::Specification.new do |s|\n  s.name = \"rack\"\n  s.files = Dir['lib/**/*']\nend\n";

/// rack's layout: a gemspec at the root, the library under `lib/`, its tests
/// under `test/`.
fn rack() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "rack.gemspec", GEMSPEC);
    write(tmp.path(), "lib/rack.rb", "module Rack\nend\n");
    write(tmp.path(), "lib/rack/version.rb", "module Rack\n  VERSION = \"3.1.0\"\nend\n");
    write(tmp.path(), "lib/rack/lint.rb", "module Rack\n  class Lint\n  end\nend\n");
    write(
        tmp.path(),
        "lib/rack/auth/abstract/handler.rb",
        "module Rack\n  module Auth\n    class AbstractHandler\n    end\n  end\nend\n",
    );
    write(
        tmp.path(),
        "lib/rack/auth/basic.rb",
        "require_relative 'abstract/handler'\n\nmodule Rack\n  module Auth\n    class Basic < AbstractHandler\n    end\n  end\nend\n",
    );
    write(
        tmp.path(),
        "test/spec_files.rb",
        "require_relative 'helper'\nrequire_relative '../lib/rack/lint'\nrequire 'json'\n\ndescribe Rack::Lint do\nend\n",
    );
    write(tmp.path(), "test/helper.rb", "require 'minitest/autorun'\n");
    write(tmp.path(), "test/cgi/rackup_stub.rb", "require 'rack'\nrequire 'rack/lint.rb'\n");
    tmp
}

#[test]
fn a_require_relative_binds_the_file_beside_the_requirer_and_keeps_its_parent_hops() {
    let tmp = rack();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // R1: `lib/rack/auth/basic.rb:1`.
    assert_eq!(imported_files(rt, "lib/rack/auth/basic.rb"), ["lib/rack/auth/abstract/handler.rb"]);
    assert!(row_bound(rt, "lib/rack/auth/basic.rb", ".::abstract::handler"));
    // One row per require: the bare pattern never also captures it.
    let rows = import_rows(rt).into_iter().filter(|(path, ..)| path == "lib/rack/auth/basic.rb").count();
    assert_eq!(rows, 1, "{:?}", import_rows(rt));
    // R2: the `..` hop is kept, and the sibling `helper` binds too.
    assert_eq!(imported_files(rt, "test/spec_files.rb"), ["lib/rack/lint.rb", "test/helper.rb"]);
    assert!(row_bound(rt, "test/spec_files.rb", "..::lib::rack::lint"));
    // The gemspec is a Ruby file since S-564, so its own `require_relative` binds
    // as any other: the version file beside it.
    assert_eq!(imported_files(rt, "rack.gemspec"), ["lib/rack/version.rb"]);
}

#[test]
fn a_bare_require_binds_under_the_gems_lib() {
    let tmp = rack();
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    // R3: `require 'rack'` loads `lib/rack.rb`; a trailing `.rb` names the file.
    assert_eq!(imported_files(rt, "test/cgi/rackup_stub.rb"), ["lib/rack.rb", "lib/rack/lint.rb"]);
    assert!(row_bound(rt, "test/cgi/rackup_stub.rb", "rack"));
    assert!(row_bound(rt, "test/cgi/rackup_stub.rb", "rack::lint.rb"));
}

#[test]
fn a_require_of_an_external_gem_stays_unbound() {
    let tmp = rack();
    // Under the gem's `lib/`, the stem `json` is held by a directory and by a
    // file of another language — neither is a Ruby file it loads.
    write(tmp.path(), "lib/json/ext.rb", "module JSON\nend\n");
    write(tmp.path(), "lib/json.py", "def dumps(x):\n    return x\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(indexed(rt, "lib/json/ext.rb"));
    assert!(indexed(rt, "lib/json.py"));
    assert!(!row_bound(rt, "test/spec_files.rb", "json"), "`require 'json'` stays unbound");
    assert!(!row_bound(rt, "test/helper.rb", "minitest::autorun"));
}

#[test]
fn without_a_gemspec_lib_is_no_load_root_and_the_repository_root_is() {
    // The near miss of the gemspec gate: the same `lib/json.rb`, and no gem
    // whose `require_paths` put `lib/` on the load path.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "lib/json.rb", "module JSON\nend\n");
    write(tmp.path(), "support.rb", "module Support\nend\n");
    write(tmp.path(), "app/main.rb", "require 'json'\nrequire 'support'\nrequire 'lib/json'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(indexed(rt, "lib/json.rb"));
    assert!(!row_bound(rt, "app/main.rb", "json"));
    assert_eq!(imported_files(rt, "app/main.rb"), ["lib/json.rb", "support.rb"]);
    assert!(row_bound(rt, "app/main.rb", "lib::json"));
}

#[test]
fn a_gemspecs_require_paths_replace_lib() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "gems/tool/tool.gemspec",
        "Gem::Specification.new do |spec|\n  spec.name = 'tool'\n  spec.require_paths = %w[src]\nend\n",
    );
    write(tmp.path(), "gems/tool/src/tool/cli.rb", "module Tool\nend\n");
    write(tmp.path(), "gems/tool/lib/tool/legacy.rb", "module Tool\nend\n");
    write(tmp.path(), "script/run.rb", "require 'tool/cli'\nrequire 'tool/legacy'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(indexed(rt, "gems/tool/lib/tool/legacy.rb"));
    assert_eq!(imported_files(rt, "script/run.rb"), ["gems/tool/src/tool/cli.rb"]);
    assert!(!row_bound(rt, "script/run.rb", "tool::legacy"), "`lib/` is not this gem's require path");
}

#[test]
fn a_require_two_load_roots_answer_stays_unbound() {
    let tmp = rack();
    // `lib/` (the gem) and the repository root both hold a `version.rb`.
    write(tmp.path(), "rack/version.rb", "module Rack\nend\n");
    write(tmp.path(), "script/rackup.rb", "require 'rack/version'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(indexed(rt, "rack/version.rb"));
    assert!(!row_bound(rt, "script/rackup.rb", "rack::version"), "ambiguous across roots: unbound");
}

#[test]
fn a_bare_dot_require_reads_against_the_load_roots_never_the_requiring_file() {
    // `require './x'` is relative to the working directory, conventionally the
    // repository root — never to the requiring file, which only
    // `require_relative` reads against.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "spec/spec_helper.rb", "module SpecHelper\nend\n");
    write(tmp.path(), "spec/foo_spec.rb", "require './spec/spec_helper'\n");
    write(tmp.path(), "config.rb", "module Config\nend\n");
    write(tmp.path(), "tools/config.rb", "module ToolsConfig\nend\n");
    write(tmp.path(), "tools/run.rb", "require './config'\nrequire '../outside'\nrequire '/opt/app/config'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "spec/foo_spec.rb"), ["spec/spec_helper.rb"]);
    assert_eq!(imported_files(rt, "tools/run.rb"), ["config.rb"], "not the `tools/config.rb` beside it");
    // A spec leaving the repository records no row at all.
    let rows: Vec<String> = import_rows(rt)
        .into_iter()
        .filter(|(path, ..)| path == "tools/run.rb")
        .map(|(_, _, target, _)| target)
        .collect();
    assert_eq!(rows, ["config"]);
}

#[test]
fn a_require_relative_never_binds_a_directory_index() {
    // A TypeScript `./auth` reaches `auth/index.ts`; Ruby loads `auth.rb` or
    // nothing.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "lib/auth/index.rb", "module Auth\nend\n");
    write(tmp.path(), "lib/app.rb", "require_relative 'auth'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert!(indexed(rt, "lib/auth/index.rb"));
    assert!(!row_bound(rt, "lib/app.rb", ".::auth"));
    assert!(imported_files(rt, "lib/app.rb").is_empty());
}

/// A graph's `Imports` edges and ledger rows.
type Reading = (Vec<(String, String)>, Vec<ImportRow>);

/// The cold-index reading of `tmp`'s tree, copied to a fresh directory.
fn cold(tmp: &TempDir, files: &[&str]) -> Reading {
    let fresh = TempDir::new().unwrap();
    for rel in files {
        write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let engine = index(&fresh);
    let rt = engine.runtime().unwrap();
    (import_edges(rt), import_rows(rt))
}

#[test]
fn a_gemspec_edit_and_an_added_file_rebind_requires_on_sync_as_a_cold_index_does() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "tool.gemspec", "Gem::Specification.new do |s|\nend\n");
    write(tmp.path(), "lib/tool.rb", "module Tool\nend\n");
    write(tmp.path(), "src/tool.rb", "module Tool\nend\n");
    write(tmp.path(), "script/run.rb", "require 'tool'\nrequire 'tool/extra'\nrequire_relative 'helper'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "script/run.rb"), ["lib/tool.rb"]);

    // A gemspec carries no node: only the descriptor moves the binding.
    write(tmp.path(), "tool.gemspec", "Gem::Specification.new do |s|\n  s.require_paths = ['src']\nend\n");
    engine.sync(&["tool.gemspec".into()]);
    assert_eq!(imported_files(rt, "script/run.rb"), ["src/tool.rb"]);
    let files = ["tool.gemspec", "lib/tool.rb", "src/tool.rb", "script/run.rb"];
    assert_eq!((import_edges(rt), import_rows(rt)), cold(&tmp, &files));

    // Adding the files two requires name binds them, with nothing else changed.
    write(tmp.path(), "src/tool/extra.rb", "module Tool\nend\n");
    write(tmp.path(), "script/helper.rb", "module Helper\nend\n");
    engine.sync(&["src/tool/extra.rb".into(), "script/helper.rb".into()]);
    assert_eq!(imported_files(rt, "script/run.rb"), ["script/helper.rb", "src/tool.rb", "src/tool/extra.rb"]);
    let files = ["tool.gemspec", "lib/tool.rb", "src/tool.rb", "script/run.rb", "src/tool/extra.rb", "script/helper.rb"];
    assert_eq!((import_edges(rt), import_rows(rt)), cold(&tmp, &files));
}

#[test]
fn a_nested_gemspec_edit_and_a_gemspec_deletion_rebind_on_sync_as_a_cold_index_does() {
    // A monorepo gem's gemspec (`activesupport/activesupport.gemspec`) is a
    // descriptor as a root one is, and so is one that is gone.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "gems/tool/tool.gemspec", "Gem::Specification.new do |s|\nend\n");
    write(tmp.path(), "gems/tool/lib/tool.rb", "module Tool\nend\n");
    write(tmp.path(), "gems/tool/src/tool.rb", "module Tool\nend\n");
    write(tmp.path(), "script/run.rb", "require 'tool'\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "script/run.rb"), ["gems/tool/lib/tool.rb"]);

    write(
        tmp.path(),
        "gems/tool/tool.gemspec",
        "Gem::Specification.new do |s|\n  s.require_paths = ['src']\nend\n",
    );
    engine.sync(&["gems/tool/tool.gemspec".into()]);
    assert_eq!(imported_files(rt, "script/run.rb"), ["gems/tool/src/tool.rb"]);
    let files = ["gems/tool/tool.gemspec", "gems/tool/lib/tool.rb", "gems/tool/src/tool.rb", "script/run.rb"];
    assert_eq!((import_edges(rt), import_rows(rt)), cold(&tmp, &files));

    // No gem left: `lib/` and `src/` are no load roots, the require binds nothing.
    fs::remove_file(tmp.path().join("gems/tool/tool.gemspec")).unwrap();
    engine.sync(&["gems/tool/tool.gemspec".into()]);
    assert!(imported_files(rt, "script/run.rb").is_empty(), "no gem: no load root");
    let files = ["gems/tool/lib/tool.rb", "gems/tool/src/tool.rb", "script/run.rb"];
    assert_eq!((import_edges(rt), import_rows(rt)), cold(&tmp, &files));
}

#[test]
#[cfg(all(feature = "lang-typescript", feature = "lang-go"))]
fn a_ruby_file_in_the_tree_leaves_typescript_and_go_imports_as_they_were() {
    // A Rails app with a TypeScript frontend and a Go tool: the load-path
    // rungs are keyed by the importing file's language, so a TS `./auth` keeps
    // its `index.ts` fallback and a Go module import its package.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "app.gemspec", "Gem::Specification.new do |s|\nend\n");
    write(tmp.path(), "lib/app.rb", "require_relative 'app/user'\n");
    write(tmp.path(), "lib/app/user.rb", "class User\nend\n");
    write(tmp.path(), "web/auth/index.ts", "export function login() { return 1; }\n");
    write(tmp.path(), "web/app.ts", "import { login } from './auth';\nexport const x = login();\n");
    write(tmp.path(), "go.mod", "module example.com/m\n\ngo 1.22\n");
    write(tmp.path(), "util/util.go", "package util\n\nfunc U() int { return 1 }\n");
    write(tmp.path(), "main.go", "package main\n\nimport \"example.com/m/util\"\n\nfunc main() { util.U() }\n");
    let engine = index(&tmp);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "lib/app.rb"), ["lib/app/user.rb"]);
    assert_eq!(imported_files(rt, "web/app.ts"), ["web/auth/index.ts"]);
    assert_eq!(imported_files(rt, "main.go"), ["util/util.go"]);
}
