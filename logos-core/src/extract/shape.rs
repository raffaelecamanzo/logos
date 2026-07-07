//! Declaration-shape helpers for the annotation columns Pass 1 captures
//! (S-014): the `exported` visibility flag ([FR-AN-01]) and the normalised
//! AST-shape `fingerprint` ([FR-AN-02]).
//!
//! Both run here — inside extraction — because this is the only moment the
//! tree-sitter AST exists: the canonical store keeps nodes and edges, not
//! trees, so the annotation pass (Pass 3) consumes these as native node
//! columns instead of re-parsing the repository.
//!
//! # What the fingerprint sees
//!
//! The shape stream feeds a [`blake3`] hash with, per AST node in pre-order:
//!
//! - **named nodes** — their *kind* only (`binary_expression`, `if_expression`,
//!   …) with structure parentheses, never their text. Identifier-class nodes
//!   (`identifier`, `field_identifier`, `type_identifier`, …) are collapsed to
//!   a single `id` atom and their subtree pruned, so renaming any identifier —
//!   variables, parameters, fields, paths, types — never changes the shape
//!   ([FR-AN-02] "identifiers stripped").
//! - **anonymous tokens** — their kind, which for tree-sitter *is* the token
//!   text (`+`, `return`, `{`). Operators and keywords are structure, so
//!   `a + b` and `a - b` stay distinct.
//! - **comments** — skipped wholesale, and whitespace never appears as a node,
//!   so formatting and documentation churn cannot perturb the shape
//!   ([FR-AN-02] "whitespace/comments stripped").
//!
//! Literal *values* are therefore also outside the shape (a named literal node
//! contributes its kind only): the contract is structural identity, and two
//! functions differing only in a constant are the same shape.
//!
//! [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
//! [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md

use tree_sitter::Node;

use crate::plugin::ExportConvention;

/// `true` when the declaration is exported under the language's declared
/// [`ExportConvention`] (S-015 — the per-language extension this predicate's
/// original Rust-only form anticipated).
///
/// Each convention reads the deliberately conservative exported-is-live way
/// ([FR-AN-01]): treating a merely-visible item as a dead-code root can only
/// under-report dead code, never flag a live symbol dead (the [AR-05] failure
/// mode). `All` — the default for a descriptor that declares nothing — takes
/// that posture to its safe extreme.
///
/// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
/// [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
pub(super) fn is_exported(node: Node<'_>, name: &str, convention: ExportConvention) -> bool {
    match convention {
        ExportConvention::All => true,
        // Rust: a `visibility_modifier` child (`pub`, `pub(crate)`, …).
        ExportConvention::VisibilityModifier => has_child_of_kind(node, "visibility_modifier"),
        // TS/JS: an `export_statement` ancestor. Walked (not just the parent)
        // because `export const F = …` nests the declarator two levels down.
        ExportConvention::ExportStatement => {
            let mut ancestor = node.parent();
            while let Some(n) = ancestor {
                if n.kind() == "export_statement" {
                    return true;
                }
                ancestor = n.parent();
            }
            false
        }
        // Go: a leading-uppercase name is the language's export rule.
        ExportConvention::Capitalized => name.chars().next().is_some_and(char::is_uppercase),
        // Java: a `public` token inside the declaration's `modifiers` child.
        ExportConvention::PublicModifier => {
            let mut cursor = node.walk();
            let public = node
                .children(&mut cursor)
                .filter(|c| c.kind() == "modifiers")
                .any(|m| has_child_of_kind(m, "public"));
            public
        }
        // Python: underscore-prefixed names are private by convention.
        ExportConvention::UnderscorePrivate => !name.starts_with('_'),
        // C: a file-scope declaration has external linkage unless it carries a
        // `static` storage-class specifier (S-056, FR-AN-01). The captured name
        // nests at varying depth — `identifier → function_declarator →
        // function_definition` for a function, `identifier → declaration` (or
        // via an `init_declarator`) for a bare global — so walk the ancestry up
        // to the translation unit and treat the first enclosing `static`
        // specifier as the file-local marker. `extern` and the unqualified
        // default both stay exported (the safe AR-05 direction: a merely-visible
        // item as a root can only under-report dead code).
        ExportConvention::NonStatic => {
            let mut current = Some(node);
            while let Some(n) = current {
                if n.kind() == "translation_unit" {
                    break;
                }
                let mut cursor = n.walk();
                let has_static = n
                    .children(&mut cursor)
                    .filter(|c| c.kind() == "storage_class_specifier")
                    .any(|s| has_child_of_kind(s, "static"));
                if has_static {
                    return false;
                }
                current = n.parent();
            }
            true
        }
        // C#: an explicit `public` among the declaration's flat `modifier`
        // children (S-057, CR-009). Unlike Java, tree-sitter-c-sharp does not
        // wrap modifiers in a `modifiers` node — each `modifier` is a direct
        // child holding one anonymous keyword token (`public`, `internal`, …),
        // so the keyword is read as a child node kind, not from source.
        ExportConvention::ExplicitModifier => {
            let mut cursor = node.walk();
            let public = node
                .children(&mut cursor)
                .filter(|c| c.kind() == "modifier")
                .any(|m| has_child_of_kind(m, "public"));
            public
        }
        // C++ (S-058): external linkage unless `static`-qualified or nested in
        // an anonymous namespace — the language's own internal-linkage markers.
        ExportConvention::CppExternalLinkage => {
            !cpp_has_static_storage(node) && !cpp_in_anonymous_namespace(node)
        }
        // PHP: public-by-default — exported unless a `visibility_modifier` child
        // carries `private`/`protected`. tree-sitter-php models the keyword as
        // the modifier's anonymous child token, so its `kind()` is the keyword
        // text — no source read needed. A declaration with no visibility modifier
        // (a top-level `function`, a class, a modifier-less member) is public and
        // therefore exported.
        ExportConvention::PhpVisibility => !php_is_restricted(node),
        // Scala: public-by-default — exported unless a `modifiers` child carries
        // an `access_modifier` (`private`/`protected`/`private[pkg]`). The
        // inverse of `PublicModifier`: absence of the marker is the export.
        ExportConvention::PublicDefault => {
            let mut cursor = node.walk();
            let narrowed = node
                .children(&mut cursor)
                .filter(|c| c.kind() == "modifiers")
                .any(|m| has_child_of_kind(m, "access_modifier"));
            !narrowed
        }
    }
}

