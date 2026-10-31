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
//! An entry can also name a `rule` and its `scanner` and nothing else
//! ([S-814]): a rule suppressed project-wide, in every file, for one scanner.
//! The pair is the selector, so an entry with only one of them is rejected,
//! naming the key it lacks:
//!
//! ```toml
//! [[ignore]]
//! rule = "CKV_K8S_21"
//! scanner = "checkov"
//! reason = "the namespace is set by the deploy tool"
//! ```
//!
//! An entry can record **evidence** ([S-818], [FR-SC-32]): the symbols its reason
//! rests on, each with the hash of its exact source span when the verdict was made
//! ([`super::evidence`]):
//!
//! ```toml
//! [[ignore]]
//! fingerprint = "9f2c…64 lowercase hex characters…"
//! reason = "the cookie is Secure: DefaultCookieConfig sets it"
//! evidence = [
//!     { symbol = "internal/config/`cookie.go`/DefaultCookieConfig().", hash = "…64 hex…" },
//! ]
//! ```
//!
//! Five pieces:
//!
//! 1. **parse** ([`IgnoreFile::parse`], [`IgnoreFile::load`]) — a malformed file
//!    is rejected whole, as a [`ConfigError`] (exit 2) that names the line. An
//!    entry without a reason, a fingerprint that is not one of ours, a path glob
//!    that covers the whole project or does not compile, a date that is not a
//!    calendar date, an evidence item that is not a symbol with a span hash or
//!    names a symbol twice, and a repeated entry are all rejected: a suppression
//!    that silently did nothing — or silently did everything — would be worse
//!    than none.
//! 2. **serialise** ([`IgnoreFile::to_toml`]) — one canonical spelling, the
//!    fingerprint entries sorted by fingerprint, then the path entries sorted
//!    by path, rule and scanner, then the rule-wide entries sorted by rule and
//!    scanner, each entry's `evidence` after its `expires`, one symbol per line
//!    sorted by symbol, so a file in that spelling round-trips byte for byte and
//!    a triage edit shows as a minimal diff. The parser is more
//!    lenient than the writer (any order, any TOML string form, a quoted date, a
//!    path spelled `./dir/`); only the canonical spelling is reproduced verbatim.
//! 3. **label** ([`label`], [`label_with`]) — over stored findings, the parsed
//!    file, the project root and an injected `today`: it marks each finding
//!    suppressed, out of date or not, and counts each entry's suppressed findings
//!    and flags it expired, out of date or orphaned. The caller supplies the
//!    date, so the labelling never reads a clock and a test pins it either side
//!    of an expiry; the root only turns an absolute report path into a project
//!    path. Evidence is read through an [`EvidenceSource`]: [`label_with`] takes
//!    it, [`label`] reads the graph and the working tree at the root — only for
//!    an entry that records evidence.
//! 4. **triage** ([`suppress`], [`suppress_with_evidence`], [`unsuppress`];
//!    [FR-SC-12], [S-672]) — add or remove one entry and write the file back
//!    through the serialiser, so adding then removing an entry restores the file
//!    byte for byte. A file not in the canonical spelling is refused rather than
//!    rewritten: its comments and formatting would be lost. Removing the last
//!    entry removes the file.
//! 5. **re-confirm** ([`entry_for`], [`reconfirm`]) — re-record an unexpired
//!    entry's evidence hashes, keeping its reason and expiry, through the same
//!    write.
//!
//! An entry suppresses through its `expires` date inclusive: it is *expired*
//! from the next day on, and an expired entry no longer suppresses — the finding
//! returns, labelled so the surface can say why ([FR-SC-11]). An unexpired entry
//! whose evidence changed, vanished or could not be checked no longer suppresses
//! either: its finding returns labelled `out_of_date` ([FR-SC-32]). A suppressed
//! finding is still returned, with its reason: suppression is visible, never
//! silent ([NFR-CC-04]).
//!
//! [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
//! [FR-SC-12]: ../../../../docs/specs/requirements/FR-SC-12.md
//! [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
//! [S-818]: ../../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
//! [S-672]: ../../../../docs/planning/journal.md#s-672-findings-are-triaged-from-the-security-page
//! [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
//! [S-663]: ../../../../docs/planning/journal.md#s-663-sarif-findings-are-ingested-into-the-evidence-store-anchored-and-redacted
//! [S-666]: ../../../../docs/planning/journal.md#s-666-suppressions-live-in-a-committed-ignore-file-with-a-reason-and-an-expiry
//! [S-770]: ../../../../docs/planning/journal.md#s-770-a-suppression-can-cover-a-path
//! [S-814]: ../../../../docs/planning/journal.md#s-814-a-rule-can-be-suppressed-project-wide-with-a-reason

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use globset::GlobSet;
use serde::{Deserialize, Serialize, Serializer};
use toml::Spanned;

use super::evidence::{canonical, out_of_date, Evidence, EvidenceChange, EvidenceSource, EvidenceSymbol, GraphEvidence};
use super::{stored_finding, SecurityFinding};
use crate::config::ConfigError;

