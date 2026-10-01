//! **S-479 — the first-party TypeScript method-call residue, by receiver shape**
//! ([CR-154] §3.2 item 3, [FR-RS-06], [NFR-RA-05]).
//!
//! A read-only measurement. Since [CR-066] a method-form call (`x.foo()`, ledger
//! form [`RefForm::Method`]) binds only on scope evidence ([FR-RS-06]); [CR-154]
//! recorded that first-party TypeScript method-form calls resolve 58 of 498 on
//! the reference estate. Before anyone files TypeScript receiver typing — the
//! approach [CR-150] proposes for Java — this module classifies what is left and
//! states what a declared-type receiver rule could reach at most. It builds
//! nothing and changes no product code.
//!
//! # What a row is
//!
//! A row is one `unresolved_refs` row of kind `Calls` and form `Method` whose
//! file the shipped `typescript`/`tsx` plugins own. The ledger keys a row by
//! `(caller, method name)`, so one row can stand for several call sites; the row
//! is classified by the site at the line it records ([`locate`]), and the report
//! counts the rows whose caller holds another same-named site of a different
//! receiver shape, so the de-duplication is visible rather than assumed away.
//!
//! **The stated denominator** is the first-party `.ts`/`.tsx` population — the
//! one [CR-154] §3.1 measured. First-party `.js` is classified and reported as a
//! second table, never folded into the headline. Vendored rows are counted only.
//! The first-party/vendored split is [S-477]'s path rule ([`is_vendored`]),
//! harness-only evidence that is never shipped ([CR-154] CRA-03).
//!
//! # The four receiver shapes
//!
//! Every located row lands in exactly one [`Shape`], because the shape is a
//! function of the finer [`Sub`] and every `Sub` maps to exactly one `Shape`:
//!
//!   - **`this.<field>` (declared type)** — `this.f.m()` where the enclosing class
//!     declares `f`, as a field or a constructor parameter property, with a type
//!     annotation.
//!   - **imported symbol** — the receiver is a name a relative (first-party)
//!     `import` or `require` binds in this file.
//!   - **library/global** — the receiver is a name a package `import` binds, or a
//!     name nothing in the file declares (`console`, `Math`, `cy`).
//!   - **chained/untyped** — every other receiver: a call result, a member chain,
//!     an untyped or undeclared `this.f`, bare `this`/`super`, a local or
//!     parameter, a type declared in the file, `new X()`, a literal. Its sub-shapes
//!     are printed, because "untyped" is not true of all of them — a local with a
//!     declared type sits here, since the four shapes are [S-479]'s, not ours.
//!
//! # The upper bound
//!
//! Separately from the shape, [`receiver`] derives the type a declared-type rule
//! would give the receiver — [CR-150] §3.2 A's table, transposed to TypeScript:
//! a typed `this.f`; a local or parameter with one declared type in the file; an
//! imported or same-file type name (a static call); `this` (the enclosing class);
//! `super` (its `extends`). [`judge`] then looks the type up among the member's
//! TypeScript type nodes and the method among that type's own `Contains`
//! children. **Every judgement call is resolved toward binding**, so the count is
//! an upper bound, never a floor for later work:
//!
//!   - typing is file-scoped and ignores shadowing; an unannotated declaration of
//!     the same name never poisons an annotated one;
//!   - a type is matched by name across the member, not through the import path;
//!   - a method the type lacks is looked up through its source `extends` chain
//!     ([CR-150] §3.2 B), each supertype matched by name the same way.
//!
//! The bound is split three ways: a **self-tie** (the receiver's type is the
//! caller's own class, so the edge adds no cross-class reach), a cross-class
//! bind, and the ambiguous rows (two type nodes of that name, or two callables of
//! that name on the type) that the never-fabricate rule ([NFR-RA-05]) keeps
//! unbound.
//!
//! # A private copy only
//!
//! [`open_member_store`] is the one way this module opens a member database: the
//! shipped read-only open, which refuses — never migrates — a store at another
//! schema version. A store this binary can read was therefore written by a
//! binary carrying this sprint's migration, so the run cannot measure a store
//! that predates [S-477]'s fields; [`void_reason`] refuses one with no TypeScript
//! `Field` node as well. The estate run skips in `gate.sh` and CI; a run that
//! sees no estate reports **VOID**, never zero.
//!
//! [CR-066]: ../../../docs/requests/CR-066-receiver-method-overbinding.md
//! [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
//! [CR-154]: ../../../docs/requests/CR-154-typescript-own-field-accesses-bind.md
//! [FR-RS-06]: ../../../docs/specs/requirements/FR-RS-06.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [S-477]: ../../../docs/planning/journal.md#s-477-typescript-class-fields-and-parameter-properties-are-field-nodes-so-own-field-accesses-bind
//! [S-479]: ../../../docs/planning/journal.md#s-479-measure-the-first-party-typescript-method-call-residue-by-receiver-shape

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::federation::discover;
use logos_core::graph_store::{GraphStore, SqliteGraphStore};
use logos_core::model::{EdgeKind, NodeKind, RefForm};
use logos_core::plugin::LanguageRegistry;
use tree_sitter::{Node, Parser, Tree};

/// The recorded result, reproduced by the estate run and asserted against it.
/// Embedded, following [`super::cross_member_type_refs::RECORDED_FINDING`]: a
/// file the build embeds cannot be deleted or renamed without breaking
/// compilation.
pub const RECORDED_FINDING: &str = include_str!("ts_method_residue_finding.txt");

/// The filing baseline [CR-154] §3.1 states — reconciled against, never asserted.
///
/// [CR-154]: ../../../docs/requests/CR-154-typescript-own-field-accesses-bind.md
pub const FILING_BASELINE: (usize, usize) = (58, 498);

/// The plugins whose files are "TypeScript" here: one language split across two
/// tree-sitter grammars ([ADR-09]).
///
/// [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
pub const TS_PLUGINS: [&str; 2] = ["typescript", "tsx"];

// ── The first-party split ──────────────────────────────────────────────────

/// Members whose every file is vendored: `styleguide` is a static-resource
/// mirror of bootstrap, tinymce and azuremediaplayer.
pub const VENDORED_MEMBERS: [&str; 1] = ["styleguide"];

/// `(member, path prefix)` pairs holding a member's bundled third-party scripts.
pub const VENDORED_PREFIXES: [(&str, &str); 3] =
    [("webmail", "plugins/"), ("webmail", "skins/"), ("webmail", "a11y_report/")];

/// [S-477]'s split rule, verbatim from its implementation notes: vendored is a
/// `.min.` file name, any file of a [`VENDORED_MEMBERS`] member, or a
/// [`VENDORED_PREFIXES`] path; everything else is first-party.
///
/// [S-477]: ../../../docs/planning/journal.md#s-477-typescript-class-fields-and-parameter-properties-are-field-nodes-so-own-field-accesses-bind
pub fn is_vendored(member: &str, path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.contains(".min.")
        || VENDORED_MEMBERS.contains(&member)
        || VENDORED_PREFIXES.iter().any(|(m, p)| *m == member && path.starts_with(p))
}

/// Which population a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Population {
    /// First-party `.ts`/`.tsx` (and `.mts`/`.cts`) — the stated denominator.
    FirstPartyTs,
    /// First-party JavaScript the TypeScript plugins admit.
    FirstPartyJs,
    /// Vendored — counted, never classified.
    Vendored,
}

impl Population {
    pub fn of(member: &str, path: &str) -> Self {
        if is_vendored(member, path) {
            return Population::Vendored;
        }
        let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
        if matches!(ext, "ts" | "tsx" | "mts" | "cts") {
            Population::FirstPartyTs
        } else {
            Population::FirstPartyJs
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Population::FirstPartyTs => "first-party .ts/.tsx",
            Population::FirstPartyJs => "first-party .js",
            Population::Vendored => "vendored",
        }
    }
}

// ── Shapes ─────────────────────────────────────────────────────────────────

/// The four receiver shapes [S-479]'s acceptance criterion names.
///
/// [S-479]: ../../../docs/planning/journal.md#s-479-measure-the-first-party-typescript-method-call-residue-by-receiver-shape
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    ThisField,
    Imported,
    LibraryGlobal,
    ChainedUntyped,
}

impl Shape {
    pub const ALL: [Shape; 4] =
        [Shape::ThisField, Shape::Imported, Shape::LibraryGlobal, Shape::ChainedUntyped];

    pub fn label(self) -> &'static str {
        match self {
            Shape::ThisField => "this.<field> (declared type)",
            Shape::Imported => "imported symbol",
            Shape::LibraryGlobal => "library/global",
            Shape::ChainedUntyped => "chained/untyped",
        }
    }
}

/// The finer receiver form. [`Sub::shape`] is total and single-valued, which is
/// what makes "exactly one shape per row" true by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sub {
    ThisFieldTyped,
    RelativeImport,
    PackageImport,
    Global,
    CallResult,
    MemberChain,
    ThisFieldUntyped,
    ThisFieldUndeclared,
    This,
    Super,
    LocalTyped,
    LocalUntyped,
    LocalTypeName,
    Other,
}

