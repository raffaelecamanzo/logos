//! Reference collection — the raw-material half of the resolution engine
//! (S-011, [FR-RS-01], [FR-RS-03]).
//!
//! The `references` capability query (see `plugins/rust/queries/references.scm`)
//! captures call paths, receiver-method calls, and whole `use` declarations.
//! This module turns those captures into normalised pieces: [`split_path_text`]
//! canonicalises a path's text (whitespace and turbofish stripped, `::`-split),
//! and [`flatten_use_tree`] walks an arbitrarily nested `use` argument (groups,
//! `as` renames, `self` re-binds, globs) into flat [`UseItem`]s — something a
//! tree-sitter query cannot express on its own.
//!
//! Nothing here *binds* anything: extraction records what a file points at,
//! verbatim; deciding what (if anything) a reference means is the resolution
//! pass's job ([NFR-RA-05] — never fabricate).
//!
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
//! [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use tree_sitter::Node;

use crate::model::RefForm;

/// One flattened `use` import: a path, the name it binds in scope, and whether
/// it is a glob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UseItem {
    /// The import path segments (`use a::b::c` → `["a", "b", "c"]`).
    pub path: Vec<String>,
    /// The in-scope name the import binds: the last segment, or the explicit
    /// `as` rename. `None` for a glob (a glob binds the module's members, not
    /// one name).
    pub alias: Option<String>,
    /// `true` for `use m::*`.
    pub glob: bool,
}

/// Node kinds that legitimately appear as a plain path (or path head) inside a
/// `use` tree. Anything else (e.g. a comment node inside a use list) is skipped
/// rather than turned into a junk path segment.
const PATH_KINDS: &[&str] = &[
    "identifier",
    "scoped_identifier",
    "crate",
    "self",
    "super",
    "metavariable",
];

