//! A language's conditional-compilation regions reduced before parsing (S-555,
//! [CR-177], [FR-EX-23]) — the source pre-pass a descriptor opts into with
//! `preprocessor` ([`Preprocessor`]).
//!
//! C# writes one declaration two ways under `#if`: Newtonsoft's
//! `class JsonWriter` takes `: IAsyncDisposable` only `#if HAVE_ASYNC_DISPOSABLE`.
//! tree-sitter reads both branches as one text, and its error recovery hangs the
//! class's members off the module and invents a class named after the symbol.
//! Each `#if … #elif … #else … #endif` region is therefore reduced to its
//! **first** branch: the other branches and the region's `#if`, `#elif`,
//! `#else` and `#endif` lines are blanked to spaces; `#region`, `#pragma` and
//! the other directives are left to the parser. No symbol is evaluated, since
//! the build's defines are unknown, and the first branch is the one a project
//! usually writes for its current target.
//!
//! Blanking keeps every byte where it was: a blanked line keeps its length and
//! its line break, so each node's byte range, line and column are the raw
//! file's. A node with no region inside it reads the raw file's text; one that
//! encloses a region reads spaces where the directives and later branches were.
//!
//! Stated ceilings: a directive line is recognised by its first non-blank
//! character, and no string or comment is lexed, so a directive line inside a
//! multi-line string or block comment counts too. One that opens and closes its
//! own region there only blanks text, but a lone `#else`, `#elif` or `#endif`
//! pairs with a real region around it: it ends that region's first branch and
//! blanks the code after it, or unbalances the file. A region left open or
//! closed twice (an `#endif` with no `#if`) reduces nothing: the file is parsed
//! as written, as before.
//!
//! [CR-177]: ../../../docs/requests/CR-177-csharp-calls-members-routes-partials-and-typed-receivers-resolve.md
//! [FR-EX-23]: ../../../docs/specs/requirements/FR-EX-23.md

use std::borrow::Cow;

use crate::plugin::Preprocessor;

/// The source `preprocessor` hands the parser: `source` itself when the
/// language declares no pre-pass or the file has no region to reduce.
pub(crate) fn reduce(preprocessor: Preprocessor, source: &str) -> Cow<'_, str> {
    match preprocessor {
        Preprocessor::None => Cow::Borrowed(source),
        Preprocessor::CSharp => reduce_conditionals(source).map_or(Cow::Borrowed(source), Cow::Owned),
    }
}

/// A conditional directive, the four that open, divide and close a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directive {
    If,
    Elif,
    Else,
    Endif,
}

/// The conditional directive `line` is, if any: `#`, then optional blanks, then
/// the keyword as a whole word, all after the line's leading blanks. A byte
/// order mark is a leading blank: a file's first line may open with one.
fn directive(line: &str) -> Option<Directive> {
    let rest = line
        .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
        .strip_prefix('#')?
        .trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    match &rest[..end] {
        "if" => Some(Directive::If),
        "elif" => Some(Directive::Elif),
        "else" => Some(Directive::Else),
        "endif" => Some(Directive::Endif),
        _ => None,
    }
}

/// `source` with each region reduced to its first branch, or `None` when it has
/// no region or its regions do not balance.
fn reduce_conditionals(source: &str) -> Option<String> {
    if !source.contains('#') {
        return None;
    }
    let mut out = String::with_capacity(source.len());
    // One entry per open region: whether its first branch has ended. `ended`
    // counts the `true` entries, so a line is kept in constant time however
    // deep the regions nest.
    let mut regions: Vec<bool> = Vec::new();
    let mut ended = 0usize;
    let mut reduced = false;
    for line in source.split_inclusive('\n') {
        let keep = match directive(line) {
            Some(Directive::If) => {
                regions.push(false);
                false
            }
            Some(Directive::Elif | Directive::Else) => {
                let first_ended = regions.last_mut()?;
                if !*first_ended {
                    *first_ended = true;
                    ended += 1;
                }
                false
            }
            Some(Directive::Endif) => {
                if regions.pop()? {
                    ended -= 1;
                }
                false
            }
            None => ended == 0,
        };
        if keep {
            out.push_str(line);
        } else {
            reduced = true;
            blank(line, &mut out);
        }
    }
    (reduced && regions.is_empty()).then_some(out)
}

