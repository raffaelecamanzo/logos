//! The Python scanners' install route ([FR-SC-23], [ADR-88] §7, S-745):
//! Semgrep, Bandit and Checkov installed by a Logos-managed `uv`, each in an
//! environment of its own, on a Logos-managed Python. No system Python is
//! used or needed.
//!
//! Everything lives under the tools directory, and nothing else is written:
//! - `uv/<version>/uv` — the pinned [`Manifest::uv`](super::Manifest::uv),
//!   fetched and SHA-256-verified like every release binary, installed with
//!   the first Python tool;
//! - `python/` — the managed Python, which `uv` downloads and checks against
//!   the hashes it carries itself;
//! - `uv-cache/` — `uv`'s cache, the Python archive included
//!   (`uv-cache/python/`): what an offline [`Source::Directory`] carries;
//! - `<tool>/<version>/` — the tool's environment, its entry point at
//!   `bin/<tool>`.
//!
//! One tool's install, after the release route's checks ([`Installer`]):
//! 1. `uv python install <version> --no-bin`: no `python` is put on any
//!    `PATH`.
//! 2. `uv venv --relocatable` inside the tool's staging directory.
//! 3. `uv pip install --require-hashes --only-binary :all:` from the
//!    manifest's [requirements](super::PythonPackage): every package pinned,
//!    every wheel's hash checked, nothing built from source ([NFR-SE-10]). A
//!    hash mismatch fails the tool with [`FailStage::Checksum`].
//! 4. The finished environment is moved into place with one `rename`; on any
//!    failure the staging directory goes, and no environment is left.
//!
//! `uv` runs with a cleared environment: no `UV_*` or `PYTHON*` variable of
//! the user's reaches it, `--no-config` keeps every `uv.toml` and
//! `pyproject.toml` out, and its `HOME` and `TMPDIR` are inside the staging
//! directory. Only the proxy and CA-bundle variables pass through.
//!
//! **Offline.** With `--from <dir>`, the `uv` archive is read from `<dir>` as
//! every other artifact is, `<dir>/uv-cache/` is merged into the tools
//! directory's cache, and every `uv` step runs with `--offline`. A connected
//! machine's `<tools dir>/uv-cache` after an install of the three is such a
//! cache.
//!
//! [FR-SC-23]: ../../../docs/specs/requirements/FR-SC-23.md
//! [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
//! [NFR-SE-10]: ../../../docs/specs/requirements/NFR-SE-10.md

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path};

use super::{
    failed, failed_io, log_install, spawn, Context, FailStage, InstallOutcome, Installer, Platform,
    PythonPackage, Source, ToolEntry, DOWNLOAD_DEADLINE,
};

/// The managed Python's directory inside the tools directory.
pub const PYTHON_DIR: &str = "python";

/// `uv`'s cache inside the tools directory, and inside an offline `--from`
/// directory.
pub const UV_CACHE_DIR: &str = "uv-cache";

/// The variables that pass through to `uv` from Logos's own environment: what
/// a download behind a proxy, or a custom CA bundle, needs.
const PASSED_THROUGH: [&str; 10] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// What `uv` writes on stderr when a package's hash is not a pinned one.
const HASH_MISMATCH: &str = "Hash mismatch";

