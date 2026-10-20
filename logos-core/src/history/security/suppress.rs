//! Suppressions: accepted risk, recorded in the repository ([FR-SC-11], [S-666]).
//!
//! A committed `.logos/security-ignore.toml` suppresses findings by the
//! fingerprint [S-663] stores. Each entry carries a required `reason` and an
//! optional `expires` date, so accepted risk is reasoned and time-boxed:
//!
//! ```toml
//! [[ignore]]
//! fingerprint = "9f2c…64 lowercase hex characters…"
//! reason = "test fixture, never deployed"
//! expires = 2026-12-31
//! ```
//!
//! An entry can name a `path` glob instead of a fingerprint ([S-770]), so one
//! reviewed entry covers a fixtures directory. It suppresses every finding at
//! or under a path the glob matches, optionally narrowed to one `rule` and one
//! `scanner`:
//!
//! ```toml
//! [[ignore]]
//! path = "tests/fixtures"
//! scanner = "gitleaks"
//! reason = "planted fixture secrets, never deployed"
//! ```
//!
//! The glob is matched against the finding's project-relative location with
//! `*` kept within one path segment and `**` crossing them, and a match on a
//! directory covers everything under it, as a `.gitignore` line does. A glob
//! that would cover the whole project (`*`, `**`, `.`) is rejected, and so is an
//! entry naming both a fingerprint and a path. `rule` is the finding's rule id
//! exactly; `scanner` is its scanner as findings name it (`gitleaks`,
//! `Semgrep OSS`), either case.
//!
//! Four pieces, none of which touches the store or the reader:
//!
//! 1. **parse** ([`IgnoreFile::parse`], [`IgnoreFile::load`]) — a malformed file
//!    is rejected whole, as a [`ConfigError`] (exit 2) that names the line. An
//!    entry without a reason, a fingerprint that is not one of ours, a path glob
//!    that covers the whole project or does not compile, a date that is not a
//!    calendar date and a repeated entry are all rejected: a suppression that
//!    silently did nothing — or silently did everything — would be worse than
//!    none.
//! 2. **serialise** ([`IgnoreFile::to_toml`]) — one canonical spelling, the
//!    fingerprint entries sorted by fingerprint and then the path entries sorted
//!    by path, rule and scanner, so a file in that spelling round-trips byte for
//!    byte and a triage edit shows as a minimal diff. The parser is more
//!    lenient than the writer (any order, any TOML string form, a quoted date, a
//!    path spelled `./dir/`); only the canonical spelling is reproduced verbatim.
//! 3. **label** ([`label`]) — a **pure** function over stored findings, the
//!    parsed file, the project root and an injected `today`: it marks each
//!    finding suppressed or not, and counts each entry's suppressed findings
//!    and flags it expired or orphaned. The caller supplies the date, so the
//!    labelling never reads a clock and a test pins it either side of an
//!    expiry; the root only turns an absolute report path into a project path.
//! 4. **triage** ([`suppress`], [`unsuppress`]; [FR-SC-12], [S-672]) — add or
//!    remove one entry and write the file back through the serialiser, so
//!    adding then removing an entry restores the file byte for byte. A file not
//!    in the canonical spelling is refused rather than rewritten: its comments
//!    and formatting would be lost. Removing the last entry removes the file.
//!
//! An entry suppresses through its `expires` date inclusive: it is *expired*
//! from the next day on, and an expired entry no longer suppresses — the finding
//! returns, labelled so the surface can say why ([FR-SC-11]). A suppressed
//! finding is still returned, with its reason: suppression is visible, never
//! silent ([NFR-CC-04]).
//!
//! [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
//! [FR-SC-12]: ../../../../docs/specs/requirements/FR-SC-12.md
//! [S-672]: ../../../../docs/planning/journal.md#s-672-findings-are-triaged-from-the-security-page
//! [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
//! [S-663]: ../../../../docs/planning/journal.md#s-663-sarif-findings-are-ingested-into-the-evidence-store-anchored-and-redacted
//! [S-666]: ../../../../docs/planning/journal.md#s-666-suppressions-live-in-a-committed-ignore-file-with-a-reason-and-an-expiry
//! [S-770]: ../../../../docs/planning/journal.md#s-770-a-suppression-can-cover-a-path

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use globset::GlobSet;
use serde::{Deserialize, Serialize, Serializer};
use toml::Spanned;

use super::SecurityFinding;
use crate::config::ConfigError;

/// The ignore file's path, relative to the project root.
pub const IGNORE_FILE: &str = ".logos/security-ignore.toml";

/// The length of a stored fingerprint: a blake3 digest in hex.
const FINGERPRINT_LEN: usize = 64;

