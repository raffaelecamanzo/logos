//! One advisory on one package is one finding, whichever scanners reported it
//! ([FR-SC-29], [ADR-79] as amended by CR-252, [S-809]).
//!
//! The evidence stays stored per scanner snapshot; the **read** merges it, so
//! every surface built on [`super::read_latest`] or [`super::read_at`] — each
//! list and count, the `[security]` caps, ratchet and baseline, the branch
//! delta, the Security page and the Dashboard widget — sees one merged finding
//! and counts it once ([NFR-CC-01]).
//!
//! Two stored findings are one when they share all three of:
//! - the **lockfile** — their location, made project-relative as a path
//!   suppression matches it ([`super::project_location`]), so OSV-Scanner's
//!   absolute `file://` path and Trivy's relative one name one file;
//! - the **package name and version**, each as its scanner states it
//!   ([`super::parse`]'s reading rules) and compared in one spelling
//!   ([`comparable_name`], [`comparable_version`]): Trivy's Go `v2.11.0` is
//!   OSV-Scanner's `2.11.0`, and Dependency-Check's purl
//!   `pkg:maven/org.apache/log4j-core` is OSV-Scanner's `org.apache:log4j-core`;
//! - **an advisory id** among their alias sets — the rule id, plus every id
//!   the message lists as an alias ([`super::parse::message_aliases`]),
//!   compared ASCII case-insensitively.
//!
//! The relation is closed transitively: an advisory one scanner names by its
//! CVE and another by a GHSA that a third lists as that CVE's alias is one.
//! Two advisories on one package whose aliases do not meet stay two.
//!
//! A merged finding is its first member in read order whose message names its
//! package — what a reader with only the message, such as the dependency scope
//! of [`crate::security::lockfile`], reads it by; Dependency-Check's names none
//! and sorts before OSV-Scanner — else its first member, with:
//! - [`reporters`](SecurityFinding::reporters): every member, that one first,
//!   then the rest in read order;
//! - the highest severity any member gave, an `unrated` member never lowering
//!   a rated one ([`merged_severity`]);
//! - every member's CWEs, so each OWASP category one of them reads is its;
//! - `fresh` when any member is: a current scan still reports the advisory.
//!
//! Only a dependency scanner's finding that names a package and a location
//! takes part ([`super::parse::names_packages`]); every other finding is read
//! as it is stored. The package is the stored one (history.db migration 14),
//! else — for a row stored before it — the one its message names.
//!
//! [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
//! [NFR-CC-01]: ../../../../docs/specs/requirements/NFR-CC-01.md
//! [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
//! [S-809]: ../../../../docs/planning/journal.md#s-809-one-advisory-on-one-package-is-one-finding-whichever-scanners-reported-it

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use super::parse::{message_aliases, message_package, names_packages, PackageCoordinate};
use super::{project_location, Reporter, SecurityFinding, Severity};
use crate::history::FRESHNESS_FRESH;

/// `findings` in read order, each with the package its row stores, merged:
/// one finding per advisory on one lockfile, package and version
/// ([module docs](self)), at its first member's place. Pure apart from
/// `root`, which only makes a location project-relative.
pub(super) fn merge(findings: Vec<(SecurityFinding, Option<PackageCoordinate>)>, root: &Path) -> Vec<SecurityFinding> {
    // Each group: its members' read positions, and every advisory id one of
    // them is known by. A finding joins every group of its merge key its
    // aliases meet, and those groups become one — the earliest, so a group is
    // at its first member's place and the closure is transitive.
    let mut groups: Vec<(Vec<usize>, BTreeSet<String>)> = Vec::new();
    let mut open: HashMap<MergeKey, Vec<usize>> = HashMap::new();
    for (at, (finding, package)) in findings.iter().enumerate() {
        let Some((key, aliases)) = merge_key(finding, package.as_ref(), root) else {
            groups.push((vec![at], BTreeSet::new()));
            continue;
        };
        let of_key = open.entry(key).or_default();
        let (meeting, apart): (Vec<usize>, Vec<usize>) =
            of_key.iter().partition(|&&group| !groups[group].1.is_disjoint(&aliases));
        let into = meeting.first().copied().unwrap_or(groups.len());
        if into == groups.len() {
            groups.push((Vec::new(), BTreeSet::new()));
        }
        for &other in meeting.iter().skip(1) {
            let (members, ids) = std::mem::take(&mut groups[other]);
            groups[into].0.extend(members);
            groups[into].1.extend(ids);
        }
        groups[into].0.push(at);
        groups[into].1.extend(aliases);
        *of_key = apart;
        of_key.push(into);
        of_key.sort_unstable();
    }

    let mut findings: Vec<Option<SecurityFinding>> = findings.into_iter().map(|(finding, _)| Some(finding)).collect();
    groups
        .into_iter()
        .filter(|(members, _)| !members.is_empty())
        .map(|(mut members, _)| {
            members.sort_unstable();
            merged(members.into_iter().filter_map(|at| findings[at].take()).collect())
        })
        .collect()
}

/// One lockfile, package and version, each in its comparable spelling.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MergeKey {
    location: String,
    name: String,
    version: String,
}