impl Installer {
    /// Provision `entry`'s environment in `staging` and move it to `dest`:
    /// `Ok` with the download size once it is in place.
    pub(super) fn provision(
        &self,
        entry: &ToolEntry,
        package: &PythonPackage,
        staging: &Path,
        dest: &Path,
        ctx: &Context<'_>,
    ) -> Result<u64, InstallOutcome> {
        let uv = ctx
            .uv
            .get_or_init(|| self.ensure_support(&self.manifest.uv, ctx))
            .clone()?;
        let cache = self.tools_dir.join(UV_CACHE_DIR);
        // A symlink in place of either shared directory is removed, never
        // followed: `uv` writes through both.
        for dir in [&cache, &self.tools_dir.join(PYTHON_DIR)] {
            super::unlink_if_symlink(dir)
                .map_err(|e| failed_io(FailStage::Install, "could not remove", dir, &e))?;
        }
        if let Source::Directory(dir) = ctx.source {
            merge_cache(&dir.join(UV_CACHE_DIR), &cache)?;
        }
        let home = staging.join("home");
        let tmp = staging.join("tmp");
        for dir in [&cache, &home, &tmp] {
            fs::create_dir_all(dir)
                .map_err(|e| failed_io(FailStage::Install, "could not create", dir, &e))?;
        }
        let vars = self.uv_env(&home, &tmp, &cache);
        let offline = matches!(ctx.source, Source::Directory(_));
        let run = |step: &str, args: Vec<OsString>, stage| {
            run_uv(&uv, step, args, offline, &cache, &vars, staging, ctx, stage)
        };
        let python = &self.manifest.python.version;

        let mut install_python: Vec<OsString> = vec![python.into(), "--install-dir".into()];
        install_python.push(self.tools_dir.join(PYTHON_DIR).into());
        install_python.push("--no-bin".into());
        let had_python = self.has_managed_python();
        let started = std::time::Instant::now();
        let provisioned = run("python install", install_python, FailStage::Download);
        if !had_python {
            self.log_python(ctx, started, &provisioned);
        }
        provisioned?;

        let env = staging.join("tool");
        let mut venv: Vec<OsString> = vec![env.clone().into()];
        venv.extend(
            [
                "--python",
                python,
                "--managed-python",
                "--no-python-downloads",
            ]
            .map(OsString::from),
        );
        venv.push("--relocatable".into());
        run("venv", venv, FailStage::Install)?;

        let requirements = staging.join("requirements.txt");
        fs::write(&requirements, &package.requirements)
            .map_err(|e| failed_io(FailStage::Install, "could not write", &requirements, &e))?;
        let mut pip: Vec<OsString> = vec!["--python".into()];
        pip.push(env.join("bin").join("python").into());
        pip.extend(
            [
                "--require-hashes",
                "--only-binary",
                ":all:",
                "--default-index",
            ]
            .map(OsString::from),
        );
        pip.push(self.manifest.python.index.clone().into());
        pip.push("-r".into());
        pip.push(requirements.into());
        run("pip install", pip, FailStage::Download)?;

        let executable = env.join(entry.executable());
        let is_file = fs::symlink_metadata(&executable).is_ok_and(|m| m.file_type().is_file());
        if !is_file {
            return Err(failed(
                FailStage::Install,
                format!(
                    "the environment holds no regular file {}",
                    entry.executable().display()
                ),
            ));
        }
        let parent = dest
            .parent()
            .expect("<tools>/<tool>/<version> has a parent");
        fs::create_dir_all(parent)
            .map_err(|e| failed_io(FailStage::Install, "could not create", parent, &e))?;
        fs::rename(&env, dest)
            .map_err(|e| failed_io(FailStage::Install, "could not move the tool into", dest, &e))?;
        Ok(self
            .platform
            .and_then(|p| package.size(p))
            .unwrap_or_default())
    }

    /// The managed Python's install event, when this call fetched it: from
    /// its host (none for an offline install), with its download size once it
    /// is in place.
    fn log_python(
        &self,
        ctx: &Context<'_>,
        started: std::time::Instant,
        provisioned: &Result<(), InstallOutcome>,
    ) {
        let python = &self.manifest.python;
        let host = match ctx.source {
            Source::Network => python.host.as_str(),
            Source::Directory(_) => "",
        };
        let (outcome, bytes) = match provisioned {
            Ok(()) => (
                InstallOutcome::Installed {
                    binary: self.tools_dir.join(PYTHON_DIR),
                },
                self.platform
                    .and_then(|p| python.size(p))
                    .unwrap_or_default(),
            ),
            Err(failure) => (failure.clone(), 0),
        };
        log_install(PYTHON_DIR, &python.version, host, bytes, started, &outcome);
    }

    /// `uv`'s whole environment: its directories, all under the tools
    /// directory, and the [passed-through](PASSED_THROUGH) network variables.
    fn uv_env(&self, home: &Path, tmp: &Path, cache: &Path) -> Vec<(&'static str, OsString)> {
        let mut vars: Vec<(&'static str, OsString)> = vec![
            ("HOME", home.into()),
            ("TMPDIR", tmp.into()),
            (
                "UV_PYTHON_INSTALL_DIR",
                self.tools_dir.join(PYTHON_DIR).into(),
            ),
            // Without it the Python archive is not cached, and an offline
            // install could not provision Python.
            ("UV_PYTHON_CACHE_DIR", cache.join(PYTHON_DIR).into()),
            ("UV_NO_PROGRESS", "1".into()),
        ];
        vars.extend(
            PASSED_THROUGH
                .iter()
                .filter_map(|name| std::env::var_os(name).map(|v| (*name, v))),
        );
        vars
    }

