//! A recorded signature reduced to **one spelling per type** (S-830, [CR-260]
//! G1, [FR-RS-18]) — what the method-set pass compares a method with a spec by.
//!
//! The extraction records a callable's types as written (`extract::signature`:
//! `(ctx context.Context, e zc.Entry) error` is `(context.Context,zc.Entry)(error)`).
//! Two spellings name one type when they name one declaration, so each name in
//! the text is replaced by what it names, in the declaring file's context:
//!
//! - a bare name the package declares exactly one type of is that type,
//!   `#<node id>`; one it declares two of (a build-tag twin) names none;
//! - a bare name the package does not declare is a predeclared type
//!   ([`PREDECLARED`]), `byte` read as `uint8`, `rune` as `int32` and `any`
//!   as `interface{}`, which Go defines them as — a package declaring its own
//!   `any` keeps it its own type, never the empty interface;
//! - a qualified `x.Entry` is what the caller's import of `x` makes it: an
//!   in-repository type `#<node id>`, or `"<import path>".Entry` for a
//!   package outside the repository, so the alias never matters.
//!
//! Anything else makes the signature **unreducible** — a type parameter the
//! declaration binds (`[T](T)()`), a name neither the package nor the
//! language declares, a qualifier no import binds, a struct or interface
//! literal with members — and an unreducible signature matches nothing. That
//! can only miss an `Implements`, never invent one ([NFR-RA-05]).
//!
//! [CR-260]: ../../../../docs/requests/CR-260-go-interface-matching-by-signature-and-package.md
//! [FR-RS-18]: ../../../../docs/specs/requirements/FR-RS-18.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use crate::model::NodeId;

/// The language's predeclared type names, which a package may shadow.
const PREDECLARED: [&str; 22] = [
    "any", "bool", "byte", "comparable", "complex64", "complex128", "error", "float32", "float64", "int", "int8",
    "int16", "int32", "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr",
];

/// The words of a type's own syntax: never a name.
const KEYWORDS: [&str; 5] = ["chan", "func", "interface", "map", "struct"];

/// What a bare name names in one package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Own {
    /// The package's one type of that name.
    Type(NodeId),
    /// Two or more of them: no one type.
    Ambiguous,
    /// None: the name is not the package's.
    Absent,
}

/// How the names of one declaring file read.
pub(super) struct Names<'c> {
    /// A bare name in the declaring package.
    pub own: &'c dyn Fn(&str) -> Own,
    /// A qualified `head.name` through the file's import of `head`: its one
    /// spelling, `None` when the import binds nothing reducible.
    pub qualified: &'c dyn Fn(&str, &str) -> Option<String>,
}

/// Whether a recorded signature binds type parameters (`[T]…`): a generic
/// interface's record, or a method of a generic receiver's.
pub(super) fn binds_type_parameters(recorded: &str) -> bool {
    recorded.starts_with('[')
}

/// `recorded` with every type spelt one way, or `None` when it is unreducible
/// (the module docs).
pub(super) fn reduce(recorded: &str, names: &Names<'_>) -> Option<String> {
    let (bound, text) = match recorded.strip_prefix('[') {
        Some(rest) => {
            let (list, text) = rest.split_once(']')?;
            (list.split(',').collect::<Vec<_>>(), text)
        }
        None => (Vec::new(), recorded),
    };
    let with_members = |literal: &str| {
        let empty = format!("{literal}}}");
        text.match_indices(literal).any(|(at, _)| !text[at..].starts_with(&empty))
    };
    if with_members("interface{") || with_members("struct{") {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        if starts_name(chars[at]) {
            out.push_str(&spell_name(&chars, &mut at, &bound, names)?);
        } else {
            out.push(chars[at]);
            at += 1;
        }
    }
    Some(out)
}

