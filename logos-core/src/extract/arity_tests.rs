//! Unit tests for parameter ranges, argument counts and the Rust takes-`self`
//! fact (S-591, [CR-190], [CR-200], [FR-EX-32]).
//!
//! In their own file, the `receiver_tests.rs` shape. One fixture per language,
//! read off the facts the real plugin's queries record: required, defaulted and
//! variadic parameters, a receiver parameter, and plain, trailing-lambda,
//! multi-list and spread calls. One Rust fixture per `self` form.
//!
//! [CR-190]: ../../../docs/requests/CR-190-a-self-call-binds-only-a-callable-whose-arity-admits-it.md
//! [CR-200]: ../../../docs/requests/CR-200-a-rust-method-call-binds-only-a-callable-that-takes-self.md
//! [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md

use std::collections::BTreeMap;

use crate::extract::{extract, Facts, FileInput, SymbolContext};
use crate::model::{EdgeKind, NodeKind, ParamRange};
use crate::plugin::LanguageRegistry;

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    })
}

fn facts(path: &str, source: &str) -> Facts {
    let plugin = registry().for_path(path).expect("a grammar for the fixture");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let facts = extract(&FileInput::new(path, source), plugin, &ctx);
    assert!(!facts.partial, "{path} parses cleanly: {:?}", facts.warnings);
    facts
}

/// `[min, max]`, `None` for an unbounded maximum.
fn range(min: u32, max: Option<u32>) -> Option<ParamRange> {
    Some(ParamRange { min, max })
}

/// Every callable's range, by name.
fn ranges(path: &str, source: &str) -> BTreeMap<String, Option<ParamRange>> {
    facts(path, source)
        .nodes
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name, n.params))
        .collect()
}

