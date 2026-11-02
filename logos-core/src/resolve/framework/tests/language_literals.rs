//! The resolver's language-literal guard (S-839, [CR-264], [NFR-MA-01]).
//!
//! The resolver is language-neutral: a language is told apart by the plugin
//! descriptor it was loaded from, never by spelling its id or extension in
//! `src/resolve` code. The first guard (`jvm_parity`'s
//! `no_language_specific_composition_code_exists`) forbade a closed list —
//! the four JVM ids and extensions — and only ran with both JVM
//! features on, so `extension_of(f) == "php"` passed it, as did the `"go"`,
//! `"rs"`, `"c"` and `"cpp"` sites it never listed.
//!
//! This guard derives the forbidden set from [`LanguageRegistry`] instead: the
//! `name` and every `extensions` entry of every loaded plugin, and every
//! extension-less file name a code plugin claims (S-840: `Gemfile`, with its
//! claim key `/Gemfile`), a second way to tell a language apart. A new plugin is
//! covered without editing the guard. A quoted literal of that set in
//! non-test `src/resolve` code fails unless [`ALLOWED`] names its file and the
//! literal with a reason, and an entry that matches no site fails too, so the
//! list cannot outlive the code it excuses.
//!
//! The set is the bare spellings the registry declares (`php`, not `.php`): a
//! dotted path suffix (`ends_with(".go")`) is a different literal and is not
//! seen. Widening it adds sites; it is a decision for a story of its own.
//!
//! The JVM guard keeps its own scope (every file but those named `tests.rs`,
//! so test modules and `*_tests.rs` files are in; the closed id list
//! unexempted); this one is additive.
//!
//! [CR-264]: ../../../../../docs/requests/CR-264-sprint-109-review-follow-ups.md
//! [NFR-MA-01]: ../../../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::plugin::LanguageRegistry;
use crate::plugin::registry::{claimed_file_names, claimed_name_keys};

/// A non-test `src/resolve` site that legitimately spells a registry language
/// id or extension: `(file under src/resolve/, literal, reason)`.
///
/// The key is the file's path **below `src/resolve/`**, not its basename, so
/// `binder/package_names.rs` and a future `framework/package_names.rs` are two
/// different files. The literal is exempt in that file alone; the same literal
/// in another file is still caught.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "binder/imported_scope.rs",
        "go",
        "the Go package-qualifier rule: `pkg.F` names a package member, so only a function is a \
         candidate. No other language's qualifier is function-only, so no descriptor field would \
         have a second user",
    ),
    (
        "binder/specifiers.rs",
        "go",
        "`go.mod` package resolution: only a Go file can name a Go package, so a TypeScript \
         `import 'shared'` beside a `go.mod` is never read as a path into the Go tree",
    ),
    (
        "dispatch.rs",
        "rs",
        "the dispatch pass is Rust-only by decision (CR-043, ADR-39): its structural walk is \
         written against the Rust grammar's node kinds. (`is_dead` gating for other languages \
         is a separate, descriptor-driven mechanism, the `[reach]` capability.)",
    ),
    (
        "mod.rs",
        "c",
        "the shared-header readout: `.h` is claimed by both the C and the C++ plugin (S-582), \
         and the figure states which of the two reads a repository's headers",
    ),
    (
        "mod.rs",
        "cpp",
        "the shared-header readout, as for `c` above: it names the pair that shares `.h`",
    ),
    (
        "reference_classes.rs",
        "go",
        "`IMPORTS_NOT_NAMED`: a Go import is a module path, not a name the name test can judge, so \
         every Go import is an internal candidate (see the module docs). Only Go has the property \
         today, which is why it is a constant and not a descriptor field yet",
    ),
];