/// What an entry suppresses: one finding by its fingerprint, or every finding
/// at or under a path glob, optionally narrowed by rule and scanner. Ordered as
/// the canonical file sorts its entries: fingerprints first, then paths.
///
/// A bare string converts to a fingerprint selector, the only kind there was
/// before path entries.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(untagged)]
pub enum Selector {
    /// One finding.
    Fingerprint {
        /// Its fingerprint (64 lowercase hex characters).
        fingerprint: String,
    },
    /// Every finding at or under a path the glob matches.
    Path {
        /// The glob, in its canonical spelling ([`canonical_path`]).
        path: String,
        /// Only findings of this rule id.
        #[serde(skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
        /// Only findings of this scanner, as findings name it, either case.
        #[serde(skip_serializing_if = "Option::is_none")]
        scanner: Option<String>,
    },
}

impl From<&str> for Selector {
    fn from(fingerprint: &str) -> Self {
        Self::Fingerprint { fingerprint: fingerprint.to_string() }
    }
}

impl From<&String> for Selector {
    fn from(fingerprint: &String) -> Self {
        Self::from(fingerprint.as_str())
    }
}

impl From<String> for Selector {
    fn from(fingerprint: String) -> Self {
        Self::Fingerprint { fingerprint }
    }
}

impl Selector {
    /// The selector a surface was sent: exactly one of `fingerprint` and
    /// `path`, with `rule` and `scanner` only beside a path. The path comes
    /// back in its canonical spelling.
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2): both or neither of `fingerprint` and
    /// `path`, a narrowing without a path, or a part the file would reject.
    pub fn from_parts(
        fingerprint: Option<String>,
        path: Option<String>,
        rule: Option<String>,
        scanner: Option<String>,
    ) -> Result<Self> {
        let selector = match (fingerprint, path) {
            (Some(_), Some(_)) => return Err(refused(BOTH_SELECTORS)),
            (None, None) => return Err(refused(NO_SELECTOR)),
            (Some(_), None) if rule.is_some() || scanner.is_some() => return Err(refused(NARROWS_A_PATH)),
            (Some(fingerprint), None) => Self::Fingerprint { fingerprint },
            (None, Some(path)) => Self::Path { path, rule, scanner },
        };
        selector.validated().map_err(refused)
    }

    /// This selector checked and in its canonical spelling, or why the file
    /// would reject it.
    fn validated(self) -> std::result::Result<Self, String> {
        match self {
            Self::Fingerprint { fingerprint } => {
                check_fingerprint(&fingerprint)?;
                Ok(Self::Fingerprint { fingerprint })
            }
            Self::Path { path, rule, scanner } => Ok(Self::Path {
                path: canonical_path(&path)?,
                rule: rule.map(|rule| non_blank("rule", rule)).transpose()?,
                scanner: scanner.map(|scanner| non_blank("scanner", scanner)).transpose()?,
            }),
        }
    }

    /// The fingerprint a fingerprint selector names; `None` for a path.
    pub fn fingerprint(&self) -> Option<&str> {
        match self {
            Self::Fingerprint { fingerprint } => Some(fingerprint),
            Self::Path { .. } => None,
        }
    }

    /// How a message names the selector: `fingerprint <hex>` or
    /// `path "<glob>"` with its narrowing.
    fn describe(&self) -> String {
        match self {
            Self::Fingerprint { fingerprint } => format!("fingerprint {fingerprint}"),
            Self::Path { path, rule, scanner } => {
                let mut text = format!("path {path:?}");
                if let Some(rule) = rule {
                    let _ = write!(text, " rule {rule:?}");
                }
                if let Some(scanner) = scanner {
                    let _ = write!(text, " scanner {scanner:?}");
                }
                text
            }
        }
    }
}

const BOTH_SELECTORS: &str = "the entry names both a `fingerprint` and a `path`: an entry suppresses one finding by its \
                              fingerprint or every finding under a path, never both";
const NO_SELECTOR: &str = "the entry has no `fingerprint` (the 64-character fingerprint of the finding it suppresses) \
                           and no `path` (a glob of the files it covers)";
const NARROWS_A_PATH: &str = "`rule` and `scanner` narrow a `path` entry; a fingerprint already names one finding";

