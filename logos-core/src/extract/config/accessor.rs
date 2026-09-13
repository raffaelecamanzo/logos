//! The **use-site** half of configuration binding: reading a captured operand
//! as an accessor on a configuration-bound class, and naming the canonical key
//! it resolves to ([FR-WS-19], [NFR-MA-01], [NFR-RA-05], [ADR-64]).
//!
//! [`binding`](super::binding) owns the *declaration* half — which classes are
//! configuration-bound, what prefix each carries, and which property an accessor
//! name yields. This module owns the one question that is left, and it is the
//! question the invocation arm actually asks:
//!
//! ```text
//! restClient.get().uri(mailServerConfigurationApi.getUriGetArchive())
//!                      │                          └─ the accessor NAME
//!                      └─ the RECEIVER, whose declared type this file states
//! ```
//!
//! Answering it needs one fact the declaration half does not hold: the **simple
//! type** the reading file declares for the receiver name. That is what
//! [`DeclaredTypes`] reads, and it is the whole of this module's tree work.
//!
//! # Grammar FIELD names only, never node kinds ([NFR-MA-01])
//!
//! [`binding`](super::binding) is driven by capture names and descriptor rows;
//! it names no grammar node kind at all, and a structural guard proves it. This
//! module cannot be driven by a query — the operand it reads is the node the
//! *invocations* query already captured, and re-capturing it through a second
//! query would make two languages' worth of `.scm` disagree about which node the
//! arm judged. So it reads the parse tree directly, and stays language-neutral a
//! different way: **only tree-sitter field names**, which are the structural
//! roles a grammar author declares, never the node kinds a grammar happens to
//! spell.
//!
//! | field | what it answers |
//! |-------|-----------------|
//! | `name` | the name a node declares, and the accessor a call names |
//! | `type` | the declared type of the node that carries it, or of its parent |
//! | `object` | the receiver a member call reads through |
//! | `function` | the callee of a call whose grammar field-names it that way |
//! | `parameters` / `body` | a node that declares a **callable**, never a value binding — its `name` is skipped |
//!
//! The [NFR-MA-01] structural guard beside the declaration half
//! (`binding_tests::the_interpreter_names_no_jvm_grammar_node_kind`) covers this
//! file too, and its allowlist is empty for both: none of the six field names
//! above is a node kind in either loaded JVM grammar. `type` is the near miss —
//! `tree-sitter-kotlin-ng` declares it in `node-types.json`, but as a supertype
//! rather than a parser node kind — and the guard's own comment records what to
//! do if a grammar bump ever changes that.
//!
//! # Everything unproven resolves to nothing ([NFR-RA-05])
//!
//! Each hop below returns [`None`] rather than a guess, and the arm then leaves
//! the site exactly where it already was — marked as a runtime-composed path.
//! There is no new refusal reason and no new counter: a site this module cannot
//! resolve is indistinguishable from one it was never asked about, which is what
//! makes wiring it in a widening of what is *captured* and never of what is
//! *believed*.
//!
//! [ADR-64]: ../../../../docs/specs/architecture/decisions/ADR-64.md
//! [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
//! [NFR-MA-01]: ../../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::BTreeMap;

use tree_sitter::Node;

use super::binding::PropertiesIndex;
use super::corpus::canonical_key;

/// The grammar field naming what a node declares, and what a member call reads.
const NAME_FIELD: &str = "name";
/// The grammar field naming a declared type.
const TYPE_FIELD: &str = "type";
/// The grammar field naming a member call's receiver.
const OBJECT_FIELD: &str = "object";
/// The grammar field naming a call's callee, where the grammar spells it that
/// way instead of `name`.
const FUNCTION_FIELD: &str = "function";
/// The two fields that mark a node as declaring a **callable**. Such a node
/// carries a `name` and often a `type` (its return type), and binding the two
/// together would register a method's name as a value of its return type — a
/// binding no source makes.
const CALLABLE_FIELDS: [&str; 2] = ["parameters", "body"];

/// Every name this file binds to a declared **simple** type name.
///
/// File-scoped and scope-blind, exactly like the corpus half's module scope: it
/// answers "what type does this file say this name has", not "which binding is
/// live here". That is why a name the file declares under **two different**
/// types resolves to nothing — see [`DeclaredTypes::get`].
#[derive(Debug, Default)]
pub struct DeclaredTypes {
    /// name → the single simple type it is declared with, or [`None`] when the
    /// file declares it with two that disagree.
    by_name: BTreeMap<String, Option<String>>,
}

impl DeclaredTypes {
    /// Read every `name` → declared-type binding in the file.
    ///
    /// The rule, in full: a node that carries a `name` field whose child is a
    /// leaf takes its type from its **own** `type` field, or failing that from
    /// its parent's — one hop, which is what a C-family declarator needs
    /// (`private Foo bar;` field-names the type on the declaration and the name
    /// on the declarator) and what a parameter does not (it carries both). A
    /// node that declares a callable is skipped entirely.
    pub fn build(root: Node<'_>, src: &[u8]) -> Self {
        let mut types = Self::default();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));

            if CALLABLE_FIELDS
                .iter()
                .any(|f| node.child_by_field_name(f).is_some())
            {
                continue;
            }
            let Some(name_node) = node.child_by_field_name(NAME_FIELD) else {
                continue;
            };
            // A leaf, so a qualified or generic name node is not read as though
            // it were the identifier a use site spells.
            if name_node.named_child_count() > 0 {
                continue;
            }
            let Some(name) = name_node.utf8_text(src).ok().map(str::trim) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let declared = node
                .child_by_field_name(TYPE_FIELD)
                .or_else(|| node.parent()?.child_by_field_name(TYPE_FIELD))
                .and_then(|n| n.utf8_text(src).ok())
                .map(simple_type_name)
                .filter(|t| !t.is_empty());
            let Some(declared) = declared else {
                continue;
            };
            types
                .by_name
                .entry(name.to_string())
                // A second, DISAGREEING declaration poisons the entry rather
                // than losing to (or beating) the first: which one a use site
                // sees is a scope question this walk cannot answer, and picking
                // either would be the guess [NFR-RA-05] forbids.
                .and_modify(|held| {
                    if held.as_deref() != Some(declared) {
                        *held = None;
                    }
                })
                .or_insert_with(|| Some(declared.to_string()));
        }
        types
    }

    /// The simple type this file declares for `name`, when it declares exactly
    /// one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.by_name.get(name)?.as_deref()
    }
}