/// Every `Calls` row's argument count, by target, the counts sorted.
fn counts(path: &str, source: &str) -> BTreeMap<String, Vec<Option<u32>>> {
    let mut out: BTreeMap<String, Vec<Option<u32>>> = BTreeMap::new();
    for r in facts(path, source).refs.into_iter().filter(|r| r.kind == EdgeKind::Calls) {
        out.entry(r.target).or_default().push(r.arg_count);
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

fn map<V: Clone>(entries: &[(&str, V)]) -> BTreeMap<String, V> {
    entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

const RUST: &str = r#"
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
pub struct A;
impl A {
    pub fn by_value(self) {}
    pub fn by_mut_value(mut self) {}
    pub fn by_ref(&self, x: i32) {}
    pub fn by_mut_ref(&mut self) {}
    pub fn by_lifetime<'a>(&'a mut self) {}
    pub fn by_lifetime_ref<'a>(&'a self) {}
    pub fn rc(self: Rc<Self>) {}
    pub fn arc(self: Arc<Self>, z: u8) {}
    pub fn boxed(self: Box<Self>) {}
    pub fn pinned(self: Pin<&mut Self>, y: u8) {}
    pub fn new() -> Self { A }
    pub fn assoc(a: i32, #[allow(unused)] b: i32, cb: fn(i32, i32) -> i32) {}
}
pub trait T {
    fn t(&self, a: i32) {}
}
pub trait Named {
    fn name(&self);
    fn make() -> Self;
}
impl Named for A {
    fn name(&self) {}
    fn make() -> Self { A }
}
pub fn free(a: i32, b: i32) {}
pub fn caller(a: A) {
    free(1, 2);
    A::new();
    a.by_ref(3);
    m!(free(1, 2));
    m!(free(1, 2, 3,));
    m!(free(|x, y| x + y, 2));
    commented(1, /* c */ 2);
    attributed(#[cfg(x)] 1, 2);
}
"#;

/// Every Rust parameter is required and a receiver is not counted: each `self`
/// form leaves the range of the parameters after it, and an attribute or a
/// function-typed parameter's own list adds nothing.
#[test]
fn rust_ranges_skip_the_receiver() {
    assert_eq!(
        ranges("src/lib.rs", RUST),
        map(&[
            ("arc", range(1, Some(1))),
            ("assoc", range(3, Some(3))),
            ("boxed", range(0, Some(0))),
            ("by_lifetime", range(0, Some(0))),
            ("by_lifetime_ref", range(0, Some(0))),
            ("by_mut_ref", range(0, Some(0))),
            ("by_mut_value", range(0, Some(0))),
            ("by_ref", range(1, Some(1))),
            ("by_value", range(0, Some(0))),
            ("caller", range(1, Some(1))),
            ("free", range(2, Some(2))),
            ("make", range(0, Some(0))),
            ("name", range(0, Some(0))),
            ("new", range(0, Some(0))),
            ("pinned", range(1, Some(1))),
            ("rc", range(0, Some(0))),
            ("t", range(1, Some(1))),
        ])
    );
}

/// CR-200: every `self` form — `self`, `mut self`, `&self`, `&mut self`,
/// `&'a self`, `&'a mut self`, `self: Box<Self>`, `self: Rc<Self>`,
/// `self: Arc<Self>`, `self: Pin<&mut Self>` — takes `self`, in an inherent
/// impl and in a trait impl alike; `fn new() -> Self`, a trait impl's
/// `fn make() -> Self` and an associated function with parameters do not; a
/// free function and a trait's own default method record nothing.
#[test]
fn a_rust_impl_function_records_whether_it_takes_self() {
    let takes_self: BTreeMap<String, Option<bool>> = facts("src/lib.rs", RUST)
        .nodes
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name, n.takes_self))
        .collect();
    assert_eq!(
        takes_self,
        map(&[
            ("arc", Some(true)),
            ("assoc", Some(false)),
            ("boxed", Some(true)),
            ("by_lifetime", Some(true)),
            ("by_lifetime_ref", Some(true)),
            ("by_mut_ref", Some(true)),
            ("by_mut_value", Some(true)),
            ("by_ref", Some(true)),
            ("by_value", Some(true)),
            ("caller", None),
            ("free", None),
            ("make", Some(false)),
            ("name", Some(true)),
            ("new", Some(false)),
            ("pinned", Some(true)),
            ("rc", Some(true)),
            ("t", None),
        ])
    );
}

/// The `self` forms beyond [`RUST`]'s, as S-604 binds on them: a typed
/// `self` by value or reference, `mut self` with a type, an attributed
/// receiver, `const`/`async`/`unsafe` qualifiers, a generic or path-qualified
/// impl, a `where` clause and an impl nested in a function or module all
/// record the fact the receiver decides; an associated function stays false
/// whatever its qualifiers.
#[test]
fn every_rust_self_form_records_taking_self_and_an_associated_function_does_not() {
    const FORMS: &str = r#"
pub struct W<T>(T);
impl<T: Clone> W<T> {
    pub fn typed_value(self: Self) {}
    pub fn typed_ref(self: &Self) {}
    pub fn typed_mut_ref(self: &mut Self, x: u8) {}
    pub fn mut_boxed(mut self: Box<Self>) {}
    pub fn attributed(#[allow(unused)] &self) {}
    pub const fn const_ref(&self) {}
    pub async fn async_ref(&self) {}
    pub unsafe fn unsafe_mut(&mut self) {}
    pub fn bounded<U>(&self, u: U) where U: Clone {}
    pub const fn const_new(t: T) -> Self { W(t) }
    pub async fn async_make() -> Option<Self> { None }
    pub fn generic_assoc<U: Into<T>>(u: U) -> Self { W(u.into()) }
}
pub mod inner {
    pub struct V;
    impl crate::inner::V {
        pub fn path_ref(&self) {}
        pub fn path_new() -> Self { V }
    }
}
pub fn outer() {
    struct L;
    impl L {
        fn nested_ref(&self) {}
        fn nested_new() -> Self { L }
    }
}
"#;
    let takes_self: BTreeMap<String, Option<bool>> = facts("src/lib.rs", FORMS)
        .nodes
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name, n.takes_self))
        .collect();
    assert_eq!(
        takes_self,
        map(&[
            ("async_make", Some(false)),
            ("async_ref", Some(true)),
            ("attributed", Some(true)),
            ("bounded", Some(true)),
            ("const_new", Some(false)),
            ("const_ref", Some(true)),
            ("generic_assoc", Some(false)),
            ("mut_boxed", Some(true)),
            ("nested_new", Some(false)),
            ("nested_ref", Some(true)),
            ("outer", None),
            ("path_new", Some(false)),
            ("path_ref", Some(true)),
            ("typed_mut_ref", Some(true)),
            ("typed_ref", Some(true)),
            ("typed_value", Some(true)),
            ("unsafe_mut", Some(true)),
        ])
    );
}

