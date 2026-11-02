//! Unit tests for the signature reading (S-830, CR-260 G1) over the Go plugin's
//! `@signature.*` captures — in their own file, the `declared_types_tests.rs`
//! shape. Each fixture is a form zap and ollama write, with the near miss the
//! method-set pass must tell apart from it (`[]T` / `...T`, one result / two).

use std::collections::BTreeMap;

use crate::extract::{extract_files, FileInput, SymbolContext};
use crate::model::NodeKind;
use crate::plugin::LanguageRegistry;

fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    })
}

/// Each declaration's recorded signature, by `kind name` (one per name in the
/// fixtures).
fn signatures(source: &str) -> BTreeMap<String, Option<String>> {
    let facts = extract_files(&[FileInput::new("p/p.go", source)], registry(), &SymbolContext::default());
    facts
        .into_iter()
        .next()
        .expect("p.go is extracted")
        .nodes
        .into_iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method | NodeKind::Interface | NodeKind::Struct))
        .map(|n| (format!("{} {}", n.kind.as_str(), n.name), n.callable_signature))
        .collect()
}

fn sig(text: &str) -> Option<String> {
    Some(text.to_string())
}

#[test]
fn a_callable_records_its_parameter_types_then_its_result_types_without_names() {
    let got = signatures(
        "package p\n\n\
         func Read(p []byte) (n int, err error) { return 0, nil }\n\
         func Close() error { return nil }\n\
         func run() {}\n\
         func pair(a, b int, s string) (int, error) { return 0, nil }\n\
         func spread(f string, args ...any) {}\n\
         func slice(f string, args []any) {}\n\
         type T struct{}\n\
         func (t *T) Write(ctx  context.Context, m map[string] *Entry) (written chan<- int) { return nil }\n",
    );
    assert_eq!(got["function Read"], sig("([]byte)(int,error)"));
    assert_eq!(got["function Close"], sig("()(error)"));
    assert_eq!(got["function run"], sig("()()"));
    assert_eq!(got["function pair"], sig("(int,int,string)(int,error)"), "a name list is one type per name");
    assert_eq!(got["function spread"], sig("(string,...any)()"), "a variadic parameter keeps its `...`");
    assert_eq!(got["function slice"], sig("(string,[]any)()"), "…and a slice is not one");
    assert_eq!(
        got["method Write"],
        sig("(context.Context,map[string]*Entry)(chan<-int)"),
        "the receiver is no parameter, and blanks go but between two words"
    );
    assert_eq!(got["struct T"], None, "a non-generic type records nothing");
}

#[test]
fn a_function_type_inside_a_signature_drops_its_own_parameter_names() {
    let got = signatures(
        "package p\n\n\
         func On(cb func(n int, err error) bool, ch <-chan int) {}\n\
         func Off(cb func(int, error) bool, ch <-chan int) {}\n",
    );
    assert_eq!(got["function On"], sig("(func(int,error)bool,<-chan int)()"));
    assert_eq!(got["function On"], got["function Off"], "one type, however its parameters are named");
}

#[test]
fn an_interface_spec_records_its_signature_and_a_generic_interface_its_type_parameters() {
    let got = signatures(
        "package p\n\n\
         type Reader interface {\n\tRead(p []byte) (int, error)\n}\n\
         type Ptr[T any, U comparable] interface {\n\t*T\n}\n\
         type Plain interface{}\n",
    );
    assert_eq!(got["method Read"], sig("([]byte)(int,error)"));
    assert_eq!(got["interface Ptr"], sig("[T,U]"), "a spec-less generic interface is still generic");
    assert_eq!(got["interface Plain"], None);
    assert_eq!(got["interface Reader"], None);
}

#[test]
fn a_method_spec_inside_a_parameter_type_gives_its_result_to_no_one() {
    let got = signatures(
        "package p\n\nfunc Use(x interface{ Len() int }) {}\n",
    );
    assert_eq!(got["function Use"], sig("(interface{Len()int})()"), "the spec's `int` is no result of `Use`");
}

#[test]
fn a_method_of_a_generic_receiver_names_the_type_parameters_it_binds() {
    let got = signatures(
        "package p\n\n\
         type Set[T comparable] struct{}\n\
         func (s *Set[T]) Add(v T) bool { return true }\n\
         type Pair[K, V any] struct{}\n\
         func (p Pair[K, V]) Get(k K) V { var v V; return v }\n\
         type List struct{}\n\
         func (l List) Len() int { return 0 }\n",
    );
    assert_eq!(got["method Add"], sig("[T](T)(bool)"), "a pointer receiver's type argument");
    assert_eq!(got["method Get"], sig("[K,V](K)(V)"), "a value receiver's, two of them");
    assert_eq!(got["method Len"], sig("()(int)"), "a plain receiver binds nothing");
}

/// zap's shape: a file whose one signature capture is a generic interface's
/// type parameters — no callable, no spec — still records them.
#[test]
fn a_file_of_one_spec_less_generic_interface_records_its_type_parameters() {
    let got = signatures("package p\n\ntype Ptr[T any] interface {\n\t*T\n}\n");
    assert_eq!(got["interface Ptr"], sig("[T]"));
}
