//! The ratchet's blessed set ([FR-GV-23], [ADR-73]): `.logos/rules-baseline.json`.
//!
//! A `[constraints]` budget declared `{ max = N, ratchet = true }` tolerates the
//! offenders recorded here and fails only on new or worsened ones. This module
//! owns the file and the classification; the evaluator in [`super`] produces the
//! offenders (each [`Violation`] of a ratchet budget carries a [`RatchetMark`]
//! naming its symbol and value) and calls [`classify`] on them.
//!
//! **Who writes the file.** Only the bless command ([`BLESS_COMMAND`]) calls
//! [`BlessedSet::write`]. `check`, `gate` and `scan` read it at most, so the
//! blessed set changes only through a reviewed commit ([BR-62]). Unlike the gate
//! baseline it is **checked in**: CI must see it, and blessing new debt has to
//! show up in code review ([ADR-73]), so the generated `.logos/.gitignore` leaves
//! it tracked ([FR-IN-04]).
//!
//! **Determinism.** The file is one JSON document whose entries are sorted by
//! constraint, then symbol id, rendered with a fixed layout and a trailing
//! newline, so re-blessing an unchanged tree is byte-identical ([NFR-RA-06]).
//!
//! **Identity** is the symbol id without its repository coordinate — the
//! descriptor tail, so the checked-in file matches in a fork or a renamed clone
//! — or the file path for `no_god_files`, the module key for
//! `max_fan_in`/`max_fan_out`. A rename or move reads as one fixed entry plus
//! one new offender, which this version accepts ([FR-GV-23]).
//!
//! [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
//! [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
//! [ADR-73]: ../../../docs/specs/architecture/decisions/ADR-73.md
//! [BR-62]: ../../../docs/specs/software-spec.md#318-configuration
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::models::quality::{
    RatchetFixed, RatchetMark, RatchetReport, RatchetStatus, Violation,
};

/// The blessed set's file name inside `.logos/`.
pub const BLESSED_FILE: &str = "rules-baseline.json";

/// The blessed set's root-relative path, as every report and message names it.
pub const BLESSED_FILE_REL: &str = ".logos/rules-baseline.json";

/// The command that writes the blessed set — the only writer ([BR-62]).
///
/// [BR-62]: ../../../docs/specs/software-spec.md#318-configuration
pub const BLESS_COMMAND: &str = "logos check --bless";

/// The file format version. A file carrying any other version is refused.
const FORMAT_VERSION: u32 = 1;

/// Where the blessed set lives under `root`.
pub(crate) fn blessed_path(root: &Path) -> PathBuf {
    root.join(".logos").join(BLESSED_FILE)
}

/// The on-disk document. `deny_unknown_fields` so a hand-edited typo fails loud.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlessedFile {
    version: u32,
    entries: Vec<BlessedEntry>,
}

/// One blessed offender: constraint × symbol id → blessed value.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlessedEntry {
    rule: String,
    symbol: String,
    value: u64,
}

/// The blessed set, keyed by `(constraint, symbol id)`. A `BTreeMap`, so every
/// iteration — the rendered file and the fixed list alike — is sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BlessedSet(BTreeMap<(String, String), u64>);