/// No non-callable node records a range or the takes-`self` fact.
#[test]
fn a_non_callable_records_no_arity_fact() {
    for n in facts("src/lib.rs", RUST).nodes {
        if !matches!(n.kind, NodeKind::Function | NodeKind::Method) {
            assert_eq!((n.params, n.takes_self), (None, None), "{} ({:?})", n.name, n.kind);
        }
    }
}

/// A Rust call counts its arguments and a proven receiver's retyped row keeps
/// the count. A call inside a macro's token tree counts its top-level commas,
/// so it records the row the same call records outside one; one whose tokens
/// may hide a comma (a closure's `|x, y|`) is unknown.
#[test]
fn rust_calls_count_their_arguments() {
    assert_eq!(
        counts("src/lib.rs", RUST),
        map(&[
            ("A::by_ref", vec![Some(1)]),
            ("A::new", vec![Some(0)]),
            // A comment and an attribute in an argument list are no arguments.
            ("attributed", vec![Some(2)]),
            ("commented", vec![Some(2)]),
            ("free", vec![None, Some(2), Some(3)]),
        ])
    );
}

/// Go: `b, c string` names two parameters; `...int` is variadic; a method's
/// receiver is not counted; `xs...` spreads.
#[test]
fn go_ranges_and_counts() {
    let src = "package p\n\
        type S struct{}\n\
        func f(a int, b, c string, d ...int) {}\n\
        func (s *S) m(a int) {}\n\
        func z() {}\n\
        func caller(s *S, xs []int) { f(1, \"a\", \"b\"); f(1, \"a\", \"b\", xs...); s.m(1); z() }\n";
    assert_eq!(
        ranges("p/a.go", src),
        map(&[
            ("caller", range(2, Some(2))),
            ("f", range(3, None)),
            ("m", range(1, Some(1))),
            ("z", range(0, Some(0))),
        ])
    );
    assert_eq!(
        counts("p/a.go", src),
        map(&[("f", vec![None, Some(3)]), ("m", vec![Some(1)]), ("z", vec![Some(0)])])
    );
}

/// Java: `T...` is variadic and a receiver parameter is not counted.
#[test]
fn java_ranges_and_counts() {
    let src = "class C {\n\
        void f(int a, String... b) { g(1, 2); this.m(1); super.m(); }\n\
        void m(int a) {}\n\
        void r(C this, int a) {}\n\
        void g(int a, int b) {}\n\
        }\n";
    assert_eq!(
        ranges("src/main/java/C.java", src),
        map(&[
            ("f", range(1, None)),
            ("g", range(2, Some(2))),
            ("m", range(1, Some(1))),
            ("r", range(1, Some(1))),
        ])
    );
    assert_eq!(
        counts("src/main/java/C.java", src),
        // The own-class calls are retyped to `C::…` (S-467) and keep their
        // count; `super.m()` stays a Method row.
        map(&[("C::g", vec![Some(2)]), ("C::m", vec![Some(1)]), ("m", vec![Some(0)])])
    );
}

