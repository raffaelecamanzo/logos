//! Security-report parsing behind a small format trait ([FR-SC-01], [ADR-79]).
//!
//! One format ships — SARIF 2.1.0, the output every registered scanner can
//! produce — as an impl of [`FindingParser`], so a later format (CycloneDX) is
//! purely additive, exactly as coverage's [`CoverageParser`] is ([FR-CV-01]).
//!
//! # Atomic rejection ([FR-SC-01])
//! The whole report is parsed and validated **before** the caller opens the
//! store. A malformed, truncated or non-SARIF input is an [`Err`] naming the
//! defect — the JSON position serde reports, or the SARIF path of the missing or
//! wrong field (`runs[0].results[3].message`) — and nothing is written.
//!
//! # What is never read ([FR-SC-03], [BR-67])
//! The serde model below is a deliberate *subset* of SARIF. It has no field for
//! `region.snippet`, `contextRegion`, `codeFlows`, `relatedLocations`, rule
//! descriptions or help text, so none of them can reach a [`ParsedFinding`], let
//! alone the store. Rule text is excluded on purpose: a real Trivy secret rule
//! carries the matched source line in its `fullDescription`. Fingerprint values
//! are hashed into the identity and never kept verbatim — Gitleaks' carry commit
//! authors and messages.
//!
//! A secret-class finding's message is replaced here, at parse time, by
//! [`super::redact`]; the parse output is safe by construction.
//!
//! [FR-SC-01]: ../../../../docs/specs/requirements/FR-SC-01.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [FR-CV-01]: ../../../../docs/specs/requirements/FR-CV-01.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
//! [BR-67]: ../../../../docs/specs/software-spec.md#328-security-findings
//! [`CoverageParser`]: crate::history::coverage

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;

use super::redact;
use super::Severity;

/// The security report formats ingested ([FR-SC-01]). A `Serialize` read-model
/// field so the ingest summary renders the detected format on every surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityFormat {
    /// SARIF 2.1.0 (`version` + `runs[]`).
    Sarif,
}

impl SecurityFormat {
    /// The lowercase wire token (the stored `format`).
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityFormat::Sarif => "sarif",
        }
    }
}

/// A parser for one security-report format ([FR-SC-01] "behind a
/// format-detecting trait").
pub(crate) trait FindingParser {
    /// Parse and validate the full report, or fail loud naming the defect.
    fn parse(&self, text: &str) -> Result<Vec<ParsedRun>>;
}

/// Auto-detect the report format from content. SARIF is the only JSON format
/// today; a later JSON format (CycloneDX's `bomFormat`) is told apart here before
/// SARIF claims the object. `None` for anything that is not a JSON object, which
/// the caller turns into a loud rejection.
pub(crate) fn detect_format(text: &str) -> Option<SecurityFormat> {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    trimmed.starts_with('{').then_some(SecurityFormat::Sarif)
}

/// Parse `text` with the parser for `format`.
pub(crate) fn parse(format: SecurityFormat, text: &str) -> Result<Vec<ParsedRun>> {
    match format {
        SecurityFormat::Sarif => Sarif.parse(text),
    }
}

/// One SARIF run reduced to what the store keeps: the scanner identity and its
/// findings, in report order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRun {
    /// `tool.driver.name`, verbatim.
    pub scanner_name: String,
    /// `tool.driver.semanticVersion`, else `version`, else `"unknown"`.
    pub scanner_version: String,
    /// The run's findings, in report order: every result except a `pass` or
    /// `notApplicable` one.
    pub findings: Vec<ParsedFinding>,
}

