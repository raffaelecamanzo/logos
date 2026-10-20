//! The Security page's read-model ([FR-SC-09], [S-670]): the scanner matrix,
//! the findings in graph context, both OWASP editions and the `[security]` gate
//! band, composed by [`Engine::security_page`](crate::Engine::security_page)
//! for `GET /api/v1/security`.
//!
//! **Both editions ride on one payload**, so the page switches between them
//! without a refetch ([FR-SC-04]): [`editions`] classifies the same stored
//! findings under 2025 and 2021, each with its category coverage. It reuses
//! [`findings`](super::findings) — the one place that decides which scanners
//! have looked at a category ([BR-66]) — and keeps only the classification, so
//! "not covered" cannot be decided two ways. The graph context is asked for
//! once, by the listing the page shows; here it is not computed at all.
//!
//! No source snippet is read or carried: a stored finding has none
//! ([ADR-79] §2), and a secret-class message is stored redacted ([FR-SC-03]).
//!
//! [FR-SC-03]: ../../../docs/specs/requirements/FR-SC-03.md
//! [FR-SC-04]: ../../../docs/specs/requirements/FR-SC-04.md
//! [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
//! [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
//! [S-670]: ../../../docs/planning/journal.md#s-670-the-security-page-shows-scanners-findings-and-the-owasp-view

use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

use super::context::{FindingContext, UnboundBecause};
use super::{FindingTally, FindingsFilter, ScanEnvironment, SecurityFindings, SecurityGateBand, SecurityStatus};
use crate::history::security::owasp::{CategoryCoverage, FindingClassification};
use crate::history::security::OwaspEdition;
use crate::tools::Manifest;

/// What `GET /api/v1/security` serves ([FR-SC-09]).
///
/// [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecurityPage {
    /// Each scanner's one state, with the install command for a missing one.
    pub status: SecurityStatus,
    /// Every stored finding (unfiltered: the page filters), classified under
    /// 2025, each in graph context.
    pub findings: SecurityFindings,
    /// The stored findings the page counts — fresh per finding, unsuppressed —
    /// and those it leaves out: the figure the workspace roll-up row and Files &
    /// Risk read too ([S-760]).
    ///
    /// [S-760]: ../../../docs/planning/journal.md#s-760-one-freshness-definition-across-the-security-surfaces
    pub counts: FindingTally,
    /// The same findings under each edition, 2025 then 2021.
    pub editions: Vec<EditionClassification>,
    /// The `[security]` gate's verdict; `None` without the table.
    pub gate: Option<SecurityGateBand>,
}

/// The stored findings under one OWASP edition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditionClassification {
    pub edition: OwaspEdition,
    /// Every category of the edition: covered with a count, or not covered.
    pub categories: Vec<CategoryCoverage>,
    /// One per stored finding, keyed by scanner and fingerprint.
    pub findings: Vec<FindingClassification>,
}

/// The editions the page offers, in the order it lists them.
const EDITIONS: [OwaspEdition; 2] = [OwaspEdition::V2025, OwaspEdition::V2021];

/// The stored findings classified under each edition ([module docs](self)).
/// Starts no process, as [`findings`](super::findings) starts none.
///
/// # Errors
/// As [`findings`](super::findings): an invalid config or ignore file
/// (exit 2), or a store failure.
pub(crate) fn editions(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
) -> Result<Vec<EditionClassification>> {
    Ok(listings(root, env, manifest, today)?
        .into_iter()
        .map(|listed| EditionClassification {
            edition: listed.edition,
            categories: listed.categories,
            findings: listed
                .findings
                .into_iter()
                .map(|item| FindingClassification {
                    scanner_name: item.finding.scanner_name,
                    fingerprint: item.finding.fingerprint,
                    categories: item.owasp,
                    unclassified: item.unclassified,
                })
                .collect(),
        })
        .collect())
}

/// The stored findings listed under each edition, 2025 then 2021, with **no**
/// graph context: each finding carries a placeholder (unbound, no location)
/// where [`findings`](super::findings) would compute one. For the read-models
/// that keep only the classification, the counts and the suppression —
/// [`editions`] here and the workspace roll-up's
/// [`MemberSecurity`](super::MemberSecurity) — so neither pays for the graph
/// walk, and neither re-derives which scanners have looked at a category
/// ([BR-66]). Starts no process.
///
/// # Errors
/// As [`findings`](super::findings): an invalid config or ignore file
/// (exit 2), or a store failure.
///
/// [BR-66]: ../../../docs/specs/software-spec.md#328-security-findings
pub(crate) fn listings(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    today: NaiveDate,
) -> Result<Vec<SecurityFindings>> {
    EDITIONS
        .into_iter()
        .map(|edition| {
            let filter = FindingsFilter { edition, ..FindingsFilter::default() };
            super::findings(root, env, manifest, &filter, today, |kept| {
                Ok(kept
                    .iter()
                    .map(|f| FindingContext::unbound(f.start_line, UnboundBecause::NoLocation))
                    .collect())
            })
        })
        .collect()
}
