//! Unit tests for Rust receiver typing (S-587, [CR-188], [FR-RS-42]).
//!
//! In their own file, the `declared_types_tests.rs` shape. One fixture per proof
//! form, one per non-proof, and one per wrapper, each read off the rows the real
//! Rust plugin's `references` query records. A proven `x.f()` is the Path-form
//! `T::f` of shape `other`, with the wrappers peeled to reach `T`; everything
//! else keeps the `other` Method row it recorded before.
//!
//! [CR-188]: ../../../docs/requests/CR-188-rust-receiver-typing.md
//! [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md

use crate::extract::{extract, Facts, FileInput, SymbolContext};
use crate::model::{EdgeKind, ReceiverShape, RefForm};
use crate::plugin::LanguageRegistry;

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    })
}

fn extract_rust(source: &str) -> Facts {
    let plugin = registry().for_extension("rs").expect("rust grammar");
    let ctx = SymbolContext::cargo("logos-core", "0.1.0");
    extract(&FileInput::new("src/lib.rs", source), plugin, &ctx)
}

/// One `Calls` row: `(target, form, receiver, peeled)`.
type Row = (String, RefForm, Option<ReceiverShape>, Option<String>);

/// The distinct `Calls` rows whose target is `f` or ends in `::f` — the calls
/// of `f` the fixture's callers make — sorted. Distinct, so a fixture that
/// repeats one shape in several callers reads as that shape once.
fn calls_of_f(source: &str) -> Vec<Row> {
    let key = |r: &Row| (r.0.clone(), r.1.as_i32(), r.2.map(ReceiverShape::as_i32), r.3.clone());
    let mut rows: Vec<Row> = extract_rust(source)
        .refs
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && (r.target == "f" || r.target.ends_with("::f")))
        .map(|r| (r.target, r.form, r.receiver, r.peeled))
        .collect();
    rows.sort_by_key(key);
    rows.dedup_by_key(|r| key(r));
    rows
}

/// The retyped row `T::f`, peeling `peeled`.
fn typed(head: &str, peeled: Option<&str>) -> Row {
    (format!("{head}::f"), RefForm::Path, Some(ReceiverShape::Other), peeled.map(str::to_string))
}

/// The `other` Method row an unproven `x.f()` keeps.
fn other() -> Row {
    ("f".to_string(), RefForm::Method, Some(ReceiverShape::Other), None)
}

/// Two types that both define `f` — every fixture's receiver is one of them.
const TYPES: &str = "\
pub struct A { n: u32 }
pub struct B;
impl A { pub fn f(&self) {} }
impl B { pub fn f(&self) {} }
";

fn with_types(body: &str) -> String {
    format!("{TYPES}{body}")
}

// ── The five proof forms ─────────────────────────────────────────────────────

#[test]
fn a_typed_parameter_proves_its_receiver() {
    assert_eq!(calls_of_f(&with_types("fn g(x: A) { x.f(); }")), vec![typed("A", None)]);
}

#[test]
fn a_typed_let_proves_its_receiver() {
    assert_eq!(calls_of_f(&with_types("fn g() { let x: B = make(); x.f(); }")), vec![typed("B", None)]);
}

#[test]
fn a_constructor_whose_declared_return_is_self_or_the_type_proves_its_receiver() {
    let src = "\
pub struct A;
pub struct B;
impl A { pub fn new() -> Self { A } pub fn f(&self) {} }
impl B { pub fn make(n: u32) -> B { B } pub fn f(&self) {} }
fn g() { let x = A::new(); x.f(); }
fn h() { let y = B::make(1); y.f(); }
";
    assert_eq!(calls_of_f(src), vec![typed("A", None), typed("B", None)]);
}

