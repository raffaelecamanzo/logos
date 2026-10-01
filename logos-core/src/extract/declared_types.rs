//! A member's **declared types** as member-local facts (S-472, [CR-152] §3.2 B,
//! [ADR-70] decision point 1, following [ADR-69] decision point 1's precedent).
//!
//! A member declares a fully-qualified type in one of two ways, and this module
//! reads both and nothing else. It owns no workspace index: matching another
//! member's import against these facts is the federation's work ([ADR-70]
//! point 2), in memory, never here.
//!
//! # From source
//!
//! Every **top-level** class, interface, enum or record of a package-shaped
//! language (Java and Kotlin, whose descriptors declare `[package_modules]`) is
//! one fact ([`source_types`]). A top-level type is a type node the file's own
//! module contains — a nested type is reached through its outer type and is not
//! a declaration of the package. Its name is the one derivation
//! [`PackageLayout::type_fqn`] gives, never a second split of the path
//! ([FR-RS-01]): the file's package by its location, plus the type's name.
//!
//! The location is what the binder keys the file by, so it is what an import
//! of the type can bind to — but the file's own `package` statement is what the
//! compiler names the type by. When the two **disagree** the fact is recorded
//! **refused**, with a reason naming both, and carries no name: resolving it to
//! the path would publish a name the compiler never gave the type
//! ([NFR-RA-05]). A file with no `package` statement declares the default
//! package, which agrees only with a file directly under its source root.
//!
//! Each fact carries its node's [`LogosSymbol`] and the **tree** it sits in —
//! `test` when the source root that keyed the file is a test root
//! (`src/test/java`), otherwise `main` — so a consumer can keep a test fixture
//! from owning a production name.
//!
//! # From Avro
//!
//! Every `record` and `enum` an `.avsc` schema declares is one fact
//! ([`schema_facts`]), nested named types included, under the Avro naming rule:
//! a `name` holding a `.` is already full; otherwise it takes its own
//! `namespace`, else the nearest enclosing named type's. The schema's path is
//! its provenance. A schema that is not valid JSON, or that declares a named
//! type without a valid `name`, is recorded **malformed** with a reason and
//! yields no fact at all — no name is guessed from the file name or from the
//! parts that did parse.
//!
//! [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
//! [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
//! [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::HashSet;

use serde_json::{Map, Value};
use tree_sitter::Node;

use super::Facts;
use crate::model::{EdgeKind, LogosSymbol, NodeKind};
use crate::resolve::package_key::PackageLayout;

/// The capture a package-shaped language's `symbols` query names its `package`
/// statement's name with. Its group is not [`super::SYMBOL_CAPTURE_GROUP`], so
/// the declaration walk never reads it as a node.
pub(crate) const PACKAGE_CAPTURE: &str = "package.name";

/// The extension of an Avro schema file.
const AVRO_SCHEMA_EXTENSION: &str = ".avsc";

/// `true` when the project-relative `rel` names an Avro schema: its basename
/// ends in `.avsc` and has a stem. Exact and case-sensitive, as the Avro
/// tooling's own default include pattern is — `x.avsc.bak` and `x.AVSC` are
/// not schemas.
pub fn is_avro_schema(rel: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.len() > AVRO_SCHEMA_EXTENSION.len() && base.ends_with(AVRO_SCHEMA_EXTENSION)
}

/// The kind of a declared type — the persisted vocabulary
/// (`declared_types.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    /// A class — in Java also a `record`, which the grammar captures as one; in
    /// Kotlin also an `object` and an `enum class`.
    Class,
    Interface,
    Enum,
    /// An Avro `record`.
    Record,
}

impl TypeKind {
    /// The persisted token.
    pub fn as_str(self) -> &'static str {
        match self {
            TypeKind::Class => "class",
            TypeKind::Interface => "interface",
            TypeKind::Enum => "enum",
            TypeKind::Record => "record",
        }
    }

    /// The kind a source node declares, or `None` for a node that is not a
    /// class, interface or enum.
    fn of_node(kind: NodeKind) -> Option<Self> {
        match kind {
            NodeKind::Class => Some(TypeKind::Class),
            NodeKind::Interface => Some(TypeKind::Interface),
            NodeKind::Enum => Some(TypeKind::Enum),
            _ => None,
        }
    }
}