/// Kotlin: a default raises only the maximum, `vararg` makes it unbounded, an
/// extension's receiver is not a parameter; a trailing lambda is one argument
/// and `*arr` spreads.
#[test]
fn kotlin_ranges_and_counts() {
    let src = "class C {\n\
        fun f(a: Int, b: Int = 2, vararg c: Int) {}\n\
        fun d(a: Int, b: Int = 2) {}\n\
        fun caller(arr: IntArray) { d(1); d(1, 2); h { x -> x }; k(1) { it }; f(1, *arr); n(a = 1) }\n\
        }\n\
        fun Int.ext(a: Int) = a\n";
    assert_eq!(
        ranges("src/C.kt", src),
        map(&[
            ("caller", range(1, Some(1))),
            ("d", range(1, Some(2))),
            ("ext", range(1, Some(1))),
            ("f", range(1, None)),
        ])
    );
    assert_eq!(
        counts("src/C.kt", src),
        map(&[
            ("d", vec![Some(1), Some(2)]),
            ("f", vec![None]),
            ("h", vec![Some(1)]),
            ("k", vec![Some(2)]),
            ("n", vec![Some(1)]),
        ])
    );
}

/// Scala: a declaration is admitted by its first list; a `using` list and a
/// parameterless `def` are unknown; a call counts its first argument list, a
/// block list is one argument, and `xs: _*` / `xs*` spread.
#[test]
fn scala_ranges_and_counts() {
    let src = "class C {\n\
        def f(a: Int, b: Int = 2, c: Int*)(d: Int): Int = 1\n\
        def q(): Int = 1\n\
        def d(a: Int, b: Int = 2): Int = 1\n\
        def p: Int = 1\n\
        def r(using x: Int): Int = 1\n\
        def i(implicit x: Int): Int = 1\n\
        def caller(xs: Seq[Int]): Unit = { g(1, 2); h { x => x }; k(1)(2); m(xs: _*); n(1) { 2 }; o(xs*) }\n\
        }\n";
    assert_eq!(
        ranges("src/C.scala", src),
        map(&[
            ("caller", range(1, Some(1))),
            ("d", range(1, Some(2))),
            ("f", range(1, None)),
            ("i", None),
            ("p", None),
            ("q", range(0, Some(0))),
            ("r", None),
        ])
    );
    assert_eq!(
        counts("src/C.scala", src),
        map(&[
            ("g", vec![Some(2)]),
            ("h", vec![Some(1)]),
            ("k", vec![Some(1)]),
            ("m", vec![None]),
            ("n", vec![Some(1)]),
            ("o", vec![None]),
        ])
    );
}

/// C#: a default is optional, `params` is variadic, an extension method is
/// unknown; a named argument counts.
#[test]
fn csharp_ranges_and_counts() {
    let src = "class C {\n\
        void F(int a, int b = 2, params int[] c) { G(1, 2); H(x: 1); this.K(); }\n\
        static void E(this int s, int a) {}\n\
        void G(int a, int b) {}\n\
        void D(int a, int b = 2) {}\n\
        }\n";
    assert_eq!(
        ranges("src/C.cs", src),
        map(&[("D", range(1, Some(2))), ("E", None), ("F", range(1, None)), ("G", range(2, Some(2)))])
    );
    assert_eq!(
        counts("src/C.cs", src),
        map(&[("G", vec![Some(2)]), ("H", vec![Some(1)]), ("K", vec![Some(0)])])
    );
}

