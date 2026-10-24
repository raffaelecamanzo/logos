//! Coverage-report parsing behind a small format trait ([FR-CV-01], [ADR-23]).
//!
//! Four formats ship — LCOV (line-oriented text), Cobertura (XML), JaCoCo (XML)
//! and the Go coverprofile (`mode:` header + block lines) — each an impl of
//! [`CoverageParser`], so further formats (Clover, …) stay purely additive
//! ([CR-007] §3.3, [FR-CV-12]). Format is auto-detected from content
//! ([`detect_format`]) and overridable via `--format`.
//!
//! # Atomic rejection ([FR-CV-01], [NFR-RA-05])
//! A malformed or truncated report is a hard [`Err`] from [`parse`] — the caller
//! ([`super::ingest`]) parses **before** it opens the store, so a rejected report
//! never writes a partial snapshot: the store stays byte-identical. Branch
//! coverage (LCOV `BRDA`, Cobertura branch rates) is out of scope for v1 ([CR-007]
//! §3.3) and silently ignored; only line hits are read.
//!
//! [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
//! [FR-CV-12]: ../../../../docs/specs/requirements/FR-CV-12.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
//! [CR-007]: ../../../../docs/requests/CR-007-coverage-ingestion.md

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

/// The coverage formats ingested ([FR-CV-01], [FR-CV-12]). A `Serialize`
/// read-model field so the [`super::IngestSummary`] renders the detected/forced
/// format for free on both surfaces ([ADR-01]).
///
/// [ADR-01]: ../../../../docs/specs/architecture/decisions/ADR-01.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageFormat {
    /// LCOV tracefile (`TN:`/`SF:`/`DA:` records).
    Lcov,
    /// Cobertura XML (`<coverage>` root, `<class filename>` + `<line number hits>`).
    Cobertura,
    /// JaCoCo XML (`<report>` root, `<package>/<sourcefile>` + `<line nr mi ci>`).
    Jacoco,
    /// Go coverprofile (`mode:` header, `file:l.c,l.c stmts count` blocks).
    Gocover,
}

impl CoverageFormat {
    /// The lowercase wire token (the stored `format`, the `--format` value).
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageFormat::Lcov => "lcov",
            CoverageFormat::Cobertura => "cobertura",
            CoverageFormat::Jacoco => "jacoco",
            CoverageFormat::Gocover => "gocover",
        }
    }

    /// Parse a `--format` flag value, or `None` for an unrecognized token.
    pub fn from_flag(s: &str) -> Option<Self> {
        match s {
            "lcov" => Some(CoverageFormat::Lcov),
            "cobertura" => Some(CoverageFormat::Cobertura),
            "jacoco" => Some(CoverageFormat::Jacoco),
            "gocover" => Some(CoverageFormat::Gocover),
            _ => None,
        }
    }
}

/// One instrumented line and its execution count, as read from a report. Within
/// a single report a line number may legitimately repeat (Cobertura emits the
/// same line at class- and method-level); the caller dedups per report by line
/// number, and only **across** reports do hits sum ([FR-CV-04]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineHit {
    /// 1-based source line number.
    pub(crate) line_no: i64,
    /// Execution count for the line (`0` = instrumented but not executed).
    pub(crate) hits: i64,
}

/// One file's coverage as named **in the report** — the path is verbatim from the
/// report (often absolute or build-dir-relative) and is mapped to an indexed file
/// later by [`super::pathmap`] ([FR-CV-03]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportFile {
    /// The source path exactly as the report named it.
    pub(crate) path: String,
    /// Instrumented lines, in report order (duplicates possible; deduped later).
    pub(crate) lines: Vec<LineHit>,
}

/// A parser for one coverage format. Keeping this a trait (rather than a `match`)
/// is what makes later formats additive ([FR-CV-01] "parsers sit behind a small
/// format trait").
pub(crate) trait CoverageParser {
    /// Parse the full report text into per-file line hits, or fail loud on a
    /// malformed report ([FR-CV-01] atomic rejection).
    fn parse(&self, text: &str) -> Result<Vec<ReportFile>>;
}

