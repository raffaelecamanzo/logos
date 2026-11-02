//! The `skips` of the project's own `.bandit` (HF-3 of sprint 110), merged
//! into the `--skip` of Bandit's template ([`super::registry::Arg::Skips`]).
//!
//! Bandit 1.7.10 gives a command-line `--skip` priority over the `skips` its
//! `.bandit` names and does not merge them (`_log_option_source` in
//! `bandit/cli/main.py`): the ini value is read only while the flag is unset.
//! The template passes `--skip B404,B603,B607` (S-844), so without this a
//! project that silenced checks in its `.bandit` would see them again. Logos
//! reads that file itself and passes the union.
//!
//! What Bandit 1.7.10 reads, from its source and a run of the managed copy:
//! - the file is parsed by `configparser.ConfigParser()` with its defaults
//!   (`utils.parse_ini_file`), and only `[bandit]`'s items are kept, `[DEFAULT]`
//!   included. A YAML or TOML `.bandit` has no section header, so the parser
//!   raises and Bandit ignores the file: only the INI form carries `skips`.
//! - any parse error — a line before the first section, a line with no `=` or
//!   `:`, a repeated section or key, a `%` interpolation that fails — makes it
//!   ignore the whole file, with a warning.
//! - `skips` is split on `,` with no trimming, and an id it does not know ends
//!   the run with exit 2 (`Unknown test found in profile`): `B101, B105`
//!   fails on ` B105`, and so do a trailing comma and a retired id (`B322`).
//! - for `-r .` it walks the whole tree and takes the one `.bandit` it finds
//!   there, exiting 2 when it finds more than one.
//!
//! What Logos reads ([`read`]): only the scan root's `.bandit`, a regular file
//! (never a symbolic link) of at most [`MAX_BYTES`], as UTF-8. It parses it as
//! `configparser` would and gives up wherever that would — and on a `%` in
//! `[bandit]` or `[DEFAULT]`, whose interpolation it does not emulate, and on a
//! control or non-ASCII whitespace character or a lone CR, which Python splits
//! or strips otherwise — and
//! takes `skips` from `[bandit]`, else from `[DEFAULT]`. Each id is trimmed
//! and an empty one dropped, so `B101, B105` adds both; an id that is not one
//! of the tests Bandit 1.7.10 ships ([`TEST_IDS`]) makes the whole file
//! malformed, so no id Bandit would end the run on is ever passed. (A newer
//! Bandit's own ids read as malformed too, until the pin moves.) A missing, unreadable or
//! malformed file adds nothing, so the run line is the template's alone, and
//! a malformed or unreadable one is named in a warning. The scan never fails
//! on it.
//!
//! The ids it adds can only narrow which checks run, which the repository's
//! `[security_scan]` switch can already do wholesale by disabling Bandit
//! ([NFR-SE-08]).
//!
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// The file Bandit reads its project options from.
pub(crate) const FILE: &str = ".bandit";

/// The largest `.bandit` read: far above any real one, and a bound on a file
/// that keeps growing.
pub(crate) const MAX_BYTES: u64 = 64 * 1024;

/// The skips the `.bandit` at `root` adds to the template's: empty when it is
/// missing, names none, or is unreadable or malformed — the last two with a
/// warning saying why. Never an error: the scan runs either way.
pub(crate) fn project_skips(root: &Path) -> Vec<String> {
    read(root).unwrap_or_else(|why| {
        tracing::warn!(
            target: "logos::security",
            "the project's {FILE} adds no skips to Bandit's: {why}"
        );
        Vec::new()
    })
}

