//! `Gemfile` and `Rakefile` bind as a `.rake` file does (S-840, FR-EX-26,
//! FR-RS-33, FR-RS-34, CR-264) — exercised end to end through the public
//! [`Engine`] façade against the SHIPPED Ruby plugin.
//!
//! The Ruby plugin claims the two extension-less build files by exact name
//! (`filenames = ["Gemfile", "Rakefile"]`). Every Ruby lookup keyed on a file's
//! extension — lexical constants, the load path, a relative require, member
//! access — must ask that claim rather than the extension, or a require or a
//! constant written there stays unbound while the same line in a `.rake` file
//! binds. `Gemfile.lock` is no Ruby file and stays unclaimed.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite.

#![cfg(feature = "lang-ruby")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(root: &Path) -> Engine {
    coordinate::pin(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.pipeline().index();
    engine
}

fn indexed(fixture: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in fixture {
        write(tmp.path(), rel, text);
    }
    let engine = index(tmp.path());
    (tmp, engine)
}

fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
            })
            .collect())
    })
    .expect("read runs")
}

/// Every bound edge of `kind`, as `source -> target` labels, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Vec<String> {
    let label = labels(rt);
    let mut out: Vec<String> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| format!("{} -> {}", label[&e.source], label[&e.target]))
        .collect();
    out.sort();
    out
}

/// The files the `Imports` edges out of `rel` reach.
fn imported_files(rt: &Runtime, rel: &str) -> Vec<String> {
    let file: HashMap<NodeId, String> = rt
        .submit_read(|store| Ok(store.all_nodes()?.into_iter().map(|n| (n.id, n.file_path.unwrap_or_default())).collect()))
        .expect("read runs");
    let mut out: Vec<String> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Imports && file[&e.source] == rel)
        .map(|e| file[&e.target].clone())
        .collect();
    out.sort();
    out
}

const REQUIRE_APP: &str = "require_relative \"lib/app\"\n";

/// R1: `require_relative "lib/app"` binds from `Rakefile` and `Gemfile` as it
/// does from `root.rb` and from a `.rake` file; a bare `require 'app'` binds
/// against the load roots (the repository root's `lib/`-less name here) from
/// all four.
#[test]
fn a_relative_require_binds_from_a_rakefile_and_a_gemfile_as_from_a_rake_file() {
    let (_tmp, engine) = indexed(&[
        ("lib/app.rb", "module App\nend\n"),
        ("root.rb", REQUIRE_APP),
        ("Rakefile", REQUIRE_APP),
        ("Gemfile", REQUIRE_APP),
        ("tasks/x.rake", "require_relative \"../lib/app\"\n"),
    ]);
    let rt = engine.runtime().unwrap();
    for source in ["root.rb", "Rakefile", "Gemfile", "tasks/x.rake"] {
        assert_eq!(imported_files(rt, source), ["lib/app.rb"], "{source}");
    }
}

/// R1b: a bare `require` of a file under the gem's `lib/` binds against the
/// load roots from a `Rakefile`, a `Gemfile` and a `.rake` file alike.
#[test]
fn a_bare_require_binds_against_the_load_roots_from_a_rakefile_and_a_gemfile() {
    let (_tmp, engine) = indexed(&[
        ("tool.gemspec", "Gem::Specification.new do |s|\n  s.name = 'tool'\nend\n"),
        ("lib/tool.rb", "module Tool\nend\n"),
        ("Rakefile", "require 'tool'\n"),
        ("Gemfile", "require 'tool'\n"),
        ("tasks/x.rake", "require 'tool'\n"),
    ]);
    let rt = engine.runtime().unwrap();
    for source in ["Rakefile", "Gemfile", "tasks/x.rake"] {
        assert_eq!(imported_files(rt, source), ["lib/tool.rb"], "{source}");
    }
}

const BUILD_MODULE: &str = "module Build\n  def self.step\n    1\n  end\nend\n";