/// C++: a default is optional, `...` and a pack are variadic, `(void)` and `()`
/// declare none; a pack expansion spreads. A definition outside a class body
/// records no range — its defaults may sit on a separate declaration — while a
/// prototype and an in-class definition do.
#[test]
fn cpp_ranges_and_counts() {
    let src = "class K {\n\
        int f(int a, int b = 2, ...) { g(1, 2); return 0; }\n\
        template<typename... Ts> void v(Ts... xs) { w(xs...); }\n\
        int z(void) { return 0; }\n\
        int y() { return 0; }\n\
        int d(int a, int b = 2) { return a; }\n\
        };\n\
        int out(int a, int b = 2);\n\
        int out(int a, int b) { return a; }\n";
    let ranges: Vec<(String, Option<ParamRange>)> = facts("src/a.cpp", src)
        .nodes
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name, n.params))
        .collect();
    assert_eq!(
        ranges,
        vec![
            ("f".to_string(), range(1, None)),
            ("v".to_string(), range(0, None)),
            ("z".to_string(), range(0, Some(0))),
            ("y".to_string(), range(0, Some(0))),
            ("d".to_string(), range(1, Some(2))),
            ("out".to_string(), range(1, Some(2))),
            ("out".to_string(), None),
        ],
        "the prototype carries the default; the out-of-class definition records no range"
    );
    assert_eq!(counts("src/a.cpp", src), map(&[("g", vec![Some(2)]), ("w", vec![None])]));
}

/// C: `...` is variadic, `(void)` declares none, and `()` declares unspecified
/// parameters — unknown.
#[test]
fn c_ranges_and_counts() {
    let src = "int f(int a, int b, ...) { g(1, 2); return 0; }\n\
        int z(void) { return 0; }\n\
        int y() { return 0; }\n";
    assert_eq!(
        ranges("src/a.c", src),
        map(&[("f", range(2, None)), ("y", None), ("z", range(0, Some(0)))])
    );
    assert_eq!(counts("src/a.c", src), map(&[("g", vec![Some(2)])]));
}

/// Python: defaults are optional, keyword-only parameters without one are
/// required, `*args`/`**kw` are variadic, the `*`/`/` separators are none, and
/// a method's `self`/`cls` is its receiver unless it is a `@staticmethod`, and
/// a method defined under an `if`/`try` of its class records unknown; `*xs`
/// spreads and a lone generator is one argument.
#[test]
fn python_ranges_and_counts() {
    let src = "def f(a, b=2, *args, c, d=1, **kw):\n    g(1, 2, *xs); h(x=1); g(1)\n\
        class C:\n    def m(self, a, /, b: int = 1):\n        self.m(1)\n\
        \x20   @staticmethod\n    def s(a): pass\n\
        \x20   @classmethod\n    def c(cls, a): pass\n\
        def k(*, a): pass\n\
        def gen(): sum(x for x in y)\n\
        class K:\n    if v:\n        def cond(self, x): pass\n    try:\n        @dec\n        def tri(self, y): pass\n\
        \x20   except E:\n        pass\n\
        def m2(*args: int): pass\n\
        class V:\n    def var(*args: int): pass\n\
        def kwonly(a, **kw): pass\n\
        def calls(): g2(**kw); g3(1, **kw)\n\
        class D:\n    @cache\n    @staticmethod\n    def s1(a): pass\n\
        \x20   @staticmethod\n    @cache\n    def s2(a): pass\n";
    assert_eq!(
        ranges("pkg/a.py", src),
        map(&[
            ("c", range(1, Some(1))),
            ("calls", range(0, Some(0))),
            ("f", range(2, None)),
            // A method under a compound statement of a class body: whether it
            // takes `self` is not visible to a query, so unknown, never `self`
            // counted.
            ("cond", None),
            ("gen", range(0, Some(0))),
            ("k", range(1, Some(1))),
            ("kwonly", range(1, None)),
            ("m", range(1, Some(2))),
            ("m2", range(0, None)),
            ("s", range(1, Some(1))),
            // `@staticmethod` among other decorators, in either order.
            ("s1", range(1, Some(1))),
            ("s2", range(1, Some(1))),
            ("tri", None),
            // A method whose first parameter is `*args` receives `self` in it:
            // variadic, never a lone receiver.
            ("var", range(0, None)),
        ])
    );
    let counts = counts("pkg/a.py", src);
    assert_eq!(counts["g"], vec![None, Some(1)]);
    assert_eq!(counts["h"], vec![Some(1)]);
    assert_eq!(counts["m"], vec![Some(1)]);
    assert_eq!(counts["sum"], vec![Some(1)]);
    assert_eq!((&counts["g2"], &counts["g3"]), (&vec![None], &vec![None]), "`**kw` spreads");
}