    /// What the Python tools need that is not installed yet, as `(name,
    /// version, host, download size)` consent lines: `uv`, then the managed
    /// Python.
    pub(crate) fn python_support_pending(
        &self,
        platform: Platform,
    ) -> Vec<(String, String, String, u64)> {
        let mut lines: Vec<_> = self
            .support_pending(&self.manifest.uv, platform)
            .into_iter()
            .collect();
        let python = &self.manifest.python;
        if let Some(size) = python.size(platform) {
            if !self.has_managed_python() {
                lines.push((
                    PYTHON_DIR.to_string(),
                    python.version.clone(),
                    python.host.clone(),
                    size,
                ));
            }
        }
        lines
    }

    /// Whether the pinned managed Python is installed: `uv` names its
    /// directory `cpython-<version>-<platform>`.
    fn has_managed_python(&self) -> bool {
        let prefix = format!("cpython-{}-", self.manifest.python.version);
        fs::read_dir(self.tools_dir.join(PYTHON_DIR)).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(&prefix))
            })
        })
    }

    /// When no Python tool's directory is left, remove what only Python tools
    /// use: the managed Python, `uv`'s cache and `uv` itself. Best effort: a
    /// leftover is logged and never used.
    pub(super) fn reclaim_python_support(&self) {
        self.reclaim_support(
            |t| t.python.is_some(),
            &[PYTHON_DIR, UV_CACHE_DIR, self.manifest.uv.tool.as_str()],
        );
    }
}

/// Run the `uv` subcommand `step` (`python install`, `venv`, `pip install`)
/// with `rest` and the flags every step carries; `step` also names the step
/// in a failure. `stage` is where a failure lands unless `uv` reports a hash
/// mismatch.
#[allow(clippy::too_many_arguments)]
fn run_uv(
    uv: &Path,
    step: &str,
    rest: Vec<OsString>,
    offline: bool,
    cache: &Path,
    vars: &[(&str, OsString)],
    cwd: &Path,
    ctx: &Context<'_>,
    stage: FailStage,
) -> Result<(), InstallOutcome> {
    let mut args: Vec<OsString> = step.split(' ').map(OsString::from).collect();
    args.extend(rest);
    args.extend(["--no-config", "--quiet", "--cache-dir"].map(OsString::from));
    args.push(cache.into());
    if offline {
        args.push("--offline".into());
    }
    let launch = spawn::Launch {
        isolated: Some(vars),
        ..ctx.launch(cwd, DOWNLOAD_DEADLINE)
    };
    let finished = spawn::run(uv, &args, &launch)
        .map_err(|e| failed(stage, format!("could not start {}: {e}", uv.display())))?;
    if finished.status.is_some_and(|s| s.success()) {
        return Ok(());
    }
    // The one failure `uv` reports that is not its step's own: everything
    // else reads as a `curl` or `tar` failure does.
    let stderr = finished.stderr.trim();
    if stderr.contains(HASH_MISMATCH) {
        return Err(failed(
            FailStage::Checksum,
            format!(
                "uv {step}: a package does not match its pinned hash; the environment was \
                 removed and nothing from it was run: {stderr}"
            ),
        ));
    }
    super::exited_cleanly(&format!("uv {step}"), &finished, stage)
}

/// Copy into `cache` every entry of the offline `from` cache it lacks. A
/// symbolic link is copied as a link, and only one whose target is relative
/// and stays inside the cache ([`stays_inside`]); anything that is neither a
/// file, a directory nor such a link is refused. The whole of `from` is
/// checked before anything is copied, so a refused cache leaves `cache` as it
/// was. Nothing is ever written through a link already in `cache`.
fn merge_cache(from: &Path, cache: &Path) -> Result<(), InstallOutcome> {
    if !fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        return Err(failed(
            FailStage::Download,
            format!(
                "{} is not a directory: an offline install of a Python scanner needs the uv \
                 cache there (`<tools dir>/{UV_CACHE_DIR}` from a connected machine with the \
                 same operating system and processor, after the install)",
                from.display()
            ),
        ));
    }
    vet(from, Path::new(""))
        .and_then(|()| copy_missing(from, cache, Path::new("")))
        .map_err(|e| {
            failed(
                FailStage::Download,
                format!("could not read the uv cache: {e}"),
            )
        })
}

/// Refuse `from` if any entry under it would be refused by [`admit`].
fn vet(from: &Path, rel: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let rel = rel.join(entry.file_name());
        if admit(&entry, &rel)?.is_dir() {
            vet(&entry.path(), &rel)?;
        }
    }
    Ok(())
}

/// An offline cache entry's type, once it is a directory, a file, or a link
/// that [stays inside](stays_inside) the cache; anything else is an error.
fn admit(entry: &fs::DirEntry, rel: &Path) -> io::Result<fs::FileType> {
    let kind = entry.file_type()?;
    if kind.is_symlink() && !stays_inside(rel, &fs::read_link(entry.path())?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} links outside the cache", entry.path().display()),
        ));
    }
    if !(kind.is_dir() || kind.is_file() || kind.is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not a file, a directory or a link",
                entry.path().display()
            ),
        ));
    }
    Ok(kind)
}