/// `true` when a C++ declaration carries the `static` storage-class specifier
/// (internal linkage). The captured name's parent (`node`) is the declarator or
/// specifier; the `static` token lives on it or on its immediate declaration
/// parent — a free function's `storage_class_specifier` is a child of the
/// `function_definition`, one level above the captured `function_declarator`.
///
/// The keyword is read structurally, not from source: a `storage_class_specifier`
/// node holds an anonymous token child whose `kind()` *is* its text, so a
/// `static` child distinguishes internal linkage from `extern`/`thread_local`
/// (which do not mark a symbol non-exported) without the source bytes.
fn cpp_has_static_storage(node: Node<'_>) -> bool {
    let declares_static = |n: Node<'_>| {
        let mut cursor = n.walk();
        let found = n.children(&mut cursor).any(|c| {
            c.kind() == "storage_class_specifier" && has_child_of_kind(c, "static")
        });
        found
    };
    declares_static(node) || node.parent().is_some_and(declares_static)
}

/// `true` when `node` is lexically nested in an *anonymous* namespace
/// (`namespace { … }`), whose members have internal linkage. A named namespace
/// (`namespace app { … }`) exports normally and is skipped.
fn cpp_in_anonymous_namespace(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if n.kind() == "namespace_definition" && n.child_by_field_name("name").is_none() {
            return true;
        }
        ancestor = n.parent();
    }
    false
}

/// `true` when `node` has a direct `visibility_modifier` child whose keyword is
/// `private` or `protected` (the only two that demote a PHP declaration below
/// public). The keyword is the modifier's first child token, whose `kind()` is
/// the literal text in tree-sitter.
fn php_is_restricted(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let restricted = node
        .children(&mut cursor)
        .filter(|c| c.kind() == "visibility_modifier")
        .any(|m| matches!(m.child(0).map(|k| k.kind()), Some("private" | "protected")));
    restricted
}

/// `true` when `node` has a direct child of `kind` (named or anonymous).
fn has_child_of_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    // Bound to a local so the cursor's borrow of `node` ends before return.
    let found = node.children(&mut cursor).any(|c| c.kind() == kind);
    found
}

/// The normalised AST-shape fingerprint of one declaration ([FR-AN-02]).
///
/// Deterministic ([NFR-RA-06]): a pure function of the parse tree's shape and
/// the grammar name (included so an improbable cross-language shape collision
/// cannot pair nodes from different grammars).
///
/// [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn shape_fingerprint(node: Node<'_>, language: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(language.as_bytes());
    hasher.update(b"\x00");
    visit(node, &mut hasher);
    hasher.finalize().to_hex().to_string()
}