/// A path glob in the one spelling the file is written in: `/`-separated, each
/// segment trimmed, with no empty or `.` segment — so no leading `./` and no
/// trailing `/`: `./tests/fixtures/` and `tests//fixtures /` are both
/// `tests/fixtures`. The spelling is a fixed point: canonicalising it again
/// changes nothing, so a file Logos wrote can always be edited by triage again.
///
/// # Errors
/// Why the file would reject it: the glob covers the whole project — every
/// segment is `*` or `**`, nothing is left, or it matches every name at the
/// project root, as `?*` or `{*}` do — or it escapes the project root or does
/// not compile, as every other path glob in the configuration
/// ([`crate::config::globs`]).
pub fn canonical_path(raw: &str) -> std::result::Result<String, String> {
    let slashed = raw.replace('\\', "/");
    let segments: Vec<&str> = slashed.split('/').map(str::trim).filter(|s| !matches!(*s, "" | ".")).collect();
    // An absolute path stays absolute, for the containment check to refuse.
    let lead = if slashed.trim_start().starts_with('/') { "/" } else { "" };
    let path = format!("{lead}{}", segments.join("/"));
    let whole = || format!("`path` {raw:?} covers the whole project: name the directory or files the entry covers");
    if segments.iter().all(|segment| matches!(*segment, "*" | "**")) {
        return Err(whole());
    }
    let glob = compile_path(&path).map_err(|err| format!("`path` {raw:?} is not a usable glob: {err}"))?;
    // A glob matching every name at the root covers everything under them.
    if WHOLE_PROJECT_PROBES.iter().all(|probe| at_or_under(&glob, probe)) {
        return Err(whole());
    }
    Ok(path)
}

/// Names at the project root no reviewed entry names together — a plain one, a
/// dotfile and one of mixed characters: a glob matching all three matches any
/// name there, and so covers the whole project.
const WHOLE_PROJECT_PROBES: [&str; 3] = ["a", ".z", "Q_9-x.y"];

/// `path` compiled with the documentation globs' semantics: `*` within one
/// segment, `**` across them.
fn compile_path(path: &str) -> std::result::Result<GlobSet, ConfigError> {
    crate::config::globs::compile_anchored(&[path.to_string()])
}

fn check_fingerprint(fingerprint: &str) -> std::result::Result<(), String> {
    if is_fingerprint(fingerprint) {
        Ok(())
    } else {
        Err(format!(
            "{fingerprint:?} is not a finding fingerprint (expected {FINGERPRINT_LEN} lowercase hex characters)"
        ))
    }
}

fn non_blank(key: &str, value: String) -> std::result::Result<String, String> {
    if value.trim().is_empty() {
        Err(format!("`{key}` is empty: leave it out to cover every {key}"))
    } else {
        Ok(value)
    }
}

/// One suppression: what it suppresses, why it is accepted, and until when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IgnoreEntry {
    /// The finding, or the findings, it suppresses.
    #[serde(flatten)]
    pub selector: Selector,
    /// Why the risk is accepted. Never empty.
    pub reason: String,
    /// The last day the entry suppresses; `None` suppresses until removed.
    #[serde(serialize_with = "serialize_date_opt")]
    pub expires: Option<NaiveDate>,
}

impl IgnoreEntry {
    /// Whether the entry has stopped suppressing on `today`: strictly after its
    /// `expires` date. An entry with no expiry never expires.
    pub fn is_expired(&self, today: NaiveDate) -> bool {
        self.expires.is_some_and(|expires| today > expires)
    }
}

/// A parsed `.logos/security-ignore.toml`: unique selectors, sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreFile {
    entries: Vec<IgnoreEntry>,
}

impl IgnoreFile {
    /// The entries in canonical order: fingerprints, then paths.
    pub fn entries(&self) -> &[IgnoreEntry] {
        &self.entries
    }