/// Canonicalise a **member path**'s source text into its segments — the
/// `a.b.c` / `a::b::c` grammar, and the import text of a language whose
/// specifiers are names ([`ImportSpecifier::Name`]).
///
/// Strips whitespace and any `<…>` span (turbofish / generic arguments — for
/// binding purposes `Vec::<u8>::new` is the path `Vec::new`), then splits on
/// the path separators of the supported languages — `::` (Rust/C++/Ruby), `.`
/// (Python/TS/Go/Java member paths), `/` (Ruby `require` paths), and `\` (PHP
/// namespace paths, S-060) — dropping empty segments (which also normalises a
/// leading global-path `::std::x` to `std::x` and PHP's leading-`\`
/// fully-qualified `\App\X` to `App::X`). Each separator is unique to its
/// languages' path text, so a language that never uses one is left
/// byte-identical by its inclusion ([NFR-RA-03]) — Rust path text contains no
/// bare `.`/`/`/`\`, and PHP's backslash appears in no other language's paths.
///
/// A **module specifier** written as a path (`"./nav.ts"`,
/// `"github.com/lib/pq"`) is *not* this grammar — splitting it here records
/// `nav::ts` and cuts the host in half — and goes through
/// [`specifier_segments`] instead (S-439, [CR-142] D1).
///
/// [NFR-RA-03]: ../../../docs/specs/requirements/NFR-RA-03.md
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
/// [`ImportSpecifier::Name`]: crate::plugin::ImportSpecifier::Name
pub(crate) fn split_path_text(text: &str) -> Vec<String> {
    let mut cleaned = String::with_capacity(text.len());
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 && !c.is_whitespace() => cleaned.push(c),
            _ => {}
        }
    }
    cleaned
        .split("::")
        .flat_map(|seg| seg.split(['.', '/', '\\']))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The ledger target of a **fully qualified call** `<T as Tr>::m()` (S-606,
/// [CR-202]): `<T as Tr>::m`, its type `ty` and trait `tr` recorded as type
/// paths ([`item_path`](super::assoc::item_path) — generics stripped) and
/// `segments` the call's path past the bracket, as [`split_path_text`] reads it
/// (`["m"]`). [`split_path_text`] alone strips the whole bracket and records
/// the bare `m`, which names neither the type nor the trait.
///
/// [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
pub(crate) fn qualified_call_target(ty: &str, tr: &str, segments: &[String]) -> String {
    format!(
        "<{} as {}>::{}",
        super::assoc::item_path(ty),
        super::assoc::item_path(tr),
        segments.join("::")
    )
}

/// Canonicalise one captured `@ref.import` node's text into path segments, for
/// a language whose specifiers are **names** ([`ImportSpecifier::Name`]).
///
/// Name-shaped import sources arrive in language-shaped clothing: a Python
/// dotted name (`django.urls`), a Java scoped identifier
/// (`org.springframework.web`), a PHP namespace path, a Ruby `require` string.
/// One matching pair of surrounding quotes is stripped, then the text splits
/// like any member path. The result feeds a `RefFact` whose `::`-joined target
/// is the ledger's canonical form — the framework candidacy gate ([FR-FW-04])
/// and the binder both read that form, whatever the source language. A
/// language whose specifiers are paths uses [`specifier_segments`].
///
/// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
/// [`ImportSpecifier::Name`]: crate::plugin::ImportSpecifier::Name
pub(crate) fn import_segments(text: &str) -> Vec<String> {
    split_path_text(unquote(text))
}

/// Canonicalise the module of a `from m import …` (`@ref.import.from`, S-519,
/// [FR-RS-14]) into the segments each imported name is recorded under. A
/// **relative** module keeps its level as leading [`is_relative_head`]
/// segments, the shape a relative path specifier records: one dot (the
/// importing file's own package) is `.`, and every further dot one `..` —
/// `.rules` → `[., rules]`, `..` → `[..]`, `...a.b` → `[.., .., a, b]`. An
/// absolute module splits as any member path does.
///
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
pub(crate) fn from_module_segments(text: &str) -> Vec<String> {
    let text = text.trim();
    let level = text.chars().take_while(|c| *c == '.').count();
    let mut segments: Vec<String> = match level {
        0 => Vec::new(),
        1 => vec![".".to_string()],
        n => vec!["..".to_string(); n - 1],
    };
    segments.extend(split_path_text(&text[level..]));
    segments
}

/// The leading segment a relative specifier keeps in the ledger target: `.`
/// (the importing file's directory) or `..` (its parent). Neither can survive
/// [`split_path_text`] — `.` is one of its separators — so a target headed by
/// one is unambiguously a relative path specifier to the binder.
pub(crate) fn is_relative_head(segment: &str) -> bool {
    matches!(segment, "." | "..")
}

/// Canonicalise one captured `@ref.import` node's text by **path** rules, for a
/// language whose specifiers are paths ([`ImportSpecifier::Path`]; S-439,
/// [CR-142] D1, [FR-RS-01]).
///
/// A module specifier is a path, not a member expression:
/// - one pair of surrounding quotes is stripped, and **only `/`** separates — a
///   dot belongs to the segment it sits in, so `github.com/org/repo` keeps its
///   host whole (`github.com::org::repo`) and `lodash.debounce` stays one name;
/// - a **relative** specifier (`./x`, `../x`, `.`) keeps its leading `.`/`..`
///   segment ([`is_relative_head`]) so the binder can resolve it against the
///   importing file, which extraction deliberately does not do — the ledger
///   records what the file wrote, and binding is the resolution pass's job
///   ([NFR-RA-05]). Interior `.` hops are dropped; interior `..` hops are kept
///   for the binder to fold;
/// - a relative specifier's trailing extension is stripped **iff** it is one of
///   `extensions` — the extensions that name the imported file itself — so
///   `"./nav.ts"` and `"./nav"` canonicalise to the one target `.::nav`. Any
///   other extension (`"./styles.css"`) is kept, so it can never be read as a
///   code file of the same stem. A bare specifier (`react`, `next/link`) names
///   a package, never a file, and is never stripped.
///
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
/// [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [`ImportSpecifier::Path`]: crate::plugin::ImportSpecifier::Path
pub(crate) fn specifier_segments(text: &str, extensions: &[String]) -> Vec<String> {
    let spec = unquote(text);
    let relative = spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../");
    let mut segments: Vec<String> = Vec::new();
    for (i, seg) in spec.split('/').enumerate() {
        match seg {
            "" => {}
            // The leading `.` is the relative marker; an interior one is a no-op.
            "." if i > 0 => {}
            _ => segments.push(seg.to_string()),
        }
    }
    if relative {
        if let Some(last) = segments.last_mut().filter(|s| !is_relative_head(s)) {
            if let Some((stem, ext)) = last.rsplit_once('.') {
                if !stem.is_empty() && extensions.iter().any(|e| e == ext) {
                    *last = stem.to_string();
                }
            }
        }
    }
    segments
}

/// Strip one matching pair of surrounding string-literal quotes, if present.
pub(crate) fn unquote(text: &str) -> &str {
    let t = text.trim();
    let mut chars = t.chars();
    match (chars.next(), t.chars().next_back()) {
        (Some(first), Some(last))
            if first == last && matches!(first, '"' | '\'' | '`') && t.len() >= 2 =>
        {
            &t[first.len_utf8()..t.len() - last.len_utf8()]
        }
        _ => t,
    }
}

/// One call recognised inside a macro invocation's token tree (S-162,
/// [CR-043]): the call target plus its reference form and 1-based line.
///
/// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MacroCall {
    /// The call target — a `::`-joined path (`RefForm::Path`, e.g. `activity_card`
    /// or `a::b::f`) or a bare receiver-method name (`RefForm::Method`, e.g.
    /// `chip_class`).
    pub target: String,
    /// `Path` for a free or scoped call (`f()`, `a::b()`); `Method` for a
    /// receiver-method call (`x.f()`).
    pub form: RefForm,
    /// 1-based line of the call's name token (the enclosing function carries the
    /// attribution; the line records where in the macro the call sits).
    pub line: u32,
    /// `true` for a receiver-method call whose receiver token is exactly `self`
    /// (`self.f()`, never `self.x.f()`): the token-tree twin of the
    /// `@ref.method.self` capture, which cannot reach inside a macro (S-514).
    pub self_receiver: bool,
    /// How many arguments the call passes (S-591, [FR-EX-32]), read from its
    /// `(`-delimited token tree ([`token_tree_arg_count`]) so a call inside a
    /// macro records the count the same call records outside one; `None` where
    /// a token tree cannot be counted reliably.
    ///
    /// [FR-EX-32]: ../../../docs/specs/requirements/FR-EX-32.md
    pub arg_count: Option<u32>,
}

