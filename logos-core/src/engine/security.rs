//! [`Security`] — the security surfaces ([FR-SC-01]..[FR-SC-32]), reached by
//! [`Engine::security`] (S-829, [CR-259]): report ingest (manual and
//! watcher-driven), the scanner runs and their consent, the scanner status and
//! the scan-or-install lock, with the stored [`Findings`] and the managed
//! scanner [`Tools`] beneath it.
//!
//! [FR-SC-01]: ../../../docs/specs/requirements/FR-SC-01.md
//! [FR-SC-32]: ../../../docs/specs/requirements/FR-SC-32.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use super::{today_utc, Engine, Findings, Tools};
use crate::observability::Tool;
use crate::tools::surface;

/// The security surfaces over one engine, reached by [`Engine::security`].
#[derive(Clone, Copy)]
pub struct Security<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Security<'e> {
    /// The stored findings: their listing, the Security page and widget, and
    /// the suppressions over them.
    pub fn findings(&self) -> Findings<'e> {
        Findings { engine: self.engine }
    }

    /// The managed scanner tools and where the security surfaces find them.
    pub fn tools(&self) -> Tools<'e> {
        Tools { engine: self.engine }
    }

    /// Ingest a security report (SARIF 2.1.0, from any tool) into the evidence
    /// store ([FR-SC-01]..[FR-SC-03], [ADR-79]): parsed and validated before any
    /// write, findings bound to indexed files by longest unique suffix, anchored
    /// to HEAD and the scanner, keyed by fingerprint, and secret-class messages
    /// redacted before the write. A byte-identical re-ingest at the same HEAD is a
    /// no-op reported `already_ingested`. Security evidence is not gate input
    /// here: nothing reads it on the gate path.
    ///
    /// # Errors
    /// Returns an error (a non-zero exit on a surface) when the report is
    /// unreadable, not SARIF, or malformed — the defect is named and nothing is
    /// written — or `root` has no resolvable HEAD. Per-finding outcomes ride in
    /// the returned summary.
    ///
    /// [FR-SC-01]: ../../../docs/specs/requirements/FR-SC-01.md
    /// [FR-SC-03]: ../../../docs/specs/requirements/FR-SC-03.md
    /// [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md
    pub fn ingest(&self, report_path: &Path) -> Result<crate::history::SecurityIngestSummary> {
        crate::observability::traced(Tool::SecurityIngest, || self.ingest_untraced(report_path, None))
    }

    /// The ingest body behind [`Self::ingest`],
    /// [`Self::ingest_auto`] and [`Self::scan`]. Not
    /// `traced`: each public caller owns the span.
    fn ingest_untraced(
        &self,
        report_path: &Path,
        scope: Option<crate::history::security::IngestScope<'_>>,
    ) -> Result<crate::history::SecurityIngestSummary> {
        let indexed_paths = super::evidence_indexed_paths(self.engine)?;
        crate::history::security::ingest_scoped(&self.engine.root, report_path, &indexed_paths, scope)
    }

    /// Watcher-driven **automatic** security ingest ([FR-SC-06]): ingest the
    /// SARIF report the watcher saw change under `.logos/security/`, through the
    /// same path as [`Self::ingest`] — a local read, parse and store,
    /// **never** a scanner run. Its own span, so a watcher ingest is told apart
    /// from one the user asked for, as [`Coverage::ingest_auto`](super::Coverage::ingest_auto) is.
    ///
    /// Only a regular file is read: `.logos/security/` can hold what a
    /// repository committed, and a symbolic link to a FIFO would block the one
    /// sync worker for good, or one to `/dev/zero` grow its read without end.
    ///
    /// # Errors
    /// As [`Self::ingest`], or when `artifact` is not a regular file
    /// (a symbolic link, FIFO or device); the watcher degrades them to a warning.
    ///
    /// [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
    pub fn ingest_auto(&self, artifact: &Path) -> Result<crate::history::SecurityIngestSummary> {
        crate::observability::traced(Tool::SecurityIngestAuto, || {
            let kind = std::fs::symlink_metadata(artifact)
                .with_context(|| format!("reading {}", artifact.display()))?
                .file_type();
            if !kind.is_file() {
                return Err(anyhow!(
                    "{} is not a regular file; the watcher ingests a report only from one",
                    artifact.display()
                ));
            }
            self.ingest_untraced(artifact, None)
        })
    }

    /// `logos security scan` / `security_scan` ([FR-SC-06], [FR-SC-07]): run
    /// every installed, enabled scanner and ingest each report it writes under
    /// `.logos/security/`, after checking the project's recorded consent. With
    /// `consent`, the scanners the record does not name yet are added to it.
    /// The payload is one outcome per registered scanner
    /// ([`crate::security::scan`]).
    ///
    /// # Errors
    /// Exit 2 for a missing consent (naming `--consent` and the scanners it
    /// lacks) or an invalid repository or user config — nothing is started
    /// then. A scanner's own failure is its `failed` outcome, never an error.
    ///
    /// [FR-SC-06]: ../../../docs/specs/requirements/FR-SC-06.md
    /// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
    pub fn scan(&self, consent: bool) -> Result<crate::security::SecurityScanSummary> {
        self.scan_selected(consent, &crate::security::ScanRequest::full(), |_| {})
    }

    /// [`scan`](Self::scan), with each scanner's start and
    /// settled outcome handed to `progress` as it happens — the web UI's scan
    /// job ([FR-SC-15]), which runs it on a job thread, so a cancel kills the
    /// scanner running and starts nothing after it.
    ///
    /// # Errors
    /// As [`scan`](Self::scan), and the jobs module's
    /// `Cancelled` when the job was cancelled.
    ///
    /// [FR-SC-15]: ../../../docs/specs/requirements/FR-SC-15.md
    pub fn scan_reporting(
        &self,
        consent: bool,
        progress: impl FnMut(&crate::security::ScanProgress),
    ) -> Result<crate::security::SecurityScanSummary> {
        self.scan_selected(consent, &crate::security::ScanRequest::full(), progress)
    }

    /// `logos security scan --scanner … --path …` ([FR-SC-17]): the scan the
    /// two methods above run, over the scanners and paths `request` names.
    /// Each path is confined to the project root before any process starts; a
    /// path-capable scanner's report replaces its findings only within the
    /// paths. Every scan goes through here, progress and cancel included, and
    /// holds the project's scan-or-install lock while it runs (S-758).
    ///
    /// # Errors
    /// As [`scan_reporting`](Self::scan_reporting), exit
    /// 2 for an unregistered scanner name or a refused path, and the lock's
    /// `Busy` — naming the scan or install running — while another surface or
    /// process holds the lock: nothing is started then.
    ///
    /// [FR-SC-17]: ../../../docs/specs/requirements/FR-SC-17.md
    pub fn scan_selected(
        &self,
        consent: bool,
        request: &crate::security::ScanRequest,
        mut progress: impl FnMut(&crate::security::ScanProgress),
    ) -> Result<crate::security::SecurityScanSummary> {
        crate::observability::traced(Tool::SecurityScan, || {
            crate::security::exclusively(&self.engine.root, crate::security::RunKind::SecurityScan, || {
                crate::security::scan(
                    &self.engine.root,
                    &self.tools().environment(),
                    self.tools().manifest(),
                    consent,
                    request,
                    &mut progress,
                    |report, scope| self.ingest_untraced(report, scope),
                )
            })
        })
    }

    /// The installed, enabled scanners this project's scan consent does not
    /// name yet ([FR-SC-07]): what [`scan`](Self::scan)
    /// without `consent` would refuse over, and what the web UI's consent
    /// dialog lists. Starts no process.
    ///
    /// # Errors
    /// Exit 2 for an invalid config or consent record.
    ///
    /// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
    pub fn consent_pending(&self) -> Result<Vec<&'static str>> {
        crate::security::consent_pending(&self.engine.root, &self.tools().environment(), self.tools().manifest())
    }

    /// Record this project's scan consent ([FR-SC-07]) for exactly `scanners`
    /// — the ones the web UI's consent dialog showed and the user accepted —
    /// in the user-level `security-consent.toml`, never in the repository.
    ///
    /// # Errors
    /// Exit 2 when there is no user-level directory or the record is invalid;
    /// an I/O error writing it.
    ///
    /// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
    pub fn record_consent(&self, scanners: &[&str]) -> Result<()> {
        crate::config::record_scan_consent(self.tools().environment().user_dir.as_deref(), &self.engine.root, scanners)?;
        Ok(())
    }

    /// `logos security scan --install` ([FR-SC-22]): install the default
    /// scanners this project has not disabled and that are missing — under
    /// the per-user install consent — then [scan](Self::scan). With
    /// `consent`, both the install consent and the project's scan consent
    /// are recorded for what is missing. CLI only: no MCP tool installs. The
    /// install and the scan hold the project's scan-or-install lock throughout
    /// (S-758).
    ///
    /// # Errors
    /// Exit 2 for a missing install consent (nothing is downloaded then), a
    /// missing scan consent, or an invalid config. One tool's failed install
    /// is its outcome; the scan runs over what is installed. The lock's `Busy`
    /// while another scan or install holds it.
    ///
    /// [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
    pub fn scan_install(&self, consent: bool) -> Result<surface::SecurityScanInstall> {
        crate::security::exclusively(&self.engine.root, crate::security::RunKind::ToolInstall, || {
            let install = surface::install_for_scan(&self.engine.root, &self.tools().environment(), self.tools().manifest(), consent)?;
            let scan = self.scan(consent)?;
            Ok(surface::SecurityScanInstall { install, scan })
        })
    }

    /// `logos security status` / `security_status` ([FR-SC-08]): each
    /// registered scanner's one state — not installed, disabled, never run,
    /// not applicable (nothing for it to scan here, S-769), stale, or fresh
    /// with its count of fresh, unsuppressed findings in production code and
    /// the rest by why, test code among them (S-795) — the copy in use with its version
    /// and any `update_available`, the install command for a missing scanner,
    /// a failed last scan's log and one-line cause, and the consent state
    /// ([`crate::security::status`]). Runs no scanner beyond its version
    /// probe, and not that for a scanner with nothing to scan.
    ///
    /// # Errors
    /// Exit 2 for an invalid config, consent record or ignore file; otherwise
    /// only a store failure.
    ///
    /// [FR-SC-08]: ../../../docs/specs/requirements/FR-SC-08.md
    pub fn status(&self) -> Result<crate::security::SecurityStatus> {
        self.status_probing(crate::security::VersionProbes::Every, true)
    }

    /// [`status`](Self::status) with the version probes
    /// `probes` allows — a page read probes only consented scanners, each
    /// binary once per process (HF-3) — and each row's ignored-paths coverage
    /// only when `coverage` asks for it, since it walks the tree. Whether an
    /// installed gosec or Bandit with no stored report is `not_applicable`
    /// walks it whatever `coverage` says, stopping at the first file it reads.
    ///
    /// # Errors
    /// As [`status`](Self::status).
    pub(crate) fn status_probing(
        &self,
        probes: crate::security::VersionProbes,
        coverage: bool,
    ) -> Result<crate::security::SecurityStatus> {
        crate::observability::traced(Tool::SecurityStatus, || {
            crate::security::status(&self.engine.root, &self.tools().environment(), self.tools().manifest(), today_utc(), probes, coverage, |f| {
                crate::security::context::in_test_symbols(self.engine, f)
            })
        })
    }

    /// This project's scan-or-install lock file (S-758), its directory
    /// created: what the web UI's jobs hold for as long as they run
    /// (`AnalysisJobs::start_exclusive`) and peek at before a start records
    /// anything.
    ///
    /// # Errors
    /// A symlinked `.logos` or `.logos/security`, or the directory cannot be
    /// created.
    pub fn lock_file(&self) -> Result<PathBuf> {
        crate::security::lock_file(&self.engine.root)
    }
}
