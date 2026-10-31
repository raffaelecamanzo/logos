//! The two-crate dependency-inversion fixture ([CR-226]) — the one definition
//! the dispatch marker suite (S-712) and the architecture-view suite (S-713)
//! both index.
//!
//! Crate `a` declares the trait and calls it through `&dyn Tr` and `&impl Tr`;
//! crate `b` depends on `a` and implements it. `m` is a required signature, `d`
//! a default `B` overrides. Written inline into a temp directory: a fixture
//! tree checked into this repository would be indexed into its own graph.
//!
//! [CR-226]: ../../../docs/requests/CR-226-dispatch-is-not-a-dependency.md

#![allow(dead_code)] // each including binary uses a different subset

use std::fs;

use tempfile::TempDir;

/// Crate `a`'s library: the trait and its two trait-typed callers.
pub const A_LIB: &str = "\
pub trait Tr {
    fn m(&self);
    fn d(&self) {}
}
pub fn by_dyn(t: &dyn Tr) { t.m(); t.d(); }
pub fn by_impl(t: &impl Tr) { t.m(); }
";

/// [`A_LIB`] with a direct call from `by_impl` to `b`'s implementation of `m`.
pub const A_LIB_DIRECT: &str = "\
pub trait Tr {
    fn m(&self);
    fn d(&self) {}
}
pub fn by_dyn(t: &dyn Tr) { t.m(); t.d(); }
pub fn by_impl(t: &impl Tr) { t.m(); b::B::m(&b::B); }
";

/// Crate `b`'s library: the implementation.
pub const B_LIB: &str = "\
use a::Tr;
pub struct B;
impl Tr for B {
    fn m(&self) {}
    fn d(&self) {}
}
";

/// The fixture with `a_lib` as crate `a`'s library, plus any `extra` files.
pub fn two_crates_with(a_lib: &str, extra: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let files = [
        ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n"),
        ("a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("a/src/lib.rs", a_lib),
        (
            "b/Cargo.toml",
            "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\na = { path = \"../a\" }\n",
        ),
        ("b/src/lib.rs", B_LIB),
    ];
    for (rel, text) in files.iter().chain(extra) {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    tmp
}

/// The fixture with `a_lib` as crate `a`'s library.
pub fn two_crates(a_lib: &str) -> TempDir {
    two_crates_with(a_lib, &[])
}
