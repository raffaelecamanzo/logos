//! The load roots a tree declares — what a bare Ruby `require 'a/b'` binds
//! against (S-566, [CR-179] §3.2 (2), [FR-RS-34]). The twin of
//! [`super::go_module`] for the languages whose plugin declares
//! `bare_specifiers = "load-path"`.
//!
//! `require 'rack/lint'` names no file by itself: Ruby searches its load path,
//! and the load path a repository contributes is each gem's `require_paths`
//! (`lib/` unless its gemspec says otherwise) plus the repository root. Those
//! are the roots read here, and nothing else — a require that no root holds a
//! file for (`require 'json'`, a gem from outside the tree) stays unbound
//! ([NFR-RA-05]).
//!
//! A gemspec is `*.gemspec` in a directory that holds, or is above, an indexed
//! file of an opted-in language: only those directories are listed, each at
//! most once, so a tree with no such file reads nothing. Its `require_paths`
//! are read from the gemspec's own text ([`require_paths`]); a gemspec whose
//! value is not a literal contributes no root, and an unreadable one is treated
//! as absent — losing a root only leaves a require unbound, never mis-bound.
//!
//! [CR-179]: ../../../docs/requests/CR-179-ruby-constants-requires-ancestry-and-rails-routes-resolve.md
//! [FR-RS-34]: ../../../docs/specs/requirements/FR-RS-34.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use super::rel_path::{dir_segments, extension_of, fold_path, names_suffixed_file, parent_of};

/// The extension a gem specification is written under.
const GEMSPEC_EXTENSION: &str = ".gemspec";

/// The `require_paths` a gem has when its gemspec assigns none.
const DEFAULT_REQUIRE_PATH: &str = "lib";

/// The load roots of the opted-in languages' files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LoadPath {
    /// The extensions of the files whose bare specifiers bind here (the
    /// plugins declaring `bare_specifiers = "load-path"`). Empty: none does.
    extensions: HashSet<String>,
    /// The project-relative load roots (`""` is the repository root), sorted
    /// and distinct.
    roots: Vec<String>,
}

impl LoadPath {
    /// The load roots a bare specifier of the file at `source_file` binds
    /// against, or `None` when the file's language declares no load path.
    pub(crate) fn roots_for(&self, source_file: &str) -> Option<&[String]> {
        self.extensions
            .contains(&extension_of(source_file))
            .then_some(self.roots.as_slice())
    }
}

/// Whether a project-relative path names a gem specification — a file a sync
/// must hand to the resolution pass whether or not it is indexed (it is a Ruby
/// file since S-564), because its `require_paths` move every bare require
/// binding in other files (S-566).
pub(crate) fn is_descriptor(rel: &str) -> bool {
    names_suffixed_file(rel, GEMSPEC_EXTENSION)
}

/// The `require_paths` a gemspec's text declares: the gem default `["lib"]`
/// when it assigns none, the last literal assignment when it does (each
/// `<< 'x'` or `+= [...]` adding to it), and `None` when the value is not a
/// literal this reader can state — an expression, a constant, a `Dir[…]` glob,
/// an interpolation, or a mutating call (`.push`, `.concat`, …).
///
/// Read forms, on the receiver the `Gem::Specification.new` block names
/// (`s.`, `spec.`, …): `require_paths = ["lib", 'ext']` (each string may be
/// `.freeze`d, as RubyGems writes them), `require_paths = %w[lib ext]`,
/// `require_paths = "lib"`, the singular `require_path = 'lib'`,
/// `require_paths << "ext"` and `require_paths += ["ext"]`. Only code is read:
/// a `# comment`, an `=begin`…`=end` block and a heredoc body are not, so a
/// mention of the attribute there assigns nothing.
pub(crate) fn require_paths(gemspec: &str) -> Option<Vec<String>> {
    let mut paths = vec![DEFAULT_REQUIRE_PATH.to_string()];
    // The line that ends the block or heredoc being skipped, if any.
    let mut skip_until: Option<String> = None;
    for raw in gemspec.lines() {
        if let Some(end) = &skip_until {
            let closed = raw.trim() == end || (end == "=end" && raw.starts_with("=end"));
            skip_until = skip_until.filter(|_| !closed);
            continue;
        }
        if raw.starts_with("=begin") {
            skip_until = Some("=end".to_string());
            continue;
        }
        let (code_end, shifts) = scan(raw);
        skip_until = heredoc_terminator(raw, &shifts);
        if let Some(rest) = after_attribute(raw[..code_end].trim()) {
            assign(&mut paths, rest)?;
        }
    }
    Some(paths)
}

