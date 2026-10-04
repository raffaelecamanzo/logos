//! TypeScript, TSX, JavaScript, Python, PHP and Ruby record their receivers'
//! shapes (S-515, CR-169, FR-EX-13, FR-RS-12, NFR-RA-05) — exercised end to end
//! through the public [`Engine`] façade against each language's SHIPPED
//! `references.scm` and `plugin.toml`, never a query override.
//!
//! Every fixture is the same four-case shape over methods `A.m` and `B.m`:
//!
//! - the language's self form inside `A.n` records `self` and binds to `A.m`;
//! - `other.m()` inside `A.m` records `other` and stays unbound — the binder's
//!   `no-receiver-evidence`, a fixed function of the shape pinned by the binder
//!   unit test `an_other_or_unshaped_call_never_binds_the_callers_own_method`;
//! - the super form records `super` and stays unbound: these plugins record no
//!   proven `Extends`, so not even the same-file `Base` that declares the name is
//!   reached — and never the caller's own class;
//! - no `Calls` edge is a self-loop.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn tree(rel: &str, text: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`.
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

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn call_edges(rt: &Runtime) -> Vec<(String, String)> {
    let label = labels(rt);
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// One `Calls` ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// Every `Calls` ledger row, sorted.
fn call_rows(rt: &Runtime) -> Vec<Row> {
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| {
                    let file = n.file_path.unwrap_or_default();
                    let label = format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0));
                    (n.symbol.as_str().to_string(), label)
                })
                .collect())
        })
        .expect("read runs");
    let rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            Some((label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    sorted(rows)
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        (&a.0, &a.1, a.2.as_i32(), a.3.map(ReceiverShape::as_i32))
            .cmp(&(&b.0, &b.1, b.2.as_i32(), b.3.map(ReceiverShape::as_i32)))
    });
    rows
}

fn row(source: &str, target: &str, form: RefForm, receiver: Option<ReceiverShape>, resolved: bool) -> Row {
    (source.to_string(), target.to_string(), form, receiver, resolved)
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

/// `edges` in the order [`call_edges`] returns them, so an expectation can be
/// written in source order.
fn sorted_edges(mut edges: Vec<(String, String)>) -> Vec<(String, String)> {
    edges.sort();
    edges
}

const SELF: Option<ReceiverShape> = Some(ReceiverShape::SelfInstance);
const SUPER: Option<ReceiverShape> = Some(ReceiverShape::Super);
const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);
const METHOD: RefForm = RefForm::Method;
const PATH: RefForm = RefForm::Path;

/// Index `text` as `file` and return its rows and edges, after asserting that
/// no `Calls` edge is a self-loop.
fn indexed(file: &str, text: &str) -> (Vec<Row>, Vec<(String, String)>) {
    let tmp = tree(file, text);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let (rows, edges) = (call_rows(rt), call_edges(rt));
    let loops: Vec<_> = edges.iter().filter(|(from, to)| from == to).collect();
    assert!(loops.is_empty(), "self-loop Calls edges: {loops:?}");
    (rows, edges)
}

// ── TypeScript, TSX, JavaScript: `this.` / `super.` ──────────────────────────

/// One class body, written once for the three TS-family fixtures: `this.m()`,
/// `super.m()` and `other.m()`. `Base` declares `m` in the same file — the
/// super call still binds nothing, since no `Extends` is proven.
const TS_FAMILY: &str = "\
class Base {
  m() {
    return 0;
  }
}

class A extends Base {
  m(other) {
    return other.m();
  }

  n() {
    return this.m(this);
  }

  k() {
    return super.m();
  }
}

class B {
  m() {
    return 1;
  }
}
";