/// Append `line` with every character but its line break turned into as many
/// spaces as it has bytes.
fn blank(line: &str, out: &mut String) {
    for c in line.chars() {
        match c {
            '\n' | '\r' => out.push(c),
            _ => out.extend(std::iter::repeat_n(' ', c.len_utf8())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reduced(source: &str) -> String {
        reduce(Preprocessor::CSharp, source).into_owned()
    }

    #[test]
    fn the_first_branch_is_kept_and_the_rest_and_the_directives_are_blanked() {
        let source = "a\n#if X\nb\n#elif Y\nc\n#else\nd\n#endif\ne\n";
        assert_eq!(reduced(source), "a\n     \nb\n       \n \n     \n \n      \ne\n");
    }

    #[test]
    fn every_line_keeps_its_length_and_break() {
        let source = "class A\r\n#if HAVE_ASYNC_DISPOSABLE\r\n    : IAsyncDisposable\r\n#else\r\n    : IDisposable\r\n#endif\r\n{ }\r\n";
        let out = reduced(source);
        assert_eq!(out.len(), source.len());
        let lines = |s: &str| s.split_inclusive('\n').map(str::len).collect::<Vec<_>>();
        assert_eq!(lines(&out), lines(source));
        assert!(out.contains(": IAsyncDisposable"));
        assert!(!out.contains("IDisposable\r") && !out.contains("HAVE_ASYNC_DISPOSABLE"));
    }

    #[test]
    fn a_multi_byte_character_in_a_blanked_branch_becomes_one_space_per_byte() {
        let source = "#if X\na\n#else\n// é\n#endif\n";
        let out = reduced(source);
        assert_eq!(out.len(), source.len());
        assert_eq!(out, "     \na\n     \n     \n      \n");
    }

    #[test]
    fn a_nested_region_reduces_inside_a_kept_branch_and_vanishes_in_a_blanked_one() {
        let source = "#if A\n#if B\nb\n#else\nnb\n#endif\n#else\n#if C\nc\n#endif\n#endif\n";
        let out = reduced(source);
        let kept: Vec<&str> = out.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect();
        assert_eq!(kept, ["b"]);
    }

    #[test]
    fn a_directive_may_be_indented_and_spaced_after_its_hash() {
        let source = "  #  if X\nx\n\t# else // other\ny\n  #endif\n";
        assert_eq!(reduced(source).trim(), "x");
    }

    #[test]
    fn a_byte_order_mark_before_the_first_directive_is_a_blank() {
        // Visual Studio writes C# files with one: the region still balances.
        let source = "\u{feff}#if X\na\n#else\nb\n#endif\n";
        let out = reduced(source);
        assert_eq!(out.len(), source.len());
        assert_eq!(out, "        \na\n     \n \n      \n");
    }

    #[test]
    fn a_word_that_only_starts_with_a_keyword_is_no_directive() {
        // `#ifdef`, `#endifx`, `#elsewhere` and `#iffy` open and close nothing;
        // `#region`, `#pragma` and `#define` are left for the parser.
        for source in ["#ifdef X\na\n", "#endifx\na\n", "#region R\na\n#endregion\n", "#pragma warning disable\n"] {
            assert!(matches!(reduce(Preprocessor::CSharp, source), Cow::Borrowed(_)), "{source:?}");
        }
        assert_eq!(directive("#elsewhere"), None);
        assert_eq!(directive("#iffy"), None);
        assert_eq!(directive("#if(X)"), Some(Directive::If));
        assert_eq!(directive("#if!X"), Some(Directive::If));
    }

    #[test]
    fn a_hash_after_code_on_its_line_is_no_directive() {
        let source = "var s = \"#if X\";\n";
        assert!(matches!(reduce(Preprocessor::CSharp, source), Cow::Borrowed(_)));
    }

    #[test]
    fn unbalanced_regions_leave_the_source_as_written() {
        for source in ["#if X\na\n", "a\n#endif\n", "#else\na\n", "#if X\n#endif\n#endif\n", "#elif Y\n#endif\n"] {
            assert!(matches!(reduce(Preprocessor::CSharp, source), Cow::Borrowed(_)), "{source:?}");
        }
    }

    #[test]
    fn a_language_without_the_pre_pass_is_parsed_as_written() {
        let source = "#if X\na\n#else\nb\n#endif\n";
        assert!(matches!(reduce(Preprocessor::None, source), Cow::Borrowed(s) if s == source));
    }
}