/// Apply what follows the attribute on one code line (`rest`) to `paths`: an
/// append (`<< "a" << "b"`), an extension (`+= [...]`), an assignment
/// (`= ...`), or nothing for a read. `None` when the value is not a literal,
/// or the line mutates the list through a call this reader cannot state.
fn assign(paths: &mut Vec<String>, rest: &str) -> Option<()> {
    const MUTATING_CALLS: [&str; 7] = [".push", ".unshift", ".concat", ".replace", ".insert", ".prepend", ".append"];
    if rest.starts_with("<<") {
        let appended: Option<Vec<String>> = rest.split("<<").skip(1).map(|v| string_literal(v.trim())).collect();
        paths.extend(appended?);
        return Some(());
    }
    if let Some(value) = rest.strip_prefix("+=") {
        paths.extend(literal_list(value.trim())?);
        return Some(());
    }
    if let Some(value) = rest.strip_prefix('=').filter(|v| !v.starts_with(['=', '~'])) {
        *paths = literal_list(value.trim())?;
        return Some(());
    }
    (!MUTATING_CALLS.iter().any(|call| rest.starts_with(call))).then_some(())
}

/// One line's code read outside its string literals: where a comment starts
/// (the line's length when none does) and the byte offsets of every `<<`
/// before it. A `#` or `<<` inside a string literal — an interpolation's
/// included — is neither.
fn scan(line: &str) -> (usize, Vec<usize>) {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut shifts = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        match quote {
            Some(_) if escaped => escaped = false,
            Some(_) if b == b'\\' => escaped = true,
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'#' => return (i, shifts),
            None if b == b'<' && bytes.get(i + 1) == Some(&b'<') && (i == 0 || bytes[i - 1] != b'<') => shifts.push(i),
            None => {}
        }
    }
    (line.len(), shifts)
}

/// The terminator of a heredoc the line opens at one of its code `shifts`
/// (`<<~EOS`, `<<-EOS`, `<<EOS`, `<<~'EOS'`), whose body lines are text, not
/// code. An append (`<< "ext"`, `<<x`) opens none: a bare `<<` heredoc's name
/// starts upper-case.
fn heredoc_terminator(line: &str, shifts: &[usize]) -> Option<String> {
    shifts.iter().find_map(|&at| {
        let after = &line[at + 2..];
        let (squiggly, after) = match after.strip_prefix(['~', '-']) {
            Some(tail) => (true, tail),
            None => (false, after),
        };
        let after = after.trim_start_matches(['\'', '"', '`']);
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let first = name.chars().next()?;
        ((squiggly && (first.is_ascii_alphabetic() || first == '_')) || first.is_ascii_uppercase()).then_some(name)
    })
}

/// The text after `.require_paths` / `.require_path` on `line`, leading
/// whitespace trimmed — `None` when the line names neither.
fn after_attribute(line: &str) -> Option<&str> {
    let at = line.find(".require_path")?;
    let rest = &line[at + ".require_path".len()..];
    let rest = rest.strip_prefix('s').unwrap_or(rest);
    // `.require_paths_extra` is another attribute.
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(rest.trim_start())
}