/// The skips the `.bandit` at `root` names, in its order: `Ok` and empty when
/// there is no such file or it names none; `Err` with the reason when it is
/// unreadable or malformed.
pub(crate) fn read(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join(FILE);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("it could not be read: {e}")),
    };
    // Bandit's walk lists a directory among the directories, never as its file.
    if meta.is_dir() {
        return Ok(Vec::new());
    }
    if !meta.is_file() {
        return Err("it is not a regular file, and Logos follows no symbolic link".to_string());
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("it is larger than {} KiB", MAX_BYTES / 1024));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut bytes = Vec::new();
    options
        .open(&path)
        .and_then(|file| file.take(MAX_BYTES).read_to_end(&mut bytes))
        .map_err(|e| format!("it could not be read: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_string())?;
    parse(&text)
}

/// The ids `skips` names in a `.bandit`'s text, read as Bandit's
/// `configparser` reads it ([module docs](self)): `Ok` and empty when the file
/// has no `[bandit]` section or no `skips`; `Err` with the reason where the
/// parser would raise, or where an id is not one Bandit 1.7.10 ships.
pub(crate) fn parse(text: &str) -> Result<Vec<String>, String> {
    let sections = sections(text)?;
    let Some(bandit) = sections.get(SECTION) else {
        return Ok(Vec::new());
    };
    let defaults = sections.get(DEFAULT);
    // `items("bandit")` interpolates every value of both sections.
    if bandit.values().chain(defaults.into_iter().flat_map(HashMap::values)).any(|v| v.contains('%')) {
        return Err(format!("a value in [{SECTION}] or [{DEFAULT}] holds a `%`, which Logos does not interpolate"));
    }
    let Some(skips) = bandit.get(KEY).or_else(|| defaults.and_then(|d| d.get(KEY))) else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for id in skips.split(',').map(str::trim).filter(|id| !id.is_empty()) {
        if !is_test_id(id) {
            return Err(format!("`{KEY}` holds {id:?}, which is no test Bandit 1.7.10 ships"));
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

const SECTION: &str = "bandit";
const DEFAULT: &str = "DEFAULT";
const KEY: &str = "skips";

/// Every test id Bandit 1.7.10 ships — its plugins, its blacklist calls and
/// imports, and the legacy `B001` — read from the managed copy's
/// `extension_loader.MANAGER`. Bandit ends a run on any other id in `--skip`
/// (`Unknown test found in profile`, exit 2), so an id outside this list,
/// retired (`B322`) or never shipped, is one Logos never passes.
const TEST_IDS: [&str; 76] = [
    "B001", "B101", "B102", "B103", "B104", "B105", "B106", "B107", "B108", "B110", "B112", "B113", "B201",
    "B202", "B301", "B302", "B303", "B304", "B305", "B306", "B307", "B308", "B310", "B311", "B312", "B313",
    "B314", "B315", "B316", "B317", "B318", "B319", "B320", "B321", "B323", "B324", "B401", "B402", "B403",
    "B404", "B405", "B406", "B407", "B408", "B409", "B410", "B411", "B412", "B413", "B415", "B501", "B502",
    "B503", "B504", "B505", "B506", "B507", "B508", "B509", "B601", "B602", "B603", "B604", "B605", "B606",
    "B607", "B608", "B609", "B610", "B611", "B612", "B613", "B614", "B701", "B702", "B703",
];

/// Whether Bandit 1.7.10 ships a test with this id ([`TEST_IDS`]).
fn is_test_id(id: &str) -> bool {
    TEST_IDS.contains(&id)
}

/// Every section's keys and values, as `configparser.ConfigParser()` reads
/// them with its defaults — keys lowercased and trimmed, values trimmed, an
/// indented line deeper than its key continuing the value — or the reason it
/// would raise.
fn sections(text: &str) -> Result<HashMap<&str, HashMap<String, String>>, String> {
    if let Some(why) = foreign_character(text) {
        return Err(why);
    }
    let mut sections: HashMap<&str, HashMap<String, String>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut section: Option<&str> = None;
    // The key a deeper-indented line continues, and that key's indent.
    let mut open: Option<(String, usize)> = None;
    for (n, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') || value.starts_with(';') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let at = n + 1;
        match (section, &open) {
            (Some(name), Some((key, depth))) if indent > *depth => {
                let held = sections.get_mut(name).and_then(|s| s.get_mut(key)).expect("an open key is held");
                held.push('\n');
                held.push_str(value);
                continue;
            }
            _ => {}
        }
        if let Some(header) = header(value) {
            if header != DEFAULT && sections.contains_key(header) {
                return Err(format!("line {at}: section [{header}] is repeated"));
            }
            sections.entry(header).or_default();
            section = Some(header);
            open = None;
            continue;
        }
        let Some(name) = section else {
            return Err(format!("line {at}: a line before the first section header"));
        };
        let Some(split) = value.find(['=', ':']) else {
            return Err(format!("line {at}: a line with no `=` or `:`"));
        };
        let key = value[..split].trim_end().to_lowercase();
        if key.is_empty() {
            return Err(format!("line {at}: a value with no key"));
        }
        if !seen.insert((name, key.clone())) {
            return Err(format!("line {at}: `{key}` is repeated in [{name}]"));
        }
        let held = value[split + 1..].trim_start().to_string();
        sections.get_mut(name).expect("the section is held").insert(key.clone(), held);
        open = Some((key, indent));
    }
    Ok(sections)
}

/// A section header's name: `\[(?P<header>.+)\]` as configparser matches it,
/// from the start of the trimmed line to its last `]`, never empty.
fn header(value: &str) -> Option<&str> {
    value.strip_prefix('[').and_then(|v| v.rfind(']').map(|end| &v[..end])).filter(|h| !h.is_empty())
}

/// Why `text` holds a character Python reads otherwise, if it does. Python
/// splits lines on a lone CR too, strips `\x1c`..`\x1f` as whitespace, and
/// counts an indent in characters: past space, tab, LF and a CR before LF,
/// the two readers could disagree on a file's lines.
fn foreign_character(text: &str) -> Option<String> {
    let (at, c) = text.char_indices().find(|&(at, c)| {
        let lone_cr = c == '\r' && !text[at + 1..].starts_with('\n');
        lone_cr || (!matches!(c, ' ' | '\t' | '\n' | '\r') && (c.is_control() || c.is_whitespace()))
    })?;
    let line = text[..at].matches('\n').count() + 1;
    Some(format!("line {line}: it holds {c:?}, which Logos does not read as configparser would"))
}

/// The template's comma list `base` with each of `project` it lacks appended,
/// in order and once: `B404,B603,B607` and `B101, B603, B101` give
/// `B404,B603,B607,B101`.
pub(crate) fn merge(base: &str, project: &[String]) -> String {
    let mut ids: Vec<&str> = base.split(',').collect();
    for id in project {
        if !ids.contains(&id.as_str()) {
            ids.push(id);
        }
    }
    ids.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn skips_are_read_from_the_bandit_section_with_whitespace_tolerated() {
        assert_eq!(parse("[bandit]\nskips = B101, B105\n"), Ok(ids(&["B101", "B105"])));
        assert_eq!(parse("[bandit]\nskips=B101,B105"), Ok(ids(&["B101", "B105"])));
        // `:` is configparser's other delimiter, and keys are case-insensitive.
        assert_eq!(parse("[bandit]\nSkips: B101\n"), Ok(ids(&["B101"])));
        // A continuation line, a trailing comma and a CRLF ending.
        assert_eq!(parse("[bandit]\r\nskips = B101,\r\n    B105,\r\n"), Ok(ids(&["B101", "B105"])));
        // Comments and other keys and sections around it.
        let text = "# project options\n[other]\nskips = B999\n\n[bandit]\n; silenced\nexclude = ./tests\nskips = B311\ntargets = src\n";
        assert_eq!(parse(text), Ok(ids(&["B311"])));
        // An indented key is still a key, and `[DEFAULT]` flows into `[bandit]`.
        assert_eq!(parse("[bandit]\n  skips = B101\n"), Ok(ids(&["B101"])));
        assert_eq!(parse("[DEFAULT]\nskips = B110\n[bandit]\n"), Ok(ids(&["B110"])));
        assert_eq!(parse("[DEFAULT]\nskips = B110\n[bandit]\nskips = B112\n"), Ok(ids(&["B112"])));
        // `[DEFAULT]` may open twice, as long as no key repeats.
        assert_eq!(parse("[DEFAULT]\nx = 1\n[DEFAULT]\ny = 2\n[bandit]\nskips = B101\n"), Ok(ids(&["B101"])));
        // A blank or comment line does not end a value; text after a header's
        // last `]` is not part of its name.
        assert_eq!(parse("[bandit]\nskips = B101,\n\n# c\n    B105\n"), Ok(ids(&["B101", "B105"])));
        assert_eq!(parse("[bandit] trailing\nskips = B101\n"), Ok(ids(&["B101"])));
    }

    #[test]
    fn a_file_without_skips_adds_none() {
        for text in [
            "",
            "[bandit]\n",
            "[bandit]\nskips =\n",
            "[bandit]\nskips = , ,\n",
            "[bandit]\nexclude = ./tests\n",
            // `[DEFAULT]` alone: Bandit reads `[bandit]`'s items, and there is none.
            "[DEFAULT]\nskips = B101\n",
            "[tool]\nskips = B101\n",
        ] {
            assert_eq!(parse(text), Ok(Vec::new()), "{text:?}");
        }
    }

    #[test]
    fn a_malformed_file_is_refused_with_its_reason() {
        for text in [
            // YAML and TOML forms: no section header before the first key.
            "skips: [B101]\n",
            "skips = B101\n[bandit]\n",
            // `[tool.bandit]` parses, but `skips = [\"B101\"]` is no id list;
            // under `[bandit]` it is malformed.
            "[bandit]\nskips = [\"B101\"]\n",
            // Ids Bandit would end the run on, or not an id at all.
            "[bandit]\nskips = B101 # inline comment\n",
            "[bandit]\nskips = B10\n",
            "[bandit]\nskips = b101\n",
            "[bandit]\nskips = B1011\n",
            "[bandit]\nskips = X101\n",
            // Shaped as an id, but not one Bandit 1.7.10 ships: B322 was
            // retired, B999 never existed. Bandit would end the run on either.
            "[bandit]\nskips = B101, B322\n",
            "[bandit]\nskips = B999\n",
            // Ids are separated by commas; a line break alone joins two into one.
            "[bandit]\nskips = B101\n    B105\n",
            // What configparser raises on.
            "[bandit]\nskips = B101\nskips = B105\n",
            "[bandit]\nskips = B101\n[bandit]\n",
            "[DEFAULT]\nskips = B101\n[DEFAULT]\nskips = B105\n[bandit]\n",
            "[]\nskips = B101\n",
            "[bandit]\nskips B101\n",
            "[bandit]\n= B101\n",
            "[bandit\nskips = B101\n",
            "  continued\n[bandit]\nskips = B101\n",
            // An interpolation it does not emulate.
            "[bandit]\nskips = B101\nexclude = %(dir)s\n",
            "[DEFAULT]\nroot = 50%\n[bandit]\nskips = B101\n",
        ] {
            assert!(parse(text).is_err(), "{text:?} reads as {:?}", parse(text));
        }
        // Characters configparser splits or strips differently from this
        // reader: a lone CR ends a line there, `\x1c`..`\x1f` are whitespace
        // there, and an ideographic space indents by one there. Each hides a
        // repeated `skips` that makes Bandit ignore the file.
        for text in [
            "[bandit]\nskips = B101\nexclude = ./x\rskips = B105\n",
            "[bandit]\n\x1c\x1c\x1cexclude = ./x\n skips = B105\nskips = B101\n",
            "[bandit]\n   exclude = ./x\n\u{3000}\u{3000}skips = B105\nskips = B101\n",
            "[bandit]\nskips = B101\x0b\n",
        ] {
            assert!(parse(text).is_err(), "{text:?} reads as {:?}", parse(text));
        }
        // The reason is logged: a line the file joins into an id must not
        // start a forged log line of its own.
        let forged = parse("[bandit]\nskips = B101\n    2026-11-02T00:00:00Z  INFO logos::security: forged\n");
        let why = forged.expect_err("a joined id is no test id");
        assert!(!why.contains('\n') && why.contains(r"B101\n2026"), "{why}");
        // A `%` in a section Bandit does not read is not interpolated.
        assert_eq!(parse("[other]\nx = 50%\n[bandit]\nskips = B101\n"), Ok(ids(&["B101"])));
        // A key repeated in another section is no repeat.
        assert_eq!(parse("[other]\nskips = B105\n[bandit]\nskips = B101\n"), Ok(ids(&["B101"])));
    }

    /// The docs promise a warning naming why a `.bandit` adds nothing; a
    /// missing file says nothing.
    #[test]
    fn a_malformed_file_is_named_in_a_warning_and_a_missing_one_is_silent() {
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Captured {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let (bad, missing) = (tmp.path().join("bad"), tmp.path().join("missing"));
        fs::create_dir_all(&bad).unwrap();
        fs::create_dir_all(&missing).unwrap();
        fs::write(bad.join(FILE), "skips: [B101]\n").unwrap();
        let log = |root: &Path| {
            let captured = Captured::default();
            let writer = captured.clone();
            let subscriber =
                tracing_subscriber::fmt().with_writer(move || writer.clone()).with_ansi(false).finish();
            tracing::subscriber::with_default(subscriber, || {
                // Sibling tests reach the callsite from threads with no subscriber.
                tracing::callsite::rebuild_interest_cache();
                assert!(project_skips(root).is_empty());
            });
            let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
            text
        };
        let warned = log(&bad);
        assert!(warned.contains("WARN"), "{warned}");
        assert!(warned.contains(".bandit adds no skips to Bandit's: line 1:"), "{warned}");
        assert_eq!(log(&missing), "", "a missing file is silent");
    }

    #[test]
    fn the_templates_own_skips_are_tests_bandit_ships() {
        let bandit = crate::security::registry::scanner("bandit").unwrap();
        let base = bandit
            .run
            .iter()
            .find_map(|a| match *a {
                crate::security::registry::Arg::Skips(base) => Some(base),
                _ => None,
            })
            .expect("Bandit's template has its skips");
        assert!(base.split(',').all(is_test_id), "{base}");
    }

    #[test]
    fn the_merge_appends_the_projects_ids_the_template_lacks_in_order_once() {
        let base = "B404,B603,B607";
        assert_eq!(merge(base, &ids(&["B101", "B105"])), "B404,B603,B607,B101,B105");
        assert_eq!(merge(base, &ids(&["B603", "B101", "B404", "B101"])), "B404,B603,B607,B101");
        assert_eq!(merge(base, &ids(&["B607"])), base);
        assert_eq!(merge(base, &[]), base);
    }

    #[test]
    fn the_file_at_the_root_is_read_and_a_bad_one_adds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(read(root), Ok(Vec::new()), "no file");
        assert!(project_skips(root).is_empty());

        fs::write(root.join(FILE), "[bandit]\nskips = B101, B105\n").unwrap();
        assert_eq!(read(root), Ok(ids(&["B101", "B105"])));
        assert_eq!(project_skips(root), ids(&["B101", "B105"]));

        fs::write(root.join(FILE), "skips: [B101]\n").unwrap();
        assert!(read(root).is_err(), "malformed");
        assert!(project_skips(root).is_empty(), "a malformed file adds nothing");
        // Not even the valid ids a malformed file holds.
        fs::write(root.join(FILE), "[bandit]\nskips = B101\nskips = B105\n").unwrap();
        assert!(project_skips(root).is_empty(), "a malformed file holding valid ids adds nothing");

        fs::write(root.join(FILE), [b'[', 0xff, b']']).unwrap();
        assert_eq!(read(root), Err("it is not UTF-8".to_string()));

        let big = format!("[bandit]\nskips = B101\n#{}\n", "x".repeat(MAX_BYTES as usize));
        fs::write(root.join(FILE), big).unwrap();
        assert!(read(root).unwrap_err().contains("larger than 64 KiB"));

        // Only the root's file: one in a subdirectory, or in a parent, is not read.
        fs::remove_file(root.join(FILE)).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src").join(FILE), "[bandit]\nskips = B101\n").unwrap();
        assert_eq!(read(root), Ok(Vec::new()), "a nested file");
        assert_eq!(read(&root.join("src").join("pkg")), Ok(Vec::new()), "a parent's file");

        // A directory of that name is no file; a symbolic link is not followed.
        fs::create_dir(root.join(FILE)).unwrap();
        assert_eq!(read(root), Ok(Vec::new()), "a directory");
        fs::remove_dir(root.join(FILE)).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("src").join(FILE), root.join(FILE)).unwrap();
            assert!(read(root).unwrap_err().contains("symbolic link"));
        }
    }
}