#[test]
fn a_constructor_returning_anything_but_the_type_proves_nothing() {
    // `Result<Self, E>`, `()` and another type are no proof; neither is a type
    // whose `new` the file never declares, nor a `new` declared on two impls
    // that disagree.
    let src = "\
pub struct A;
pub struct B;
pub struct C;
pub struct D;
impl A { pub fn new() -> Result<Self, String> { Ok(A) } pub fn f(&self) {} }
impl B { pub fn new() {} pub fn f(&self) {} }
impl C { pub fn new() -> A { A } pub fn f(&self) {} }
impl D { pub fn new() -> Self { D } }
impl Default for D { fn default() -> Self { D } }
impl D { pub fn new() -> Box<Self> { Box::new(D) } }
fn g() { let a = A::new(); a.f(); let b = B::new(); b.f(); let c = C::new(); c.f(); }
fn h() { let e = Elsewhere::new(); e.f(); let d = D::new(); d.f(); }
";
    assert_eq!(calls_of_f(src), vec![other()]);
}

#[test]
fn a_struct_literal_proves_its_receiver() {
    assert_eq!(calls_of_f(&with_types("fn g() { let x = A { n: 1 }; x.f(); }")), vec![typed("A", None)]);
}

#[test]
fn an_own_field_proves_its_receiver_through_the_callers_self_type() {
    let src = "\
pub struct A;
impl A { pub fn f(&self) {} }
pub struct Holder { inner: A, shared: std::sync::Arc<A> }
impl Holder {
    fn g(&self) { self.inner.f(); }
    fn h(&self) { self.shared.f(); }
}
";
    assert_eq!(calls_of_f(src), vec![typed("A", None), typed("A", Some("Arc"))]);
}

#[test]
fn a_field_of_another_struct_or_of_no_struct_the_file_declares_proves_nothing() {
    // `self.inner` in `Other`'s impl names `Other`'s field, which the file never
    // declares; a trait's default body has no self type at all.
    let src = "\
pub struct A;
impl A { pub fn f(&self) {} }
pub struct Holder { inner: A }
impl Other { fn g(&self) { self.inner.f(); } }
trait T { fn h(&self) { self.inner.f(); } }
";
    assert_eq!(calls_of_f(src), vec![other()]);
}

#[test]
fn self_names_the_callers_own_type() {
    let src = "\
pub struct A;
impl A {
    pub fn f(&self) {}
    fn merge(&self, other: &Self) { other.f(); }
    fn make() -> Self { let x = Self::make(); x.f(); A }
}
trait T { fn g(&self, other: &Self) { other.f(); } }
";
    assert_eq!(calls_of_f(src), vec![typed("A", None), typed("A", Some("&")), other()]);
}

// ── Non-proofs ───────────────────────────────────────────────────────────────

#[test]
fn a_shadowed_name_proves_nothing() {
    // A typed `let` shadowed by an untyped one: two bindings in scope.
    assert_eq!(
        calls_of_f(&with_types("fn g() { let x: A = make(); let x = convert(x); x.f(); }")),
        vec![other()]
    );
}