/// The four cases for a TS-family file `file`, which parses `TS_FAMILY`.
fn assert_ts_family(file: &str) {
    let (rows, edges) = indexed(file, TS_FAMILY);
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            // `other.m()` inside `A.m`: `other`, unbound — no self-loop.
            row(&at(8, "m"), "m", METHOD, OTHER, false),
            // `this.m()` inside `A.n`: `self`, bound to `A.m`.
            row(&at(12, "n"), "m", METHOD, SELF, true),
            // `super.m()` inside `A.k`: `super`, unbound — neither `Base.m`
            // (no proven `Extends`) nor `A.m`.
            row(&at(16, "k"), "m", METHOD, SUPER, false),
        ])
    );
    assert_eq!(
        edges,
        sorted_edges(vec![edge(&at(12, "n"), &at(8, "m"))])
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_receivers_record_their_shape_and_only_this_binds() {
    assert_ts_family("src/a.ts");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn tsx_receivers_record_their_shape_and_only_this_binds() {
    assert_ts_family("src/a.tsx");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn javascript_receivers_record_their_shape_and_only_this_binds() {
    assert_ts_family("src/a.js");
}

/// A `this.m()` inside an anonymous class expression has no class node to
/// bind through: it is never the enclosing class's `m` — unless the anonymous
/// class declares `m` itself, when it is the free call that reaches exactly
/// that member. Each TS-family plugin carries its own marker, so each runs it.
fn assert_class_expression(file: &str) {
    let (rows, edges) = indexed(
        file,
        "\
class A {
  m() {
    return 0;
  }

  make() {
    return class {
      run() {
        return this.m();
      }

      own() {
        return 1;
      }

      go() {
        return this.own();
      }
    };
  }
}
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            row(&at(8, "run"), "m", METHOD, OTHER, false),
            row(&at(16, "go"), "own", PATH, None, true),
        ])
    );
    assert_eq!(
        edges,
        sorted_edges(vec![edge(&at(16, "go"), &at(12, "own"))])
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_this_call_inside_a_class_expression_never_binds_the_enclosing_class() {
    assert_class_expression("src/anon.ts");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_tsx_this_call_inside_a_class_expression_never_binds_the_enclosing_class() {
    assert_class_expression("src/anon.tsx");
}

/// JavaScript rebinds `this` in an object-literal method and in a non-arrow
/// `function`: there `this.m()` is never the enclosing class's `m`. An arrow
/// function keeps the method's `this`, so its `this.m()` still binds.
fn assert_rebound_this(file: &str) {
    let (rows, edges) = indexed(
        file,
        "\
class A {
  m() {
    return 0;
  }

  literal() {
    return {
      m() {
        return 1;
      },
      run() {
        return this.m();
      },
      cb: () => this.m(),
    };
  }

  nested() {
    function inner() {
      return this.m();
    }
    const f = function () {
      return this.m();
    };
    return [inner, f];
  }
}
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            // The arrow callback: the class instance, `A.m`.
            row(&at(6, "literal"), "m", METHOD, SELF, true),
            // The literal's own method: its `this` is the literal.
            row(&at(11, "run"), "m", METHOD, OTHER, false),
            row(&at(19, "inner"), "m", METHOD, OTHER, false),
            row(&at(22, "f"), "m", METHOD, OTHER, false),
        ])
    );
    assert_eq!(edges, sorted_edges(vec![edge(&at(6, "literal"), &at(2, "m"))]));
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_this_call_where_javascript_rebinds_this_never_binds_the_class() {
    assert_rebound_this("src/rebound.ts");
}

#[cfg(feature = "lang-typescript")]
#[test]
fn a_tsx_this_call_where_javascript_rebinds_this_never_binds_the_class() {
    assert_rebound_this("src/rebound.tsx");
}

// ── Python: `self.` / `cls.` / `super().` ────────────────────────────────────

#[cfg(feature = "lang-python")]
#[test]
fn python_receivers_record_their_shape_and_only_self_and_cls_bind() {
    let file = "pkg/a.py";
    let (rows, edges) = indexed(
        file,
        "\
class Base:
    def m(self, other):
        return 0


class A(Base):
    def m(self, other):
        return other.m(self)

    def n(self):
        return self.m(self)

    @classmethod
    def c(cls):
        return cls.m(None, None)

    def k(self):
        return super().m(self)

    def g(self):
        return get().m(self)

    def t(self):
        return super(A, self).m(self)


class B:
    def m(self, other):
        return 1
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            row(&at(7, "m"), "m", METHOD, OTHER, false),
            row(&at(10, "n"), "m", METHOD, SELF, true),
            row(&at(14, "c"), "m", METHOD, SELF, true),
            row(&at(17, "k"), "m", METHOD, SUPER, false),
            // The `super()` call itself is a free call, as before.
            row(&at(17, "k"), "super", PATH, None, false),
            // A call result is any other receiver, never `super`.
            row(&at(20, "g"), "m", METHOD, OTHER, false),
            row(&at(20, "g"), "get", PATH, None, false),
            // The two-argument form is `super` too.
            row(&at(23, "t"), "m", METHOD, SUPER, false),
            row(&at(23, "t"), "super", PATH, None, false),
        ])
    );
    assert_eq!(
        edges,
        sorted_edges(vec![edge(&at(10, "n"), &at(7, "m")), edge(&at(14, "c"), &at(7, "m"))])
    );
}