/// R2: `module Build` declared in `Rakefile` (or `Gemfile`) is the constant
/// `Build`, so `Build.step` from `lib/use.rb` binds it — as it does when the
/// module is declared in `tasks/b.rake`.
#[test]
fn a_module_declared_in_a_rakefile_is_reached_from_lib_as_one_in_a_rake_file_is() {
    for declaring in ["tasks/b.rake", "Rakefile", "Gemfile", "sub/Rakefile"] {
        let (_tmp, engine) = indexed(&[
            (declaring, BUILD_MODULE),
            ("lib/use.rb", "def go\n  Build.step\nend\n"),
        ]);
        let rt = engine.runtime().unwrap();
        assert_eq!(
            edges(rt, EdgeKind::Calls),
            [format!("lib/use.rb:go@1 -> {declaring}:step@2")],
            "Build.step reaches the module declared in {declaring}"
        );
    }
}

/// R2b: a constant is read through the lexical nesting from inside a
/// `Rakefile` too — `Helper` inside `module Build` is `Build::Helper`.
#[test]
fn a_constant_written_in_a_rakefile_is_read_through_its_lexical_nesting() {
    let rake = "module Build\n  class Helper\n    def self.run\n      1\n    end\n  end\n\n  def self.step\n    Helper.run\n  end\nend\n";
    for declaring in ["tasks/b.rake", "Rakefile"] {
        let (_tmp, engine) = indexed(&[(declaring, rake)]);
        let rt = engine.runtime().unwrap();
        assert_eq!(
            edges(rt, EdgeKind::Calls),
            [format!("{declaring}:step@8 -> {declaring}:run@3")],
            "{declaring}"
        );
    }
}

/// R3: `Gemfile.lock` stays unclaimed — it is no Ruby file, so it is not
/// indexed and a `Gemfile` beside it binds exactly as it does alone.
#[test]
fn a_gemfile_lock_stays_unclaimed() {
    let (_tmp, engine) = indexed(&[
        ("lib/app.rb", "module App\nend\n"),
        ("Gemfile", REQUIRE_APP),
        ("Gemfile.lock", REQUIRE_APP),
    ]);
    let rt = engine.runtime().unwrap();
    let mut files: Vec<String> = rt
        .submit_read(|store| Ok(store.indexed_files()?.into_iter().map(|f| f.path).collect()))
        .expect("read runs");
    files.sort();
    assert_eq!(files, ["Gemfile", "lib/app.rb"]);
    assert_eq!(imported_files(rt, "Gemfile"), ["lib/app.rb"]);
    assert!(imported_files(rt, "Gemfile.lock").is_empty());
}

/// R4: a repository with neither build file reads as it did — the same tree
/// gains a `Rakefile` and every other file's edges stay the same.
#[test]
fn a_tree_without_build_files_keeps_its_graph() {
    let files: &[(&str, &str)] = &[
        ("lib/app.rb", "module App\n  def self.run\n    1\n  end\nend\n"),
        ("lib/use.rb", "require_relative 'app'\n\ndef go\n  App.run\nend\n"),
    ];
    let (_a, plain) = indexed(files);
    let mut with_rakefile = files.to_vec();
    // A build file with requires and a module of its own: nothing it holds may
    // move another file's edges.
    with_rakefile.push(("Rakefile", "require 'json'\nrequire_relative 'lib/missing'\nmodule Tasks\nend\n"));
    let (_b, extended) = indexed(&with_rakefile);
    for kind in [EdgeKind::Calls, EdgeKind::Imports, EdgeKind::Instantiates] {
        assert_eq!(edges(plain.runtime().unwrap(), kind), edges(extended.runtime().unwrap(), kind), "{kind:?}");
    }
}

/// Every `Imports` and `Calls` edge, for comparing a synced graph with a cold
/// index of the same tree.
fn reading(rt: &Runtime) -> (Vec<String>, Vec<String>) {
    (edges(rt, EdgeKind::Imports), edges(rt, EdgeKind::Calls))
}