/// The forbidden spellings of one plugin: its `name` and each of its
/// `extensions`, lower-cased and without a leading dot — the spelling a source
/// literal would use.
fn spellings(name: &str, extensions: &[String]) -> Vec<String> {
    std::iter::once(name)
        .chain(extensions.iter().map(|e| e.trim_start_matches('.')))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Every loaded plugin's [`spellings`], and the file names a code plugin claims
/// by exact name with their claim keys (`Gemfile`, `/Gemfile`): case kept, as
/// the registry matches a name exactly.
fn forbidden_literals(registry: &LanguageRegistry) -> BTreeSet<String> {
    registry
        .iter()
        .flat_map(|plugin| {
            spellings(plugin.name(), plugin.extensions())
                .into_iter()
                .chain(claimed_file_names(plugin).map(str::to_string))
                .chain(claimed_name_keys(plugin))
        })
        .collect()
}

/// Every `.rs` file under `dir` as `(path below src/resolve/, source)`,
/// verbatim — no comment stripping, for the reason `jvm_parity`'s
/// `resolver_sources` records: deciding what is a comment needs a lexer, and a
/// quoted literal in prose is cured by writing it in backticks.
fn read_tree(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, String)>) {
    for entry in std::fs::read_dir(dir).expect("resolve/ is readable") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            read_tree(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let source = std::fs::read_to_string(&path).expect("source is readable");
            out.push((
                path.strip_prefix(root).expect("under root").to_path_buf(),
                source,
            ));
        }
    }
}

/// The files a parent declares `#[cfg(test)] [#[path = "…"]] mod name;`: test
/// code in a file of its own, which the file name alone does not always say
/// (`route_method/matrix.rs`).
fn test_only_files(files: &[(PathBuf, String)]) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for (file, code) in files {
        // `foo.rs` declares `foo/name.rs`; `mod.rs` declares `name.rs` beside it.
        let dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
        let module_dir = if file.file_name().is_some_and(|f| f == "mod.rs") {
            dir.clone()
        } else {
            dir.join(file.file_stem().expect("file stem"))
        };
        let lines: Vec<&str> = code.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            if !is_test_attribute(line) {
                continue;
            }
            let mut path_attr = None;
            for next in &lines[at + 1..] {
                let t = next.trim();
                if let Some(p) = t.strip_prefix("#[path = \"") {
                    path_attr = p.strip_suffix("\"]").map(str::to_string);
                } else if t.starts_with("#[") || t.starts_with("//") {
                    continue;
                } else {
                    let decl = strip_visibility(t);
                    if let Some(name) = decl.strip_prefix("mod ").and_then(|m| m.strip_suffix(';'))
                    {
                        match &path_attr {
                            Some(p) => out.insert(normalize(&dir.join(p))),
                            None => out.insert(module_dir.join(format!("{name}.rs"))),
                        };
                    }
                    break;
                }
            }
        }
    }
    out
}

/// `t` without a leading `pub`, `pub(crate)`, `pub(super)` or `pub(in …)`.
fn strip_visibility(t: &str) -> &str {
    match t.strip_prefix("pub") {
        Some(rest) => match rest.strip_prefix('(') {
            Some(inner) => inner.split_once(") ").map_or(t, |(_, tail)| tail),
            None => rest.strip_prefix(' ').unwrap_or(t),
        },
        None => t,
    }
}