impl Sub {
    pub const ALL: [Sub; 14] = [
        Sub::ThisFieldTyped,
        Sub::RelativeImport,
        Sub::PackageImport,
        Sub::Global,
        Sub::CallResult,
        Sub::MemberChain,
        Sub::ThisFieldUntyped,
        Sub::ThisFieldUndeclared,
        Sub::This,
        Sub::Super,
        Sub::LocalTyped,
        Sub::LocalUntyped,
        Sub::LocalTypeName,
        Sub::Other,
    ];

    pub fn shape(self) -> Shape {
        match self {
            Sub::ThisFieldTyped => Shape::ThisField,
            Sub::RelativeImport => Shape::Imported,
            Sub::PackageImport | Sub::Global => Shape::LibraryGlobal,
            Sub::CallResult
            | Sub::MemberChain
            | Sub::ThisFieldUntyped
            | Sub::ThisFieldUndeclared
            | Sub::This
            | Sub::Super
            | Sub::LocalTyped
            | Sub::LocalUntyped
            | Sub::LocalTypeName
            | Sub::Other => Shape::ChainedUntyped,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sub::ThisFieldTyped => "this.f, f declared with a type",
            Sub::RelativeImport => "relative import / require",
            Sub::PackageImport => "package import",
            Sub::Global => "global (declared nowhere in the file)",
            Sub::CallResult => "call result  a.b().m()",
            Sub::MemberChain => "member chain  a.b.m()",
            Sub::ThisFieldUntyped => "this.f, f declared without a type",
            Sub::ThisFieldUndeclared => "this.f, f not declared by the class",
            Sub::This => "this.m()",
            Sub::Super => "super.m()",
            Sub::LocalTyped => "local/parameter with a declared type",
            Sub::LocalUntyped => "local/parameter without one",
            Sub::LocalTypeName => "type declared in this file (static)",
            Sub::Other => "other (new / literal / await / subscript / as)",
        }
    }
}

// ── Per-file facts ─────────────────────────────────────────────────────────

const CLASS_KINDS: [&str; 3] = ["class_declaration", "abstract_class_declaration", "class"];

fn text<'s>(n: Node, src: &'s str) -> &'s str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Every node under `root`, in no particular order (an explicit stack: no
/// recursion depth to blow on a deep file).
fn each_node<'t>(root: Node<'t>, mut f: impl FnMut(Node<'t>)) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        f(n);
        let mut c = n.walk();
        stack.extend(n.children(&mut c));
    }
}

/// The head of a declared type: `Foo`, `Foo<T>`, `ns.Foo`, `Foo | null`,
/// `string`. `None` when the annotation names no single type (a function or
/// object type, a union of two types, `any`).
pub fn type_head(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "type_annotation" | "parenthesized_type" => {
            node.named_child(0).and_then(|c| type_head(c, src))
        }
        "type_identifier" => Some(text(node, src).to_string()),
        "nested_type_identifier" | "generic_type" => {
            node.child_by_field_name("name").and_then(|n| type_head(n, src))
        }
        "predefined_type" => match text(node, src) {
            "any" | "unknown" | "never" | "void" | "object" => None,
            t => Some(t.to_string()),
        },
        "union_type" => {
            let mut members = Vec::new();
            let mut stack = vec![node];
            while let Some(n) = stack.pop() {
                if n.kind() == "union_type" {
                    let mut c = n.walk();
                    stack.extend(n.named_children(&mut c));
                } else if !matches!(text(n, src), "null" | "undefined") {
                    members.push(n);
                }
            }
            match members.as_slice() {
                [one] => type_head(*one, src),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The bindings a file declares, file-scoped (shadowing ignored, toward binding).
#[derive(Debug, Default)]
pub struct FileFacts {
    /// Imported name → whether its specifier is relative (first-party).
    pub imports: BTreeMap<String, bool>,
    /// Local / parameter name → the declared type head of each declaration
    /// (`None` for an unannotated one).
    pub locals: BTreeMap<String, Vec<Option<String>>>,
    /// Classes, interfaces, enums, type aliases and functions declared here.
    pub type_names: BTreeSet<String>,
}

fn unquote(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'' || c == '`')
}

fn is_relative(specifier: &str) -> bool {
    specifier.starts_with('.') || specifier.starts_with('/')
}

/// Identifiers a binding pattern introduces (`a`, `{ a, b: c }`, `[a, ...b]`).
fn pattern_names(node: Node, src: &str, out: &mut Vec<String>) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(text(node, src).into()),
        "pair_pattern" => {
            if let Some(v) = node.child_by_field_name("value") {
                pattern_names(v, src, out);
            }
        }
        "assignment_pattern" => {
            if let Some(l) = node.child_by_field_name("left") {
                pattern_names(l, src, out);
            }
        }
        "object_pattern" | "array_pattern" | "rest_pattern" | "object_assignment_pattern" => {
            let mut c = node.walk();
            for ch in node.named_children(&mut c) {
                pattern_names(ch, src, out);
            }
        }
        _ => {}
    }
}

/// `require("x")` → `Some("x")`.
fn require_specifier(value: Node, src: &str) -> Option<String> {
    if value.kind() != "call_expression" {
        return None;
    }
    let f = value.child_by_field_name("function")?;
    if f.kind() != "identifier" || text(f, src) != "require" {
        return None;
    }
    let args = value.child_by_field_name("arguments")?;
    let first = args.named_child(0)?;
    (first.kind() == "string").then(|| unquote(text(first, src)).to_string())
}

