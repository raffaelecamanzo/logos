//! Unit tests for the local-binding facts (S-593, [CR-191], [FR-EX-33]).
//!
//! In their own file, the `receiver_tests.rs` shape. One fixture per language,
//! read off the facts the real plugin's `locals` query records: each captured
//! form with its kind, scope span and position. Each binding renders as
//! `name kind line:col in start-end`, where the span is `line:col` to
//! `line:col` (the end column exclusive), with `hoisted` appended when it is
//! in force throughout its scope.
//!
//! [CR-191]: ../../../docs/requests/CR-191-a-local-binding-shadows-the-receiver-and-the-callee.md
//! [FR-EX-33]: ../../../docs/specs/requirements/FR-EX-33.md

use crate::extract::{extract, Facts, FileInput, LocalBindingFact, RefFact, SymbolContext};
use crate::model::{EdgeKind, ReceiverShape};
use crate::resolve::{LOCAL_IMPORT_ALIAS, LOCAL_VALUE_ALIAS};
use crate::plugin::LanguageRegistry;

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    })
}

fn facts_with(registry: &LanguageRegistry, path: &str, source: &str) -> Facts {
    let plugin = registry.for_path(path).expect("a grammar for the fixture");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let facts = extract(&FileInput::new(path, source), plugin, &ctx);
    assert!(!facts.partial, "{path} parses cleanly: {:?}", facts.warnings);
    facts
}

fn render(b: &LocalBindingFact) -> String {
    format!(
        "{} {} {}:{} in {}:{}-{}:{}{}",
        b.name,
        b.kind.as_str(),
        b.line,
        b.column,
        b.scope_start_line,
        b.scope_start_column,
        b.scope_end_line,
        b.scope_end_column,
        if b.hoisted { " hoisted" } else { "" }
    )
}

/// Every binding the fixture records, rendered, in position order.
fn bindings(path: &str, source: &str) -> Vec<String> {
    facts_with(registry(), path, source).local_bindings.iter().map(render).collect()
}

/// Print a fixture's bindings, one per line, for a failing assertion's message.
fn listing(rows: &[String]) -> String {
    rows.iter().map(|r| format!("\n    {r:?},")).collect()
}

#[track_caller]
fn assert_bindings(path: &str, source: &str, expected: &[&str]) {
    let actual = bindings(path, source);
    assert_eq!(actual, expected, "{path} records:{}", listing(&actual));
}

/// The fixtures of every language, by path — one per plugin declaring the
/// `locals` capability, JavaScript's read by the TypeScript plugin.
const FIXTURES: &[(&str, &str)] = &[
    ("svc.go", GO),
    ("lib.rs", RUST),
    ("svc.py", PYTHON),
    ("svc.js", JAVASCRIPT),
    ("svc.ts", JAVASCRIPT),
    ("svc.tsx", JAVASCRIPT),
    ("C.java", JAVA),
    ("C.kt", KOTLIN),
    ("C.scala", SCALA),
    ("C.cs", CSHARP),
    ("svc.cpp", CPP),
    ("svc.c", C),
    ("svc.php", PHP),
    ("svc.rb", RUBY),
];

const GO: &str = "package p

func (s *Svc) CloseAll(xs []Svc, n, limit int, rest ...string) (count int, err error) {
	var v, w, _ int
	const k = 1
	x, _ := s.open()
	for i := 0; i < n; i++ {
	}
	for _, s := range xs {
		s.Close()
	}
	if z := s.ok(); z {
	}
	switch t := v.(type) {
	}
	select {
	case m := <-ch:
		_ = m
	}
	f := func(s Svc) { s.Close() }
	var t = s
	var g = func() (int, error) {
		u := v
		return u, err
	}
	return 0, nil
}

func Plain(a int, _ string) {}

var top = 1
";

const RUST: &str = "use std::fmt;

fn handle(&self, a: i32, (b, c): (i32, i32), Point { x, y: yy }: Point) {
    early();
    let v = 1;
    let (p, mut q) = t;
    let Some(r) = o else { return };
    use std::collections::HashMap;
    use a::{b as bb, c::d, e::*};
    for (i, j) in xs {}
    if let Some(s) = o {}
    while let Ok(w) = x {}
    match o {
        Some(n) => {}
        Foo::Bar { f, .. } => {}
        z @ 1..=2 => {}
        None => {}
    }
    let cl = |u, w2: i32| u;
    let [s1, .., s2] = arr;
    let by_ref = |&k| k;
    use m::{g1, g2};
    if let Some(MAX) = o {}
    fn early() {}
}