/// `a/b/../c` → `a/c`, so a `#[path = "../x.rs"]` compares with a listed file.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A `#[cfg(test)]`-gated item's attribute (`all(test, …)` included), at any
/// indent: a test-only associated function sits inside an `impl`.
fn is_test_attribute(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(all(test")
}

/// Whether an item's first line is the whole item: its brackets balance (string
/// literals set aside) and it ends in `;` (`mod tests;`, `use a::{b};`), `,`
/// (a struct field, enum variant or match arm) or `}` (`fn f() {}`).
fn ends_on_first_line(line: &str) -> bool {
    let (mut depth, mut quoted) = (0i32, false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            '{' | '[' | '(' if !quoted => depth += 1,
            '}' | ']' | ')' if !quoted => depth -= 1,
            _ => {}
        }
    }
    depth == 0 && line.trim_end().ends_with([';', ',', '}'])
}

/// `code` with its `#[cfg(test)]` items blanked (their lines stay, empty, so a
/// reported line number is the file's own).
///
/// An item ends on its first line when that line is complete
/// ([`ends_on_first_line`]); otherwise at the next line at the attribute's
/// indent that opens with `}` or `]`, which is where rustfmt closes a braced
/// item or a multi-line array. Anything else (a multi-line field type, a
/// closing line at another indent) is not recognised, and blanks on to the next
/// such line: the failure is a missed site, never a false one.
fn without_test_items(code: &str) -> String {
    let mut out = String::new();
    let mut lines = code.lines();
    while let Some(line) = lines.next() {
        if !is_test_attribute(line) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push('\n');
        let indent = line.len() - line.trim_start().len();
        let mut first = true;
        for item in lines.by_ref() {
            out.push('\n');
            let t = item.trim_start();
            if t.starts_with("#[") || t.starts_with("//") {
                continue;
            }
            if std::mem::take(&mut first) && ends_on_first_line(t) {
                break;
            }
            if item.len() - t.len() == indent && t.starts_with(['}', ']']) {
                break;
            }
        }
    }
    out
}

/// Whether `file` is test code by where it lives: a `tests.rs` or `*_tests.rs`
/// file, or anything below a `tests/` or `*_tests/` directory.
fn is_test_file(file: &Path) -> bool {
    let named = |stem: &str| stem == "tests" || stem.ends_with("_tests");
    file.with_extension("")
        .components()
        .any(|c| c.as_os_str().to_str().is_some_and(named))
}

/// The `(file, literal)` pairs where a non-test file spells a forbidden literal
/// in double quotes, with the line of the first occurrence.
fn sites(
    files: &[(PathBuf, String)],
    forbidden: &BTreeSet<String>,
) -> BTreeMap<(String, String), usize> {
    let test_only = test_only_files(files);
    let mut found = BTreeMap::new();
    for (file, code) in files {
        if is_test_file(file) || test_only.contains(file) {
            continue;
        }
        let code = without_test_items(code);
        for literal in forbidden {
            let quoted = format!("\"{literal}\"");
            if let Some(at) = code.find(&quoted) {
                let line = code[..at].matches('\n').count() + 1;
                found.insert(
                    (file.to_string_lossy().replace('\\', "/"), literal.clone()),
                    line,
                );
            }
        }
    }
    found
}

/// Every way `files` breaks the guard, one message each: an unallowed site, or
/// an allowlist entry that matches no site / names no registry literal.
fn violations(
    files: &[(PathBuf, String)],
    forbidden: &BTreeSet<String>,
    allowed: &[(&str, &str, &str)],
) -> Vec<String> {
    let found = sites(files, forbidden);
    let mut out = Vec::new();
    for ((file, literal), line) in &found {
        if !allowed.iter().any(|(f, l, _)| f == file && l == literal) {
            out.push(format!(
                "{file}:{line} spells the registry language id or extension \"{literal}\"; the \
                 resolver must read a language's facts from its plugin descriptor. If this is not \
                 a language branch, add (\"{file}\", \"{literal}\", <reason>) to ALLOWED"
            ));
        }
    }
    for (file, literal, reason) in allowed {
        if reason.trim().is_empty() {
            out.push(format!(
                "ALLOWED entry ({file}, \"{literal}\") has no reason"
            ));
        }
        if !forbidden.contains(*literal) {
            out.push(format!(
                "ALLOWED entry ({file}, \"{literal}\") is no registry language id or extension"
            ));
        } else if !found.contains_key(&(file.to_string(), literal.to_string())) {
            out.push(format!(
                "ALLOWED entry ({file}, \"{literal}\") is stale: that file no longer spells it"
            ));
        }
    }
    out
}

fn resolve_sources() -> Vec<(PathBuf, String)> {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/resolve"));
    let mut files = Vec::new();
    read_tree(root, root, &mut files);
    assert!(
        files.len() > 10,
        "src/resolve must contribute many files: {}",
        files.len()
    );
    files
}

#[test]
fn resolve_code_spells_no_registry_language_literal_without_a_reason() {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let forbidden = forbidden_literals(&registry);
    // The derivation must be the registry, not an empty set that passes.
    for expected in ["php", "go", "rs", "c", "cpp"] {
        assert!(
            forbidden.contains(expected),
            "registry set lacks {expected:?}: {forbidden:?}"
        );
    }
    for old in super::JVM_ID_LITERALS {
        assert!(
            forbidden.contains(old.trim_matches('"')),
            "the registry set must subsume the JVM guard's closed list: {old}"
        );
    }
    let found = violations(&resolve_sources(), &forbidden, ALLOWED);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

// ── The guard itself, on planted sources ─────────────────────────────────────

fn forbidden() -> BTreeSet<String> {
    forbidden_literals(&LanguageRegistry::load(std::env::temp_dir()).expect("registry loads"))
}

fn tree(files: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
    files
        .iter()
        .map(|(f, c)| (PathBuf::from(f), c.to_string()))
        .collect()
}

const PLANTED: &str = "fn is_php(f: &str) -> bool {\n    extension_of(f) == \"php\"\n}\n";

#[test]
fn the_forbidden_set_is_every_plugin_name_and_extension() {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let set = forbidden_literals(&registry);
    let mut expected = 0;
    for plugin in registry.iter() {
        assert!(
            set.contains(&plugin.name().to_ascii_lowercase()),
            "name of {}",
            plugin.name()
        );
        for ext in plugin.extensions() {
            assert!(
                set.contains(ext.trim_start_matches('.')),
                "extension {ext} of {}",
                plugin.name()
            );
            expected += 1;
        }
    }
    assert!(
        expected > set.len() / 2,
        "the registry must contribute extensions, not names only"
    );
    // S-840's claimed file names are a language's spelling too.
    for name in ["Gemfile", "Rakefile", "/Gemfile", "/Rakefile"] {
        assert!(set.contains(name), "claimed file name {name:?}: {set:?}");
    }
}

/// A branch on a claimed file name tells Ruby apart as surely as `"rb"` does.
#[test]
fn a_planted_claimed_file_name_comparison_fails() {
    let src = "fn is_build(p: &str) -> bool {\n    p == \"Gemfile\" || key == \"/Rakefile\"\n}\n";
    let found = violations(&tree(&[("planted.rs", src)]), &forbidden(), &[]);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().any(|f| f.contains("\"Gemfile\"")), "{found:?}");
    assert!(found.iter().any(|f| f.contains("\"/Rakefile\"")), "{found:?}");
}

/// The story's planted fixture: the old closed list (`kotlin`, `java`, `kt`,
/// `scala`) does not name `php`, so it passed; this guard names the file and
/// the literal.
#[test]
fn a_planted_extension_comparison_passes_the_old_guard_and_fails_this_one() {
    assert!(
        super::JVM_ID_LITERALS
            .iter()
            .all(|old| !PLANTED.contains(old)),
        "the planted source must be invisible to the closed list"
    );
    let found = violations(&tree(&[("planted.rs", PLANTED)]), &forbidden(), &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("planted.rs:2 "),
        "names file and line: {}",
        found[0]
    );
    assert!(
        found[0].contains("\"php\""),
        "names the literal: {}",
        found[0]
    );
}

#[test]
fn an_allowlist_entry_excuses_its_file_and_literal_only() {
    let allowed = [("planted.rs", "php", "a reason")];
    assert!(violations(&tree(&[("planted.rs", PLANTED)]), &forbidden(), &allowed).is_empty());
    // The same literal in another file is still a site.
    let found = violations(
        &tree(&[("planted.rs", PLANTED), ("other.rs", PLANTED)]),
        &forbidden(),
        &allowed,
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("other.rs:"), "{found:?}");
    // And another literal in the allowed file is too.
    let two = "fn f(e: &str) -> bool { e == \"php\" || e == \"go\" }\n";
    let found = violations(&tree(&[("planted.rs", two)]), &forbidden(), &allowed);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("\"go\""), "{found:?}");
}