impl FileFacts {
    pub fn read(tree: &Tree, src: &str) -> Self {
        let mut f = FileFacts::default();
        each_node(tree.root_node(), |n| match n.kind() {
            "import_statement" => {
                let Some(source) = n.child_by_field_name("source") else { return };
                let relative = is_relative(unquote(text(source, src)));
                let mut names = Vec::new();
                let mut c = n.walk();
                for clause in n.named_children(&mut c) {
                    match clause.kind() {
                        "import_clause" => {
                            let mut c2 = clause.walk();
                            for part in clause.named_children(&mut c2) {
                                match part.kind() {
                                    "identifier" => names.push(text(part, src).to_string()),
                                    "namespace_import" => {
                                        if let Some(id) = part.named_child(0) {
                                            names.push(text(id, src).to_string());
                                        }
                                    }
                                    "named_imports" => {
                                        let mut c3 = part.walk();
                                        for spec in part.named_children(&mut c3) {
                                            let bound = spec
                                                .child_by_field_name("alias")
                                                .or_else(|| spec.child_by_field_name("name"));
                                            if let Some(b) = bound {
                                                names.push(unquote(text(b, src)).to_string());
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "import_require_clause" => {
                            if let Some(id) = clause.named_child(0) {
                                names.push(text(id, src).to_string());
                            }
                        }
                        _ => {}
                    }
                }
                for name in names {
                    f.imports.insert(name, relative);
                }
            }
            "variable_declarator" => {
                let Some(name) = n.child_by_field_name("name") else { return };
                let mut names = Vec::new();
                pattern_names(name, src, &mut names);
                if let Some(spec) =
                    n.child_by_field_name("value").and_then(|v| require_specifier(v, src))
                {
                    for b in names {
                        f.imports.insert(b, is_relative(&spec));
                    }
                    return;
                }
                let head = n.child_by_field_name("type").and_then(|t| type_head(t, src));
                for b in names {
                    f.locals.entry(b).or_default().push(head.clone());
                }
            }
            "required_parameter" | "optional_parameter" => {
                let Some(p) = n.child_by_field_name("pattern") else { return };
                let mut names = Vec::new();
                pattern_names(p, src, &mut names);
                let head = n.child_by_field_name("type").and_then(|t| type_head(t, src));
                for b in names {
                    f.locals.entry(b).or_default().push(head.clone());
                }
            }
            "arrow_function" | "catch_clause" | "for_in_statement" => {
                let field = match n.kind() {
                    "arrow_function" => "parameter",
                    "catch_clause" => "parameter",
                    _ => "left",
                };
                if let Some(p) = n.child_by_field_name(field) {
                    let head = n.child_by_field_name("type").and_then(|t| type_head(t, src));
                    let mut names = Vec::new();
                    pattern_names(p, src, &mut names);
                    for b in names {
                        f.locals.entry(b).or_default().push(head.clone());
                    }
                }
            }
            "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "type_alias_declaration"
            | "function_declaration"
            | "generator_function_declaration" => {
                if let Some(name) = n.child_by_field_name("name") {
                    f.type_names.insert(text(name, src).to_string());
                }
            }
            _ => {}
        });
        f
    }
}

/// What a class declares, read from its own body.
#[derive(Debug, Default)]
pub struct ClassFacts {
    pub name: Option<String>,
    /// Field / parameter-property name → its declared type head (`None` when
    /// declared without an annotation the head rule reads).
    pub fields: BTreeMap<String, Option<String>>,
    /// The `extends` clause's type head.
    pub extends: Option<String>,
    /// An untyped field's initialiser — report-only, see [`Initialiser`].
    pub inits: BTreeMap<String, Initialiser>,
}

/// What initialises a field declared without a type: `inject(T)`, `new T()`,
/// `signal()`. **Report-only**: inferring a type from it is *not* the
/// declared-type rule the upper bound measures, so its would-bind figure is
/// printed beside the bound as a separate extension, never added to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initialiser {
    pub label: String,
    /// The type `inject(T)` or `new T()` names.
    pub ty: Option<String>,
}

/// The type an expression names, as `inject(…)`'s argument or `new …()`'s
/// constructor spells it: `T`, `T<A, B>` (an `instantiation_expression`), or
/// `ns.T` (its last segment).
pub fn type_name_of(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "identifier" => Some(text(n, src).to_string()),
        "instantiation_expression" => n.named_child(0).and_then(|c| type_name_of(c, src)),
        "member_expression" => n.child_by_field_name("property").map(|p| text(p, src).to_string()),
        _ => None,
    }
}

impl Initialiser {
    pub fn read(value: Option<Node>, src: &str) -> Self {
        let Some(v) = value else {
            return Initialiser { label: "no initialiser".into(), ty: None };
        };
        match v.kind() {
            "call_expression" => {
                let f = v.child_by_field_name("function");
                let name = f.filter(|f| f.kind() == "identifier").map(|f| text(f, src));
                let arg = v
                    .child_by_field_name("arguments")
                    .and_then(|a| a.named_child(0))
                    .and_then(|a| type_name_of(a, src));
                match name {
                    Some("inject") => Initialiser { label: "inject(T)".into(), ty: arg },
                    Some(n) => Initialiser { label: format!("{n}()"), ty: None },
                    None => Initialiser { label: "call".into(), ty: None },
                }
            }
            "new_expression" => Initialiser {
                label: "new T()".into(),
                ty: v.child_by_field_name("constructor").and_then(|c| type_name_of(c, src)),
            },
            kind => Initialiser { label: kind.into(), ty: None },
        }
    }
}

fn is_parameter_property(param: Node) -> bool {
    let mut c = param.walk();
    let found = param
        .children(&mut c)
        .any(|ch| matches!(ch.kind(), "accessibility_modifier" | "override_modifier" | "readonly"));
    found
}

impl ClassFacts {
    pub fn read(class: Node, src: &str) -> Self {
        let mut out = ClassFacts {
            name: class.child_by_field_name("name").map(|n| text(n, src).to_string()),
            ..ClassFacts::default()
        };
        let mut c = class.walk();
        for ch in class.children(&mut c) {
            if ch.kind() == "class_heritage" {
                let mut c2 = ch.walk();
                for clause in ch.named_children(&mut c2) {
                    if clause.kind() == "extends_clause" {
                        out.extends = clause
                            .child_by_field_name("value")
                            .map(|v| text(v, src).rsplit('.').next().unwrap_or("").to_string());
                    }
                }
            }
        }
        let Some(body) = class.child_by_field_name("body") else { return out };
        let mut c = body.walk();
        for member in body.named_children(&mut c) {
            match member.kind() {
                "public_field_definition" => {
                    if let Some(name) = member.child_by_field_name("name") {
                        let head =
                            member.child_by_field_name("type").and_then(|t| type_head(t, src));
                        if head.is_none() {
                            let init = Initialiser::read(member.child_by_field_name("value"), src);
                            out.inits.insert(text(name, src).to_string(), init);
                        }
                        out.fields.insert(text(name, src).to_string(), head);
                    }
                }
                "method_definition" => {
                    let is_ctor = member
                        .child_by_field_name("name")
                        .is_some_and(|n| text(n, src) == "constructor");
                    let params = member.child_by_field_name("parameters");
                    if let (true, Some(params)) = (is_ctor, params) {
                        let mut c2 = params.walk();
                        for p in params.named_children(&mut c2) {
                            if !is_parameter_property(p) {
                                continue;
                            }
                            let Some(pat) = p.child_by_field_name("pattern") else { continue };
                            if pat.kind() != "identifier" {
                                continue;
                            }
                            let head =
                                p.child_by_field_name("type").and_then(|t| type_head(t, src));
                            out.fields.insert(text(pat, src).to_string(), head);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

fn enclosing_class(mut n: Node) -> Option<Node> {
    while let Some(p) = n.parent() {
        if CLASS_KINDS.contains(&p.kind()) {
            return Some(p);
        }
        n = p;
    }
    None
}

// ── Receiver classification ────────────────────────────────────────────────

/// One method-form call site: the receiver's form and the type a declared-type
/// rule would give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// 1-based line of the method name — the line the ledger records.
    pub line: usize,
    /// Byte offset of the method name, to order same-line sites.
    pub at: usize,
    pub method: String,
    pub sub: Sub,
    /// The receiver's source text, single-lined and truncated for the report.
    pub receiver: String,
    /// The type a declared-type rule would give the receiver.
    pub ty: Option<String>,
    /// The enclosing class's name, when the call sits in one.
    pub class: Option<String>,
    /// For a [`Sub::ThisFieldUntyped`] receiver, the field's initialiser.
    pub init: Option<Initialiser>,
}

/// Strip what does not change the receiver: `x!`, `(x)`.
fn unwrap_receiver(mut n: Node) -> Node {
    while matches!(n.kind(), "non_null_expression" | "parenthesized_expression") {
        match n.named_child(0) {
            Some(inner) => n = inner,
            None => break,
        }
    }
    n
}

/// Classify one receiver expression.
pub fn receiver(
    obj: Node,
    src: &str,
    facts: &FileFacts,
    class: Option<&ClassFacts>,
) -> (Sub, Option<String>) {
    let obj = unwrap_receiver(obj);
    match obj.kind() {
        "this" => (Sub::This, class.and_then(|c| c.name.clone())),
        "super" => (Sub::Super, class.and_then(|c| c.extends.clone())),
        "member_expression" => {
            let inner = obj.child_by_field_name("object").map(unwrap_receiver);
            let prop = obj.child_by_field_name("property");
            match (inner, prop) {
                (Some(i), Some(p)) if i.kind() == "this" => {
                    match class.and_then(|c| c.fields.get(text(p, src))) {
                        Some(Some(ty)) => (Sub::ThisFieldTyped, Some(ty.clone())),
                        Some(None) => (Sub::ThisFieldUntyped, None),
                        None => (Sub::ThisFieldUndeclared, None),
                    }
                }
                _ => (Sub::MemberChain, None),
            }
        }
        "call_expression" => (Sub::CallResult, None),
        "identifier" => {
            let name = text(obj, src);
            if let Some(&relative) = facts.imports.get(name) {
                let sub = if relative { Sub::RelativeImport } else { Sub::PackageImport };
                return (sub, Some(name.to_string()));
            }
            if let Some(decls) = facts.locals.get(name) {
                let typed: BTreeSet<&String> = decls.iter().flatten().collect();
                return match typed.into_iter().collect::<Vec<_>>().as_slice() {
                    [one] => (Sub::LocalTyped, Some((*one).clone())),
                    _ => (Sub::LocalUntyped, None),
                };
            }
            if facts.type_names.contains(name) {
                return (Sub::LocalTypeName, Some(name.to_string()));
            }
            (Sub::Global, None)
        }
        "as_expression" | "satisfies_expression" => {
            (Sub::Other, obj.named_child(1).and_then(|t| type_head(t, src)))
        }
        "new_expression" => {
            (Sub::Other, obj.child_by_field_name("constructor").and_then(|c| type_name_of(c, src)))
        }
        _ => (Sub::Other, None),
    }
}

/// Every method-form call site in a parsed file — the sites `references.scm`'s
/// `@ref.method` captures: a `call_expression` whose function is a
/// `member_expression`.
pub fn sites(tree: &Tree, src: &str) -> Vec<Site> {
    let facts = FileFacts::read(tree, src);
    let mut out = Vec::new();
    each_node(tree.root_node(), |n| {
        if n.kind() != "call_expression" {
            return;
        }
        let Some(f) = n.child_by_field_name("function") else { return };
        if f.kind() != "member_expression" {
            return;
        }
        let (Some(obj), Some(prop)) =
            (f.child_by_field_name("object"), f.child_by_field_name("property"))
        else {
            return;
        };
        if prop.kind() != "property_identifier" {
            return;
        }
        let class = enclosing_class(n).map(|c| ClassFacts::read(c, src));
        let (sub, ty) = receiver(obj, src, &facts, class.as_ref());
        let init = (sub == Sub::ThisFieldUntyped)
            .then(|| unwrap_receiver(obj).child_by_field_name("property"))
            .flatten()
            .and_then(|p| class.as_ref()?.inits.get(text(p, src)).cloned());
        let mut shown: String = text(obj, src).split_whitespace().collect::<Vec<_>>().join(" ");
        if shown.chars().count() > 60 {
            shown = shown.chars().take(57).collect::<String>() + "...";
        }
        out.push(Site {
            line: prop.start_position().row + 1,
            at: prop.start_byte(),
            method: text(prop, src).to_string(),
            sub,
            receiver: shown,
            ty,
            class: class.and_then(|c| c.name),
            init,
        });
    });
    out.sort_by_key(|s| s.at);
    out
}

/// The site a ledger row records: the first site naming `method` on `line`.
/// `None` when the line holds none — harness drift, which the estate run refuses.
pub fn locate<'s>(sites: &'s [Site], line: usize, method: &str) -> Option<&'s Site> {
    sites.iter().find(|s| s.line == line && s.method == method)
}

// ── The upper bound ────────────────────────────────────────────────────────

/// One TypeScript type node of a member, and its own `Contains` children.
#[derive(Debug, Clone, Default)]
pub struct TypeNode {
    pub file: Option<String>,
    /// Child name → the kinds of the children of that name.
    pub members: BTreeMap<String, Vec<NodeKind>>,
    /// The type head its source `extends` clause names. Read by the harness from
    /// the declaring file: the TypeScript plugins record no `Extends` edge.
    pub extends: Option<String>,
}

/// How many `extends` hops the lookup follows before giving up — a bound, so a
/// cycle (`A extends B`, `B extends A`) terminates as a miss.
pub const MAX_SUPERTYPE_HOPS: usize = 8;

/// A member's TypeScript types by name.
#[derive(Debug, Default)]
pub struct TypeIndex {
    pub types: BTreeMap<String, Vec<TypeNode>>,
}

/// What a declared-type rule would do with one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    /// One callable of that name on the receiver's type, the caller's own class.
    SelfTie,
    /// One callable of that name on the receiver's type, another class.
    CrossClass,
    /// One callable of that name on an in-member supertype of the receiver's type.
    Inherited,
    /// More than one type node of that name in the member.
    TypeAmbiguous,
    /// More than one callable of that name on the type.
    OverloadAmbiguous,
    /// The type's member of that name is a field (an Angular `signal()` read,
    /// an arrow-function property) — not a callable.
    MemberIsField,
    /// Neither the type nor its in-member supertypes has a member of that name
    /// (an interface's signatures are not nodes).
    MemberMissing,
    /// The type lacks the member and its `extends` names a type not declared in
    /// the member — a library base class ([CR-150]'s `supertype-unreached`).
    ///
    /// [CR-150]: ../../../docs/requests/CR-150-java-receiver-typing-for-method-calls.md
    SupertypeExternal,
    /// No TypeScript type node of that name in the member (a library type, or
    /// another member's).
    ExternalType,
    /// The receiver yields no type at all.
    NoEvidence,
}

impl Outcome {
    pub const ALL: [Outcome; 10] = [
        Outcome::SelfTie,
        Outcome::CrossClass,
        Outcome::Inherited,
        Outcome::TypeAmbiguous,
        Outcome::OverloadAmbiguous,
        Outcome::MemberIsField,
        Outcome::MemberMissing,
        Outcome::SupertypeExternal,
        Outcome::ExternalType,
        Outcome::NoEvidence,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Outcome::SelfTie => "would bind: self-tie (own class)",
            Outcome::CrossClass => "would bind: cross-class",
            Outcome::Inherited => "would bind: on an in-member supertype",
            Outcome::TypeAmbiguous => "ambiguous: two types of that name",
            Outcome::OverloadAmbiguous => "ambiguous: two callables of that name",
            Outcome::MemberIsField => "miss: the member is a field",
            Outcome::MemberMissing => "miss: no such member on the type",
            Outcome::SupertypeExternal => "miss: inherited from a type not in the member",
            Outcome::ExternalType => "miss: type not declared in the member",
            Outcome::NoEvidence => "miss: no receiver type",
        }
    }

    pub fn binds(self) -> bool {
        matches!(self, Outcome::SelfTie | Outcome::CrossClass | Outcome::Inherited)
    }

    pub fn ambiguous(self) -> bool {
        matches!(self, Outcome::TypeAmbiguous | Outcome::OverloadAmbiguous)
    }
}

/// Judge one site against its member's types. `path` is the caller's file: a
/// self-tie binds on the receiver's own type when that type is the caller's own
/// class (its name, declared in the caller's file).
pub fn judge(site: &Site, path: &str, index: &TypeIndex) -> Outcome {
    let Some(ty) = site.ty.as_deref() else { return Outcome::NoEvidence };
    let one = |name: &str| match index.types.get(name).map(Vec::as_slice).unwrap_or_default() {
        [] => Err(None),
        [one] => Ok(one),
        _ => Err(Some(Outcome::TypeAmbiguous)),
    };
    let mut node = match one(ty) {
        Ok(node) => node,
        Err(e) => return e.unwrap_or(Outcome::ExternalType),
    };
    for hop in 0..=MAX_SUPERTYPE_HOPS {
        let kinds = node.members.get(&site.method).map(Vec::as_slice).unwrap_or_default();
        let callables =
            kinds.iter().filter(|k| matches!(k, NodeKind::Method | NodeKind::Function)).count();
        match callables {
            0 if kinds.contains(&NodeKind::Field) => return Outcome::MemberIsField,
            0 => {}
            1 if hop > 0 => return Outcome::Inherited,
            1 if site.class.as_deref() == Some(ty) && node.file.as_deref() == Some(path) => {
                return Outcome::SelfTie;
            }
            1 => return Outcome::CrossClass,
            _ => return Outcome::OverloadAmbiguous,
        }
        let Some(sup) = node.extends.as_deref() else { return Outcome::MemberMissing };
        node = match one(sup) {
            Ok(next) => next,
            Err(e) => return e.unwrap_or(Outcome::SupertypeExternal),
        };
    }
    Outcome::MemberMissing
}

/// `(file, class name) → extends head` for every class a parsed file declares.
pub fn class_extends(
    tree: &Tree,
    src: &str,
    path: &str,
    out: &mut BTreeMap<(String, String), String>,
) {
    each_node(tree.root_node(), |n| {
        if CLASS_KINDS.contains(&n.kind()) {
            let facts = ClassFacts::read(n, src);
            if let (Some(name), Some(sup)) = (facts.name, facts.extends) {
                out.insert((path.to_string(), name), sup);
            }
        }
    });
}

fn parse_file(registry: &LanguageRegistry, root: &Path, path: &str) -> (Tree, String) {
    let plugin = registry.for_path(path).expect("a TypeScript-plugin file");
    let src = std::fs::read_to_string(root.join(path))
        .unwrap_or_else(|e| panic!("{}: {e}", root.join(path).display()));
    let mut parser = Parser::new();
    parser.set_language(plugin.language()).expect("the plugin's grammar loads");
    let tree = parser.parse(&src, None).expect("tree-sitter returns a tree");
    (tree, src)
}

// ── The estate read ────────────────────────────────────────────────────────

/// Open one member store the only way this module does: the shipped read-only
/// open, which refuses — never migrates — a store at another schema version.
pub fn open_member_store(db: &Path) -> Result<SqliteGraphStore, String> {
    SqliteGraphStore::open_readonly(db).map_err(|e| format!("{e:#}"))
}

/// One classified first-party row.
#[derive(Debug, Clone)]
pub struct Classified {
    pub member: String,
    pub path: String,
    pub population: Population,
    pub site: Site,
    pub outcome: Outcome,
    /// What the report-only initialiser-inference extension would do with a
    /// [`Sub::ThisFieldUntyped`] row whose initialiser names a type.
    pub inferred: Option<Outcome>,
    /// The caller holds another same-named site of a different receiver shape.
    pub mixed: bool,
}

/// Row totals per population: `(rows, resolved)`.
pub type Totals = BTreeMap<Population, (usize, usize)>;

/// What the estate read produced.
#[derive(Debug, Default)]
pub struct Estate {
    pub members: usize,
    pub stores_read: usize,
    pub totals: Totals,
    /// Every first-party unresolved row, located and classified.
    pub classified: Vec<Classified>,
    /// First-party unresolved rows whose recorded line holds no site.
    pub unlocated: Vec<String>,
    /// TypeScript `Field` nodes across the members read — zero means the stores
    /// predate [S-477].
    ///
    /// [S-477]: ../../../docs/planning/journal.md#s-477-typescript-class-fields-and-parameter-properties-are-field-nodes-so-own-field-accesses-bind
    pub ts_fields: usize,
}

/// Why a run measured nothing worth recording, or `None` when it engaged.
pub fn void_reason(e: &Estate) -> Option<String> {
    let (rows, _) = e.totals.get(&Population::FirstPartyTs).copied().unwrap_or_default();
    if rows == 0 {
        return Some(
            "no first-party .ts/.tsx method-form rows were read — this is not the reference \
             estate, or its TypeScript members were never indexed"
                .into(),
        );
    }
    if e.ts_fields == 0 {
        return Some(
            "no TypeScript Field node exists in any member read — the stores predate S-477, so \
             the residue would be measured on the wrong graph"
                .into(),
        );
    }
    None
}

/// What one member's graph contributes: its TypeScript types, its TypeScript
/// `Field` count, and each TypeScript node's line span by symbol.
struct MemberGraph {
    index: TypeIndex,
    ts_fields: usize,
    spans: BTreeMap<String, (i64, i64)>,
}

fn member_graph(
    store: &SqliteGraphStore,
    ts_files: &BTreeSet<String>,
) -> Result<MemberGraph, String> {
    let nodes = store.all_nodes().map_err(|e| format!("nodes: {e:#}"))?;
    let edges = store.all_edges().map_err(|e| format!("edges: {e:#}"))?;
    let in_ts = |f: &Option<String>| f.as_ref().is_some_and(|p| ts_files.contains(p));
    let by_id: BTreeMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let mut children: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for e in edges.iter().filter(|e| e.kind == EdgeKind::Contains) {
        if let Some(child) = by_id.get(&e.target) {
            children.entry(e.source).or_default().push(*child);
        }
    }
    let mut index = TypeIndex::default();
    let mut fields = 0;
    let mut spans = BTreeMap::new();
    for n in &nodes {
        if !in_ts(&n.file_path) {
            continue;
        }
        if let (Some(s), Some(e)) = (n.start_line, n.end_line) {
            spans.insert(n.symbol.to_string(), (s, e));
        }
        match n.kind {
            NodeKind::Field => fields += 1,
            NodeKind::Class | NodeKind::Interface | NodeKind::Enum | NodeKind::TypeAlias => {
                let mut members: BTreeMap<String, Vec<NodeKind>> = BTreeMap::new();
                for c in children.get(&n.id).into_iter().flatten() {
                    members.entry(c.name.clone()).or_default().push(c.kind);
                }
                index.types.entry(n.name.clone()).or_default().push(TypeNode {
                    file: n.file_path.clone(),
                    members,
                    extends: None,
                });
            }
            _ => {}
        }
    }
    Ok(MemberGraph { index, ts_fields: fields, spans })
}

/// Read the estate: every member store read-only, every first-party unresolved
/// method-form row located in its source and classified. Panics — the run
/// fails — naming a member whose store is absent or refused.
pub fn read_estate(root: &Path) -> Estate {
    let federation = discover(root).expect("the workspace manifest parses").unwrap_or_else(|| {
        panic!("{} is not a Logos workspace — this run never enrols one", root.display())
    });
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("plugin registry loads");
    let mut estate = Estate { members: federation.members.len(), ..Estate::default() };
    let mut refused = Vec::new();

    for member in &federation.members {
        let db = member.root.join(".logos").join("logos.db");
        if !db.is_file() {
            refused.push(format!("{}: no store at {}", member.name, db.display()));
            continue;
        }
        let store = match open_member_store(&db) {
            Ok(store) => store,
            Err(e) => {
                refused.push(format!("{}: {e}", member.name));
                continue;
            }
        };
        estate.stores_read += 1;

        let files =
            store.indexed_files().unwrap_or_else(|e| panic!("{}: files: {e:#}", member.name));
        let ts: BTreeMap<i64, String> = files
            .into_iter()
            .filter(|f| registry.for_path(&f.path).is_some_and(|p| TS_PLUGINS.contains(&p.name())))
            .map(|f| (f.id, f.path))
            .collect();
        if ts.is_empty() {
            continue;
        }
        let refs =
            store.unresolved_refs().unwrap_or_else(|e| panic!("{}: ledger: {e:#}", member.name));
        let mut by_file: BTreeMap<&str, Vec<_>> = BTreeMap::new();
        for r in refs.iter().filter(|r| r.kind == EdgeKind::Calls && r.form == RefForm::Method) {
            let Some(path) = r.file_id.and_then(|id| ts.get(&id)) else { continue };
            let population = Population::of(&member.name, path);
            let t = estate.totals.entry(population).or_default();
            t.0 += 1;
            t.1 += usize::from(r.resolved);
            if population != Population::Vendored && !r.resolved {
                by_file.entry(path.as_str()).or_default().push(r);
            }
        }
        let ts_paths: BTreeSet<String> = ts.values().cloned().collect();
        let MemberGraph { mut index, ts_fields, spans } =
            member_graph(&store, &ts_paths).unwrap_or_else(|e| panic!("{}: {e}", member.name));
        estate.ts_fields += ts_fields;
        if by_file.is_empty() {
            continue;
        }
        // The supertype walk's `extends` heads, read from every first-party
        // declaring file (the plugins record no `Extends` edge for TypeScript).
        let mut extends = BTreeMap::new();
        for path in ts_paths.iter().filter(|p| !is_vendored(&member.name, p)) {
            let (tree, src) = parse_file(&registry, &member.root, path);
            class_extends(&tree, &src, path, &mut extends);
        }
        for (name, nodes) in &mut index.types {
            for node in nodes {
                let key = (node.file.clone().unwrap_or_default(), name.clone());
                node.extends = extends.get(&key).cloned();
            }
        }

        for (path, rows) in by_file {
            let (tree, src) = parse_file(&registry, &member.root, path);
            let all = sites(&tree, &src);
            for r in rows {
                let line = r.line.unwrap_or(0).max(0) as usize;
                let Some(site) = locate(&all, line, &r.target) else {
                    estate.unlocated.push(format!("{}/{path}:{line} .{}()", member.name, r.target));
                    continue;
                };
                let mixed = spans.get(&r.source_symbol).is_some_and(|&(s, e)| {
                    all.iter().any(|o| {
                        o.method == r.target
                            && (s..=e).contains(&(o.line as i64))
                            && o.sub.shape() != site.sub.shape()
                    })
                });
                let inferred = site
                    .init
                    .as_ref()
                    .and_then(|i| i.ty.clone())
                    .map(|ty| judge(&Site { ty: Some(ty), ..site.clone() }, path, &index));
                estate.classified.push(Classified {
                    member: member.name.clone(),
                    path: path.to_string(),
                    population: Population::of(&member.name, path),
                    outcome: judge(site, path, &index),
                    inferred,
                    site: site.clone(),
                    mixed,
                });
            }
        }
    }
    assert!(
        refused.is_empty(),
        "{} member store(s) were refused or absent — the run never migrates a store and never \
         measures a partial workspace. Re-index a private copy with this sprint's binary:\n  {}",
        refused.len(),
        refused.join("\n  ")
    );
    estate
}

// ── Tallies and the report ─────────────────────────────────────────────────

/// The figures one population's verdict lines carry.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub rows: usize,
    pub resolved: usize,
    pub unresolved: usize,
    pub by_shape: BTreeMap<Shape, usize>,
    pub by_sub: BTreeMap<Sub, usize>,
    pub by_outcome: BTreeMap<Outcome, usize>,
    /// The report-only inference extension's outcomes, over the rows it types.
    pub by_inferred: BTreeMap<Outcome, usize>,
    pub mixed: usize,
}

impl Tally {
    pub fn of(e: &Estate, population: Population) -> Self {
        let (rows, resolved) = e.totals.get(&population).copied().unwrap_or_default();
        let mut t = Tally { rows, resolved, unresolved: rows - resolved, ..Tally::default() };
        for c in e.classified.iter().filter(|c| c.population == population) {
            *t.by_shape.entry(c.site.sub.shape()).or_default() += 1;
            *t.by_sub.entry(c.site.sub).or_default() += 1;
            *t.by_outcome.entry(c.outcome).or_default() += 1;
            if let Some(o) = c.inferred {
                *t.by_inferred.entry(o).or_default() += 1;
            }
            t.mixed += usize::from(c.mixed);
        }
        t
    }

    pub fn classified(&self) -> usize {
        self.by_shape.values().sum()
    }

    fn outcome(&self, o: Outcome) -> usize {
        self.by_outcome.get(&o).copied().unwrap_or(0)
    }

    pub fn would_bind(&self) -> usize {
        Outcome::ALL.iter().filter(|o| o.binds()).map(|o| self.outcome(*o)).sum()
    }

    pub fn ambiguous(&self) -> usize {
        Outcome::ALL.iter().filter(|o| o.ambiguous()).map(|o| self.outcome(*o)).sum()
    }

    /// The four verdict lines the recorded finding must carry, verbatim. The
    /// fourth is the report-only inference extension, stated beside the bound
    /// and never added to it.
    pub fn verdict_lines(&self, population: Population) -> [String; 4] {
        let inferred = |o: Outcome| self.by_inferred.get(&o).copied().unwrap_or(0);
        let label = population.label();
        let shapes = Shape::ALL
            .iter()
            .map(|s| format!("{} {}", s.label(), self.by_shape.get(s).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(" · ");
        [
            format!(
                "DENOMINATOR {label}: method-form Calls rows {}, resolved {}, unresolved {}",
                self.rows, self.resolved, self.unresolved
            ),
            format!("SHAPES {label}: {shapes} (sum {})", self.classified()),
            format!(
                "UPPER BOUND {label}: would bind {} of {} (self-tie {} · cross-class {} · via \
                 supertype {}) · ambiguous {} (two types {} · two callables {})",
                self.would_bind(),
                self.unresolved,
                self.outcome(Outcome::SelfTie),
                self.outcome(Outcome::CrossClass),
                self.outcome(Outcome::Inherited),
                self.ambiguous(),
                self.outcome(Outcome::TypeAmbiguous),
                self.outcome(Outcome::OverloadAmbiguous),
            ),
            format!(
                "INFERENCE EXTENSION {label} (not the declared-type rule): of {} untyped this.f \
                 rows, {} have an inject(T)/new T() initialiser; would bind {} more (self-tie {} · \
                 cross-class {} · via supertype {}) · ambiguous {}",
                self.by_sub.get(&Sub::ThisFieldUntyped).copied().unwrap_or(0),
                self.by_inferred.values().sum::<usize>(),
                Outcome::ALL.iter().filter(|o| o.binds()).map(|o| inferred(*o)).sum::<usize>(),
                inferred(Outcome::SelfTie),
                inferred(Outcome::CrossClass),
                inferred(Outcome::Inherited),
                inferred(Outcome::TypeAmbiguous) + inferred(Outcome::OverloadAmbiguous),
            ),
        ]
    }
}

fn report_population(e: &Estate, population: Population) -> Tally {
    let t = Tally::of(e, population);
    println!("\n== {} ==", population.label());
    for line in t.verdict_lines(population) {
        println!("{line}");
    }
    println!("  rows whose caller holds a same-named site of another shape: {}", t.mixed);
    println!("  sub-shapes:");
    for sub in Sub::ALL {
        let n = t.by_sub.get(&sub).copied().unwrap_or(0);
        if n > 0 {
            println!("    {:<16} {:<48} {n:>4}", sub.shape().label(), sub.label());
        }
    }
    println!("  declared-type rule, shape x outcome:");
    for shape in Shape::ALL {
        let mut cells = BTreeMap::new();
        for c in e
            .classified
            .iter()
            .filter(|c| c.population == population && c.site.sub.shape() == shape)
        {
            *cells.entry(c.outcome).or_insert(0usize) += 1;
        }
        let row =
            cells.iter().map(|(o, n)| format!("{} {n}", o.label())).collect::<Vec<_>>().join(" · ");
        println!("    {:<30} {row}", shape.label());
    }
    let of = |pred: &dyn Fn(&Classified) -> Option<String>| {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for c in e.classified.iter().filter(|c| c.population == population) {
            if let Some(k) = pred(c) {
                *m.entry(k).or_default() += 1;
            }
        }
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(" · ")
    };
    println!(
        "  types named by the 'not declared in the member' misses: {}",
        of(&|c| (c.outcome == Outcome::ExternalType).then(|| c
            .site
            .ty
            .clone()
            .unwrap_or_default()))
    );
    println!(
        "  untyped this.f receivers by initialiser: {}",
        of(&|c| c.site.init.as_ref().map(|i| i.label.clone()))
    );
    println!(
        "  inference extension outcomes: {}",
        of(&|c| c.inferred.map(|o| o.label().to_string()))
    );
    println!(
        "  receivers of the global rows (declared nowhere in the file): {}",
        of(&|c| (c.site.sub == Sub::Global).then(|| c.site.receiver.clone()))
    );
    println!("  binding and ambiguous rows:");
    for c in e
        .classified
        .iter()
        .filter(|c| c.population == population && (c.outcome.binds() || c.outcome.ambiguous()))
    {
        println!(
            "    {:<36} {}/{}:{}  {}.{}()  : {}",
            c.outcome.label(),
            c.member,
            c.path,
            c.site.line,
            c.site.receiver,
            c.site.method,
            c.site.ty.as_deref().unwrap_or("-"),
        );
    }
    t
}

/// Measure the residue over the reference estate and hold it to the recorded
/// finding.
///
/// Skips — reporting VOID — without `LOGOS_REF_WORKSPACE`, like every estate
/// run in this binary; that is why every classifier the figures rest on is
/// pinned by the always-run fixtures in [`tests`], and why this run must be
/// shown explicitly: it is invisible to `gate.sh` and to CI.
#[test]
fn measure_ts_method_residue_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "VOID: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference workspace, \
             re-indexed with this sprint's binary> to run the S-479 TypeScript method-call \
             residue measurement. A run that sees no estate reports VOID, never zero (see \
             ts_method_residue_finding.txt for the recorded result)."
        );
        return;
    };
    let estate = read_estate(&root);
    if let Some(reason) = void_reason(&estate) {
        panic!("VOID: {reason}. Nothing is recorded from this run.");
    }
    println!(
        "S-479 over {} — {} manifest members, {} stores read (read-only); vendored rows {} \
         (resolved {}), not classified. Filing baseline (CR-154 §3.1): {} of {} resolved.",
        root.display(),
        estate.members,
        estate.stores_read,
        estate.totals.get(&Population::Vendored).map_or(0, |t| t.0),
        estate.totals.get(&Population::Vendored).map_or(0, |t| t.1),
        FILING_BASELINE.0,
        FILING_BASELINE.1,
    );
    let ts = report_population(&estate, Population::FirstPartyTs);
    let js = report_population(&estate, Population::FirstPartyJs);

    assert!(
        estate.unlocated.is_empty(),
        "{} first-party row(s) record a line holding no method-form site of that name — the \
         harness's site rule has drifted from references.scm. VOID, not a finding:\n  {}",
        estate.unlocated.len(),
        estate.unlocated.join("\n  ")
    );
    for (t, population) in [(&ts, Population::FirstPartyTs), (&js, Population::FirstPartyJs)] {
        assert_eq!(
            t.classified(),
            t.unresolved,
            "{}: the shape counts must sum to the unresolved denominator",
            population.label()
        );
        assert_eq!(t.by_outcome.values().sum::<usize>(), t.unresolved, "every row has one outcome");
        for line in t.verdict_lines(population) {
            assert!(
                RECORDED_FINDING.contains(&line),
                "the recorded finding does not carry this run's line `{line}` — record the new \
                 figures in ts_method_residue_finding.txt, never bend the classifier to \
                 reproduce the old ones",
            );
        }
    }
}

// ── Always-run classifier fixtures ─────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(path: &str, src: &str) -> Tree {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        let plugin = registry.for_path(path).expect("a TypeScript-plugin path");
        let mut parser = Parser::new();
        parser.set_language(plugin.language()).expect("language");
        parser.parse(src, None).expect("tree")
    }

    /// The single site calling `method` in `src` (asserted single).
    fn site(src: &str, method: &str) -> Site {
        let found: Vec<Site> = sites(&parse("app/x.ts", src), src)
            .into_iter()
            .filter(|s| s.method == method)
            .collect();
        assert_eq!(found.len(), 1, "fixture holds exactly one .{method}() site: {found:?}");
        found.into_iter().next().unwrap()
    }

    fn shape_of(src: &str, method: &str) -> (Shape, Sub, Option<String>) {
        let s = site(src, method);
        (s.sub.shape(), s.sub, s.ty)
    }

    const SERVICE: &str = r#"
import { Injectable } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { StatsService } from './stats.service';
import { Builder } from '../builder';
import * as helpers from './helpers';
const util = require('./util');
const lodash = require('lodash');

@Injectable()
export class Wizard {
  private api: StatsService | null = null;
  count = signal(0);
  constructor(private readonly svc: StatsService, private http: HttpClient, plain: Other) {}
  load(local: LocalType) {
    this.svc.fetchP01();
    this.api!.fetchP02();
    this.count.set(1);
    this.missing.go();
    this.refresh();
    this.http.get('/x').pipe();
    Builder.build();
    util.run();
    helpers.help();
    console.log('x');
    lodash.chunk();
    local.walk();
    let y = 1;
    y.toFixed();
    this.a.b.deep();
    new Date().getTime();
    Wizard.create();
  }
}
"#;

    // ── One fixture per receiver shape ─────────────────────────────────────

    #[test]
    fn a_typed_field_or_parameter_property_receiver_is_the_this_field_shape() {
        assert_eq!(
            shape_of(SERVICE, "fetchP01"),
            (Shape::ThisField, Sub::ThisFieldTyped, Some("StatsService".into())),
            "a constructor parameter property carries its declared type"
        );
        assert_eq!(
            shape_of(SERVICE, "fetchP02"),
            (Shape::ThisField, Sub::ThisFieldTyped, Some("StatsService".into())),
            "a field typed `T | null`, read through `!`, carries T"
        );
        let ro = "class K { constructor(readonly ro: Foo, override ov: Bar) {} m() { this.ro.a(); this.ov.b(); } }";
        assert_eq!(
            shape_of(ro, "a"),
            (Shape::ThisField, Sub::ThisFieldTyped, Some("Foo".into())),
            "`readonly` alone, with no accessibility modifier, makes a parameter property"
        );
        assert_eq!(shape_of(ro, "b").1, Sub::ThisFieldTyped, "so does `override` alone");
    }

    #[test]
    fn an_untyped_or_undeclared_field_receiver_is_chained_untyped() {
        assert_eq!(shape_of(SERVICE, "set"), (Shape::ChainedUntyped, Sub::ThisFieldUntyped, None));
        assert_eq!(
            shape_of(SERVICE, "go"),
            (Shape::ChainedUntyped, Sub::ThisFieldUndeclared, None)
        );
    }

    #[test]
    fn a_relatively_imported_receiver_is_the_imported_shape() {
        assert_eq!(
            shape_of(SERVICE, "build"),
            (Shape::Imported, Sub::RelativeImport, Some("Builder".into()))
        );
        assert_eq!(
            shape_of(SERVICE, "run"),
            (Shape::Imported, Sub::RelativeImport, Some("util".into()))
        );
        assert_eq!(
            shape_of(SERVICE, "help"),
            (Shape::Imported, Sub::RelativeImport, Some("helpers".into()))
        );
    }

    #[test]
    fn a_package_import_or_an_undeclared_name_is_the_library_global_shape() {
        assert_eq!(shape_of(SERVICE, "log"), (Shape::LibraryGlobal, Sub::Global, None));
        assert_eq!(
            shape_of(SERVICE, "chunk"),
            (Shape::LibraryGlobal, Sub::PackageImport, Some("lodash".into()))
        );
        let s = "import { TestBed } from '@angular/core/testing';\nTestBed.configureTestingModule({});\ncy.get('x');\n";
        assert_eq!(shape_of(s, "configureTestingModule").1, Sub::PackageImport);
        assert_eq!(shape_of(s, "get").1, Sub::Global);
    }

    #[test]
    fn every_other_receiver_is_chained_untyped_with_its_sub_shape() {
        assert_eq!(shape_of(SERVICE, "pipe"), (Shape::ChainedUntyped, Sub::CallResult, None));
        assert_eq!(shape_of(SERVICE, "deep"), (Shape::ChainedUntyped, Sub::MemberChain, None));
        assert_eq!(
            shape_of(SERVICE, "refresh"),
            (Shape::ChainedUntyped, Sub::This, Some("Wizard".into()))
        );
        assert_eq!(
            shape_of(SERVICE, "walk"),
            (Shape::ChainedUntyped, Sub::LocalTyped, Some("LocalType".into()))
        );
        assert_eq!(shape_of(SERVICE, "toFixed"), (Shape::ChainedUntyped, Sub::LocalUntyped, None));
        assert_eq!(
            shape_of(SERVICE, "getTime"),
            (Shape::ChainedUntyped, Sub::Other, Some("Date".into()))
        );
        assert_eq!(
            shape_of(SERVICE, "create"),
            (Shape::ChainedUntyped, Sub::LocalTypeName, Some("Wizard".into()))
        );
    }

    #[test]
    fn a_plain_constructor_parameter_is_not_a_field() {
        let s = "class K { constructor(plain: Other) { plain.x(); } m() { this.plain.y(); } }";
        assert_eq!(
            shape_of(s, "x").1,
            Sub::LocalTyped,
            "inside the constructor it is a typed local"
        );
        assert_eq!(shape_of(s, "y").1, Sub::ThisFieldUndeclared, "it never becomes `this.plain`");
    }

    #[test]
    fn super_takes_the_extends_type_and_a_this_call_the_enclosing_class() {
        let s = "class A extends ng.Base { m() { super.init(); this.n(); } }";
        assert_eq!(shape_of(s, "init"), (Shape::ChainedUntyped, Sub::Super, Some("Base".into())));
        assert_eq!(shape_of(s, "n"), (Shape::ChainedUntyped, Sub::This, Some("A".into())));
    }

    #[test]
    fn the_type_head_rule_reads_one_named_type_and_refuses_the_rest() {
        let s = "class K {\n a: Foo<Bar>;\n b: ns.Qual;\n c: string;\n d: Foo | Bar;\n e: () => void;\n f: any;\n g?: Opt | undefined;\n m() { this.a.x1(); this.b.x2(); this.c.x3(); this.d.x4(); this.e.x5(); this.f.x6(); this.g.x7(); }\n}";
        let heads: Vec<_> = (1..=7).map(|i| shape_of(s, &format!("x{i}")).2).collect();
        assert_eq!(
            heads,
            vec![
                Some("Foo".into()),
                Some("Qual".into()),
                Some("string".into()),
                None,
                None,
                None,
                Some("Opt".into()),
            ]
        );
    }

    #[test]
    fn a_local_declared_with_two_types_has_none() {
        let s = "function f(a: Foo) { a.x(); }\nfunction g(a: Bar) { a.y(); }\nfunction h(b: Foo) { let b2 = b; b.z(); }\nfunction i() { const b = make(); b.w(); }";
        assert_eq!(shape_of(s, "x").1, Sub::LocalUntyped, "two disagreeing types poison the name");
        assert_eq!(
            shape_of(s, "z"),
            (Shape::ChainedUntyped, Sub::LocalTyped, Some("Foo".into())),
            "an unannotated same-name declaration never poisons the annotated one (toward binding)"
        );
    }

    #[test]
    fn an_initialiser_names_its_type_through_generics_and_qualified_names() {
        let s = "class K {\n  q = inject(QueryService<A, B>);\n  r = new ns.Repo();\n  t = new Box<T>();\n  m() { this.q.a(); this.r.b(); this.t.c(); new ns.Repo().d(); }\n}";
        let init = |m: &str| site(s, m).init.and_then(|i| i.ty);
        assert_eq!(init("a"), Some("QueryService".into()), "inject(T<A, B>) names T");
        assert_eq!(init("b"), Some("Repo".into()), "new ns.T() names T");
        assert_eq!(init("c"), Some("Box".into()), "new T<U>() names T");
        assert_eq!(site(s, "d").ty, Some("Repo".into()), "a `new ns.T()` receiver names T");
    }

    // ── Site location ───────────────────────────────────────────────────────

    #[test]
    fn a_row_is_located_at_its_recorded_line_by_the_first_same_named_site() {
        let src = "a.m(b.m());\nc.n();\n";
        let all = sites(&parse("app/x.ts", src), src);
        assert_eq!(locate(&all, 1, "m").map(|s| s.receiver.as_str()), Some("a"));
        assert_eq!(locate(&all, 2, "n").map(|s| s.receiver.as_str()), Some("c"));
        assert_eq!(locate(&all, 2, "m"), None, "a line holding no such site is unlocated");
        assert_eq!(locate(&all, 3, "n"), None);
    }

    #[test]
    fn a_multi_line_chain_is_located_at_the_method_name_line() {
        let src = "this.svc\n  .load()\n  .pipe(map(x => x))\n  .subscribe();\n";
        let all = sites(&parse("app/x.ts", src), src);
        assert_eq!(locate(&all, 3, "pipe").map(|s| s.sub), Some(Sub::CallResult));
        assert_eq!(locate(&all, 2, "load").map(|s| s.sub), Some(Sub::ThisFieldUndeclared));
    }

    #[test]
    fn a_tsx_file_and_an_optional_chain_classify_the_same_way() {
        let src = "import { Api } from './api';\nexport function C() { Api?.go(); return <div onClick={() => Api.stop()} />; }\n";
        let all = sites(&parse("app/c.tsx", src), src);
        assert_eq!(locate(&all, 2, "go").map(|s| s.sub), Some(Sub::RelativeImport));
        assert_eq!(locate(&all, 2, "stop").map(|s| s.sub), Some(Sub::RelativeImport));
    }

    // ── The first-party split ───────────────────────────────────────────────

    #[test]
    fn the_split_is_s477s_path_rule_and_its_near_misses_stay_first_party() {
        assert_eq!(Population::of("pec-agid", "src/app/a.service.ts"), Population::FirstPartyTs);
        assert_eq!(Population::of("pec-agid", "src/app/a.tsx"), Population::FirstPartyTs);
        assert_eq!(Population::of("webmail", "selenium/BasePage.js"), Population::FirstPartyJs);
        assert_eq!(Population::of("webmail", "plugins/x/y.js"), Population::Vendored);
        assert_eq!(Population::of("webmail", "skins/elastic/ui.js"), Population::Vendored);
        assert_eq!(Population::of("styleguide", "src/app.ts"), Population::Vendored);
        assert_eq!(Population::of("any", "lib/jquery.min.js"), Population::Vendored);
        // Near misses: one character from each rule.
        assert_eq!(Population::of("webmail", "pluginsx/y.js"), Population::FirstPartyJs);
        assert_eq!(Population::of("webmail", "src/plugins/y.js"), Population::FirstPartyJs);
        assert_eq!(Population::of("other", "plugins/y.js"), Population::FirstPartyJs);
        assert_eq!(Population::of("styleguide-v2", "src/app.ts"), Population::FirstPartyTs);
        assert_eq!(Population::of("any", "lib/minimal.js"), Population::FirstPartyJs);
        assert_eq!(Population::of("any", "lib/x.minimal.js"), Population::FirstPartyJs);
        assert_eq!(Population::of("any", "min.d/x.ts"), Population::FirstPartyTs);
    }

    // ── The upper bound ─────────────────────────────────────────────────────

    fn ty(file: &str, members: &[(&str, NodeKind)]) -> TypeNode {
        let mut m: BTreeMap<String, Vec<NodeKind>> = BTreeMap::new();
        for (name, kind) in members {
            m.entry((*name).into()).or_default().push(*kind);
        }
        TypeNode { file: Some(file.into()), members: m, extends: None }
    }

    fn sub(file: &str, sup: &str, members: &[(&str, NodeKind)]) -> TypeNode {
        TypeNode { extends: Some(sup.into()), ..ty(file, members) }
    }

    fn index() -> TypeIndex {
        let mut i = TypeIndex::default();
        i.types.insert(
            "StatsService".into(),
            vec![ty(
                "src/stats.service.ts",
                &[
                    ("fetchP01", NodeKind::Method),
                    ("api", NodeKind::Field),
                    ("twice", NodeKind::Method),
                    ("twice", NodeKind::Method),
                ],
            )],
        );
        i.types.insert(
            "Wizard".into(),
            vec![ty("app/x.ts", &[("refresh", NodeKind::Method), ("count", NodeKind::Field)])],
        );
        i.types.insert(
            "Dup".into(),
            vec![ty("a.ts", &[("go", NodeKind::Method)]), ty("b.ts", &[("go", NodeKind::Method)])],
        );
        i.types.insert("Page".into(), vec![sub("p.js", "BasePage", &[("open", NodeKind::Method)])]);
        i.types.insert(
            "BasePage".into(),
            vec![sub("b.js", "Root", &[("click", NodeKind::Method), ("url", NodeKind::Field)])],
        );
        i.types.insert("Root".into(), vec![ty("r.js", &[("wait", NodeKind::Method)])]);
        i.types.insert("Widget".into(), vec![sub("w.ts", "LibraryBase", &[])]);
        i.types.insert("ToDup".into(), vec![sub("d.ts", "Dup", &[])]);
        i.types.insert("Loop".into(), vec![sub("l.ts", "Loop", &[])]);
        i
    }

    #[test]
    fn a_missing_member_is_looked_up_through_the_in_member_extends_chain() {
        let i = index();
        let j = |m, t| judge(&at(m, Some(t), Some("Page")), "p.js", &i);
        assert_eq!(j("open", "Page"), Outcome::SelfTie, "the type's own method is not inherited");
        assert_eq!(j("click", "Page"), Outcome::Inherited, "one hop");
        assert_eq!(j("wait", "Page"), Outcome::Inherited, "two hops");
        assert_eq!(j("url", "Page"), Outcome::MemberIsField, "a supertype's field is not callable");
        assert_eq!(j("gone", "Page"), Outcome::MemberMissing, "the chain ends in the member");
        assert_eq!(j("render", "Widget"), Outcome::SupertypeExternal, "a library base class");
        assert_eq!(j("go", "ToDup"), Outcome::TypeAmbiguous, "an ambiguous supertype never binds");
        assert_eq!(j("spin", "Loop"), Outcome::MemberMissing, "a cycle ends after the hop bound");
    }

    #[test]
    fn extends_heads_are_read_per_file_and_class() {
        let src = "class A extends ng.Base {}\nclass B {}\nconst C = class extends A {};\nabstract class D extends A {}\n";
        let mut out = BTreeMap::new();
        class_extends(&parse("x.ts", src), src, "x.ts", &mut out);
        let got: Vec<_> = out.iter().map(|((f, c), s)| format!("{f}:{c}<{s}")).collect();
        assert_eq!(
            got,
            vec!["x.ts:A<Base", "x.ts:D<A"],
            "a class expression has no name to key on"
        );
    }

    fn at(method: &str, ty: Option<&str>, class: Option<&str>) -> Site {
        Site {
            line: 1,
            at: 0,
            method: method.into(),
            sub: Sub::Other,
            receiver: "r".into(),
            ty: ty.map(Into::into),
            class: class.map(Into::into),
            init: None,
        }
    }

    #[test]
    fn the_declared_type_rule_binds_only_one_callable_on_one_member_type() {
        let i = index();
        let j = |m, t, c| judge(&at(m, t, c), "app/x.ts", &i);
        assert_eq!(j("fetchP01", Some("StatsService"), Some("Wizard")), Outcome::CrossClass);
        assert_eq!(j("refresh", Some("Wizard"), Some("Wizard")), Outcome::SelfTie);
        assert_eq!(j("twice", Some("StatsService"), None), Outcome::OverloadAmbiguous);
        assert_eq!(j("go", Some("Dup"), None), Outcome::TypeAmbiguous);
        assert_eq!(j("api", Some("StatsService"), None), Outcome::MemberIsField);
        assert_eq!(j("count", Some("Wizard"), Some("Wizard")), Outcome::MemberIsField);
        assert_eq!(j("absent", Some("StatsService"), None), Outcome::MemberMissing);
        assert_eq!(j("get", Some("HttpClient"), None), Outcome::ExternalType);
        assert_eq!(j("pipe", None, None), Outcome::NoEvidence);
    }

    #[test]
    fn a_self_tie_needs_the_callers_own_class_in_the_callers_own_file() {
        let i = index();
        assert_eq!(
            judge(&at("refresh", Some("Wizard"), Some("Wizard")), "app/other.ts", &i),
            Outcome::CrossClass,
            "a same-named class in another file is not the caller's own"
        );
        assert_eq!(
            judge(&at("refresh", Some("Wizard"), Some("Other")), "app/x.ts", &i),
            Outcome::CrossClass
        );
    }

    #[test]
    fn every_sub_shape_has_exactly_one_shape_and_every_shape_is_reachable() {
        let reached: BTreeSet<Shape> = Sub::ALL.iter().map(|s| s.shape()).collect();
        assert_eq!(reached, Shape::ALL.into_iter().collect());
        assert_eq!(Sub::ALL.iter().collect::<BTreeSet<_>>().len(), Sub::ALL.len());
    }

    // ── Tallies, VOID ───────────────────────────────────────────────────────

    fn classified(population: Population, sub: Sub, outcome: Outcome) -> Classified {
        Classified {
            member: "m".into(),
            path: "p.ts".into(),
            population,
            site: Site { sub, ..at("x", None, None) },
            outcome,
            inferred: None,
            mixed: false,
        }
    }

    #[test]
    fn the_shape_counts_sum_to_the_unresolved_denominator_per_population() {
        let mut e = Estate::default();
        e.totals.insert(Population::FirstPartyTs, (5, 2));
        e.totals.insert(Population::FirstPartyJs, (1, 0));
        e.classified = vec![
            classified(Population::FirstPartyTs, Sub::ThisFieldTyped, Outcome::CrossClass),
            classified(Population::FirstPartyTs, Sub::Global, Outcome::NoEvidence),
            classified(Population::FirstPartyTs, Sub::This, Outcome::SelfTie),
            classified(Population::FirstPartyJs, Sub::CallResult, Outcome::NoEvidence),
        ];
        let t = Tally::of(&e, Population::FirstPartyTs);
        assert_eq!((t.rows, t.resolved, t.unresolved, t.classified()), (5, 2, 3, 3));
        assert_eq!((t.would_bind(), t.ambiguous()), (2, 0));
        let [d, s, u, _] = t.verdict_lines(Population::FirstPartyTs);
        assert_eq!(
            d,
            "DENOMINATOR first-party .ts/.tsx: method-form Calls rows 5, resolved 2, unresolved 3"
        );
        assert_eq!(
            s,
            "SHAPES first-party .ts/.tsx: this.<field> (declared type) 1 · imported symbol 0 · \
             library/global 1 · chained/untyped 1 (sum 3)"
        );
        assert_eq!(
            u,
            "UPPER BOUND first-party .ts/.tsx: would bind 2 of 3 (self-tie 1 · cross-class 1 · via \
             supertype 0) · ambiguous 0 (two types 0 · two callables 0)"
        );
        assert_eq!(
            Tally::of(&e, Population::FirstPartyJs).classified(),
            1,
            "populations never mix"
        );
    }

    #[test]
    fn an_estate_blind_run_is_void_never_zero() {
        let blind = Estate::default();
        assert!(void_reason(&blind).is_some_and(|r| r.contains("no first-party")));
        let mut stale = Estate::default();
        stale.totals.insert(Population::FirstPartyTs, (10, 1));
        assert!(void_reason(&stale).is_some_and(|r| r.contains("predate S-477")));
        stale.ts_fields = 1;
        assert_eq!(void_reason(&stale), None);
    }

    #[test]
    fn the_recorded_finding_carries_the_filing_baseline_it_reconciles_against() {
        let (resolved, rows) = FILING_BASELINE;
        assert!(RECORDED_FINDING.contains(&format!("{resolved} of {rows}")));
    }
}