/// Whether `c` starts a name.
fn starts_name(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

/// The one spelling of the name at `at`, consumed: a qualified `x.T` through
/// the file's import, a keyword as written, a bare name by [`bare`] — `None`
/// for a type parameter the declaration binds (`bound`) or any name that
/// reduces to nothing.
fn spell_name(chars: &[char], at: &mut usize, bound: &[&str], names: &Names<'_>) -> Option<String> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let word = take(chars, at, ident);
    if chars.get(*at) == Some(&'.') && chars.get(*at + 1).copied().is_some_and(starts_name) {
        *at += 1;
        let name = take(chars, at, ident);
        return (names.qualified)(&word, &name);
    }
    if KEYWORDS.contains(&word.as_str()) {
        return Some(word);
    }
    if bound.contains(&word.as_str()) {
        return None;
    }
    bare(&word, names)
}

/// The run of characters at `at` that `keep`, consumed.
fn take(chars: &[char], at: &mut usize, keep: impl Fn(char) -> bool) -> String {
    let start = *at;
    while *at < chars.len() && keep(chars[*at]) {
        *at += 1;
    }
    chars[start..*at].iter().collect()
}

/// A bare name's one spelling: the package's own type, else a predeclared one.
fn bare(word: &str, names: &Names<'_>) -> Option<String> {
    match (names.own)(word) {
        Own::Type(id) => Some(format!("#{}", id.0)),
        Own::Ambiguous => None,
        Own::Absent if PREDECLARED.contains(&word) => Some(
            match word {
                "byte" => "uint8",
                "rune" => "int32",
                "any" => "interface{}",
                other => other,
            }
            .to_string(),
        ),
        Own::Absent => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Entry` is the package's node 7, `Twin` declared twice; `zc` imports
    /// node 9's package and `ctx` the standard library's `context`.
    fn reduced(recorded: &str) -> Option<String> {
        let own = |name: &str| match name {
            "Entry" => Own::Type(NodeId(7)),
            "Twin" => Own::Ambiguous,
            _ => Own::Absent,
        };
        let qualified = |head: &str, name: &str| match head {
            "zc" if name == "Entry" => Some("#9".to_string()),
            "ctx" | "context" => Some(format!("\"context\".{name}")),
            _ => None,
        };
        reduce(recorded, &Names { own: &own, qualified: &qualified })
    }

    #[test]
    fn each_name_is_replaced_by_what_it_names() {
        assert_eq!(reduced("(context.Context,[]*Entry)(error)").as_deref(), Some("(\"context\".Context,[]*#7)(error)"));
        assert_eq!(reduced("(ctx.Context)()"), reduced("(context.Context)()"), "the alias never matters");
        assert_eq!(reduced("(zc.Entry)()").as_deref(), Some("(#9)()"));
        assert_eq!(reduced("([]byte,rune)()"), reduced("([]uint8,int32)()"), "byte is uint8, rune int32");
        assert_eq!(reduced("(...interface{})()"), reduced("(...any)()"));
        assert_ne!(reduced("(...any)()"), reduced("([]any)()"));
        assert_eq!(reduced("(map[string]chan struct{},func(int)error)()").as_deref(), Some("(map[string]chan struct{},func(int)error)()"));
        assert_eq!(reduced("([16]uint8)()").as_deref(), Some("([16]uint8)()"), "a length is no name");
    }

    #[test]
    fn an_unreducible_spelling_reduces_to_nothing() {
        assert_eq!(reduced("(Twin)()"), None, "a name the package declares twice");
        assert_eq!(reduced("(Missing)()"), None, "a name nothing declares");
        assert_eq!(reduced("(other.Entry)()"), None, "a qualifier no import binds");
        assert_eq!(reduced("(zc.Field)()"), None, "a name the imported package lacks");
        // Each guarded name is one the package declares: `Entry` alone would
        // reduce to `#7`.
        assert_eq!(reduced("[Entry](Entry)(bool)"), None, "a type parameter the declaration binds, shadowing a type");
        assert_eq!(reduced("[T](int)(bool)").as_deref(), Some("(int)(bool)"), "…unless no type spells it");
        assert_eq!(reduced("(struct{Entry string})()"), None, "a struct literal's member names");
        assert_eq!(reduced("(interface{Entry()})()"), None, "an interface literal's method names");
        assert_eq!(reduced("(chan struct{},*Entry)()").as_deref(), Some("(chan struct{},*#7)()"), "an empty struct is no literal with members");
    }

    #[test]
    fn a_package_type_shadows_the_predeclared_name() {
        let own = |name: &str| match name {
            "error" => Own::Type(NodeId(3)),
            "any" => Own::Type(NodeId(4)),
            _ => Own::Absent,
        };
        let qualified = |_: &str, _: &str| None;
        let shadowed = |recorded: &str| reduce(recorded, &Names { own: &own, qualified: &qualified });
        assert_eq!(shadowed("()(error)").as_deref(), Some("()(#3)"));
        assert_eq!(shadowed("(any)()").as_deref(), Some("(#4)()"), "the package's own `any`");
        assert_eq!(shadowed("(interface{})()").as_deref(), Some("(interface{})()"), "…is no empty interface");
    }
}