/// Auto-detect the report format from content ([FR-CV-01], [FR-CV-12]): the XML
/// root element (JaCoCo `<report>` vs. Cobertura `<coverage>`), the Go
/// coverprofile's `mode:` header, or LCOV's line records. Returns `None` for an
/// unrecognized (non-coverage) file, which the caller turns into a loud rejection
/// unless `--format` forces a parser.
pub(crate) fn detect_format(text: &str) -> Option<CoverageFormat> {
    let trimmed = text.trim_start();
    // XML route: only a document carrying a known coverage root is accepted — an
    // arbitrary XML/HTML file is NOT a coverage report and must be rejected, not
    // silently parsed to zero files (FR-CV-01 "non-coverage file is rejected").
    if trimmed.starts_with('<') {
        if is_jacoco(text) {
            return Some(CoverageFormat::Jacoco);
        }
        return text
            .contains("<coverage")
            .then_some(CoverageFormat::Cobertura);
    }
    // Go coverprofile: the first line is `mode: set|count|atomic`. Checked before
    // LCOV so no profile line can be mistaken for a tracefile record.
    if is_gocover_header(trimmed.lines().next().unwrap_or_default()) {
        return Some(CoverageFormat::Gocover);
    }
    // LCOV route: a tracefile is a sequence of records; `TN:` (test name) or
    // `SF:` (source file) is the unambiguous opener.
    let is_lcov = text
        .lines()
        .any(|l| l.starts_with("TN:") || l.starts_with("SF:"));
    is_lcov.then_some(CoverageFormat::Lcov)
}

/// A JaCoCo XML report: the root element is `<report>` **and** the document
/// carries a JaCoCo marker (its DTD id, a `<sessioninfo>`, a `<counter>` or a
/// `<package>`). A bare `<report>` root is common to other tools, so the root
/// alone never claims the format.
fn is_jacoco(text: &str) -> bool {
    let mut reader = Reader::from_str(text);
    let mut buf = Vec::new();
    let root_is_report = loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => break e.local_name().as_ref() == b"report",
            Ok(Event::Eof) | Err(_) => break false,
            Ok(_) => buf.clear(),
        }
    };
    root_is_report
        && ["-//JACOCO//", "<sessioninfo", "<counter", "<package"]
            .iter()
            .any(|marker| text.contains(marker))
}

/// The Go coverprofile header line: `mode: set`, `mode: count` or `mode: atomic`.
fn is_gocover_header(line: &str) -> bool {
    line.strip_prefix("mode:")
        .is_some_and(|m| matches!(m.trim(), "set" | "count" | "atomic"))
}

/// Parse `text` with the parser for `format`. Each format is a [`CoverageParser`]
/// impl, which is what makes further formats additive ([FR-CV-01]); the dispatch
/// is a static `match` — no trait object is needed when the format is known at
/// the call site.
pub(crate) fn parse(format: CoverageFormat, text: &str) -> Result<Vec<ReportFile>> {
    match format {
        CoverageFormat::Lcov => Lcov.parse(text),
        CoverageFormat::Cobertura => Cobertura.parse(text),
        CoverageFormat::Jacoco => Jacoco.parse(text),
        CoverageFormat::Gocover => Gocover.parse(text),
    }
}

/// The LCOV tracefile parser ([FR-CV-01]).
struct Lcov;

impl CoverageParser for Lcov {
    fn parse(&self, text: &str) -> Result<Vec<ReportFile>> {
        let mut files = Vec::new();
        let mut current: Option<ReportFile> = None;

        for (idx, line) in text.lines().enumerate() {
            if let Some(path) = line.strip_prefix("SF:") {
                // A new source-file record opens; close any prior one defensively
                // (a well-formed tracefile ends each with `end_of_record`).
                if let Some(f) = current.take() {
                    files.push(f);
                }
                current = Some(ReportFile {
                    path: path.trim().to_string(),
                    lines: Vec::new(),
                });
            } else if let Some(rest) = line.strip_prefix("DA:") {
                // `DA:<line>,<hits>[,<checksum>]` — only the first two fields are
                // read; a missing field or non-integer is a corrupt tracefile.
                let f = current.as_mut().with_context(|| {
                    format!("LCOV line {}: `DA:` record before any `SF:`", idx + 1)
                })?;
                let mut parts = rest.splitn(3, ',');
                let (Some(line_field), Some(hits_field)) = (parts.next(), parts.next()) else {
                    bail!("LCOV line {}: malformed `DA:` record `{rest}`", idx + 1);
                };
                let line_no: i64 = line_field.trim().parse().with_context(|| {
                    format!(
                        "LCOV line {}: non-integer line number `{line_field}`",
                        idx + 1
                    )
                })?;
                let hits: i64 = hits_field.trim().parse().with_context(|| {
                    format!(
                        "LCOV line {}: non-integer hit count `{hits_field}`",
                        idx + 1
                    )
                })?;
                f.lines.push(LineHit { line_no, hits });
            } else if line.trim() == "end_of_record" {
                if let Some(f) = current.take() {
                    files.push(f);
                }
            }
            // FN/FNDA/BRDA/LF/LH/BRF/BRH and blank lines are ignored: function and
            // branch coverage are out of scope for v1 (CR-007 §3.3).
        }
        // A trailing record with no `end_of_record` (some tools omit the final
        // one) is still captured.
        if let Some(f) = current.take() {
            files.push(f);
        }
        Ok(files)
    }
}