/// The ignore file's path, relative to the project root.
pub const IGNORE_FILE: &str = ".logos/security-ignore.toml";

/// The length of a stored fingerprint: a blake3 digest in hex.
const FINGERPRINT_LEN: usize = 64;

/// What an entry suppresses: one finding by its fingerprint, every finding at
/// or under a path glob (optionally narrowed by rule and scanner), or every
/// finding of one rule of one scanner in any file. Ordered as the canonical
/// file sorts its entries: fingerprints first, then paths, then rule-wide.
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
    /// Every finding of one rule of one scanner, in every file ([S-814]).
    ///
    /// [S-814]: ../../../../docs/planning/journal.md#s-814-a-rule-can-be-suppressed-project-wide-with-a-reason
    Rule {
        /// The rule id, exactly.
        rule: String,
        /// The scanner as findings name it, either case.
        scanner: String,
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
    /// `path`, with `rule` and `scanner` only beside a path; or, with neither,
    /// both `rule` and `scanner`. The path comes back in its canonical
    /// spelling.
    ///
    /// # Errors
    /// A [`ConfigError`] (exit 2): both or neither of `fingerprint` and
    /// `path` without a rule-wide pair, a `rule` without its `scanner` or the
    /// reverse (naming the missing key), a narrowing beside a fingerprint, or a
    /// part the file would reject.
    pub fn from_parts(
        fingerprint: Option<String>,
        path: Option<String>,
        rule: Option<String>,
        scanner: Option<String>,
    ) -> Result<Self> {
        let selector = match (fingerprint, path) {
            (Some(_), Some(_)) => return Err(refused(BOTH_SELECTORS)),
            (None, None) => match rule_wide(rule, scanner) {
                Ok(Some((rule, scanner))) => Self::Rule { rule, scanner },
                Ok(None) => return Err(refused(NO_SELECTOR)),
                Err(missing) => return Err(refused(missing)),
            },
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
                rule: rule.map(|rule| non_blank("rule", rule, true)).transpose()?,
                scanner: scanner.map(|scanner| non_blank("scanner", scanner, true)).transpose()?,
            }),
            Self::Rule { rule, scanner } => {
                Ok(Self::Rule { rule: non_blank("rule", rule, false)?, scanner: non_blank("scanner", scanner, false)? })
            }
        }
    }

    /// The fingerprint a fingerprint selector names; `None` for the others.
    pub fn fingerprint(&self) -> Option<&str> {
        match self {
            Self::Fingerprint { fingerprint } => Some(fingerprint),
            Self::Path { .. } | Self::Rule { .. } => None,
        }
    }

    /// How a message names the selector: `fingerprint <hex>`,
    /// `path "<glob>"` with its narrowing, or `rule "<id>" scanner "<name>"`.
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
            Self::Rule { rule, scanner } => format!("rule {rule:?} scanner {scanner:?}"),
        }
    }
}

/// The `rule` and `scanner` of a selector with neither a fingerprint nor a
/// path: both make a rule-wide selector, neither makes none, and one without
/// the other is the message naming the key that is missing.
fn rule_wide<T>(rule: Option<T>, scanner: Option<T>) -> std::result::Result<Option<(T, T)>, &'static str> {
    match (rule, scanner) {
        (Some(rule), Some(scanner)) => Ok(Some((rule, scanner))),
        (None, None) => Ok(None),
        (Some(_), None) => Err(MISSING_SCANNER),
        (None, Some(_)) => Err(MISSING_RULE),
    }
}

const MISSING_SCANNER: &str = "the entry has a `rule` but no `scanner`: a rule-wide entry names the rule and the scanner \
                               that reports it, or add a `path` to narrow it to files";
const MISSING_RULE: &str = "the entry has a `scanner` but no `rule`: a rule-wide entry names the rule and the scanner \
                            that reports it, or add a `path` to narrow it to files";
const BOTH_SELECTORS: &str = "the entry names both a `fingerprint` and a `path`: an entry suppresses one finding by its \
                              fingerprint or every finding under a path, never both";
const NO_SELECTOR: &str = "the entry has no `fingerprint` (the 64-character fingerprint of the finding it suppresses), \
                           no `path` (a glob of the files it covers) and no `rule` with its `scanner` (a rule \
                           suppressed in every file)";
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