/// Sync ≡ reindex (NFR-RA-06): a `Rakefile` that gains the `require_relative`
/// and a `Gemfile` added by the sync leave the synced graph the fresh index of
/// the same tree, and the file a require names arriving later binds it.
#[test]
fn build_files_edited_or_added_by_a_sync_read_as_a_cold_index_does() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "Rakefile", "# tasks\n");
    write(tmp.path(), "lib/use.rb", "def go\n  Build.step\nend\n");
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(edges(rt, EdgeKind::Calls).is_empty());

    // The Rakefile gains the module and a require; a Gemfile and the required file arrive.
    write(tmp.path(), "Rakefile", &format!("{REQUIRE_APP}{BUILD_MODULE}"));
    write(tmp.path(), "Gemfile", REQUIRE_APP);
    write(tmp.path(), "lib/app.rb", "module App\nend\n");
    engine.pipeline().sync(&["Rakefile".into(), "Gemfile".into(), "lib/app.rb".into()]);
    assert_eq!(imported_files(rt, "Rakefile"), ["lib/app.rb"]);
    assert_eq!(imported_files(rt, "Gemfile"), ["lib/app.rb"]);
    assert_eq!(edges(rt, EdgeKind::Calls), ["lib/use.rb:go@1 -> Rakefile:step@3"]);

    let fresh = TempDir::new().unwrap();
    for rel in ["Rakefile", "Gemfile", "lib/use.rb", "lib/app.rb"] {
        write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold = index(fresh.path());
    assert_eq!(reading(rt), reading(cold.runtime().unwrap()));

    // The module leaves the Rakefile: the call unbinds on the sync as on a cold index.
    write(tmp.path(), "Rakefile", REQUIRE_APP);
    engine.pipeline().sync(&["Rakefile".into()]);
    assert!(edges(rt, EdgeKind::Calls).is_empty());
    write(fresh.path(), "Rakefile", REQUIRE_APP);
    let cold = index(fresh.path());
    assert_eq!(reading(rt), reading(cold.runtime().unwrap()));
}

/// A gemspec edit alone re-binds the bare requires of a `Rakefile` and a
/// `Gemfile` it does not name (NFR-RA-06): they are path-specifier files, so the
/// incremental run selects their import rows (`is_path_specifier_file`) though
/// neither is in the synced set — and the result is the cold index's. The
/// gemspec is named apart from the required `tool`, so no dirty token of the
/// sync re-selects the rows: only the path-specifier rung does.
#[test]
fn a_gemspec_edit_rebinds_the_requires_of_build_files_the_sync_did_not_name() {
    let files = ["pack.gemspec", "lib/tool.rb", "src/tool.rb", "Rakefile", "Gemfile", "tasks/x.rake"];
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "pack.gemspec", "Gem::Specification.new do |s|\nend\n");
    write(tmp.path(), "lib/tool.rb", "module Tool\nend\n");
    write(tmp.path(), "src/tool.rb", "module Tool\nend\n");
    for requirer in ["Rakefile", "Gemfile", "tasks/x.rake"] {
        write(tmp.path(), requirer, "require 'tool'\n");
    }
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    for requirer in ["Rakefile", "Gemfile", "tasks/x.rake"] {
        assert_eq!(imported_files(rt, requirer), ["lib/tool.rb"], "{requirer}");
    }

    write(tmp.path(), "pack.gemspec", "Gem::Specification.new do |s|\n  s.require_paths = ['src']\nend\n");
    engine.pipeline().sync(&["pack.gemspec".into()]);
    for requirer in ["Rakefile", "Gemfile", "tasks/x.rake"] {
        assert_eq!(imported_files(rt, requirer), ["src/tool.rb"], "{requirer} follows the gemspec");
    }
    let fresh = TempDir::new().unwrap();
    for rel in files {
        write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
    }
    let cold = index(fresh.path());
    assert_eq!(reading(rt), reading(cold.runtime().unwrap()));
}
