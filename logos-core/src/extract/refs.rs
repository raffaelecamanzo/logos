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

use std::collections::HashSet;

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
    /// The receiver of a non-`self` method call whose type the file may prove
    /// (S-610, [FR-RS-42]): `None` for a path call, a `self.f()` call and a
    /// receiver no proof form reads — a chain, a path, a literal, a call result.
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    pub receiver: Option<MacroReceiver>,
}

/// A receiver written inside a macro's token tree that [FR-RS-42]'s proof forms
/// can type — the two the `references` query marks outside one (`variable` and
/// `self_field`).
///
/// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MacroReceiver {
    /// `x.f()`: a plain name — not preceded by a `.` (a field of something) or
    /// a `::` (a path), and not followed by anything but the `.`.
    Name(String),
    /// `self.x.f()`: a field of the caller's own struct.
    OwnField(String),
}

/// Walk a Rust `macro_invocation`'s token tree(s) for the call-shaped token
/// sequences the `references` query cannot see — tree-sitter does not parse a
/// macro's `token_tree` body as expressions, so `call_expression` /
/// `field_expression` patterns never match inside it (the documented S-011
/// limitation, lifted here for the `Calls` relation, S-162 / [CR-043] §3.2).
///
/// A call is an `identifier` immediately followed (past a turbofish, below) by
/// a `(`-delimited `token_tree`, with no intervening `!` (a `!` makes the
/// identifier a *nested macro* name — `format!(…)` — not a function call). It is a receiver-method
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
/// A call written with a turbofish records the path it records outside a macro
/// (S-610): `Vec::<u8>::new()` is `Vec::new`, `T::make::<u8>()` is `T::make`,
/// `f::<T>()` is `f` — the `::<…>` run is skipped, both between a path's
/// segments and between the name and its `(`-group, and never scanned for
/// calls (`Box::<dyn Fn(u8)>::new` calls no `Fn`). The run is closed by
/// counting `<` against `>` / `>>`, and a `::<` that never closes records
/// nothing. A `>` before a call group that no `::<` opens (`a < b && c > (d)`)
/// is a comparison, not a turbofish. A turbofish *method* call (`x.f::<T>()`)
/// records no row, as outside a macro, where no query pattern captures it. A
/// qualified path (`<T as Tr>::m()`) is not read here: its call records the
/// bare `m`.
///
/// A method call also carries the receiver a proof form can type
/// ([`MacroReceiver`]); `extract/receiver.rs` proves it, as for a call outside a
/// macro. A name a pattern inside the macro binds (`|m| m.f()`, `let m`,
/// `for m`, `Some(m) =>` / `Some(m) if`) is no typable receiver
/// ([`bound_names`]): it is not the caller's `m`. A binding form of a user
/// macro is not seen.
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
            let mut bound = HashSet::new();
            bound_names(child, source, &mut bound);
            for call in &mut out {
                if matches!(&call.receiver, Some(MacroReceiver::Name(n)) if bound.contains(n.as_str())) {
                    call.receiver = None;
                }
            }
        }
    }
    out
}

/// The identifiers of `tt`'s children in `range`, descending into groups, except
/// the arguments of a call written with a lowercase name (`recv(rx)`), which
/// read names and bind none; `Some(x)` and `Point(x)` still do.
fn pattern_idents<'a>(tt: Node<'_>, range: std::ops::Range<usize>, source: &'a [u8], out: &mut HashSet<&'a str>) {
    for i in range {
        let Some(child) = tt.child(i) else { continue };
        match child.kind() {
            "identifier" => out.extend(child.utf8_text(source).ok()),
            "token_tree" => {
                let call = i > 0
                    && tt.child(i - 1).is_some_and(|p| {
                        p.kind() == "identifier"
                            && p.utf8_text(source).is_ok_and(|t| t.starts_with(|c: char| c.is_lowercase() || c == '_'))
                    });
                if !call {
                    pattern_idents(child, 0..child.child_count(), source, out);
                }
            }
            _ => {}
        }
    }
}