/// The simple name of a possibly-generic, possibly-qualified type:
/// `com.acme.Props<String>` → `Props`.
fn simple_type_name(declared: &str) -> &str {
    let head = declared.split(['<', '[']).next().unwrap_or(declared).trim();
    head.rsplit(['.', ':']).next().unwrap_or(head).trim()
}

/// Whether `text` is a single unqualified identifier — the only receiver shape
/// [`DeclaredTypes`] can answer for.
///
/// A `this.client` or a `Holder.INSTANCE` receiver is not one, and is refused
/// rather than trimmed down to its last segment: the trimming would resolve
/// `holder.api` against a same-named local, which is a different object.
///
/// This is also where a **chained** accessor is refused —
/// `config.getMail().getHost()`, the nested-properties-type ceiling each
/// `properties.scm` records — because the receiver's text is then a whole call
/// expression and no file declares a type for that. Stated here rather than
/// guarded separately: a second, earlier "is the receiver a call" test was
/// written, and mutation-testing showed it could not fail, because a call
/// expression's text is never a bare identifier in any grammar.
fn simple_identifier(text: &str) -> Option<&str> {
    let text = text.trim();
    let mut chars = text.chars();
    let first = chars.next()?;
    (first.is_alphabetic() || first == '_')
        .then_some(text)
        .filter(|t| t.chars().all(|c| c.is_alphanumeric() || c == '_'))
}

/// The `(receiver, callee-name)` of a member call, or [`None`] for any other
/// operand shape — a bare call with no receiver, a plain name, a literal.
///
/// It makes no judgement about the receiver beyond its existence;
/// [`simple_identifier`] is where a receiver this module cannot answer for —
/// qualified, or itself a call — is refused.
fn member_call<'t>(node: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
    let callee = node
        .child_by_field_name(NAME_FIELD)
        .or_else(|| node.child_by_field_name(FUNCTION_FIELD))?;
    Some((node.child_by_field_name(OBJECT_FIELD)?, callee))
}

/// Everything the invocation arm needs to read one operand as a configuration
/// accessor: the member's class index, the reading file's declared types, and
/// the language whose accessor convention judges the name.
///
/// A struct rather than four parameters, for the reason `EmitCtx` gives beside
/// it: these travel together through every call in the arm, and the capture
/// dispatch was already at its argument limit.
pub struct BindingView<'a> {
    /// Every configuration-bound class the member declares.
    pub index: &'a PropertiesIndex,
    /// The reading file's `name` → declared simple type bindings.
    pub types: DeclaredTypes,
    /// The plugin name whose accessor convention judges a name
    /// ([`PropertiesIndex::names_an_accessor`]).
    pub language: &'a str,
    /// The module scope the class lookup is made in
    /// ([`PropertiesIndex::get`]).
    pub module: &'a str,
}

impl BindingView<'_> {
    /// The **canonical** configuration key `operand` names, walking
    /// [FR-WS-19]'s chain — *accessor → field → owning class → annotation
    /// prefix → canonical key* — or [`None`] at the first hop the source does
    /// not prove.
    ///
    /// [FR-WS-19]: ../../../../docs/specs/requirements/FR-WS-19.md
    pub fn key_for(&self, operand: Node<'_>, src: &[u8]) -> Option<String> {
        let (receiver, callee) = member_call(operand)?;
        let accessor = callee.utf8_text(src).ok()?.trim();
        // The shape question, asked in the READING file's own language, and it
        // is a correctness guard rather than a fast path. `bind` below judges by
        // the DECLARING class's language, so a Java file reading a Kotlin-
        // declared class would have its member call judged by Kotlin's
        // convention — and Kotlin declares the empty accessor prefix, under
        // which every name is already a property name. Without this test a Java
        // `k.compute()` would bind to a Kotlin `compute` property: the
        // convention leak `PropertiesIndex::conventions` documents at length,
        // reaching production through the use site instead of through the index.
        //
        // For a same-language read it changes nothing — `bind` returns
        // `NotAnAccessor` for the same names — which is why the guard is stated
        // as the cross-language one it is.
        if !self.index.names_an_accessor(self.language, accessor) {
            return None;
        }
        let receiver_name = simple_identifier(receiver.utf8_text(src).ok()?)?;
        let declared = self.types.get(receiver_name)?;
        let class = self.index.get(declared, self.module)?;
        let binding = self.index.bind(class, accessor).ok()?;
        // Canonical, not the source spelling: this key is recorded for a
        // resolution that canonicalises anyway, and recording the canonical form
        // makes the stored operand key byte-identical for two sites that spell
        // the same property differently.
        Some(canonical_key(&binding.key))
    }
}

#[cfg(all(test, feature = "lang-java"))]
#[path = "accessor_tests.rs"]
mod tests;
