//! [`Coverage`] — the coverage evidence surfaces ([FR-CV-01]..[FR-CV-18]),
//! reached by [`Engine::coverage`] (S-829, [CR-259]): ingest (manual and
//! watcher-driven), the coverage run and its consent, the status readouts, and
//! the managed coverage tools' install.
//!
//! Coverage is advisory: none of these moves the gate ([BR-28]), and the
//! evidence store is never `ATTACH`-ed to the canonical graph.
//!
//! [FR-CV-01]: ../../../docs/specs/requirements/FR-CV-01.md
//! [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
//! [BR-28]: ../../../docs/specs/software-spec.md#323-coverage-test-evidence
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::Path;

use anyhow::{anyhow, Result};

use super::Engine;
use crate::observability::Tool;

/// The coverage evidence surfaces over one engine, reached by
/// [`Engine::coverage`].
#[derive(Clone, Copy)]
pub struct Coverage<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Coverage<'e> {
    /// Ingest an external coverage report into the evidence store ([FR-CV-01]..
    /// [FR-CV-04]): the `logos coverage ingest <report>` subcommand and the MCP
    /// `coverage_ingest` twin behind this one `api` entrypoint ([NFR-CC-01]).
    ///
    /// `format` is the `--format` flag (`"lcov"`/`"cobertura"`/`"jacoco"`/`"gocover"`); `None`
    /// auto-detects from content. Supplies the indexed-file set the report-path
    /// mapper binds against from a `logos.db` graph read **here at the `api`
    /// layer** — the evidence store is never `ATTACH`-ed to the canonical graph
    /// ([BR-28]). Coverage is advisory: this never moves the gate ([BR-28]).
    ///
    /// # Errors
    /// Returns an error (mapping to a non-zero exit) when `--format` is an
    /// unknown token, the report is unreadable / unrecognized / malformed (atomic
    /// rejection, no partial write), `root` has no resolvable HEAD, or
    /// `rules.toml` is invalid. Per-file outcomes (unmatched, stale-rejected,
    /// idempotent no-op) ride inside the returned summary, never errors.
    ///
    /// [FR-CV-01]: ../../../docs/specs/requirements/FR-CV-01.md
    /// [BR-28]: ../../../docs/specs/software-spec.md#323-coverage-test-evidence
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    pub fn ingest(
        &self,
        report_path: &Path,
        format: Option<&str>,
    ) -> Result<crate::history::IngestSummary> {
        crate::observability::traced(Tool::CoverageIngest, || {
            let format_override = parse_coverage_format(format)?;
            // One `config.toml` read resolves the `[coverage_ingest]` table that
            // both the format default and the provenance hash derive from — never
            // two reads that could disagree if the file changes mid-call.
            let ingest_cfg = crate::config::load_config_from_root(&self.engine.root)?
                .coverage_ingest
                .effective();
            self.ingest_with(report_path, format_override, &ingest_cfg)
        })
    }

    /// The shared ingest body behind both [`Self::ingest`] and
    /// [`Self::ingest_auto`]: it takes an already-resolved
    /// [`EffectiveCoverageIngest`](crate::config::EffectiveCoverageIngest) so a
    /// single `config.toml` read drives both the format selection and the snapshot
    /// provenance hash (no time-of-check/time-of-use skew, [FR-CV-09]). Not
    /// `traced` itself — each public caller owns the span, so an auto-ingest emits
    /// exactly one telemetry record, not a nested pair.
    ///
    /// The `[coverage]` table ([FR-CV-09], `rules.toml`) and `[coverage_ingest]`
    /// table (`config.toml`) are folded together into the snapshot's config hash
    /// ([ADR-38]).
    fn ingest_with(
        &self,
        report_path: &Path,
        format_override: Option<crate::history::CoverageFormat>,
        ingest_cfg: &crate::config::EffectiveCoverageIngest,
    ) -> Result<crate::history::IngestSummary> {
        let rules = crate::config::load_rules_from_root(&self.engine.root)?;
        let cfg = rules.coverage.effective();
        let indexed_paths = super::evidence_indexed_paths(self.engine)?;
        crate::history::coverage::ingest(
            &self.engine.root,
            report_path,
            format_override,
            &cfg,
            ingest_cfg,
            &indexed_paths,
        )
    }

    /// Watcher-driven **automatic** coverage ingest ([FR-CV-10], [ADR-38]): ingest
    /// the coverage `artifact` the watcher observed change, through the same path
    /// as [`Self::ingest`] — a local file read + parse + store, **never**
    /// a test run ([NFR-SE-01]). The `[coverage_ingest].format` override is honored
    /// (default `auto` content-detection).
    ///
    /// This is **non-load-bearing** ([ADR-11], [FR-SY-06]): the watcher treats any
    /// error (a half-written artifact mid-CI-run, a malformed report, a missing
    /// HEAD) as a warning and never blocks the sync. Reusing the manual ingest path
    /// keeps a single atomic-rejection contract — a bad artifact leaves the store
    /// byte-identical ([NFR-RA-05]).
    ///
    /// # Errors
    /// The same loud errors as [`Self::ingest`] (unreadable/unrecognized/
    /// malformed report, no resolvable HEAD, invalid config). The watcher degrades
    /// them to a warning at the call site.
    ///
    /// [FR-CV-10]: ../../../docs/specs/requirements/FR-CV-10.md
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    /// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
    /// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    pub fn ingest_auto(&self, artifact: &Path) -> Result<crate::history::IngestSummary> {
        crate::observability::traced(Tool::CoverageIngestAuto, || {
            // One config read drives both the configured format and the snapshot
            // hash — the same `ingest_cfg` instance flows into both, so the parse
            // format and the recorded provenance can never disagree.
            let ingest_cfg = crate::config::load_config_from_root(&self.engine.root)?
                .coverage_ingest
                .effective();
            let format_override = parse_coverage_format(ingest_cfg.format_override())?;
            self.ingest_with(artifact, format_override, &ingest_cfg)
        })
    }

    /// `logos coverage refresh` / `coverage_refresh` ([FR-CV-10] as modified
    /// by CR-234): the custom entry of [`run`](Self::run) —
    /// `coverage run --tool custom` — run under its own name. The configured
    /// `[coverage_ingest].refresh_cmd` runs through `sh -c` in the project
    /// root, only under the project's coverage execution consent, which pins
    /// its exact text ([FR-CV-13]); then the artifact it wrote or rewrote is
    /// ingested.
    ///
    /// # Errors
    /// An error when no `refresh_cmd` is configured; exit 2 — naming
    /// `--consent` and showing the command verbatim — when its consent is
    /// missing or its text changed and `consent` is false. Nothing is spawned
    /// then. The command's own failure is its outcome in the payload.
    ///
    /// [FR-CV-10]: ../../../docs/specs/requirements/FR-CV-10.md
    /// [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
    pub fn refresh(&self, consent: bool) -> Result<crate::coverage::CoverageRunSummary> {
        crate::observability::traced(Tool::CoverageRefresh, || {
            let request = crate::coverage::RunRequest {
                tool: Some(crate::coverage::consent::CUSTOM_TOOL),
                consent,
                entry: "logos coverage refresh",
                dependency_consent: false,
            };
            self.run_untraced(&request)
        })
    }

    /// The coverage run ([FR-CV-14], S-737) with no dependency consent —
    /// `logos coverage run` is [`run_adding`](Self::run_adding),
    /// and MCP `coverage_run` the job's [`run_reporting`](Self::run_reporting):
    /// run every enabled, installed coverage tool in registry order,
    /// then the custom entry when a `refresh_cmd` is configured — or only
    /// `tool` — and ingest each report into the HEAD snapshot as its tool
    /// ends ([`crate::coverage::run`]). With `consent`, what the project's
    /// execution consent does not cover yet is recorded first. Explicit only:
    /// the watcher, timers and read paths never start a run ([ADR-87] §4).
    /// Advisory: a run never moves the gate ([BR-28]).
    ///
    /// # Errors
    /// Exit 2, with nothing started, for an invalid config, an unregistered
    /// tool, or a missing consent without `consent`; an error, also with
    /// nothing started, for `custom` with no `refresh_cmd`. One tool's failure
    /// is its outcome, never an error.
    ///
    /// [FR-CV-14]: ../../../docs/specs/requirements/FR-CV-14.md
    /// [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md
    /// [BR-28]: ../../../docs/specs/software-spec.md#323-coverage-test-evidence
    pub fn run(&self, tool: Option<&str>, consent: bool) -> Result<crate::coverage::CoverageRunSummary> {
        self.run_adding(tool, consent, false)
    }

    /// [`run`](Self::run), and with `dependency_consent`
    /// (`logos coverage run --dependency-consent`, S-749) the project's
    /// dependency consent recorded for each missing coverage dev dependency's
    /// command before the project's own package manager adds it
    /// ([FR-CV-18]). Without it, a dependency the consent does not name yet is
    /// reported in the summary with the consent statement, and nothing is
    /// added. Logos never commits the change.
    ///
    /// # Errors
    /// As [`run`](Self::run). A refused or failed addition
    /// is its outcome in the payload, never an error.
    ///
    /// [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
    pub fn run_adding(
        &self,
        tool: Option<&str>,
        consent: bool,
        dependency_consent: bool,
    ) -> Result<crate::coverage::CoverageRunSummary> {
        crate::observability::traced(Tool::CoverageRun, || {
            let entry = crate::coverage::RUN_ENTRY;
            self.run_untraced(&crate::coverage::RunRequest { tool, consent, entry, dependency_consent })
        })
    }

    /// The run behind [`run`](Self::run) and
    /// [`refresh`](Self::refresh): each report goes through
    /// the one ingest body, with the `[coverage_ingest]` table the run loaded
    /// — one config read for the whole run. Not `traced`: each caller owns
    /// the span.
    fn run_untraced(&self, request: &crate::coverage::RunRequest<'_>) -> Result<crate::coverage::CoverageRunSummary> {
        self.run_progress(request, |_| {})
    }

    /// [`run_untraced`](Self::run_untraced), each tool's
    /// start and settled part handed to `progress` as it happens.
    fn run_progress(
        &self,
        request: &crate::coverage::RunRequest<'_>,
        progress: impl FnMut(&crate::coverage::RunProgress),
    ) -> Result<crate::coverage::CoverageRunSummary> {
        crate::coverage::run(&self.engine.root, &self.engine.security().tools().environment(), request, progress, |report, format, ingest_cfg| {
            self.ingest_with(report, format, ingest_cfg)
        })
    }

    /// [`run`](Self::run) with each tool's start and
    /// settled outcome handed to `progress` as it happens: what the
    /// `coverage-run` job runs ([`crate::coverage::job`], S-738), on a job
    /// thread, so a cancel kills the tool running and starts nothing after it.
    /// Never records consent: [`run_admit`](Self::run_admit)
    /// does, before the job starts. The dependency consent only a terminal
    /// records, so a missing coverage dev dependency is added here only when
    /// an earlier `--dependency-consent` named its command (S-749).
    ///
    /// # Errors
    /// As [`run`](Self::run) without `consent`.
    pub(crate) fn run_reporting(
        &self,
        tool: Option<&str>,
        progress: impl FnMut(&crate::coverage::RunProgress),
    ) -> Result<crate::coverage::CoverageRunSummary> {
        crate::observability::traced(Tool::CoverageRun, || {
            let request =
                crate::coverage::RunRequest { tool, consent: false, entry: crate::coverage::RUN_ENTRY, dependency_consent: false };
            self.run_progress(&request, progress)
        })
    }

    /// `logos coverage install` ([FR-CV-17], S-748): install the managed
    /// coverage tools that are missing — `cargo-llvm-cov` — and add
    /// `llvm-tools-preview` to the user's toolchain with their own `rustup`,
    /// under the install consent ([`crate::tools::surface::coverage_install`]).
    /// CLI only: no MCP tool installs.
    ///
    /// # Errors
    /// Exit 2 for a missing consent (naming each line; nothing is downloaded
    /// and no `rustup` runs), a platform without managed installs or no tools
    /// directory. The lock's `Busy` while another scan or install of the
    /// project holds it.
    ///
    /// [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
    pub fn install(
        &self,
        request: &crate::tools::surface::CoverageInstallRequest,
    ) -> Result<crate::tools::surface::CoverageInstall> {
        super::tools::install_locked(self.engine, || {
            crate::tools::surface::coverage_install(&self.engine.root, &self.engine.security().tools().environment(), self.engine.security().tools().manifest(), request)
        })
    }

    /// The install a Coverage page **Install** readies before its run
    /// ([FR-CV-17], S-750): [`install`](Self::install)
    /// without consent — the page's dialog recorded it
    /// ([`crate::coverage::job::admit_prepared`]), so a line it did not
    /// accept is refused — each step handed to `progress`. Run on the
    /// `coverage-run` job's thread, a cancel kills the download, unpack or
    /// `rustup` going and leaves no partial tool.
    ///
    /// # Errors
    /// As [`install`](Self::install).
    ///
    /// [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
    pub fn install_job(
        &self,
        mut progress: impl FnMut(&crate::tools::surface::CoverageInstallStep<'_>),
    ) -> Result<crate::tools::surface::CoverageInstall> {
        let request = crate::tools::surface::CoverageInstallRequest { from: None, consent: false };
        super::tools::install_locked(self.engine, || {
            let (root, env) = (&self.engine.root, self.engine.security().tools().environment());
            crate::tools::surface::coverage_install_reporting(root, &env, self.engine.security().tools().manifest(), &request, &mut progress)
        })
    }

    /// Admit a run of `tool` — every tool when `None` — against this
    /// project's coverage execution consent before a `coverage-run` job starts
    /// it ([FR-CV-13], S-738). `answer` is the web UI's consent dialog: when it
    /// accepts everything the consent lacks — every tool, and the custom
    /// command's exact current text — exactly that is recorded and the run may
    /// start (`None`); when it accepts less, nothing is recorded and what is
    /// pending is returned, to be asked again. `answer` `None` is a surface
    /// that cannot give the consent (MCP): a run the consent does not cover is
    /// refused as `coverage run` without `--consent` refuses it. Starts no
    /// process.
    ///
    /// # Errors
    /// Exit 2, naming `logos coverage run --consent`, for a refused run;
    /// otherwise as [`run`](Self::run), the consent aside,
    /// and an I/O error recording the consent.
    ///
    /// [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
    pub(crate) fn run_admit(
        &self,
        tool: Option<&str>,
        answer: Option<&crate::coverage::consent::ConsentAnswer<'_>>,
    ) -> Result<Option<crate::coverage::consent::PendingConsent>> {
        use crate::coverage::consent;
        let env = self.engine.security().tools().environment();
        let pending = crate::coverage::consent_pending(&self.engine.root, &env, tool)?;
        match answer {
            _ if pending.is_empty() => Ok(None),
            None => Err(consent::refused(&pending, crate::coverage::RUN_ENTRY).into()),
            Some(answer) if !pending.accepted_by(answer) => Ok(Some(pending)),
            Some(_) => {
                consent::record(env.user_dir.as_deref(), &self.engine.root, &pending)?;
                Ok(None)
            }
        }
    }

    /// The coverage status surface ([FR-CV-05], [FR-CV-06]): per-file freshness
    /// (fresh value / stale label / `n/a`, hash-based against the snapshot anchor)
    /// plus the overall freshness fraction and snapshot provenance. The
    /// `logos coverage status` subcommand and the MCP `coverage_status` twin share
    /// this one `api` entrypoint byte-for-byte ([NFR-CC-01]).
    ///
    /// With no coverage ingested, the evidence reads `n/a` with a one-line
    /// notice — never an error ([FR-CV-06]). The evidence comes from
    /// `history.db` alone ([BR-28]); advisory, never moves the gate.
    ///
    /// The payload also carries the coverage runner's state (S-737): each
    /// tool's one state — detected, else its last run, else never run — and
    /// the execution consent ([`crate::coverage::runner_status`]). Detection
    /// asks a tool outside the project for its version; it runs nothing the
    /// project supplies.
    ///
    /// # Errors
    /// Exit 2 for an invalid repository or user-level config or consent
    /// record; otherwise only an unexpected store failure.
    ///
    /// [FR-CV-05]: ../../../docs/specs/requirements/FR-CV-05.md
    /// [FR-CV-06]: ../../../docs/specs/requirements/FR-CV-06.md
    pub fn status(&self) -> Result<crate::history::CoverageStatus> {
        self.status_probing(crate::coverage::Probes::Every)
    }

    /// [`status`](Self::status) with the version probes
    /// `probes` allows: the web Coverage page reads with
    /// [`Probes::Never`](crate::coverage::Probes::Never), since a page read is
    /// on the serve path, which starts no tool process ([ADR-38]).
    ///
    /// # Errors
    /// As [`status`](Self::status).
    ///
    /// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
    pub fn status_probing(&self, probes: crate::coverage::Probes) -> Result<crate::history::CoverageStatus> {
        crate::observability::traced(Tool::CoverageStatus, || {
            let mut status = crate::history::coverage::status(&self.engine.root)?;
            status.runner = Some(crate::coverage::runner_status(&self.engine.root, &self.engine.security().tools().environment(), probes)?);
            Ok(status)
        })
    }

    /// [`status`](Self::status)'s evidence without the
    /// runner state: the Dashboard's read, which probes no tool — a JVM
    /// build tool's version command takes seconds.
    ///
    /// # Errors
    /// Only an unexpected store failure.
    pub fn evidence(&self) -> Result<crate::history::CoverageStatus> {
        crate::observability::traced(Tool::CoverageStatus, || crate::history::coverage::status(&self.engine.root))
    }
}

/// Resolve a `--format`-style token (`"lcov"`/`"cobertura"`/`"jacoco"`/`"gocover"`) into a
/// [`CoverageFormat`](crate::history::CoverageFormat); `None` means content
/// auto-detection. Shared by the manual, automatic, and refresh ingest paths so
/// the unknown-token error is identical across all three ([NFR-CC-01]).
///
/// # Errors
/// Errors on an unrecognized token.
fn parse_coverage_format(
    token: Option<&str>,
) -> Result<Option<crate::history::CoverageFormat>> {
    match token {
        Some(token) => Ok(Some(crate::history::CoverageFormat::from_flag(token).ok_or_else(
            || anyhow!("unknown coverage --format {token:?} (expected lcov, cobertura, jacoco or gocover)"),
        )?)),
        None => Ok(None),
    }
}
