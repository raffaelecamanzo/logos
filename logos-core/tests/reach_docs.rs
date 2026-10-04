//! The manual's language-support table and the README's level list are
//! **generated from the plugin descriptors and checked against them**
//! ([FR-PL-09], [S-570], [CR-180]).
//!
//! The two blocks live between `<!-- reach:begin -->` / `<!-- reach:end -->`
//! markers in `docs/howto/usage.md` and `README.md`. The test renders each block
//! from `Engine::languages()` — the surface that reads the descriptors — and
//! fails when the committed text differs, so a language that changes its
//! declared reach cannot leave the docs claiming the old one. To regenerate
//! after changing a descriptor:
//!
//! ```text
//! LOGOS_WRITE_REACH_DOCS=1 cargo test -p logos-core --test reach_docs
//! ```
//!
//! [FR-PL-09]: ../../docs/specs/requirements/FR-PL-09.md
//! [S-570]: ../../docs/planning/journal.md#s-570-every-language-declares-its-verified-cross-file-reach-and-scala-is-declared-same-file
//! [CR-180]: ../../docs/requests/CR-180-scala-is-declared-as-limited-support-and-every-language-declares-its-reach.md

#![cfg(all(
    feature = "lang-rust",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-scala",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-go",
    feature = "lang-python",
    feature = "lang-typescript",
    feature = "lang-c-sharp",
    feature = "lang-ruby",
    feature = "lang-php"
))]

use std::fs;
use std::path::PathBuf;

use logos_core::models::quality::LanguageDescriptor;
use logos_core::Engine;
use tempfile::TempDir;

const BEGIN: &str = "<!-- reach:begin (generated from plugins/*/plugin.toml — see logos-core/tests/reach_docs.rs) -->";
const END: &str = "<!-- reach:end -->";

/// Levels in the order the docs list them: most capable first.
const LEVELS: &[(&str, &str)] = &[
    (
        "resolved",
        "calls, imports and type relations bind across files",
    ),
    ("partial", "some relations bind across files, not all"),
    (
        "same-file",
        "references bind only inside the file that wrote them",
    ),
    (
        "symbols",
        "declarations are extracted; nothing binds across files",
    ),
];

/// The name a language goes by in prose, keyed by its descriptor name.
fn display_name(descriptor: &str) -> &'static str {
    match descriptor {
        "rust" => "Rust",
        "java" => "Java",
        "go" => "Go",
        "typescript" => "TypeScript (incl. JavaScript)",
        "tsx" => "TSX (incl. JSX)",
        "python" => "Python",
        "php" => "PHP",
        "c-sharp" => "C#",
        "kotlin" => "Kotlin",
        "ruby" => "Ruby",
        "scala" => "Scala",
        "c" => "C",
        "cpp" => "C++",
        other => panic!("no display name for code language `{other}` — add it to `display_name`"),
    }
}