/// Walk a Rust `macro_invocation`'s token tree(s) for the call-shaped token
/// sequences the `references` query cannot see — tree-sitter does not parse a
/// macro's `token_tree` body as expressions, so `call_expression` /
/// `field_expression` patterns never match inside it (the documented S-011
/// limitation, lifted here for the `Calls` relation, S-162 / [CR-043] §3.2).
///
/// A call is an `identifier` immediately followed by a `(`-delimited
/// `token_tree`, with no intervening `!` (a `!` makes the identifier a *nested
/// macro* name — `format!(…)` — not a function call). It is a receiver-method
/// call ([`RefForm::Method`], bare name) when the identifier is immediately
/// preceded by a `.` token, otherwise a path call ([`RefForm::Path`]) whose
/// leading `ident (:: ident)*` run is assembled into a `::`-joined path
/// (`scoped_identifier` never forms inside a token tree, so a path arrives as a
/// raw `identifier`/`::` token run). Nested token trees — call arguments and
/// nested macros alike — are scanned recursively, so a call at any depth is
/// recognised. A method call whose receiver is the `self` token itself is
/// flagged ([`MacroCall::self_receiver`]): its receiver shape is `self`, which
/// the query's `@ref.method.self` capture records outside a macro (S-514).
///
/// Like the rest of this module it records **what the file points at, verbatim**
/// — it never binds. A target that resolves to no, or several, candidates stays
/// in `unresolved_refs` ([NFR-RA-05]); the false-live bias is the resolution
/// pass's, not extraction's.
///
/// Known recall gap (never a fabrication): a **turbofish-qualified** call inside
/// a macro (`f::<T>()`, `Vec::<u8>::new()`) is *not* recognised — the `<…>` run
/// sits between the name and the `(`-group, so the name is no longer immediately
/// followed by the call group and the shape is skipped. This only ever *omits* a
/// real call (biasing toward false-live for the callee, never fabricating), and
/// is rare for a dead-code candidate; a turbofish call outside a macro is
/// captured normally by the `references` query. Pinned by
/// `turbofish_call_in_a_macro_is_a_known_recall_gap`.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn macro_call_refs(macro_node: Node<'_>, source: &[u8]) -> Vec<MacroCall> {
    let mut out = Vec::new();
    // The macro's argument body is its `token_tree` child; `m!(…)`, `m![…]`, and
    // `m!{…}` all expose the delimited group as a `token_tree`.
    let mut cursor = macro_node.walk();
    for child in macro_node.children(&mut cursor) {
        if child.kind() == "token_tree" {
            scan_token_tree(child, source, &mut out);
        }
    }
    out
}

/// Scan one `token_tree`'s ordered children (named and anonymous) for call
/// shapes, recursing into every nested `token_tree`.
fn scan_token_tree(tt: Node<'_>, source: &[u8], out: &mut Vec<MacroCall>) {
    for i in 0..tt.child_count() {
        let Some(child) = tt.child(i) else { continue };
        if child.kind() == "token_tree" {
            scan_token_tree(child, source, out);
            continue;
        }
        if child.kind() != "identifier" {
            continue;
        }
        // A call: the immediately following token is a `(`-delimited token tree.
        // A `!` next (nested macro) or a `[`/`{` group (index / struct literal)
        // is not a function call.
        let Some(next) = tt.child(i + 1) else { continue };
        if next.kind() != "token_tree" || !opens_with_paren(next) {
            continue;
        }
        let Ok(name) = child.utf8_text(source) else { continue };
        let line = child.start_position().row as u32 + 1;
        // Receiver-method call `.name(…)`: the `.` is an anonymous prev token.
        let preceded_by_dot = i > 0 && tt.child(i - 1).is_some_and(|p| p.kind() == ".");
        if preceded_by_dot {
            // `self.f()`: the receiver token is `self`, and nothing — no `.` —
            // precedes it (`self.x.f()` reaches `f` through the field `x`).
            let self_receiver = i >= 2
                && tt.child(i - 2).is_some_and(|r| r.kind() == "self")
                && (i < 3 || tt.child(i - 3).is_none_or(|p| p.kind() != "."));
            out.push(MacroCall {
                target: name.to_string(),
                form: RefForm::Method,
                line,
                self_receiver,
                arg_count: token_tree_arg_count(next),
            });
            continue;
        }
        // Path call: assemble the leading `ident (:: ident)*` run ending at this
        // identifier into a `::`-joined path (a bare `foo` stays a single segment).
        out.push(MacroCall {
            target: assemble_path(tt, i, source),
            form: RefForm::Path,
            line,
            self_receiver: false,
            arg_count: token_tree_arg_count(next),
        });
    }
}