/// Every name a pattern inside `tt` (recursively) may bind: the parameters
/// between a closure's `|`s, the pattern after `let` / `for`, and the run
/// before a match arm's `=>` or a guard's `if` back to the preceding `,`, `;`
/// or `{ … }` (the previous arm). The run is over-read — it takes every
/// identifier in it — because the cost of a name wrongly read as bound is an
/// unproven receiver, and of one wrongly read as free a wrong type (S-610).
fn bound_names<'a>(tt: Node<'_>, source: &'a [u8], out: &mut HashSet<&'a str>) {
    let kinds: Vec<&str> = (0..tt.child_count()).filter_map(|i| tt.child(i)).map(|c| c.kind()).collect();
    let mut idents = |from: usize, to: usize| pattern_idents(tt, from..to, source, out);
    for (i, kind) in kinds.iter().enumerate() {
        match *kind {
            // `|a, (b, c): T|`: up to the closing `|` of the same group.
            "|" => {
                if let Some(close) = (i + 1..kinds.len()).find(|&j| kinds[j] == "|") {
                    idents(i + 1, close);
                }
            }
            "let" | "for" => {
                let end = (i + 1..kinds.len()).find(|&j| matches!(kinds[j], "=" | ":" | ";" | "in"));
                idents(i + 1, end.unwrap_or(kinds.len()));
            }
            "=>" | "if" => {
                // The pattern starts after the previous arm, statement or argument.
                let ends = |j: usize| matches!(kinds[j], "," | ";") || tt.child(j).is_some_and(|c| c.kind() == "token_tree" && c.child(0).is_some_and(|d| d.kind() == "{"));
                let start = (0..i).rev().find(|&j| ends(j)).map_or(0, |j| j + 1);
                idents(start, i);
            }
            _ => {}
        }
    }
    for c in (0..tt.child_count()).filter_map(|i| tt.child(i)).filter(|c| c.kind() == "token_tree") {
        bound_names(c, source, out);
    }
}

/// Scan one `token_tree`'s ordered children (named and anonymous) for call
/// shapes, recursing into every nested `token_tree` except a turbofish's type
/// arguments, which are types and never calls.
fn scan_token_tree(tt: Node<'_>, source: &[u8], out: &mut Vec<MacroCall>) {
    let mut i = 0;
    while i < tt.child_count() {
        let Some(child) = tt.child(i) else { break };
        if child.kind() == "token_tree" {
            if opens_with_paren(child) {
                if let Some(call) = call_before(tt, i, source) {
                    out.push(call);
                }
            }
            scan_token_tree(child, source, out);
        } else if child.kind() == "::" && tt.child(i + 1).is_some_and(|n| matches!(n.kind(), "<" | "<<")) {
            // A turbofish's type arguments: `Vec::<Box<dyn Fn(u8)>>::new()`. A
            // `{ … }` const argument holds expressions: its calls are scanned.
            if let Some(close) = angle_close(tt, i + 1) {
                for block in (i + 2..close).filter_map(|j| tt.child(j)).filter(|n| n.kind() == "token_tree" && !opens_with_paren(*n)) {
                    scan_token_tree(block, source, out);
                }
                i = close;
            }
        }
        i += 1;
    }
}