/// A comment opening a Python method's parameter list (`def update(  # type:
/// ignore[override]`) does not hide its receiver: `self` is still not counted
/// (S-592; werkzeug's `MultiDict.update` read `[2, 2]` before).
#[test]
fn a_python_receiver_behind_a_comment_is_still_the_receiver() {
    let src = "class C:\n    def m(  # type: ignore[override]\n        self,\n        a,\n    ):\n        pass\n\n\
        \x20   @dec\n    def d(  # note\n        self, a, b=1\n    ):\n        pass\n\n\
        \x20   @staticmethod\n    def s(  # note\n        a\n    ):\n        pass\n";
    assert_eq!(
        ranges("pkg/a.py", src),
        map(&[("d", range(1, Some(2))), ("m", range(1, Some(1))), ("s", range(1, Some(1)))])
    );
}

/// A Kotlin `override fun` or Scala `override def` inherits the default values
/// of what it overrides, which its own list never writes (S-592; koin's
/// `SingleInstanceFactory.drop(scope: Scope?)` read `[1, 1]` under
/// `abstract fun drop(scope: Scope? = null)`): its range is unknown.
#[test]
fn an_override_inherits_defaults_so_its_range_is_unknown() {
    let kt = "abstract class B {\n    abstract fun drop(scope: Int? = null)\n}\n\
        class C : B() {\n    override fun drop(scope: Int?) {}\n    fun keep(a: Int) {}\n}\n";
    assert_eq!(ranges("src/a.kt", kt), map(&[("drop", None), ("keep", range(1, Some(1)))]));
    let scala = "class C extends B {\n  override def drop(scope: Int): Unit = ()\n  \
        def keep(a: Int): Unit = ()\n}\n\
        abstract class D extends B {\n  override def shed(scope: Int): Unit\n  def hold(a: Int): Unit\n}\n";
    assert_eq!(
        ranges("src/a.scala", scala),
        map(&[("drop", None), ("hold", range(1, Some(1))), ("keep", range(1, Some(1))), ("shed", None)]),
        "an abstract `override def` declaration too"
    );
}

/// PHP's first-class callable `f(...)` (S-592) makes a closure and passes no
/// argument, so its count is unknown, not one.
#[test]
fn a_php_first_class_callable_records_an_unknown_argument_count() {
    let src = "<?php\nclass C { public function h($a, $b) {}\n\
        public function r() { set_error_handler($this->h(...)); $this->h(1, 2); } }\n";
    let counts = counts("src/a.php", src);
    assert_eq!(counts["h"], vec![None, Some(2)]);
}

/// Python's `cls.m(…)` (S-592): it may call a class method, whose `cls` is
/// implicit, or an instance method, to which it passes the instance itself —
/// so its count is unknown, never the explicit instance counted against `m`'s
/// range. `self.m(…)` and any other receiver's call count as before.
#[test]
fn a_python_cls_call_records_an_unknown_argument_count() {
    let src = "class C:\n    def m(self, a):\n        pass\n\
        \x20   @classmethod\n    def c(cls):\n        cls.m(None, 1); cls.k(1); self.n(1); obj.p(1); clsx.q(1)\n";
    let counts = counts("pkg/a.py", src);
    assert_eq!(counts["m"], vec![None], "`cls.m(instance, a)`");
    assert_eq!(counts["k"], vec![None], "`cls.k(a)`");
    assert_eq!(counts["n"], vec![Some(1)], "`self.n(a)` counts its argument");
    assert_eq!(counts["p"], vec![Some(1)]);
    assert_eq!(counts["q"], vec![Some(1)], "only the exact name `cls`");
}