impl BlessedSet {
    /// Read the blessed set at `path`: `Ok(None)` when the file does not exist,
    /// an exit-2 [`ConfigError`] when it is unreadable, malformed, of another
    /// format version, or names one offender twice.
    pub(crate) fn read(path: &Path) -> Result<Option<Self>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                }
                .into())
            }
        };
        Self::parse(&text).map(Some)
    }

    /// The blessed set a file's `text` holds, refused as [`read`](Self::read)
    /// refuses a file: malformed, of another format version, or naming one
    /// offender twice. The text is a file read from the working tree, or one
    /// read from a commit (`logos review --headroom`).
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let invalid = |message: String| ConfigError::InvalidValue {
            key: BLESSED_FILE_REL.to_string(),
            message,
        };
        let file: BlessedFile = serde_json::from_str(text)
            .map_err(|e| invalid(format!("{e} — re-run `{BLESS_COMMAND}` to rewrite it")))?;
        if file.version != FORMAT_VERSION {
            return Err(invalid(format!(
                "format version {} is not the supported version {FORMAT_VERSION}",
                file.version
            ))
            .into());
        }
        let mut set = BTreeMap::new();
        for entry in file.entries {
            let key = (entry.rule, entry.symbol);
            if set.contains_key(&key) {
                return Err(invalid(format!(
                    "`{}` × `{}` is blessed twice — one entry per constraint and symbol",
                    key.0, key.1
                ))
                .into());
            }
            set.insert(key, entry.value);
        }
        Ok(Self(set))
    }

    /// Every blessed entry as `(constraint, symbol id, blessed value)`, sorted.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &str, u64)> {
        self.0.iter().map(|((rule, symbol), value)| (rule.as_str(), symbol.as_str(), *value))
    }

    /// The set the current offenders bless: every marked violation of a rule in
    /// `rules`, at its current value. Two offenders sharing an identity keep the
    /// larger value, so the result never depends on violation order.
    pub(crate) fn from_violations(violations: &[Violation], rules: &[&str]) -> Self {
        let mut set: BTreeMap<(String, String), u64> = BTreeMap::new();
        for v in violations {
            let Some(mark) = &v.ratchet else { continue };
            if !rules.contains(&v.rule.as_str()) {
                continue;
            }
            let slot = set
                .entry((v.rule.clone(), mark.symbol.clone()))
                .or_insert(mark.value);
            *slot = (*slot).max(mark.value);
        }
        Self(set)
    }

    /// This set re-blessed for `rules`: every entry of a rule in `rules` is
    /// replaced by `current`'s, and every other rule's entry is kept. Blessing
    /// under an alternate contract (`check --bless --rules FILE`) therefore
    /// never drops the debt another contract blessed in the same file.
    pub(crate) fn reblessed(self, current: BlessedSet, rules: &[&str]) -> BlessedSet {
        let mut kept: BTreeMap<(String, String), u64> = self
            .0
            .into_iter()
            .filter(|((rule, _), _)| !rules.contains(&rule.as_str()))
            .collect();
        kept.extend(current.0);
        BlessedSet(kept)
    }

    /// The number of blessed entries.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// The blessed value of `rule` × `symbol`, if blessed.
    fn blessed_value(&self, rule: &str, symbol: &str) -> Option<u64> {
        self.0.get(&(rule.to_string(), symbol.to_string())).copied()
    }

    /// The canonical rendering: sorted entries, two-space indent, trailing
    /// newline — byte-identical for an equal set.
    pub(crate) fn render(&self) -> String {
        let file = BlessedFile {
            version: FORMAT_VERSION,
            entries: self
                .0
                .iter()
                .map(|((rule, symbol), value)| BlessedEntry {
                    rule: rule.clone(),
                    symbol: symbol.clone(),
                    value: *value,
                })
                .collect(),
        };
        let mut text =
            serde_json::to_string_pretty(&file).expect("a string/integer document serialises");
        text.push('\n');
        text
    }

    /// Write the canonical rendering to `path` atomically (write-temp-then-
    /// rename). An unchanged file is left untouched, mtime included. Called by
    /// the bless command only.
    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        let text = self.render();
        if std::fs::read(path).is_ok_and(|current| current == text.as_bytes()) {
            return Ok(());
        }
        let write_err = |source: std::io::Error| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(write_err)?;
        }
        crate::config::atomic::publish(path, text.as_bytes(), None).map_err(write_err)?;
        Ok(())
    }
}

/// A ratchet mark for an offender of a budget, or `None` when the budget is not
/// under ratchet — so a non-ratchet violation serialises exactly as before.
/// The status starts as [`RatchetStatus::New`], which is what the offender is
/// against an empty blessed set; [`classify`] settles it.
pub(crate) fn mark(ratchet: bool, symbol: impl Into<String>, value: u64) -> Option<RatchetMark> {
    ratchet.then(|| RatchetMark {
        symbol: symbol.into(),
        value,
        blessed: None,
        status: RatchetStatus::New,
    })
}