/// The per-format half of an XML report scan: what to do on an element opening
/// and closing. One value holds the state both halves share.
trait XmlHandler {
    /// A start (`is_start`) or self-closing element opened.
    fn open(&mut self, e: &quick_xml::events::BytesStart, is_start: bool) -> Result<()>;
    /// An end element (by local name) closed.
    fn close(&mut self, name: &[u8]);
}

/// Stream an XML report through `handler`. Malformed XML — a mismatched end tag,
/// or a document truncated before its root closes — is a hard error naming
/// `label`: the atomic-rejection path every XML format shares.
fn scan_xml(text: &str, label: &str, handler: &mut impl XmlHandler) -> Result<()> {
    let mut reader = Reader::from_str(text);
    let config = reader.config_mut();
    config.trim_text(true);
    // Reject a mismatched end tag (`<a></b>`) as a hard error.
    config.check_end_names = true;

    // Track element nesting so a truncated document (unclosed tags at EOF) is
    // rejected — `check_end_names` alone does not catch premature EOF.
    let mut depth: i32 = 0;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf).with_context(|| {
            format!("malformed {label} XML at byte {}", reader.buffer_position())
        })? {
            Event::Eof => break,
            Event::Start(e) => {
                depth += 1;
                handler.open(&e, true)?;
            }
            Event::Empty(e) => handler.open(&e, false)?,
            Event::End(e) => {
                depth -= 1;
                handler.close(e.local_name().as_ref());
            }
            _ => {}
        }
        buf.clear();
    }
    anyhow::ensure!(
        depth == 0,
        "malformed {label} XML: unclosed element(s) at end of document"
    );
    Ok(())
}

/// The Cobertura XML parser ([FR-CV-01]).
///
/// Streams the document with `quick-xml`, tracking the current `<class filename>`
/// and collecting its `<line number hits>` children. Malformed XML (a truncated
/// or corrupt report) is a hard error from the reader — the atomic-rejection path.
struct Cobertura;

impl CoverageParser for Cobertura {
    fn parse(&self, text: &str) -> Result<Vec<ReportFile>> {
        let mut state = CoberturaState::default();
        scan_xml(text, "Cobertura", &mut state)?;
        Ok(state.files)
    }
}

/// Files collected so far and the `<class filename>` whose `<line>`s are being read.
#[derive(Default)]
struct CoberturaState {
    files: Vec<ReportFile>,
    current_path: Option<String>,
}

impl XmlHandler for CoberturaState {
    fn open(&mut self, e: &quick_xml::events::BytesStart, _is_start: bool) -> Result<()> {
        handle_open(e, &mut self.files, &mut self.current_path)
    }

    fn close(&mut self, name: &[u8]) {
        if name == b"class" {
            self.current_path = None;
        }
    }
}