/// A literal list of paths: `["a", 'b'.freeze]`, `%w[a b]` (any `%w`/`%W`
/// bracket), or one string, optionally `.freeze`d. `None` for any other value,
/// a `%W` interpolation and a word holding a bracket included.
fn literal_list(value: &str) -> Option<Vec<String>> {
    let value = unfrozen(value);
    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return inner
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(string_literal)
            .collect();
    }
    if let Some(body) = value.strip_prefix("%w").or_else(|| value.strip_prefix("%W")) {
        let close = match body.chars().next()? {
            '[' => ']',
            '(' => ')',
            '{' => '}',
            '<' => '>',
            _ => return None,
        };
        let inner = body[1..].strip_suffix(close)?;
        let words: Vec<String> = inner.split_whitespace().map(str::to_string).collect();
        let plain = |w: &String| !w.contains(['[', ']', '(', ')', '{', '}', '<', '>', '#', '\\', '"', '\'']);
        return words.iter().all(plain).then_some(words);
    }
    string_literal(value).map(|one| vec![one])
}

/// One quoted string with no interpolation or escape, unquoted, optionally
/// `.freeze`d; `None` for anything else.
fn string_literal(value: &str) -> Option<String> {
    let value = unfrozen(value);
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let inner = value[1..].strip_suffix(quote)?;
    (!inner.contains(['"', '\'', '#', '\\'])).then(|| inner.to_string())
}

/// `value` without a trailing `.freeze` call.
fn unfrozen(value: &str) -> &str {
    let value = value.trim();
    value.strip_suffix(".freeze").map_or(value, str::trim_end)
}

/// The load roots of the files under `root` whose extension is one of
/// `extensions`; `files` is every indexed file.
///
/// The roots are the repository root and each gemspec's `require_paths`,
/// folded against the gemspec's directory — one escaping the repository, or
/// written absolute, is no root ([NFR-RA-05]). Sorted and distinct, so the
/// result does not depend on the order the files were indexed in
/// ([NFR-RA-06]). Without a file of an opted-in language nothing is read and
/// nothing binds.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn discover<'p>(
    root: &Path,
    extensions: HashSet<String>,
    files: impl IntoIterator<Item = &'p str>,
) -> LoadPath {
    // An ancestor already present had its own ancestors added with it.
    let mut dirs: HashSet<String> = HashSet::new();
    for file in files.into_iter().filter(|f| extensions.contains(&extension_of(f))) {
        let mut dir = parent_of(file);
        while dirs.insert(dir.to_string()) && !dir.is_empty() {
            dir = parent_of(dir);
        }
    }
    if dirs.is_empty() {
        return LoadPath::default();
    }
    let mut roots: BTreeSet<String> = BTreeSet::from([String::new()]);
    for dir in &dirs {
        let at = if dir.is_empty() { root.to_path_buf() } else { root.join(dir) };
        let Ok(entries) = std::fs::read_dir(at) else { continue };
        let gemspecs = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| is_descriptor(name));
        for name in gemspecs {
            let rel = if dir.is_empty() { name } else { format!("{dir}/{name}") };
            roots.extend(gem_roots(root, &rel));
        }
    }
    LoadPath {
        extensions,
        roots: roots.into_iter().collect(),
    }
}