/// PHP: the required parameters (a promoted constructor's too) set the
/// minimum, and the maximum is unbounded — a user function accepts surplus
/// arguments (`func_get_args()`); `...$xs` spreads.
#[test]
fn php_ranges_and_counts() {
    let src = "<?php\n\
        function f($a, $b = 2, ...$c) { g(1, 2); h(...$xs); }\n\
        class C { public function m(int $a, ?int $b = null) { $this->m(1); }\n\
        public function __construct(private int $x = 1) {} }\n";
    assert_eq!(
        ranges("src/a.php", src),
        map(&[
            ("__construct", range(0, None)),
            ("f", range(1, None)),
            ("m", range(1, None)),
        ])
    );
    let counts = counts("src/a.php", src);
    assert_eq!(counts["g"], vec![Some(2)]);
    assert_eq!(counts["h"], vec![None]);
    assert_eq!(counts["m"], vec![Some(1)]);
}

const TS: &str = "function f(a: number, b = 2, c?: number, ...d: number[]) { g(1, 2); h(...xs); this.m(1); }\n\
    class C { m(this: C, a: number) {} }\n\
    const k = (a, b) => a;\n\
    const one = x => x;\n\
    const l = function (a) {};\n\
    function d(a: number, b = 2, c?: number) {}\n\
    function tagged() { tag`x`(1); }\n";

/// TypeScript (and TSX): `= v` and `?` are optional, `...d` variadic, a `this`
/// parameter the receiver, a lone arrow parameter its own list; `...xs`
/// spreads.
#[test]
fn typescript_ranges_and_counts() {
    for path in ["src/a.ts", "src/a.tsx"] {
        assert_eq!(
            ranges(path, TS),
            map(&[
                ("d", range(1, Some(3))),
                ("f", range(1, None)),
                ("k", range(2, Some(2))),
                ("l", range(1, Some(1))),
                ("m", range(1, Some(1))),
                ("one", range(1, Some(1))),
                ("tagged", range(0, Some(0))),
            ]),
            "{path}"
        );
        let counts = counts(path, TS);
        assert_eq!(counts["g"], vec![Some(2)], "{path}");
        assert_eq!(counts["h"], vec![None], "{path}");
        assert_eq!(counts["m"], vec![Some(1)], "{path}");
        // The tagged template is the callee of a further call: never that
        // call's count.
        assert_eq!(counts["tag"], vec![None], "{path}: a tagged template is not counted");
    }
}

/// Ruby: a default is optional, a keyword without one required, `*c`/`**f`
/// variadic, `&g` none; `*xs` and a bare `key:` run make a count unknown, `x.n`
/// passes none, and the constant a call is made on is no callee of it.
#[test]
fn ruby_ranges_and_counts() {
    let src = "def f(a, b = 2, *c, d:, e: 1, **f, &g)\n  g(1, 2); h(*xs); m 1, 2; x.n; User.find(1); cfg(1, timeout: 1, retries: 2)\nend\n\
        class C\n  def m(a)\n    self.m(1)\n  end\nend\n\
        def kw(a, **o)\n  gb(1, &blk); hs(**opts)\nend\n\
        def sp(a, *r); end\n\
        def fw(...)\n  fa(...)\nend\n";
    assert_eq!(
        ranges("lib/a.rb", src),
        map(&[
            ("f", range(2, None)),
            ("fw", range(0, None)),
            ("kw", range(1, None)),
            ("m", range(1, Some(1))),
            ("sp", range(1, None)),
        ])
    );
    let counts = counts("lib/a.rb", src);
    assert_eq!(counts["g"], vec![Some(2)]);
    assert_eq!(counts["h"], vec![None]);
    assert_eq!(counts["m"], vec![Some(1), Some(2)]);
    assert_eq!(counts["n"], vec![Some(0)]);
    assert_eq!(counts["cfg"], vec![None], "a bare `key:` run is one hash or several keywords");
    assert_eq!(counts["User"], vec![None], "the receiver row is no callee");
    assert_eq!(counts["gb"], vec![Some(1)], "a `&block` is no argument");
    assert_eq!((&counts["hs"], &counts["fa"]), (&vec![None], &vec![None]), "`**h` and `...` spread");
}