/// Handle a Cobertura start/empty element: open a file record on `<class>` and
/// collect a `<line>`'s hit count under the current class. Cobertura nests
/// method-level `<lines>` *and* a class-level `<lines>`; both reference the same
/// source lines, so all are collected and deduped by line number in the caller
/// (taking max), never double-counted.
fn handle_open(
    e: &quick_xml::events::BytesStart,
    files: &mut Vec<ReportFile>,
    current_path: &mut Option<String>,
) -> Result<()> {
    match e.local_name().as_ref() {
        b"class" => {
            // Always reset the current file — a `<class>` with no `filename`
            // sets it to `None`, so its `<line>`s are ignored rather than
            // silently attributed to the preceding class (never fabricate).
            *current_path = attr(e, b"filename")?;
            if let Some(path) = current_path.as_ref() {
                if !files.iter().any(|f| &f.path == path) {
                    files.push(ReportFile {
                        path: path.clone(),
                        lines: Vec::new(),
                    });
                }
            }
        }
        b"line" => {
            if let Some(path) = current_path.as_ref() {
                let (Some(number), Some(hits)) = (attr(e, b"number")?, attr(e, b"hits")?) else {
                    // A `<line>` missing `number`/`hits` is not a line-coverage
                    // record (e.g. a condition-coverage row); skip, never fabricate.
                    return Ok(());
                };
                let line_no: i64 = number
                    .trim()
                    .parse()
                    .with_context(|| format!("Cobertura: non-integer line number `{number}`"))?;
                let hits: i64 = hits
                    .trim()
                    .parse()
                    .with_context(|| format!("Cobertura: non-integer hit count `{hits}`"))?;
                if let Some(f) = files.iter_mut().find(|f| &f.path == path) {
                    f.lines.push(LineHit { line_no, hits });
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The JaCoCo XML parser ([FR-CV-12]).
///
/// A report names a file by `<package name>` (slash-separated, the Java package
/// directory) plus `<sourcefile name>`; the report path is their join — a
/// source-root-relative path the mapper binds by longest-unique-suffix, so a Maven
/// multi-module aggregate (`<group>` per module) needs no module knowledge here
/// and two modules holding the same package-relative file stay unmatched, never
/// guessed between ([FR-CV-03]). Each `<line nr mi ci>` under a `<sourcefile>` is
/// one instrumented line whose hit count is `ci` (covered instructions), so
/// `ci > 0` is covered. A line with no instructions at all (`mi + ci == 0`) is not
/// instrumented and is skipped; `<counter>` rows (branch, method, class) are
/// ignored, as branch counters are for LCOV and Cobertura.
struct Jacoco;

impl CoverageParser for Jacoco {
    fn parse(&self, text: &str) -> Result<Vec<ReportFile>> {
        let mut state = JacocoState::default();
        scan_xml(text, "JaCoCo", &mut state)?;
        Ok(state.files)
    }
}

/// Files collected so far, the enclosing `<package name>`, and the index of the
/// `<sourcefile>` whose `<line>`s are being read.
#[derive(Default)]
struct JacocoState {
    files: Vec<ReportFile>,
    package: String,
    current: Option<usize>,
}

impl XmlHandler for JacocoState {
    fn open(&mut self, e: &quick_xml::events::BytesStart, is_start: bool) -> Result<()> {
        match e.local_name().as_ref() {
            // An empty `<package/>` has no children and no end tag to clear it.
            b"package" if is_start => self.package = attr(e, b"name")?.unwrap_or_default(),
            b"sourcefile" => {
                self.current = None;
                let Some(name) = attr(e, b"name")? else {
                    return Ok(());
                };
                let path = if self.package.is_empty() {
                    name
                } else {
                    format!("{}/{name}", self.package.trim_end_matches('/'))
                };
                let idx = match self.files.iter().position(|f| f.path == path) {
                    Some(idx) => idx,
                    None => {
                        self.files.push(ReportFile {
                            path,
                            lines: Vec::new(),
                        });
                        self.files.len() - 1
                    }
                };
                // An empty `<sourcefile/>` has no children to claim.
                self.current = is_start.then_some(idx);
            }
            b"line" => {
                let Some(idx) = self.current else {
                    return Ok(());
                };
                let Some(nr) = attr(e, b"nr")? else {
                    return Ok(());
                };
                let line_no = non_negative(&nr, "line number", "JaCoCo")?;
                let missed = attr(e, b"mi")?
                    .map_or(Ok(0), |v| non_negative(&v, "missed count", "JaCoCo"))?;
                let covered = attr(e, b"ci")?
                    .map_or(Ok(0), |v| non_negative(&v, "covered count", "JaCoCo"))?;
                // No instructions at all: not an instrumented line.
                if missed > 0 || covered > 0 {
                    self.files[idx].lines.push(LineHit {
                        line_no,
                        hits: covered,
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn close(&mut self, name: &[u8]) {
        match name {
            b"sourcefile" => self.current = None,
            b"package" => {
                self.current = None;
                self.package.clear();
            }
            _ => {}
        }
    }
}

/// The most per-line hits one Go profile may expand to. A block names a line
/// *range*, so a hostile `1.1,999999999.1` would otherwise ask for a billion
/// entries from one short line; past this a report is rejected as malformed,
/// before anything is written. Real profiles expand to one entry per source line
/// (a few hundred thousand for a very large module).
const GOCOVER_MAX_EXPANDED_LINES: usize = 5_000_000;

/// The Go coverprofile parser ([FR-CV-12]).
///
/// The first line is `mode: set|count|atomic`; every later line is one block,
/// `<import-path>/<file>.go:<l>.<c>,<l>.<c> <statements> <count>`. A block is
/// expanded to one hit per line it spans (`count` for `count`/`atomic` modes, `0`
/// or `1` for `set`); where blocks share a line the **maximum** count wins, so a
/// line is covered when any block touching it ran. A block with `0` statements
/// instruments nothing (an empty body) and is skipped. The `go.mod` module prefix
/// is *not* stripped here — the parser sees only the text; [`super::ingest`]
/// rewrites it against the project's `go.mod` files before path mapping.
struct Gocover;

impl CoverageParser for Gocover {
    fn parse(&self, text: &str) -> Result<Vec<ReportFile>> {
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        if !lines
            .next()
            .is_some_and(|(_, l)| is_gocover_header(l.trim_start()))
        {
            bail!("Go coverprofile: missing the `mode: set|count|atomic` header line");
        }

        // Per file, in first-seen order: line → max count. A `BTreeMap` keeps the
        // expansion deterministic ([NFR-RA-06]) and bounds memory by distinct lines.
        let mut order: Vec<String> = Vec::new();
        let mut per_file: std::collections::HashMap<String, std::collections::BTreeMap<i64, i64>> =
            std::collections::HashMap::new();
        let mut expanded = 0usize;

        for (idx, raw) in lines {
            let row = idx + 1;
            let raw = raw.trim();
            let (name, rest) = raw.rsplit_once(':').with_context(|| {
                format!("Go coverprofile line {row}: no `file:range` separator")
            })?;
            let mut fields = rest.split_whitespace();
            let (Some(range), Some(stmts), Some(count), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                bail!("Go coverprofile line {row}: expected `<range> <statements> <count>`");
            };
            let (start, end) = range.split_once(',').with_context(|| {
                format!("Go coverprofile line {row}: malformed range `{range}`")
            })?;
            let start_line = go_position(start, row)?;
            let end_line = go_position(end, row)?;
            let stmts = non_negative(
                stmts,
                "statement count",
                format_args!("Go coverprofile line {row}"),
            )?;
            let count = non_negative(
                count,
                "hit count",
                format_args!("Go coverprofile line {row}"),
            )?;
            anyhow::ensure!(
                start_line <= end_line,
                "Go coverprofile line {row}: range `{range}` ends before it starts"
            );
            if name.is_empty() {
                bail!("Go coverprofile line {row}: empty file name");
            }
            if stmts == 0 {
                continue;
            }
            expanded = expanded.saturating_add((end_line - start_line + 1) as usize);
            anyhow::ensure!(
                expanded <= GOCOVER_MAX_EXPANDED_LINES,
                "Go coverprofile line {row}: report expands past {GOCOVER_MAX_EXPANDED_LINES} lines"
            );
            let slot = per_file.entry(name.to_string()).or_insert_with(|| {
                order.push(name.to_string());
                Default::default()
            });
            for line_no in start_line..=end_line {
                let hits = slot.entry(line_no).or_insert(count);
                if count > *hits {
                    *hits = count;
                }
            }
        }

        Ok(order
            .into_iter()
            .map(|path| {
                let lines = per_file
                    .remove(&path)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(line_no, hits)| LineHit { line_no, hits })
                    .collect();
                ReportFile { path, lines }
            })
            .collect())
    }
}

/// The line of a Go `line.column` position (`12.5` → `12`); the column must be an
/// integer too, so a corrupt position is rejected rather than half-read.
fn go_position(pos: &str, row: usize) -> Result<i64> {
    let (line, col) = pos
        .split_once('.')
        .with_context(|| format!("Go coverprofile line {row}: malformed position `{pos}`"))?;
    let line = non_negative(
        line,
        "line number",
        format_args!("Go coverprofile line {row}"),
    )?;
    non_negative(col, "column", format_args!("Go coverprofile line {row}"))?;
    anyhow::ensure!(
        line >= 1,
        "Go coverprofile line {row}: line numbers start at 1"
    );
    Ok(line)
}

/// A non-negative integer from a report field, or a hard error naming where
/// (`origin`, e.g. `JaCoCo` or `Go coverprofile line 7`) and what it was.
fn non_negative(value: &str, what: &str, origin: impl std::fmt::Display) -> Result<i64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .with_context(|| format!("{origin}: invalid {what} `{value}`"))
}

/// Read a UTF-8 attribute value from a start/empty element, decoding XML entities
/// (`&amp;` in a path, etc.). `Ok(None)` when the attribute is absent.
fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Result<Option<String>> {
    for attr in e.attributes() {
        let attr = attr.context("malformed XML attribute")?;
        if attr.key.local_name().as_ref() == key {
            // `normalized_value` decodes XML entities and applies the spec's
            // attribute-value normalization, assuming UTF-8 (the coverage XML
            // formats are always UTF-8 in practice; the `encoding` feature is
            // off). They are XML 1.0 formats, and the parser does not read the
            // declaration, so the implicit-1.0 rules are the correct ones to apply.
            let decoded = attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .context("decoding XML attribute value")?;
            return Ok(Some(decoded.into_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_lcov_and_cobertura_and_rejects_unknown() {
        assert_eq!(
            detect_format("TN:\nSF:src/lib.rs\nDA:1,1\nend_of_record\n"),
            Some(CoverageFormat::Lcov)
        );
        assert_eq!(
            detect_format("<?xml version=\"1.0\"?>\n<coverage><packages/></coverage>"),
            Some(CoverageFormat::Cobertura)
        );
        // A non-coverage XML/HTML document is not detected (→ loud rejection).
        assert_eq!(detect_format("<html><body>nope</body></html>"), None);
        // Random text is not a coverage report.
        assert_eq!(detect_format("hello world\nnot coverage\n"), None);
    }

    /// `--format` flag parsing: the two recognized tokens map; anything else is
    /// `None` (the CLI surface rejects it).
    #[test]
    fn from_flag_parses_known_tokens() {
        assert_eq!(
            CoverageFormat::from_flag("lcov"),
            Some(CoverageFormat::Lcov)
        );
        assert_eq!(
            CoverageFormat::from_flag("cobertura"),
            Some(CoverageFormat::Cobertura)
        );
        assert_eq!(
            CoverageFormat::from_flag("jacoco"),
            Some(CoverageFormat::Jacoco)
        );
        assert_eq!(
            CoverageFormat::from_flag("gocover"),
            Some(CoverageFormat::Gocover)
        );
        assert_eq!(CoverageFormat::from_flag("clover"), None);
        assert_eq!(CoverageFormat::from_flag("LCOV"), None, "case-sensitive");
        for f in [
            CoverageFormat::Lcov,
            CoverageFormat::Cobertura,
            CoverageFormat::Jacoco,
            CoverageFormat::Gocover,
        ] {
            assert_eq!(CoverageFormat::from_flag(f.as_str()), Some(f), "round-trip");
        }
    }

    #[test]
    fn lcov_parses_files_and_lines() {
        let text = "TN:suite\n\
                    SF:/build/ci/src/lib.rs\n\
                    DA:1,5\n\
                    DA:2,0\n\
                    BRDA:1,0,0,1\n\
                    LF:2\n\
                    LH:1\n\
                    end_of_record\n\
                    SF:src/util.rs\n\
                    DA:10,3\n\
                    end_of_record\n";
        let files = Lcov.parse(text).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "/build/ci/src/lib.rs");
        assert_eq!(
            files[0].lines,
            vec![
                LineHit {
                    line_no: 1,
                    hits: 5
                },
                LineHit {
                    line_no: 2,
                    hits: 0
                },
            ]
        );
        assert_eq!(files[1].path, "src/util.rs");
        assert_eq!(
            files[1].lines,
            vec![LineHit {
                line_no: 10,
                hits: 3
            }]
        );
    }

    #[test]
    fn lcov_rejects_truncated_da_record() {
        // A `DA:` line cut off mid-record (no hit count) is a corrupt tracefile.
        let text = "SF:src/lib.rs\nDA:12\nend_of_record\n";
        assert!(Lcov.parse(text).is_err(), "truncated DA is rejected");
        // A non-integer hit count is likewise corrupt.
        let bad = "SF:src/lib.rs\nDA:12,notanumber\nend_of_record\n";
        assert!(Lcov.parse(bad).is_err());
    }

    #[test]
    fn cobertura_parses_classes_and_lines() {
        let text = r#"<?xml version="1.0"?>
<coverage>
  <packages>
    <package name="p">
      <classes>
        <class filename="src/lib.rs">
          <methods>
            <method name="f"><lines><line number="1" hits="5"/></lines></method>
          </methods>
          <lines>
            <line number="1" hits="5"/>
            <line number="2" hits="0" branch="true"/>
          </lines>
        </class>
      </classes>
    </package>
  </packages>
</coverage>"#;
        let files = Cobertura.parse(text).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/lib.rs");
        // Method-level + class-level `<line number="1">` both captured (deduped by
        // the caller); the raw parse keeps all three line entries.
        assert_eq!(files[0].lines.len(), 3);
        assert!(files[0].lines.contains(&LineHit {
            line_no: 1,
            hits: 5
        }));
        assert!(files[0].lines.contains(&LineHit {
            line_no: 2,
            hits: 0
        }));
    }

    #[test]
    fn cobertura_decodes_entities_in_filename() {
        let text = r#"<coverage><classes><class filename="src/a&amp;b.rs"><lines><line number="1" hits="1"/></lines></class></classes></coverage>"#;
        let files = Cobertura.parse(text).unwrap();
        assert_eq!(files[0].path, "src/a&b.rs", "XML entity decoded in path");
    }

    #[test]
    fn cobertura_rejects_malformed_xml() {
        // A truncated document (unclosed tag) is a hard parse error.
        let text =
            r#"<coverage><classes><class filename="x.rs"><lines><line number="1" hits="1"/>"#;
        assert!(Cobertura.parse(text).is_err(), "unclosed XML is rejected");
    }

    /// A `<class>` with no `filename` must not attribute its `<line>`s to the
    /// preceding class — they are ignored, never fabricated onto another file.
    #[test]
    fn cobertura_class_without_filename_does_not_leak_lines() {
        let text = r#"<coverage><classes>
            <class filename="src/lib.rs"><lines><line number="1" hits="5"/></lines></class>
            <class><lines><line number="2" hits="9"/></lines></class>
        </classes></coverage>"#;
        let files = Cobertura.parse(text).unwrap();
        assert_eq!(files.len(), 1, "the filename-less class adds no file");
        assert_eq!(files[0].path, "src/lib.rs");
        assert_eq!(
            files[0].lines,
            vec![LineHit {
                line_no: 1,
                hits: 5
            }],
            "line 2 from the filename-less class is not leaked onto src/lib.rs"
        );
    }

    const JACOCO: &str =
        include_str!("../../../tests/fixtures/coverage/jacoco/maven-aggregate.xml");
    const GOCOVER: &str = include_str!("../../../tests/fixtures/coverage/gocover/coverprofile.out");

    /// Auto-detection picks each of the four parsers from content alone, with
    /// LCOV and Cobertura unchanged ([FR-CV-12]); each near miss — a bare
    /// `<report>`, a `mode:` line with no valid mode — is not claimed.
    #[test]
    fn detects_all_four_formats_and_not_their_near_misses() {
        assert_eq!(
            detect_format("TN:\nSF:src/lib.rs\nDA:1,1\nend_of_record\n"),
            Some(CoverageFormat::Lcov)
        );
        assert_eq!(
            detect_format("<?xml version=\"1.0\"?>\n<coverage><packages/></coverage>"),
            Some(CoverageFormat::Cobertura)
        );
        assert_eq!(detect_format(JACOCO), Some(CoverageFormat::Jacoco));
        assert_eq!(detect_format(GOCOVER), Some(CoverageFormat::Gocover));
        // Leading blank lines before the Go header, and each of the three modes.
        for mode in ["set", "count", "atomic"] {
            assert_eq!(
                detect_format(&format!("\n\nmode: {mode}\na.go:1.1,2.1 1 1\n")),
                Some(CoverageFormat::Gocover),
                "{mode}"
            );
        }
        // A JaCoCo report with no DOCTYPE is still recognised by its content.
        assert_eq!(
            detect_format("<report name=\"x\"><sessioninfo id=\"a\"/></report>"),
            Some(CoverageFormat::Jacoco)
        );

        // Near misses.
        assert_eq!(
            detect_format("<report><item/></report>"),
            None,
            "a bare `<report>` root is not JaCoCo"
        );
        assert_eq!(
            detect_format("<html><report name=\"x\"><sessioninfo/></report></html>"),
            None,
            "`<report>` must be the ROOT element"
        );
        assert_eq!(detect_format("mode: banana\na.go:1.1,2.1 1 1\n"), None);
        assert_eq!(
            detect_format("a.go:1.1,2.1 1 1\n"),
            None,
            "no header, no Go"
        );
        assert_eq!(detect_format("modes: set\n"), None);
    }

    #[test]
    fn jacoco_joins_package_and_sourcefile_and_reads_ci_as_hits() {
        let files = Jacoco.parse(JACOCO).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "com/acme/core/Greeter.java",
                "com/acme/core/Util.java",
                "com/acme/web/Handler.java",
                "com/acme/gone/Missing.java",
            ],
            "package/sourcefile joins; the repeated Util.java is one entry"
        );
        assert_eq!(
            files[0].lines,
            vec![
                LineHit {
                    line_no: 3,
                    hits: 4
                },
                LineHit {
                    line_no: 4,
                    hits: 2
                },
                LineHit {
                    line_no: 5,
                    hits: 0
                },
                LineHit {
                    line_no: 6,
                    hits: 1
                },
            ]
        );
        assert_eq!(
            files[2].lines,
            vec![
                LineHit {
                    line_no: 10,
                    hits: 7
                },
                LineHit {
                    line_no: 11,
                    hits: 0
                },
                LineHit {
                    line_no: 13,
                    hits: 3
                },
            ],
            "line 12 has no instructions: not instrumented, not read"
        );
    }

    #[test]
    fn jacoco_ignores_counters_and_lines_outside_a_sourcefile() {
        let text = r#"<report name="r"><sessioninfo id="a"/>
            <package name="p"><class name="p/A"><method name="m" line="1"><line nr="9" mi="1" ci="1"/></method></class>
              <sourcefile name="A.java"><line nr="1" mi="0" ci="2"/></sourcefile>
              <counter type="LINE" missed="0" covered="1"/>
            </package>
            <package name=""><sourcefile name="Root.java"><line nr="1" mi="1" ci="0"/></sourcefile></package>
            <package name="q"/>
            <sourcefile name="Stray.java"/></report>"#;
        let files = Jacoco.parse(text).unwrap();
        assert_eq!(
            files.len(),
            3,
            "A.java, the default-package Root.java, Stray.java"
        );
        assert_eq!(files[0].path, "p/A.java");
        assert_eq!(
            files[0].lines,
            vec![LineHit {
                line_no: 1,
                hits: 2
            }]
        );
        assert_eq!(
            files[1].path, "Root.java",
            "an empty package name adds no slash"
        );
        assert_eq!(
            files[1].lines,
            vec![LineHit {
                line_no: 1,
                hits: 0
            }]
        );
        assert_eq!(
            files[2].path, "Stray.java",
            "an empty `<package/>` never becomes the enclosing package"
        );
        assert!(files[2].lines.is_empty());
    }

    #[test]
    fn jacoco_rejects_malformed_xml_and_bad_integers() {
        assert!(Jacoco
            .parse(r#"<report><package name="p"><sourcefile name="A.java"><line nr="1" mi="0" ci="1"/>"#)
            .is_err());
        for bad in [
            r#"<report><package name="p"><sourcefile name="A.java"><line nr="x" mi="0" ci="1"/></sourcefile></package></report>"#,
            r#"<report><package name="p"><sourcefile name="A.java"><line nr="1" mi="-1" ci="1"/></sourcefile></package></report>"#,
            r#"<report><package name="p"><sourcefile name="A.java"><line nr="1" mi="0" ci="1.5"/></sourcefile></package></report>"#,
        ] {
            assert!(Jacoco.parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn gocover_expands_blocks_per_line_keeping_the_max_and_skipping_empty_blocks() {
        let files = Gocover.parse(GOCOVER).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "github.com/acme/app/main.go",
                "github.com/acme/app/pkg/calc/calc.go",
                "github.com/acme/app-tools/gen/gen.go",
                "github.com/other/dep/x.go",
            ],
            "report order, verbatim import paths (module stripping is the caller's)"
        );
        let calc: Vec<(i64, i64)> = files[1].lines.iter().map(|l| (l.line_no, l.hits)).collect();
        assert_eq!(
            calc,
            [(3, 4), (4, 4), (5, 4), (6, 4), (7, 0), (8, 0), (12, 2), (13, 2)],
            "line 6 is in a count-4 and a count-0 block: max wins; the 0-stmt block adds no line 10"
        );
    }

    #[test]
    fn gocover_set_mode_and_a_repeated_block_do_not_double_count() {
        let text = "mode: set\na.go:1.1,1.9 1 1\na.go:1.1,1.9 1 0\na.go:2.1,2.9 1 0\n";
        let files = Gocover.parse(text).unwrap();
        assert_eq!(
            files[0].lines,
            vec![
                LineHit {
                    line_no: 1,
                    hits: 1
                },
                LineHit {
                    line_no: 2,
                    hits: 0
                }
            ]
        );
    }

    #[test]
    fn gocover_rejects_a_corrupt_profile() {
        for bad in [
            "",
            "main.go:1.1,2.1 1 1\n",
            "mode: set\nmain.go\n",
            "mode: set\nmain.go:1.1,2.1 1\n",
            "mode: set\nmain.go:1.1,2.1 1 1 extra\n",
            "mode: set\nmain.go:1.1-2.1 1 1\n",
            "mode: set\nmain.go:1,2 1 1\n",
            "mode: set\nmain.go:1.1,2.x 1 1\n",
            "mode: set\nmain.go:0.1,2.1 1 1\n",
            "mode: set\nmain.go:3.1,2.1 1 1\n",
            "mode: set\nmain.go:1.1,2.1 -1 1\n",
            "mode: set\n:1.1,2.1 1 1\n",
            "mode: count\nmain.go:1.1,6000000.1 1 1\n",
        ] {
            assert!(Gocover.parse(bad).is_err(), "{bad:?}");
        }
        // A header and nothing else is a valid, empty profile.
        assert!(Gocover.parse("mode: set\n").unwrap().is_empty());
    }

    /// Two counters at the integer limit are a line with instructions, not an
    /// arithmetic overflow ([FR-CV-12]): the parse neither panics nor drops it.
    #[test]
    fn jacoco_counters_at_the_integer_limit_do_not_overflow() {
        let text = format!(
            r#"<report><sessioninfo id="a"/><package name="p"><sourcefile name="A.java"><line nr="1" mi="{max}" ci="{max}"/></sourcefile></package></report>"#,
            max = i64::MAX
        );
        let files = Jacoco.parse(&text).unwrap();
        assert_eq!(
            files[0].lines,
            vec![LineHit {
                line_no: 1,
                hits: i64::MAX
            }]
        );
    }

    /// The expansion cap bounds the REPORT, not each block: many blocks that are
    /// each under it but together over it are rejected, a profile of exactly the
    /// cap is accepted and one line more is not.
    #[test]
    fn gocover_expansion_cap_is_cumulative_and_exact() {
        let cap = GOCOVER_MAX_EXPANDED_LINES as i64;
        let half = cap / 2;
        let two_halves_plus_one = format!(
            "mode: set\na.go:1.1,{half}.1 1 1\nb.go:1.1,{}.1 1 1\n",
            half + 1
        );
        assert!(
            Gocover.parse(&two_halves_plus_one).is_err(),
            "two blocks that together pass the cap are rejected"
        );
        let exact = format!("mode: set\na.go:1.1,{half}.1 1 1\nb.go:1.1,{half}.1 1 1\n");
        assert_eq!(
            Gocover.parse(&exact).unwrap().len(),
            2,
            "a profile of exactly the cap is accepted"
        );
    }
}