/// The load roots the gemspec at the project-relative `rel` contributes: its
/// `require_paths` folded against its directory, an absolute one or one
/// escaping the repository dropped. None for an unreadable gemspec or a
/// non-literal value.
fn gem_roots(root: &Path, rel: &str) -> Vec<String> {
    let Some(paths) = std::fs::read_to_string(root.join(rel))
        .ok()
        .and_then(|text| require_paths(&text))
    else {
        return Vec::new();
    };
    let base = dir_segments(rel);
    paths
        .iter()
        .filter(|p| !p.starts_with('/'))
        .filter_map(|p| fold_path(&base, p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rb() -> HashSet<String> {
        HashSet::from(["rb".to_string()])
    }

    #[test]
    fn a_gemspec_without_require_paths_has_the_gem_default() {
        let rack = "Gem::Specification.new do |s|\n  s.name = \"rack\"\n  s.files = Dir['lib/**/*']\nend\n";
        assert_eq!(require_paths(rack), Some(vec!["lib".to_string()]));
    }

    #[test]
    fn require_paths_reads_every_literal_form_and_the_last_assignment_wins() {
        let read = |line: &str| require_paths(&format!("Gem::Specification.new do |spec|\n  {line}\nend\n"));
        assert_eq!(read("spec.require_paths = [\"lib\", 'ext']"), Some(vec!["lib".into(), "ext".into()]));
        assert_eq!(read("spec.require_paths = %w[lib ext]"), Some(vec!["lib".into(), "ext".into()]));
        assert_eq!(read("spec.require_paths = %w(src)"), Some(vec!["src".into()]));
        assert_eq!(read("spec.require_paths = \"src\" # the sources"), Some(vec!["src".into()]));
        assert_eq!(read("spec.require_path = 'src'"), Some(vec!["src".into()]));
        assert_eq!(read("spec.require_paths << \"ext\""), Some(vec!["lib".into(), "ext".into()]));
        assert_eq!(
            require_paths("s.require_paths = ['a']\ns.require_paths = ['b']\n"),
            Some(vec!["b".into()])
        );
        // A comment line naming the attribute is no assignment.
        assert_eq!(read("# s.require_paths = ['src']"), Some(vec!["lib".into()]));
    }

    #[test]
    fn a_value_that_is_not_a_literal_declares_no_require_paths() {
        let read = |line: &str| require_paths(&format!("Gem::Specification.new do |s|\n  {line}\nend\n"));
        assert_eq!(read("s.require_paths = Dir['lib']"), None);
        assert_eq!(read("s.require_paths = PATHS"), None);
        assert_eq!(read("s.require_paths = [\"#{root}/lib\"]"), None);
        // A comparison, and a near-miss attribute, assign nothing.
        assert_eq!(read("raise unless s.require_paths == ['lib']"), Some(vec!["lib".into()]));
        assert_eq!(read("s.require_paths_extra = ['src']"), Some(vec!["lib".into()]));
    }

    #[test]
    fn only_code_assigns_so_a_mention_in_a_comment_block_or_heredoc_assigns_nothing() {
        // The S-566 review's reproductions: each of these read `src` before.
        let lib = Some(vec!["lib".to_string()]);
        assert_eq!(require_paths("s.files = Dir['lib/**/*'] # was: s.require_paths = ['src']\n"), lib);
        assert_eq!(require_paths("=begin\n  s.require_paths = ['src']\n=end\n"), lib);
        let heredoc = "s.description = <<~EOS\n  s.require_paths = ['src']\nEOS\ns.name = 'x'\n";
        assert_eq!(require_paths(heredoc), lib);
        assert_eq!(require_paths("s.description = <<-EOS\n  s.require_paths = ['src']\n  EOS\n"), lib);
        // A `<<` inside a string opens no heredoc: the assignment after it reads.
        assert_eq!(
            require_paths("s.summary = \"a <<EOS b\"\ns.require_paths = ['src']\n"),
            Some(vec!["src".to_string()])
        );
        // A `#` inside a string is no comment.
        assert_eq!(require_paths("s.require_paths = ['src#x']\n"), None, "a `#` in a path is refused, never cut");
    }

    #[test]
    fn a_trailing_comment_and_a_frozen_string_are_read_as_the_literal_they_follow() {
        let read = |line: &str| require_paths(&format!("Gem::Specification.new do |s|\n  {line}\nend\n"));
        let src = Some(vec!["src".to_string()]);
        // RubyGems writes `["lib".freeze]` in the gemspecs it generates.
        assert_eq!(read("s.require_paths = [\"src\".freeze]"), src);
        assert_eq!(read("s.require_paths = [\"src\"] # don't use lib"), src);
        assert_eq!(read("s.require_paths = %w[src] # see [ext]"), src);
        assert_eq!(read("s.require_paths = %w[src].freeze"), src);
        assert_eq!(read("s.require_paths += ['ext']"), Some(vec!["lib".into(), "ext".into()]));
        assert_eq!(read("s.require_paths << 'a' << 'b'"), Some(vec!["lib".into(), "a".into(), "b".into()]));
    }

    #[test]
    fn an_interpolated_word_or_a_mutating_call_declares_no_require_paths() {
        let read = |line: &str| require_paths(&format!("Gem::Specification.new do |s|\n  {line}\nend\n"));
        assert_eq!(read("s.require_paths = %W[#{x}/lib]"), None);
        assert_eq!(read("s.require_paths = %w[lib] ]"), None);
        assert_eq!(read("s.require_paths << File.join('ext')"), None);
        assert_eq!(read("s.require_paths.push('ext')"), None);
        assert_eq!(read("s.require_paths.concat(%w[ext])"), None);
        // Reading the attribute assigns nothing.
        assert_eq!(read("puts s.require_paths.inspect"), Some(vec!["lib".into()]));
    }

    #[test]
    fn is_descriptor_names_a_gemspec_only() {
        assert!(is_descriptor("rack.gemspec"));
        assert!(is_descriptor("gems/a/a.gemspec"));
        // Case-sensitive, as RubyGems' `*.gemspec` glob is.
        assert!(!is_descriptor("gems/a/A.GEMSPEC"));
        assert!(!is_descriptor(".gemspec"));
        assert!(!is_descriptor("rack.gemspec.bak"));
        assert!(!is_descriptor("lib/gemspec.rb"));
    }

    #[test]
    fn discover_reads_each_gemspec_above_a_ruby_file_and_always_the_repository_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("rack.gemspec"), "Gem::Specification.new do |s|\nend\n").unwrap();
        std::fs::create_dir_all(root.join("gems/tool/src")).unwrap();
        std::fs::write(
            root.join("gems/tool/tool.gemspec"),
            "Gem::Specification.new do |s|\n  s.require_paths = ['src', '../../../escape', '/abs']\nend\n",
        )
        .unwrap();
        // A gemspec above no Ruby file is never listed.
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/other.gemspec"), "s.require_paths = ['x']\n").unwrap();
        let files = ["lib/rack.rb", "gems/tool/src/tool.rb", "docs/readme.md"];
        let load = discover(root, rb(), files);
        assert_eq!(load.roots_for("lib/rack.rb"), Some(&["".to_string(), "gems/tool/src".into(), "lib".into()][..]));
        assert_eq!(load.roots_for("web/app.ts"), None, "a language with no load path");
    }

    #[test]
    fn discover_reads_every_gemspec_of_a_directory_and_a_non_literal_one_adds_no_root() {
        // A gem shipped twice (`foo.gemspec` + `foo-java.gemspec`) contributes
        // both; one whose `require_paths` is computed contributes nothing — not
        // even the default `lib`.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.gemspec"), "Gem::Specification.new do |s|\nend\n").unwrap();
        std::fs::write(root.join("b.gemspec"), "Gem::Specification.new do |s|\n  s.require_paths = ['ext']\nend\n").unwrap();
        std::fs::create_dir_all(root.join("gems/c")).unwrap();
        std::fs::write(root.join("gems/c/c.gemspec"), "Gem::Specification.new do |s|\n  s.require_paths = Dir['lib']\nend\n").unwrap();
        let load = discover(root, rb(), ["lib/a.rb", "gems/c/lib/c.rb"]);
        assert_eq!(load.roots_for("lib/a.rb"), Some(&["".to_string(), "ext".into(), "lib".into()][..]));
    }

    #[test]
    fn discover_reads_nothing_without_a_file_of_an_opted_in_language() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("rack.gemspec"), "").unwrap();
        assert_eq!(discover(tmp.path(), rb(), ["src/main.rs"]), LoadPath::default());
        assert_eq!(discover(tmp.path(), HashSet::new(), ["lib/rack.rb"]), LoadPath::default());
    }
}