    /// How many entries the file holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the file holds no entry.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read the ignore file under `root`. An absent file is an empty one: no
    /// suppression is the default, and nothing is created.
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2): the file exists but cannot be read, or is
    /// malformed ([`IgnoreFile::parse`]).
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(IGNORE_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(ConfigError::Io { path, source }.into()),
        };
        Self::parse_at(&text, &path)
    }

    /// Parse the text of an ignore file.
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2) naming the line: TOML that does not parse, a
    /// key or table Logos does not know, an entry with neither a `fingerprint`
    /// nor a `path` or with both, a `rule` or `scanner` beside a fingerprint,
    /// an entry without a `reason` (or with a blank one), a fingerprint that is
    /// not 64 lowercase hex characters, a path glob that covers the whole
    /// project or does not compile, an `expires` that is not a calendar date,
    /// or the same selector given twice.
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_at(text, Path::new(IGNORE_FILE))
    }

    fn parse_at(text: &str, path: &Path) -> Result<Self> {
        let raw: RawFile = toml::from_str(text)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })?;

        let mut first_line: HashMap<Selector, usize> = HashMap::new();
        let mut entries = Vec::with_capacity(raw.ignore.len());
        for spanned in raw.ignore {
            // The entry's span starts at its `[[ignore]]` header.
            let line = line_of(text, spanned.span().start);
            let raw = spanned.into_inner();
            let at = |value: &Spanned<String>| line_of(text, value.span().start);
            // Each part is checked at its own line, so a fault names the line
            // that holds it.
            let check = |value: Spanned<String>, valid: &dyn Fn(String) -> std::result::Result<String, String>| {
                let value_line = at(&value);
                valid(value.into_inner()).map_err(|message| invalid(value_line, message))
            };

            let (selector, selector_line) = match (raw.fingerprint, raw.path) {
                (Some(_), Some(path)) => return Err(invalid(at(&path), BOTH_SELECTORS)),
                (None, None) => return Err(invalid(line, NO_SELECTOR)),
                (Some(fingerprint), None) => {
                    if let Some(narrowing) = raw.rule.as_ref().or(raw.scanner.as_ref()) {
                        return Err(invalid(at(narrowing), NARROWS_A_PATH));
                    }
                    let selector_line = at(&fingerprint);
                    let fingerprint = check(fingerprint, &|fp| check_fingerprint(&fp).map(|()| fp))?;
                    (Selector::Fingerprint { fingerprint }, selector_line)
                }
                (None, Some(path)) => {
                    let selector_line = at(&path);
                    let path = check(path, &|path| canonical_path(&path))?;
                    let rule = raw.rule.map(|rule| check(rule, &|rule| non_blank("rule", rule))).transpose()?;
                    let scanner = raw
                        .scanner
                        .map(|scanner| check(scanner, &|scanner| non_blank("scanner", scanner)))
                        .transpose()?;
                    (Selector::Path { path, rule, scanner }, selector_line)
                }
            };

            let reason = raw.reason.ok_or_else(|| {
                invalid(line, "the entry has no `reason`: a suppression must say why the risk is accepted")
            })?;
            let reason_line = line_of(text, reason.span().start);
            let reason = reason.into_inner();
            if reason.trim().is_empty() {
                return Err(invalid(reason_line, "`reason` is empty: a suppression must say why the risk is accepted"));
            }

            let expires = raw
                .expires
                .map(|value| {
                    let expires_line = line_of(text, value.span().start);
                    parse_expiry(value.into_inner()).map_err(|message| invalid(expires_line, message))
                })
                .transpose()?;

            if let Some(first) = first_line.insert(selector.clone(), selector_line) {
                return Err(invalid(
                    selector_line,
                    format!("duplicate {}: already given at line {first}", selector.describe()),
                ));
            }
            entries.push(IgnoreEntry { selector, reason, expires });
        }
        entries.sort_by(|a, b| a.selector.cmp(&b.selector));
        Ok(Self { entries })
    }

    /// The canonical text of this file: fingerprint entries sorted by
    /// fingerprint, then path entries sorted by path, rule and scanner; one
    /// `[[ignore]]` table each, blank-line separated, the selector's keys
    /// first, a dated expiry written as a native TOML date, a single trailing
    /// newline — and nothing at all for no entry. Parsing a file in this
    /// spelling and serialising it again yields the same bytes.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str("[[ignore]]\n");
            match &entry.selector {
                Selector::Fingerprint { fingerprint } => {
                    let _ = writeln!(out, "fingerprint = {}", basic_string(fingerprint));
                }
                Selector::Path { path, rule, scanner } => {
                    let _ = writeln!(out, "path = {}", basic_string(path));
                    if let Some(rule) = rule {
                        let _ = writeln!(out, "rule = {}", basic_string(rule));
                    }
                    if let Some(scanner) = scanner {
                        let _ = writeln!(out, "scanner = {}", basic_string(scanner));
                    }
                }
            }
            let _ = writeln!(out, "reason = {}", basic_string(&entry.reason));
            if let Some(expires) = entry.expires {
                let _ = writeln!(out, "expires = {expires}");
            }
        }
        out
    }

    /// Add `entry` at its sorted place.
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2), the same faults [`IgnoreFile::parse`]
    /// rejects: a fingerprint that is not one of ours, a path glob that covers
    /// the whole project or does not compile, a blank rule, scanner or reason,
    /// or a selector the file already holds. A path is stored in its canonical
    /// spelling, and the entry is returned as stored.
    pub fn insert(&mut self, mut entry: IgnoreEntry) -> Result<&IgnoreEntry> {
        entry.selector = entry.selector.validated().map_err(refused)?;
        if entry.reason.trim().is_empty() {
            return Err(refused("`reason` is empty: a suppression must say why the risk is accepted"));
        }
        match self.entries.binary_search_by(|e| e.selector.cmp(&entry.selector)) {
            Ok(_) => Err(refused(format!(
                "{} is already suppressed: remove its entry first to change it",
                entry.selector.describe()
            ))),
            Err(at) => {
                self.entries.insert(at, entry);
                Ok(&self.entries[at])
            }
        }
    }

    /// Remove the entry for `selector` and return it; `None` when the file
    /// holds none. A path matches in any spelling [`canonical_path`] accepts.
    pub fn remove(&mut self, selector: &Selector) -> Option<IgnoreEntry> {
        let selector = selector.clone().validated().ok()?;
        let at = self.entries.iter().position(|e| e.selector == selector)?;
        Some(self.entries.remove(at))
    }
}

