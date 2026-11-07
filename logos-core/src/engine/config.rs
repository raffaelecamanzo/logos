//! [`Configuration`] — the validated config write-back seam the web config
//! editor builds on (S-096, [CR-025], [ADR-31]), reached by [`Engine::config`]
//! (S-829, [CR-259]): read both checked-in policy files, write one atomically,
//! write the chat API key, and apply a saved edit.
//!
//! [CR-025]: ../../../docs/requests/CR-025-interactive-config-editing.md
//! [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::Path;

use anyhow::Result;

use super::{Engine, Pipeline};
use crate::observability::Tool;

/// The policy files of one engine's root, reached by [`Engine::config`].
#[derive(Clone, Copy)]
pub struct Configuration<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Configuration<'e> {
    /// Read both checked-in policy files for this engine's root — raw content +
    /// parsed model ([FR-UI-12], [CR-025]): the read half of the validated
    /// config-write seam the web config editor builds on.
    ///
    /// A pure read of `.logos/config.toml` and `.logos/rules.toml`; it touches no
    /// graph store, so it works on both engine flavours (a transient
    /// [`Engine::open`] engine is enough). An absent file is reported
    /// `exists = false` with the effective default model, not an error
    /// ([NFR-DM-04]).
    ///
    /// `workspace_root` is the federation root the caller already resolved —
    /// `None` in single-root mode, never a discovered one. It reaches only the
    /// read-model's [`effective_chat`](crate::config::ConfigReadModel::effective_chat)
    /// and [`effective_wiki`](crate::config::ConfigReadModel::effective_wiki)
    /// slices ([FR-WS-30], S-448, Sprint 77 HF-1); the literal documents are this
    /// root's alone. The [`workspace_rules`](crate::config::ConfigReadModel::workspace_rules)
    /// slice is read from the rules tier this root's governance runs inherit
    /// ([FR-CF-08], S-508), whatever `workspace_root` is.
    ///
    /// # Errors
    /// A present-but-invalid policy file fails loud through the load path — an
    /// unknown key, a non-compiling glob, or an out-of-range value is a
    /// [`ConfigError`](crate::config::ConfigError) (usage exit 2).
    ///
    /// [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
    /// [CR-025]: ../../../docs/requests/CR-025-interactive-config-editing.md
    /// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
    /// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
    /// [FR-CF-08]: ../../../docs/specs/requirements/FR-CF-08.md
    pub fn read(
        &self,
        workspace_root: Option<&Path>,
    ) -> Result<crate::config::ConfigReadModel> {
        self.on_root(Tool::ConfigRead, |root| {
            let mut model = crate::config::read_documents(root, workspace_root)?;
            // S-508: what the member inherits from its workspace rules tier, beside
            // (never inside) its own literal rules document.
            model.workspace_rules = crate::config::inherited_rules(root, &model.rules.parsed);
            Ok(model)
        })
    }

    /// Validate an edited policy-file `candidate` against the **existing** load
    /// path and, only if valid, replace the file **atomically**
    /// (write-temp-then-rename) — the write half of the config-editor seam
    /// ([FR-UI-12], [ADR-31]).
    ///
    /// `candidate` is the full candidate TOML document. It is run through the
    /// same `#[serde(deny_unknown_fields)]` parse + glob/range validation the CLI
    /// loader runs ([FR-CF-01]/[FR-CF-03]) — no new validation is invented — so an
    /// invalid edit is rejected with **no partial write** and the file is left
    /// byte-identical ([NFR-RA-07]). A [`PolicyFile::Rules`](crate::config::PolicyFile)
    /// write additionally stamps a provenance comment and is validated *with* the
    /// stamp, so the written contract still parses via the standard load path
    /// ([BR-35]).
    ///
    /// Like [`read`](Self::read) this is pure filesystem I/O over
    /// the resolved root and needs no graph runtime. Save alone runs no pipeline —
    /// reindex/re-eval is a separate explicit action ([FR-UI-13]).
    ///
    /// # Errors
    /// A [`ConfigError`](crate::config::ConfigError) (usage exit 2) for an invalid
    /// candidate, or [`ConfigError::Write`](crate::config::ConfigError::Write) if
    /// the atomic write itself fails (the original file is unchanged).
    ///
    /// [FR-UI-12]: ../../../docs/specs/requirements/FR-UI-12.md
    /// [FR-UI-13]: ../../../docs/specs/requirements/FR-UI-13.md
    /// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
    /// [FR-CF-03]: ../../../docs/specs/requirements/FR-CF-03.md
    /// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
    /// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
    /// [BR-35]: ../../../docs/specs/software-spec.md#326-web-ui
    pub fn write(
        &self,
        file: crate::config::PolicyFile,
        candidate: &str,
    ) -> Result<crate::config::ConfigWriteOutcome> {
        crate::observability::traced(Tool::ConfigWrite, || {
            let outcome = match file {
                crate::config::PolicyFile::Config => {
                    crate::config::write_config(&self.engine.root, candidate)?
                }
                crate::config::PolicyFile::Rules => {
                    crate::config::write_rules(&self.engine.root, candidate)?
                }
            };
            Ok(outcome)
        })
    }

    /// Write (or clear) the chat API key in the gitignored `.logos/secrets.toml`
    /// via the same validated atomic write the policy files use (S-169,
    /// [FR-CF-06], [NFR-SE-07]).
    ///
    /// `api_key` is the raw key the Config editor posted; a blank value clears it.
    /// The key writes to `secrets.toml` (never the checked-in `config.toml`), and
    /// the returned [`SecretWriteOutcome`](crate::config::SecretWriteOutcome)
    /// carries only the **masked** new state (presence + last-4) — the raw key is
    /// never echoed back ([NFR-SE-07]). Like the policy write-back this is pure
    /// filesystem I/O and runs no pipeline.
    ///
    /// # Errors
    /// A [`ConfigError`](crate::config::ConfigError) (usage exit 2) if an existing
    /// `secrets.toml` does not parse, or
    /// [`ConfigError::Write`](crate::config::ConfigError::Write) if the atomic
    /// write itself fails (the original file is unchanged).
    ///
    /// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
    /// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
    pub fn write_secret(
        &self,
        api_key: &str,
    ) -> Result<crate::config::SecretWriteOutcome> {
        self.on_root(Tool::ConfigWriteSecret, |root| crate::config::write_secret(root, api_key))
    }

    /// Apply a validated config edit ([FR-UI-13], [ADR-31]): act on the new
    /// policy the editor just saved through [`write`](Self::write).
    /// This is the **explicit Apply** action — Save alone runs no pipeline, so
    /// the engine's derived state changes only when this is invoked.
    ///
    /// - [`PolicyFile::Config`](crate::config::PolicyFile::Config) runs
    ///   reconcile/index through the [pipeline-orchestrator], reusing the
    ///   existing admission-fingerprint reconciliation ([FR-SY-07]) so the graph
    ///   reflects the new admission policy — purging now-unadmitted files,
    ///   O(changed), not a full reindex.
    /// - [`PolicyFile::Rules`](crate::config::PolicyFile::Rules) re-runs the
    ///   [governance-engine] scan against the *unchanged* graph (`reconcile =
    ///   false`, no reindex) so the gate / quality views reflect the new
    ///   contract — a rules change alters governance evaluation, not graph
    ///   admission. Like every scan it persists a `metric_snapshots` row and
    ///   refreshes the violation set ([FR-GV-09]), so the dashboard's
    ///   latest-scan view reflects the new contract after Apply.
    ///
    /// The whole apply is a **single** instrumented façade span: it dispatches
    /// to the *inner* reconcile/scan seams (`run_reconcile`,
    /// [`crate::governance::scan`]) rather than the already-traced public
    /// [`index`](super::Pipeline::index)/[`scan`](super::Quality::scan), so it emits exactly one
    /// `config_apply` telemetry record and never nests a second façade span
    /// ([NFR-CC-01]). It runs on the engine pool, not the surface thread — every
    /// mutation routes through the writer actor, which serializes it against a
    /// running `serve` watcher ([ADR-02], [ADR-31]).
    ///
    /// # Errors
    /// Returns an error for a transient [`Engine::open`] engine (no runtime), or
    /// on a structural reconcile/scan failure ([ADR-14] fail-loud); per-file
    /// degradations ride inside the summary's `warnings`/`files_failed`.
    ///
    /// [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
    /// [governance-engine]: ../../../docs/specs/architecture/components/governance-engine.md
    /// [FR-UI-13]: ../../../docs/specs/requirements/FR-UI-13.md
    /// [FR-SY-07]: ../../../docs/specs/requirements/FR-SY-07.md
    /// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
    /// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
    /// [ADR-31]: ../../../docs/specs/architecture/decisions/ADR-31.md
    pub fn apply(
        &self,
        file: crate::config::PolicyFile,
    ) -> Result<crate::config::ConfigApplyOutcome> {
        use crate::config::{ConfigApplyOutcome, PolicyFile};
        crate::observability::traced(Tool::ConfigApply, || match file {
            PolicyFile::Config => {
                let pipeline: Pipeline<'_> = self.engine.pipeline();
                let outcome = pipeline.run_reconcile()?;
                Ok(ConfigApplyOutcome::Reconciled {
                    reconciled_files: outcome.reconciled_files,
                    full_index: outcome.full_index,
                    unresolved_refs: outcome.resolution.refs_unresolved,
                    files_failed: outcome.files_failed,
                    warnings: outcome.warnings,
                    notes: outcome.notes,
                })
            }
            PolicyFile::Rules => {
                // No reindex: a rules change alters governance evaluation, not
                // graph admission ([FR-UI-13]). Re-evaluate against the current
                // graph (`reconcile = false`) through the inner scan body — not
                // the traced public `scan` — so the apply stays one façade span.
                // Going through the inner body also deliberately skips
                // `record_scan`: an apply is not an operator-triggered scan and
                // must not become the parameters a later `rescan` replays.
                let scan = crate::governance::scan(self.engine, false)?;
                Ok(ConfigApplyOutcome::Reevaluated {
                    signal: scan.signal,
                    violations: scan.violations.len(),
                    freshness: scan.freshness,
                    warnings: scan.warnings,
                })
            }
        })
    }

    /// `f` over this engine's root, under `tool`'s span, its typed
    /// [`ConfigError`](crate::config::ConfigError) carried in the `anyhow`
    /// error so a caller can still downcast it.
    fn on_root<T>(
        &self,
        tool: Tool,
        f: impl FnOnce(&Path) -> Result<T, crate::config::ConfigError>,
    ) -> Result<T> {
        crate::observability::traced(tool, || Ok(f(&self.engine.root)?))
    }
}
