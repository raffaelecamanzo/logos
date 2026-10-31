//! Ruby constants resolve, and calls on a constant bind (S-565, CR-179,
//! FR-RS-33, NFR-RA-05, NFR-RA-06) — exercised end to end through the public
//! [`Engine`] façade against the SHIPPED Ruby `references.scm`, `symbols.scm`
//! and `plugin.toml`, never a query override.
//!
//! The fixtures reproduce the shapes the 2026-10-03 inspection's spot checks
//! name: lobsters' `Keystore.value_for`, `ModNote.tattle_on_story_domain!`,
//! `CommentRedirectCache.write` (a `class << self` method) and
//! `TimeSeries.new`; rack's `Request.new` and `Rack::Request.new` from inside
//! `module Rack`, and `Utils.escape_html`, a `module_function` defined in an
//! `if`/`else` branch.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(feature = "lang-ruby")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

type Fixture = &'static [(&'static str, &'static str)];

/// lobsters' shapes: three singleton forms and a constructor.
const LOBSTERS: Fixture = &[
    (
        "app/models/keystore.rb",
        "\
class Keystore < ApplicationRecord
  def self.value_for(key)
    key
  end

  def value_for(key)
    key
  end
end
",
    ),
    (
        "app/models/mod_note.rb",
        "\
class ModNote < ApplicationRecord
  def self.tattle_on_story_domain!(story, reason)
    story
  end
end
",
    ),
    (
        "app/models/comment_redirect_cache.rb",
        "\
class CommentRedirectCache
  class << self
    def write(short_id, target_path)
      short_id
    end
  end

  def read(short_id)
    short_id
  end
end
",
    ),
    (
        "lib/time_series.rb",
        "\
class TimeSeries
  def initialize(opts)
    opts
  end
end
",
    ),
    (
        "app/models/user.rb",
        "\
class User < ApplicationRecord
  def comments_posted
    Keystore.value_for(\"posted\")
  end

  def ban
    ModNote.tattle_on_story_domain!(self, \"banned\")
  end

  def remember(id, url)
    CommentRedirectCache.write(id, url)
  end

  def forget(id)
    CommentRedirectCache.read(id)
  end

  def graph
    TimeSeries.new({})
  end

  def stamp
    Time.now
  end
end
",
    ),
];

/// rack's shapes: every file reopens `module Rack`, `Utils` is a
/// `module_function` module, and the callers sit inside the nesting.
const RACK: Fixture = &[
    (
        "lib/rack/request.rb",
        "\
module Rack
  class Request
    def initialize(env)
      env
    end
  end
end
",
    ),
    (
        "lib/rack/utils.rb",
        "\
module Rack
  module Utils
    def self.default_query_parser
      nil
    end

    def before_module_function(s)
      s
    end

    module_function

    def escape(s)
      s
    end

    if defined?(ERB::Escape)
      define_method(:escape_html, ERB::Escape.instance_method(:html_escape))
    else
      def escape_html(string)
        string
      end
    end
  end
end
",
    ),
    (
        "lib/rack/common_logger.rb",
        "\
module Rack
  class CommonLogger
    def log(env)
      request = Rack::Request.new(env)
      Request.new(env)
      Utils.escape_html(request)
      Utils.escape(request)
      Utils.before_module_function(request)
      ::Rack::Utils.escape(request)
    end
  end
end
",
    ),
];

/// Constant lookup: the lexical nesting outward, then the top level; a reopened
/// class is one constant; a class and a module of one name are two declarations.
const LOOKUP: Fixture = &[
    (
        "lib/helper.rb",
        "\
class Helper
  def self.run
    0
  end
end
",
    ),
    (
        "lib/outer/helper.rb",
        "\
module Outer
  class Helper
    def self.run
      1
    end
  end
end
",
    ),
    (
        "lib/outer/caller.rb",
        "\
module Outer
  class Caller
    def go
      Helper.run
    end
  end
end

class TopCaller
  def go
    Helper.run
  end
end
",
    ),
    (
        "lib/widget.rb",
        "\
class Widget
  def self.build
    0
  end
end
",
    ),
    (
        "lib/widget_ext.rb",
        "\
class Widget
  def self.render
    1
  end
end
",
    ),
    (
        "lib/clash_class.rb",
        "\
class Clash
  def self.go
    0
  end
end
",
    ),
    (
        "lib/clash_module.rb",
        "\
module Clash
  def self.go
    1
  end
end
",
    ),
    (
        "lib/mod/flagged.rb",
        "\
class Mod::Flagged
end
",
    ),
    (
        "lib/use.rb",
        "\
class Use
  def go
    Widget.build
    Widget.render
    Widget.new
    Clash.go
    Clash.new
    Mod::Flagged.new
  end
end

module Mod
  class Sibling
    def go
      Flagged.new
    end
  end
end
",
    ),
];

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index(root: &Path) -> Engine {
    coordinate::pin(root);
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
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

/// Every bound edge of `kind` as `source label -> target label`, sorted.
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

/// Every ledger row of `kind` as `target form resolved`, sorted.
fn rows(rt: &Runtime, kind: EdgeKind) -> Vec<String> {
    let mut out: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == kind)
        .map(|r| format!("{} {:?} {}", r.target, r.form, r.resolved))
        .collect();
    out.sort();
    out
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// L1–L4: a call on a constant records the Path `Foo::bar` and binds among the
/// constant's singleton methods — `def self.x` and a `class << self` method —
/// never its same-named instance method; `.new` records `Instantiates`. An
/// instance method called on the constant, and an external constant, stay
/// unbound.
#[test]
fn a_call_on_a_constant_binds_among_its_singleton_methods_and_new_instantiates() {
    let (_tmp, engine) = indexed(LOBSTERS);
    let rt = engine.runtime().unwrap();
    let calls: Vec<String> = edges(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|e| e.starts_with("app/models/user.rb"))
        .collect();
    assert_eq!(
        calls,
        strings(&[
            "app/models/user.rb:ban@6 -> app/models/mod_note.rb:tattle_on_story_domain!@2",
            "app/models/user.rb:comments_posted@2 -> app/models/keystore.rb:value_for@2",
            "app/models/user.rb:remember@10 -> app/models/comment_redirect_cache.rb:write@3",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        strings(&["app/models/user.rb:graph@18 -> lib/time_series.rb:TimeSeries@1"])
    );
    let user_calls: Vec<String> = rows(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|r| r.contains("::"))
        .collect();
    assert_eq!(
        user_calls,
        strings(&[
            "CommentRedirectCache::read Path false",
            "CommentRedirectCache::write Path true",
            "Keystore::value_for Path true",
            "ModNote::tattle_on_story_domain! Path true",
            "Time::now Path false",
        ]),
        "one Path row per call on a constant — no bare method row beside it"
    );
}

/// R6–R8: inside `module Rack`, `Request` is `Rack::Request` by the lexical
/// nesting, `Rack::Request` is the same constant written in full, and
/// `Utils.escape_html` binds the `module_function` defined in an `else`
/// branch. A method defined before `module_function` is no module function.
/// `::Rack::Utils` is read from the top level.
#[test]
fn rack_constants_resolve_through_the_nesting_and_module_functions_bind() {
    let (_tmp, engine) = indexed(RACK);
    let rt = engine.runtime().unwrap();
    // One edge per (caller, target): both instantiations, and both `escape`
    // calls, are one edge each — the ledger rows below show every one bound.
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        strings(&["lib/rack/common_logger.rb:log@3 -> lib/rack/request.rb:Request@2"])
    );
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        strings(&[
            "lib/rack/common_logger.rb:log@3 -> lib/rack/utils.rb:escape@13",
            "lib/rack/common_logger.rb:log@3 -> lib/rack/utils.rb:escape_html@20",
        ])
    );
    assert_eq!(
        rows(rt, EdgeKind::Instantiates),
        strings(&["Rack::Request Path true", "Request Path true"])
    );
    let calls: Vec<String> = rows(rt, EdgeKind::Calls).into_iter().filter(|r| r.contains("Utils")).collect();
    assert_eq!(
        calls,
        strings(&[
            "Utils::before_module_function Path false",
            "Utils::escape Path true",
            "Utils::escape_html Path true",
            "\\::Rack::Utils::escape Path true",
        ])
    );
}

/// Lookup order and identity: the innermost nesting that holds the name
/// decides; a reopened class pools its singleton methods; a class and a module
/// of one name stay unbound, and so does `.new` on a class declared twice. A
/// compact `class Mod::Flagged` is the constant `Mod::Flagged`, from the top
/// level and from inside `module Mod`.
#[test]
fn a_constant_resolves_by_nesting_then_top_level_and_two_declarations_stay_unbound() {
    let (_tmp, engine) = indexed(LOOKUP);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        strings(&[
            "lib/outer/caller.rb:go@10 -> lib/helper.rb:run@2",
            "lib/outer/caller.rb:go@3 -> lib/outer/helper.rb:run@3",
            "lib/use.rb:go@2 -> lib/widget.rb:build@2",
            "lib/use.rb:go@2 -> lib/widget_ext.rb:render@2",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        strings(&[
            "lib/use.rb:go@14 -> lib/mod/flagged.rb:Flagged@1",
            "lib/use.rb:go@2 -> lib/mod/flagged.rb:Flagged@1",
        ])
    );
    let unbound: Vec<String> = rows(rt, EdgeKind::Calls)
        .into_iter()
        .chain(rows(rt, EdgeKind::Instantiates))
        .filter(|r| r.ends_with("false") && (r.starts_with("Clash") || r.starts_with("Widget")))
        .collect();
    assert_eq!(
        unbound,
        strings(&["Clash::go Path false", "Clash Path false", "Widget Path false"])
    );
}

/// Every binding fact of a store, id-free: bound edges by symbol, and every
/// ledger row with its resolution (NFR-RA-06).
fn binding_facts(rt: &Runtime) -> (Vec<(String, String, String)>, Vec<String>) {
    rt.submit_read(|store| {
        let sym: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.symbol.as_str().to_string()))
            .collect();
        let mut edges: Vec<(String, String, String)> = store
            .all_edges()?
            .into_iter()
            .map(|e| (sym[&e.source].clone(), sym[&e.target].clone(), e.kind.as_str().to_string()))
            .collect();
        edges.sort();
        let mut refs: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .map(|r| format!("{} {} {:?} {:?} {}", r.source_symbol, r.target, r.form, r.kind, r.resolved))
            .collect();
        refs.sort();
        Ok((edges, refs))
    })
    .expect("read runs")
}

/// Index a fresh copy of `tmp`'s `files` and return its binding facts.
fn cold_facts(tmp: &TempDir, files: &[&str]) -> (Vec<(String, String, String)>, Vec<String>) {
    let cold = TempDir::new().unwrap();
    for rel in files {
        if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
            write(cold.path(), rel, &text);
        }
    }
    let engine = index(cold.path());
    binding_facts(engine.runtime().unwrap())
}

/// The caller of the sync test: each call on a constant in a method of its
/// own, so a row re-binds only when the sync selects it by its own target —
/// never swept in beside another row of its method (`resolve::swept_sources`).
const CALLER: (&str, &str) = (
    "lib/rack/common_logger.rb",
    "\
module Rack
  class CommonLogger
    def log(env)
      Rack::Request.new(env)
    end

    def escape(s)
      Utils.escape_html(s)
    end
  end
end
",
);

/// A singleton method that becomes an instance method, and a nearer constant
/// that appears in a file the caller never touches, each move a binding no
/// edit to the caller's file announces — a sync must land where a cold index
/// does (NFR-RA-06).
#[test]
fn sync_equals_a_full_reindex_after_a_constant_or_a_singleton_method_moves() {
    let fixture: Vec<(&str, &str)> = RACK.iter().map(|&f| if f.0 == CALLER.0 { CALLER } else { f }).collect();
    let (tmp, engine) = indexed(&fixture);
    let rt = engine.runtime().unwrap();
    let mut files: Vec<&str> = fixture.iter().map(|(rel, _)| *rel).collect();
    assert!(
        edges(rt, EdgeKind::Calls)
            .contains(&"lib/rack/common_logger.rb:escape@7 -> lib/rack/utils.rb:escape_html@20".to_string()),
        "the caller binds `Rack::Utils` before the sync"
    );

    let shadow = "lib/rack/common_logger/utils.rb";
    write(
        tmp.path(),
        shadow,
        "\
module Rack
  class CommonLogger
    module Utils
      def self.escape_html(s)
        s
      end
    end
  end
end
",
    );
    engine.sync(&[shadow.into()]);
    files.push(shadow);
    assert!(
        edges(rt, EdgeKind::Calls).contains(
            &"lib/rack/common_logger.rb:escape@7 -> lib/rack/common_logger/utils.rb:escape_html@4".to_string()
        ),
        "the nearer `Rack::CommonLogger::Utils` now decides"
    );
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));

    let utils = "lib/rack/utils.rb";
    write(
        tmp.path(),
        utils,
        "\
module Rack
  module Utils
    def escape(s)
      s
    end
  end
end
",
    );
    engine.sync(&[utils.into()]);
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));

    let request = "lib/rack/request.rb";
    fs::remove_file(tmp.path().join(request)).unwrap();
    engine.sync(&[request.into()]);
    assert!(edges(rt, EdgeKind::Instantiates).is_empty(), "the constant is gone");
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));
}