/// `finding`'s merge key and alias set, or `None` when it takes no part: not
/// a dependency scanner's, or with no location or package.
fn merge_key(
    finding: &SecurityFinding,
    stored: Option<&PackageCoordinate>,
    root: &Path,
) -> Option<(MergeKey, BTreeSet<String>)> {
    if !names_packages(&finding.scanner_name) {
        return None;
    }
    let read;
    let package = match stored {
        Some(package) => package,
        None => {
            read = message_package(&finding.message)?;
            &read
        }
    };
    let location = finding.location()?;
    let location = project_location(root, &location).unwrap_or(location);
    let aliases = std::iter::once(finding.rule_id.clone())
        .chain(message_aliases(&finding.message))
        .map(|id| id.to_ascii_uppercase())
        .collect();
    Some((
        MergeKey { location, name: comparable_name(&package.name), version: comparable_version(&package.version) },
        aliases,
    ))
}

/// A package name in the one spelling every scanner's compares in: a purl's
/// `pkg:<type>/` taken off and its `%xx` escapes decoded (`%40vitest` is
/// `@vitest`), `:` read as `/` (Maven's `group:artifact` is the purl's
/// `group/artifact`), ASCII lower-case.
fn comparable_name(name: &str) -> String {
    let name = name.trim();
    let bare = match name.strip_prefix("pkg:") {
        Some(purl) => purl.split_once('/').map_or(purl, |(_, rest)| rest),
        None => name,
    };
    percent_decoded(bare).replace(':', "/").to_ascii_lowercase()
}

/// A version in the one spelling every scanner's compares in: a leading `v`
/// before a digit taken off (Trivy writes a Go module's `v2.11.0`,
/// OSV-Scanner `2.11.0`), ASCII lower-case.
fn comparable_version(version: &str) -> String {
    let version = version.trim();
    let bare = match version.strip_prefix(['v', 'V']) {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest,
        _ => version,
    };
    bare.to_ascii_lowercase()
}

/// `text` with each `%xx` escape decoded; a malformed escape, or one that
/// decodes to no UTF-8, is kept as written.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escape = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// The highest severity among `severities` that is rated, else `unrated`: an
/// `unrated` member — its scanner gave none — never lowers a rated one, even
/// one rated below where `unrated` sorts ([FR-SC-29]).
///
/// [FR-SC-29]: ../../../../docs/specs/requirements/FR-SC-29.md
pub(super) fn merged_severity(severities: impl IntoIterator<Item = Severity>) -> Severity {
    severities.into_iter().filter(|s| *s != Severity::Unrated).max().unwrap_or(Severity::Unrated)
}

/// One group's merged finding ([module docs](self)); a group of one is its
/// finding unchanged.
fn merged(mut group: Vec<SecurityFinding>) -> SecurityFinding {
    if group.len() == 1 {
        return group.swap_remove(0);
    }
    if let Some(at) = group.iter().position(|f| message_package(&f.message).is_some()) {
        let named = group.remove(at);
        group.insert(0, named);
    }
    let reporters: Vec<Reporter> = group
        .iter()
        .map(|f| Reporter {
            scanner_name: f.scanner_name.clone(),
            scanner_version: f.scanner_version.clone(),
            fingerprint: f.fingerprint.clone(),
            rule_id: f.rule_id.clone(),
            severity: f.severity,
            legacy_fingerprints: f.legacy_fingerprints.clone(),
            freshness: f.freshness,
        })
        .collect();
    let severity = merged_severity(group.iter().map(|f| f.severity));
    let cwes: BTreeSet<String> = group.iter().flat_map(|f| f.cwes.iter().cloned()).collect();
    let fresh = group.iter().any(|f| f.freshness == FRESHNESS_FRESH);
    let mut first = group.swap_remove(0);
    first.severity = severity;
    first.cwes = cwes.into_iter().collect();
    if fresh {
        first.freshness = FRESHNESS_FRESH;
    }
    first.reporters = reporters;
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each spelling a scanner gives one package compares alike, and the
    /// near misses one character off do not.
    #[test]
    fn one_package_compares_alike_in_every_scanners_spelling() {
        assert_eq!(comparable_name("pkg:npm/%40vitest/mocker"), comparable_name("@vitest/mocker"));
        assert_eq!(comparable_name("pkg:maven/org.apache.logging.log4j/log4j-core"), comparable_name("org.apache.logging.log4j:log4j-core"));
        assert_eq!(comparable_name("pkg:golang/github.com/xuri/excelize/v2"), comparable_name("github.com/xuri/excelize/v2"));
        assert_eq!(comparable_name("pkg:pypi/django"), comparable_name("Django"));
        assert_ne!(comparable_name("@vitest/mocker"), comparable_name("vitest/mocker"));
        assert_ne!(comparable_name("github.com/xuri/excelize/v2"), comparable_name("github.com/xuri/excelize"));
        assert_eq!(comparable_version("v2.11.0"), comparable_version("2.11.0"));
        assert_ne!(comparable_version("v2.11.0"), comparable_version("2.11.1"));
        assert_eq!(comparable_version("vendor"), "vendor", "a leading v before a letter is part of the version");
        assert_eq!(percent_decoded("a%2"), "a%2", "a truncated escape is kept as written");
    }

    /// An `unrated` member never lowers a rated one, even a rating `unrated`
    /// sorts above; with none rated the finding is `unrated`.
    #[test]
    fn an_unrated_member_never_lowers_a_rated_one() {
        assert_eq!(merged_severity([Severity::Unrated, Severity::Low]), Severity::Low);
        assert_eq!(merged_severity([Severity::Info, Severity::Unrated]), Severity::Info);
        assert_eq!(merged_severity([Severity::High, Severity::Critical, Severity::Unrated]), Severity::Critical);
        assert_eq!(merged_severity([Severity::Unrated, Severity::Unrated]), Severity::Unrated);
    }
}