#[test]
fn a_re_bound_name_proves_nothing() {
    // A typed parameter re-bound by a pattern in scope at the call.
    let src = with_types(
        "fn g(x: A, xs: Vec<A>) { for x in xs { x.f(); } }\n\
         fn h(x: A, o: Option<A>) { if let Some(x) = o { x.f(); } }\n\
         fn k(x: A, o: Option<B>) { match o { Some(x) => x.f(), None => {} } }\n",
    );
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn a_two_typed_name_proves_nothing() {
    assert_eq!(
        calls_of_f(&with_types("fn g() { let x: A = make(); let x: B = make(); x.f(); }")),
        vec![other()]
    );
}

#[test]
fn a_generic_parameter_proves_nothing() {
    let src = with_types(
        "fn g<G: Tr>(x: G) { x.f(); }\n\
         fn h<G: Tr>(x: &G) { x.f(); }\n\
         pub struct S<T> { t: T }\n\
         impl<T: Tr> S<T> { fn k(&self, x: T) { x.f(); self.t.f(); } }\n",
    );
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn an_impl_trait_or_a_dyn_receiver_proves_nothing_here() {
    // `impl Trait` names no type; `dyn Trait` is FR-RS-08's dispatch row.
    let src = with_types("fn g(x: impl Tr) { x.f(); }\nfn h(x: &dyn Tr) { x.f(); }\n");
    assert_eq!(
        calls_of_f(&src),
        vec![("Tr::f".to_string(), RefForm::Method, None, None), other()]
    );
}

#[test]
fn a_chain_result_proves_nothing() {
    assert_eq!(calls_of_f(&with_types("fn g(a: A) { a.b().f(); }")), vec![other()]);
}

#[test]
fn a_name_bound_only_where_the_call_cannot_see_it_proves_nothing() {
    // A `let` in a sibling block, a closure's parameter outside the closure,
    // and a `let` after the call: none is in scope at the call.
    let src = with_types(
        "fn g() { { let x: A = make(); } x.f(); }\n\
         fn h() { let c = |x: A| 0; x.f(); }\n\
         fn k() { x.f(); let x: A = make(); }\n",
    );
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn a_binding_out_of_scope_at_the_call_does_not_shadow_the_one_in_scope() {
    // The sibling block's `x` and the closure's `x` are not in scope at the
    // call; the parameter is the one binding there. A `let`'s own initializer
    // still sees the binding before it.
    let src = with_types(
        "fn g(x: A) { { let x: B = make(); } let c = |x: B| 0; x.f(); }\n\
         fn h(x: B) { let x = x.f(); }\n",
    );
    assert_eq!(calls_of_f(&src), vec![typed("A", None), typed("B", None)]);
}

// ── Wrappers ─────────────────────────────────────────────────────────────────

#[test]
fn a_reference_box_arc_or_rc_is_peeled_and_the_wrapper_recorded() {
    // One fixture per form: the deduping `calls_of_f` must never let one
    // form's row stand in for another's.
    for (declared, peeled) in [
        ("&A", "&"),
        ("&mut A", "&mut"),
        ("&'l mut A", "&mut"),
        ("&'l A", "&"),
        ("Box<A>", "Box"),
        ("std::sync::Arc<A>", "Arc"),
        ("Rc<A>", "Rc"),
        ("&Arc<A>", "& Arc"),
    ] {
        let src = with_types(&format!("fn g<'l>(x: {declared}) {{ x.f(); }}"));
        assert_eq!(calls_of_f(&src), vec![typed("A", Some(peeled))], "{declared}");
    }
}

#[test]
fn option_vec_and_mutex_are_not_peeled() {
    // The receiver's type is the wrapper itself, which the binder resolves (or
    // finds external) on its own — never the type inside it.
    let src = with_types(
        "fn a(x: Option<A>) { x.f(); }\n\
         fn b(x: Vec<A>) { x.f(); }\n\
         fn c(x: std::sync::Mutex<A>) { x.f(); }\n\
         fn d(x: Arc<Mutex<A>>) { x.f(); }\n\
         fn e(x: ArcSwap<A>) { x.f(); }\n",
    );
    // `ArcSwap` is one character class from `Arc`, and no wrapper this rule
    // peels.
    assert_eq!(
        calls_of_f(&src),
        vec![
            typed("ArcSwap", None),
            typed("Mutex", Some("Arc")),
            typed("Option", None),
            typed("Vec", None),
            typed("std::sync::Mutex", None),
        ]
    );
}

#[test]
fn a_constructor_returning_a_wrapped_self_records_the_wrapper() {
    let src = "\
pub struct A;
impl A { pub fn shared() -> std::sync::Arc<Self> { todo!() } pub fn f(&self) {} }
fn g() { let x = A::shared(); x.f(); }
";
    assert_eq!(calls_of_f(src), vec![typed("A", Some("Arc"))]);
}

#[test]
fn a_qualified_declared_type_is_written_as_the_file_names_it() {
    let src = "fn g(x: crate::store::A<u8>) { x.f(); }\n";
    assert_eq!(calls_of_f(src), vec![typed("crate::store::A", None)]);
}

#[test]
fn a_type_that_names_no_single_type_proves_nothing() {
    let src = "\
fn a(x: [A; 2]) { x.f(); }
fn b(x: (A, B)) { x.f(); }
fn c(x: *const A) { x.f(); }
fn d(x: fn() -> A) { x.f(); }
fn e(x: &[A]) { x.f(); }
fn g(x: Box<A, Alloc>) { x.f(); }
fn h(x: <A as Tr>::Out) { x.f(); }
fn k(x: Self::Item) { x.f(); }
";
    assert_eq!(calls_of_f(src), vec![("Box::f".to_string(), RefForm::Path, Some(ReceiverShape::Other), None), other()]);
}

#[test]
fn a_written_path_call_and_a_proven_receiver_call_are_two_rows() {
    // `A::f(&x)` is the call a programmer wrote; `x.f()` is a proven receiver.
    // They share a target and differ in shape, so both reach the ledger.
    let src = with_types("fn g(x: A) { A::f(&x); x.f(); }");
    assert_eq!(
        calls_of_f(&src),
        vec![("A::f".to_string(), RefForm::Path, None, None), typed("A", None)]
    );
}

#[test]
fn one_caller_calling_through_a_wrapper_and_through_the_type_records_two_rows() {
    // The peeled wrappers are part of the row's identity: `a.f()` through
    // `Arc<A>` may reach a method the `Arc` provides, `b.f()` cannot.
    let src = with_types("fn g(a: Arc<A>, b: A) { a.f(); b.f(); }");
    assert_eq!(calls_of_f(&src), vec![typed("A", None), typed("A", Some("Arc"))]);
}

#[test]
fn a_qualified_struct_literal_proves_nothing() {
    // `E::V { … }` builds an `E`, not an `E::V`: from the text a variant
    // literal and a module-qualified struct literal are the same shape, so
    // neither proves a type.
    let src = "\
pub enum E { V { n: u32 } }
impl E { pub fn f(&self) {} }
pub mod m { pub struct S; impl S { pub fn f(&self) {} } }
fn g() { let x = E::V { n: 1 }; x.f(); }
fn h() { let y = m::S {}; y.f(); }
";
    assert_eq!(calls_of_f(src), vec![other()]);
}

#[test]
fn a_constructor_proves_only_through_an_impl_of_the_same_name_in_the_callers_module() {
    // Another module's `A`, or a qualified `elsewhere::A`, is not the `A` the
    // local impl declares: neither proves the call's type.
    let qualified = "\
mod support { pub struct A; impl A { pub fn new() -> Self { A } pub fn f(&self) {} } }
fn g() { let x = elsewhere::A::new(); x.f(); }
";
    let imported = "\
use crate::model::A;
mod tests { pub struct A; impl A { pub fn new() -> Self { A } pub fn f(&self) {} } }
fn g() { let x = A::new(); x.f(); }
";
    assert_eq!(calls_of_f(qualified), vec![other()]);
    assert_eq!(calls_of_f(imported), vec![other()]);
}

#[test]
fn an_own_field_or_self_proves_only_through_the_callers_own_module() {
    // `impl S` names the imported `S`, not `tests::S`, whose field type is
    // written relative to `tests`; `impl crate::other::S` is not the `S` this
    // file's scope names, so its `Self` proves nothing either.
    let field = "\
use crate::model::S;
pub struct B;
impl B { pub fn f(&self) {} }
mod tests { pub struct S { pub x: super::B } }
impl S { fn g(&self) { self.x.f(); } }
";
    let self_type = "\
mod inner { pub struct S; }
use inner::S;
impl crate::other::S { fn g(x: &Self) { x.f(); } }
";
    assert_eq!(calls_of_f(field), vec![other()]);
    assert_eq!(calls_of_f(self_type), vec![other()]);
}

#[test]
fn a_constructor_and_a_field_inside_one_inline_module_still_prove() {
    let src = "\
mod m {
    pub struct A;
    impl A { pub fn new() -> Self { A } pub fn f(&self) {} }
    pub struct H { a: A }
    impl H { fn g(&self) { let x = A::new(); x.f(); self.a.f(); } }
}
";
    assert_eq!(calls_of_f(src), vec![typed("A", None)]);
}

#[test]
fn an_if_let_binding_poisons_only_its_own_callable() {
    // An `if let` binding is scoped to its whole callable, and to no other.
    let src = with_types("fn g(x: A) { x.f(); }\nfn h(o: Option<B>) { if let Some(x) = o { let _ = x; } }");
    assert_eq!(calls_of_f(&src), vec![typed("A", None)]);
}

#[test]
fn a_comma_inside_a_nested_argument_list_still_peels_its_wrapper() {
    let src = "fn g(x: Arc<HashMap<K, V>>) { x.f(); }\n";
    assert_eq!(calls_of_f(src), vec![typed("HashMap", Some("Arc"))]);
}

#[test]
fn a_reference_to_a_path_starting_with_mut_is_not_a_mutable_reference() {
    let src = "fn g(x: &mutation::A) { x.f(); }\n";
    assert_eq!(calls_of_f(src), vec![typed("mutation::A", Some("&"))]);
}

#[test]
fn a_struct_declared_twice_in_one_module_proves_no_field() {
    // Not valid Rust, but parsed: two `Holder`s in the caller's module, whose
    // `inner` disagree, prove neither.
    let src = with_types(
        "pub struct Holder { inner: A }\n\
         pub struct Holder { inner: B }\n\
         impl Holder { fn g(&self) { self.inner.f(); } }\n",
    );
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn a_match_guard_is_not_a_binding() {
    // Only an arm's pattern binds; the guard reads `x`, the parameter.
    let src = with_types("fn k(x: A, o: Option<B>) { match o { Some(y) if x.ready() => x.f(), _ => {} } }");
    assert_eq!(calls_of_f(&src), vec![typed("A", None)]);
}

#[test]
fn a_closure_parameter_shadows_the_callers_parameter_inside_the_closure() {
    // Two bindings of `x` are in scope inside the closure: no proof.
    let src = with_types("fn g(x: A) { let c = |x: B| x.f(); }");
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn an_inferred_let_type_proves_nothing() {
    assert_eq!(calls_of_f(&with_types("fn g() { let x: _ = make(); x.f(); }")), vec![other()]);
}

#[test]
fn an_associated_type_of_self_proves_nothing_inside_an_impl() {
    let src = with_types(
        "impl Iterator for A { type Item = B; \
         fn next(&mut self) -> Option<B> { let x: Self::Item = make(); x.f(); None } }\n",
    );
    assert_eq!(calls_of_f(&src), vec![other()]);
}

/// Extraction's peeled wrappers and the Rust plugin's `[wrapper_methods]` table
/// (S-588) name the same wrappers: a wrapper peeled with no table entry would
/// let `x.clone()` through it bind `T::clone`, and a table key no extraction
/// peels never matches a row's `peeled`.
#[test]
fn every_peeled_wrapper_has_a_wrapper_methods_entry_and_no_other_does() {
    let declared = registry().wrapper_methods();
    let mut keys: Vec<&str> = declared["rs"].keys().map(String::as_str).collect();
    let mut peeled = super::PEELED_WRAPPERS.to_vec();
    keys.sort_unstable();
    peeled.sort_unstable();
    assert_eq!(keys, peeled);
}

// ── A call inside a macro (S-610) ────────────────────────────────────────────
//
// A macro's token tree is never parsed as expressions, so the `references`
// query marks no receiver inside one; the token-tree walk hands the receiver
// it reads to the same proof. A fixture's `CALL` is the call, once written
// bare and once as an argument of `format!`, and the two must record one row.

/// The fixture's calls of `f` with `CALL` written outside a macro, and inside.
fn outside_and_inside(template: &str) -> (Vec<Row>, Vec<Row>) {
    let call = |text: &str| calls_of_f(&template.replace("CALL", text));
    (call("x.f();"), call("let _ = format!(\"{:?}\", x.f());"))
}

/// Both spellings record `want`, and only it.
fn assert_parity(template: &str, want: Vec<Row>) {
    let (outside, inside) = outside_and_inside(template);
    assert_eq!(outside, want, "outside a macro: {template}");
    assert_eq!(inside, want, "inside a macro: {template}");
}

#[test]
fn a_typed_parameter_or_let_proves_its_receiver_inside_a_macro() {
    assert_parity(&with_types("fn g(x: A) { CALL }"), vec![typed("A", None)]);
    assert_parity(&with_types("fn g() { let x: B = make(); CALL }"), vec![typed("B", None)]);
    assert_parity(&with_types("fn g(x: &A) { CALL }"), vec![typed("A", Some("&"))]);
    assert_parity(&with_types("fn g(x: Arc<B>) { CALL }"), vec![typed("B", Some("Arc"))]);
}

#[test]
fn a_constructor_or_a_literal_proves_its_receiver_inside_a_macro() {
    let src = "\
pub struct A;
pub struct B { n: u32 }
impl A { pub fn new() -> Self { A } pub fn f(&self) {} }
impl B { pub fn f(&self) {} }
";
    assert_parity(&format!("{src}fn g() {{ let x = A::new(); CALL }}"), vec![typed("A", None)]);
    assert_parity(&format!("{src}fn g() {{ let x = B {{ n: 1 }}; CALL }}"), vec![typed("B", None)]);
}

#[test]
fn an_own_field_and_self_prove_their_receiver_inside_a_macro() {
    let src = "\
pub struct A;
impl A { pub fn f(&self) {} }
pub struct Holder { x: A }
impl Holder { fn g(&self) { CALL } }
";
    let (outside, inside) = (
        calls_of_f(&src.replace("CALL", "self.x.f();")),
        calls_of_f(&src.replace("CALL", "let _ = format!(\"{:?}\", self.x.f());")),
    );
    assert_eq!(outside, vec![typed("A", None)]);
    assert_eq!(inside, outside);
    let selfish = "\
pub struct A;
impl A {
    pub fn f(&self) {}
    fn merge(&self, x: &Self) { CALL }
}
";
    assert_parity(selfish, vec![typed("A", Some("&"))]);
}

#[test]
fn a_receiver_no_proof_form_reads_stays_other_inside_a_macro() {
    // Shadowed, two-typed, generic, inferred, a name no binding is in scope
    // for, and a chain: each stays the bare `other` row it records outside one.
    for body in [
        "fn g(x: A) { let x = convert(x); CALL }",
        "fn g() { let x: A = make(); let x: B = make(); CALL }",
        "fn g<G: Tr>(x: G) { CALL }",
        "fn g() { let x: _ = make(); CALL }",
        "fn g() { CALL }",
        "fn g() { { let x: A = make(); } CALL }",
    ] {
        assert_parity(&with_types(body), vec![other()]);
    }
}

#[test]
fn a_chain_a_path_or_a_field_of_another_value_is_never_a_macro_receiver() {
    let src = with_types("fn g(a: A, p: Holder) { format!(\"{:?}\", a.b().f()); format!(\"{:?}\", p.inner.f()); format!(\"{:?}\", m::a.f()); }");
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn a_macro_call_and_a_plain_call_of_one_receiver_are_one_row() {
    // The same site recorded by the query and by the token-tree walk would be
    // two rows of one shape; they dedup to one.
    let src = with_types("fn g(x: A) { x.f(); format!(\"{:?}\", x.f()); }");
    assert_eq!(calls_of_f(&src), vec![typed("A", None)]);
}

#[test]
fn a_macro_receiver_of_an_untyped_caller_keeps_the_other_shape() {
    // A macro at module level has no caller: nothing to prove, nothing lost.
    let src = with_types("static N: u32 = foo!(x.f());");
    assert_eq!(calls_of_f(&src), vec![other()]);
}

#[test]
fn a_turbofish_method_call_records_no_row_inside_a_macro_or_outside_one() {
    // The query has no pattern for a generic method call, so neither spelling
    // records one: a macro never records a row the same call lacks outside it.
    let outside = calls_of_f(&with_types("fn g(x: A) { x.f::<u8>(); }"));
    let inside = calls_of_f(&with_types("fn g(x: A) { let _ = format!(\"{:?}\", x.f::<u8>()); }"));
    assert_eq!((outside, inside), (vec![], vec![]));
}

#[test]
fn a_name_a_macro_binds_proves_nothing_from_the_callers_binding() {
    // The closure's `x` shadows the parameter inside the macro, as outside one.
    let src = with_types(
        "fn g(x: A, v: Vec<B>) { assert!(v.iter().all(|x| x.f())); }\n\
         fn h(x: A, o: Option<B>) { assert!(matches!(o, Some(x) if x.f())); }\n",
    );
    assert_eq!(calls_of_f(&src), vec![other()]);
    // A name the macro does not bind is still the parameter.
    let src = with_types("fn g(x: A, v: Vec<B>) { assert!(v.iter().all(|y| y.g()) && x.f()); }");
    assert_eq!(calls_of_f(&src), vec![typed("A", None)]);
}