#[test]
fn a_stale_or_unreasoned_allowlist_entry_fails() {
    let files = tree(&[("planted.rs", PLANTED)]);
    let stale = violations(
        &files,
        &forbidden(),
        &[("planted.rs", "php", "r"), ("gone.rs", "php", "r")],
    );
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert!(
        stale[0].contains("gone.rs") && stale[0].contains("stale"),
        "{stale:?}"
    );
    let wrong_literal = violations(
        &files,
        &forbidden(),
        &[("planted.rs", "php", "r"), ("planted.rs", "go", "r")],
    );
    assert_eq!(wrong_literal.len(), 1, "{wrong_literal:?}");
    assert!(
        wrong_literal[0].contains("\"go\"") && wrong_literal[0].contains("stale"),
        "{wrong_literal:?}"
    );
    let not_registry = violations(
        &files,
        &forbidden(),
        &[("planted.rs", "php", "r"), ("planted.rs", "cobol", "r")],
    );
    assert!(
        not_registry
            .iter()
            .any(|m| m.contains("cobol") && m.contains("no registry")),
        "{not_registry:?}"
    );
    let unreasoned = violations(&files, &forbidden(), &[("planted.rs", "php", "  ")]);
    assert!(
        unreasoned.iter().any(|m| m.contains("no reason")),
        "{unreasoned:?}"
    );
}