const TOP: i32 = 1;
";

const PYTHON: &str = "import top

def f(self, a, b: int, c=1, *rest, d: int = 2, **kw):
    early()
    import os, sys, a.b as ab
    from m import h, j, k as kk
    v = 1
    p, (q, r) = t
    for i in xs:
        pass
    with open() as fh:
        pass
    try:
        pass
    except E as err:
        pass
    if (n := len(a)) > 1:
        pass
    ys = [y for y in xs]
    lam = lambda la: la
    match o:
        case [m1, *rest2]:
            pass
        case Point(x=px) as pt:
            pass
        case _:
            pass
    global g
    g = 2
    acc += 1
    def early():
        pass
    @cached
    def memo():
        pass
    class Local:
        pass

module_level = 1
";

const JAVASCRIPT: &str = "function f(a, { b, c: cc = 1 }, [d], ...rest) {
  early();
  var v = 1;
  let l = 2;
  const { k } = o;
  for (let i = 0; i < 3; i++) {}
  for (const x of xs) {}
  for (var y in o) {}
  for (z of xs) {}
  try {} catch (err) {}
  const arrow = (p) => p;
  const one = q => q;
  if (a) {
    var hoisted = 1;
    function blockFn() {}
  }
  function early() {}
  const { dflt = 1 } = o;
  class K {}
  function* gen(g1) {}
}

const top = 1;
class C { m(mp) { return mp; } }
";

const JAVA: &str = "class C {
  void m(int a, String... rest) {
    int v = 1, w;
    for (int i = 0; i < 3; i++) {}
    for (String s : xs) {}
    try (Res r = open()) {} catch (IOException e) {}
    Runnable lam = (p, q) -> {};
    Function<A, B> one = z -> z;
    if (o instanceof String str) {}
    if (o instanceof Point(int qx, int qy)) {}
    Predicate<Object> pr = o2 -> o2 instanceof String s2 && s2.isEmpty();
    switch (o) {
      case Point(int px, int py) -> {}
      default -> {}
    }
  }
  C(int ca) {}
  int field = 1;
  void colon(Object o) {
    switch (o) {
      case Point(int cx, int cy):
        break;
      default:
        break;
    }
  }
  static { int st = 1; }
}
";

const KOTLIN: &str = "class C(val prop: Int) {
    fun m(a: Int, vararg rest: String) {
        val v = 1
        val (d1, d2) = pair
        for (i in xs) {}
        try {} catch (e: Exception) {}
        val lam = { p: Int -> p }
        fun local(lp: Int) = lp
        when (val s = o) { else -> {} }
        xs.forEach { val y = it }
        for ((k, vv) in m) {}
    }
    constructor(b: String) : this(0)
    var x: Int = 0
        set(value) { field = value }
}
";

const SCALA: &str = "class C(cp: Int) {
  def m(a: Int)(implicit c: Ctx): Unit = {
    early()
    val v = 1
    val (d1, d2) = t
    val Some(sv) = o
    import scala.collection.mutable
    import a.b.{c2, d => dd, _}
    for (i <- xs; (k, l) <- ys) {}
    o match {
      case Some(n) => n
      case p @ Foo(_) => p
      case None => 0
    }
    val lam = (p: Int) => p
    def early(): Unit = {}
    xs match { case h :: t => h }
  }
  def indented(ia: Int): Int =
    val iv = ia
    iv
}
";

const CSHARP: &str = "class C {
  void M(int a, out int o) {
    Early();
    int v = 1, w;
    var (d1, d2) = t;
    for (int i = 0; i < 3; i++) {}
    foreach (var s in xs) {}
    try {} catch (Exception e) {}
    using (var u = Open()) {}
    Func<int, int> lam = p => p;
    if (o is string str) {}
    if (int.TryParse(s, out var parsed)) {}
    Func<string, bool> tp = x => int.TryParse(x, out var m);
    switch (o) { case int n: break; }
    void Early() {}
    var r = o switch { int n2 => n2, _ => 0 };
    var q = from qa in xs select qa;
  }
}
";