/// One result, before path binding. Carries no snippet and no rule text; its
/// `message` is already redacted when the finding is secret-class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFinding {
    /// The rule the result reports.
    pub rule_id: String,
    /// The normalised severity ([`Severity::from_sarif`]).
    pub severity: Severity,
    /// Sorted, de-duplicated `CWE-<n>` identifiers.
    pub cwes: Vec<String>,
    /// The rule message, or the fixed redacted text for a secret-class finding.
    pub message: String,
    /// `true` when `message` is the redacted text ([FR-SC-03]).
    pub redacted: bool,
    /// The primary location's path as the report named it (scheme stripped,
    /// percent-decoded); `None` when the result has no physical location.
    pub report_path: Option<String>,
    /// The primary location's region, as far as the report gave it.
    pub region: Region,
    /// The report's own identity for the result — `fingerprints`, else
    /// `partialFingerprints`, empty values dropped — as `key=value` pairs in key
    /// order. Hashed into the stored fingerprint, never stored itself.
    pub identity: Vec<String>,
}

/// A source region: 1-based lines and columns, each as given (or absent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    /// 1-based start line.
    pub start_line: Option<i64>,
    /// 1-based start column.
    pub start_column: Option<i64>,
    /// 1-based end line.
    pub end_line: Option<i64>,
    /// 1-based end column.
    pub end_column: Option<i64>,
}

// ── The SARIF subset (serde ignores every field not named here) ──────────────

#[derive(Deserialize)]
struct SarifLog {
    version: Option<String>,
    runs: Option<Vec<Run>>,
}

#[derive(Deserialize)]
struct Run {
    tool: Option<Tool>,
    results: Option<Vec<SarifResult>>,
    #[serde(default)]
    invocations: Vec<Invocation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Invocation {
    execution_successful: Option<bool>,
}

#[derive(Deserialize)]
struct Tool {
    driver: Option<ToolComponent>,
    #[serde(default)]
    extensions: Vec<ToolComponent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolComponent {
    name: Option<String>,
    version: Option<String>,
    semantic_version: Option<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    id: Option<String>,
    #[serde(default)]
    message_strings: BTreeMap<String, Message>,
    properties: Option<PropertyBag>,
    #[serde(default)]
    relationships: Vec<Relationship>,
    default_configuration: Option<ReportingConfiguration>,
}

#[derive(Deserialize)]
struct ReportingConfiguration {
    level: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Relationship {
    target: Option<DescriptorReference>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescriptorReference {
    id: Option<String>,
    tool_component: Option<ComponentReference>,
}

#[derive(Deserialize)]
struct ComponentReference {
    name: Option<String>,
    index: Option<usize>,
}

#[derive(Deserialize)]
struct PropertyBag {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "security-severity")]
    security_severity: Option<serde_json::Value>,
    cwe: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    rule_id: Option<String>,
    rule_index: Option<usize>,
    rule: Option<RuleReference>,
    kind: Option<String>,
    level: Option<String>,
    message: Option<Message>,
    #[serde(default)]
    locations: Vec<Location>,
    fingerprints: Option<BTreeMap<String, String>>,
    partial_fingerprints: Option<BTreeMap<String, String>>,
    properties: Option<PropertyBag>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuleReference {
    id: Option<String>,
    index: Option<usize>,
    tool_component: Option<ComponentReference>,
}

#[derive(Deserialize)]
struct Message {
    text: Option<String>,
    markdown: Option<String>,
    id: Option<String>,
    #[serde(default)]
    arguments: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    physical_location: Option<PhysicalLocation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    artifact_location: Option<ArtifactLocation>,
    region: Option<Region>,
}

#[derive(Deserialize)]
struct ArtifactLocation {
    uri: Option<String>,
}

/// The SARIF 2.1.0 parser.
struct Sarif;

impl FindingParser for Sarif {
    fn parse(&self, text: &str) -> Result<Vec<ParsedRun>> {
        let text = text.trim_start_matches('\u{feff}');
        let log: SarifLog = serde_json::from_str(text)
            .map_err(|err| anyhow!("not a well-formed SARIF report: {}", without_report_values(&err.to_string())))?;
        match log.version.as_deref() {
            Some("2.1.0") => {}
            Some(_) => bail!("SARIF `version` is unsupported; only \"2.1.0\" is supported"),
            None => bail!("not a SARIF report: the top-level `version` is missing"),
        }
        let runs = log
            .runs
            .ok_or_else(|| anyhow!("not a SARIF report: the top-level `runs` array is missing"))?;
        runs.into_iter()
            .enumerate()
            .map(|(i, run)| parse_run(i, run))
            .collect()
    }
}

/// Validate and reduce one run.
fn parse_run(i: usize, run: Run) -> Result<ParsedRun> {
    let tool = run
        .tool
        .ok_or_else(|| anyhow!("runs[{i}].tool is missing"))?;
    let driver = tool
        .driver
        .ok_or_else(|| anyhow!("runs[{i}].tool.driver is missing"))?;
    let scanner_name = driver
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| anyhow!("runs[{i}].tool.driver.name is missing"))?;
    let scanner_version = driver
        .semantic_version
        .clone()
        .or_else(|| driver.version.clone())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    // A scan that did not complete is no evidence, least of all of a clean tree:
    // stored, it would replace the scanner's real findings with "0 findings".
    // SARIF marks it two ways — a failed invocation, or no `results` at all (an
    // empty array is a completed scan that found nothing).
    if let Some(k) = run
        .invocations
        .iter()
        .position(|inv| inv.execution_successful == Some(false))
    {
        bail!("runs[{i}].invocations[{k}].executionSuccessful is false: the scan did not complete");
    }
    let results = run.results.ok_or_else(|| {
        anyhow!("runs[{i}].results is absent: the tool computed no results (an empty array is a clean scan)")
    })?;