#[test]
fn test_code_is_not_a_site_but_the_code_after_it_is() {
    let files = tree(&[
        // A test module, an indented test-only fn in an impl, and a one-line
        // test-only `use`: none counts. The non-test fn between and after does.
        (
            "inline.rs",
            "struct S;\nimpl S {\n    #[cfg(test)]\n    pub(crate) fn for_tests() -> &'static str {\n        \"rs\"\n    }\n\n    fn real(&self) -> &'static str {\n        \"go\"\n    }\n}\n\
             #[cfg(test)]\nuse std::fmt::Write as _;\n\
             #[cfg(test)]\nmod tests {\n    fn t() { let _ = \"php\"; }\n}\n\
             fn after() -> &'static str { \"ruby\" }\n",
        ),
        ("a_tests.rs", PLANTED),
        ("tests.rs", PLANTED),
        ("tests/inner.rs", PLANTED),
        // Declared test-only by its parent, with and without a `#[path]`.
        ("parent.rs", "#[cfg(test)]\npub(crate) mod helper;\n#[cfg(test)]\n#[path = \"elsewhere.rs\"]\nmod renamed;\n"),
        ("parent/helper.rs", PLANTED),
        ("elsewhere.rs", PLANTED),
    ]);
    let found = violations(&files, &forbidden(), &[]);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|m| m.starts_with("inline.rs:9 ") && m.contains("\"go\"")),
        "{found:?}"
    );
    assert!(found.iter().any(|m| m.contains("\"ruby\"")), "{found:?}");
}

#[test]
fn a_near_miss_is_not_a_site() {
    let near = "const A: &str = \"phpx\";\nconst B: &str = \"xphp\";\nconst C: &str = \"php_ext\";\nconst D: &str = \"go.mod\";\nconst E: &str = \"PHP\";\n";
    let found = violations(&tree(&[("near.rs", near)]), &forbidden(), &[]);
    assert!(found.is_empty(), "{found:?}");
}

/// Each shape below is a `#[cfg(test)]` item that is not a braced block ending
/// at the attribute's indent. The first version of the stripper blanked on past
/// all of them, into the production code after, which hid a registry literal
/// there (`framework/fold.rs` has the field shape: 92 production lines).
#[test]
fn a_cfg_test_item_of_any_shape_hides_nothing_after_it() {
    let after = "fn real() -> &'static str {\n    \"php\"\n}\n";
    let shapes = [
        // A struct field, then the impl whose body holds the literal.
        "struct S {\n    a: u8,\n    #[cfg(test)]\n    n: u8,\n}\nimpl S {\n    fn new() -> &'static str {\n        \"php\"\n    }\n}\n",
        // A struct-literal field, an enum variant and a match arm.
        "fn mk() -> S {\n    S {\n        #[cfg(test)]\n        n: Cell::new(0),\n        a: 0,\n    }\n}\n",
        "enum E {\n    #[cfg(test)]\n    T,\n    A,\n}\n",
        "fn m(x: u8) -> u8 {\n    match x {\n        #[cfg(test)]\n        0 => 1,\n        _ => 2,\n    }\n}\n",
        // One-line braced items, with a brace inside a string.
        "#[cfg(test)]\nuse std::{fmt};\n",
        "#[cfg(test)]\nfn t() { }\n",
        "#[cfg(test)]\nmod tests {}\n",
        "#[cfg(test)]\nconst OPEN: &str = \"{\";\n",
        // A one-line `use`, then production code, then another test item whose
        // closing `}` would end a runaway blank early.
        "#[cfg(test)]\nuse std::fmt::Write as _;\nfn real() -> &'static str {\n    \"php\"\n}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n",
        // A multi-line array, closed by `];`.
        "#[cfg(test)]\nconst X: [&str; 1] = [\n    \"a\",\n];\n",
        // A multi-line signature whose `)` line is not the end.
        "#[cfg(test)]\nfn t(\n    a: u8,\n) -> u8 {\n    a\n}\n",
    ];
    for shape in shapes {
        let source = if shape.contains("\"php\"") {
            shape.to_string()
        } else {
            format!("{shape}{after}")
        };
        let found = violations(&tree(&[("shape.rs", &source)]), &forbidden(), &[]);
        assert_eq!(
            found.len(),
            1,
            "the literal after {shape:?} must still be seen: {found:?}"
        );
        assert!(found[0].contains("\"php\""), "{found:?}");
        let line = source
            .lines()
            .position(|l| l.contains("\"php\""))
            .expect("planted")
            + 1;
        assert!(
            found[0].starts_with(&format!("shape.rs:{line} ")),
            "line {line}: {found:?}"
        );
    }
}