const CPP: &str = "void f(int a, const std::string& b, int* c) {
  int v = 1, w;
  auto [s1, s2] = pair;
  for (int i = 0; i < 3; i++) {}
  for (auto& e : xs) {}
  try {} catch (const std::exception& ex) {}
  using ns::fn;
  using namespace std;
  auto lam = [cap](int p) { return p; };
  if (int z = g(); z) {}
  while (auto* n = next()) { use(n); }
  switch (int sw = h()) { default: use(sw); }
}

Node** find(Node** head, int key) { return head; }

int* first(int* xs2) { if (auto ptr = xs2) { return ptr; } return 0; }

int top = 1;
";

const C: &str = "void f(int a, char *b) {
  int v = 1, w;
  for (int i = 0; i < 3; i++) {}
  int *p;
}

char **split(const char *s, int n) { return 0; }

char *dup(char *src) { return src; }

int top = 1;
";

const PHP: &str = "<?php
function f($a, int $b = 1, ...$rest) {
  $v = 1;
  [$d1, $d2] = $t;
  foreach ($xs as $k => $vv) {}
  try {} catch (Exception $e) {}
  $cl = function ($p) use ($v) { return $p; };
  $ar = fn($q) => $q;
  global $g;
  static $st = 0;
  $this->x = 1;
  foreach ($xs as $x) {}
}

class K { function m($ma) { return $ma; } }

$top = 1;
";

const RUBY: &str = "def m(a, b = 1, *rest, k:, &blk)
  v = 1
  d1, d2 = t
  for i in xs do end
  xs.each { |e| e }
  begin
  rescue StandardError => err
  end
  lam = ->(p) { p }
  @iv = 1
  w += 1
  xs.each do |x| x end
  case o
  in [m1, *mr] then nil
  in {name: nm} then nil
  end
end

def self.s(sa); end

top = 1
";

/// Go: parameters, named results, `var` / `const`, `:=`, a `for` clause, a
/// `range` (rebinding the receiver's name), an `if` initializer, a type switch,
/// a `select` case and a function literal's parameter (the receiver's name
/// again). The method's own receiver and `_` are never bindings, nor is any
/// name an initializer or a function literal's body reads (`var t = s` binds
/// `t` only); nothing at file scope is a callable's.
#[test]
fn go_records_parameters_declarations_loops_and_closures_but_never_the_receiver() {
    assert_bindings(
        "svc.go",
        GO,
        &[
            "xs parameter 3:23 in 3:0-27:1",
            "n parameter 3:33 in 3:0-27:1",
            "limit parameter 3:36 in 3:0-27:1",
            "rest parameter 3:47 in 3:0-27:1",
            "count parameter 3:64 in 3:0-27:1",
            "err parameter 3:75 in 3:0-27:1",
            "v variable 4:5 in 4:1-27:0",
            "w variable 4:8 in 4:1-27:0",
            "k variable 5:7 in 4:1-27:0",
            "x variable 6:1 in 4:1-27:0",
            "i variable 7:5 in 7:1-8:2",
            "s variable 9:8 in 9:1-11:2",
            "z variable 12:4 in 12:1-13:2",
            "t variable 14:8 in 14:1-15:2",
            "m variable 17:6 in 17:1-19:0",
            "f variable 20:1 in 4:1-27:0",
            "s parameter 20:11 in 20:6-20:31",
            "t variable 21:5 in 4:1-27:0",
            "g variable 22:5 in 4:1-27:0",
            "u variable 23:2 in 23:2-25:0",
            "a parameter 29:11 in 29:0-29:30",
        ],
    );
}