    let rules = RuleTable::new(&driver, &tool.extensions);
    let findings = results
        .into_iter()
        .enumerate()
        .map(|(j, result)| parse_result(i, j, result, &rules, &scanner_name))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(ParsedRun {
        scanner_name,
        scanner_version,
        findings,
    })
}

/// Every rule a run declares — on the driver and on its extensions (CodeQL keeps
/// its query rules in `tool.extensions[].rules`, leaving `driver.rules` empty).
struct RuleTable<'a> {
    driver: &'a [Rule],
    extensions: Vec<&'a [Rule]>,
    by_id: HashMap<&'a str, &'a Rule>,
}

impl<'a> RuleTable<'a> {
    fn new(driver: &'a ToolComponent, extensions: &'a [ToolComponent]) -> Self {
        let mut by_id = HashMap::new();
        for rule in driver
            .rules
            .iter()
            .chain(extensions.iter().flat_map(|e| e.rules.iter()))
        {
            if let Some(id) = rule.id.as_deref() {
                by_id.entry(id).or_insert(rule);
            }
        }
        Self {
            driver: &driver.rules,
            extensions: extensions.iter().map(|e| e.rules.as_slice()).collect(),
            by_id,
        }
    }

    /// The rule a result names by index: `ruleIndex` into the driver, or
    /// `rule.index` into the component `rule.toolComponent.index` names (the
    /// driver when absent).
    fn by_index(&self, result: &SarifResult) -> Option<&'a Rule> {
        if let Some(r) = &result.rule {
            if let Some(index) = r.index {
                let rules = match r.tool_component.as_ref().and_then(|c| c.index) {
                    Some(component) => *self.extensions.get(component)?,
                    None => self.driver,
                };
                return rules.get(index);
            }
        }
        result.rule_index.and_then(|index| self.driver.get(index))
    }
}