/// The arguments a call's `(`-delimited token tree passes (S-591): its
/// top-level comma-separated segments, a trailing comma closing none, `0` for
/// `()`. A nested group (`g(a, b)`, `[a, b]`) is one token tree, so its commas
/// are not top-level. `None` when a top-level token could carry a comma that
/// separates no arguments — a closure's `|a, b|`, a generic's `<A, B>`, a
/// macro metavariable's `$`, an attribute's `#` — since a token tree is never
/// parsed as expressions.
fn token_tree_arg_count(args: Node<'_>) -> Option<u32> {
    let mut cursor = args.walk();
    let tokens: Vec<Node<'_>> = args.children(&mut cursor).filter(|t| !t.is_extra()).collect();
    // The delimiters themselves: `(` first, `)` last.
    let inner = tokens.get(1..tokens.len().saturating_sub(1)).unwrap_or_default();
    if inner.iter().any(|t| matches!(t.kind(), "|" | "||" | "<" | ">" | "$" | "#")) {
        return None;
    }
    let Some(last) = inner.last() else {
        return Some(0);
    };
    let commas = inner.iter().filter(|t| t.kind() == ",").count();
    let segments = commas + usize::from(last.kind() != ",");
    u32::try_from(segments).ok()
}

/// `true` if `tt`'s first child is an opening parenthesis — the delimiter of a
/// *call* argument group (as opposed to a `[…]` index or `{…}` block).
fn opens_with_paren(tt: Node<'_>) -> bool {
    tt.child(0).is_some_and(|c| c.kind() == "(")
}

/// Assemble the `ident (:: ident)*` path run ending at child index `call_idx`
/// into a `::`-joined string (walking left over `identifier`/`::` token pairs).
fn assemble_path(tt: Node<'_>, call_idx: usize, source: &[u8]) -> String {
    let mut segs: Vec<&str> = Vec::new();
    let mut idx = call_idx as isize;
    while let Some(node) = usize::try_from(idx).ok().and_then(|u| tt.child(u)) {
        if node.kind() != "identifier" {
            break;
        }
        let Ok(text) = node.utf8_text(source) else { break };
        segs.push(text);
        // A preceding `::` continues the path; anything else ends it.
        let sep_idx = idx - 1;
        let continues = usize::try_from(sep_idx)
            .ok()
            .and_then(|u| tt.child(u))
            .is_some_and(|s| s.kind() == "::");
        if !continues {
            break;
        }
        idx = sep_idx - 1;
    }
    segs.reverse();
    segs.join("::")
}

/// Flatten a `use_declaration`'s argument node into [`UseItem`]s.
///
/// Handles every shape the Rust grammar produces: plain paths, `as` renames
/// (`use a::b as c`), groups (`use a::{b, c::d}`), nested groups, `self`
/// rebinding (`use a::b::{self}` binds `b`), and globs (`use a::*`).
pub(crate) fn flatten_use_tree(node: Node<'_>, source: &[u8], out: &mut Vec<UseItem>) {
    flatten_with_prefix(node, source, &[], out);
}

fn flatten_with_prefix(node: Node<'_>, source: &[u8], prefix: &[String], out: &mut Vec<UseItem>) {
    let text = |n: Node<'_>| n.utf8_text(source).unwrap_or_default().to_string();
    match node.kind() {
        "use_as_clause" => {
            let (Some(path_node), Some(alias_node)) = (
                node.child_by_field_name("path"),
                node.child_by_field_name("alias"),
            ) else {
                return;
            };
            let mut path = prefix.to_vec();
            path.extend(split_path_text(&text(path_node)));
            let alias = text(alias_node).trim().to_string();
            if !path.is_empty() && !alias.is_empty() {
                out.push(UseItem {
                    path,
                    alias: Some(alias),
                    glob: false,
                });
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                flatten_with_prefix(child, source, prefix, out);
            }
        }
        "scoped_use_list" => {
            let mut new_prefix = prefix.to_vec();
            if let Some(p) = node.child_by_field_name("path") {
                new_prefix.extend(split_path_text(&text(p)));
            }
            if let Some(list) = node.child_by_field_name("list") {
                flatten_with_prefix(list, source, &new_prefix, out);
            }
        }
        "use_wildcard" => {
            let mut path = prefix.to_vec();
            // The globbed module path is the wildcard's only named child;
            // a bare `use *;` (no module) is meaningless and skipped.
            if let Some(child) = node.named_child(0) {
                path.extend(split_path_text(&text(child)));
            }
            if !path.is_empty() {
                out.push(UseItem {
                    path,
                    alias: None,
                    glob: true,
                });
            }
        }
        kind if PATH_KINDS.contains(&kind) => {
            let mut path = prefix.to_vec();
            path.extend(split_path_text(&text(node)));
            // `use a::b::{self}` binds the *module* `b`: drop the trailing
            // `self` so the path is the module's and the alias its name.
            if path.last().is_some_and(|s| s == "self") && path.len() > 1 {
                path.pop();
            }
            let Some(alias) = path.last().cloned() else {
                return;
            };
            out.push(UseItem {
                path,
                alias: Some(alias),
                glob: false,
            });
        }
        // Comments or future grammar nodes inside a use tree: skip, never
        // fabricate a path out of non-path text.
        _ => {}
    }
}