/// Rust: destructured parameters, `let` (a `let … else` too), loops, `if let`,
/// `while let`, match arms and closures; a block's `use` and `fn` items bind
/// for the whole block. Never `self`, and never an upper-case pattern name — a
/// variant (`Some`, `None`) or a constant — nor a struct or path name.
#[test]
fn rust_records_patterns_and_block_items_but_never_self_or_a_variant() {
    assert_bindings(
        "lib.rs",
        RUST,
        &[
            "a parameter 3:17 in 3:0-25:1",
            "b parameter 3:26 in 3:0-25:1",
            "c parameter 3:29 in 3:0-25:1",
            "x parameter 3:53 in 3:0-25:1",
            "yy parameter 3:59 in 3:0-25:1",
            "v variable 5:8 in 3:72-25:1",
            "p variable 6:9 in 3:72-25:1",
            "q variable 6:16 in 3:72-25:1",
            "r variable 7:13 in 3:72-25:1",
            "HashMap import 8:26 in 3:72-25:1 hoisted",
            "bb import 9:17 in 3:72-25:1 hoisted",
            "d import 9:24 in 3:72-25:1 hoisted",
            "i variable 10:9 in 10:4-10:23",
            "j variable 10:12 in 10:4-10:23",
            "s variable 11:16 in 11:4-11:25",
            "w variable 12:17 in 12:4-12:26",
            "n variable 14:13 in 14:8-14:21",
            "f variable 15:19 in 15:8-15:32",
            "z variable 16:8 in 16:8-16:23",
            "cl variable 19:8 in 3:72-25:1",
            "u parameter 19:14 in 19:13-19:27",
            "w2 parameter 19:17 in 19:13-19:27",
            "s1 variable 20:9 in 3:72-25:1",
            "s2 variable 20:17 in 3:72-25:1",
            "by_ref variable 21:8 in 3:72-25:1",
            "k parameter 21:19 in 21:17-21:23",
            "g1 import 22:12 in 3:72-25:1 hoisted",
            "g2 import 22:16 in 3:72-25:1 hoisted",
            "early variable 24:7 in 3:72-25:1 hoisted",
        ],
    );
}

/// Python: every local is the function's and binds throughout it — imports,
/// assignments, loop, `with`, `except`, walrus and `match` captures, a nested
/// `def` — but a comprehension's variable, scoped to the comprehension, and a
/// lambda's parameter, to the lambda. `self` is a parameter; `_`, a class
/// pattern's name and a keyword are not bindings; nothing at module level is.
#[test]
fn python_binds_every_local_for_the_whole_function_but_a_comprehension_variable() {
    assert_bindings(
        "svc.py",
        PYTHON,
        &[
            "self parameter 3:6 in 3:0-37:12",
            "a parameter 3:12 in 3:0-37:12",
            "b parameter 3:15 in 3:0-37:12",
            "c parameter 3:23 in 3:0-37:12",
            "rest parameter 3:29 in 3:0-37:12",
            "d parameter 3:35 in 3:0-37:12",
            "kw parameter 3:49 in 3:0-37:12",
            "os import 5:11 in 3:0-37:12 hoisted",
            "sys import 5:15 in 3:0-37:12 hoisted",
            "ab import 5:27 in 3:0-37:12 hoisted",
            "h import 6:18 in 3:0-37:12 hoisted",
            "j import 6:21 in 3:0-37:12 hoisted",
            "kk import 6:29 in 3:0-37:12 hoisted",
            "v variable 7:4 in 3:0-37:12 hoisted",
            "p variable 8:4 in 3:0-37:12 hoisted",
            "q variable 8:8 in 3:0-37:12 hoisted",
            "r variable 8:11 in 3:0-37:12 hoisted",
            "i variable 9:8 in 3:0-37:12 hoisted",
            "fh variable 11:19 in 3:0-37:12 hoisted",
            "err variable 15:16 in 3:0-37:12 hoisted",
            "n variable 17:8 in 3:0-37:12 hoisted",
            "ys variable 19:4 in 3:0-37:12 hoisted",
            "y variable 19:16 in 19:9-19:24",
            "lam variable 20:4 in 3:0-37:12 hoisted",
            "la parameter 20:17 in 20:10-20:23",
            "m1 variable 22:14 in 3:0-37:12 hoisted",
            "rest2 variable 22:19 in 3:0-37:12 hoisted",
            "px variable 24:21 in 3:0-37:12 hoisted",
            "pt variable 24:28 in 3:0-37:12 hoisted",
            "g variable 29:4 in 3:0-37:12 hoisted",
            "acc variable 30:4 in 3:0-37:12 hoisted",
            "early variable 31:8 in 3:0-37:12 hoisted",
            "memo variable 34:8 in 3:0-37:12 hoisted",
            "Local variable 36:10 in 3:0-37:12 hoisted",
        ],
    );
}