/// Validate and reduce one result; `None` for a result that reports no finding
/// (`kind` `pass` or `notApplicable`).
fn parse_result(
    i: usize,
    j: usize,
    result: SarifResult,
    rules: &RuleTable<'_>,
    scanner_name: &str,
) -> Result<Option<ParsedFinding>> {
    let at = format!("runs[{i}].results[{j}]");
    // SARIF §3.27.9: `kind` defaults to `fail`. A passed or not-applicable check
    // is not a finding; any other non-`fail` kind is one, at level `none` unless
    // it states a level.
    let kind = result.kind.as_deref().unwrap_or("fail");
    match kind {
        "pass" | "notApplicable" => return Ok(None),
        "fail" | "open" | "review" | "informational" => {}
        _ => bail!(
            "{at}.kind is not one of \"pass\", \"fail\", \"open\", \"review\", \"informational\", \"notApplicable\""
        ),
    }
    let indexed_rule = rules.by_index(&result);
    // SARIF makes the rule optional: a result naming none is still a finding,
    // kept under a fixed id rather than rejected.
    let rule_id = result
        .rule_id
        .clone()
        .or_else(|| result.rule.as_ref().and_then(|r| r.id.clone()))
        .or_else(|| indexed_rule.and_then(|r| r.id.clone()))
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| NO_RULE.to_string());
    let rule = rules.by_id.get(rule_id.as_str()).copied().or(indexed_rule);

    let message = result
        .message
        .as_ref()
        .and_then(|m| resolve_message(m, rule))
        .ok_or_else(|| anyhow!("{at}.message has no `text`, `markdown` or `id`"))?;

    let level = match result.level.as_deref() {
        Some(level) => Some(check_level(level).map_err(|e| anyhow!("{at}.level {e}"))?),
        None if kind != "fail" => Some("none"),
        None => match rule
            .and_then(|r| r.default_configuration.as_ref())
            .and_then(|c| c.level.as_deref())
        {
            Some(level) => Some(check_level(level).map_err(|e| {
                anyhow!("rule {rule_id:?} defaultConfiguration.level {e}")
            })?),
            None => None,
        },
    };
    // `security-severity` lives in a tool-defined property bag, not in SARIF
    // itself: a value that is not a usable CVSS score is ignored and the level
    // decides, rather than rejecting a well-formed report.
    let score = result
        .properties
        .as_ref()
        .and_then(|p| p.security_severity.as_ref())
        .and_then(cvss)
        .or_else(|| {
            rule.and_then(|r| r.properties.as_ref())
                .and_then(|p| p.security_severity.as_ref())
                .and_then(cvss)
        });
    let severity = Severity::from_sarif(score, level);

    let tags: Vec<&str> = rule
        .and_then(|r| r.properties.as_ref())
        .into_iter()
        .chain(result.properties.as_ref())
        .flat_map(|p| p.tags.iter().map(String::as_str))
        .collect();
    let cwes = collect_cwes(rule, result.properties.as_ref(), &tags);

    let physical = result
        .locations
        .first()
        .and_then(|l| l.physical_location.as_ref());
    let report_path = physical
        .and_then(|p| p.artifact_location.as_ref())
        .and_then(|a| a.uri.as_deref())
        .map(uri_to_path)
        .filter(|p| !p.is_empty());
    let region = physical.and_then(|p| p.region).unwrap_or_default();

    let identity = identity_pairs(&result);

    let secret = redact::is_secret_class(scanner_name, &rule_id, &tags, &cwes);
    let (message, redacted) = if secret {
        (
            redact::redacted_message(&rule_id, report_path.as_deref(), region.start_line),
            true,
        )
    } else {
        // Every other message is stored masked ([BR-67]): a rule can echo a
        // matched value in an ordinary message too.
        (super::mask::mask_secrets(&message), false)
    };

    Ok(Some(ParsedFinding {
        rule_id,
        severity,
        cwes,
        message,
        redacted,
        report_path,
        region,
        identity,
    }))
}

/// A SARIF `level` must be one of the four the spec defines.
fn check_level(level: &str) -> std::result::Result<&str, String> {
    match level {
        "none" | "note" | "warning" | "error" => Ok(level),
        _ => Err("is not one of \"none\", \"note\", \"warning\", \"error\"".to_string()),
    }
}