/// Classify every marked violation against `blessed` ([FR-GV-23]).
///
/// - Blessed at or below its blessed value → `warning` ("blessed debt").
/// - Blessed but above its blessed value → `error` ("worsened").
/// - Not blessed → `error`; with **no** blessed file the message names
///   [`BLESS_COMMAND`].
///
/// Blessed entries of a rule in `rules` that no current violation names are
/// reported as fixed, sorted by rule then symbol. Violations without a mark
/// pass through unchanged and in order, so classification never reorders the
/// report ([NFR-RA-06]). Returns `None` for the report when `rules` is empty —
/// no constraint is under ratchet.
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn classify(
    violations: Vec<Violation>,
    rules: &[&str],
    blessed: Option<&BlessedSet>,
) -> (Vec<Violation>, Option<RatchetReport>) {
    if rules.is_empty() {
        return (violations, None);
    }
    let empty = BlessedSet::default();
    let set: &BlessedSet = blessed.unwrap_or(&empty);
    let mut report = RatchetReport {
        blessed_file: BLESSED_FILE_REL.to_string(),
        blessed_file_present: blessed.is_some(),
        bless_command: BLESS_COMMAND.to_string(),
        rules: rules.iter().map(ToString::to_string).collect(),
        ..RatchetReport::default()
    };
    let mut offending: BTreeSet<(String, String)> = BTreeSet::new();

    let classified = violations
        .into_iter()
        .map(|mut v| {
            let Some(mark) = v.ratchet.as_mut() else {
                return v;
            };
            offending.insert((v.rule.clone(), mark.symbol.clone()));
            mark.blessed = set.blessed_value(&v.rule, &mark.symbol);
            let (status, severity, suffix) = match mark.blessed {
                Some(b) if mark.value <= b => {
                    report.blessed += 1;
                    (RatchetStatus::Blessed, "warning", format!("blessed debt (blessed at {b})"))
                }
                Some(b) => {
                    report.worsened += 1;
                    (RatchetStatus::Worsened, "error", format!("worse than its blessed {b}"))
                }
                None if blessed.is_some() => {
                    report.new += 1;
                    (RatchetStatus::New, "error", format!("new: not in {BLESSED_FILE_REL}"))
                }
                None => {
                    report.new += 1;
                    (
                        RatchetStatus::New,
                        "error",
                        format!(
                            "no blessed set ({BLESSED_FILE_REL}): run `{BLESS_COMMAND}` to \
                             accept the current debt"
                        ),
                    )
                }
            };
            mark.status = status;
            v.severity = severity.to_string();
            v.message = format!("{} — {suffix}", v.message);
            v
        })
        .collect();

    report.fixed = set
        .0
        .iter()
        .filter(|((rule, _), _)| rules.contains(&rule.as_str()))
        .filter(|(key, _)| !offending.contains(*key))
        .map(|((rule, symbol), value)| RatchetFixed {
            rule: rule.clone(),
            symbol: symbol.clone(),
            blessed: *value,
        })
        .collect();
    (classified, Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(rule: &str, symbol: &str, value: u64) -> Violation {
        Violation {
            rule: rule.to_string(),
            rule_type: "constraint".to_string(),
            severity: "error".to_string(),
            message: format!("{symbol} = {value}"),
            ratchet: mark(true, symbol, value),
            ..Violation::default()
        }
    }

    #[test]
    fn render_is_sorted_and_round_trips() {
        let set = BlessedSet::from_violations(
            &[
                marked("max_dead", "b", 1),
                marked("max_cc", "z", 22),
                marked("max_cc", "a", 30),
            ],
            &["max_cc", "max_dead"],
        );
        let text = set.render();
        let a = text.find("\"a\"").expect("a rendered");
        let z = text.find("\"z\"").expect("z rendered");
        let b = text.find("\"b\"").expect("b rendered");
        assert!(a < z && z < b, "sorted by rule then symbol:\n{text}");
        assert!(text.ends_with("}\n"), "trailing newline");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BLESSED_FILE);
        std::fs::write(&path, &text).unwrap();
        assert_eq!(BlessedSet::read(&path).unwrap(), Some(set));
    }

    #[test]
    fn from_violations_keeps_only_ratchet_rules_and_marked_violations() {
        let unmarked = Violation {
            rule: "max_cc".to_string(),
            ..Violation::default()
        };
        let set = BlessedSet::from_violations(
            &[unmarked, marked("max_cc", "f", 22), marked("max_fn_lines", "f", 90)],
            &["max_cc"],
        );
        assert_eq!(set.len(), 1);
        assert_eq!(set.blessed_value("max_cc", "f"), Some(22));
    }

    #[test]
    fn an_absent_file_reads_as_none_and_a_malformed_one_fails_loud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(BLESSED_FILE);
        assert_eq!(BlessedSet::read(&path).unwrap(), None);

        std::fs::write(&path, "{ not json").unwrap();
        let err = BlessedSet::read(&path).unwrap_err();
        assert!(err.downcast_ref::<ConfigError>().is_some(), "exit-2 class: {err}");

        std::fs::write(&path, r#"{"version":2,"entries":[]}"#).unwrap();
        let err = BlessedSet::read(&path).unwrap_err().to_string();
        assert!(err.contains("format version 2"), "{err}");

        std::fs::write(
            &path,
            r#"{"version":1,"entries":[{"rule":"max_cc","symbol":"f","value":1},{"rule":"max_cc","symbol":"f","value":2}]}"#,
        )
        .unwrap();
        let err = BlessedSet::read(&path).unwrap_err().to_string();
        assert!(err.contains("blessed twice"), "{err}");

        // An unreadable path is an I/O error, never a silent "no blessed set".
        let dir = tempfile::tempdir().unwrap();
        let err = BlessedSet::read(dir.path()).unwrap_err();
        assert!(
            matches!(err.downcast_ref::<ConfigError>(), Some(ConfigError::Io { .. })),
            "{err}"
        );
    }

    #[test]
    fn reblessing_replaces_only_the_evaluated_rules_entries() {
        let before = BlessedSet::from_violations(
            &[marked("max_cc", "f", 22), marked("max_fn_lines", "f", 90)],
            &["max_cc", "max_fn_lines"],
        );
        // Re-blessed under a contract that ratchets max_fn_lines only.
        let after = before.reblessed(
            BlessedSet::from_violations(&[marked("max_fn_lines", "g", 95)], &["max_fn_lines"]),
            &["max_fn_lines"],
        );
        assert_eq!(after.blessed_value("max_cc", "f"), Some(22), "another rule's debt is kept");
        assert_eq!(after.blessed_value("max_fn_lines", "f"), None, "the evaluated rule is replaced");
        assert_eq!(after.blessed_value("max_fn_lines", "g"), Some(95));
        assert_eq!(after.len(), 2);
    }

    #[test]
    fn write_leaves_an_unchanged_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".logos").join(BLESSED_FILE);
        let set = BlessedSet::from_violations(&[marked("max_cc", "f", 22)], &["max_cc"]);
        set.write(&path).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        set.write(&path).unwrap();
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after, "an identical re-bless does not rewrite the file");
    }

    #[test]
    fn unmarked_violations_pass_through_and_no_ratchet_rule_means_no_report() {
        let v = Violation {
            rule: "layer-ordering".to_string(),
            severity: "error".to_string(),
            message: "m".to_string(),
            ..Violation::default()
        };
        let (out, report) = classify(vec![v.clone()], &[], None);
        assert_eq!(out, vec![v.clone()]);
        assert!(report.is_none());

        let (out, report) = classify(vec![v.clone()], &["max_cc"], None);
        assert_eq!(out, vec![v]);
        assert_eq!(report.expect("ratchet report").new, 0);
    }
}