/// JavaScript: a `var` binds for its whole function, even inside an `if`
/// block; a function declaration for its whole block; a `let` / `const` for its
/// whole block (its temporal dead zone); a loop, `catch` or parameter name from
/// its position. `for (z of xs)` declares nothing, and nothing at the top level
/// is a callable's.
#[test]
fn javascript_var_and_function_declarations_bind_for_their_whole_scope() {
    assert_bindings(
        "svc.js",
        JAVASCRIPT,
        &[
            "a parameter 1:11 in 1:0-21:1",
            "b parameter 1:16 in 1:0-21:1",
            "cc parameter 1:22 in 1:0-21:1",
            "d parameter 1:33 in 1:0-21:1",
            "rest parameter 1:40 in 1:0-21:1",
            "v variable 3:6 in 1:0-21:1 hoisted",
            "l variable 4:6 in 1:46-21:1 hoisted",
            "k variable 5:10 in 1:46-21:1 hoisted",
            "i variable 6:11 in 6:2-6:32 hoisted",
            "x variable 7:13 in 7:2-7:24",
            "y variable 8:11 in 1:0-21:1 hoisted",
            "err variable 10:16 in 10:9-10:23",
            "arrow variable 11:8 in 1:46-21:1 hoisted",
            "p parameter 11:17 in 11:16-11:24",
            "one variable 12:8 in 1:46-21:1 hoisted",
            "q parameter 12:14 in 12:14-12:20",
            "hoisted variable 14:8 in 1:0-21:1 hoisted",
            "blockFn variable 15:13 in 13:9-16:3 hoisted",
            "early variable 17:11 in 1:46-21:1 hoisted",
            "dflt variable 18:10 in 1:46-21:1 hoisted",
            "K variable 19:8 in 1:46-21:1 hoisted",
            "gen variable 20:12 in 1:46-21:1 hoisted",
            "g1 parameter 20:16 in 20:2-20:22",
            "mp parameter 24:12 in 24:10-24:30",
        ],
    );
}

/// TypeScript and TSX read JavaScript's fixture as JavaScript does.
#[test]
fn typescript_and_tsx_record_what_javascript_records() {
    let javascript = bindings("svc.js", JAVASCRIPT);
    assert!(!javascript.is_empty());
    assert_eq!(bindings("svc.ts", JAVASCRIPT), javascript);
    assert_eq!(bindings("svc.tsx", JAVASCRIPT), javascript);
}

/// Java: parameters (varargs included), declarations, loops, resources,
/// `catch`, lambda parameters, an `instanceof` type or record pattern (its
/// block's — or its lambda's, when the lambda has no block) and a `switch`
/// record pattern (its case's). A field is not a local.
#[test]
fn java_records_parameters_declarations_resources_and_patterns() {
    assert_bindings(
        "C.java",
        JAVA,
        &[
            "a parameter 2:13 in 2:2-16:3",
            "rest parameter 2:26 in 2:2-16:3",
            "v variable 3:8 in 2:32-16:3",
            "w variable 3:15 in 2:32-16:3",
            "i variable 4:13 in 4:4-4:34",
            "s variable 5:16 in 5:4-5:26",
            "r variable 6:13 in 6:4-6:52",
            "e variable 6:47 in 6:28-6:52",
            "lam variable 7:13 in 2:32-16:3",
            "p parameter 7:20 in 7:19-7:31",
            "q parameter 7:23 in 7:19-7:31",
            "one variable 8:19 in 2:32-16:3",
            "z parameter 8:25 in 8:25-8:31",
            "str variable 9:28 in 2:32-16:3",
            "qx variable 10:31 in 2:32-16:3",
            "qy variable 10:39 in 2:32-16:3",
            "pr variable 11:22 in 2:32-16:3",
            "o2 parameter 11:27 in 11:27-11:72",
            "s2 variable 11:54 in 11:27-11:72",
            "px variable 13:21 in 13:6-13:38",
            "py variable 13:29 in 13:6-13:38",
            "ca parameter 17:8 in 17:2-17:14",
            "o parameter 19:20 in 19:2-26:3",
            "cx variable 21:21 in 21:6-22:14",
            "cy variable 21:29 in 21:6-22:14",
            "st variable 27:15 in 27:9-27:24",
        ],
    );
}