/// A JSON error's text with every value it quotes from the report replaced by
/// `"…"`. serde quotes the offending value (`invalid type: string "…"`,
/// `unknown variant `…``), and a report's strings can hold a secret, while
/// this text reaches the CLI, MCP and the watcher's log ([FR-SC-03]). What
/// it was expected to be, and where, stay: that names the defect.
///
/// [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
fn without_report_values(message: &str) -> String {
    static QUOTED: OnceLock<Regex> = OnceLock::new();
    let quoted = QUOTED.get_or_init(|| Regex::new(r#""(?:[^"\\]|\\.)*"|`[^`]*`"#).expect("a valid pattern"));
    let (found, expected) = match message.find(", expected") {
        Some(at) => message.split_at(at),
        None => (message, ""),
    };
    format!("{}{expected}", quoted.replace_all(found, "\"…\""))
}

/// A `security-severity` value: a CVSS score as a string (the GitHub convention
/// CodeQL and Trivy follow) or a number, in `0.0..=10.0`; `None` for anything
/// else.
fn cvss(raw: &serde_json::Value) -> Option<f64> {
    let score = match raw {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        serde_json::Value::Number(n) => n.as_f64(),
        _ => None,
    };
    score.filter(|s| (0.0..=10.0).contains(s))
}

/// The rule id a result naming no rule is stored under.
pub(crate) const NO_RULE: &str = "(no rule)";

/// A result's message text: `text`, else `markdown`, else the rule's
/// `messageStrings[id]` with its `{n}` placeholders filled from `arguments`
/// (SARIF §3.11.7), else the bare `id`. `None` when the message carries none of
/// them.
fn resolve_message(message: &Message, rule: Option<&Rule>) -> Option<String> {
    if let Some(text) = message.text.clone().or_else(|| message.markdown.clone()) {
        return Some(text);
    }
    let id = message.id.as_deref()?;
    let template = rule
        .and_then(|r| r.message_strings.get(id))
        .and_then(|m| m.text.clone().or_else(|| m.markdown.clone()));
    Some(match template {
        Some(mut text) => {
            for (n, arg) in message.arguments.iter().enumerate() {
                text = text.replace(&format!("{{{n}}}"), arg);
            }
            text
        }
        None => id.to_string(),
    })
}

/// The CWE identifiers a finding carries, from every place scanners put them:
/// rule and result tags (`CWE-89: …` from Semgrep, `external/cwe/cwe-089` from
/// CodeQL and Bandit), a `cwe` property (string or list), and rule
/// relationships into a `CWE` taxonomy (gosec). Sorted numerically, deduped.
fn collect_cwes(rule: Option<&Rule>, result_props: Option<&PropertyBag>, tags: &[&str]) -> Vec<String> {
    // Every string that may name a CWE, then one pass of the pattern over all.
    let mut sources: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    for props in rule.and_then(|r| r.properties.as_ref()).into_iter().chain(result_props) {
        match &props.cwe {
            Some(serde_json::Value::String(s)) => sources.push(s.clone()),
            Some(serde_json::Value::Array(items)) => {
                sources.extend(items.iter().filter_map(|v| v.as_str()).map(str::to_string))
            }
            _ => {}
        }
    }
    for rel in rule.map(|r| r.relationships.as_slice()).unwrap_or_default() {
        let Some(target) = &rel.target else { continue };
        let taxonomy = target
            .tool_component
            .as_ref()
            .and_then(|c| c.name.as_deref());
        if !taxonomy.is_some_and(|n| n.eq_ignore_ascii_case("cwe")) {
            continue;
        }
        if let Some(id) = target.id.as_deref() {
            // A taxonomy id is the bare number (gosec: `"798"`) or a full token.
            let id = id.trim();
            if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                sources.push(format!("CWE-{id}"));
            } else {
                sources.push(id.to_string());
            }
        }
    }

    let found: BTreeSet<u32> = sources
        .iter()
        .flat_map(|s| cwe_pattern().captures_iter(s).filter_map(|c| c[1].parse::<u32>().ok()))
        .filter(|&n| n > 0)
        .collect();
    found.into_iter().map(|n| format!("CWE-{n}")).collect()
}

/// `CWE-89`, `cwe-089`, `external/cwe/cwe-89`, `CWE 89` — a `cwe` token
/// followed by its number. Word-bounded so `subcwe-1` does not match.
fn cwe_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)\bcwe[-_: ]?0*([0-9]{1,6})\b").expect("valid CWE pattern"))
}