/// The ignore file as TOML deserialises it, before validation. Every field is an
/// `Option` so a missing one is reported by line, not as a bare serde message.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    ignore: Vec<Spanned<RawEntry>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    fingerprint: Option<Spanned<String>>,
    path: Option<Spanned<String>>,
    rule: Option<Spanned<String>>,
    scanner: Option<Spanned<String>>,
    reason: Option<Spanned<String>>,
    expires: Option<Spanned<toml::Value>>,
}

/// A fault in the ignore file at `line`, as the exit-2 [`ConfigError`] every
/// configuration fault is.
fn invalid(line: usize, message: impl Into<String>) -> anyhow::Error {
    ConfigError::InvalidValue { key: format!("{IGNORE_FILE} line {line}"), message: message.into() }.into()
}

/// A refused triage write, as the exit-2 [`ConfigError`] a malformed file is.
fn refused(message: impl Into<String>) -> anyhow::Error {
    ConfigError::InvalidValue { key: IGNORE_FILE.to_string(), message: message.into() }.into()
}

/// The 1-based line holding byte `offset` of `text`.
fn line_of(text: &str, offset: usize) -> usize {
    1 + text.as_bytes()[..offset.min(text.len())].iter().filter(|b| **b == b'\n').count()
}

fn is_fingerprint(text: &str) -> bool {
    text.len() == FINGERPRINT_LEN && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// An `expires` value as a date: a native TOML date (`2026-12-31`), or the same
/// written as a string. A date-time is refused — an expiry is a day.
fn parse_expiry(value: toml::Value) -> std::result::Result<NaiveDate, String> {
    const EXPECTED: &str = "expected a calendar date such as 2026-12-31";
    match value {
        toml::Value::Datetime(dt) => match (dt.date, dt.time, dt.offset) {
            (Some(date), None, None) => NaiveDate::from_ymd_opt(i32::from(date.year), u32::from(date.month), u32::from(date.day))
                .ok_or_else(|| format!("`expires` is not a real date ({EXPECTED})")),
            _ => Err(format!("`expires` carries a time ({EXPECTED})")),
        },
        toml::Value::String(text) => parse_ymd(&text)
            .ok_or_else(|| format!("`expires` is {text:?}, which is not a calendar date ({EXPECTED})")),
        other => Err(format!("`expires` is a {}, not a date ({EXPECTED})", other.type_str())),
    }
}

/// An expiry a surface received as text (`2026-12-31`), read with the same
/// strict `YYYY-MM-DD` rule the file's quoted dates follow.
///
/// # Errors
/// A [`ConfigError`] (exit 2) naming the text when it is not a calendar date.
pub fn parse_expires(text: &str) -> Result<NaiveDate> {
    parse_ymd(text.trim()).ok_or_else(|| {
        refused(format!("`expires` is {text:?}, which is not a calendar date (expected a date such as 2026-12-31)"))
    })
}

/// `YYYY-MM-DD`, strictly: four, two and two digits.
fn parse_ymd(text: &str) -> Option<NaiveDate> {
    let mut parts = text.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let digits = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_digit());
    if !(digits(year, 4) && digits(month, 2) && digits(day, 2)) {
        return None;
    }
    NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)
}

/// `text` as a TOML basic string: quoted, with `"` and `\` and every control
/// character escaped, so any reason survives a round trip.
pub(super) fn basic_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn serialize_date_opt<S: Serializer>(date: &Option<NaiveDate>, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    match date {
        Some(date) => serializer.serialize_some(&date.to_string()),
        None => serializer.serialize_none(),
    }
}

/// What one triage write left on disk ([`suppress`], [`unsuppress`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IgnoreWrite {
    /// The file written, relative to the project root: [`IGNORE_FILE`].
    pub file: &'static str,
    /// The entry added or removed.
    pub entry: IgnoreEntry,
    /// How many entries the file holds now.
    pub entries: usize,
}

