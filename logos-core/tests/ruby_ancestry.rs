//! Ruby superclasses and mixins are relations, and implicit calls resolve along
//! the ancestor chain (S-568, CR-179, FR-RS-35, NFR-RA-05, NFR-RA-06) —
//! exercised end to end through the public [`Engine`] façade against the
//! SHIPPED Ruby `references.scm`, `symbols.scm` and `plugin.toml`, never a
//! query override.
//!
//! The fixtures reproduce the shapes the 2026-10-03 inspection's inheritance
//! and mixin spot checks name: lobsters' `Story < ApplicationRecord`,
//! `include Token` and `refill_story_page_cache` (inherited from
//! `ApplicationController`, called from an `after_action` block); rack's
//! `Basic < AbstractHandler` (whose bare `unauthorized` is inherited),
//! `include Rack::Utils`, and `to_app.call(env)` in `Builder`.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(feature = "lang-ruby")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

type Fixture = &'static [(&'static str, &'static str)];

/// lobsters' shapes: an external base, a mixin with an external `extend`,
/// controllers calling an inherited helper from an `after_action` block, and a
/// compact `Mod::` controller two levels down.
const LOBSTERS: Fixture = &[
    (
        "app/models/application_record.rb",
        "\
class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
",
    ),
    (
        "app/models/story.rb",
        "\
class Story < ApplicationRecord
  def title
    \"t\"
  end
end
",
    ),
    (
        "app/models/concerns/token.rb",
        "\
module Token
  extend ActiveSupport::Concern

  def token=(new)
    new
  end
end
",
    ),
    (
        "app/models/category.rb",
        "\
class Category < ApplicationRecord
  include Token
end
",
    ),
    (
        "app/controllers/application_controller.rb",
        "\
class ApplicationController < ActionController::Base
  def refill_story_page_cache(story, is_moderator: false)
    story
  end
end
",
    ),
    (
        "app/controllers/comments_controller.rb",
        "\
class CommentsController < ApplicationController
  after_action only: [:create] do
    refill_story_page_cache @comment.story
  end

  def create
    refill_story_page_cache(@comment.story)
  end
end
",
    ),
    (
        "app/controllers/mod/mod_controller.rb",
        "\
class Mod::ModController < ApplicationController
end
",
    ),
    (
        "app/controllers/mod/stories_controller.rb",
        "\
class Mod::StoriesController < Mod::ModController
  after_action only: [:destroy] do
    refill_story_page_cache @story, is_moderator: true
  end
end
",
    ),
];

