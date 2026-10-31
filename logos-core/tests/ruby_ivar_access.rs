//! Ruby instance-variable reads and writes are accesses (S-569, CR-179,
//! [FR-EX-27], [FR-QM-11]): a method's `@x` — read, assigned, or both — is an
//! `Accesses` edge to the Field its class declares for `@x` (S-564), and Ruby
//! declares `member_access`, so Cohesion scores its classes from those edges.
//!
//! Every expectation is on the shipped `references.scm` and `plugin.toml`,
//! never a query override. Checks run through the public [`Engine`].
//!
//! [FR-EX-27]: ../../docs/specs/requirements/FR-EX-27.md
//! [FR-QM-11]: ../../docs/specs/requirements/FR-QM-11.md
#![cfg(feature = "lang-ruby")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::EdgeKind;
use logos_core::{metrics, Engine, Granularity, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;
#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every bound `Accesses` edge as `(source name, field name)`, sorted.
fn accesses(rt: &Runtime) -> Vec<(String, String)> {
    let mut pairs = rt
        .submit_read(|store| {
            let names: std::collections::HashMap<_, _> =
                store.all_nodes()?.into_iter().map(|n| (n.id, n.name)).collect();
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Accesses)
                .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    pairs.sort();
    pairs
}

/// The unresolved `Accesses` targets the ledger holds, sorted.
fn unresolved_accesses(rt: &Runtime) -> Vec<String> {
    let mut targets: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Accesses && !r.resolved)
        .map(|r| r.target)
        .collect();
    targets.sort();
    targets
}

fn pair(source: &str, field: &str) -> (String, String) {
    (source.to_string(), field.to_string())
}

/// lobsters' `Search` shape (`app/models/search.rb`): `initialize` assigns `@q`,
/// `@what` and `@page` (also an `attr_reader`), `to_url_params` and `what_class` read them. `@missing` is read
/// and never assigned, so no Field declares it.
const SEARCH: &str = "\
class Search
  attr_reader :page

  def initialize(params)
    @q = params[:q]
    @what = params[:what]
    @page = 1
  end

  def to_url_params
    { q: @q, what: @what, page: @page, miss: @missing }
  end

  def what_class
    @what ||= \"all\"
  end

  def current_page
    page
  end
end
";

#[test]
fn an_ivar_read_or_write_in_a_method_binds_to_its_classs_field() {
    let tmp = tree(&[("app/models/search.rb", SEARCH)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        accesses(rt),
        [
            pair("initialize", "@page"),
            pair("initialize", "@q"),
            pair("initialize", "@what"),
            pair("to_url_params", "@page"),
            pair("to_url_params", "@q"),
            pair("to_url_params", "@what"),
            pair("what_class", "@what"),
        ],
        "each `@x` of a declared Field is one Accesses edge, a write as much as a read (FR-EX-27)"
    );
    assert_eq!(
        unresolved_accesses(rt),
        ["@missing"],
        "an `@x` no class declares stays in the ledger, unbound (NFR-RA-05)"
    );
}

#[test]
fn an_ivar_of_another_class_binds_nothing_across_the_class_boundary() {
    let tmp = tree(&[(
        "a.rb",
        "\
class A
  def set
    @shared = 1
  end
end

class B
  def read
    @shared
  end
end
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        accesses(rt),
        [pair("set", "@shared")],
        "`A#set` binds the Field `A` declares; `@shared` is no Field of `B`, so `B#read` binds nothing"
    );
    assert_eq!(unresolved_accesses(rt), ["@shared"], "and `B#read`'s access stays in the ledger (NFR-RA-05)");
}

/// A `module` nested in a class (a Rails concern's `ClassMethods`) is a mixin: its
/// methods' `@x` is whatever object extends or includes it, never the class
/// around it, so it binds nothing — as the `@x` of a top-level module does.
#[test]
fn an_ivar_in_a_module_nested_in_a_class_binds_nothing() {
    let tmp = tree(&[(
        "a.rb",
        "\
class Foo
  def initialize
    @x = 1
  end

  def m
    @x
  end

  module ClassMethods
    def cm
      @x
    end
  end
end

module Util
  def helper
    @cache
  end
end
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        accesses(rt),
        [pair("initialize", "@x"), pair("m", "@x")],
        "`ClassMethods#cm` is no method of `Foo`: its `@x` is not `Foo`'s Field"
    );
    assert_eq!(unresolved_accesses(rt), ["@cache", "@x"], "the nested module's and the top-level module's reads stay in the ledger");
}

/// Every form a Ruby method writes or reads `@x` by records one access: a
/// multiple assignment's targets, an operator assignment, an interpolation, a
/// receiver, an argument, and a block body of the method.
#[test]
fn every_ivar_form_in_a_method_is_one_access_per_field() {
    let tmp = tree(&[(
        "a.rb",
        "\
class Forms
  def multi
    @a, @b = 1, 2
  end

  def reads
    @a += 1
    puts \"#{@b}\", @a.to_s
    [1].each { |i| @c = i }
  end
end
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        accesses(rt),
        [pair("multi", "@a"), pair("multi", "@b"), pair("reads", "@a"), pair("reads", "@b"), pair("reads", "@c")],
        "one edge per (method, Field): `@a` read twice in `reads` is one"
    );
}

/// A class-body `@x` and a singleton method's `@x` are the class object's own
/// state, which the graph cannot tell from the instance Field of the same name
/// (the limitation `references.scm` documents): they bind to it. The class-body
/// access is recorded from the class itself, which Cohesion ignores.
#[test]
fn class_level_and_singleton_ivars_bind_to_the_one_field_of_that_name() {
    let tmp = tree(&[(
        "a.rb",
        "\
class Counter
  @count = 0

  def self.bump
    @count += 1
  end

  def read
    @count
  end
end
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        accesses(rt),
        [pair("Counter", "@count"), pair("bump", "@count"), pair("read", "@count")],
        "the class body, the singleton method and the instance method share the one `@count` Field"
    );
}

/// A class nested in another binds its own Fields only: the inner method's `@x`
/// is not the outer class's Field, and the outer's `@y` is not the inner's.
#[test]
fn a_nested_class_binds_its_own_fields_and_never_its_enclosers() {
    let tmp = tree(&[(
        "a.rb",
        "\
class Outer
  def initialize
    @y = 1
  end

  def read_y
    @y
  end

  class Inner
    def initialize
      @x = 1
    end

    def read_y
      @y
    end

    def read_x
      @x
    end
  end
end
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        accesses(rt),
        [pair("initialize", "@x"), pair("initialize", "@y"), pair("read_x", "@x"), pair("read_y", "@y")],
        "`Inner#read_y` binds nothing: `@y` is a Field of `Outer`, not of `Inner`"
    );
    assert_eq!(unresolved_accesses(rt), ["@y"]);
}

#[test]
fn a_class_variable_and_a_global_are_not_instance_variable_accesses() {
    let tmp = tree(&[(
        "a.rb",
        "\
class A
  def initialize
    @x = 1
  end

  def read
    [@@count, $glob, @x]
  end
end
",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(accesses(rt), [pair("initialize", "@x"), pair("read", "@x")]);
    assert_eq!(unresolved_accesses(rt), Vec::<String>::new(), "`@@count` and `$glob` record no access at all");
}

/// A synced edit leaves the graph a fresh index of the edited tree builds
/// ([NFR-RA-06]): the edit gives `@b` its first writer, so the read that was
/// unresolved before binds after.
///
/// [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn a_synced_edit_matches_a_fresh_reindex() {
    const BEFORE: &str = "\
class Box
  def initialize
    @a = 1
  end

  def total
    @a + @b
  end
end
";
    const AFTER: &str = "\
class Box
  def initialize
    @a = 1
    @b = 2
  end

  def total
    @a + @b
  end

  def reset
    @a = 0
  end
end
";
    let tmp_a = tree(&[("app/box.rb", BEFORE)]);
    let engine_a = index(tmp_a.path());
    assert_eq!(accesses(engine_a.runtime().unwrap()), [pair("initialize", "@a"), pair("total", "@a")]);
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("app/box.rb"), AFTER).unwrap();
    engine_a.sync(&[root_a.join("app/box.rb")] as &[PathBuf]);
    let rt_a = engine_a.runtime().unwrap();

    let tmp_b = tree(&[("app/box.rb", AFTER)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    assert_eq!(
        accesses(rt_a),
        [
            pair("initialize", "@a"),
            pair("initialize", "@b"),
            pair("reset", "@a"),
            pair("total", "@a"),
            pair("total", "@b"),
        ]
    );
}

/// Cohesion over `source`, read under a minimum of one class, with the
/// extensions that declare `member_access` as the metrics see them.
fn cohesion(source: &str) -> Option<f64> {
    let tmp = tree(&[("shared.rb", source)]);
    let engine = index(tmp.path());
    let view = engine.hydrate(Granularity::ExcludeContains).expect("dependency view hydrates");
    let rt = engine.runtime().unwrap();
    let thresholds = metrics::Thresholds { cohesion_min_classes: 1, ..metrics::Thresholds::default() };
    let member_access = engine.registry().expect("a started engine holds its registry").member_access_extensions();
    assert!(member_access.contains("rb"), "Ruby declares member_access: {member_access:?}");
    metrics::snapshot(rt, &view, None, thresholds, &member_access).expect("snapshot runs").metrics.cohesion.map(|c| c.raw)
}

/// LCOM4 reads the Ruby fields ([FR-QM-11]): `Shared`'s methods meet through
/// `@http`, so they are one component (cohesion 1); `Split`'s two readers touch
/// disjoint fields, so they are two (1/2). Mean 0.75 — unreachable without the
/// accesses, where every method is its own component.
///
/// [FR-QM-11]: ../../docs/specs/requirements/FR-QM-11.md
#[test]
fn lcom4_reflects_the_shared_ruby_fields() {
    let raw = cohesion(
        "\
class Shared
  def initialize(http)
    @count = 0
    @http = http
  end

  def a
    @count + @http
  end

  def b
    @http
  end
end

class Split
  attr_reader :x, :y

  def get_x
    @x
  end

  def get_y
    @y
  end
end
",
    )
    .expect("two Ruby classes with methods → Cohesion applies");
    assert!((raw - 0.75).abs() < 1e-12, "mean of Shared 1/1 and Split 1/2 = 0.75, got {raw}");
}
