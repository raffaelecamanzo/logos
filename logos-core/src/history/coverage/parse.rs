//! Coverage-report parsing behind a small format trait ([FR-CV-01], [ADR-23]).
//!
//! Two formats ship in this increment — LCOV (line-oriented text) and Cobertura
//! (XML) — each an impl of [`CoverageParser`], so later formats (JaCoCo, Clover,
//! Go coverprofile) are purely additive ([CR-007] §3.3). Format is auto-detected
//! from content ([`detect_format`]) and overridable via `--format`.
//!
//! # Atomic rejection ([FR-CV-01], [NFR-RA-05])
//! A malformed or truncated report is a hard [`Err`] from [`parse`] — the caller
//! ([`super::ingest`]) parses **before** it opens the store, so a rejected report
//! never writes a partial snapshot: the store stays byte-identical. Branch
//! coverage (LCOV `BRDA`, Cobertura branch rates) is out of scope for v1 ([CR-007]
//! §3.3) and silently ignored; only line hits are read.
//!
//! [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
//! [CR-007]: ../../../../docs/requests/CR-007-coverage-ingestion.md

use anyhow::{bail, Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

/// The two coverage formats ingested in this increment ([FR-CV-01]). A `Serialize`
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
}

impl CoverageFormat {
    /// The lowercase wire token (the stored `format`, the `--format` value).
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageFormat::Lcov => "lcov",
            CoverageFormat::Cobertura => "cobertura",
        }
    }

    /// Parse a `--format` flag value, or `None` for an unrecognized token.
    pub fn from_flag(s: &str) -> Option<Self> {
        match s {
            "lcov" => Some(CoverageFormat::Lcov),
            "cobertura" => Some(CoverageFormat::Cobertura),
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

/// Auto-detect the report format from content ([FR-CV-01]): the Cobertura XML
/// root element vs. LCOV's line records. Returns `None` for an unrecognized
/// (non-coverage) file, which the caller turns into a loud rejection unless
/// `--format` forces a parser.
pub(crate) fn detect_format(text: &str) -> Option<CoverageFormat> {
    let trimmed = text.trim_start();
    // XML route: only a document carrying the Cobertura `<coverage>` root is
    // accepted — an arbitrary XML/HTML file is NOT a coverage report and must be
    // rejected, not silently parsed to zero files (FR-CV-01 "non-coverage file is
    // rejected").
    if trimmed.starts_with('<') {
        return text
            .contains("<coverage")
            .then_some(CoverageFormat::Cobertura);
    }
    // LCOV route: a tracefile is a sequence of records; `TN:` (test name) or
    // `SF:` (source file) is the unambiguous opener.
    let is_lcov = text
        .lines()
        .any(|l| l.starts_with("TN:") || l.starts_with("SF:"));
    is_lcov.then_some(CoverageFormat::Lcov)
}

/// Parse `text` with the parser for `format`. Keeping `Lcov`/`Cobertura` as
/// [`CoverageParser`] impls is what makes later formats additive ([FR-CV-01]); the
/// dispatch is a static `match` — no trait object is needed when the format is
/// known at the call site.
pub(crate) fn parse(format: CoverageFormat, text: &str) -> Result<Vec<ReportFile>> {
    match format {
        CoverageFormat::Lcov => Lcov.parse(text),
        CoverageFormat::Cobertura => Cobertura.parse(text),
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

/// The Cobertura XML parser ([FR-CV-01]).
///
/// Streams the document with `quick-xml`, tracking the current `<class filename>`
/// and collecting its `<line number hits>` children. Malformed XML (a truncated
/// or corrupt report) is a hard error from the reader — the atomic-rejection path.
struct Cobertura;

impl CoverageParser for Cobertura {
    fn parse(&self, text: &str) -> Result<Vec<ReportFile>> {
        let mut reader = Reader::from_str(text);
        let config = reader.config_mut();
        config.trim_text(true);
        // Reject a mismatched end tag (`<a></b>`) as a hard error.
        config.check_end_names = true;

        let mut files: Vec<ReportFile> = Vec::new();
        let mut current_path: Option<String> = None;
        // Track element nesting so a truncated document (unclosed tags at EOF) is
        // rejected — `check_end_names` alone does not catch premature EOF.
        let mut depth: i32 = 0;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf).with_context(|| {
                format!(
                    "malformed Cobertura XML at byte {}",
                    reader.buffer_position()
                )
            })? {
                Event::Eof => break,
                Event::Start(e) => {
                    depth += 1;
                    handle_open(&e, &mut files, &mut current_path)?;
                }
                Event::Empty(e) => handle_open(&e, &mut files, &mut current_path)?,
                Event::End(e) => {
                    depth -= 1;
                    if e.local_name().as_ref() == b"class" {
                        current_path = None;
                    }
                }
                _ => {}
            }
            buf.clear();
        }
        anyhow::ensure!(
            depth == 0,
            "malformed Cobertura XML: unclosed element(s) at end of document"
        );
        Ok(files)
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

/// Read a UTF-8 attribute value from a start/empty element, decoding XML entities
/// (`&amp;` in a path, etc.). `Ok(None)` when the attribute is absent.
fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Result<Option<String>> {
    for attr in e.attributes() {
        let attr = attr.context("malformed Cobertura attribute")?;
        if attr.key.local_name().as_ref() == key {
            // `unescape_value` decodes XML entities assuming UTF-8 (Cobertura is
            // always UTF-8 in practice; the `encoding` feature is off).
            let decoded = attr
                .unescape_value()
                .context("decoding Cobertura attribute value")?;
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
        assert_eq!(CoverageFormat::from_flag("jacoco"), None);
        assert_eq!(CoverageFormat::from_flag("LCOV"), None, "case-sensitive");
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
}