/// Which tree of its member a source type is declared in
/// (`declared_types.tree`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTree {
    Main,
    Test,
}

impl SourceTree {
    /// The persisted token.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceTree::Main => "main",
            SourceTree::Test => "test",
        }
    }
}

/// One top-level type a source file declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceType {
    /// The type's own name, as declared.
    pub name: String,
    /// The dotted fully-qualified name, or why none is recorded — the file's
    /// `package` statement disagrees with its directory.
    pub fqn: Result<String, String>,
    pub kind: TypeKind,
    /// The declaring node's symbol.
    pub symbol: LogosSymbol,
    pub tree: SourceTree,
}

/// The top-level types a file declares, in node order, read off its extracted
/// `facts` (its path, nodes and `Contains` edges).
///
/// `package` is the file's `package` statement as its grammar captured it
/// ([`package_name`]). Empty for a file whose language is not package-shaped
/// under `layout`, and for one that declares no class, interface or enum at
/// file scope.
pub fn source_types(facts: &Facts, package: Option<&str>, layout: &PackageLayout) -> Vec<SourceType> {
    let (path, nodes, edges) = (facts.path.as_str(), &facts.nodes, &facts.edges);
    let Some(directory) = layout.package_of(path) else {
        return Vec::new();
    };
    // The file's own module: the one Module node no other node contains.
    let contained: HashSet<&LogosSymbol> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| &e.target)
        .collect();
    let Some(module) = nodes
        .iter()
        .find(|n| n.kind == NodeKind::Module && !contained.contains(&n.symbol))
    else {
        return Vec::new();
    };
    let top_level: HashSet<&LogosSymbol> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains && e.source == module.symbol)
        .map(|e| &e.target)
        .collect();
    let directory = directory.join(".");
    let declared = package.unwrap_or("");
    let tree = tree_of(path, layout);
    nodes
        .iter()
        .filter(|n| top_level.contains(&n.symbol))
        .filter_map(|n| {
            let kind = TypeKind::of_node(n.kind)?;
            let fqn = if declared == directory {
                layout
                    .type_fqn(path, &n.name)
                    .map(|segments| segments.join("."))
                    .ok_or_else(|| "the file's language is not package-shaped".to_string())
            } else {
                Err(package_disagreement(package, &directory))
            };
            Some(SourceType {
                name: n.name.clone(),
                fqn,
                kind,
                symbol: n.symbol.clone(),
                tree,
            })
        })
        .collect()
}

/// The refusal reason for a `package` statement that disagrees with the
/// directory the file sits in.
fn package_disagreement(declared: Option<&str>, directory: &str) -> String {
    let declared = match declared {
        Some(p) => format!("declares package `{p}`"),
        None => "declares no package".to_string(),
    };
    let directory = if directory.is_empty() {
        "the default package".to_string()
    } else {
        format!("package `{directory}`")
    };
    format!("package-mismatch: the file {declared} but its directory is {directory}")
}

/// The tree a package-shaped file sits in: `test` when the source root that
/// keyed it has a `test` segment, `main` for any other root. A file outside
/// every root is `test` when its path is test-shaped by the one naming
/// convention the rest of the product uses ([`crate::navigate::is_test_path`]).
fn tree_of(path: &str, layout: &PackageLayout) -> SourceTree {
    let test = match layout.source_root(path) {
        Some(root) => root.iter().any(|s| s == "test"),
        None => crate::navigate::is_test_path(path),
    };
    if test {
        SourceTree::Test
    } else {
        SourceTree::Main
    }
}

