//! [`Findings`] — the stored security findings, reached by
//! [`Security::findings`](super::Security::findings) (S-829, [CR-259]): the
//! listing in graph context ([FR-SC-08], [FR-SC-13]), the Security page and the
//! Dashboard widget, the suppressions with their evidence ([FR-SC-12],
//! [FR-SC-32]) and the ratchet's checked-in baseline ([FR-SC-25]).
//!
//! [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
//! [FR-SC-12]: ../../../docs/specs/requirements/FR-SC-12.md
//! [FR-SC-13]: ../../../docs/specs/requirements/FR-SC-13.md
//! [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
//! [FR-SC-32]: ../../../docs/specs/requirements/FR-SC-32.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use anyhow::Result;

use super::{today_utc, Engine};
use crate::observability::Tool;

/// The stored security findings over one engine, reached by
/// [`Security::findings`](super::Security::findings).
#[derive(Clone, Copy)]
pub struct Findings<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Findings<'e> {
    /// `logos security findings` / `security_findings` ([FR-SC-08]): the
    /// stored findings with their OWASP categories ([S-664]) and suppression
    /// ([S-666]), narrowed by `filter` ([`crate::security::findings`]), each in
    /// the graph context this engine computes from logos.db — enclosing
    /// symbol, route reachability and blast radius, each labelled
    /// ([FR-SC-13], [`crate::security::context`]) — and with its code scope by
    /// this graph ([`crate::security::ScopeFilter`], S-795): `filter`'s scope keeps
    /// production code unless it names another, and the payload states how
    /// many the scope hid. Starts no scanner.
    ///
    /// # Errors
    /// Exit 2 for an invalid config or ignore file; otherwise only a store
    /// failure.
    ///
    /// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
    /// [FR-SC-13]: ../../../docs/specs/requirements/FR-SC-13.md
    /// [S-664]: ../../../docs/planning/journal.md#s-664-findings-are-classified-by-owasp-top-10-2025-and-2021
    /// [S-666]: ../../../docs/planning/journal.md#s-666-suppressions-live-in-a-committed-ignore-file-with-a-reason-and-an-expiry
    pub fn list(
        &self,
        filter: &crate::security::FindingsFilter,
    ) -> Result<crate::security::SecurityFindings> {
        crate::observability::traced(Tool::SecurityFindings, || {
            crate::security::findings(
                &self.engine.root,
                &self.engine.security().tools().environment(),
                self.engine.security().tools().manifest(),
                filter,
                today_utc(),
                |stored| crate::security::context::in_test_symbols(self.engine, stored),
                |listed| crate::security::context::contexts(self.engine, listed),
            )
        })
    }

    /// The web UI's Security page ([FR-SC-09]): the
    /// [status](super::Security::status) matrix, every stored finding in graph
    /// context ([`list`](Self::list), unfiltered,
    /// every code scope, each finding labelled with its own),
    /// the same findings classified under both OWASP editions so the page
    /// switches edition without a refetch ([FR-SC-04]), and — only with a
    /// `rules.toml [security]` table — the verdict `check` reaches on it
    /// ([FR-SC-10]). One status readout serves the matrix and the gate band.
    /// Starts no scanner beyond the version probe of a scanner the project's
    /// scan consent names, once per process for each binary
    /// ([`VersionProbes::Consented`](crate::security::VersionProbes::Consented)):
    /// the page is a read a cross-site page can trigger.
    ///
    /// # Errors
    /// As [`Security::status`](super::Security::status) and
    /// [`list`](Self::list), and an invalid
    /// `.logos/rules.toml` (exit 2).
    ///
    /// [FR-SC-04]: ../../../docs/specs/requirements/FR-SC-04.md
    /// [FR-SC-09]: ../../../docs/specs/requirements/FR-SC-09.md
    /// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
    pub fn page(&self) -> Result<crate::security::SecurityPage> {
        let status = self.engine.security().status_probing(crate::security::VersionProbes::Consented, true)?;
        let gate = crate::security::SecurityGateEvidence(self.engine).band(&status)?;
        let findings = self.list(&crate::security::FindingsFilter::default())?;
        Ok(crate::security::SecurityPage {
            counts: crate::security::FindingTally::of(&findings.findings),
            findings,
            editions: crate::security::editions(
                &self.engine.root,
                &self.engine.security().tools().environment(),
                self.engine.security().tools().manifest(),
                today_utc(),
                |stored| crate::security::context::in_test_symbols(self.engine, stored),
            )?,
            status,
            gate,
        })
    }

    /// The Dashboard's Security widget ([FR-UI-09], S-789): the stored security
    /// state in a handful of figures ([`SecuritySummary`](crate::security::SecuritySummary)) —
    /// the counted and left-out findings as the Security page states them, the
    /// `[security]` gate band, the newest stored scan, the scanners whose last
    /// scan failed, and the consent record. **Stored-only**: the status is read
    /// with [`VersionProbes::Never`](crate::security::VersionProbes::Never), so
    /// no scanner process starts — not even a version probe — and nothing is
    /// written ([ADR-28]); a Dashboard load is a read a cross-site page can
    /// trigger. Test code is classified by this graph, as the page does.
    ///
    /// # Errors
    /// As [`page`](Self::page): an invalid config, consent
    /// record, ignore file or `.logos/rules.toml`, or a store failure.
    ///
    /// [FR-UI-09]: ../../../docs/specs/requirements/FR-UI-09.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    pub fn summary(&self) -> Result<crate::security::SecuritySummary> {
        let status = self.engine.security().status_probing(crate::security::VersionProbes::Never, false)?;
        let gate = crate::security::SecurityGateEvidence(self.engine).band(&status)?;
        crate::security::summary(
            &self.engine.root,
            &self.engine.security().tools().environment(),
            self.engine.security().tools().manifest(),
            today_utc(),
            status,
            gate,
            |stored| crate::security::context::in_test_symbols(self.engine, stored),
        )
    }

    /// Suppress from the Security page ([FR-SC-12]): add an entry for
    /// `selector` — one finding's fingerprint, a path glob with its
    /// narrowing ([S-770]), or a rule of one scanner in every file ([S-814]) — to `.logos/security-ignore.toml` with `reason` and
    /// an optional `expires` date written `YYYY-MM-DD`
    /// ([`crate::history::security::suppress`]). Touches no store: the next
    /// [`list`](Self::list) labels the findings from
    /// the file.
    ///
    /// # Errors
    /// Exit 2 and no write for a blank reason, a malformed or already
    /// suppressed selector, a path glob covering the whole project, an expiry
    /// that is not a date or is in the past, or an ignore file that is
    /// malformed or hand-spelled; a failed atomic write.
    ///
    /// [FR-SC-12]: ../../../docs/specs/requirements/FR-SC-12.md
    /// [S-770]: ../../../docs/planning/journal.md#s-770-a-suppression-can-cover-a-path
    /// [S-814]: ../../../docs/planning/journal.md#s-814-a-rule-can-be-suppressed-project-wide-with-a-reason
    pub fn suppress(
        &self,
        selector: crate::history::security::Selector,
        reason: &str,
        expires: Option<&str>,
    ) -> Result<crate::history::security::IgnoreWrite> {
        let expires = expires.map(crate::history::security::parse_expires).transpose()?;
        crate::history::security::suppress(&self.engine.root, selector, reason, expires, today_utc())
    }

    /// [`suppress`](Self::suppress) that records the
    /// `evidence` its reason rests on ([FR-SC-32], [S-818]): each of `evidence`
    /// — a symbol identity, a canonical symbol or a bare name that names one
    /// symbol — with the hash of its exact source span now, and, for a
    /// fingerprint selector, the finding's own enclosing symbol when it binds
    /// one. The entry then suppresses only while every one holds. No `evidence`
    /// is `suppress`: an entry that reads "no evidence recorded".
    ///
    /// # Errors
    /// As [`suppress`](Self::suppress); and exit 2 and no
    /// write for evidence that names no symbol, several (listing them), or one
    /// the index is behind.
    ///
    /// [FR-SC-32]: ../../../docs/specs/requirements/FR-SC-32.md
    /// [S-818]: ../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
    pub fn suppress_with_evidence(
        &self,
        selector: crate::history::security::Selector,
        reason: &str,
        expires: Option<&str>,
        evidence: &[String],
    ) -> Result<crate::history::security::IgnoreWrite> {
        crate::history::security::suppress_recording(
            &self.engine.root,
            selector,
            reason,
            expires,
            today_utc(),
            evidence,
            &|finding| self.enclosing_identity(finding),
        )
    }

    /// The identity of the symbol that encloses `finding` in this engine's
    /// graph, or `None` when it binds none ([`crate::security::context`]).
    fn enclosing_identity(&self, finding: &crate::history::SecurityFinding) -> Result<Option<String>> {
        let nodes = crate::security::context::enclosing_symbols(self.engine, &[finding])?;
        Ok(nodes.into_iter().next().flatten().map(|node| node.identity().to_string()))
    }

    /// `logos security reconfirm <fingerprint>` / `security_reconfirm` / the
    /// Security page's Re-confirm ([FR-SC-32], [S-818]): re-record the hash of
    /// every evidence symbol of the entry that suppresses the finding with this
    /// `fingerprint` (or is the entry for it), keeping its reason and expiry —
    /// the verdict re-read against the code as it is now and confirmed. The
    /// findings it covers are suppressed again. A write to the ignore file, so
    /// guarded as one ([FR-IN-14]).
    ///
    /// # Errors
    /// Exit 2 and no write when no entry or stored finding carries the
    /// fingerprint, no entry suppresses the finding, the entry records no
    /// evidence, an evidence symbol no longer exists (suppress again with other
    /// evidence), the index is behind a file, or the ignore file is malformed
    /// or hand-spelled; a failed write.
    ///
    /// [FR-SC-32]: ../../../docs/specs/requirements/FR-SC-32.md
    /// [FR-IN-14]: ../../../docs/specs/requirements/FR-IN-14.md
    /// [S-818]: ../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
    pub fn reconfirm(&self, fingerprint: &str) -> Result<crate::history::security::IgnoreWrite> {
        self.reconfirm_with_evidence(fingerprint, &[])
    }

    /// [`reconfirm`](Self::reconfirm) naming the symbols the
    /// verdict now rests on: with `evidence`, the entry records exactly those
    /// symbols (each as [`suppress_with_evidence`](Self::suppress_with_evidence)
    /// pins it, the finding's enclosing symbol included) in place of the ones it
    /// recorded — the way to give an entry that records none its evidence, or to
    /// swap a symbol that was deleted. Without, every recorded symbol is
    /// re-recorded as it is now.
    ///
    /// # Errors
    /// As [`reconfirm`](Self::reconfirm), and for `evidence`
    /// that names no symbol, several, or one the index is behind.
    pub fn reconfirm_with_evidence(
        &self,
        fingerprint: &str,
        evidence: &[String],
    ) -> Result<crate::history::security::IgnoreWrite> {
        let enclosing = |finding: &crate::history::SecurityFinding| self.enclosing_identity(finding);
        crate::history::security::reconfirm_entry(&self.engine.root, fingerprint, today_utc(), evidence, &enclosing)
    }

    /// Remove the entry for `selector` ([FR-SC-12],
    /// [`crate::history::security::unsuppress`]); the findings it suppressed
    /// count again.
    ///
    /// # Errors
    /// Exit 2 and no write when no entry carries `selector` or the ignore
    /// file is malformed or hand-spelled; a failed write.
    ///
    /// [FR-SC-12]: ../../../docs/specs/requirements/FR-SC-12.md
    pub fn unsuppress(
        &self,
        selector: crate::history::security::Selector,
    ) -> Result<crate::history::security::IgnoreWrite> {
        crate::history::security::unsuppress(&self.engine.root, selector)
    }

    /// `logos security bless` ([FR-SC-25]): write the security ratchet's
    /// baseline findings — fingerprint, rule id and project-relative path,
    /// sorted — to the checked-in `.logos/security-baseline.json`, which the
    /// ratchet then reads in place of the set `gate --save` records in the
    /// gitignored `history.db`, so a CI clone reads only new findings as new.
    /// Its only writer; runs no scanner. Exit 5 and no write when a
    /// `[security]` table's required scan is missing
    /// ([`SecurityBless::exit_code`](crate::security::SecurityBless::exit_code)).
    ///
    /// # Errors
    /// Exit 2 for an invalid `.logos/rules.toml`, config or ignore file, or a
    /// failed write; otherwise only a store failure.
    ///
    /// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
    pub fn bless(&self) -> Result<crate::security::SecurityBless> {
        crate::security::SecurityGateEvidence(self.engine).bless()
    }
}