/// Only a callable records a range: a droppable Java override that captures a
/// `record`'s component list still leaves the record's class node unknown.
#[test]
fn a_list_a_non_callable_owns_records_no_range() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join(".logos/plugins/java/queries");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("symbols.scm"),
        "(record_declaration name: (identifier) @symbol.class)\n\
         (record_declaration parameters: (formal_parameters) @arity.parameters)\n\
         (formal_parameters (formal_parameter) @arity.required)\n",
    )
    .unwrap();
    let registry = LanguageRegistry::load(tmp.path()).expect("override loads");
    let plugin = registry.for_extension("java").expect("java grammar");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let facts = extract(&FileInput::new("src/R.java", "record R(int a, int b) {}\n"), plugin, &ctx);
    let record = facts.nodes.iter().find(|n| n.kind == NodeKind::Class).expect("the record's class node");
    assert_eq!((record.name.as_str(), record.params), ("R", None));
}

/// A list child no capture covers makes the range unknown rather than
/// miscounted: a Rust `fn` with a parameter the query does not classify (here a
/// droppable override that captures the list and no parameter).
#[test]
fn an_uncovered_parameter_makes_the_range_unknown() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join(".logos/plugins/rust/queries");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("symbols.scm"),
        "(function_item name: (identifier) @symbol.function)\n\
         (function_item parameters: (parameters) @arity.parameters)\n",
    )
    .unwrap();
    let registry = LanguageRegistry::load(tmp.path()).expect("override loads");
    let plugin = registry.for_extension("rs").expect("rust grammar");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let facts = extract(&FileInput::new("src/lib.rs", "fn a(x: i32) {}\nfn b() {}\n"), plugin, &ctx);
    let ranges: BTreeMap<String, Option<ParamRange>> = facts.nodes.into_iter().map(|n| (n.name, n.params)).collect();
    assert_eq!(ranges["a"], None, "an uncovered parameter is never silently dropped");
    assert_eq!(ranges["b"], range(0, Some(0)), "an empty list is countable");
}

/// A parameter list inside a declaration's body belongs to no outer
/// declaration: here a droppable override captures only the lists of
/// function types written in `let` statements, so `outer`'s one captured list
/// sits in its body and `outer` records no range rather than that type's.
#[test]
fn a_list_inside_a_body_is_not_the_declarations_own() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join(".logos/plugins/rust/queries");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("symbols.scm"),
        "(function_item name: (identifier) @symbol.function)\n\
         (let_declaration type: (function_type parameters: (parameters) @arity.parameters))\n\
         (parameters (primitive_type) @arity.required)\n",
    )
    .unwrap();
    let registry = LanguageRegistry::load(tmp.path()).expect("override loads");
    let plugin = registry.for_extension("rs").expect("rust grammar");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    let src = "fn outer() { let f: fn(i32, i32) = g; }\n";
    let facts = extract(&FileInput::new("src/lib.rs", src), plugin, &ctx);
    let outer = facts.nodes.iter().find(|n| n.name == "outer").expect("outer");
    assert_eq!(outer.params, None, "a list in the body is never the declaration's own");
}

/// Re-extraction is deterministic: the same source yields the same facts.
#[test]
fn re_extraction_is_deterministic() {
    for (path, src) in [("src/lib.rs", RUST), ("src/a.ts", TS)] {
        assert_eq!(facts(path, src), facts(path, src), "{path}");
    }
}