/// Flatten an import declaration whose grammar spreads each imported path over
/// repeated `path:` children of the declaration itself, with no node spanning
/// one path (S-518) — Scala's `import a.b.C`, `import a.b._` / `a.b.*`,
/// `import a.b.{C, D => E, _}` and `import a.B, c.D`. A capture-name-driven
/// walk of the one declaration ([`flatten_use_tree`]'s twin for this shape):
///
/// - the `path:` children accumulate one expression's segments, and a `,`
///   token ends it;
/// - an expression with nothing after its path imports its last segment;
/// - a wildcard after it (`_` or `*`) is a [`UseItem::glob`] of the path;
/// - a braced group imports each selector under the path — a name, the `name`
///   of a rename (`D => E`, `D as E`), or a wildcard — and a selector renamed
///   to `_` hides its name, so it imports nothing;
/// - a rename directly after the path (Scala 3's `import a.b as c`) imports
///   its `name` under the path.
///
/// Every non-glob item's alias is its last segment, as for every other
/// language's import — except a rename's, which is the local name it binds
/// (`E`, S-520): the renamed `D` is not in view, so an unqualified `D` still
/// names a same-package `D`. A `given` selector — bare, or `given T`, which imports
/// given instances of `T` and not `T` — imports no declaration and is skipped,
/// as is anything else no rule above reads — never a path made of other text.
pub(crate) fn flatten_dotted_import(node: Node<'_>, source: &[u8], out: &mut Vec<UseItem>) {
    let mut path: Vec<String> = Vec::new();
    let mut selected = false;
    let mut cursor = node.walk();
    for (i, child) in node.children(&mut cursor).enumerate() {
        if node.field_name_for_child(i as u32) == Some("path") {
            if child.is_named() {
                path.push(node_text(child, source));
            }
            continue;
        }
        if !child.is_named() {
            if child.kind() == "," {
                finish_dotted_expression(&mut path, selected, out);
                selected = false;
            }
            continue;
        }
        selected = true;
        dotted_selector(child, source, &path, true, out);
    }
    finish_dotted_expression(&mut path, selected, out);
}

/// End one expression of [`flatten_dotted_import`]: a path no selector
/// followed imports its last segment. Clears `path` for the next expression.
fn finish_dotted_expression(path: &mut Vec<String>, selected: bool, out: &mut Vec<UseItem>) {
    let path = std::mem::take(path);
    if selected {
        return;
    }
    if let Some(alias) = path.last().cloned() {
        out.push(UseItem {
            path,
            alias: Some(alias),
            glob: false,
        });
    }
}

/// One selector of [`flatten_dotted_import`] under `path`: a wildcard, a name,
/// a rename, or — when `group_allowed` — a braced group of them.
fn dotted_selector(
    node: Node<'_>,
    source: &[u8],
    path: &[String],
    group_allowed: bool,
    out: &mut Vec<UseItem>,
) {
    if path.is_empty() {
        return;
    }
    let text = node_text(node, source);
    let item = |name: String| {
        let mut full = path.to_vec();
        full.push(name.clone());
        UseItem {
            path: full,
            alias: Some(name),
            glob: false,
        }
    };
    if node.named_child_count() == 0 {
        match text.as_str() {
            "_" | "*" => out.push(UseItem {
                path: path.to_vec(),
                alias: None,
                glob: true,
            }),
            "given" | "" => {}
            name => out.push(item(name.to_string())),
        }
        return;
    }
    if let Some(name) = node.child_by_field_name("name") {
        let local = node.child_by_field_name("alias").map(|alias| node_text(alias, source));
        match local.as_deref() {
            Some("_") => {} // `D => _` hides `D`: it imports nothing
            Some(local) if !local.is_empty() => out.push(UseItem {
                alias: Some(local.to_string()),
                ..item(node_text(name, source))
            }),
            _ => out.push(item(node_text(name, source))),
        }
        return;
    }
    let braced = node.child(0).is_some_and(|c| c.kind() == "{");
    if group_allowed && braced {
        // A `given T` selector imports given instances of `T`, never the type
        // `T` itself: the type after a `given` token is skipped.
        let mut after_given = false;
        let mut cursor = node.walk();
        for selector in node.children(&mut cursor) {
            if !selector.is_named() {
                after_given = selector.kind() == "given";
                continue;
            }
            if !std::mem::take(&mut after_given) {
                dotted_selector(selector, source, path, false, out);
            }
        }
    }
}