/// The call whose `(`-delimited argument group is `tt`'s child `group`, or
/// `None` when no name sits before it. The name is the `identifier` immediately
/// before the group, or, past a turbofish (`f::<T>(…)`, `x.collect::<Vec<_>>()`),
/// the one before the `::<…>` run. A `!` before the group (a nested macro) or a
/// `[` / `{` group (index, struct literal) is not a call; `opens_with_paren`
/// is the caller's.
fn call_before(tt: Node<'_>, group: usize, source: &[u8]) -> Option<MacroCall> {
    let at = |i: usize| tt.child(i);
    let name_idx = match group.checked_sub(1).and_then(at)?.kind() {
        "identifier" => group - 1,
        ">" | ">>" => {
            let open = angle_open(tt, group - 1)?;
            // `f::<T>(…)`: the `::` before the `<`, then the name.
            let sep = open.checked_sub(1).filter(|&s| at(s).is_some_and(|n| n.kind() == "::"))?;
            let name = sep.checked_sub(1).filter(|&n| at(n).is_some_and(|n| n.kind() == "identifier"))?;
            // `x.f::<T>()` records no row outside a macro either: the query
            // has no pattern for a generic method call, so neither does this.
            if name.checked_sub(1).is_some_and(|d| at(d).is_some_and(|p| p.kind() == ".")) {
                return None;
            }
            name
        }
        _ => return None,
    };
    let name_node = at(name_idx)?;
    let name = name_node.utf8_text(source).ok()?;
    let line = name_node.start_position().row as u32 + 1;
    let args = at(group)?;
    // Receiver-method call `.name(…)`: the `.` is an anonymous prev token.
    let preceded_by_dot = name_idx > 0 && at(name_idx - 1).is_some_and(|p| p.kind() == ".");
    if !preceded_by_dot {
        // Path call: assemble the leading `ident (:: ident)*` run ending at this
        // identifier into a `::`-joined path (a bare `foo` stays a single segment).
        return Some(MacroCall {
            target: assemble_path(tt, name_idx, source),
            form: RefForm::Path,
            line,
            self_receiver: false,
            arg_count: token_tree_arg_count(args),
            receiver: None,
        });
    }
    let kind_at = |i: Option<usize>| i.and_then(at).map(|n| n.kind());
    let before = |back: usize| name_idx.checked_sub(back);
    // `self.f()`: the receiver token is `self`, and nothing — no `.` —
    // precedes it (`self.x.f()` reaches `f` through the field `x`).
    let self_receiver = kind_at(before(2)) == Some("self") && kind_at(before(3)) != Some(".");
    // `self` is not an `identifier` token, so a `self.f()` has no typable receiver.
    let receiver = if kind_at(before(2)) == Some("identifier") {
        let text = at(name_idx - 2).and_then(|n| n.utf8_text(source).ok()).map(str::to_string);
        match (kind_at(before(3)), kind_at(before(4)), kind_at(before(5))) {
            // `self.x.f()`: a field of the caller's own struct.
            (Some("."), Some("self"), prev) if prev != Some(".") => text.map(MacroReceiver::OwnField),
            // `x.f()` — not `a.x.f()` (a field of something), `a::x.f()` (a path).
            (prev, ..) if prev != Some(".") && prev != Some("::") => text.map(MacroReceiver::Name),
            _ => None,
        }
    } else {
        None
    };
    Some(MacroCall {
        target: name.to_string(),
        form: RefForm::Method,
        line,
        self_receiver,
        arg_count: token_tree_arg_count(args),
        receiver,
    })
}

/// How a token moves the angle-bracket depth: `<` opens one, `>` closes one,
/// `>>` closes two, `<<` opens two. Any other token — `->` included — is none.
fn angle_depth(kind: &str) -> i32 {
    match kind {
        "<" => 1,
        "<<" => 2,
        ">" => -1,
        ">>" => -2,
        _ => 0,
    }
}

/// The index of the `>` that closes the `<` at `tt`'s child `open`, counting
/// nested generics; `None` when it never closes there.
fn angle_close(tt: Node<'_>, open: usize) -> Option<usize> {
    let mut depth = 0;
    for i in open..tt.child_count() {
        depth += angle_depth(tt.child(i)?.kind());
        if depth == 0 {
            return Some(i);
        }
        if depth < 0 {
            return None;
        }
    }
    None
}