/// Serialises the triage writes of this process, so two concurrent ones
/// cannot each read the file and the second publish drop the first's entry.
static TRIAGE_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Add one suppression to the ignore file under `root` ([FR-SC-12]): `reason`
/// (trimmed) is required, and `expires` must not be before `today` — an entry
/// that is already expired would suppress nothing. The file is created if it is
/// absent.
///
/// The write goes through the canonical serialiser ([`IgnoreFile::to_toml`]),
/// so the edit is the one entry's lines, and [`unsuppress`] of the same
/// selector restores the previous bytes exactly. A path entry is written in its
/// canonical spelling ([`canonical_path`]).
///
/// # Errors
/// A [`ConfigError`] (exit 2) and no write: a refused entry
/// ([`IgnoreFile::insert`]), an expiry in the past, or a file that is malformed
/// or not in the canonical spelling (a rewrite would drop its comments or
/// formatting). A `ConfigError::Write` when the atomic write fails.
///
/// [FR-SC-12]: ../../../../docs/specs/requirements/FR-SC-12.md
pub fn suppress(
    root: &Path,
    selector: impl Into<Selector>,
    reason: &str,
    expires: Option<NaiveDate>,
    today: NaiveDate,
) -> Result<IgnoreWrite> {
    if let Some(expires) = expires.filter(|expires| *expires < today) {
        return Err(refused(format!("`expires` {expires} is already past: the entry would suppress nothing")));
    }
    let entry = IgnoreEntry { selector: selector.into(), reason: reason.trim().to_string(), expires };
    rewrite(root, |file| file.insert(entry).cloned())
}

/// Remove the suppression for `selector` from the ignore file under `root`
/// ([FR-SC-12]). Removing the last entry removes the file, so a first
/// suppression undone leaves the tree as it was.
///
/// # Errors
/// A [`ConfigError`] (exit 2) and no write: no entry carries `selector`, or
/// the file is malformed or not in the canonical spelling. A
/// `ConfigError::Write` when the write fails.
///
/// [FR-SC-12]: ../../../../docs/specs/requirements/FR-SC-12.md
pub fn unsuppress(root: &Path, selector: impl Into<Selector>) -> Result<IgnoreWrite> {
    let selector = selector.into();
    rewrite(root, |file| {
        file.remove(&selector)
            .ok_or_else(|| refused(format!("no entry suppresses {}", selector.describe())))
    })
}

/// Read, edit and write back the ignore file under `root`: `edit` changes one
/// entry and returns it. Nothing is written when the file is not in the
/// canonical spelling or `edit` refuses.
fn rewrite(root: &Path, edit: impl FnOnce(&mut IgnoreFile) -> Result<IgnoreEntry>) -> Result<IgnoreWrite> {
    let _serialised = TRIAGE_WRITE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = root.join(IGNORE_FILE);
    let mut file = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let file = IgnoreFile::parse_at(&text, &path)?;
            if file.to_toml() != text {
                return Err(refused(
                    "the file is not in the canonical spelling Logos writes (one `[[ignore]]` table per entry, \
                     fingerprint entries sorted by fingerprint then path entries sorted by path, rule and scanner, \
                     no comments), and rewriting it would drop its comments or formatting: edit it by hand instead",
                ));
            }
            file
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => IgnoreFile::default(),
        Err(source) => return Err(ConfigError::Io { path, source }.into()),
    };
    let entry = edit(&mut file)?;
    let written = if file.is_empty() {
        std::fs::remove_file(&path)
    } else {
        path.parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| crate::config::atomic::publish(&path, file.to_toml().as_bytes(), None))
    };
    written.map_err(|source| ConfigError::Write { path, source })?;
    Ok(IgnoreWrite { file: IGNORE_FILE, entry, entries: file.len() })
}

/// What the ignore file says about one finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Suppression {
    /// An entry suppresses it: still listed, with the reason, never silent.
    Suppressed {
        /// The entry's reason.
        reason: String,
        /// The last day the entry suppresses; `None` when it never expires.
        #[serde(serialize_with = "serialize_date_opt")]
        expires: Option<NaiveDate>,
        /// The entry's selector: what removing the suppression removes.
        by: Selector,
    },
    /// An entry names it but expired: the finding is back, and says why.
    Expired {
        /// The entry's reason.
        reason: String,
        /// The day the entry last suppressed.
        #[serde(serialize_with = "serialize_date_opt")]
        expires: Option<NaiveDate>,
        /// The entry's selector.
        by: Selector,
    },
}

/// One finding with what the ignore file says about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LabelledFinding {
    /// The stored finding, unchanged.
    pub finding: SecurityFinding,
    /// `None` when no entry names it.
    pub suppression: Option<Suppression>,
}