/// `value` unless it is blank. `optional` says whether the key may be left out
/// (a path entry's narrowing) or is what the entry names (a rule-wide entry's
/// rule and scanner), which is what the message advises.
fn non_blank(key: &str, value: String, optional: bool) -> std::result::Result<String, String> {
    if !value.trim().is_empty() {
        return Ok(value);
    }
    Err(if optional {
        format!("`{key}` is empty: leave it out to cover every {key}")
    } else {
        format!("`{key}` is empty: a rule-wide entry names the rule and the scanner that reports it")
    })
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
    /// The symbols the reason rests on, each with the hash of its exact source
    /// span when the verdict was made ([FR-SC-32], [S-818]). The entry
    /// suppresses only while every one exists with that span. Empty: "no
    /// evidence recorded", and the entry is never checked. Sorted by symbol.
    ///
    /// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
    /// [S-818]: ../../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
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
    /// The entries in canonical order: fingerprints, then paths, then rule-wide.
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
    /// nor a `path` or with both (a `rule` with its `scanner` alone is a
    /// rule-wide entry; either one without the other names the missing key),
    /// a `rule` or `scanner` beside a fingerprint,
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

            // The line of the entry's `rule` or `scanner`: the one a lone key or a narrowing fault names.
            let narrowing_line = raw.rule.as_ref().or(raw.scanner.as_ref()).map(at);
            let (selector, selector_line) = match (raw.fingerprint, raw.path) {
                (Some(_), Some(path)) => return Err(invalid(at(&path), BOTH_SELECTORS)),
                (None, None) => match rule_wide(raw.rule, raw.scanner) {
                    Ok(Some((rule, scanner))) => {
                        let selector_line = at(&rule);
                        let rule = check(rule, &|rule| non_blank("rule", rule, false))?;
                        let scanner = check(scanner, &|scanner| non_blank("scanner", scanner, false))?;
                        (Selector::Rule { rule, scanner }, selector_line)
                    }
                    Ok(None) => return Err(invalid(line, NO_SELECTOR)),
                    Err(missing) => return Err(invalid(narrowing_line.unwrap_or(line), missing)),
                },
                (Some(fingerprint), None) => {
                    if let Some(narrowing) = narrowing_line {
                        return Err(invalid(narrowing, NARROWS_A_PATH));
                    }
                    let selector_line = at(&fingerprint);
                    let fingerprint = check(fingerprint, &|fp| check_fingerprint(&fp).map(|()| fp))?;
                    (Selector::Fingerprint { fingerprint }, selector_line)
                }
                (None, Some(path)) => {
                    let selector_line = at(&path);
                    let path = check(path, &|path| canonical_path(&path))?;
                    let rule = raw.rule.map(|rule| check(rule, &|rule| non_blank("rule", rule, true))).transpose()?;
                    let scanner = raw
                        .scanner
                        .map(|scanner| check(scanner, &|scanner| non_blank("scanner", scanner, true)))
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

            let mut evidence = Vec::new();
            for item in raw.evidence.unwrap_or_default() {
                let item_line = line_of(text, item.span().start);
                let item = item.into_inner();
                let (Some(symbol), Some(hash)) = (item.symbol, item.hash) else {
                    return Err(invalid(item_line, "an evidence item needs a `symbol` and a `hash`"));
                };
                evidence.push(Evidence { symbol, hash }.validated().map_err(|message| invalid(item_line, message))?);
            }
            let evidence = canonical(evidence).map_err(|message| invalid(line, message))?;

            if let Some(first) = first_line.insert(selector.clone(), selector_line) {
                return Err(invalid(
                    selector_line,
                    format!("duplicate {}: already given at line {first}", selector.describe()),
                ));
            }
            entries.push(IgnoreEntry { selector, reason, expires, evidence });
        }
        entries.sort_by(|a, b| a.selector.cmp(&b.selector));
        Ok(Self { entries })
    }

    /// The canonical text of this file: fingerprint entries sorted by
    /// fingerprint, then path entries sorted by path, rule and scanner, then
    /// rule-wide entries sorted by rule and scanner; one
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
                Selector::Rule { rule, scanner } => {
                    let _ = writeln!(out, "rule = {}", basic_string(rule));
                    let _ = writeln!(out, "scanner = {}", basic_string(scanner));
                }
            }
            let _ = writeln!(out, "reason = {}", basic_string(&entry.reason));
            if let Some(expires) = entry.expires {
                let _ = writeln!(out, "expires = {expires}");
            }
            if !entry.evidence.is_empty() {
                out.push_str("evidence = [\n");
                for item in &entry.evidence {
                    let _ = writeln!(
                        out,
                        "    {{ symbol = {}, hash = {} }},",
                        basic_string(&item.symbol),
                        basic_string(&item.hash)
                    );
                }
                out.push_str("]\n");
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
    /// evidence that is not a symbol with a span hash or names a symbol twice,
    /// or a selector the file already holds. A path is stored in its canonical
    /// spelling, and the entry is returned as stored.
    pub fn insert(&mut self, mut entry: IgnoreEntry) -> Result<&IgnoreEntry> {
        entry.selector = entry.selector.validated().map_err(refused)?;
        if entry.reason.trim().is_empty() {
            return Err(refused("`reason` is empty: a suppression must say why the risk is accepted"));
        }
        entry.evidence = canonical(entry.evidence).map_err(refused)?;
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

    /// Where the entry for `selector` sits, in any spelling a path is accepted in.
    fn position(&self, selector: &Selector) -> Option<usize> {
        let selector = selector.clone().validated().ok()?;
        self.entries.iter().position(|e| e.selector == selector)
    }

    /// Remove the entry for `selector` and return it; `None` when the file
    /// holds none. A path matches in any spelling [`canonical_path`] accepts.
    pub fn remove(&mut self, selector: &Selector) -> Option<IgnoreEntry> {
        let at = self.position(selector)?;
        Some(self.entries.remove(at))
    }

    /// The entry for `selector`, in any spelling a path is accepted in.
    pub fn get(&self, selector: &Selector) -> Option<&IgnoreEntry> {
        self.position(selector).map(|at| &self.entries[at])
    }

    /// Re-record the evidence of the entry for `selector`, keeping its reason
    /// and expiry. `None` when the file holds no such entry.
    fn replace_evidence(&mut self, selector: &Selector, evidence: Vec<Evidence>) -> Option<&IgnoreEntry> {
        let at = self.position(selector)?;
        self.entries[at].evidence = evidence;
        Some(&self.entries[at])
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
    evidence: Option<Vec<Spanned<RawEvidence>>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvidence {
    symbol: Option<String>,
    hash: Option<String>,
}

/// A fault in the ignore file at `line`, as the exit-2 [`ConfigError`] every
/// configuration fault is.
fn invalid(line: usize, message: impl Into<String>) -> anyhow::Error {
    ConfigError::InvalidValue { key: format!("{IGNORE_FILE} line {line}"), message: message.into() }.into()
}

/// A refused triage write, as the exit-2 [`ConfigError`] a malformed file is.
pub(super) fn refused(message: impl Into<String>) -> anyhow::Error {
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
    suppress_with_evidence(root, selector, reason, expires, today, Vec::new())
}

/// [`suppress`] with the `evidence` the reason rests on ([FR-SC-32]): each
/// symbol with the hash of its exact source span now, recorded by
/// [`GraphEvidence::pin`](super::evidence::GraphEvidence::pin). The entry then
/// suppresses only while every one still holds. No evidence is [`suppress`].
///
/// # Errors
/// As [`suppress`], and a repeated evidence symbol or a hash that is not one.
///
/// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
pub fn suppress_with_evidence(
    root: &Path,
    selector: impl Into<Selector>,
    reason: &str,
    expires: Option<NaiveDate>,
    today: NaiveDate,
    evidence: Vec<Evidence>,
) -> Result<IgnoreWrite> {
    if let Some(expires) = expires.filter(|expires| *expires < today) {
        return Err(refused(format!("`expires` {expires} is already past: the entry would suppress nothing")));
    }
    let entry = IgnoreEntry { selector: selector.into(), reason: reason.trim().to_string(), expires, evidence };
    rewrite(root, |file| file.insert(entry).cloned())
}

/// The entry a surface re-confirms for `fingerprint` ([FR-SC-32]): the entry
/// whose own key it is, else the entry that names the stored finding with that
/// fingerprint (or a key it had before a re-key) — whatever its evidence says,
/// so an entry whose evidence changed is found too.
///
/// # Errors
/// A [`ConfigError`] (exit 2): no entry and no stored finding carries
/// `fingerprint`, or no entry names the finding.
///
/// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
pub fn entry_for(root: &Path, fingerprint: &str, today: NaiveDate) -> Result<Selector> {
    let ignore = IgnoreFile::load(root)?;
    let own = Selector::from(fingerprint);
    if ignore.get(&own).is_some() {
        return Ok(own);
    }
    let finding = stored_finding(root, fingerprint)?
        .ok_or_else(|| refused(format!("no entry or stored finding has the fingerprint {fingerprint:?}")))?;
    let labelled = label_with(&[finding], &ignore, root, today, &super::evidence::NoGraph);
    match labelled.findings.into_iter().next().and_then(|item| item.suppression) {
        Some(Suppression::Suppressed { by, .. } | Suppression::OutOfDate { by, .. } | Suppression::Expired { by, .. }) => Ok(by),
        None => Err(refused(format!("no entry suppresses the finding {fingerprint:?}: nothing to re-confirm"))),
    }
}

/// Re-record the evidence of the entry for `selector` ([FR-SC-32]): the verdict
/// re-read against the code as it is now, and confirmed. `current` is given the
/// evidence the entry records now and answers what it records from here on —
/// the same symbols with the hash of each span now, or the symbols the caller
/// names instead — and refuses what cannot be confirmed (a symbol that no
/// longer exists). The entry's reason and expiry are kept.
///
/// # Errors
/// A [`ConfigError`] (exit 2) and no write: no entry carries `selector`, the
/// entry is expired on `today` (it suppresses nothing, so re-confirming would
/// change nothing: extend or remove it), the entry would record no evidence
/// (nothing to re-confirm), `current` refuses, or the file is malformed or not
/// in the canonical spelling. A `ConfigError::Write` when the write fails.
///
/// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
pub fn reconfirm(
    root: &Path,
    selector: &Selector,
    today: NaiveDate,
    current: impl FnOnce(&[Evidence]) -> Result<Vec<Evidence>>,
) -> Result<IgnoreWrite> {
    rewrite(root, |file| {
        let entry = file
            .get(selector)
            .ok_or_else(|| refused(format!("no entry suppresses {}", selector.describe())))?;
        if entry.is_expired(today) {
            return Err(refused(format!(
                "{} expired on {}: it suppresses nothing, so there is nothing to re-confirm; extend or remove it",
                selector.describe(),
                entry.expires.map(|day| day.to_string()).unwrap_or_default()
            )));
        }
        let recorded = entry.evidence.clone();
        let mut now = current(&recorded)?;
        now.sort();
        now.dedup_by(|a, b| a.symbol == b.symbol);
        if now.is_empty() {
            return Err(refused(format!(
                "{} records no evidence: there is nothing to re-confirm; name the symbols its reason rests on",
                selector.describe()
            )));
        }
        file.replace_evidence(selector, canonical(now).map_err(refused)?)
            .cloned()
            .ok_or_else(|| refused(format!("no entry suppresses {}", selector.describe())))
    })
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
                     fingerprint entries sorted by fingerprint, then path entries sorted by path, rule and \
                     scanner, then rule-wide entries sorted by rule and scanner, no comments), and rewriting it would drop its comments or formatting: edit it by hand instead",
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

// ── What the engine's two evidence writes do ([FR-SC-32], [S-818]) ───────────
//
// `Engine::security_suppress_with_evidence` and
// `Engine::security_reconfirm_with_evidence` are one call each into the two
// functions below. The one thing they cannot know is the graph's answer to
// "which symbol encloses this finding", which the engine computes from the open
// index and hands in as `enclosing`: the identity of the symbol, or `None` when
// the finding binds none.

/// The identity of the symbol enclosing a finding, as the graph reads it.
pub type Enclosing<'a> = &'a dyn Fn(&SecurityFinding) -> Result<Option<String>>;

/// The evidence to record for an entry on `selector` that names `symbols`: each
/// pinned to its span now, plus the enclosing symbol of the one finding a
/// fingerprint selector names; a symbol named twice is recorded once. Nothing
/// named, nothing recorded.
fn pin_evidence(root: &Path, selector: &Selector, symbols: &[String], enclosing: Enclosing<'_>) -> Result<Vec<Evidence>> {
    let named: Vec<&str> = symbols.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if named.is_empty() {
        return Ok(Vec::new());
    }
    let graph = GraphEvidence::at(root);
    let mut pinned = named.into_iter().map(|s| graph.pin(s)).collect::<Result<Vec<_>>>()?;
    if let Some(finding) = selector.fingerprint().map(|fp| stored_finding(root, fp)).transpose()?.flatten() {
        if let Some(identity) = enclosing(&finding)? {
            pinned.push(graph.pin(&identity)?);
        }
    }
    pinned.sort();
    pinned.dedup_by(|a, b| a.symbol == b.symbol);
    Ok(pinned)
}

/// Add a suppression for `selector` recording `symbols` as its evidence.
///
/// # Errors
/// A [`ConfigError`](crate::config::ConfigError) (exit 2) and no write: as
/// [`suppress_with_evidence`], an expiry that is not a date, or evidence that
/// names no symbol, several, or one the index is behind.
pub fn suppress_recording(
    root: &Path,
    selector: Selector,
    reason: &str,
    expires: Option<&str>,
    today: NaiveDate,
    symbols: &[String],
    enclosing: Enclosing<'_>,
) -> Result<IgnoreWrite> {
    let expires = expires.map(parse_expires).transpose()?;
    let evidence = pin_evidence(root, &selector, symbols, enclosing)?;
    suppress_with_evidence(root, selector, reason, expires, today, evidence)
}

/// Re-confirm the entry for `fingerprint` ([`entry_for`]): with `symbols`, it
/// records exactly those (and the finding's enclosing symbol); without, every
/// symbol it recorded, as the code is now.
///
/// # Errors
/// As [`entry_for`], [`reconfirm`] and [`suppress_recording`]'s evidence faults.
pub fn reconfirm_entry(
    root: &Path,
    fingerprint: &str,
    today: NaiveDate,
    symbols: &[String],
    enclosing: Enclosing<'_>,
) -> Result<IgnoreWrite> {
    let by = entry_for(root, fingerprint.trim(), today)?;
    let named = pin_evidence(root, &by, symbols, enclosing)?;
    reconfirm(root, &by, today, |recorded| {
        if !named.is_empty() {
            return Ok(named);
        }
        let graph = GraphEvidence::at(root);
        recorded.iter().map(|item| graph.pin(&item.symbol)).collect()
    })
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
        /// The symbols the reason rests on, every one confirmed unchanged
        /// ([FR-SC-32]). Absent from the payload when the entry records none:
        /// that reads "no evidence recorded", and the payload is as it was before
        /// evidence existed.
        ///
        /// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
        #[serde(skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<EvidenceSymbol>,
    },
    /// An entry names it, unexpired, but a symbol its reason rests on changed
    /// or is gone: the finding counts again, "verdict out of date: re-review",
    /// until the verdict is re-confirmed ([FR-SC-32], [BR-102]).
    ///
    /// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
    /// [BR-102]: ../../../../docs/specs/software-spec.md#328-security-findings
    #[serde(rename = "out_of_date")]
    OutOfDate {
        /// The entry's reason, as it was given.
        reason: String,
        /// The last day the entry would suppress; `None` when it never expires.
        #[serde(serialize_with = "serialize_date_opt")]
        expires: Option<NaiveDate>,
        /// The entry's selector: what re-confirming re-records.
        by: Selector,
        /// Each evidence symbol that no longer supports the verdict.
        changed: Vec<EvidenceChange>,
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

    /// Why it is left out of every count in code of `scope` ([`left_out`]).
    pub fn left_out(&self, scope: CodeScope) -> Option<LeftOut> {
        left_out(&self.finding, self.suppression.as_ref(), scope)
    }
}

/// Whether a finding sits in test code, on a dev-only dependency or in
/// production code ([FR-SC-28], [FR-SC-30]): computed when it is read, from the
/// graph, its path and its lockfile, and never stored. The classifier is
/// [`crate::security::scope`].
///
/// [FR-SC-28]: ../../../../docs/specs/requirements/FR-SC-28.md
/// [FR-SC-30]: ../../../../docs/specs/requirements/FR-SC-30.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeScope {
    #[default]
    Production,
    Test,
    /// A lockfile finding on a package the lockfile marks dev-only: the
    /// dependency scope `dev`, listed under `--scope all` alone.
    Dev,
}

/// Why a finding is left out of every count: the first reason that holds,
/// in this order — so each surface's excluded buckets partition the findings
/// left out, each in exactly one ([`left_out`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOut {
    /// An unexpired entry suppresses it, whatever its file's state.
    Suppressed,
    /// Its own file changed since the scan.
    NotFresh,
    /// It sits in test code ([FR-SC-28]).
    ///
    /// [FR-SC-28]: ../../../../docs/specs/requirements/FR-SC-28.md
    Test,
    /// It is on a dev-only dependency ([FR-SC-30]): "N more in dev-only
    /// dependencies". After test code, so a dev package's finding in a test
    /// path is counted as test code.
    ///
    /// [FR-SC-30]: ../../../../docs/specs/requirements/FR-SC-30.md
    Dev,
    /// Its scanner marks it low-confidence: "needs review" ([FR-SC-31]). Last,
    /// so a low-confidence finding in test code is counted as test code.
    ///
    /// [FR-SC-31]: ../../../../docs/specs/requirements/FR-SC-31.md
    NeedsReview,
}

/// **The one definition of a counted finding**, used by every security
/// surface — `security status`, the Security page's counts, Files & Risk, and
/// the workspace roll-up ([S-760], [FR-SC-08]): a finding counts unless an
/// unexpired entry suppresses it, its own file changed since the scan
/// ([`FRESHNESS_STALE`]), it sits in test code ([FR-SC-28]), it is on a
/// dev-only dependency ([FR-SC-30]), or its scanner
/// marks it low-confidence ([FR-SC-31]); this returns the first of those that
/// holds, `None` for a counted finding. Freshness is
/// the finding's, never its report's: a `--path` scan's report is not fresh at
/// HEAD, yet a finding in a file nothing has touched is current. A finding
/// bound to no file (`n/a`) has none to compare and counts. The gate never
/// asks: it reads every finding, whatever its scope ([BR-98]).
///
/// [FR-SC-08]: ../../../../docs/specs/requirements/FR-SC-08.md
/// [FR-SC-28]: ../../../../docs/specs/requirements/FR-SC-28.md
/// [FR-SC-30]: ../../../../docs/specs/requirements/FR-SC-30.md
/// [FR-SC-31]: ../../../../docs/specs/requirements/FR-SC-31.md
/// [BR-98]: ../../../../docs/specs/software-spec.md#328-security-findings
/// [S-760]: ../../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
pub fn left_out(finding: &SecurityFinding, suppression: Option<&Suppression>, scope: CodeScope) -> Option<LeftOut> {
    if matches!(suppression, Some(Suppression::Suppressed { .. })) {
        return Some(LeftOut::Suppressed);
    }
    if finding.freshness == crate::history::FRESHNESS_STALE {
        return Some(LeftOut::NotFresh);
    }
    match scope {
        CodeScope::Test => return Some(LeftOut::Test),
        CodeScope::Dev => return Some(LeftOut::Dev),
        CodeScope::Production => {}
    }
    finding.confidence.needs_review().then_some(LeftOut::NeedsReview)
}

/// Whether a finding counts: [`left_out`] finds no reason to leave it out.
pub fn counts(finding: &SecurityFinding, suppression: Option<&Suppression>, scope: CodeScope) -> bool {
    left_out(finding, suppression, scope).is_none()
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
    /// For an entry written under a key a finding had before a re-key
    /// (history.db migrations 7, 10, 11 and 13): the fingerprint that finding has now,
    /// which the entry should name. The entry still applies to the finding
    /// through its old key; the file is never rewritten to the new one.
    /// `None` otherwise.
    pub should_name: Option<String>,
    /// For an unexpired entry whose evidence no longer holds ([FR-SC-32]): each
    /// symbol that changed, is gone or could not be checked. The entry stops
    /// suppressing while it is non-empty. Empty otherwise — an expired entry is
    /// not checked, and one with no evidence has none to check.
    ///
    /// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
    pub out_of_date: Vec<EvidenceChange>,
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
    /// order [`naming_reporter`](Self::naming_reporter) reports them in. A glob the file would
    /// have rejected matches nothing.
    pub fn new(entries: impl IntoIterator<Item = &'e IgnoreEntry>, root: &Path) -> Self {
        let entries = entries
            .into_iter()
            .map(|entry| {
                let glob = match &entry.selector {
                    Selector::Path { path, .. } => compile_path(path).ok(),
                    Selector::Fingerprint { .. } | Selector::Rule { .. } => None,
                };
                (entry, glob)
            })
            .collect();
        Self { entries, root: root_key(root) }
    }

    /// The entries that name `finding` as reported by `reporter` — one of its
    /// [`reported_by`](SecurityFinding::reported_by) `(scanner, rule)` pairs —
    /// as `(position, entry)` in the order the matcher was given them. A
    /// fingerprint entry names the finding with its fingerprint, a key it had
    /// before a re-key (its legacy fingerprints), so a re-key never
    /// un-suppresses a finding, or any other reporter's key, so one naming any
    /// reporter's key names every reporter ([FR-SC-29]). A path entry names it
    /// where its project-relative location, or a directory holding it, the
    /// glob matches, and where the reporter's rule and scanner are the ones it
    /// narrows to; a finding with no location, or one reported outside the
    /// project, is named by no path entry. A rule-wide entry names it, with or
    /// without a location, where the reporter's rule and scanner are its own.
    ///
    /// [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
    pub fn naming_reporter<'m>(
        &'m self,
        finding: &'m SecurityFinding,
        reporter: (&'m str, &'m str),
    ) -> impl Iterator<Item = (usize, &'e IgnoreEntry)> + 'm {
        let location = self.project_path(finding);
        self.entries.iter().enumerate().filter_map(move |(at, (entry, glob))| {
            names(entry, glob.as_ref(), finding, location.as_deref(), reporter).then_some((at, *entry))
        })
    }

    /// The entry that suppresses `finding` when every entry the matcher was
    /// given suppresses what it names: the first naming its first reporter,
    /// provided each of its reporters is named by one ([FR-SC-29]); `None`
    /// otherwise. A finding one result stands behind is suppressed by the
    /// first entry naming it.
    ///
    /// [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
    pub fn suppressing(&self, finding: &SecurityFinding) -> Option<&'e IgnoreEntry> {
        let mut first = None;
        for reporter in finding.reported_by() {
            let (_, entry) = self.naming_reporter(finding, reporter).next()?;
            first.get_or_insert(entry);
        }
        first
    }

    /// `finding`'s location relative to the project root: its bound path, or
    /// the path its report named with the root taken off an absolute one.
    /// `None` without a location or for an absolute path outside the root.
    fn project_path(&self, finding: &SecurityFinding) -> Option<String> {
        under_root(&self.root, finding.location()?)
    }
}