/// Kotlin: parameters, `val` (destructuring too), loops, `catch`, lambda and
/// setter parameters, a local `fun` and a `when` subject. A class parameter or
/// property is not a local.
#[test]
fn kotlin_records_parameters_declarations_and_local_functions() {
    assert_bindings(
        "C.kt",
        KOTLIN,
        &[
            "a parameter 2:10 in 2:4-12:5",
            "rest parameter 2:25 in 2:4-12:5",
            "v variable 3:12 in 2:39-12:5",
            "d1 variable 4:13 in 2:39-12:5",
            "d2 variable 4:17 in 2:39-12:5",
            "i variable 5:13 in 5:8-5:24",
            "e variable 6:22 in 6:15-6:38",
            "lam variable 7:12 in 2:39-12:5",
            "p parameter 7:20 in 7:18-7:33",
            "local variable 8:12 in 2:39-12:5",
            "lp parameter 8:18 in 8:8-8:31",
            "s variable 9:18 in 9:8-9:39",
            "y variable 10:25 in 10:19-10:33",
            "k variable 11:14 in 11:8-11:29",
            "vv variable 11:17 in 11:8-11:29",
            "b parameter 13:16 in 13:4-13:36",
            "value parameter 15:12 in 15:8-15:36",
        ],
    );
}

/// Scala: every parameter list of a method, block definitions binding for the
/// whole block, local imports from their position, enumerators and `case`
/// patterns — never an upper-case stable identifier (`None`, `Some`, `Foo`).
#[test]
fn scala_records_every_parameter_list_block_definitions_and_local_imports() {
    assert_bindings(
        "C.scala",
        SCALA,
        &[
            "a parameter 2:8 in 2:2-18:3",
            "c parameter 2:25 in 2:2-18:3",
            "v variable 4:8 in 2:41-18:3 hoisted",
            "d1 variable 5:9 in 2:41-18:3 hoisted",
            "d2 variable 5:13 in 2:41-18:3 hoisted",
            "sv variable 6:13 in 2:41-18:3 hoisted",
            "mutable import 7:28 in 2:41-18:3",
            "c2 import 8:16 in 2:41-18:3",
            "dd import 8:25 in 2:41-18:3",
            "i variable 9:9 in 9:4-9:34",
            "k variable 9:19 in 9:4-9:34",
            "l variable 9:22 in 9:4-9:34",
            "n variable 11:16 in 11:6-12:6",
            "p variable 12:11 in 12:6-13:6",
            "lam variable 15:8 in 2:41-18:3 hoisted",
            "p parameter 15:15 in 15:14-15:27",
            "early variable 16:8 in 2:41-18:3 hoisted",
            "h variable 17:20 in 17:15-17:31",
            "t variable 17:25 in 17:15-17:31",
            "ia parameter 19:15 in 19:2-22:0",
            "iv variable 20:8 in 20:4-22:0 hoisted",
        ],
    );
}

/// C#: a local binds for its whole block — a declaration, a deconstruction, an
/// `out var` or `is` pattern variable (an expression-bodied lambda's, when it
/// sits in one), a local function; a loop, `catch`, `using` or `switch`-section
/// variable for its statement or section.
#[test]
fn c_sharp_locals_bind_for_their_whole_block() {
    assert_bindings(
        "C.cs",
        CSHARP,
        &[
            "a parameter 2:13 in 2:2-18:3",
            "o parameter 2:24 in 2:2-18:3",
            "v variable 4:8 in 2:27-18:3 hoisted",
            "w variable 4:15 in 2:27-18:3 hoisted",
            "d1 variable 5:9 in 2:27-18:3 hoisted",
            "d2 variable 5:13 in 2:27-18:3 hoisted",
            "i variable 6:13 in 6:4-6:34 hoisted",
            "s variable 7:17 in 7:4-7:28 hoisted",
            "e variable 8:28 in 8:11-8:33 hoisted",
            "u variable 9:15 in 9:4-9:29 hoisted",
            "lam variable 10:19 in 2:27-18:3 hoisted",
            "p parameter 10:25 in 10:25-10:31",
            "str variable 11:20 in 2:27-18:3 hoisted",
            "parsed variable 12:32 in 2:27-18:3 hoisted",
            "tp variable 13:23 in 2:27-18:3 hoisted",
            "x parameter 13:28 in 13:28-13:59",
            "m variable 13:57 in 13:28-13:59 hoisted",
            "n variable 14:26 in 14:17-14:35 hoisted",
            "Early variable 15:9 in 2:27-18:3 hoisted",
            "r variable 16:8 in 2:27-18:3 hoisted",
            "n2 variable 16:27 in 16:23-16:35 hoisted",
            "q variable 17:8 in 2:27-18:3 hoisted",
            "qa variable 17:17 in 17:12-17:35",
        ],
    );
}