/// Record the file's package from a `symbols` capture: `true` when `capture` is
/// the [`PACKAGE_CAPTURE`] (so the declaration walk skips it), and `package`
/// takes its name if not already set — the first statement only, a second
/// being a syntax error.
pub(crate) fn note_package(
    package: &mut Option<String>,
    capture: &str,
    node: Node<'_>,
    source: &[u8],
) -> bool {
    if capture != PACKAGE_CAPTURE {
        return false;
    }
    if package.is_none() {
        *package = package_name(node, source);
    }
    true
}

/// The dotted name a captured `package` name node spells: its identifier
/// leaves joined by `.`, whatever the grammar nests them in (Java's
/// `scoped_identifier`, Kotlin's `qualified_identifier`) and whatever
/// whitespace or comment sits between them. Kotlin's backtick escapes are
/// dropped, as the compiler drops them. `None` when the node holds no
/// identifier or is not UTF-8.
pub(crate) fn package_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut parts = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.is_extra() {
            continue;
        }
        if n.named_child_count() == 0 {
            if n.is_named() {
                parts.push(n.utf8_text(source).ok()?.replace('`', ""));
            }
            continue;
        }
        let mut cursor = n.walk();
        let children: Vec<Node<'_>> = n.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    (!parts.is_empty() && parts.iter().all(|p| !p.is_empty())).then(|| parts.join("."))
}

// ── Avro ─────────────────────────────────────────────────────────────────────

/// What reading one schema file concluded (`avro_schemas.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStatus {
    /// Parsed; its types are in [`SchemaFacts::types`].
    Read,
    /// Read, but not a valid Avro schema; it yields no type.
    Malformed,
    /// Could not be read from disk as UTF-8; it yields no type.
    Unreadable,
}

impl SchemaStatus {
    /// The persisted token.
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaStatus::Read => "read",
            SchemaStatus::Malformed => "malformed",
            SchemaStatus::Unreadable => "unreadable",
        }
    }
}

/// One `record` or `enum` an Avro schema declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvroType {
    /// The dotted full name (`namespace.name`, or `name` in the null namespace).
    pub fqn: String,
    /// The name's last segment.
    pub name: String,
    /// [`TypeKind::Record`] or [`TypeKind::Enum`].
    pub kind: TypeKind,
}

/// One schema file, read or not — the census denominator is schemas **found**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFacts {
    /// Project-relative path — the types' provenance.
    pub path: String,
    pub status: SchemaStatus,
    /// Why the schema yielded no type; `None` exactly when it was read.
    pub detail: Option<String>,
    /// Its types, in document order; empty unless read.
    pub types: Vec<AvroType>,
}

impl SchemaFacts {
    /// A schema that could not be read from disk.
    pub fn unreadable(path: &str, detail: String) -> Self {
        Self {
            path: path.to_string(),
            status: SchemaStatus::Unreadable,
            detail: Some(detail),
            types: Vec::new(),
        }
    }
}

/// Read one `.avsc` schema's declared records and enums.
pub fn schema_facts(path: &str, text: &str) -> SchemaFacts {
    let walked = serde_json::from_str::<Value>(text)
        .map_err(|e| format!("not valid JSON: {e}"))
        .and_then(|value| {
            let mut types = Vec::new();
            walk_schema(&value, None, &mut types)?;
            let mut seen = HashSet::new();
            match types.iter().find(|t| !seen.insert(t.fqn.as_str())) {
                Some(dup) => Err(format!("the schema defines `{}` twice", dup.fqn)),
                None => Ok(types),
            }
        });
    match walked {
        Ok(types) => SchemaFacts {
            path: path.to_string(),
            status: SchemaStatus::Read,
            detail: None,
            types,
        },
        Err(detail) => SchemaFacts {
            path: path.to_string(),
            status: SchemaStatus::Malformed,
            detail: Some(detail),
            types: Vec::new(),
        },
    }
}