/// rack's shapes: every file reopens `module Rack`; `Basic`'s bare
/// `unauthorized` / `bad_request` are `AbstractHandler`'s; `ContentLength`
/// includes `Rack::Utils` and reads locals of the same names as methods
/// nowhere; `Builder#call` calls its own `to_app` bare.
const RACK: Fixture = &[
    (
        "lib/rack/auth/abstract/handler.rb",
        "\
module Rack
  module Auth
    class AbstractHandler
      def initialize(app, realm = nil)
        @app = app
      end

      private

      def unauthorized(www_authenticate = challenge)
        [401, {}, []]
      end

      def bad_request
        [400, {}, []]
      end
    end
  end
end
",
    ),
    (
        "lib/rack/auth/basic.rb",
        "\
module Rack
  module Auth
    class Basic < AbstractHandler
      def call(env)
        auth = env
        return unauthorized unless auth
        return bad_request unless auth.basic?
        unauthorized
      end

      def challenge
        'Basic'
      end
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
    def status_code(status)
      status
    end
  end
end
",
    ),
    (
        "lib/rack/content_length.rb",
        "\
module Rack
  class ContentLength
    include Rack::Utils

    def call(env)
      status, headers, body = response = @app.call(env)
      status_code(status)
      response
    end
  end
end
",
    ),
    (
        "lib/rack/builder.rb",
        "\
module Rack
  class Builder
    def to_app
      self
    end

    def call(env)
      to_app.call(env)
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

/// The `Calls` edges whose source label starts with `file`.
fn calls_from(rt: &Runtime, file: &str) -> Vec<String> {
    edges(rt, EdgeKind::Calls).into_iter().filter(|e| e.starts_with(file)).collect()
}

/// The `Calls` rows sourced in `file`, as `source-name target form resolved`.
fn call_rows_in(rt: &Runtime, file: &str) -> Vec<String> {
    let label = labels(rt);
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone()))
                .collect())
        })
        .expect("read runs");
    let mut out: Vec<String> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .filter_map(|r| {
            let source = by_symbol.get(&r.source_symbol)?;
            source
                .starts_with(file)
                .then(|| format!("{source} {} {:?} {}", r.target, r.form, r.resolved))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// The inheritance and mixin spot checks: `class A < B` records `Extends` → the
/// class `B` names, read from the scope enclosing `A`; `include M` records
/// `Implements` → the module, read inside the body. An external base and an
/// external `extend` stay unbound rows, and the superclass row is the Rails
/// fingerprint it always was.
#[test]
fn superclasses_record_extends_and_mixins_record_implements_through_the_constant_index() {
    let fixture: Vec<(&str, &str)> = LOBSTERS.iter().chain(RACK).copied().collect();
    let (_tmp, engine) = indexed(&fixture);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        strings(&[
            "app/controllers/comments_controller.rb:CommentsController@1 -> app/controllers/application_controller.rb:ApplicationController@1",
            "app/controllers/mod/mod_controller.rb:ModController@1 -> app/controllers/application_controller.rb:ApplicationController@1",
            "app/models/category.rb:Category@1 -> app/models/application_record.rb:ApplicationRecord@1",
            "app/models/story.rb:Story@1 -> app/models/application_record.rb:ApplicationRecord@1",
            "lib/rack/auth/basic.rb:Basic@3 -> lib/rack/auth/abstract/handler.rb:AbstractHandler@3",
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Implements),
        strings(&[
            "app/models/category.rb:Category@1 -> app/models/concerns/token.rb:Token@1",
            "lib/rack/content_length.rb:ContentLength@2 -> lib/rack/utils.rb:Utils@2",
        ])
    );
    assert_eq!(
        rows(rt, EdgeKind::Extends),
        strings(&[
            "AbstractHandler Path true",
            "ActionController::Base Path false",
            "ActiveRecord::Base Path false",
            "ApplicationController Path true",
            "ApplicationController Path true",
            "ApplicationRecord Path true",
            "ApplicationRecord Path true",
            // No file declares `Mod`, which Zeitwerk makes an implicit module
            // of `app/controllers/mod/`: the constant lookup reads no
            // undeclared namespace, so the path names no constant.
            "Mod::ModController Path false",
        ])
    );
    assert_eq!(
        rows(rt, EdgeKind::Implements),
        strings(&["ActiveSupport::Concern Path false", "Rack::Utils Path true", "Token Path true"])
    );
}

/// A receiver-less call binds along the caller's ancestors: lobsters'
/// `refill_story_page_cache` from an `after_action` block one and two
/// superclasses down, rack's `unauthorized` / `bad_request` written bare, an
/// included module's method, and `to_app` — a zero-argument bare name in
/// receiver position — in the caller's own class.
#[test]
fn implicit_calls_resolve_along_the_ancestor_chain() {
    let fixture: Vec<(&str, &str)> = LOBSTERS.iter().chain(RACK).copied().collect();
    let (_tmp, engine) = indexed(&fixture);
    let rt = engine.runtime().unwrap();
    let refill = "app/controllers/application_controller.rb:refill_story_page_cache@2";
    assert_eq!(
        calls_from(rt, "app/controllers/"),
        strings(&[
            &format!("app/controllers/comments_controller.rb:CommentsController@1 -> {refill}"),
            &format!("app/controllers/comments_controller.rb:create@6 -> {refill}"),
        ]),
        "`Mod::StoriesController`'s superclass names an undeclared namespace, so its walk ends at itself"
    );
    assert_eq!(
        calls_from(rt, "lib/rack/auth/basic.rb"),
        strings(&[
            "lib/rack/auth/basic.rb:call@4 -> lib/rack/auth/abstract/handler.rb:bad_request@14",
            "lib/rack/auth/basic.rb:call@4 -> lib/rack/auth/abstract/handler.rb:unauthorized@10",
        ])
    );
    assert_eq!(
        calls_from(rt, "lib/rack/content_length.rb"),
        strings(&["lib/rack/content_length.rb:call@5 -> lib/rack/utils.rb:status_code@3"])
    );
    assert_eq!(
        calls_from(rt, "lib/rack/builder.rb"),
        strings(&["lib/rack/builder.rb:call@7 -> lib/rack/builder.rb:to_app@3"])
    );
}

/// The ancestor order: prepended modules, the class, included modules, then
/// the superclass — exactly one at the first level holding the name. Two
/// mixins defining the name decide nothing; a mixin the graph cannot see
/// stops the walk where it sits; a singleton method answers no instance; a
/// `super` starts past the caller's own class; a module mixed in through a
/// mixin is read with it.
#[test]
fn the_first_ancestor_holding_the_name_decides_exactly_one() {
    let (_tmp, engine) = indexed(&[
        (
            "lib/mixins.rb",
            "\
module Loud
  def speak
  end
end

module Polite
  def speak
  end

  def greet
  end

  def wave
  end
end

module Kind
  def greet
  end
end

module Deep
  def dig
  end
end

module Shallow
  include Deep
end
",
        ),
        (
            "lib/animal.rb",
            "\
class Animal
  def speak
  end

  def wave
  end

  def sleep
  end

  def self.create
  end
end
",
        ),
        (
            "lib/dog.rb",
            "\
class Dog < Animal
  prepend Loud
  include Polite
  include Kind
  include Shallow

  def speak
  end

  def a
    speak
  end

  def b
    wave
  end

  def c
    sleep
  end

  def d
    greet
  end

  def e
    create
  end

  def f
    dig
  end
end
",
        ),
        (
            "lib/cat.rb",
            "\
class Cat < Animal
  include Polite

  def g
    speak
  end

  def sleep
    super
  end

  def wave
    super
  end
end
",
        ),
        (
            "lib/bird.rb",
            "\
class Bird < Animal
  include Comparable

  def own
  end

  def h
    own
  end

  def i
    sleep
  end
end
",
        ),
        (
            "lib/fish.rb",
            "\
class Fish < Animal
  prepend Instrumented::Calls

  def own
  end

  def j
    own
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "lib/dog.rb"),
        strings(&[
            // The prepended module comes before the class's own `speak`.
            "lib/dog.rb:a@10 -> lib/mixins.rb:speak@2",
            // An included module comes before the superclass.
            "lib/dog.rb:b@14 -> lib/mixins.rb:wave@13",
            // Nothing nearer: the superclass.
            "lib/dog.rb:c@18 -> lib/animal.rb:sleep@8",
            // `Shallow` includes `Deep`: read with the class's mixins.
            "lib/dog.rb:f@30 -> lib/mixins.rb:dig@23",
        ]),
        "`greet` (two mixins) and `create` (a singleton method) bind nothing"
    );
    assert_eq!(
        calls_from(rt, "lib/cat.rb"),
        strings(&[
            "lib/cat.rb:g@4 -> lib/mixins.rb:speak@7",
            // `super` from `Cat#sleep`: past `Cat`, `Polite` has none, `Animal` does.
            "lib/cat.rb:sleep@8 -> lib/animal.rb:sleep@8",
            // `super` from `Cat#wave`: `Polite`'s, before `Animal`'s.
            "lib/cat.rb:wave@12 -> lib/mixins.rb:wave@13",
        ])
    );
    assert_eq!(
        calls_from(rt, "lib/bird.rb"),
        strings(&["lib/bird.rb:h@7 -> lib/bird.rb:own@4"]),
        "an unseen include stops the walk before the superclass, after the class"
    );
    assert_eq!(calls_from(rt, "lib/fish.rb"), Vec::<String>::new(), "an unseen prepend stops it before the class");
}

/// A bare name in a method is a call only when no local of the method binds
/// it: a parameter of every kind, a block parameter, an assignment's target, a
/// multiple assignment, a `rescue => e`, a `for` variable. A name in a class
/// body (`private`) records nothing; a bare name outside every class is the
/// free call it always was.
#[test]
fn a_bare_name_is_a_call_unless_a_local_of_its_method_binds_it() {
    let file = "lib/locals.rb";
    let (_tmp, engine) = indexed(&[(
        file,
        "\
class Locals
  private

  def helper
  end

  def m(a, b = helper, *c, d:, e: 1, **f, &g)
    h = 1
    i, j = 1, 2
    k += 1
    [1].each { |l| l }
    [1].each do |n|
      n
      helper
    end
    for o in [1]
      o
    end
    begin
      p
    rescue => q
      q
    end
    a
    b
    c
    d
    e
    f
    g
    h
    i
    j
    k
    helper
  end
end

def top
end

def caller_outside
  top
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        call_rows_in(rt, file),
        strings(&[
            // `caller_outside`'s `top`: the free call, bound by scope.
            "lib/locals.rb:caller_outside@42 top Path true",
            // `helper` (a default value, in a block, a statement); `p`, no
            // local of `m`'s, a call on the instance that binds nothing; the
            // two `each` calls on an array.
            "lib/locals.rb:m@7 each Method false",
            "lib/locals.rb:m@7 helper Method true",
            "lib/locals.rb:m@7 p Method false",
        ])
    );
    assert_eq!(
        calls_from(rt, file),
        strings(&[
            "lib/locals.rb:caller_outside@42 -> lib/locals.rb:top@39",
            "lib/locals.rb:m@7 -> lib/locals.rb:helper@4",
        ])
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

/// The ancestry moves in files the caller never touches — a reopening of the
/// caller's class mixes in a module that already defines the name, which no
/// dirty name of the sync spells but the class's own; the module loses the
/// method; the superclass loses it — and each sync lands where a cold index
/// does (NFR-RA-06).
#[test]
fn sync_equals_a_full_reindex_after_the_ancestry_moves() {
    let base = (
        "lib/base.rb",
        "\
class Base
  def run
  end
end
",
    );
    let caller = (
        "lib/worker.rb",
        "\
class Worker < Base
  def go
    run
  end
end
",
    );
    let runner = (
        "lib/runner.rb",
        "\
module Runner
  def run
  end
end
",
    );
    let (tmp, engine) = indexed(&[base, caller, runner]);
    let rt = engine.runtime().unwrap();
    let runner = runner.0;
    let mut files = vec![base.0, caller.0, runner];
    assert_eq!(calls_from(rt, caller.0), strings(&["lib/worker.rb:go@2 -> lib/base.rb:run@2"]));

    // A reopening of `Worker` in another file mixes in `Runner`, which
    // defines `run`: the mixin comes before the superclass.
    let reopening = "lib/worker_ext.rb";
    write(
        tmp.path(),
        reopening,
        "\
class Worker
  include Runner
end
",
    );
    engine.sync(&[reopening.into()]);
    files.push(reopening);
    assert_eq!(calls_from(rt, caller.0), strings(&["lib/worker.rb:go@2 -> lib/runner.rb:run@2"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));

    // The module loses its `run`: the superclass's binds again.
    write(
        tmp.path(),
        runner,
        "\
module Runner
  def walk
  end
end
",
    );
    engine.sync(&[runner.into()]);
    assert_eq!(calls_from(rt, caller.0), strings(&["lib/worker.rb:go@2 -> lib/base.rb:run@2"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));

    // The superclass loses its `run` too.
    write(
        tmp.path(),
        base.0,
        "\
class Base
end
",
    );
    engine.sync(&[base.0.into()]);
    assert!(calls_from(rt, caller.0).is_empty(), "nothing defines `run` any more");
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));
}

/// A mixin names a module, never a class; an `include` inside a method or a
/// block, and one with a receiver, mix into something else and record no row;
/// a class is never its own superclass.
#[test]
fn only_a_body_statement_mixes_in_and_only_a_module_is_mixed_in() {
    let (_tmp, engine) = indexed(&[(
        "lib/a.rb",
        "\
class Plain
end

module Mixin
end

class User
  include Plain
  include Mixin

  def setup
    include Mixin
    self.class.include(Mixin)
  end
end

module Outer
  class Error < Error
  end
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::Implements), strings(&["lib/a.rb:User@7 -> lib/a.rb:Mixin@4"]));
    assert_eq!(rows(rt, EdgeKind::Implements), strings(&["Mixin Path true", "Plain Path false"]));
    assert_eq!(rows(rt, EdgeKind::Extends), strings(&["Error Path false"]));
    assert!(edges(rt, EdgeKind::Extends).is_empty());
}

/// Ruby skips an `include` of a module its superclass's ancestors already
/// hold, so the module stays after the superclass's own methods: a subclass
/// re-including it — directly, or through another module — never reads it
/// before the superclass.
#[test]
fn a_module_the_superclass_already_mixes_in_stays_after_the_superclass() {
    let (_tmp, engine) = indexed(&[
        (
            "app/controllers/concerns/authentication.rb",
            "\
module Authentication
  def current_user
  end
end
",
        ),
        (
            "app/controllers/concerns/admin_tools.rb",
            "\
module AdminTools
  include Authentication

  def audit
  end
end
",
        ),
        (
            "app/controllers/application_controller.rb",
            "\
class ApplicationController
  include Authentication

  def current_user
    super
  end
end
",
        ),
        (
            "app/controllers/admin_controller.rb",
            "\
class AdminController < ApplicationController
  include AdminTools
  include Authentication

  def show
    current_user
  end

  def log
    audit
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "app/controllers/admin_controller.rb"),
        strings(&[
            // `AdminTools` itself is new to the chain: read before the superclass.
            "app/controllers/admin_controller.rb:log@9 -> app/controllers/concerns/admin_tools.rb:audit@4",
            "app/controllers/admin_controller.rb:show@5 -> app/controllers/application_controller.rb:current_user@4",
        ])
    );
}

/// A method defined in a block of a class or module body — a Rails concern's
/// `class_methods do … end` — is defined on whatever object the block is
/// evaluated against, never as one of the body's instance methods: an instance
/// call walking the ancestors does not reach it.
#[test]
fn a_method_defined_in_a_body_block_answers_no_instance_call() {
    let (_tmp, engine) = indexed(&[
        (
            "app/models/concerns/findable.rb",
            "\
module Findable
  class_methods do
    def finder
    end
  end

  def found
  end
end
",
        ),
        (
            "app/models/post.rb",
            "\
class Post
  include Findable

  def a
    finder
  end

  def b
    found
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "app/models/post.rb"),
        strings(&["app/models/post.rb:b@8 -> app/models/concerns/findable.rb:found@7"])
    );
}

/// The pattern-matching forms that bind a local without a `value:` — `in
/// {name:}`'s key, `h => x`, `h in x` — hide a same-named method as every other
/// local does.
#[test]
fn a_pattern_variable_bound_by_its_key_or_a_rightward_match_is_a_local() {
    let file = "lib/patterns.rb";
    let (_tmp, engine) = indexed(&[(
        file,
        "\
class Patterns
  def name
  end

  def x
  end

  def y
  end

  def by_key(h)
    case h
    in {name:}
      name
    end
  end

  def rightward(h)
    h => x
    x
  end

  def test(h)
    h in y
    y
  end
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(calls_from(rt, file), Vec::<String>::new());
}

/// A block's implicit parameters — `it` and `_1` … `_9` — are block locals no
/// parameter list names: a bare one is never a call, even beside a method of
/// the same name.
#[test]
fn an_implicit_block_parameter_is_a_local() {
    let file = "lib/implicit.rb";
    let (_tmp, engine) = indexed(&[(
        file,
        "\
class Implicit
  def it
  end

  def sizes(xs)
    xs.map { it.size }
  end

  def firsts(xs)
    xs.map { _1 }
  end
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        call_rows_in(rt, file),
        strings(&[
            "lib/implicit.rb:firsts@9 map Method false",
            "lib/implicit.rb:sizes@5 map Method false",
            "lib/implicit.rb:sizes@5 size Method false",
        ])
    );
}

/// A reopening in another file that holds a class's only mixin loses it, then
/// is deleted: the class's own name is all the sync dirties, and the hierarchy
/// no longer holds it. Each sync still lands where a cold index does
/// (NFR-RA-06).
#[test]
fn sync_equals_a_full_reindex_after_a_class_loses_its_last_ancestry_row() {
    let caller = (
        "lib/worker.rb",
        "\
class Worker
  def go
    run
  end
end
",
    );
    let runner = (
        "lib/runner.rb",
        "\
module Runner
  def run
  end
end
",
    );
    let reopening = (
        "lib/worker_ext.rb",
        "\
class Worker
  include Runner
end
",
    );
    let (tmp, engine) = indexed(&[caller, runner, reopening]);
    let rt = engine.runtime().unwrap();
    let files = [caller.0, runner.0, reopening.0];
    assert_eq!(calls_from(rt, caller.0), strings(&["lib/worker.rb:go@2 -> lib/runner.rb:run@2"]));

    write(
        tmp.path(),
        reopening.0,
        "\
class Worker
  def other
  end
end
",
    );
    engine.sync(&[reopening.0.into()]);
    assert!(calls_from(rt, caller.0).is_empty(), "the reopening mixes nothing in now");
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));

    write(tmp.path(), reopening.0, reopening.1);
    engine.sync(&[reopening.0.into()]);
    assert_eq!(calls_from(rt, caller.0), strings(&["lib/worker.rb:go@2 -> lib/runner.rb:run@2"]));
    fs::remove_file(tmp.path().join(reopening.0)).unwrap();
    engine.sync(&[reopening.0.into()]);
    assert!(calls_from(rt, caller.0).is_empty(), "the reopening is gone");
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &files));
}

/// An `extend` adds the object's singleton methods, never a level of its
/// instances' ancestors: an extended module's method answers no instance call,
/// and an external `extend` (a Rails concern's `ActiveSupport::Concern`) does
/// not stop the walk the way an external `include` does.
#[test]
fn an_extended_module_is_no_level_of_the_instance_walk() {
    let file = "lib/extended.rb";
    let (_tmp, engine) = indexed(&[(
        file,
        "\
module Helpers
  def x
  end
end

module Concern
  extend ActiveSupport::Concern

  def y
  end
end

class Foo
  extend Helpers
  include Concern

  def m
    x
    y
  end
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(calls_from(rt, file), strings(&["lib/extended.rb:m@17 -> lib/extended.rb:y@9"]));
}

/// A mixed-in module brings what it mixes in itself — its `prepend`s as well
/// as its `include`s — and a module it mixes in that the graph cannot see stops
/// the walk before the superclass, as the class's own would.
#[test]
fn a_mixins_own_mixins_are_read_with_it() {
    let file = "lib/nested.rb";
    let (_tmp, engine) = indexed(&[(
        file,
        "\
module Inner
  def z
  end
end

module Outer
  prepend Inner
end

module Ext
  include Comparable
end

class Base
  def run
  end
end

class Bar < Base
  include Outer

  def m
    z
  end

  def n
    run
  end
end

class W < Base
  include Ext

  def go
    run
  end
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, file),
        strings(&[
            "lib/nested.rb:m@22 -> lib/nested.rb:z@2",
            "lib/nested.rb:n@26 -> lib/nested.rb:run@15",
        ]),
        "`W`'s `Ext` includes an unseen module, which may define `run`"
    );
}

/// A reopened class is one class: a method only a reopening defines, and a
/// superclass only a reopening names, are the class's. Two declarations
/// naming different superclasses contradict each other and bind nothing.
#[test]
fn every_reopening_of_the_callers_class_is_read_as_one_class() {
    let (_tmp, engine) = indexed(&[
        (
            "lib/re.rb",
            "\
class Re
  def m
    only_here
  end

  def n
    from_parent
  end
end
",
        ),
        (
            "lib/re_ext.rb",
            "\
class Re < Par
  def only_here
  end
end
",
        ),
        (
            "lib/par.rb",
            "\
class Par
  def from_parent
  end
end
",
        ),
        (
            "lib/two.rb",
            "\
class Two < P1
  def m
    q
  end
end
",
        ),
        (
            "lib/two_ext.rb",
            "\
class Two < P2
end
",
        ),
        (
            "lib/ps.rb",
            "\
class P1
  def q
  end
end

class P2
  def q
  end
end
",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_from(rt, "lib/re.rb"),
        strings(&["lib/re.rb:m@2 -> lib/re_ext.rb:only_here@2", "lib/re.rb:n@6 -> lib/par.rb:from_parent@2"])
    );
    assert_eq!(calls_from(rt, "lib/two.rb"), Vec::<String>::new(), "two superclasses decide nothing");
}

/// A superclass is read where Ruby evaluates it, before the class body opens:
/// `class Foo < Bar` names the `Bar` around `Foo`, never a `Foo::Bar` the body
/// declares.
#[test]
fn a_superclass_is_read_from_the_scope_around_the_class() {
    let file = "lib/scope.rb";
    let (_tmp, engine) = indexed(&[(
        file,
        "\
class Bar
  def k
  end
end

class Foo < Bar
  class Bar
  end

  def m
    k
  end
end
",
    )]);
    let rt = engine.runtime().unwrap();
    assert_eq!(edges(rt, EdgeKind::Extends), strings(&["lib/scope.rb:Foo@6 -> lib/scope.rb:Bar@1"]));
    assert_eq!(calls_from(rt, file), strings(&["lib/scope.rb:m@10 -> lib/scope.rb:k@2"]));
}

/// A sync that dirties only a mixed-in module's name — an unseen `include`
/// that starts naming a module, a module that starts mixing one in — moves
/// the walk of every class mixing it in, in a project whose only ancestry rows
/// are mixins too; each sync lands where a cold index does (NFR-RA-06).
#[test]
fn sync_equals_a_full_reindex_after_a_mixed_in_module_moves() {
    let worker = (
        "lib/worker.rb",
        "\
class Worker < Base
  include Runner

  def go
    run
  end
end

class Base
  def run
  end
end
",
    );
    let (tmp, engine) = indexed(&[worker]);
    let rt = engine.runtime().unwrap();
    let runner = "lib/runner.rb";
    assert!(calls_from(rt, worker.0).is_empty(), "`Runner` is unseen: it may define `run`");
    write(
        tmp.path(),
        runner,
        "\
module Runner
  def other
  end
end
",
    );
    engine.sync(&[runner.into()]);
    assert_eq!(calls_from(rt, worker.0), strings(&["lib/worker.rb:go@4 -> lib/worker.rb:run@10"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &[worker.0, runner]));

    // Mixins only: `Solo` includes `Helper`, which starts including `Other`.
    let solo = (
        "lib/solo.rb",
        "\
class Solo
  include Helper

  def go
    run
  end
end
",
    );
    let helper = ("lib/helper.rb", "module Helper\nend\n");
    let other = (
        "lib/other.rb",
        "\
module Other
  def run
  end
end
",
    );
    let (tmp, engine) = indexed(&[solo, helper, other]);
    let rt = engine.runtime().unwrap();
    assert!(calls_from(rt, solo.0).is_empty());
    write(tmp.path(), helper.0, "module Helper\n  include Other\nend\n");
    engine.sync(&[helper.0.into()]);
    assert_eq!(calls_from(rt, solo.0), strings(&["lib/solo.rb:go@4 -> lib/other.rb:run@2"]));
    assert_eq!(binding_facts(rt), cold_facts(&tmp, &[solo.0, helper.0, other.0]));
}

/// Every remaining binding form hides a same-named method: a lambda's
/// parameter, a destructured parameter, a rest or destructured assignment, and
/// the pattern-matching variables — an alternative's whole pattern, an array or
/// find pattern's element, a keyword pattern's value, an `=>` binding.
#[test]
fn every_binding_form_is_a_local() {
    let file = "lib/forms.rb";
    let methods: String = ["a", "b", "c", "d", "e", "g", "h", "i", "j", "k", "n", "o"]
        .iter()
        .map(|m| format!("  def {m}\n  end\n\n"))
        .collect();
    let text = format!(
        "\
class Forms
{methods}  def lam
    ->(a) {{ a }}
  end

  def destructured(x, (b, c))
    b
    c
  end

  def rest
    *d = [1]
    d
  end

  def nested
    (e, g), h = [[1, 2], 3]
    e
    g
    h
  end

  def patterns(x)
    case x
    in Integer => i
      i
    in [k, *]
      k
    in [*, n, *]
      n
    in {{key: o}}
      o
    in j
      j
    end
  end
end
"
    );
    let (_tmp, engine) = indexed(&[(file, text.as_str())]);
    let rt = engine.runtime().unwrap();
    assert_eq!(calls_from(rt, file), Vec::<String>::new());
}

/// A bare name is a call in every value position the query lists: one method
/// per position, each reading the method `t` bare, each binds it.
#[test]
fn a_bare_name_is_a_call_in_every_value_position() {
    let file = "lib/positions.rb";
    let bodies: &[(&str, &str)] = &[
        ("statement", "t"),
        ("brace_block", "[1].each { t }"),
        ("then_branch", "if u\n      t\n    end"),
        ("else_branch", "if u\n      1\n    else\n      t\n    end"),
        ("ensure_clause", "begin\n      1\n    ensure\n      t\n    end"),
        ("begin_block", "begin\n      t\n    end"),
        ("parenthesized", "(t)"),
        ("receiver", "t.to_s"),
        ("argument", "puts(t)"),
        ("assigned", "v = t\n    v"),
        ("op_assigned", "v = 1\n    v += t"),
        ("binary_left", "t + 1"),
        ("binary_right", "1 + t"),
        ("unary", "!t"),
        ("ternary_condition", "t ? 1 : 2"),
        ("ternary_consequence", "u ? t : 1"),
        ("ternary_alternative", "u ? 1 : t"),
        ("if_condition", "if t\n    end"),
        ("unless_condition", "unless t\n    end"),
        ("while_condition", "while t\n    end"),
        ("until_condition", "until t\n    end"),
        ("if_modifier_body", "t if u"),
        ("if_modifier_condition", "1 if t"),
        ("unless_modifier_body", "t unless u"),
        ("unless_modifier_condition", "1 unless t"),
        ("while_modifier", "1 while t"),
        ("until_modifier", "1 until t"),
        ("element", "t[0]"),
        ("pair_value", "{ a: t }"),
        ("interpolated", "\"#{t}\""),
        ("array_element", "[t]"),
        ("case_value", "case t\n    when 1\n    end"),
    ];
    let mut text = String::from("class Positions\n  def t\n  end\n\n  def optional_value(x = t)\n  end\n\n  def keyword_value(x: t)\n  end\n");
    for (name, body) in bodies {
        text.push_str(&format!("\n  def {name}(u)\n    {body}\n  end\n"));
    }
    text.push_str("end\n");
    let (_tmp, engine) = indexed(&[(file, text.as_str())]);
    let rt = engine.runtime().unwrap();
    let mut callers: Vec<String> = calls_from(rt, file)
        .iter()
        .filter(|e| e.ends_with(&format!("-> {file}:t@2")))
        .map(|e| e.split(':').nth(1).unwrap().split('@').next().unwrap().to_string())
        .collect();
    callers.sort();
    let mut expected: Vec<String> = bodies.iter().map(|(name, _)| name.to_string()).collect();
    expected.extend(["optional_value".to_string(), "keyword_value".to_string()]);
    expected.sort();
    assert_eq!(callers, expected);
}