/// C++: parameters through reference and pointer declarators (a `T**` return
/// too), declarations
/// (structured bindings too), loops, `catch`, a lambda's parameters, an `if`
/// initializer, a `while` / `switch` condition declaration (the statement's)
/// and a block-scope `using ns::f` — never `using namespace`, a
/// lambda capture, or a top-level declaration.
#[test]
fn cpp_records_declarations_structured_bindings_and_a_block_using() {
    assert_bindings(
        "svc.cpp",
        CPP,
        &[
            "a parameter 1:11 in 1:0-13:1",
            "b parameter 1:33 in 1:0-13:1",
            "c parameter 1:41 in 1:0-13:1",
            "v variable 2:6 in 1:44-13:1",
            "w variable 2:13 in 1:44-13:1",
            "s1 variable 3:8 in 1:44-13:1",
            "s2 variable 3:12 in 1:44-13:1",
            "i variable 4:11 in 4:2-4:32",
            "e variable 5:13 in 5:2-5:23",
            "ex variable 6:38 in 6:9-6:44",
            "fn import 7:12 in 1:44-13:1",
            "lam variable 9:7 in 1:44-13:1",
            "p parameter 9:23 in 9:13-9:39",
            "z variable 10:10 in 10:2-10:24",
            "n variable 11:15 in 11:2-11:38",
            "sw variable 12:14 in 12:2-12:45",
            "head parameter 15:19 in 15:0-15:50",
            "key parameter 15:29 in 15:0-15:50",
            "xs2 parameter 17:16 in 17:0-17:70",
            "ptr variable 17:32 in 17:23-17:58",
        ],
    );
}

/// C: parameters — of a function returning a pointer to a pointer too —
/// declarations through pointer declarators, a `for` initializer; nothing at
/// file scope.
#[test]
fn c_records_parameters_and_declarations() {
    assert_bindings(
        "svc.c",
        C,
        &[
            "a parameter 1:11 in 1:0-5:1",
            "b parameter 1:20 in 1:0-5:1",
            "v variable 2:6 in 1:23-5:1",
            "w variable 2:13 in 1:23-5:1",
            "i variable 3:11 in 3:2-3:32",
            "p variable 4:7 in 1:23-5:1",
            "s parameter 7:25 in 7:0-7:48",
            "n parameter 7:32 in 7:0-7:48",
            "src parameter 9:16 in 9:0-9:36",
        ],
    );
}

/// PHP: every variable is its function's, keeping its `$`; a closure's `use`
/// captures are its parameters. `global`, `$this->x` and file scope bind none.
#[test]
fn php_records_function_scoped_variables_and_closure_captures() {
    assert_bindings(
        "svc.php",
        PHP,
        &[
            "$a parameter 2:11 in 2:0-13:1",
            "$b parameter 2:19 in 2:0-13:1",
            "$rest parameter 2:30 in 2:0-13:1",
            "$v variable 3:2 in 2:0-13:1",
            "$d1 variable 4:3 in 2:0-13:1",
            "$d2 variable 4:8 in 2:0-13:1",
            "$k variable 5:18 in 2:0-13:1",
            "$vv variable 5:24 in 2:0-13:1",
            "$e variable 6:26 in 2:0-13:1",
            "$cl variable 7:2 in 2:0-13:1",
            "$p parameter 7:18 in 7:8-7:45",
            "$v parameter 7:27 in 7:8-7:45",
            "$ar variable 8:2 in 2:0-13:1",
            "$q parameter 8:11 in 8:8-8:20",
            "$st variable 10:9 in 2:0-13:1",
            "$x variable 12:18 in 2:0-13:1",
            "$ma parameter 15:21 in 15:10-15:41",
        ],
    );
}