/// Walk one schema **position** — a type name, a union, or a complex type —
/// with `namespace` the enclosing named type's namespace.
///
/// A position is not a field: a record's `fields` are walked as fields, whose
/// `type` is a position, so a field named `x` whose `type` is the string
/// `"record"` is never mistaken for a record named `x`.
fn walk_schema(value: &Value, namespace: Option<&str>, out: &mut Vec<AvroType>) -> Result<(), String> {
    match value {
        Value::Array(union) => union.iter().try_for_each(|branch| walk_schema(branch, namespace, out)),
        Value::Object(map) => walk_object(map, namespace, out),
        // A type name (primitive or a reference to a named type) declares nothing.
        _ => Ok(()),
    }
}

fn walk_object(map: &Map<String, Value>, namespace: Option<&str>, out: &mut Vec<AvroType>) -> Result<(), String> {
    let kind = match map.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        // `{"type": {…}}` wraps a schema in a schema.
        Some(nested @ (Value::Object(_) | Value::Array(_))) => return walk_schema(nested, namespace, out),
        Some(other) => return Err(format!("a schema's `type` is {other}, not a type")),
        None => return Err("a schema object has no `type`".to_string()),
    };
    match kind {
        "record" | "error" | "enum" | "fixed" => {
            let (fqn, own_namespace) = full_name(map, kind, namespace)?;
            let name = fqn.rsplit('.').next().unwrap_or(&fqn).to_string();
            match kind {
                "record" | "error" => {
                    out.push(AvroType { fqn: fqn.clone(), name, kind: TypeKind::Record });
                    let fields = match map.get("fields") {
                        Some(Value::Array(fields)) => fields,
                        _ => return Err(format!("the {kind} `{fqn}` has no `fields` array")),
                    };
                    for field in fields {
                        let Some(field_type) = field.as_object().and_then(|f| f.get("type")) else {
                            return Err(format!("a field of `{fqn}` has no `type`"));
                        };
                        walk_schema(field_type, own_namespace.as_deref(), out)?;
                    }
                }
                "enum" => out.push(AvroType { fqn, name, kind: TypeKind::Enum }),
                // A `fixed` is named, and so validated, but is not a class or enum.
                _ => {}
            }
            Ok(())
        }
        "array" => match map.get("items") {
            Some(items) => walk_schema(items, namespace, out),
            None => Err("an `array` has no `items`".to_string()),
        },
        "map" => match map.get("values") {
            Some(values) => walk_schema(values, namespace, out),
            None => Err("a `map` has no `values`".to_string()),
        },
        // A primitive written as an object (`{"type": "string"}`), or a
        // reference to a named type, declares nothing.
        _ => Ok(()),
    }
}

/// A named type's full name and the namespace its nested types inherit, by
/// the Avro naming rule — or why it has none. Every segment must be a valid
/// Avro name (`[A-Za-z_][A-Za-z0-9_]*`); nothing is repaired or guessed.
fn full_name(
    map: &Map<String, Value>,
    kind: &str,
    enclosing: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let name = match map.get("name") {
        Some(Value::String(name)) => name.as_str(),
        Some(other) => return Err(format!("a `{kind}` has a non-string `name` {other}")),
        None => return Err(format!("a `{kind}` has no `name`")),
    };
    let namespace = match map.get("namespace") {
        None | Some(Value::Null) => enclosing,
        Some(Value::String(ns)) if ns.is_empty() => None,
        Some(Value::String(ns)) => Some(ns.as_str()),
        Some(other) => return Err(format!("the `{kind}` `{name}` has a non-string `namespace` {other}")),
    };
    let fqn = match namespace {
        _ if name.contains('.') => name.to_string(),
        Some(ns) => format!("{ns}.{name}"),
        None => name.to_string(),
    };
    if !fqn.split('.').all(is_avro_name) {
        return Err(format!("the `{kind}` full name `{fqn}` is not a valid Avro name"));
    }
    let own_namespace = fqn.rsplit_once('.').map(|(ns, _)| ns.to_string());
    Ok((fqn, own_namespace))
}

/// One Avro name segment: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_avro_name(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
#[path = "declared_types_tests.rs"]
mod tests;