/// `node`'s source text, trimmed; empty when it is not UTF-8.
fn node_text(node: Node<'_>, source: &[u8]) -> String {
    node.utf8_text(source).unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_strips_whitespace_turbofish_and_leading_colons() {
        assert_eq!(split_path_text("a::b::c"), ["a", "b", "c"]);
        assert_eq!(split_path_text("a :: b"), ["a", "b"]);
        assert_eq!(split_path_text("Vec::<u8>::new"), ["Vec", "new"]);
        assert_eq!(split_path_text("::std::mem::swap"), ["std", "mem", "swap"]);
        assert_eq!(split_path_text("f"), ["f"]);
        assert!(split_path_text("").is_empty());
        // Nested generics collapse entirely.
        assert_eq!(
            split_path_text("HashMap::<String, Vec<u8>>::new"),
            ["HashMap", "new"]
        );
    }

    #[test]
    fn split_handles_the_other_languages_separators() {
        // Python / Java dotted paths.
        assert_eq!(split_path_text("django.urls"), ["django", "urls"]);
        assert_eq!(
            split_path_text("org.springframework.web"),
            ["org", "springframework", "web"]
        );
        // Ruby `require` paths are name-grammar import text that uses `/`.
        assert_eq!(
            split_path_text("active_support/core_ext"),
            ["active_support", "core_ext"]
        );
        // A TS/Go module specifier is NOT this grammar (S-439): it goes through
        // `specifier_segments`, pinned below. What stays here is that the member
        // path grammar still splits every dot — `a.b.c` is three names.
        assert_eq!(split_path_text("a.b.c"), ["a", "b", "c"]);
        // PHP namespace paths (S-060): backslash is a separator, so a
        // `use Illuminate\Support\Facades\Route` import and a leading-`\`
        // fully-qualified name both canonicalise to `::`-joined segments — the
        // form the framework candidacy gate and the binder read.
        assert_eq!(
            split_path_text("Illuminate\\Support\\Facades\\Route"),
            ["Illuminate", "Support", "Facades", "Route"]
        );
        assert_eq!(split_path_text("\\App\\Models\\User"), ["App", "Models", "User"]);
    }

    #[test]
    fn import_segments_strip_one_pair_of_quotes() {
        assert_eq!(import_segments("\"net/http\""), ["net", "http"]);
        assert_eq!(import_segments("'express'"), ["express"]);
        assert_eq!(import_segments("`next/link`"), ["next", "link"]);
        // Unquoted text passes through; mismatched quotes are left alone.
        assert_eq!(import_segments("fastapi"), ["fastapi"]);
        assert_eq!(import_segments("\"unterminated"), ["\"unterminated"]);
        assert!(import_segments("\"\"").is_empty());
    }

    fn ts_exts() -> Vec<String> {
        ["ts", "tsx", "js", "jsx", "mjs", "cjs"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn a_relative_specifier_with_an_extension_keeps_its_relative_head_and_drops_the_extension() {
        // The CR-142 §3.1 evidence row: `App.tsx:22` wrote `"./nav.ts"` and the
        // ledger recorded `nav::ts`. Path grammar: `.` marks it relative, the
        // declared extension is the file's own and is stripped.
        assert_eq!(specifier_segments("\"./nav.ts\"", &ts_exts()), [".", "nav"]);
        assert_eq!(
            specifier_segments("'./shell/Header.tsx'", &ts_exts()),
            [".", "shell", "Header"]
        );
    }

    #[test]
    fn a_relative_specifier_without_an_extension_canonicalises_to_the_same_shape() {
        // desk-picker's spelling. Pinned separately from the extension-present
        // case: one of the two produced 0 bound imports and the other 9, so
        // neither may be inferred from the other.
        assert_eq!(
            specifier_segments("\"./auth/AuthContext\"", &ts_exts()),
            [".", "auth", "AuthContext"]
        );
        assert_eq!(specifier_segments("'./nav'", &ts_exts()), [".", "nav"]);
    }

    #[test]
    fn a_specifier_splits_on_slash_only_so_a_dotted_host_name_stays_whole() {
        // The Go evidence row: the host was cut mid-name into `github::com`.
        assert_eq!(
            specifier_segments("\"github.com/sourcesense/desk-picker/internal/admin\"", &[]),
            [
                "github.com",
                "sourcesense",
                "desk-picker",
                "internal",
                "admin"
            ]
        );
        assert_eq!(specifier_segments("\"gopkg.in/yaml.v3\"", &[]), ["gopkg.in", "yaml.v3"]);
        // Bare package specifiers are unchanged where they carry no dot…
        assert_eq!(specifier_segments("\"net/http\"", &[]), ["net", "http"]);
        assert_eq!(specifier_segments("'next/link'", &ts_exts()), ["next", "link"]);
        assert_eq!(specifier_segments("'react'", &ts_exts()), ["react"]);
        // …and a bare specifier names a package, never a file: no stripping.
        assert_eq!(specifier_segments("'chart.js'", &ts_exts()), ["chart.js"]);
    }

    #[test]
    fn a_relative_specifier_keeps_an_undeclared_extension_and_its_parent_hops() {
        // `.css` is not a code extension of the language: kept, so it can never
        // be read as a `styles.ts` of the same stem.
        assert_eq!(
            specifier_segments("'./styles.css'", &ts_exts()),
            [".", "styles.css"]
        );
        assert_eq!(
            specifier_segments("'../api/client.js'", &ts_exts()),
            ["..", "api", "client"]
        );
        assert_eq!(specifier_segments("'../../a/./b'", &ts_exts()), ["..", "..", "a", "b"]);
        // A bare-directory specifier keeps only its marker.
        assert_eq!(specifier_segments("'.'", &ts_exts()), ["."]);
        assert_eq!(specifier_segments("'..'", &ts_exts()), [".."]);
        // A dotfile stem is not an extension to strip.
        assert_eq!(specifier_segments("'./.ts'", &ts_exts()), [".", ".ts"]);
        assert!(specifier_segments("\"\"", &ts_exts()).is_empty());
    }

    #[test]
    fn only_a_relative_head_is_a_relative_marker() {
        assert!(is_relative_head("."));
        assert!(is_relative_head(".."));
        assert!(!is_relative_head("..."));
        assert!(!is_relative_head(".nav"));
        assert!(!is_relative_head("self"));
        // The member-path grammar can never produce the marker.
        assert!(split_path_text("./a.b").iter().all(|s| !is_relative_head(s)));
    }
}

#[cfg(all(test, feature = "lang-rust"))]
mod tree_tests {
    use super::*;
    use tree_sitter::Parser;

    /// Parse a `use` declaration and flatten its argument.
    fn flatten(source: &str) -> Vec<UseItem> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let root = tree.root_node();
        let use_decl = root.named_child(0).expect("a use_declaration");
        assert_eq!(use_decl.kind(), "use_declaration");
        let arg = use_decl
            .child_by_field_name("argument")
            .expect("an argument");
        let mut out = Vec::new();
        flatten_use_tree(arg, source.as_bytes(), &mut out);
        out
    }

    fn item(path: &[&str], alias: Option<&str>, glob: bool) -> UseItem {
        UseItem {
            path: path.iter().map(|s| s.to_string()).collect(),
            alias: alias.map(str::to_string),
            glob,
        }
    }

    #[test]
    fn plain_scoped_path_binds_its_last_segment() {
        assert_eq!(
            flatten("use a::b::c;"),
            vec![item(&["a", "b", "c"], Some("c"), false)]
        );
    }

    #[test]
    fn as_rename_binds_the_alias() {
        assert_eq!(
            flatten("use a::b as c;"),
            vec![item(&["a", "b"], Some("c"), false)]
        );
    }

    #[test]
    fn groups_and_nested_groups_expand_with_their_prefix() {
        assert_eq!(
            flatten("use a::{b, c::d};"),
            vec![
                item(&["a", "b"], Some("b"), false),
                item(&["a", "c", "d"], Some("d"), false),
            ]
        );
        assert_eq!(
            flatten("use a::{b::{c, d as e}, f};"),
            vec![
                item(&["a", "b", "c"], Some("c"), false),
                item(&["a", "b", "d"], Some("e"), false),
                item(&["a", "f"], Some("f"), false),
            ]
        );
    }

    #[test]
    fn self_in_a_group_binds_the_module_itself() {
        assert_eq!(
            flatten("use a::b::{self, c};"),
            vec![
                item(&["a", "b"], Some("b"), false),
                item(&["a", "b", "c"], Some("c"), false),
            ]
        );
    }

    #[test]
    fn glob_imports_record_the_module_with_no_alias() {
        assert_eq!(flatten("use a::b::*;"), vec![item(&["a", "b"], None, true)]);
    }

    #[test]
    fn crate_and_super_heads_are_kept_verbatim_for_resolution() {
        assert_eq!(
            flatten("use crate::x::y;"),
            vec![item(&["crate", "x", "y"], Some("y"), false)]
        );
        assert_eq!(
            flatten("use super::z;"),
            vec![item(&["super", "z"], Some("z"), false)]
        );
    }

    // ── macro-token-tree call scanning (S-162, CR-043) ───────────────────────

    /// Parse `source`, find the first `macro_invocation`, and scan its token
    /// tree for calls — the unit-testable core of the macro-arg coverage.
    fn macro_calls(source: &str) -> Vec<MacroCall> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        // Depth-first search for the first macro_invocation node.
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "macro_invocation" {
                return macro_call_refs(n, source.as_bytes());
            }
            for i in (0..n.child_count()).rev() {
                if let Some(c) = n.child(i) {
                    stack.push(c);
                }
            }
        }
        panic!("no macro_invocation in source");
    }

    fn path(target: &str) -> MacroCall {
        MacroCall { target: target.to_string(), form: RefForm::Path, line: 1, self_receiver: false, arg_count: None }
    }
    fn method(target: &str) -> MacroCall {
        MacroCall { target: target.to_string(), form: RefForm::Method, line: 1, self_receiver: false, arg_count: None }
    }

    /// The `(target, form)` pairs, ignoring line (the snippets are one line;
    /// binding does not key on the line, so the assertions compare on this).
    fn want(calls: &[MacroCall]) -> Vec<(String, RefForm)> {
        calls.iter().map(|c| (c.target.clone(), c.form)).collect()
    }

    #[test]
    fn bare_path_call_in_a_macro_arg_is_a_path_ref() {
        // The `activity_card`/`noscript_twin` shape: a free function called only
        // as a `format!` argument.
        let got = macro_calls(r#"format!("{x}", x = activity_card(stats))"#);
        assert_eq!(want(&got), want(&[path("activity_card")]));
    }

    #[test]
    fn receiver_method_call_in_a_macro_arg_is_a_method_ref() {
        // The `chip_class`/`chip_label` shape: a method called on a field
        // receiver inside a `format!` named argument.
        let got = macro_calls(r#"format!("{c}", c = self.state.chip_class())"#);
        // `state` is a field access (not followed by `(`), so it is not a call;
        // only `chip_class()` is, and the leading `.` makes it a method ref.
        assert_eq!(want(&got), want(&[method("chip_class")]));
    }

    #[test]
    fn only_a_call_on_self_itself_is_flagged_as_a_self_receiver() {
        // S-514: `self.label()` is a `self` call; `self.state.chip()` reaches
        // `chip` through a field, `other.label()` through another value.
        let got = macro_calls(r#"format!("{}{}{}", self.label(), self.state.chip(), other.label())"#);
        let flags: Vec<(String, bool)> = got.iter().map(|c| (c.target.clone(), c.self_receiver)).collect();
        assert_eq!(
            flags,
            vec![
                ("label".to_string(), true),
                ("chip".to_string(), false),
                ("label".to_string(), false),
            ]
        );
    }

    /// S-591: a call's count is its token tree's top-level comma-separated
    /// segments — a nested group is one argument, a trailing comma closes none
    /// — and unknown when a top-level token could hide a non-separating comma.
    #[test]
    fn a_macro_call_counts_its_top_level_arguments() {
        let got = macro_calls(
            r#"format!("{}", f(), g(a), h(a, b,), n((a, b)), o([a, b], c), k(|x, y| x), m(x as Map<A, B>))"#,
        );
        let counts: Vec<(String, Option<u32>)> = got.iter().map(|c| (c.target.clone(), c.arg_count)).collect();
        assert_eq!(
            counts,
            vec![
                ("f".to_string(), Some(0)),
                ("g".to_string(), Some(1)),
                ("h".to_string(), Some(2)),
                ("n".to_string(), Some(1)),
                ("o".to_string(), Some(2)),
                ("k".to_string(), None),
                ("m".to_string(), None),
            ]
        );
    }

    #[test]
    fn scoped_path_call_assembles_the_full_path() {
        let got = macro_calls(r#"write!(f, "{}", a::b::render(x))"#);
        assert_eq!(want(&got), want(&[path("a::b::render")]));
    }

    #[test]
    fn nested_calls_at_every_depth_are_recognised() {
        // The overview.rs shape: a call whose arguments are themselves calls.
        let got = macro_calls(r#"format!("{p}", p = dashboard_pair(&graph_card(s), &activity_card(t)))"#);
        assert_eq!(
            want(&got),
            want(&[
                path("dashboard_pair"),
                path("graph_card"),
                path("activity_card"),
            ])
        );
    }

    #[test]
    fn nested_macro_name_is_not_a_call() {
        // `format!` inside `write!`: the inner macro NAME (`format`) is followed
        // by `!`, not a `(`-group, so it is not captured — but the call inside
        // the inner macro (`foo()`) is.
        let got = macro_calls(r#"write!(out, "{}", format!("{}", foo()))"#);
        assert_eq!(want(&got), want(&[path("foo")]));
    }

    #[test]
    fn bracket_and_brace_groups_are_not_calls_but_their_contents_scan() {
        // `vec![foo()]` — the macro body is a `[…]` token tree; `bar[i]` indexing
        // is a `[…]` group (not a call); `Struct { … }` is a `{…}` group. Only
        // `foo()` is a call.
        let got = macro_calls(r#"vec![foo(), bar[i], Thing { f: 1 }]"#);
        assert_eq!(want(&got), want(&[path("foo")]));
    }

    #[test]
    fn macro_with_no_calls_yields_nothing() {
        assert!(macro_calls(r#"println!("just {} text", value)"#).is_empty());
    }

    #[test]
    fn turbofish_call_in_a_macro_is_a_known_recall_gap() {
        // Documented limitation: a turbofish-qualified call inside a macro arg is
        // NOT recognised — the `::<T>` run sits between the name and the
        // `(`-group, so no identifier is immediately followed by the call group.
        // This only omits a real call (false-live for the callee, never a
        // fabricated edge); a `<` never leaks into a captured path either.
        let got = macro_calls(r#"format!("{v}", v = parse::<u32>(s))"#);
        assert!(
            got.iter().all(|c| !c.target.contains('<')),
            "no angle-bracket fragment ever leaks into a captured path: {got:?}"
        );
        // The plain, non-turbofish call on the same source IS captured — proving
        // the gap is specific to the turbofish form, not the whole expression.
        let plain = macro_calls(r#"format!("{v}", v = parse(s))"#);
        assert_eq!(want(&plain), want(&[path("parse")]));
    }

    #[test]
    fn call_line_is_the_name_token_row() {
        let src = "format!(\n    \"{x}\",\n    x = activity_card(s),\n)";
        let got = macro_calls(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].target, "activity_card");
        assert_eq!(got[0].line, 3); // the `activity_card(s)` line (1-based)
    }
}