fn copy_missing(from: &Path, to: &Path, rel: &Path) -> io::Result<()> {
    real_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let rel = rel.join(entry.file_name());
        let kind = admit(&entry, &rel)?;
        if kind.is_dir() {
            copy_missing(&src, &dst, &rel)?;
        } else if fs::symlink_metadata(&dst).is_ok() {
            // Already there, as a file or a link: kept, never written through.
        } else if kind.is_file() {
            fs::copy(&src, &dst)?;
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        }
    }
    Ok(())
}

/// `dir` as a real directory, created if absent (its parent must exist). A
/// link in its place is refused, never followed.
fn real_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_dir() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a directory", dir.display()),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(dir),
        Err(e) => Err(e),
    }
}

/// Whether a link at `rel` (relative to the cache root) pointing at `target`
/// stays inside the cache. The target may climb with `..` only before its
/// first name: after a name — which may itself be a link, to `.` say — a
/// `..` could leave the cache however the names are counted.
fn stays_inside(rel: &Path, target: &Path) -> bool {
    let mut depth = rel.components().count().saturating_sub(1) as i64;
    let mut named = false;
    for component in target.components() {
        match component {
            Component::Normal(_) => {
                depth += 1;
                named = true;
            }
            Component::CurDir => {}
            Component::ParentDir if named => return false,
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_cache_link_must_stay_inside_the_cache() {
        let at = Path::new("wheels-v6/pypi/orjson/3.13.0-cp312");
        assert!(stays_inside(at, Path::new("../../../archive-v0/fa_MeXg")));
        assert!(!stays_inside(at, Path::new("../../../../outside")));
        assert!(!stays_inside(at, Path::new("/etc/passwd")));
        assert!(stays_inside(Path::new("top"), Path::new("archive-v0/x")));
        assert!(!stays_inside(Path::new("top"), Path::new("../x")));
        // `d1 -> .` then `l -> d1/..` would resolve to the cache's parent.
        assert!(!stays_inside(Path::new("l"), Path::new("d1/..")));
        assert!(!stays_inside(at, Path::new("../../../archive-v0/x/../y")));
    }

    #[test]
    fn a_merge_never_writes_through_a_link_already_in_the_cache() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (from, cache, outside) = (
            tmp.path().join("from"),
            tmp.path().join("cache"),
            tmp.path().join("outside"),
        );
        for dir in [from.join("wheels"), cache.clone(), outside.clone()] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(from.join("wheels").join("planted.whl"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, cache.join("wheels")).unwrap();
        assert!(merge_cache(&from, &cache).is_err());
        assert_eq!(
            fs::read_dir(&outside).unwrap().count(),
            0,
            "nothing landed through the link"
        );
    }

    #[test]
    fn a_refused_offline_cache_leaves_the_tools_cache_as_it_was() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (from, cache) = (tmp.path().join("from"), tmp.path().join("cache"));
        for dir in [from.join("wheels"), cache.clone()] {
            fs::create_dir_all(dir).unwrap();
        }
        // Whatever order the directory is read in, a plain file sits beside
        // the escaping link.
        fs::write(from.join("wheels").join("a.whl"), "x").unwrap();
        fs::write(from.join("z.whl"), "x").unwrap();
        std::os::unix::fs::symlink("../../outside", from.join("wheels").join("m")).unwrap();
        let refused = merge_cache(&from, &cache).unwrap_err();
        assert!(
            matches!(&refused, InstallOutcome::Failed { reason, .. } if reason.contains("links outside the cache")),
            "{refused:?}"
        );
        assert_eq!(
            fs::read_dir(&cache).unwrap().count(),
            0,
            "nothing was copied"
        );
    }

    #[test]
    fn a_merge_never_writes_through_a_dangling_link_in_the_cache() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (from, cache, outside) = (
            tmp.path().join("from"),
            tmp.path().join("cache"),
            tmp.path().join("outside"),
        );
        for dir in [from.join("wheels"), cache.join("wheels"), outside.clone()] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(from.join("wheels").join("w.whl"), "planted").unwrap();
        std::os::unix::fs::symlink(outside.join("f"), cache.join("wheels").join("w.whl")).unwrap();
        merge_cache(&from, &cache).unwrap();
        assert!(!outside.join("f").exists(), "the link's target was written");
        assert!(fs::symlink_metadata(cache.join("wheels").join("w.whl"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