/// Whether `entry` (with its compiled path `glob`) names `finding`, at its
/// project-relative `location`, as reported by `reporter` — `(scanner, rule)`
/// ([`EntryMatcher::naming_reporter`]).
fn names(
    entry: &IgnoreEntry,
    glob: Option<&GlobSet>,
    finding: &SecurityFinding,
    location: Option<&str>,
    (scanner_name, rule_id): (&str, &str),
) -> bool {
    match &entry.selector {
        Selector::Fingerprint { fingerprint } => finding.known_by(fingerprint),
        Selector::Path { rule, scanner, .. } => {
            glob.zip(location).is_some_and(|(glob, path)| at_or_under(glob, path))
                && rule.as_ref().is_none_or(|rule| rule == rule_id)
                && scanner.as_ref().is_none_or(|scanner| scanner.eq_ignore_ascii_case(scanner_name))
        }
        Selector::Rule { rule, scanner } => rule == rule_id && scanner.eq_ignore_ascii_case(scanner_name),
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
/// Reads no clock: `today` is given. `root` makes an absolute report path
/// project-relative for the path globs. A finding is suppressed by the
/// first unexpired entry that names it ([`EntryMatcher::naming_reporter`]) and whose
/// evidence holds, in the file's order: a fingerprint entry before a path entry
/// before a rule-wide one. When no entry naming it suppresses it, the finding is
/// unsuppressed and labelled by the first that names it: [`Suppression::Expired`]
/// when that one has expired, [`Suppression::OutOfDate`] when its evidence no
/// longer holds. A merged finding ([FR-SC-29]) is suppressed only when every
/// one of its reporters is ([`EntryMatcher::naming_reporter`]) — a fingerprint
/// entry naming any reporter's key suppresses them all — and is labelled by
/// its first reporter's entry; else by the first entry naming a reporter none
/// suppresses. Each entry counts the findings it
/// suppresses. An entry is orphaned when it names no finding in `findings`, so
/// pass every finding that could match — an entry for a scanner that has never
/// run reads as orphaned until it does. An entry naming a key a finding had
/// before a re-key (history.db migrations 7, 10, 11 and 13) still names, and so
/// suppresses, that finding, and names the finding's fingerprint as the one it
/// should name, unless some finding has the entry's key as its own.
///
/// An unexpired entry that records evidence ([FR-SC-32]) suppresses only while
/// every evidence symbol still has the span it was recorded with, read from the
/// graph and the working tree at `root` ([`GraphEvidence`]); the graph is opened
/// only for a file that records some. One whose evidence changed, vanished or
/// could not be checked no longer suppresses: its findings are labelled
/// [`Suppression::OutOfDate`] and count again. See [`label_with`] to read the
/// evidence from elsewhere.
///
/// [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
/// [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
/// [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
pub fn label(findings: &[SecurityFinding], ignore: &IgnoreFile, root: &Path, today: NaiveDate) -> Labelled {
    label_with(findings, ignore, root, today, &GraphEvidence::at(root))
}

/// [`label`] with the evidence spans read from `source`: the one labeller, pure
/// over its inputs. A source with no graph ([`NoGraph`](super::evidence::NoGraph))
/// verifies nothing, so an entry with evidence suppresses nothing.
pub fn label_with(
    findings: &[SecurityFinding],
    ignore: &IgnoreFile,
    root: &Path,
    today: NaiveDate,
    source: &dyn EvidenceSource,
) -> Labelled {
    let matcher = EntryMatcher::new(&ignore.entries, root);
    let mut named = vec![false; ignore.entries.len()];
    let mut suppressed = vec![0_usize; ignore.entries.len()];
    // An expired entry suppresses nothing whatever its evidence says, so it is
    // not read; an unexpired one is read once, for every finding it names.
    let changed: Vec<Vec<EvidenceChange>> = ignore
        .entries
        .iter()
        .map(|entry| if entry.is_expired(today) { Vec::new() } else { out_of_date(&entry.evidence, source) })
        .collect();

    let labelled = findings
        .iter()
        .map(|finding| {
            // Per reporter: the first entry naming it, and the first that
            // suppresses it. A merged finding is suppressed only when each of
            // its reporters is, by the first reporter's entry; otherwise it is
            // labelled by the first entry naming a reporter none suppresses —
            // expired or out of date — or by none (FR-SC-29).
            let reporters: Vec<_> = finding
                .reported_by()
                .into_iter()
                .map(|reporter| {
                    let mut first = None;
                    let mut live = None;
                    for (at, entry) in matcher.naming_reporter(finding, reporter) {
                        named[at] = true;
                        first.get_or_insert((at, entry));
                        if live.is_none() && !entry.is_expired(today) && changed[at].is_empty() {
                            live = Some((at, entry));
                        }
                    }
                    (first, live)
                })
                .collect();
            let label_by = if reporters.iter().any(|(_, live)| live.is_none()) {
                reporters.iter().filter(|(_, live)| live.is_none()).find_map(|(first, _)| *first)
            } else {
                reporters.first().and_then(|(_, live)| *live)
            };
            let suppression = label_by.map(|(at, entry)| {
                let (reason, expires, by) = (entry.reason.clone(), entry.expires, entry.selector.clone());
                if entry.is_expired(today) {
                    Suppression::Expired { reason, expires, by }
                } else if !changed[at].is_empty() {
                    Suppression::OutOfDate { reason, expires, by, changed: changed[at].clone() }
                } else {
                    suppressed[at] += 1;
                    let evidence = entry.evidence.iter().map(EvidenceSymbol::of).collect();
                    Suppression::Suppressed { reason, expires, by, evidence }
                }
            });
            LabelledFinding { finding: finding.clone(), suppression }
        })
        .collect();

    // The lowest current fingerprint per old key, so the answer is the same in
    // any finding order. A key some finding has now is no old key.
    // A merged finding's reporters are each a stored finding with keys of its
    // own (FR-SC-29).
    let stored: Vec<(&str, &[String])> = findings
        .iter()
        .flat_map(|f| -> Vec<(&str, &[String])> {
            if f.reporters.is_empty() {
                vec![(f.fingerprint.as_str(), f.legacy_fingerprints.as_slice())]
            } else {
                f.reporters.iter().map(|r| (r.fingerprint.as_str(), r.legacy_fingerprints.as_slice())).collect()
            }
        })
        .collect();
    let current: std::collections::HashSet<&str> = stored.iter().map(|(fingerprint, _)| *fingerprint).collect();
    let mut renamed: BTreeMap<&str, &str> = BTreeMap::new();
    for (fingerprint, legacy) in &stored {
        for old in legacy.iter().filter(|old| !current.contains(old.as_str())) {
            let now = renamed.entry(old.as_str()).or_insert(fingerprint);
            *now = (*now).min(fingerprint);
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
                out_of_date: changed[at].clone(),
                should_name: entry
                    .selector
                    .fingerprint()
                    .and_then(|fingerprint| renamed.get(fingerprint))
                    .map(|now| now.to_string()),
            }
        })
        .collect();

    Labelled { findings: labelled, entries }
}