impl LabelledFinding {
    /// Whether an unexpired entry suppresses it — what a gate excludes.
    pub fn is_suppressed(&self) -> bool {
        matches!(self.suppression, Some(Suppression::Suppressed { .. }))
    }

    /// Whether it counts ([`counts`]).
    pub fn is_counted(&self) -> bool {
        counts(&self.finding, self.suppression.as_ref())
    }
}

/// **The one definition of a counted finding**, used by every security
/// surface — the Security page's counts, Files & Risk, and the workspace
/// roll-up ([S-760], [FR-SC-08]): a finding counts unless an unexpired entry
/// suppresses it or its own file changed since the scan ([`FRESHNESS_STALE`]).
/// Freshness is the finding's, never its report's: a `--path` scan's report is
/// not fresh at HEAD, yet a finding in a file nothing has touched is current.
/// A finding bound to no file (`n/a`) has none to compare and counts.
///
/// [FR-SC-08]: ../../../../docs/specs/requirements/FR-SC-08.md
/// [S-760]: ../../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
pub fn counts(finding: &SecurityFinding, suppression: Option<&Suppression>) -> bool {
    finding.freshness != crate::history::FRESHNESS_STALE && !matches!(suppression, Some(Suppression::Suppressed { .. }))
}

/// One ignore entry with what the findings say about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryStatus {
    /// The entry.
    pub entry: IgnoreEntry,
    /// It is past its `expires` date and no longer suppresses.
    pub expired: bool,
    /// It names no finding: the finding was fixed, the glob matches nothing,
    /// or the file holds a typo. Independent of `expired`.
    pub orphaned: bool,
    /// How many findings it suppresses: `0` when expired or orphaned. A
    /// finding two entries name counts once, for the one that labels it.
    pub suppressed: usize,
    /// For an orphaned entry written under a Gitleaks finding's identity
    /// before history.db migration 7: the fingerprint its finding has now,
    /// which the entry should name. The file is never rewritten to it; `None`
    /// otherwise.
    pub should_name: Option<String>,
}

/// The labelled findings and the audited entries, as [`label`] returns them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Labelled {
    /// One row per input finding, in input order.
    pub findings: Vec<LabelledFinding>,
    /// One row per ignore entry, in the file's canonical order.
    pub entries: Vec<EntryStatus>,
}

/// Ignore entries compiled for matching findings of the project at one root:
/// each path glob compiled once, and the root an absolute report path is made
/// project-relative against.
pub struct EntryMatcher<'e> {
    entries: Vec<(&'e IgnoreEntry, Option<GlobSet>)>,
    root: String,
}

impl<'e> EntryMatcher<'e> {
    /// The matcher for `entries`, in the order they are given — which is the
    /// order [`naming`](Self::naming) reports them in. A glob the file would
    /// have rejected matches nothing.
    pub fn new(entries: impl IntoIterator<Item = &'e IgnoreEntry>, root: &Path) -> Self {
        let entries = entries
            .into_iter()
            .map(|entry| {
                let glob = match &entry.selector {
                    Selector::Path { path, .. } => compile_path(path).ok(),
                    Selector::Fingerprint { .. } => None,
                };
                (entry, glob)
            })
            .collect();
        Self { entries, root: root_key(root) }
    }

    /// The entries that name `finding`, as `(position, entry)` in the order
    /// the matcher was given them. A fingerprint entry names the finding with
    /// its fingerprint; a path entry names a finding whose project-relative
    /// location, or a directory holding it, the glob matches, and whose rule
    /// and scanner are the ones it narrows to. A finding with no location, or
    /// one reported outside the project, is named by no path entry.
    pub fn naming<'m>(&'m self, finding: &'m SecurityFinding) -> impl Iterator<Item = (usize, &'e IgnoreEntry)> + 'm {
        let location = self.project_path(finding);
        self.entries.iter().enumerate().filter_map(move |(at, (entry, glob))| {
            let names = match &entry.selector {
                Selector::Fingerprint { fingerprint } => *fingerprint == finding.fingerprint,
                Selector::Path { rule, scanner, .. } => {
                    glob.as_ref().zip(location.as_deref()).is_some_and(|(glob, path)| at_or_under(glob, path))
                        && rule.as_ref().is_none_or(|rule| *rule == finding.rule_id)
                        && scanner.as_ref().is_none_or(|scanner| scanner.eq_ignore_ascii_case(&finding.scanner_name))
                }
            };
            names.then_some((at, *entry))
        })
    }

    /// `finding`'s location relative to the project root: its bound path, or
    /// the path its report named with the root taken off an absolute one.
    /// `None` without a location or for an absolute path outside the root.
    fn project_path(&self, finding: &SecurityFinding) -> Option<String> {
        under_root(&self.root, finding.location()?)
    }
}

