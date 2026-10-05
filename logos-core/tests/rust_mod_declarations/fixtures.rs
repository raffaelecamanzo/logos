//! Rust crates whose modules are declared with bodyless `mod x;` lines — the
//! shape every real crate has (S-585, [FR-RS-41]).
//!
//! [FR-RS-41]: ../../../docs/specs/requirements/FR-RS-41.md

pub type Fixture = &'static [(&'static str, &'static str)];

const CARGO_TOML: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// CR-186's reproduction: one declared module, one import through it, one call.
pub const MINIMAL: Fixture = &[
    ("Cargo.toml", CARGO_TOML),
    (
        "src/lib.rs",
        "pub mod util;\n\nuse crate::util::run;\n\npub fn alpha() {\n    run();\n}\n",
    ),
    ("src/util.rs", "pub fn run() {}\n"),
];

/// `mod a;` → `a/mod.rs` → `mod b;` → `b.rs`, with an import through both
/// declarations, a `self::` path from `a` and a `super::` path from `b`.
pub const CHAIN_MOD_RS: Fixture = &[
    ("Cargo.toml", CARGO_TOML),
    (
        "src/lib.rs",
        "mod a;\n\nuse crate::a::b::c;\n\npub fn alpha() {\n    c();\n}\n",
    ),
    (
        "src/a/mod.rs",
        "pub mod b;\n\npub fn helper() {\n    self::b::c();\n}\n",
    ),
    ("src/a/b.rs", "pub fn c() {}\n\npub fn up() {\n    super::helper();\n}\n"),
];

/// [`CHAIN_MOD_RS`] in the 2018 layout: `a.rs` beside the `a/` directory.
pub const CHAIN_A_RS: Fixture = &[
    ("Cargo.toml", CARGO_TOML),
    (
        "src/lib.rs",
        "mod a;\n\nuse crate::a::b::c;\n\npub fn alpha() {\n    c();\n}\n",
    ),
    (
        "src/a.rs",
        "pub mod b;\n\npub fn helper() {\n    self::b::c();\n}\n",
    ),
    ("src/a/b.rs", "pub fn c() {}\n\npub fn up() {\n    super::helper();\n}\n"),
];

/// [`CHAIN_MOD_RS`] with module names that sort **after** `lib.rs`. A file's
/// nodes are numbered in path order, so here every declaration is numbered
/// before the file it declares — the order that used to hand the key to the
/// empty declaration. (`src/a/…` sorts before `src/lib.rs`, so the `a`/`b`
/// chain bound by luck before S-585.)
pub const CHAIN_LATE_MOD_RS: Fixture = &[
    ("Cargo.toml", CARGO_TOML),
    (
        "src/lib.rs",
        "mod x;\n\nuse crate::x::y::c;\n\npub fn alpha() {\n    c();\n}\n",
    ),
    (
        "src/x/mod.rs",
        "pub mod y;\n\npub fn helper() {\n    self::y::c();\n}\n",
    ),
    ("src/x/y.rs", "pub fn c() {}\n\npub fn up() {\n    super::helper();\n}\n"),
];

/// [`CHAIN_LATE_MOD_RS`] in the 2018 layout: `x.rs` beside the `x/` directory.
pub const CHAIN_LATE_X_RS: Fixture = &[
    ("Cargo.toml", CARGO_TOML),
    (
        "src/lib.rs",
        "mod x;\n\nuse crate::x::y::c;\n\npub fn alpha() {\n    c();\n}\n",
    ),
    (
        "src/x.rs",
        "pub mod y;\n\npub fn helper() {\n    self::y::c();\n}\n",
    ),
    ("src/x/y.rs", "pub fn c() {}\n\npub fn up() {\n    super::helper();\n}\n"),
];

/// Declarations whose file cannot be read off the path — `#[path]` names
/// `x_impl.rs`, and `missing.rs` does not exist — beside an inline module.
pub const UNDECLARABLE: Fixture = &[
    ("Cargo.toml", CARGO_TOML),
    (
        "src/lib.rs",
        "#[path = \"x_impl.rs\"]\nmod x;\nmod missing;\n\nmod inner {\n    pub fn deep() {}\n}\n\n\
         use crate::x::go;\nuse crate::missing::gone;\n\n\
         pub fn alpha() {\n    go();\n    gone();\n    inner::deep();\n}\n",
    ),
    ("src/x_impl.rs", "pub fn go() {}\n"),
];