/// The rows a call on a constant records, Path form, are the ones an index
/// reads back: not a `Method` row of the bare name beside them.
#[test]
fn a_call_on_a_constant_records_no_bare_method_row() {
    let (_tmp, engine) = indexed(LOBSTERS);
    let rt = engine.runtime().unwrap();
    let bare: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form == RefForm::Method)
        .filter(|r| ["value_for", "write", "read", "new", "now", "tattle_on_story_domain!"].contains(&r.target.as_str()))
        .map(|r| r.target)
        .collect();
    assert!(bare.is_empty(), "{bare:?}");
}

/// The constant-path matcher's near misses: a lowercase receiver is an object
/// (`store.value_for`), a chain's outer call has a call for its receiver
/// (`Keystore.value_for(1).to_i` — `Keystore::value_for` and the bare `to_i`),
/// and a value constant (`STORE.value_for`) is a constant path that names no
/// class or module here, so it stays unbound — never a guess.
#[test]
fn only_a_constant_path_receiver_is_a_call_on_a_constant() {
    let (_tmp, engine) = indexed(&[
        LOBSTERS[0],
        (
            "app/models/user.rb",
            "\
class User
  STORE = nil

  def a(store)
    store.value_for(1)
  end

  def b
    Keystore.value_for(1).to_i
  end

  def c
    STORE.value_for(1)
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    let mut calls: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.source_symbol.contains("user.rb"))
        .map(|r| format!("{} {:?} {}", r.target, r.form, r.resolved))
        .collect();
    calls.sort();
    assert_eq!(
        calls,
        strings(&[
            "Keystore::value_for Path true",
            "STORE::value_for Path false",
            "to_i Method false",
            "value_for Method false",
        ])
    );
}

/// A bare `module_function` makes the module's own later methods module
/// functions — one written behind a modifier `unless` too — and nothing else:
/// not the instance methods of a class nested in the module, and not a method
/// defined after a later bare `private`, which ends it. A call on the module
/// binds only the first two.
#[test]
fn module_function_marks_only_the_modules_own_methods_until_a_visibility_call() {
    let (_tmp, engine) = indexed(&[
        (
            "lib/util.rb",
            "\
module Util
  module_function

  def pub(x)
    x
  end

  def gated(x)
    x
  end unless false

  class Inner
    def inst(x)
      x
    end
  end

  private

  def hidden(x)
    x
  end
end
",
        ),
        (
            "lib/caller.rb",
            "\
class Caller
  def go
    Util.pub(1)
    Util.gated(1)
    Util.hidden(1)
    Util::Inner.inst(2)
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        strings(&["lib/caller.rb:go@2 -> lib/util.rb:gated@8", "lib/caller.rb:go@2 -> lib/util.rb:pub@4"])
    );
}

/// A compact `class A::B` spans its whole body (S-564), so its members are its
/// own. None of them may be credited to the scope around it (`Outer.leaf`) or
/// become a constant of that scope (a top-level `Inner`). A constant the
/// compact declaration holds binds from inside its body (`Inner.y` in
/// `Mod::Flagged#x`). What follows its `end` belongs to the enclosing scope
/// again: `Outer.after` binds. A placed constant beside them still binds.
#[test]
fn a_compact_declaration_owns_its_body_and_what_follows_is_its_scopes() {
    let (_tmp, engine) = indexed(&[
        ("lib/top.rb", "module Top\nend\n"),
        (
            "lib/outer.rb",
            "\
module Outer
  class Top::Leaf
    def self.leaf(x)
      x
    end
  end

  def self.after
    0
  end
end
",
        ),
        (
            "lib/mod/flagged.rb",
            "\
class Mod::Flagged
  module Inner
    def self.y
      0
    end
  end

  def self.x
    Inner.y
  end
end
",
        ),
        (
            "lib/caller.rb",
            "\
class Placed
  def self.p
    0
  end
end

class Caller
  def run
    Outer.leaf(1)
    Inner.y
    Placed.p
    Outer.after
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        strings(&[
            "lib/caller.rb:run@8 -> lib/caller.rb:p@2",
            "lib/caller.rb:run@8 -> lib/outer.rb:after@8",
            "lib/mod/flagged.rb:x@8 -> lib/mod/flagged.rb:y@3",
        ])
    );
}

/// The lookup rules a single fixture can each break alone: a name declared a
/// class in one file and a module in another binds nothing even when only the
/// class defines the method; `::Helper` is read from the top level, past a
/// nearer `Outer::Helper`; `.new` on a module instantiates nothing; and a
/// compact declaration's scope is read from the scope around it — the top-level
/// `Mod`, never the `Mod::Flagged::Mod` that sits inside the constant it
/// declares.
#[test]
fn lookup_rules_each_decide_their_own_case() {
    let (_tmp, engine) = indexed(&[
        LOOKUP[0],
        LOOKUP[1],
        ("lib/solo_class.rb", "class Solo\n  def self.go\n    0\n  end\nend\n"),
        ("lib/solo_module.rb", "module Solo\nend\n"),
        ("lib/mod2.rb", "module Mod2\nend\n"),
        ("lib/a.rb", "module Mod\n  class Flagged\n    module Mod\n    end\n  end\nend\n"),
        ("lib/b.rb", "class Mod::Flagged\nend\n"),
        (
            "lib/outer/root_caller.rb",
            "\
module Outer
  class RootCaller
    def go
      ::Helper.run
      Solo.go
      Mod2.new
    end
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        strings(&["lib/outer/root_caller.rb:go@3 -> lib/helper.rb:run@2"])
    );
    assert!(edges(rt, EdgeKind::Instantiates).is_empty(), "a module is never instantiated");
    assert_eq!(edges(rt, EdgeKind::TypeUses), strings(&["lib/b.rb:Flagged@1 -> lib/a.rb:Mod@1"]));
}
