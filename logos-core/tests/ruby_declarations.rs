//! Ruby declarations are complete, and Ruby build files are indexed (S-564,
//! CR-179, [FR-EX-26]): a compact `class A::B` keeps its body, a setter or
//! operator `def` is a Method, `attr_*` symbols and the first assignment of each
//! `@ivar` are Fields of their class, and `.rake`, `.ru`, `.gemspec`, `Gemfile`
//! and `Rakefile` are Ruby files.
//!
//! Capture-level checks run one in-memory file through the embedded Ruby plugin;
//! the indexed-file checks go through the public [`Engine`]. Every expectation
//! is on the shipped `symbols.scm` and `plugin.toml`, never a query override.
//!
//! [FR-EX-26]: ../../docs/specs/requirements/FR-EX-26.md
#![cfg(feature = "lang-ruby")]

use std::fs;
use std::path::Path;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::plugin::LanguageRegistry;
use logos_core::Engine;

#[path = "support/coordinate.rs"]
mod coordinate;
#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn extract_rb(source: &str) -> Facts {
    extract_as("a.rb", source)
}

fn extract_as(path: &str, source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_path(path).unwrap_or_else(|| panic!("the Ruby plugin claims {path}"));
    assert_eq!(plugin.name(), "ruby", "{path}");
    let facts = extract(&FileInput::new(path, source), plugin, &SymbolContext::default());
    assert!(facts.warnings.is_empty(), "{path}: {:?}", facts.warnings);
    facts
}

/// `name:kind` of every node, in source order.
fn declared(facts: &Facts) -> Vec<String> {
    facts.nodes.iter().map(|n| format!("{}:{}", n.name, n.kind.as_str())).collect()
}

/// Every symbol, minus the `logos . . . ` prefix, with its kind.
fn symbols(facts: &Facts, kind: NodeKind) -> Vec<String> {
    facts
        .nodes
        .iter()
        .filter(|n| n.kind == kind)
        .map(|n| n.symbol.as_str().trim_start_matches("logos . . . ").to_string())
        .collect()
}

fn strings(of: &[&str]) -> Vec<String> {
    of.iter().map(|s| s.to_string()).collect()
}

fn span_of(facts: &Facts, name: &str) -> (u32, u32) {
    let node = facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no node {name}: {:?}", declared(facts)));
    (node.start_line, node.end_line)
}

/// The `Contains` edges as `parent -> child`, minus the symbol prefix.
fn contains(facts: &Facts) -> Vec<String> {
    let trim = |s: &str| s.trim_start_matches("logos . . . ").to_string();
    facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| format!("{} -> {}", trim(e.source.as_str()), trim(e.target.as_str())))
        .collect()
}

#[test]
fn a_compact_class_keeps_its_span_and_contains_its_methods() {
    let facts = extract_rb(
        "\
class Mod::FlaggedController < Base
  def index
    1
  end

  def create
    2
  end
end

module Outer::Inner
  def helper
  end
end

class Plain
end
",
    );
    assert_eq!(span_of(&facts, "FlaggedController"), (1, 9), "the class, not the scope `Mod::FlaggedController`");
    assert_eq!(span_of(&facts, "Inner"), (11, 14));
    assert_eq!(span_of(&facts, "Plain"), (16, 17));
    assert_eq!(
        symbols(&facts, NodeKind::Method),
        strings(&["`a.rb`/FlaggedController#index().", "`a.rb`/FlaggedController#create().", "`a.rb`/Inner/helper()."])
    );
    // A method directly under the file is a method the class lost.
    assert!(
        !contains(&facts).iter().any(|e| e.ends_with("-> `a.rb`/index().") || e.ends_with("-> `a.rb`/helper().")),
        "{:?}",
        contains(&facts)
    );
}

