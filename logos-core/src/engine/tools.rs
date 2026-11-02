//! [`Tools`] — the managed scanner tools ([FR-SC-22], [FR-SC-24]), reached by
//! [`Security::tools`](super::Security::tools) (S-829, [CR-259]): install,
//! uninstall, the Security page's install admission and job, and where the
//! security and coverage surfaces find the user-level directory, the scanner
//! binaries and the tool manifest.
//!
//! [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
//! [FR-SC-24]: ../../../docs/specs/requirements/FR-SC-24.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use anyhow::Result;

use super::Engine;
use crate::tools::surface;

/// The managed scanner tools over one engine, reached by
/// [`Security::tools`](super::Security::tools).
#[derive(Clone, Copy)]
pub struct Tools<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Tools<'e> {
    /// `logos security install` ([FR-SC-22]): install the requested scanners
    /// that have no usable copy — not the user's own, at or above the
    /// manifest's minimum — after checking the per-user, per-tool-version
    /// install consent ([`crate::tools::surface::install`]). CLI only. Holds
    /// the project's scan-or-install lock while it runs (S-758) — in a
    /// project whose `.logos` is a directory. Anywhere else no scan can run:
    /// with no `.logos` none has, and a symlinked one refuses every scan. So
    /// there the install takes no lock and writes nothing, as before.
    ///
    /// # Errors
    /// Exit 2 for a missing consent (naming each tool, version, source and
    /// size; nothing is downloaded), an unknown tool, a platform without
    /// managed installs or no tools directory. The lock's `Busy` while
    /// another scan or install of the project holds it.
    ///
    /// [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
    pub fn install(
        &self,
        request: &surface::InstallRequest,
    ) -> Result<surface::SecurityInstall> {
        install_locked(self.engine, || surface::install(&self.engine.root, &self.environment(), self.manifest(), request))
    }

    /// `logos security uninstall` ([FR-SC-22]): remove the managed copies of
    /// the selected scanners, never a copy the user installed, and report
    /// those that stay ([`crate::tools::surface::uninstall`]). CLI only.
    ///
    /// # Errors
    /// Exit 2 for an unknown tool, a platform without managed installs or no
    /// tools directory.
    ///
    /// [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
    pub fn uninstall(
        &self,
        tools: &crate::security::Selection,
    ) -> Result<surface::SecurityUninstall> {
        self.on_surface(|root, env, manifest| surface::uninstall(root, env, manifest, tools))
    }

    /// The Security page's Install, Update or Install & scan ([FR-SC-24],
    /// S-747): ready to run once the consent dialog answered every line it
    /// needs, else the plan the dialog shows
    /// ([`crate::tools::surface::admit_install`]). Decides on recorded state
    /// only: it starts no process, not even a copy's version probe, and
    /// writes nothing (S-766).
    ///
    /// # Errors
    /// Exit 2 for an unknown or unoffered tool, a platform without managed
    /// installs, no tools directory, or an invalid config.
    ///
    /// [FR-SC-24]: ../../../docs/specs/requirements/FR-SC-24.md
    pub fn install_admit(
        &self,
        ask: &surface::InstallAsk,
    ) -> Result<surface::InstallAdmission> {
        self.on_surface(|root, env, manifest| surface::admit_install(root, env, manifest, ask))
    }

    /// The `tool-install` job's work ([FR-SC-24]): the admitted install, then
    /// — with `scan` — the full [scan](super::Security::scan_reporting) without
    /// recording consent, each step handed to `progress`. Run on a job thread,
    /// a cancel stops the download, leaves no partial tool and reports the
    /// tools installed by then ([`crate::tools::surface::install_job`]). The
    /// web UI's job already holds the project's lock; called anywhere else,
    /// this takes it.
    ///
    /// # Errors
    /// As [`install`](Self::install) and the scan, and the
    /// jobs module's `Cancelled` when the job was cancelled.
    ///
    /// [FR-SC-24]: ../../../docs/specs/requirements/FR-SC-24.md
    pub fn install_job(
        &self,
        request: &surface::InstallRequest,
        scan: bool,
        mut progress: impl FnMut(&surface::InstallStep<'_>),
    ) -> Result<surface::SecurityInstallJob> {
        crate::security::exclusively(&self.engine.root, crate::security::RunKind::ToolInstall, || {
            let mut run_scan = |step: &mut dyn FnMut(&crate::security::ScanProgress)| self.engine.security().scan_reporting(false, step);
            let scan: Option<surface::ScanRun<'_>> = if scan { Some(&mut run_scan) } else { None };
            surface::install_job(&self.engine.root, &self.environment(), self.manifest(), request, scan, &mut progress)
        })
    }

    /// `f` over this engine's root and where the security surfaces find the
    /// user-level directory, the scanner binaries and the tool manifest — the
    /// three every tools surface call is handed.
    fn on_surface<T>(
        &self,
        f: impl FnOnce(&std::path::Path, &crate::security::ScanEnvironment, &crate::tools::Manifest) -> Result<T>,
    ) -> Result<T> {
        f(&self.engine.root, &self.environment(), self.manifest())
    }

    pub(crate) fn manifest(&self) -> &'e crate::tools::Manifest {
        self.engine.tools_manifest
            .as_ref()
            .unwrap_or_else(|| crate::tools::Manifest::builtin())
    }

    pub(crate) fn environment(&self) -> crate::security::ScanEnvironment {
        self.engine.scan_env
            .clone()
            .unwrap_or_else(crate::security::ScanEnvironment::from_process)
    }
}

/// Run a per-user `install` — `security install`, `coverage install` —
/// holding the project's scan-or-install lock as a tool install, or
/// unlocked in a project with no `.logos` yet: a per-user install needs no
/// index, and the lock file lives under it.
pub(super) fn install_locked<T>(engine: &Engine, install: impl FnOnce() -> Result<T>) -> Result<T> {
    if !std::fs::symlink_metadata(engine.root.join(".logos")).is_ok_and(|m| m.is_dir()) {
        return crate::security::unlocked(install);
    }
    crate::security::exclusively(&engine.root, crate::security::RunKind::ToolInstall, install)
}