/// The report's identity pairs: `fingerprints` when it carries a non-empty value,
/// else `partialFingerprints`; empty values dropped (Gitleaks emits empty commit
/// fields outside a git scan).
fn identity_pairs(result: &SarifResult) -> Vec<String> {
    let pairs = |map: &Option<BTreeMap<String, String>>| -> Vec<String> {
        map.iter()
            .flatten()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    };
    let full = pairs(&result.fingerprints);
    if full.is_empty() {
        pairs(&result.partial_fingerprints)
    } else {
        full
    }
}

/// A SARIF artifact URI as a path: a `file://` scheme stripped (keeping the
/// absolute path) and `%XX` escapes decoded. Relative URIs pass through; the
/// suffix binder needs no `uriBaseId` resolution.
fn uri_to_path(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    percent_decode(rest)
}

/// Decode `%XX` escapes; a malformed escape is kept literally. Decoded bytes that
/// are not UTF-8 are replaced, never dropped.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut k = 0;
    while k < bytes.len() {
        if bytes[k] == b'%' && k + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[k + 1..k + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                k += 3;
                continue;
            }
        }
        out.push(bytes[k]);
        k += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> ParsedFinding {
        let mut runs = Sarif.parse(text).expect("parses");
        runs.remove(0).findings.remove(0)
    }

    fn log(result: &str, rules: &str) -> String {
        format!(
            r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"t","rules":[{rules}]}}}},"results":[{result}]}}]}}"#
        )
    }

    #[test]
    fn cwe_tokens_are_read_in_every_scanner_spelling_and_a_near_miss_is_not() {
        let found = |s: &str| {
            cwe_pattern()
                .captures_iter(s)
                .map(|c| c[1].to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(found("CWE-89: Improper Neutralization"), ["89"]);
        assert_eq!(found("external/cwe/cwe-089"), ["89"]);
        assert_eq!(found("cwe_22"), ["22"]);
        assert!(found("subcwe-12").is_empty(), "a word-internal `cwe` is not a CWE");
        assert!(found("OWASP-A03:2021 - Injection").is_empty());
    }

    #[test]
    fn severity_prefers_the_cvss_score_then_the_level() {
        let f = one(&log(
            r#"{"ruleId":"r","level":"note","message":{"text":"m"},"properties":{"security-severity":"9.1"}}"#,
            "",
        ));
        assert_eq!(f.severity, Severity::Critical, "the score outranks the level");
        let f = one(&log(r#"{"ruleId":"r","message":{"text":"m"}}"#, r#"{"id":"r","defaultConfiguration":{"level":"error"}}"#));
        assert_eq!(f.severity, Severity::High, "the rule's default level stands in");
        let f = one(&log(r#"{"ruleId":"r","message":{"text":"m"}}"#, ""));
        assert_eq!(f.severity, Severity::Medium, "SARIF's default level is warning");
    }

    /// `kind`: a passed or not-applicable check is no finding; an `open` result
    /// without a level is `none` (info), not SARIF's `warning` default; an
    /// unknown kind is a named defect.
    #[test]
    fn kind_decides_whether_a_result_is_a_finding() {
        let text = log(
            r#"{"ruleId":"r","kind":"pass","message":{"text":"check passed"}},
               {"ruleId":"r","kind":"notApplicable","message":{"text":"not applicable"}},
               {"ruleId":"r","kind":"open","message":{"text":"needs a look"}},
               {"ruleId":"r","message":{"text":"failed"}}"#,
            r#"{"id":"r","defaultConfiguration":{"level":"error"}}"#,
        );
        let findings = Sarif.parse(&text).unwrap().remove(0).findings;
        let got: Vec<(&str, Severity)> = findings.iter().map(|f| (f.message.as_str(), f.severity)).collect();
        assert_eq!(got, [("needs a look", Severity::Info), ("failed", Severity::High)]);
        let err = Sarif
            .parse(&log(r#"{"ruleId":"r","kind":"broken","message":{"text":"m"}}"#, ""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("runs[0].results[0].kind") && !err.contains("broken"), "named, never quoted: {err}");
    }

    /// [BR-67]: a message that is not secret-class is still stored masked, so a
    /// rule echoing a matched value persists no secret; a plain message is kept
    /// as written, and neither is flagged redacted.
    #[test]
    fn every_message_is_masked_before_the_write() {
        let text = log(
            r#"{"ruleId":"r","message":{"text":"query built with password=hunter2 and AKIAQWERTYUIOPASDFGH"}},
               {"ruleId":"r","message":{"text":"Possible SQL injection via string concatenation."}}"#,
            "",
        );
        let findings = Sarif.parse(&text).unwrap().remove(0).findings;
        let echoed = &findings[0];
        assert!(!echoed.message.contains("hunter2") && !echoed.message.contains("AKIAQWERTYUIOPASDFGH"), "{}", echoed.message);
        assert!(echoed.message.starts_with("query built with password=") && echoed.message.contains(super::super::mask::MASK), "{}", echoed.message);
        assert!(!echoed.redacted, "masked, not replaced as secret-class");
        assert_eq!(findings[1].message, "Possible SQL injection via string concatenation.");
        assert!(!findings[1].redacted);
    }

    #[test]
    fn a_bad_level_names_the_field() {
        let err = Sarif
            .parse(&log(r#"{"ruleId":"r","level":"fatal","message":{"text":"m"}}"#, ""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("runs[0].results[0].level") && !err.contains("fatal"), "named, never quoted: {err}");
    }

    /// `security-severity` is a tool-defined property: a value that is no CVSS
    /// score — a word, or a number outside 0..=10 — is ignored and the level
    /// decides; a numeric score is read like a string one.
    #[test]
    fn an_unusable_score_falls_back_to_the_level() {
        for bad in [r#""high""#, r#""11""#, r#""-1""#, "true"] {
            let f = one(&log(
                &format!(r#"{{"ruleId":"r","level":"note","message":{{"text":"m"}},"properties":{{"security-severity":{bad}}}}}"#),
                "",
            ));
            assert_eq!(f.severity, Severity::Low, "{bad} is ignored; `note` decides");
        }
        let f = one(&log(
            r#"{"ruleId":"r","level":"note","message":{"text":"m"}}"#,
            r#"{"id":"r","properties":{"security-severity":9.5}}"#,
        ));
        assert_eq!(f.severity, Severity::Critical, "a numeric score on the rule");
    }

    /// SARIF allows a message by reference — `id` into the rule's
    /// `messageStrings`, with `{n}` arguments — and a result naming no rule.
    #[test]
    fn a_message_by_id_and_a_rule_less_result_are_kept() {
        let f = one(&log(
            r#"{"ruleId":"BA2008","message":{"id":"Error","arguments":["app.dll","CFG"]}}"#,
            r#"{"id":"BA2008","messageStrings":{"Error":{"text":"'{0}' does not enable {1}."}}}"#,
        ));
        assert_eq!(f.message, "'app.dll' does not enable CFG.");
        let f = one(&log(r#"{"ruleId":"r","message":{"id":"Missing"}}"#, ""));
        assert_eq!(f.message, "Missing", "an unresolvable id stands for itself");
        let f = one(&log(r#"{"ruleId":"r","message":{"markdown":"**m**"}}"#, ""));
        assert_eq!(f.message, "**m**");
        let f = one(&log(r#"{"message":{"text":"m"}}"#, ""));
        assert_eq!(f.rule_id, NO_RULE);
    }

    #[test]
    fn a_rule_is_resolved_by_index_into_an_extension() {
        let text = r#"{"version":"2.1.0","runs":[{"tool":{
            "driver":{"name":"CodeQL","rules":[]},
            "extensions":[{"name":"pack","rules":[{"id":"py/x","properties":{"tags":["external/cwe/cwe-079"]}}]}]},
            "results":[{"rule":{"index":0,"toolComponent":{"index":0}},"message":{"text":"m"}}]}]}"#;
        let f = one(text);
        assert_eq!(f.rule_id, "py/x");
        assert_eq!(f.cwes, ["CWE-79"]);
    }

    /// A rule found by `ruleIndex` into the driver gives the result its id and
    /// its CWE tags.
    #[test]
    fn a_rule_is_resolved_by_index_into_the_driver() {
        let f = one(&log(
            r#"{"ruleIndex":1,"message":{"text":"m"}}"#,
            r#"{"id":"first"},{"id":"second","properties":{"tags":["CWE-22"]}}"#,
        ));
        assert_eq!((f.rule_id.as_str(), f.cwes.as_slice()), ("second", &["CWE-22".to_string()][..]));
    }

    /// A `secret` tag on the **result** makes it secret-class even under an
    /// untagged rule from a non-secret scanner: its message is redacted.
    #[test]
    fn a_result_level_secret_tag_redacts_the_message() {
        let f = one(&log(
            r#"{"ruleId":"r","message":{"text":"token=hunter2"},"properties":{"tags":["secret"]},
                "locations":[{"physicalLocation":{"artifactLocation":{"uri":"a.py"},"region":{"startLine":2}}}]}"#,
            r#"{"id":"r"}"#,
        ));
        assert!(f.redacted);
        assert_eq!(f.message, "secret-class finding redacted: rule r at a.py:2");
        let f = one(&log(r#"{"ruleId":"r","message":{"text":"plain"},"properties":{"tags":["style"]}}"#, r#"{"id":"r"}"#));
        assert!(!f.redacted && f.message == "plain", "a non-secret result tag changes nothing");
    }

    /// A result naming its rule only by `ruleId`, with the rule declared on an
    /// extension rather than the driver, still reads that rule's CWE tags.
    #[test]
    fn a_rule_named_by_id_is_found_on_an_extension() {
        let text = r#"{"version":"2.1.0","runs":[{"tool":{
            "driver":{"name":"CodeQL","rules":[]},
            "extensions":[{"name":"pack","rules":[{"id":"py/x","properties":{"tags":["external/cwe/cwe-079"]}}]}]},
            "results":[{"ruleId":"py/x","message":{"text":"m"}}]}]}"#;
        assert_eq!(one(text).cwes, ["CWE-79"]);
    }

    #[test]
    fn a_result_without_a_message_is_rejected_by_path() {
        let err = Sarif.parse(&log(r#"{"ruleId":"r"}"#, "")).unwrap_err().to_string();
        assert!(err.contains("runs[0].results[0].message"), "{err}");
        let err = Sarif
            .parse(&log(r#"{"ruleId":"r","message":{"arguments":["x"]}}"#, ""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("has no `text`, `markdown` or `id`"), "{err}");
    }

    #[test]
    fn identity_prefers_full_fingerprints_and_drops_empty_values() {
        let f = one(&log(
            r#"{"ruleId":"r","message":{"text":"m"},"fingerprints":{"a/v1":"x"},"partialFingerprints":{"p":"y"}}"#,
            "",
        ));
        assert_eq!(f.identity, ["a/v1=x"]);
        let f = one(&log(
            r#"{"ruleId":"r","message":{"text":"m"},"partialFingerprints":{"commitSha":"","email":"e"}}"#,
            "",
        ));
        assert_eq!(f.identity, ["email=e"]);
        let f = one(&log(
            r#"{"ruleId":"r","message":{"text":"m"},"partialFingerprints":{"commitSha":""}}"#,
            "",
        ));
        assert!(f.identity.is_empty(), "an all-empty map is no identity");
    }

    #[test]
    fn uris_lose_their_scheme_and_escapes() {
        assert_eq!(uri_to_path("file:///home/ci/p/my%20app/x.py"), "/home/ci/p/my app/x.py");
        assert_eq!(uri_to_path("src/a.py"), "src/a.py");
        assert_eq!(uri_to_path("a%2"), "a%2", "a truncated escape is kept literally");
        assert_eq!(uri_to_path("a%zz"), "a%zz", "a non-hex escape is kept literally");
    }

    #[test]
    fn detection_takes_a_json_object_only() {
        assert_eq!(detect_format("\u{feff}  {\"version\":"), Some(SecurityFormat::Sarif));
        assert_eq!(detect_format("TN:\nSF:a"), None);
        assert_eq!(detect_format("<coverage/>"), None);
        assert_eq!(detect_format(""), None);
    }
}