/// `root` as [`EntryMatcher`] compares locations against it: `/`-separated,
/// with no trailing `/`.
fn root_key(root: &Path) -> String {
    root.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string()
}

/// `location` relative to the project at `root_key`: an absolute path under it
/// with the root taken off, a relative one as it is, `None` for an absolute
/// path outside it.
fn under_root(root_key: &str, location: String) -> Option<String> {
    let under_root = location.strip_prefix(root_key).and_then(|rest| rest.strip_prefix('/'));
    if let Some(rest) = under_root.filter(|_| !root_key.is_empty()) {
        return Some(rest.to_string());
    }
    (!location.starts_with('/') && !Path::new(&location).is_absolute()).then_some(location)
}

/// A finding's location — its bound path, else the path its report named —
/// relative to the project at `root`, exactly as a path suppression matches it
/// ([`EntryMatcher`]); `None` for a path outside the project. The checked-in
/// security baseline records this spelling ([FR-SC-25]), so it never carries
/// the checkout's own directory.
///
/// [FR-SC-25]: ../../../../docs/specs/requirements/FR-SC-25.md
pub fn project_location(root: &Path, location: &str) -> Option<String> {
    under_root(&root_key(root), super::normalise_report_path(location))
}

/// Whether `glob` matches `path` or a directory holding it.
fn at_or_under(glob: &GlobSet, path: &str) -> bool {
    std::iter::successors(Some(path), |p| p.rfind('/').map(|end| &p[..end])).any(|p| glob.is_match(p))
}

/// Label `findings` against `ignore` as of `today` ([FR-SC-11]), for the
/// project at `root`.
///
/// Pure: no clock, no store, no file — `root` only makes an absolute report
/// path project-relative for the path globs. A finding is suppressed by the
/// first unexpired entry that names it ([`EntryMatcher::naming`]), in the
/// file's order: a fingerprint entry before a path entry. When every entry
/// naming it has expired, the finding is unsuppressed and labelled
/// [`Suppression::Expired`] by the first. Each entry counts the findings it
/// suppresses. An entry is orphaned when it names no finding in `findings`, so
/// pass every finding that could match — an entry for a scanner that has never
/// run reads as orphaned until it does. An orphaned entry naming a key a
/// finding had before it was keyed by its secret names that finding's
/// fingerprint as the one it should name: it is still orphaned, and suppresses
/// nothing until edited.
///
/// [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
pub fn label(findings: &[SecurityFinding], ignore: &IgnoreFile, root: &Path, today: NaiveDate) -> Labelled {
    let matcher = EntryMatcher::new(&ignore.entries, root);
    let mut named = vec![false; ignore.entries.len()];
    let mut suppressed = vec![0_usize; ignore.entries.len()];

    let labelled = findings
        .iter()
        .map(|finding| {
            let mut first = None;
            let mut live = None;
            for (at, entry) in matcher.naming(finding) {
                named[at] = true;
                first.get_or_insert((at, entry));
                if live.is_none() && !entry.is_expired(today) {
                    live = Some((at, entry));
                }
            }
            let suppression = live.or(first).map(|(at, entry)| {
                let (reason, expires, by) = (entry.reason.clone(), entry.expires, entry.selector.clone());
                if entry.is_expired(today) {
                    Suppression::Expired { reason, expires, by }
                } else {
                    suppressed[at] += 1;
                    Suppression::Suppressed { reason, expires, by }
                }
            });
            LabelledFinding { finding: finding.clone(), suppression }
        })
        .collect();

    // The lowest current fingerprint per old key, so the answer is the same in
    // any finding order.
    let mut renamed: BTreeMap<&str, &str> = BTreeMap::new();
    for finding in findings {
        for old in &finding.legacy_fingerprints {
            let now = renamed.entry(old.as_str()).or_insert(finding.fingerprint.as_str());
            *now = (*now).min(finding.fingerprint.as_str());
        }
    }
    let entries = ignore
        .entries
        .iter()
        .enumerate()
        .map(|(at, entry)| {
            let orphaned = !named[at];
            EntryStatus {
                entry: entry.clone(),
                expired: entry.is_expired(today),
                orphaned,
                suppressed: suppressed[at],
                should_name: entry
                    .selector
                    .fingerprint()
                    .filter(|_| orphaned)
                    .and_then(|fingerprint| renamed.get(fingerprint))
                    .map(|now| now.to_string()),
            }
        })
        .collect();

    Labelled { findings: labelled, entries }
}