// ── PHP: `$this->` / `self::` / `static::` / `parent::` ──────────────────────

#[cfg(feature = "lang-php")]
#[test]
fn php_receivers_record_their_shape_and_only_this_self_and_static_bind() {
    let file = "src/A.php";
    let (rows, edges) = indexed(
        file,
        "\
<?php
class Base {
    public function m($other) { return 0; }
}

class A extends Base {
    public function m($other) {
        return $other->m($this) + B::m($this);
    }

    public function n() {
        return $this->m($this);
    }

    public static function s() {
        return self::m(null) + static::m(null);
    }

    public function k() {
        return parent::m($this);
    }
}

class B {
    public function m($other) { return 1; }
}
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            // `$other->m()` and the named-class `B::m()`: both `other`.
            row(&at(7, "m"), "m", METHOD, OTHER, false),
            row(&at(11, "n"), "m", METHOD, SELF, true),
            // `self::m()` and `static::m()`: one `self` row.
            row(&at(15, "s"), "m", METHOD, SELF, true),
            row(&at(19, "k"), "m", METHOD, SUPER, false),
        ])
    );
    assert_eq!(
        edges,
        sorted_edges(vec![edge(&at(11, "n"), &at(7, "m")), edge(&at(15, "s"), &at(7, "m"))])
    );
}

/// PHP's anonymous class (`new class { … }`) — the TS class-expression case.
#[cfg(feature = "lang-php")]
#[test]
fn a_this_call_inside_a_php_anonymous_class_never_binds_the_enclosing_class() {
    let file = "src/Anon.php";
    let (rows, edges) = indexed(
        file,
        "\
<?php
class A {
    public function m() { return 0; }

    public function make() {
        return new class {
            public function run() { return $this->m(); }

            public function own() { return 1; }

            public function go() { return $this->own(); }
        };
    }
}
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            row(&at(7, "run"), "m", METHOD, OTHER, false),
            row(&at(11, "go"), "own", PATH, None, true),
        ])
    );
    assert_eq!(edges, sorted_edges(vec![edge(&at(11, "go"), &at(9, "own"))]));
}

// ── Ruby: `self.`, the implicit receiver, `super` ────────────────────────────