#[test]
fn a_test_only_module_declared_with_any_visibility_is_not_scanned() {
    for vis in [
        "",
        "pub ",
        "pub(crate) ",
        "pub(super) ",
        "pub(in crate::resolve) ",
    ] {
        let parent = format!("#[cfg(test)]\n{vis}mod helper;\n");
        let files = tree(&[("parent.rs", &parent), ("parent/helper.rs", PLANTED)]);
        assert!(
            violations(&files, &forbidden(), &[]).is_empty(),
            "visibility {vis:?}"
        );
    }
}

#[test]
fn a_descriptor_spelling_is_lower_case_and_dotless() {
    let exts = [".PHP".to_string(), "Phtml".to_string()];
    assert_eq!(spellings("PHP", &exts), ["php", "php", "phtml"]);
}

/// The allowlist key is the path below `src/resolve/`, so the same basename in
/// two directories is two files — the reason `ALLOWED` is not keyed on a name.
#[test]
fn an_allowlist_entry_is_keyed_on_the_path_not_the_basename() {
    let files = tree(&[("a/x.rs", PLANTED), ("b/x.rs", PLANTED)]);
    let found = violations(&files, &forbidden(), &[("a/x.rs", "php", "r")]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("b/x.rs:2 "), "{found:?}");
    // A bare `x.rs` entry names neither directory's file: a site and a stale entry.
    let found = violations(
        &tree(&[("a/x.rs", PLANTED)]),
        &forbidden(),
        &[("x.rs", "php", "r")],
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found.iter().any(|m| m.starts_with("a/x.rs:2 ")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|m| m.contains("x.rs") && m.contains("stale")),
        "{found:?}"
    );
}

#[test]
fn a_literal_spelled_twice_is_one_site_at_its_first_line() {
    let twice = "fn a() -> &'static str { \"php\" }\nfn b() -> &'static str { \"php\" }\n";
    let found = violations(&tree(&[("x.rs", twice)]), &forbidden(), &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("x.rs:1 "), "{found:?}");
}

/// Test-only files declared from a `mod.rs` (the module dir is its own) and by
/// a `#[path]` that climbs out of the parent's directory.
#[test]
fn a_mod_rs_child_and_a_parent_relative_path_are_test_only() {
    let files = tree(&[
        ("dir/mod.rs", "#[cfg(test)]\nmod helper;\n"),
        ("dir/helper.rs", PLANTED),
        (
            "sub/parent.rs",
            "#[cfg(test)]\n#[path = \"../x.rs\"]\nmod x;\n",
        ),
        ("x.rs", PLANTED),
    ]);
    let found = violations(&files, &forbidden(), &[]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_feature_gated_test_item_and_a_comment_before_its_item_are_test_code() {
    let gated = "#[cfg(all(test, feature = \"x\"))]\nmod m {\n    const P: &str = \"php\";\n}\n";
    let commented = "#[cfg(test)]\n// why: a note ending like an item;\nmod m {\n    const P: &str = \"php\";\n}\n";
    for source in [gated, commented] {
        let found = violations(&tree(&[("x.rs", source)]), &forbidden(), &[]);
        assert!(found.is_empty(), "{source:?}: {found:?}");
    }
}
