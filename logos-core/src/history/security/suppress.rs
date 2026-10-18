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
//! Four pieces, none of which touches the store or the reader:
//!
//! 1. **parse** ([`IgnoreFile::parse`], [`IgnoreFile::load`]) — a malformed file
//!    is rejected whole, as a [`ConfigError`] (exit 2) that names the line. An
//!    entry without a reason, a fingerprint that is not one of ours, a date that
//!    is not a calendar date and a repeated fingerprint are all rejected: a
//!    suppression that silently did nothing would be worse than none.
//! 2. **serialise** ([`IgnoreFile::to_toml`]) — one canonical spelling, entries
//!    sorted by fingerprint, so a file in that spelling round-trips byte for
//!    byte and a triage edit shows as a minimal diff. The parser is more
//!    lenient than the writer (any order, any TOML string form, a quoted date);
//!    only the canonical spelling is reproduced verbatim.
//! 3. **label** ([`label`]) — a **pure** function over stored findings, the
//!    parsed file and an injected `today`: it marks each finding suppressed or
//!    not, and each entry expired or orphaned. The caller supplies the date, so
//!    the labelling never reads a clock and a test pins it either side of an
//!    expiry.
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize, Serializer};
use toml::Spanned;

use super::SecurityFinding;
use crate::config::ConfigError;

/// The ignore file's path, relative to the project root.
pub const IGNORE_FILE: &str = ".logos/security-ignore.toml";

/// The length of a stored fingerprint: a blake3 digest in hex.
const FINGERPRINT_LEN: usize = 64;

/// One suppression: a finding's fingerprint, why it is accepted, and until when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IgnoreEntry {
    /// The fingerprint of the finding it suppresses (64 lowercase hex characters).
    pub fingerprint: String,
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

/// A parsed `.logos/security-ignore.toml`: unique fingerprints, sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreFile {
    entries: Vec<IgnoreEntry>,
}

impl IgnoreFile {
    /// The entries, sorted by fingerprint.
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
    /// key or table Logos does not know, an entry without a `fingerprint` or a
    /// `reason` (or with a blank one), a fingerprint that is not 64 lowercase
    /// hex characters, an `expires` that is not a calendar date, or a
    /// fingerprint given twice.
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_at(text, Path::new(IGNORE_FILE))
    }

    fn parse_at(text: &str, path: &Path) -> Result<Self> {
        let raw: RawFile = toml::from_str(text)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })?;

        let mut first_line: HashMap<String, usize> = HashMap::new();
        let mut entries = Vec::with_capacity(raw.ignore.len());
        for spanned in raw.ignore {
            // The entry's span starts at its `[[ignore]]` header.
            let line = line_of(text, spanned.span().start);
            let raw = spanned.into_inner();

            let fingerprint = raw.fingerprint.ok_or_else(|| {
                invalid(line, "the entry has no `fingerprint` (the 64-character fingerprint of the finding it suppresses)")
            })?;
            let fingerprint_line = line_of(text, fingerprint.span().start);
            let fingerprint = fingerprint.into_inner();
            if !is_fingerprint(&fingerprint) {
                return Err(invalid(
                    fingerprint_line,
                    format!("{fingerprint:?} is not a finding fingerprint (expected {FINGERPRINT_LEN} lowercase hex characters)"),
                ));
            }

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

            if let Some(first) = first_line.insert(fingerprint.clone(), fingerprint_line) {
                return Err(invalid(
                    fingerprint_line,
                    format!("duplicate fingerprint {fingerprint}: already given at line {first}"),
                ));
            }
            entries.push(IgnoreEntry { fingerprint, reason, expires });
        }
        entries.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
        Ok(Self { entries })
    }

    /// The canonical text of this file: entries sorted by fingerprint, one
    /// `[[ignore]]` table each, blank-line separated, a dated expiry written as
    /// a native TOML date, a single trailing newline — and nothing at all for no
    /// entry. Parsing a file in this spelling and serialising it again yields the
    /// same bytes.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str("[[ignore]]\n");
            let _ = writeln!(out, "fingerprint = {}", basic_string(&entry.fingerprint));
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
    /// rejects: a fingerprint that is not one of ours, a blank reason, or a
    /// fingerprint the file already holds.
    pub fn insert(&mut self, entry: IgnoreEntry) -> Result<()> {
        if !is_fingerprint(&entry.fingerprint) {
            return Err(refused(format!(
                "{:?} is not a finding fingerprint (expected {FINGERPRINT_LEN} lowercase hex characters)",
                entry.fingerprint
            )));
        }
        if entry.reason.trim().is_empty() {
            return Err(refused("`reason` is empty: a suppression must say why the risk is accepted"));
        }
        match self.entries.binary_search_by(|e| e.fingerprint.cmp(&entry.fingerprint)) {
            Ok(_) => Err(refused(format!(
                "{} is already suppressed: remove its entry first to change it",
                entry.fingerprint
            ))),
            Err(at) => {
                self.entries.insert(at, entry);
                Ok(())
            }
        }
    }

    /// Remove the entry for `fingerprint` and return it; `None` when the file
    /// holds none.
    pub fn remove(&mut self, fingerprint: &str) -> Option<IgnoreEntry> {
        let at = self.entries.iter().position(|e| e.fingerprint == fingerprint)?;
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
/// fingerprint restores the previous bytes exactly.
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
    fingerprint: &str,
    reason: &str,
    expires: Option<NaiveDate>,
    today: NaiveDate,
) -> Result<IgnoreWrite> {
    if let Some(expires) = expires.filter(|expires| *expires < today) {
        return Err(refused(format!("`expires` {expires} is already past: the entry would suppress nothing")));
    }
    let entry = IgnoreEntry { fingerprint: fingerprint.to_string(), reason: reason.trim().to_string(), expires };
    rewrite(root, |file| {
        file.insert(entry.clone())?;
        Ok(entry)
    })
}

