//! [`Setup`] — the bootstrap that precedes any engine (S-829, [CR-259]):
//! initialise a project's `.logos/` (FR-IN-01, FR-IN-04) with the optional
//! host-integration steps (S-023), install the sync git hooks ([FR-IN-03]),
//! and remove what Logos wrote ([FR-IN-12]). Each is an associated function: it
//! takes the root an engine is not yet started on.
//!
//! [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
//! [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::Path;

use anyhow::Result;

use super::{Engine, Pipeline, LOG_TARGET};
use crate::init::InitOptions;
use crate::models::pipeline::InitResult;
use crate::observability::Tool;

/// A project's bootstrap: `init`, its host-integration steps, the git hooks and
/// the uninstall. No engine is needed to reach it; `init` starts the one it
/// needs.
pub struct Setup;

impl Setup {
    /// Initialise a `.logos/` directory in `root` and create the canonical DB,
    /// starter policy templates, and the generated `.gitignore` (FR-IN-01,
    /// FR-IN-04).
    ///
    /// Idempotent and non-clobbering (DL-07): on an already-initialised
    /// project this re-opens the store, applies any pending forward
    /// migrations, refreshes only the managed `.gitignore` block, and never
    /// overwrites an existing policy file.
    ///
    /// Equivalent to [`Self::init_with`] with [`InitOptions::default`] —
    /// the plain (non-`-i`) CLI contract.
    ///
    /// # Errors
    /// Returns an error if `.logos/` cannot be created, the store cannot be
    /// opened/migrated, or a Logos-owned artifact cannot be written.
    pub fn init(root: impl AsRef<Path>) -> Result<InitResult> {
        Self::init_with(root, &InitOptions::default())
    }

    /// [`Self::init`] plus the optional host-integration steps selected by
    /// `options` (S-023): `.mcp.json` MCP-server-block injection and the
    /// managed `CLAUDE.md` steer (FR-IN-02), and the `core.hooksPath`
    /// git-hook installer (FR-IN-03).
    ///
    /// Extends — does not replace — the Sprint 4 minimal bootstrap: the
    /// store is still created/migrated through [`Engine::start`] (the
    /// canonical create-dir + open/migrate path), then [`crate::init`] runs
    /// the file-generation steps. Each step's outcome is reported in
    /// [`InitResult::steps`]; targets that cannot be touched *safely*
    /// (malformed host config, foreign `core.hooksPath`, not a git repo) are
    /// `Skipped` with a reason, never clobbered (DL-06, DL-07).
    ///
    /// # Errors
    /// Returns an error if `.logos/` cannot be created, the store cannot be
    /// opened/migrated, or a Logos-owned artifact cannot be written.
    pub fn init_with(root: impl AsRef<Path>, options: &InitOptions) -> Result<InitResult> {
        Self::init_with_contributions(root, options, &[], &[])
    }