/// Ruby's self forms are `self.m()` and the bare `m()` inside a class; its
/// super form is the keyword `super`, which calls the base's method of the
/// ENCLOSING method's name — recorded as that name, from that method. A bare
/// call outside any class is the free call it always was.
#[cfg(feature = "lang-ruby")]
#[test]
fn ruby_receivers_record_their_shape_and_only_self_and_bare_in_class_calls_bind() {
    let file = "lib/a.rb";
    let (rows, edges) = indexed(
        file,
        "\
class Base
  def m(other)
    0
  end

  def k
    0
  end

  def j(x)
    x
  end
end

class A < Base
  def m(other)
    other.m(self)
  end

  def n
    self.m(nil)
  end

  def i
    m(nil)
  end

  def k
    super
  end

  def j(x)
    y = super(x)
    y
  end
end

class B
  def m(other)
    1
  end
end

def m(other)
  2
end

def helper
  m(nil)
end
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            // The superclass constant: a free framework fingerprint, as before.
            row(&at(15, "A"), "Base", PATH, None, false),
            row(&at(16, "m"), "m", METHOD, OTHER, false),
            row(&at(20, "n"), "m", METHOD, SELF, true),
            // A bare call inside a class: `self` — `A.m`, never the top-level
            // `m` the scope walk would reach first.
            row(&at(24, "i"), "m", METHOD, SELF, true),
            // `super` inside `A.k` calls `Base#k`: recorded as `k`, unbound —
            // never `A.k` itself, the self-loop.
            row(&at(28, "k"), "k", METHOD, SUPER, false),
            // `super(x)` nested in an assignment.
            row(&at(32, "j"), "j", METHOD, SUPER, false),
            // A bare call outside any class is a free call, bound by scope.
            row(&at(48, "helper"), "m", PATH, None, true),
        ])
    );
    assert_eq!(
        edges,
        sorted_edges(vec![
            edge(&at(20, "n"), &at(16, "m")),
            edge(&at(24, "i"), &at(16, "m")),
            edge(&at(48, "helper"), &at(44, "m")),
        ])
    );
}

/// A Ruby `module` is a mixin: its methods' `self` is whatever includes it, so
/// a module — even one nested in a class, rack's `Request::Helpers` shape — is
/// never its enclosing class. A bare call inside it is the free call it always
/// was, and `self.x()` binds only to an `x` the module declares itself.
#[cfg(feature = "lang-ruby")]
#[test]
fn a_ruby_module_is_never_its_enclosing_class() {
    let file = "lib/request.rb";
    let (rows, edges) = indexed(
        file,
        "\
class Request
  def m
    0
  end

  module Helpers
    def host
      split(1)
    end

    def split(x)
      x
    end

    def port
      self.split(2)
    end
  end
end

module Utils
  def self.escape(s)
    self.unescape(s)
  end

  def self.unescape(s)
    s
  end
end
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            row(&at(7, "host"), "split", PATH, None, true),
            row(&at(15, "port"), "split", PATH, None, true),
            row(&at(22, "escape"), "unescape", PATH, None, true),
        ])
    );
    assert_eq!(
        edges,
        sorted_edges(vec![
            edge(&at(7, "host"), &at(11, "split")),
            edge(&at(15, "port"), &at(11, "split")),
            edge(&at(22, "escape"), &at(26, "unescape")),
        ])
    );
}

/// Where a Ruby class's `self` is not one of its instances — inside `def
/// self.x`, `class << self`, or a `Struct.new` / `Class.new` block — a
/// `self.m()` is never the class's instance method `m`. The graph cannot tell a
/// singleton method from an instance one, so such a call binds nothing.
#[cfg(feature = "lang-ruby")]
#[test]
fn a_ruby_self_call_on_the_class_object_never_binds_an_instance_method() {
    let file = "lib/widget.rb";
    let (rows, edges) = indexed(
        file,
        "\
class Widget
  def name
    1
  end

  def self.key
    self.name
  end

  def self.label
    self.key
  end

  class << self
    def table
      self.name
    end
  end

  Point = Struct.new(:a) do
    def len
      self.name
    end
  end
end
",
    );
    let at = |line: u32, name: &str| format!("{file}:{name}@{line}");
    assert_eq!(
        rows,
        sorted(vec![
            row(&at(1, "Widget"), "Struct", PATH, None, false),
            row(&at(1, "Widget"), "new", METHOD, OTHER, false),
            row(&at(6, "key"), "name", METHOD, OTHER, false),
            // A singleton calling a singleton: unbound too, not wrong.
            row(&at(10, "label"), "key", METHOD, OTHER, false),
            row(&at(15, "table"), "name", METHOD, OTHER, false),
            row(&at(21, "len"), "name", METHOD, OTHER, false),
        ])
    );
    assert_eq!(edges, Vec::<(String, String)>::new());
}