/// One frame of the iterative shape walk: enter a subtree, or emit its
/// closing structure parenthesis.
enum Frame<'tree> {
    Enter(Node<'tree>),
    Close,
}

/// Pre-order shape walk feeding the hasher (see the module docs for what is
/// kept and what is stripped).
///
/// Iterative on an explicit stack — **never** native recursion: AST depth is
/// attacker/input-controlled (deeply nested expressions, generated code) and a
/// stack overflow would abort the whole index run, violating [FR-IX-04] error
/// tolerance. Same hazard and same mitigation as the cyclomatic-complexity
/// walk in [`super::complexity`].
///
/// [FR-IX-04]: ../../../docs/specs/requirements/FR-IX-04.md
fn visit(root: Node<'_>, hasher: &mut blake3::Hasher) {
    let mut stack: Vec<Frame<'_>> = vec![Frame::Enter(root)];
    while let Some(frame) = stack.pop() {
        let node = match frame {
            Frame::Enter(node) => node,
            Frame::Close => {
                hasher.update(b")");
                continue;
            }
        };
        let kind = node.kind();

        // Comments vanish from the shape (FR-AN-02). Covers `line_comment`,
        // `block_comment`, and other grammars' `comment` variants.
        if kind.ends_with("comment") {
            continue;
        }

        // Identifier-class nodes collapse to one atom; pruning the subtree
        // also flattens composite paths (`a::b::c` vs `x`) to the same atom —
        // a path is a name, and names are stripped (FR-AN-02).
        if node.is_named() && kind.ends_with("identifier") {
            hasher.update(b"(id)");
            continue;
        }

        // The kind contributes for both named nodes (the grammar production)
        // and anonymous tokens (the literal token text: operators, keywords,
        // punctuation). Parentheses encode the tree structure so sibling
        // order and nesting are part of the shape. Children are pushed in
        // reverse so they pop in source order, with the close-paren frame
        // beneath them.
        hasher.update(b"(");
        hasher.update(kind.as_bytes());
        stack.push(Frame::Close);
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(Frame::Enter(child));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse `source` with `language`, find the first node of `kind`
    /// (pre-order), and probe [`is_exported`] on it under `convention`,
    /// reading the declaration's `name` field the way extraction does.
    #[cfg(any(
        feature = "lang-rust",
        feature = "lang-python",
        feature = "lang-typescript",
        feature = "lang-go",
        feature = "lang-java",
        feature = "lang-c",
        feature = "lang-c-sharp",
        feature = "lang-cpp",
        feature = "lang-php",
        feature = "lang-scala"
    ))]
    fn probe(
        language: &tree_sitter::Language,
        source: &str,
        kind: &str,
        convention: ExportConvention,
    ) -> bool {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language).expect("grammar binds");
        let tree = parser.parse(source, None).expect("source parses");
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == kind {
                let name = n
                    .child_by_field_name("name")
                    .and_then(|c| c.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("");
                return is_exported(n, name, convention);
            }
            for i in (0..n.child_count()).rev() {
                if let Some(c) = n.child(i) {
                    stack.push(c);
                }
            }
        }
        panic!("no {kind} node in fixture");
    }

    /// FR-AN-01 / S-015: the TS/JS convention — an `export_statement`
    /// ancestor marks the declaration, including the two-levels-down
    /// `export const F = () => …` declarator shape.
    #[cfg(feature = "lang-typescript")]
    #[test]
    fn export_statement_convention_follows_the_export_keyword() {
        use ExportConvention::ExportStatement;
        let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        assert!(
            probe(
                &lang,
                "export function f() {}",
                "function_declaration",
                ExportStatement
            ),
            "an exported function is a root"
        );
        assert!(
            !probe(
                &lang,
                "function g() {}",
                "function_declaration",
                ExportStatement
            ),
            "a bare function is not"
        );
        assert!(
            probe(
                &lang,
                "export const F = () => {};",
                "variable_declarator",
                ExportStatement
            ),
            "the declarator nests two levels under export_statement"
        );
        assert!(
            !probe(
                &lang,
                "const G = () => {};",
                "variable_declarator",
                ExportStatement
            ),
            "an unexported declarator is not a root"
        );
    }

    /// FR-AN-01 / S-015: the Go convention — a leading-uppercase name.
    #[cfg(feature = "lang-go")]
    #[test]
    fn capitalized_convention_follows_the_name_case() {
        use ExportConvention::Capitalized;
        let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
        let src_exported = "package p\n\nfunc Exported() {}\n";
        let src_unexported = "package p\n\nfunc unexported() {}\n";
        assert!(probe(
            &lang,
            src_exported,
            "function_declaration",
            Capitalized
        ));
        assert!(!probe(
            &lang,
            src_unexported,
            "function_declaration",
            Capitalized
        ));
    }

    /// FR-AN-01 / S-015: the Java convention — a `public` token inside the
    /// declaration's `modifiers`; package-private and `private` are not roots.
    #[cfg(feature = "lang-java")]
    #[test]
    fn public_modifier_convention_reads_the_modifiers_child() {
        use ExportConvention::PublicModifier;
        let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
        assert!(probe(
            &lang,
            "class A { public void f() {} }",
            "method_declaration",
            PublicModifier
        ));
        assert!(!probe(
            &lang,
            "class A { void g() {} }",
            "method_declaration",
            PublicModifier
        ));
        assert!(!probe(
            &lang,
            "class A { private void h() {} }",
            "method_declaration",
            PublicModifier
        ));
        assert!(probe(
            &lang,
            "public class A {}",
            "class_declaration",
            PublicModifier
        ));
    }

    /// FR-AN-01 / S-057 ([CR-009]): the C# convention — an explicit `public`
    /// among the declaration's flat `modifier` children; `internal`/`private`
    /// (and the implicit defaults) are not roots.
    #[cfg(feature = "lang-c-sharp")]
    #[test]
    fn explicit_modifier_convention_reads_the_flat_modifier_children() {
        use ExportConvention::ExplicitModifier;
        let lang: tree_sitter::Language = tree_sitter_c_sharp::LANGUAGE.into();
        assert!(probe(
            &lang,
            "class A { public void F() {} }",
            "method_declaration",
            ExplicitModifier
        ));
        assert!(
            !probe(
                &lang,
                "class A { void G() {} }",
                "method_declaration",
                ExplicitModifier
            ),
            "an implicitly-private member is not a root"
        );
        assert!(
            !probe(
                &lang,
                "class A { internal void H() {} }",
                "method_declaration",
                ExplicitModifier
            ),
            "internal is assembly-visible, not an exported root"
        );
        assert!(probe(
            &lang,
            "public class A {}",
            "class_declaration",
            ExplicitModifier
        ));
    }

    /// FR-AN-01 / S-015: the Python convention — `_`-prefixed names are
    /// private; everything else is a root.
    #[cfg(feature = "lang-python")]
    #[test]
    fn underscore_private_convention_reads_the_name() {
        use ExportConvention::UnderscorePrivate;
        let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        assert!(probe(
            &lang,
            "def f():\n    pass\n",
            "function_definition",
            UnderscorePrivate
        ));
        assert!(!probe(
            &lang,
            "def _f():\n    pass\n",
            "function_definition",
            UnderscorePrivate
        ));
    }

    /// FR-AN-01 / S-056: the C convention — a file-scope declaration has
    /// external linkage unless it carries a `static` storage-class specifier.
    /// `extern` and the unqualified default are roots; `static` is file-local.
    /// Probed on `function_definition`/`declaration` directly (C's name nests in
    /// a declarator, so `child_by_field_name("name")` is empty here — the
    /// `NonStatic` rule reads the storage class, not the name).
    #[cfg(feature = "lang-c")]
    #[test]
    fn non_static_convention_reads_the_storage_class() {
        use ExportConvention::NonStatic;
        let lang: tree_sitter::Language = tree_sitter_c::LANGUAGE.into();
        assert!(
            probe(
                &lang,
                "int f() { return 0; }",
                "function_definition",
                NonStatic
            ),
            "a non-static file-scope function has external linkage"
        );
        assert!(
            !probe(
                &lang,
                "static int g() { return 0; }",
                "function_definition",
                NonStatic
            ),
            "a static function is file-local — not a dead-code root"
        );
        assert!(
            probe(&lang, "extern int h(void);", "declaration", NonStatic),
            "an extern declaration is exported"
        );
        assert!(
            !probe(&lang, "static int x = 0;", "declaration", NonStatic),
            "a static global is file-local"
        );
        assert!(
            probe(&lang, "int y;", "declaration", NonStatic),
            "a plain file-scope global has external linkage"
        );
    }

    /// FR-AN-01 / S-058: the C++ convention — external linkage unless `static`
    /// or in an anonymous namespace. Probed on the `function_declarator` (the
    /// node extraction passes: the captured name's parent) and the
    /// `class_specifier`.
    #[cfg(feature = "lang-cpp")]
    #[test]
    fn cpp_external_linkage_excludes_static_and_anonymous_namespace() {
        use ExportConvention::CppExternalLinkage;
        let lang: tree_sitter::Language = tree_sitter_cpp::LANGUAGE.into();
        // A plain free function and a namespace-scoped class are exported.
        assert!(
            probe(
                &lang,
                "int add(int a, int b) { return a + b; }",
                "function_declarator",
                CppExternalLinkage
            ),
            "a free function has external linkage"
        );
        assert!(
            probe(
                &lang,
                "namespace app { class Widget {}; }",
                "class_specifier",
                CppExternalLinkage
            ),
            "a namespace-scoped class is exported"
        );
        // `static` at file scope is internal linkage — not a root.
        assert!(
            !probe(
                &lang,
                "static int helper() { return 0; }",
                "function_declarator",
                CppExternalLinkage
            ),
            "a static free function is internal linkage"
        );
        // An anonymous namespace gives its members internal linkage.
        assert!(
            !probe(
                &lang,
                "namespace { int hidden() { return 1; } }",
                "function_declarator",
                CppExternalLinkage
            ),
            "a function in an anonymous namespace is not exported"
        );
        // A *named* namespace does not suppress export.
        assert!(
            probe(
                &lang,
                "namespace app { int visible() { return 1; } }",
                "function_declarator",
                CppExternalLinkage
            ),
            "a function in a named namespace is exported"
        );
    }

    /// FR-AN-01 / S-061: the Scala convention — public-by-default, narrowed only
    /// by an `access_modifier` (`private`/`protected`) inside the declaration's
    /// `modifiers`. The inverse of the Java `public`-token rule.
    #[cfg(feature = "lang-scala")]
    #[test]
    fn public_default_convention_treats_access_modifier_as_non_export() {
        use ExportConvention::PublicDefault;
        let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
        // No access modifier → exported (the public-by-default case).
        assert!(probe(
            &lang,
            "class A { def f(): Int = 1 }",
            "function_definition",
            PublicDefault
        ));
        assert!(probe(&lang, "class A {}", "class_definition", PublicDefault));
        // `private`/`protected` → non-root.
        assert!(!probe(
            &lang,
            "class A { private def g(): Int = 1 }",
            "function_definition",
            PublicDefault
        ));
        assert!(!probe(
            &lang,
            "class A { protected def h(): Int = 1 }",
            "function_definition",
            PublicDefault
        ));
        // Package-scoped `private[pkg]` is still an access modifier → non-root.
        assert!(!probe(
            &lang,
            "class A { private[pkg] def i(): Int = 1 }",
            "function_definition",
            PublicDefault
        ));
    }

    /// The `All` default marks everything exported — the safe direction for a
    /// language with no declared convention (under-reports dead code,
    /// never flags a live symbol dead).
    #[cfg(feature = "lang-rust")]
    #[test]
    fn all_convention_marks_everything_exported() {
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        assert!(probe(
            &lang,
            "fn private_fn() {}",
            "function_item",
            ExportConvention::All
        ));
    }

    /// FR-AN-01 / S-060: the PHP convention — public-by-default. A
    /// `private`/`protected` `visibility_modifier` demotes a member below
    /// public; a `public` modifier, no modifier, and a top-level `function`
    /// are all exported.
    #[cfg(feature = "lang-php")]
    #[test]
    fn php_visibility_convention_is_public_by_default() {
        use ExportConvention::PhpVisibility;
        let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
        let src = "<?php
class A {
    public function pub() {}
    private function priv() {}
    protected function prot() {}
    function plain() {}
}
function freeFn() {}
";
        assert!(
            probe(&lang, src, "function_definition", PhpVisibility),
            "a top-level function is public and exported"
        );
        // The four methods in declaration order: pub, priv, prot, plain.
        // `probe` returns the first `method_declaration`, so assert per-keyword
        // with focused fixtures instead.
        assert!(
            probe(
                &lang,
                "<?php class A { public function f() {} }",
                "method_declaration",
                PhpVisibility
            ),
            "a public method is exported"
        );
        assert!(
            !probe(
                &lang,
                "<?php class A { private function f() {} }",
                "method_declaration",
                PhpVisibility
            ),
            "a private method is not exported"
        );
        assert!(
            !probe(
                &lang,
                "<?php class A { protected function f() {} }",
                "method_declaration",
                PhpVisibility
            ),
            "a protected method is not exported"
        );
        assert!(
            probe(
                &lang,
                "<?php class A { function f() {} }",
                "method_declaration",
                PhpVisibility
            ),
            "a modifier-less method is public-by-default → exported"
        );
        assert!(
            probe(
                &lang,
                "<?php class A {}",
                "class_declaration",
                PhpVisibility
            ),
            "a class carries no visibility modifier → exported"
        );
    }
}