    /// [`Self::init_with`] plus the `contributions` an extension host supplies
    /// ([FR-DS-04], [ADR-74]): each runs after the core steps, against the same
    /// resolved root and options, and its rows join [`InitResult::steps`].
    /// With none it is exactly [`Self::init_with`]. `owned_skills` are the
    /// skill names every registered extension owns, active or not: `init` never
    /// retires a skill copy under one of them (HF-10).
    ///
    /// # Errors
    /// As [`Self::init_with`], plus a contribution's own I/O failure.
    ///
    /// [FR-DS-04]: ../../../docs/specs/requirements/FR-DS-04.md
    /// [ADR-74]: ../../../docs/specs/architecture/decisions/ADR-74.md
    pub fn init_with_contributions(
        root: impl AsRef<Path>,
        options: &InitOptions,
        contributions: &[std::sync::Arc<dyn crate::init::InitContribution>],
        owned_skills: &[String],
    ) -> Result<InitResult> {
        crate::observability::traced(Tool::Init, || {
            // Resolve the hint exactly as `start` will ([ADR-15], [FR-WT-01]) so
            // the reported paths match where the store actually lands.
            let root = &crate::workspace::resolve_root(root.as_ref());
            let logos_dir = root.join(".logos");
            let db_path = logos_dir.join("logos.db");
            let existed = db_path.exists();
            let engine = Engine::start(root)?;
            let mut steps = crate::init::run(root, options, owned_skills)?;
            // FR-CF-10: the store records the coordinate the config `init` just
            // completed resolves to, so `status` reads it before any index. A
            // config that does not load is `init`'s to report, never to fail on
            // (DL-06) — its `[project] id` row says why, and `index` fails loud.
            match crate::config::load_config_from_root(root) {
                Ok(config) => {
                    let pipeline: Pipeline<'_> = engine.pipeline();
                    pipeline.record_project_id(&config)?;
                }
                Err(err) => tracing::warn!(target: LOG_TARGET,
                    "project id not recorded: the configuration does not load ({err})"
                ),
            }
            drop(engine);
            for contribution in contributions {
                steps.extend(contribution.run(root, options)?);
            }
            Ok(InitResult {
                logos_dir: logos_dir.display().to_string(),
                db_path: db_path.display().to_string(),
                message: if existed {
                    "already initialised — store opened, pending migrations applied".to_string()
                } else if options.workspace_member {
                    // CR-119: true in the workspace context, unlike the plain
                    // message below — the FR-WS-14 supervisor `init --workspace`
                    // just spawned is already warming this member, and `logos
                    // index` at the workspace root would build only the root.
                    // Names `WarmStartDisclosure::STATUS_COMMAND` rather than a
                    // second literal, so this and the workspace-level disclosure
                    // cannot drift onto different surface names.
                    format!(
                        "enrolled — queued for background warming (see `{}`)",
                        crate::federation::enable::WarmStartDisclosure::STATUS_COMMAND
                    )
                } else {
                    "initialised — run `logos index` to build the code graph".to_string()
                },
                steps,
            })
        })
    }

    /// Install the optional sync git hooks for the repository at `root`
    /// ([FR-SY-05], [FR-IN-03]): managed post-commit/post-checkout/post-merge
    /// scripts under `.logos/hooks/` + `core.hooksPath`, non-clobbering over
    /// foreign hook setups. The `init -i` flow (S-023) calls this for its
    /// optional hook-install step; see [`crate::hooks`] for the contract.
    ///
    /// # Errors
    /// Returns an error if `root` is not a git work tree, `git` is absent, or
    /// the scripts cannot be written.
    ///
    /// [FR-SY-05]: ../../../docs/specs/requirements/FR-SY-05.md
    /// [FR-IN-03]: ../../../docs/specs/requirements/FR-IN-03.md
    pub fn install_hooks(root: impl AsRef<Path>) -> Result<crate::hooks::HooksResult> {
        crate::hooks::install(root.as_ref())
    }

    /// `logos init --uninstall` ([FR-IN-12]): remove every artefact Logos
    /// wrote into the project — the managed blocks, the MCP, hook and plugin
    /// enrolment entries, the scripts and skill copies with their pointers, and
    /// the managed git hooks, unsetting `core.hooksPath` only if Logos set it
    /// ([`crate::hooks::uninstall`]). Foreign content and `.logos/` stay, and
    /// no store is opened or created. A second run reports every row
    /// `Unchanged`.
    ///
    /// # Errors
    /// On an I/O failure on a Logos-written file, and when git fails part-way
    /// through removing the managed git hooks.
    ///
    /// [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
    pub fn uninstall(root: impl AsRef<Path>) -> Result<crate::models::pipeline::UninstallResult> {
        use crate::models::pipeline::InitAction;
        let root = &crate::workspace::resolve_root(root.as_ref());
        let steps = crate::init::uninstall(root)?;
        let removed = steps.iter().any(|s| s.action != InitAction::Unchanged && s.action != InitAction::Skipped);
        Ok(crate::models::pipeline::UninstallResult {
            message: if removed {
                "Logos integration removed; .logos/ is left in place (delete it to drop the index)"
            } else {
                "nothing of Logos's to remove; .logos/ is left in place"
            }
            .to_string(),
            steps,
        })
    }
}