#[test]
fn a_fully_qualified_and_a_three_deep_compact_declaration_keep_their_bodies() {
    let facts = extract_rb("class ::A::B\n  def m\n  end\nend\n\nmodule P::Q::R\n  def n\n  end\nend\n\nmodule S::T; def v; end; end\n");
    assert_eq!(span_of(&facts, "B"), (1, 4));
    assert_eq!(span_of(&facts, "R"), (6, 9));
    assert_eq!(span_of(&facts, "T"), (11, 11));
    assert_eq!(
        symbols(&facts, NodeKind::Method),
        strings(&["`a.rb`/B#m().", "`a.rb`/R/n().", "`a.rb`/T/v()."])
    );
}

#[test]
fn a_deeply_compact_class_and_a_class_nested_in_one_keep_their_bodies() {
    let facts = extract_rb(
        "\
class A::B::C
  class D::E
    def deep
    end
  end

  def shallow
  end
end

def after
end
",
    );
    assert_eq!(span_of(&facts, "C"), (1, 9));
    assert_eq!(span_of(&facts, "E"), (2, 5));
    assert_eq!(
        symbols(&facts, NodeKind::Method),
        strings(&["`a.rb`/C#E#deep().", "`a.rb`/C#shallow().", "`a.rb`/after()."])
    );
}

#[test]
fn a_setter_and_an_operator_def_is_a_method() {
    let source = "\
class Vec
  def x=(v)
    @x = v
  end

  def self.origin=(v)
    v
  end

  def [](i)
    i
  end

  def []=(i, v)
    v
  end

  def <=>(other)
    0
  end

  def ==(other)
    true
  end

  def +(other)
    other
  end

  def -@
    self
  end

  def !
    false
  end

  def call(a)
    a
  end

  def Konst
    1
  end
end
";
    let facts = extract_rb(source);
    let defs = source.lines().filter(|l| l.trim_start().starts_with("def ")).count();
    let methods: Vec<&str> = facts.nodes.iter().filter(|n| n.kind == NodeKind::Method).map(|n| n.name.as_str()).collect();
    assert_eq!(
        methods,
        ["x=", "origin=", "[]", "[]=", "<=>", "==", "+", "-@", "!", "call", "Konst"],
        "one Method per `def`"
    );
    assert_eq!(methods.len(), defs, "Method nodes equal `def` lines");
}

#[test]
fn a_def_named_by_a_symbol_or_a_variable_is_no_method() {
    // Valid to the grammar, a method to none: its name is not an identifier,
    // constant, setter or operator.
    let facts = extract_rb("class A\n  def real\n  end\n  def @x\n  end\n  def :sym\n  end\n  def $g\n  end\nend\n");
    let methods: Vec<&str> = facts.nodes.iter().filter(|n| n.kind == NodeKind::Method).map(|n| n.name.as_str()).collect();
    assert_eq!(methods, ["real"]);
}

#[test]
fn attr_symbols_and_the_first_assignment_of_each_ivar_are_fields_of_their_class() {
    let facts = extract_rb(
        "\
class Account
  attr_accessor :balance, :owner
  attr_reader \"id\"
  attr_writer :note

  def initialize(b)
    @balance = b
    @total ||= 0
    @a, @b = 1, 2
  end

  def deposit(n)
    @balance = @balance + n
    @fresh = n
  end

  def self.reset
    @count = 0
  end
end
",
    );
    let fields: Vec<&str> = facts.nodes.iter().filter(|n| n.kind == NodeKind::Field).map(|n| n.name.as_str()).collect();
    assert_eq!(
        fields,
        ["@balance", "@owner", "@id", "@note", "@total", "@a", "@b", "@fresh", "@count"],
        "an ivar named by an attr and assigned later is one Field, at its first occurrence"
    );
    assert_eq!(
        symbols(&facts, NodeKind::Field),
        strings(&[
            "`a.rb`/Account#`@balance`.",
            "`a.rb`/Account#`@owner`.",
            "`a.rb`/Account#`@id`.",
            "`a.rb`/Account#`@note`.",
            "`a.rb`/Account#`@total`.",
            "`a.rb`/Account#`@a`.",
            "`a.rb`/Account#`@b`.",
            "`a.rb`/Account#`@fresh`.",
            "`a.rb`/Account#`@count`.",
        ])
    );
}