/// The index of the `<` that opens the `>` at `tt`'s child `close`; `None` when
/// it never opens there, or the matching token is not a `<` / `<<`.
fn angle_open(tt: Node<'_>, close: usize) -> Option<usize> {
    let mut depth = 0;
    for i in (0..=close).rev() {
        let kind = tt.child(i)?.kind();
        depth -= angle_depth(kind);
        if depth == 0 {
            return matches!(kind, "<" | "<<").then_some(i);
        }
        if depth < 0 {
            return None;
        }
    }
    None
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
/// A turbofish between two segments (`Vec::<u8>::new`) is skipped, as the call
/// outside a macro records its path without it (S-610).
fn assemble_path(tt: Node<'_>, call_idx: usize, source: &[u8]) -> String {
    let kind_at = |i: usize| tt.child(i).map(|n| n.kind());
    let mut segs: Vec<&str> = Vec::new();
    let mut idx = call_idx as isize;
    while let Some(node) = usize::try_from(idx).ok().and_then(|u| tt.child(u)) {
        if node.kind() != "identifier" {
            break;
        }
        let Ok(text) = node.utf8_text(source) else { break };
        segs.push(text);
        // A preceding `::` continues the path; anything else ends it.
        let mut sep_idx = idx - 1;
        if usize::try_from(sep_idx).ok().and_then(kind_at) != Some("::") {
            break;
        }
        // `a::<T>::b`: the run before this `::` is a turbofish only when a
        // `::` precedes its `<`; the segment it follows is the next one.
        let before = usize::try_from(sep_idx - 1).ok();
        if before.and_then(kind_at).is_some_and(|k| k == ">" || k == ">>") {
            let open = before.and_then(|c| angle_open(tt, c));
            let Some(open) = open.filter(|&o| o > 0 && kind_at(o - 1) == Some("::")) else { break };
            sep_idx = open as isize - 1;
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
        MacroCall { target: target.to_string(), form: RefForm::Path, line: 1, self_receiver: false, arg_count: None, receiver: None }
    }
    fn method(target: &str) -> MacroCall {
        MacroCall { target: target.to_string(), form: RefForm::Method, line: 1, self_receiver: false, arg_count: None, receiver: None }
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

    /// A call written with a turbofish records the path the same call records
    /// outside a macro — never a bare name, and never a `<` fragment.
    #[test]
    fn a_turbofish_call_in_a_macro_records_its_path() {
        for (src, want_path) in [
            (r#"format!("{v}", v = parse::<u32>(s))"#, "parse"),
            (r#"vec![Vec::<u8>::new()]"#, "Vec::new"),
            (r#"assert!(T::make::<u8>())"#, "T::make"),
            (r#"assert!(f::<T>())"#, "f"),
            (r#"vec![a::b::<X>::c::<Y>(1)]"#, "a::b::c"),
            (r#"vec![Vec::<Vec<u8>>::new()]"#, "Vec::new"),
            (r#"vec![HashMap::<String, Vec<u8>>::with_capacity(4)]"#, "HashMap::with_capacity"),
            (r#"vec![Box::<dyn Fn(u8) -> u8>::new(g)]"#, "Box::new"),
        ] {
            let got = macro_calls(src);
            let paths: Vec<&str> = got.iter().filter(|c| c.form == RefForm::Path).map(|c| c.target.as_str()).collect();
            assert!(paths.contains(&want_path), "{src}: want {want_path}, got {got:?}");
            assert!(
                got.iter().all(|c| !c.target.contains('<') && !c.target.contains('>')),
                "no angle-bracket fragment ever leaks into a captured path: {got:?}"
            );
        }
    }

    /// The type arguments of a turbofish are types, never calls: `Fn(u8)` is a
    /// bound, not a call of a function named `Fn`.
    #[test]
    fn a_turbofish_argument_list_is_not_scanned_for_calls() {
        let got = macro_calls(r#"vec![Box::<dyn Fn(u8) -> u8>::new(g(1))]"#);
        assert_eq!(want(&got), want(&[path("Box::new"), path("g")]));
    }

    /// A turbofish call's argument count is its call group's.
    #[test]
    fn a_turbofish_call_counts_its_arguments() {
        let got = macro_calls(r#"vec![f::<T>(a, b), Vec::<u8>::new()]"#);
        let counts: Vec<(String, Option<u32>)> = got.iter().map(|c| (c.target.clone(), c.arg_count)).collect();
        assert_eq!(counts, vec![("f".to_string(), Some(2)), ("Vec::new".to_string(), Some(0))]);
    }

    /// A method call with a turbofish records no row — outside a macro no query
    /// pattern captures one — while its neighbours in the chain do.
    #[test]
    fn a_turbofish_method_call_in_a_macro_records_no_row() {
        let got = macro_calls(r#"assert!(xs.iter().collect::<Vec<_>>().is_empty())"#);
        assert_eq!(want(&got), want(&[method("iter"), method("is_empty")]));
    }

    /// Near misses of a turbofish: a comparison, a shift and a generic with no
    /// `::` before it are not turbofish calls.
    #[test]
    fn a_comparison_before_a_call_group_is_not_a_turbofish() {
        assert!(macro_calls(r#"assert!(a < b && c > (d))"#).is_empty());
        assert!(macro_calls(r#"assert!(a < b >> (d))"#).is_empty());
        // A `<…>` run no `::` precedes, one token off a turbofish.
        assert!(macro_calls(r#"assert!(a b < c > (d))"#).is_empty());
        // No identifier before the `::<…>`: no name to record.
        assert!(macro_calls(r#"assert!(::<T>(d))"#).is_empty());
    }

    /// A turbofish opened by `<<` (`::<<T as Tr>::X>`) is a turbofish, and a
    /// `{ … }` const argument in one still holds calls.
    #[test]
    fn a_turbofish_opened_by_a_shift_token_or_holding_a_block_is_read() {
        let got = macro_calls(r#"vec![Vec::<<T as Tr>::X>::new(), f::<<T as Tr>::X>(a)]"#);
        assert_eq!(want(&got), want(&[path("Vec::new"), path("f")]));
        let mut got: Vec<String> =
            macro_calls(r#"vec![f::<{ g(3) }>(1)]"#).into_iter().map(|c| c.target).collect();
        got.sort();
        assert_eq!(got, ["f", "g"]);
    }

    /// A name the macro itself binds — a closure parameter, a `let`, a `for`
    /// variable, a match or `matches!` pattern — is not the caller's binding of
    /// that name: its receiver is no typable one. A name no pattern in the macro
    /// spells stays one.
    #[test]
    fn a_name_the_macro_binds_is_not_a_typable_receiver() {
        let receiver_of_f = |src: &str| -> Option<MacroReceiver> {
            let got = macro_calls(src);
            let calls: Vec<&MacroCall> = got.iter().filter(|c| c.target == "f").collect();
            assert_eq!(calls.len(), 1, "{src}: {got:?}");
            calls[0].receiver.clone()
        };
        let name = Some(MacroReceiver::Name("x".to_string()));
        for src in [
            "assert!(x.f())",
            "assert!(v.iter().all(|y| y.g()), x.f())",
            "assert!(a | b, x.f())",
            "assert!(matches!(o, Some(y) if y.g()), x.f())",
            // `recv(x)` reads `x`; only `msg` is bound.
            "m!(select! { recv(x) -> msg => { x.f() } })",
            // An earlier arm's body is no part of a later arm's pattern.
            "m!(select! { recv(y) -> a => { x.g() } recv(z) -> b => { x.f() } })",
        ] {
            assert_eq!(receiver_of_f(src), name, "{src}");
        }
        for src in [
            "assert!(v.iter().all(|x| x.f()))",
            "assert!(v.iter().all(|x: &B| x.f()))",
            "assert!(v.iter().any(|(a, x)| x.f()))",
            "assert!(matches!(o, Some(x) if x.f()))",
            "m!({ let x = g(); x.f() })",
            "m!({ let mut x = g(); x.f() })",
            "m!({ if let Some(x) = g() { x.f() } })",
            "m!({ for x in v { x.f() } })",
            "m!(match o { Some(x) => x.f(), None => 0 })",
            "m!(match o { Point(a, x) => x.f(), None => 0 })",
        ] {
            assert_eq!(receiver_of_f(src), None, "{src}");
        }
    }

    /// An unclosed turbofish (`::<` with no `>`) records nothing and panics on
    /// nothing; the scan carries on past it.
    #[test]
    fn an_unclosed_turbofish_records_nothing() {
        assert!(macro_calls(r#"assert!(f::<T)"#).is_empty());
        let got = macro_calls(r#"assert!(f::<T, g(1))"#);
        assert_eq!(want(&got), want(&[path("g")]));
    }

    /// A plain-name receiver and a `self.field` receiver are what a call hands
    /// `extract/receiver.rs`; a chain, a path, a literal and a call result are
    /// not receivers it can type.
    #[test]
    fn only_a_plain_name_or_an_own_field_is_a_typable_receiver() {
        let got = macro_calls(
            r#"f!(m.a(), self.s.b(), self.c(), a.s.d(), p::m.e(), g().h(), 1.i(), &m.j(), x.k.l(), m.n::<T>(), m.o())"#,
        );
        let receivers: Vec<(String, Option<MacroReceiver>)> =
            got.iter().filter(|c| c.form == RefForm::Method).map(|c| (c.target.clone(), c.receiver.clone())).collect();
        let name = |n: &str| Some(MacroReceiver::Name(n.to_string()));
        assert_eq!(
            receivers,
            vec![
                ("a".to_string(), name("m")),
                ("b".to_string(), Some(MacroReceiver::OwnField("s".to_string()))),
                ("c".to_string(), None),
                ("d".to_string(), None),
                ("e".to_string(), None),
                ("h".to_string(), None),
                ("i".to_string(), None),
                ("j".to_string(), name("m")),
                ("l".to_string(), None),
                ("o".to_string(), name("m")),
            ]
        );
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