/// Remove the suppression for `fingerprint` from the ignore file under `root`
/// ([FR-SC-12]). Removing the last entry removes the file, so a first
/// suppression undone leaves the tree as it was.
///
/// # Errors
/// A [`ConfigError`] (exit 2) and no write: no entry carries `fingerprint`, or
/// the file is malformed or not in the canonical spelling. A
/// `ConfigError::Write` when the write fails.
///
/// [FR-SC-12]: ../../../../docs/specs/requirements/FR-SC-12.md
pub fn unsuppress(root: &Path, fingerprint: &str) -> Result<IgnoreWrite> {
    rewrite(root, |file| {
        file.remove(fingerprint)
            .ok_or_else(|| refused(format!("no entry suppresses {fingerprint:?}")))
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
                     sorted by fingerprint, no comments), and rewriting it would drop its comments or formatting: \
                     edit it by hand instead",
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
    },
    /// An entry names it but expired: the finding is back, and says why.
    Expired {
        /// The entry's reason.
        reason: String,
        /// The day the entry last suppressed.
        #[serde(serialize_with = "serialize_date_opt")]
        expires: Option<NaiveDate>,
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
}

/// One ignore entry with what the findings say about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryStatus {
    /// The entry.
    pub entry: IgnoreEntry,
    /// It is past its `expires` date and no longer suppresses.
    pub expired: bool,
    /// Its fingerprint matches no finding: the finding was fixed, or the file
    /// holds a typo. Independent of `expired`.
    pub orphaned: bool,
}

/// The labelled findings and the audited entries, as [`label`] returns them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Labelled {
    /// One row per input finding, in input order.
    pub findings: Vec<LabelledFinding>,
    /// One row per ignore entry, sorted by fingerprint.
    pub entries: Vec<EntryStatus>,
}

/// Label `findings` against `ignore` as of `today` ([FR-SC-11]).
///
/// Pure: no clock, no store, no file. A finding is suppressed when an entry
/// carries its fingerprint and has not expired; an expired entry leaves the
/// finding unsuppressed and labelled [`Suppression::Expired`]. An entry is
/// orphaned when no finding in `findings` carries its fingerprint, so pass every
/// finding that could match — an entry for a scanner that has never run reads as
/// orphaned until it does.
///
/// [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
pub fn label(findings: &[SecurityFinding], ignore: &IgnoreFile, today: NaiveDate) -> Labelled {
    let by_fingerprint: BTreeMap<&str, &IgnoreEntry> =
        ignore.entries.iter().map(|entry| (entry.fingerprint.as_str(), entry)).collect();

    let labelled = findings
        .iter()
        .map(|finding| {
            let suppression = by_fingerprint.get(finding.fingerprint.as_str()).map(|entry| {
                let (reason, expires) = (entry.reason.clone(), entry.expires);
                if entry.is_expired(today) {
                    Suppression::Expired { reason, expires }
                } else {
                    Suppression::Suppressed { reason, expires }
                }
            });
            LabelledFinding { finding: finding.clone(), suppression }
        })
        .collect();

    let seen: HashSet<&str> = findings.iter().map(|f| f.fingerprint.as_str()).collect();
    let entries = ignore
        .entries
        .iter()
        .map(|entry| EntryStatus {
            entry: entry.clone(),
            expired: entry.is_expired(today),
            orphaned: !seen.contains(entry.fingerprint.as_str()),
        })
        .collect();

    Labelled { findings: labelled, entries }
}