#[test]
fn a_field_owns_no_call_of_the_method_that_assigns_it() {
    let facts = extract_rb(
        "\
class Svc
  def run
    @conn = connect(1)
  end
end
",
    );
    let from: Vec<String> = facts
        .refs
        .iter()
        .filter(|r| r.target.contains("connect"))
        .map(|r| r.source.as_str().trim_start_matches("logos . . . ").to_string())
        .collect();
    assert_eq!(from, ["`a.rb`/Svc#run()."], "the call stays the method's: {from:?}");
}

#[test]
fn state_outside_a_class_or_in_a_non_ivar_shape_is_no_field() {
    let facts = extract_rb(
        "\
@top = 1
attr_accessor :loose
x = 1
$g = 2
@@cv = 3

class K
  other.attr_accessor :recv
  attr_accessor
  attr_accessor 7
  attr_accessor foo
  @@shared = 1
  local = 2
  def m
    local = 3
    @ok = local
    @ok = 4
  end
end
",
    );
    let fields: Vec<&str> = facts.nodes.iter().filter(|n| n.kind == NodeKind::Field).map(|n| n.name.as_str()).collect();
    assert_eq!(fields, ["@ok"], "{:?}", declared(&facts));
}

#[test]
fn only_attr_reader_writer_and_accessor_with_plain_names_declare_fields() {
    let facts = extract_rb(
        "\
class K
  has_many :comments
  validates :name
  attribute :age
  attr_foo :x
  attr_reader \"a#{b}\"
  attr_reader \"#{b}\"
  attr_reader 'q'
  attr_reader(:p, :r)
end
",
    );
    let fields: Vec<&str> = facts.nodes.iter().filter(|n| n.kind == NodeKind::Field).map(|n| n.name.as_str()).collect();
    assert_eq!(fields, ["@q", "@p", "@r"], "a Rails macro or an interpolated name is no Field");
}

#[test]
fn each_class_owns_its_own_state_whatever_the_name_is_elsewhere() {
    let facts = extract_rb(
        "\
class A
  def a
    @x = 1
  end
end

class B
  def b
    @x = 1
  end
end

class Outer
  @a = 1
  class Inner
    @a = 2
  end
end

class A
  def again
    @x = 2
    @y = 3
  end
end
",
    );
    assert_eq!(
        symbols(&facts, NodeKind::Field),
        strings(&[
            "`a.rb`/A#`@x`.",
            "`a.rb`/B#`@x`.",
            "`a.rb`/Outer#`@a`.",
            "`a.rb`/Outer#Inner#`@a`.",
            "`a.rb`/A#1:`@x`.",
            "`a.rb`/A#1:`@y`.",
        ]),
        "a reopened class is a node of its own, and owns the state its body writes"
    );
}

#[test]
fn state_in_a_singleton_class_a_block_or_a_top_level_def_is_placed_by_its_class() {
    let facts = extract_rb(
        "\
class A
  class << self
    attr_accessor :config
    def build
      @inst = 1
    end
  end

  included do
    attr_accessor :inc
  end
end

def top_helper
  @h = 1
end
",
    );
    assert_eq!(
        symbols(&facts, NodeKind::Field),
        strings(&["`a.rb`/A#`@config`.", "`a.rb`/A#`@inst`.", "`a.rb`/A#`@inc`."]),
        "an ivar of a def outside any class or module is no Field"
    );
}

#[test]
fn a_module_state_is_a_field_of_the_module() {
    let facts = extract_rb("module Config\n  attr_reader :path\n  def load\n    @loaded = true\n  end\nend\n");
    assert_eq!(
        symbols(&facts, NodeKind::Field),
        strings(&["`a.rb`/Config/`@path`.", "`a.rb`/Config/`@loaded`."])
    );
}

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (rel, text) in files {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

#[test]
fn rake_ru_gemspec_gemfile_and_rakefile_are_indexed_and_a_lockfile_is_not() {
    for path in ["lib/tasks/db.rake", "config.ru", "rack.gemspec", "Gemfile", "sub/Gemfile", "Rakefile", "ext/Rakefile"] {
        let facts = extract_as(path, "class Tool\n  def run\n  end\nend\n");
        assert_eq!(
            facts.nodes.iter().filter(|n| n.kind == NodeKind::Method).count(),
            1,
            "{path} is parsed as Ruby"
        );
    }
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).unwrap();
    for path in ["Gemfile.lock", "Gemfile.lock.bak", "Rakefile.md", "gems.lock", "Gemfileish", "Gemfile/inner.txt"] {
        assert!(reg.for_path(path).is_none_or(|p| p.name() != "ruby"), "{path} is no Ruby file");
    }
}