/// The code languages that declare a reach, as `(level, display name, descriptor
/// name, cross-file set)`, ordered by level then name — a deterministic order, so
/// the generated text is stable.
fn declared() -> Vec<(String, &'static str, String, Vec<String>)> {
    let tmp = TempDir::new().unwrap();
    let info = Engine::open(tmp.path()).languages();
    assert!(info.load_error.is_none(), "{:?}", info.load_error);
    let mut rows: Vec<(usize, &'static str, String, String, Vec<String>)> = info
        .languages
        .iter()
        .filter_map(|l: &LanguageDescriptor| {
            let reach = l.reach.as_ref()?;
            let rank = LEVELS
                .iter()
                .position(|(level, _)| *level == reach.level)
                .unwrap_or_else(|| panic!("unknown level `{}` on {}", reach.level, l.name));
            Some((
                rank,
                display_name(&l.name),
                l.name.clone(),
                reach.level.clone(),
                reach.cross_file.clone(),
            ))
        })
        .collect();
    rows.sort();
    rows.into_iter()
        .map(|(_, display, name, level, cross)| (level, display, name, cross))
        .collect()
}

fn relations(cross_file: &[String]) -> String {
    if cross_file.is_empty() {
        "none".to_string()
    } else {
        cross_file
            .iter()
            .map(|r| format!("`{r}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The manual's table: one row per code language.
fn render_table(rows: &[(String, &'static str, String, Vec<String>)]) -> String {
    let mut out = String::from(
        "| Language | Reach | Bound across files |\n|---|---|---|\n",
    );
    for (level, display, _, cross) in rows {
        out.push_str(&format!("| {display} | `{level}` | {} |\n", relations(cross)));
    }
    out
}

/// The README's list: one line per level, naming each language at that level.
fn render_readme(rows: &[(String, &'static str, String, Vec<String>)]) -> String {
    let mut out = String::new();
    for (level, meaning) in LEVELS {
        let names: Vec<&str> = rows
            .iter()
            .filter(|(l, ..)| l == level)
            .map(|(_, display, ..)| *display)
            .collect();
        if !names.is_empty() {
            out.push_str(&format!("- **`{level}`** — {meaning}: {}\n", names.join(", ")));
        }
    }
    out
}

fn repo_file(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

/// Split `text` around its generated block: `(before-and-begin-marker, block,
/// end-marker-and-after)`.
fn split_block(text: &str) -> (&str, &str, &str) {
    let begin = text
        .find(BEGIN)
        .unwrap_or_else(|| panic!("the begin marker is missing: {BEGIN}"));
    let body_start = begin + BEGIN.len();
    let end = text[body_start..]
        .find(END)
        .map(|i| body_start + i)
        .unwrap_or_else(|| panic!("the end marker is missing: {END}"));
    (&text[..body_start], &text[body_start..end], &text[end..])
}

/// The block's content as the file holds it, with the surrounding blank line
/// the markers sit on stripped.
fn committed_block(text: &str) -> &str {
    split_block(text).1.trim_matches('\n')
}

fn with_block(text: &str, block: &str) -> String {
    let (head, _, tail) = split_block(text);
    format!("{head}\n{block}{tail}")
}

/// Check one file's block against `expected`, or rewrite it when
/// `LOGOS_WRITE_REACH_DOCS` is set.
fn check_or_write(rel: &str, expected: &str) {
    let path = repo_file(rel);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if std::env::var("LOGOS_WRITE_REACH_DOCS").as_deref() == Ok("1") {
        fs::write(&path, with_block(&text, expected)).unwrap();
        return;
    }
    assert_eq!(
        committed_block(&text),
        expected.trim_matches('\n'),
        "{rel}: the generated language-reach block drifted from the plugin descriptors; \
         regenerate with `LOGOS_WRITE_REACH_DOCS=1 cargo test -p logos-core --test reach_docs`"
    );
}

#[test]
fn the_manual_table_matches_the_descriptors() {
    check_or_write("docs/howto/usage.md", &render_table(&declared()));
}

#[test]
fn the_readme_names_each_languages_level() {
    let rows = declared();
    check_or_write("README.md", &render_readme(&rows));

    // Whatever the block says, every code language is named in it exactly once.
    let readme = fs::read_to_string(repo_file("README.md")).unwrap();
    let block = committed_block(&readme);
    for (level, display, ..) in &rows {
        let line = block
            .lines()
            .find(|l| l.contains(&format!("**`{level}`**")))
            .unwrap_or_else(|| panic!("no README line for level `{level}`"));
        assert!(line.contains(display), "{display} is listed under `{level}`: {line}");
    }
}

/// The check has teeth: a table whose Ruby row claims more, or that omits a
/// language, is not the generated one.
#[test]
fn a_stale_table_is_detected() {
    let rows = declared();
    let generated = render_table(&rows);

    let ruby_raised = generated.replace("| Ruby | `same-file` | none |", "| Ruby | `partial` | `calls` |");
    assert_ne!(
        ruby_raised, generated,
        "the fixture edit must change the table, or this test proves nothing"
    );
    let doc = format!("{BEGIN}\n{ruby_raised}{END}\n");
    assert_ne!(committed_block(&doc), generated.trim_matches('\n'));

    let dropped: String = generated
        .lines()
        .filter(|l| !l.starts_with("| Kotlin"))
        .map(|l| format!("{l}\n"))
        .collect();
    let doc = format!("{BEGIN}\n{dropped}{END}\n");
    assert_ne!(committed_block(&doc), generated.trim_matches('\n'));

    // …and the committed block round-trips through the splice unchanged.
    let doc = format!("# t\n\n{BEGIN}\n{generated}{END}\n\ntail\n");
    assert_eq!(committed_block(&doc), generated.trim_matches('\n'));
    assert_eq!(with_block(&doc, &generated), doc);
}