/// Ruby: method, block and lambda parameters, assignments (multiple too), a
/// `for` and `rescue => e` — the method's or block's. Instance variables and
/// file scope bind none.
#[test]
fn ruby_records_parameters_assignments_and_block_parameters() {
    assert_bindings(
        "svc.rb",
        RUBY,
        &[
            "a parameter 1:6 in 1:0-17:3",
            "b parameter 1:9 in 1:0-17:3",
            "rest parameter 1:17 in 1:0-17:3",
            "k parameter 1:23 in 1:0-17:3",
            "blk parameter 1:28 in 1:0-17:3",
            "v variable 2:2 in 1:0-17:3",
            "d1 variable 3:2 in 1:0-17:3",
            "d2 variable 3:6 in 1:0-17:3",
            "i variable 4:6 in 1:0-17:3",
            "e parameter 5:13 in 5:10-5:19",
            "err variable 7:26 in 1:0-17:3",
            "lam variable 9:2 in 1:0-17:3",
            "p parameter 9:11 in 9:8-9:19",
            "w variable 11:2 in 1:0-17:3",
            "x parameter 12:14 in 12:10-12:22",
            "m1 variable 14:6 in 1:0-17:3",
            "mr variable 14:11 in 1:0-17:3",
            "nm variable 15:12 in 1:0-17:3",
            "sa parameter 19:11 in 19:0-19:19",
        ],
    );
}

/// Recording the facts moves no node, symbol or edge, and a reference only as
/// the S-594 shadowing rules decide (`local_shadow`): every fixture extracts
/// byte-identically with the `locals` query emptied by an override, which
/// records no binding at all, but for the `Calls` rows a binding in force
/// shadows — a bare call marked as through a local value or import, or a
/// rebound receiver read `other`.
#[test]
fn the_facts_change_no_node_symbol_edge_and_only_shadowed_calls() {
    let root = tempfile::tempdir().expect("tempdir");
    for (path, _) in FIXTURES {
        let plugin = registry().for_path(path).expect("a grammar");
        let dir = root.path().join(".logos").join("plugins").join(plugin.name()).join("queries");
        std::fs::create_dir_all(&dir).expect("override dir");
        std::fs::write(dir.join("locals.scm"), "").expect("empty override");
    }
    let emptied = LanguageRegistry::load(root.path()).expect("registry with emptied locals loads");
    for (path, source) in FIXTURES {
        let with = facts_with(registry(), path, source);
        let without = facts_with(&emptied, path, source);
        assert!(!with.local_bindings.is_empty(), "{path}: the fixture records bindings");
        assert!(without.local_bindings.is_empty(), "{path}: an emptied query records none");
        assert_eq!(
            (&with.nodes, &with.edges, &with.warnings),
            (&without.nodes, &without.edges, &without.warnings),
            "{path}: the facts move nothing else"
        );
        let shadowed = |r: &RefFact| {
            r.kind == EdgeKind::Calls
                && ([Some(LOCAL_VALUE_ALIAS), Some(LOCAL_IMPORT_ALIAS)].contains(&r.alias.as_deref())
                    || r.receiver == Some(ReceiverShape::Other))
        };
        let moved: Vec<&RefFact> = with.refs.iter().filter(|r| !without.refs.contains(r)).collect();
        assert!(moved.iter().all(|r| shadowed(r)), "{path}: only shadowed calls move: {moved:#?}");
        assert_eq!(
            with.refs.len(),
            without.refs.len(),
            "{path}: a shadowed call is the same call, one row each side"
        );
        assert_eq!(with.impl_blocks, without.impl_blocks, "{path}");
        assert_eq!(with.declared_types, without.declared_types, "{path}");
    }
}

/// Every code plugin declaring `locals` ships a query that compiles, and every
/// capture it names is in the vocabulary [`super`] reads — a misspelt capture
/// would silently record nothing.
#[test]
fn every_locals_query_compiles_and_speaks_only_the_vocabulary() {
    let known = |name: &str| -> bool {
        let Some(role) = name.strip_prefix("local.") else { return false };
        matches!(role, "name" | "callable" | "block")
            || crate::model::LocalBindingKind::ALL.iter().any(|k| {
                role == k.as_str()
                    || ["scope", "declaration", "block", "hoisted"]
                        .iter()
                        .any(|c| role == format!("{}.{c}", k.as_str()))
            })
    };
    let mut seen = 0;
    for plugin in registry().iter() {
        if !plugin.capabilities().iter().any(|c| c == "locals") {
            continue;
        }
        let query = plugin
            .query("locals")
            .unwrap_or_else(|| panic!("{}: the locals query compiles", plugin.name()));
        for name in query.capture_names() {
            assert!(known(name), "{}: `@{name}` is outside the local-binding vocabulary", plugin.name());
        }
        seen += 1;
    }
    assert_eq!(seen, 14, "every code plugin declares the capability");
}