#[test]
fn the_indexed_file_count_includes_ruby_build_files() {
    let tmp = tempfile::tempdir().unwrap();
    write_tree(
        tmp.path(),
        &[
            ("lib/app.rb", "class App\n  def run\n  end\nend\n"),
            ("lib/tasks/db.rake", "class DbTask\n  def migrate\n  end\nend\n"),
            ("config.ru", "class Boot\n  def go\n  end\nend\n"),
            ("Gemfile", "source 'https://rubygems.org'\ngem 'rack'\n"),
            ("Rakefile", "class Build\n  def all\n  end\nend\n"),
            ("Gemfile.lock", "GEM\n  remote: https://rubygems.org/\n  specs:\n    rack (3.0.0)\n"),
        ],
    );
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let mut files: Vec<String> = engine
        .runtime()
        .unwrap()
        .submit_read(|store| Ok(store.indexed_files()?.into_iter().map(|f| f.path).collect()))
        .expect("read runs");
    files.sort();
    assert_eq!(files, ["Gemfile", "Rakefile", "config.ru", "lib/app.rb", "lib/tasks/db.rake"]);
}

/// Sync ≡ reindex ([NFR-RA-06]): a compact class that gains a setter and a
/// field, a call to a method after its `end`, and a `Gemfile` and a `.rake`
/// file added by the sync each leave the synced graph the fresh index of the
/// final tree.
#[test]
fn sync_equals_a_full_reindex_as_declarations_and_build_files_arrive() {
    let initial = [
        (
            "lib/svc.rb",
            "module Mod\n  def self.after\n    0\n  end\nend\n\nclass Mod::Svc\n  def run\n    1\n  end\nend\n".to_string(),
        ),
        (
            "lib/use.rb",
            "class Use\n  def go\n    Mod::Svc.new\n    Mod.after\n  end\nend\n".to_string(),
        ),
    ];
    let edits = [
        (
            "lib/svc.rb",
            "module Mod\n  def self.after\n    0\n  end\nend\n\nclass Mod::Svc\n  attr_accessor :name\n  def run\n    @n = 1\n  end\n\n  def name=(v)\n    @name = v\n  end\nend\n".to_string(),
        ),
        ("Gemfile", "source 'https://rubygems.org'\n".to_string()),
        ("lib/tasks/svc.rake", "class SvcTask\n  def call\n  end\nend\n".to_string()),
    ];
    let tmp_a = tempfile::tempdir().unwrap();
    for (rel, text) in &initial {
        write_tree(tmp_a.path(), &[(rel, text)]);
    }
    coordinate::pin(tmp_a.path());
    let engine_a = Engine::start(tmp_a.path()).expect("engine starts");
    engine_a.index();
    let root = tmp_a.path().canonicalize().expect("canonical root");
    let changed: Vec<_> = edits
        .iter()
        .map(|(rel, text)| {
            write_tree(&root, &[(rel, text)]);
            root.join(rel)
        })
        .collect();
    engine_a.sync(&changed);

    let tmp_b = tempfile::tempdir().unwrap();
    for (rel, text) in std::iter::once(&initial[1]).chain(edits.iter()) {
        write_tree(tmp_b.path(), &[(rel, text)]);
    }
    coordinate::pin(tmp_b.path());
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.index();
    let (a, b) = (
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
    );
    assert!(a.contains("@name") && a.contains("SvcTask"), "the edits reached the synced graph");
    assert_eq!(a, b);
}
