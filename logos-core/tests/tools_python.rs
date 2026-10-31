//! Semgrep, Bandit and Checkov through the managed `uv` (S-745, [FR-SC-23],
//! [NFR-SE-10]). No test reaches the internet.
//!
//! - `uv` is a stub, packed as the builtin manifest's `uv` archive for this
//!   platform, served by a stub `curl` and SHA-256-verified like every
//!   artifact. It follows real `uv`'s precedence for every directory it
//!   writes — a flag, else a `UV_*` variable, else under `$HOME` — and leaves a
//!   file in `$HOME/.config/uv` on every run, as a `uv` keeping user state
//!   would: so a step Logos fails to pin down shows up as a write outside the
//!   tools directory.
//! - The package index is a local directory baked into the stub. Its
//!   "wheels" are the scanners themselves (shell scripts that print the
//!   pinned version and write one SARIF result) and a shared dependency. The
//!   stub checks every requirement's `--hash=sha256:` against the wheel, as
//!   `uv pip install --require-hashes` does, and fails with `uv`'s own
//!   `Hash mismatch` wording.
//! - The managed Python is a stub interpreter `uv python install` "downloads"
//!   from a fixture server directory, and caches only where
//!   `UV_PYTHON_CACHE_DIR` says (real `uv` caches nothing otherwise).
//!
//! The real install — `uv` from GitHub, CPython, the packages from PyPI, each
//! scanner run to SARIF on a machine with no Python — is a Phase 4 check.
//!
//! [FR-SC-23]: ../../docs/specs/requirements/FR-SC-23.md
//! [NFR-SE-10]: ../../docs/specs/requirements/NFR-SE-10.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use logos_core::config::INSTALL_CONSENT_FILE;
use logos_core::jobs::{JobKind, AnalysisJobs};
use logos_core::security::{CopyKind, Detection, Orchestrator, ScanEnvironment, Selection};
use logos_core::tools::surface::InstallRequest;
use logos_core::tools::{
    sha256_file, FailStage, InstallOutcome, Installer, Layout, Manifest, Platform, Source,
    UninstallOutcome, PYTHON_DIR, STAGING_DIR, UV_CACHE_DIR,
};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

/// Where the `uv` archive claims to come from.
const HOST: &str = "fixtures.invalid";
/// The package index the test manifest names; the stub reads a local
/// directory for it.
const INDEX: &str = "https://index.invalid/simple";
/// The dependency every Python tool shares.
const DEP: &str = "pyyaml-6.0.3.whl";
/// Turns [`sandbox_child`] into an install.
const SANDBOX_CHILD_ENV: &str = "LOGOS_TOOLS_PYTHON_SANDBOX_CHILD";

const PYTHON_TOOLS: [&str; 3] = ["semgrep", "bandit", "checkov"];

/// The utilities the stubs call, linked into the test search path.
const UTILITIES: [&str; 13] = [
    "tar", "gzip", "mkdir", "cp", "ln", "chmod", "ls", "head", "cut", "env", "sort", "dirname",
    "cat",
];

/// One machine: a search path holding the stubs, a recording directory, a
/// fixture server and index, a tools directory, and a project.
struct World {
    /// `None` in the sandbox child, which works in its parent's world.
    _tmp: Option<TempDir>,
    base: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let w = World {
            base: tmp.path().canonicalize().unwrap(),
            _tmp: Some(tmp),
        };
        for dir in [
            w.bin(),
            w.rec(),
            w.root(),
            w.user(),
            w.server().join("python"),
            w.index(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        for program in UTILITIES {
            if let Some(found) = on_process_path(program) {
                std::os::unix::fs::symlink(found, w.bin().join(program)).unwrap();
            }
        }
        w.curl_serves();
        fs::write(
            w.server().join("python/cpython-3.12.14.tar.gz"),
            "a stub CPython",
        )
        .unwrap();
        for tool in PYTHON_TOOLS {
            let version = &Manifest::builtin().tool(tool).unwrap().version;
            script(&w.wheel(tool), &scanner_script(tool, version));
        }
        fs::write(w.index().join(DEP), "a dependency every tool shares").unwrap();
        w
    }

    fn bin(&self) -> PathBuf {
        self.base.join("bin")
    }
    fn rec(&self) -> PathBuf {
        self.base.join("rec")
    }
    fn root(&self) -> PathBuf {
        self.base.join("project")
    }
    fn user(&self) -> PathBuf {
        self.base.join("user")
    }
    fn tools(&self) -> PathBuf {
        self.base.join("tools")
    }
    fn server(&self) -> PathBuf {
        self.base.join("server")
    }
    fn index(&self) -> PathBuf {
        self.base.join("index")
    }
    fn wheel(&self, tool: &str) -> PathBuf {
        let version = &Manifest::builtin().tool(tool).unwrap().version;
        self.index().join(format!("{tool}-{version}.whl"))
    }

    /// A `curl` that serves `https://fixtures.invalid/<file>` from the server
    /// directory, recording each URL.
    fn curl_serves(&self) {
        self.curl(&format!(
            "/bin/cp '{}/'\"${{url##*/}}\" \"$out\" || exit 22",
            self.server().display()
        ));
    }

    /// A `curl` that fails as an unreachable network does.
    fn curl_offline(&self) {
        self.curl("printf 'curl: (7) Failed to connect\\n' >&2\nexit 7");
    }

    fn curl(&self, body: &str) {
        script(
            &self.bin().join("curl"),
            &format!(
                "#!/bin/sh\nout=; url=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=\"$2\"; shift 2 ;;\n    *) url=\"$1\"; shift ;;\n  esac\ndone\nprintf '%s\\n' \"$url\" >> '{}/curl.log'\n{body}\n",
                self.rec().display()
            ),
        );
    }

    /// The test manifest over a freshly packed stub `uv` (see [`pinned`]).
    ///
    /// [`pinned`]: Self::pinned
    fn manifest(&self, tamper: Option<&str>) -> Manifest {
        self.pack_uv();
        self.pinned(tamper)
    }

    /// The stub `uv`, packed as the builtin manifest's layout for this
    /// platform and served from the fixture server.
    fn pack_uv(&self) {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let Layout::TarGz { member } = Manifest::builtin()
            .uv
            .artifact(platform)
            .unwrap()
            .layout
            .clone()
        else {
            panic!("uv ships as a .tar.gz");
        };
        let src = self.base.join("uv-src");
        fs::create_dir_all(src.join(&member).parent().unwrap()).unwrap();
        script(&src.join(&member), &self.uv_script());
        let status = Command::new("tar")
            .arg("-czf")
            .arg(self.server().join("uv.tar.gz"))
            .arg("-C")
            .arg(&src)
            .arg(&member)
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// The builtin Python tools, each requiring its wheel and the shared
    /// dependency by the hashes of the files on disk; `uv` pinned to the
    /// packed stub; the index [`INDEX`]. `tamper` pins a wrong hash for that
    /// tool's own wheel.
    fn pinned(&self, tamper: Option<&str>) -> Manifest {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let mut manifest = Manifest::builtin().clone();
        manifest.tools.retain(|t| t.python.is_some());
        let dep = sha256_file(&self.index().join(DEP)).unwrap();
        for entry in &mut manifest.tools {
            let mut own = sha256_file(&self.wheel(&entry.tool)).unwrap();
            if tamper == Some(entry.tool.as_str()) {
                own = "0".repeat(64);
            }
            let package = entry.python.as_mut().unwrap();
            package.requirements = format!(
                "# {tool}, pinned\n{tool}=={version} --hash=sha256:{own}\npyyaml==6.0.3 --hash=sha256:{dep}\n",
                tool = entry.tool,
                version = entry.version,
            );
        }
        manifest.python.index = INDEX.to_string();

        let archive = self.server().join("uv.tar.gz");
        let mut artifact = manifest.uv.artifact(platform).unwrap().clone();
        artifact.url = format!("https://{HOST}/uv.tar.gz");
        artifact.sha256 = sha256_file(&archive).unwrap();
        artifact.size = fs::metadata(&archive).unwrap().len();
        manifest.uv.artifacts = vec![artifact];
        manifest
    }

    /// The stub `uv`.
    fn uv_script(&self) -> String {
        let hasher = ["shasum", "sha256sum"]
            .into_iter()
            .find_map(on_process_path)
            .map(|p| {
                if p.ends_with("shasum") {
                    format!("'{}' -a 256", p.display())
                } else {
                    format!("'{}'", p.display())
                }
            })
            .expect("a SHA-256 tool");
        UV_STUB
            .replace("@REC@", &self.rec().display().to_string())
            .replace("@INDEX@", &self.index().display().to_string())
            .replace("@SERVER@", &self.server().display().to_string())
            .replace("@HASHER@", &hasher)
    }

    fn installer(&self, manifest: &Manifest) -> Installer {
        self.installer_at(manifest, self.tools())
    }

    fn installer_at(&self, manifest: &Manifest, tools: PathBuf) -> Installer {
        Installer::new(tools)
            .with_manifest(manifest.clone())
            .with_search_path(Some(self.bin().into_os_string()))
    }

    fn env(&self) -> ScanEnvironment {
        ScanEnvironment {
            user_dir: Some(self.user()),
            search_path: Some(self.bin().into_os_string()),
            tools_dir: Some(self.tools()),
            nvd_api_key: None,
        }
    }

    fn detect(&self, manifest: &Manifest) -> Vec<(&'static str, Detection)> {
        Orchestrator::open_in(&self.root(), &self.env())
            .unwrap()
            .with_manifest(manifest.clone())
            .detect()
            .into_iter()
            .map(|s| (s.scanner, s.detection))
            .collect()
    }

    fn lines(&self, log: &str) -> Vec<String> {
        fs::read_to_string(self.rec().join(log))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn managed(&self, tool: &str) -> PathBuf {
        let version = &Manifest::builtin().tool(tool).unwrap().version;
        self.tools().join(tool).join(version).join("bin").join(tool)
    }
}

/// The stub `uv`: `python install`, `venv` and `pip install`, with the
/// directory precedence real `uv` has.
const UV_STUB: &str = r#"#!/bin/sh
rec='@REC@'; index='@INDEX@'; server='@SERVER@'
printf '%s\n' "$*" >> "$rec/uv.log"
{ printf -- '--- %s\n' "$1"; env | sort; } >> "$rec/uv-env.log"
# A uv that keeps user state leaves it under $HOME.
mkdir -p "$HOME/.config/uv" && : > "$HOME/.config/uv/state"
sub="$1"; shift
# A test stopping an install mid-way makes `pip install` hang until killed.
if [ "$sub" = pip ] && [ -f "$rec/hang-pip" ]; then
  echo $$ > "$rec/uv.pid.tmp" && /bin/mv "$rec/uv.pid.tmp" "$rec/uv.pid"
  exec /bin/sleep 60
fi
op=''
case "$sub" in python|pip) op="$1"; shift ;; esac
cache=''; offline=''; installdir=''; nobin=''; py=''; req=''; idx=''; pos=''
while [ $# -gt 0 ]; do
  case "$1" in
    --cache-dir) cache="$2"; shift 2 ;;
    --offline) offline=1; shift ;;
    --install-dir) installdir="$2"; shift 2 ;;
    --no-bin) nobin=1; shift ;;
    --python) py="$2"; shift 2 ;;
    -r) req="$2"; shift 2 ;;
    --default-index) idx="$2"; shift 2 ;;
    --only-binary) shift 2 ;;
    -*) shift ;;
    *) pos="$1"; shift ;;
  esac
done
cache="${cache:-${UV_CACHE_DIR:-$HOME/.cache/uv}}"
mkdir -p "$cache"
pydir="${UV_PYTHON_INSTALL_DIR:-$HOME/.local/share/uv/python}"
case "$sub$op" in
pythoninstall)
  dir="${installdir:-$pydir}"
  ver="$pos"
  [ -x "$dir/cpython-$ver-stub/bin/python3.12" ] && exit 0
  archive="cpython-$ver.tar.gz"
  if [ -n "$UV_PYTHON_CACHE_DIR" ] && [ -f "$UV_PYTHON_CACHE_DIR/$archive" ]; then
    :
  elif [ -n "$offline" ]; then
    echo "error: Network connectivity is disabled, but the requested data wasn't found in the cache for: $archive" >&2; exit 2
  else
    printf '%s\n' "$archive" >> "$rec/net.log"
    if [ -n "$UV_PYTHON_CACHE_DIR" ]; then mkdir -p "$UV_PYTHON_CACHE_DIR"; cp "$server/python/$archive" "$UV_PYTHON_CACHE_DIR/"; fi
  fi
  mkdir -p "$dir/cpython-$ver-stub/bin"
  printf '#!/bin/sh\necho Python %s\n' "$ver" > "$dir/cpython-$ver-stub/bin/python3.12"
  chmod 755 "$dir/cpython-$ver-stub/bin/python3.12"
  if [ -z "$nobin" ]; then
    b="${UV_PYTHON_BIN_DIR:-$HOME/.local/bin}"; mkdir -p "$b"
    ln -sf "$dir/cpython-$ver-stub/bin/python3.12" "$b/python3.12"
  fi
  ;;
venv)
  [ -f "$rec/fail-venv" ] && { echo "error: boom" >&2; exit 2; }
  interp=$(ls -d "$pydir/cpython-$py-"*/bin/python3.12 2>/dev/null | head -n 1)
  [ -n "$interp" ] || { echo "error: No interpreter found for Python $py in managed installations" >&2; exit 2; }
  mkdir -p "$pos/bin"
  ln -s "$interp" "$pos/bin/python"
  printf 'home = %s\n' "$(dirname "$interp")" > "$pos/pyvenv.cfg"
  ;;
pipinstall)
  [ -x "$py" ] || { echo "error: No virtual environment found for path $py" >&2; exit 2; }
  bindir=$(dirname "$py")
  while read -r spec rest; do
    case "$spec" in ''|'#'*) continue ;; esac
    name="${spec%%==*}"; name="${name%%[*}"; ver="${spec#*==}"; whl="$name-$ver.whl"
    mkdir -p "$cache/wheels" "$cache/links"
    if [ ! -f "$cache/wheels/$whl" ]; then
      if [ -n "$offline" ]; then
        echo "error: Network connectivity is disabled, but $whl was not found in the cache" >&2; exit 2
      fi
      [ -f "$index/$whl" ] || { echo "error: $spec was not found in $idx" >&2; exit 2; }
      printf '%s %s\n' "$idx" "$whl" >> "$rec/net.log"
      cp "$index/$whl" "$cache/wheels/$whl"
      ln -sf "../wheels/$whl" "$cache/links/$whl"
    fi
    actual=$(@HASHER@ "$cache/wheels/$whl" | cut -d ' ' -f 1)
    case " $rest " in
      *" --hash=sha256:$actual "*) ;;
      *) printf 'error: Failed to download `%s`\n  cause: Hash mismatch for `%s`\n' "$spec" "$spec" >&2; exit 1 ;;
    esac
    if [ "$(head -c 2 "$cache/links/$whl")" = '#!' ]; then
      cp "$cache/links/$whl" "$bindir/$name"; chmod 755 "$bindir/$name"
    else
      mkdir -p "$bindir/../lib"; cp "$cache/links/$whl" "$bindir/../lib/$name"
    fi
  done < "$req"
  ;;
*) echo "error: unexpected uv $sub $op" >&2; exit 2 ;;
esac
"#;

/// A scanner that prints `version` and, given its registry template's output
/// flag, writes a SARIF report with one result under its registry driver
/// name — Checkov's into the directory its template names.
fn scanner_script(tool: &str, version: &str) -> String {
    let driver = logos_core::security::scanner(tool).unwrap().driver;
    let report = format!(
        r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"{driver}","rules":[]}}}},"results":[{{"ruleId":"{tool}.demo","level":"error","message":{{"text":"a {tool} finding"}},"locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"main.py"}},"region":{{"startLine":1}}}}}}]}}]}}]}}"#
    );
    format!(
        "#!/bin/sh\n\
         case \"$1\" in --version) printf '%s\\n' '{tool} {version}'; exit 0;; esac\n\
         out=''; dir=''; prev=''\n\
         for a in \"$@\"; do case \"$prev\" in --output) out=\"$a\";; --output-file-path) dir=\"$a\";; esac; prev=\"$a\"; done\n\
         [ -n \"$dir\" ] && out=\"$dir/results_sarif.sarif\"\n\
         [ -n \"$out\" ] && printf '%s' '{report}' > \"$out\"\n\
         exit 0\n"
    )
}

fn script(path: &Path, body: &str) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn on_process_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// Every file and directory under `dir`, relative, sorted; links not followed.
fn tree(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            out.push(path.strip_prefix(base).unwrap().display().to_string());
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args([
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.email=t@logos",
            "-c",
            "user.name=t",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

fn installed(outcome: &InstallOutcome) -> &Path {
    match outcome {
        InstallOutcome::Installed { binary } => binary,
        other => panic!("not installed: {other:?}"),
    }
}

#[test]
fn the_three_python_scanners_install_through_the_managed_uv_and_detect_as_managed() {
    let w = World::new();
    let manifest = w.manifest(None);
    let installer = w.installer(&manifest);
    let report = installer.install(&PYTHON_TOOLS, &Source::Network).unwrap();

    for (tool, name) in report.tools.iter().zip(PYTHON_TOOLS) {
        assert_eq!(tool.tool, name);
        assert_eq!(installed(&tool.outcome), w.managed(name), "{name}");
    }
    // `uv` is fetched once, verified, and kept as a managed binary; Python is
    // provisioned once; each package comes from the pinned index.
    assert_eq!(w.lines("curl.log"), [format!("https://{HOST}/uv.tar.gz")]);
    let uv = w.tools().join("uv/0.12.21/uv");
    assert!(uv.is_file());
    let net = w.lines("net.log");
    assert_eq!(
        net.iter().filter(|l| l.starts_with("cpython-")).count(),
        1,
        "{net:?}"
    );
    for tool in PYTHON_TOOLS {
        assert!(
            net.iter()
                .any(|l| l.starts_with(INDEX) && l.contains(&format!("{tool}-"))),
            "{tool}: {net:?}"
        );
    }

    // The flags that keep the install pinned, hash-checked and contained.
    let calls = w.lines("uv.log");
    let cache = w.tools().join(UV_CACHE_DIR);
    assert_eq!(
        calls.len(),
        9,
        "python install, venv and pip install per tool: {calls:#?}"
    );
    for call in &calls {
        assert!(call.contains("--no-config"), "{call}");
        assert!(
            call.contains(&format!("--cache-dir {}", cache.display())),
            "{call}"
        );
        assert!(!call.contains("--offline"), "{call}");
    }
    for call in calls.iter().filter(|c| c.starts_with("python install")) {
        assert!(
            call.contains("3.12.14") && call.contains("--no-bin"),
            "{call}"
        );
        assert!(
            call.contains(&format!(
                "--install-dir {}",
                w.tools().join(PYTHON_DIR).display()
            )),
            "{call}"
        );
    }
    for call in calls.iter().filter(|c| c.starts_with("venv")) {
        assert!(
            call.contains("--relocatable") && call.contains("--managed-python"),
            "{call}"
        );
        assert!(call.contains("--no-python-downloads"), "{call}");
    }
    for call in calls.iter().filter(|c| c.starts_with("pip install")) {
        assert!(
            call.contains("--require-hashes") && call.contains("--only-binary :all:"),
            "{call}"
        );
        assert!(call.contains(&format!("--default-index {INDEX}")), "{call}");
    }

    // Detection: each resolves as managed, at its pinned version, under the
    // tools directory only.
    let detected = w.detect(&manifest);
    for tool in PYTHON_TOOLS {
        let version = &manifest.tool(tool).unwrap().version;
        let (_, detection) = detected.iter().find(|(s, _)| *s == tool).unwrap();
        assert_eq!(
            detection,
            &Detection::Installed {
                binary: w.managed(tool),
                version: Some(version.clone()),
                copy: CopyKind::Managed,
            },
            "{tool}"
        );
    }

    // A second install fetches and runs nothing.
    let again = installer.install(&PYTHON_TOOLS, &Source::Network).unwrap();
    assert!(
        again
            .tools
            .iter()
            .all(|t| matches!(t.outcome, InstallOutcome::AlreadyPresent { .. })),
        "{again:?}"
    );
    assert_eq!(w.lines("uv.log").len(), 9, "nothing ran again");
    assert!(!w.tools().join(STAGING_DIR).exists());
}

/// [NFR-SE-10]: a package whose hash is not the pinned one fails that scanner
/// at `checksum` and leaves no environment; the others install.
///
/// [NFR-SE-10]: ../../docs/specs/requirements/NFR-SE-10.md
#[test]
fn a_hash_mismatch_fails_that_scanner_and_leaves_no_environment() {
    let w = World::new();
    let manifest = w.manifest(Some("bandit"));
    let report = w
        .installer(&manifest)
        .install(&PYTHON_TOOLS, &Source::Network)
        .unwrap();

    let bandit = report.tools.iter().find(|t| t.tool == "bandit").unwrap();
    match &bandit.outcome {
        InstallOutcome::Failed { stage, reason } => {
            assert_eq!(*stage, FailStage::Checksum, "{reason}");
            assert!(
                reason.contains("Hash mismatch for `bandit==1.7.10`"),
                "{reason}"
            );
            assert!(reason.contains("the environment was removed"), "{reason}");
        }
        other => panic!("bandit installed from a package with the wrong hash: {other:?}"),
    }
    assert!(!w.tools().join("bandit").exists(), "no environment is left");
    assert!(!w.tools().join(STAGING_DIR).exists(), "no staging is left");
    for tool in ["semgrep", "checkov"] {
        let t = report.tools.iter().find(|t| t.tool == tool).unwrap();
        assert_eq!(installed(&t.outcome), w.managed(tool));
    }
    let (_, bandit) = w
        .detect(&manifest)
        .into_iter()
        .find(|(s, _)| *s == "bandit")
        .unwrap();
    assert_eq!(bandit, Detection::NotInstalled);
}

/// When no Python tool of a call ends up in place, what was fetched for one —
/// `uv`, the managed Python and `uv`'s cache — is not left behind.
#[test]
fn a_failed_install_of_the_only_python_scanner_leaves_no_support_behind() {
    let w = World::new();
    let manifest = w.manifest(Some("bandit"));
    let report = w
        .installer(&manifest)
        .install(&["bandit"], &Source::Network)
        .unwrap();
    assert!(
        matches!(
            report.tools[0].outcome,
            InstallOutcome::Failed {
                stage: FailStage::Checksum,
                ..
            }
        ),
        "{report:?}"
    );
    assert!(
        w.lines("uv.log")
            .iter()
            .any(|c| c.starts_with("pip install")),
        "uv did run"
    );
    let left: Vec<_> = fs::read_dir(w.tools())
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
}

/// A failing step is named by its subcommand alone, never by the staging
/// path it was given (that path is removed with the failed install).
#[test]
fn a_failing_uv_step_is_named_by_its_subcommand() {
    let w = World::new();
    let manifest = w.manifest(None);
    fs::write(w.rec().join("fail-venv"), "").unwrap();
    let report = w
        .installer(&manifest)
        .install(&["bandit"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Failed {
            stage: FailStage::Install,
            reason: "uv venv exited with 2: error: boom".to_string(),
        }
    );
}

/// A symlink in place of `uv-cache/` or `python/` — the two directories `uv`
/// writes through — is removed and replaced by a real directory, never
/// followed.
#[test]
fn a_symlink_in_place_of_the_uv_cache_or_the_python_is_never_followed() {
    let w = World::new();
    let manifest = w.manifest(None);
    let outside = w.base.join("outside");
    fs::create_dir_all(w.tools()).unwrap();
    for name in [UV_CACHE_DIR, PYTHON_DIR] {
        fs::create_dir_all(outside.join(name)).unwrap();
        std::os::unix::fs::symlink(outside.join(name), w.tools().join(name)).unwrap();
    }
    let report = w
        .installer(&manifest)
        .install(&["bandit"], &Source::Network)
        .unwrap();
    assert_eq!(installed(&report.tools[0].outcome), w.managed("bandit"));
    assert_eq!(
        tree(&outside),
        [PYTHON_DIR, UV_CACHE_DIR],
        "nothing was written through a link"
    );
    for name in [UV_CACHE_DIR, PYTHON_DIR] {
        let meta = fs::symlink_metadata(w.tools().join(name)).unwrap();
        assert!(meta.file_type().is_dir(), "{name} is a real directory now");
    }
}

/// The managed Python and the environments live only under the tools
/// directory ([FR-SC-23] AC 3). The install runs in a child process whose
/// `HOME` and `PATH` are a sandbox and whose environment carries the `UV_*`
/// and `PYTHON*` variables a user might have set — each pointing into the
/// sandbox. Afterwards the sandbox is byte-for-byte unchanged and `uv` saw
/// none of them; only the proxy variable passed through.
///
/// [FR-SC-23]: ../../docs/specs/requirements/FR-SC-23.md
#[test]
fn nothing_is_written_outside_the_tools_directory_and_no_user_uv_setting_reaches_uv() {
    let w = World::new();
    w.manifest(None);
    let home = w.base.join("home");
    fs::create_dir_all(home.join(".local/bin")).unwrap();
    fs::write(
        home.join(".profile"),
        "export PATH=$HOME/.local/bin:$PATH\n",
    )
    .unwrap();
    let before = (tree(&home), tree(&w.bin()), tree(&w.root()));

    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args([
            "--exact",
            "sandbox_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SANDBOX_CHILD_ENV, &w.base)
        .env("HOME", &home)
        .env("PATH", w.bin())
        .env("UV_CACHE_DIR", home.join(".cache/uv"))
        .env("UV_PYTHON_INSTALL_DIR", home.join(".local/share/uv/python"))
        .env("UV_PYTHON_BIN_DIR", home.join(".local/bin"))
        .env("UV_INDEX_URL", "https://elsewhere.invalid/simple")
        .env("PYTHONPATH", home.join("site"))
        .env("HTTPS_PROXY", "http://proxy.invalid:3128")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    assert_eq!(
        (tree(&home), tree(&w.bin()), tree(&w.root())),
        before,
        "HOME, PATH and the project are untouched"
    );
    for tool in PYTHON_TOOLS {
        assert!(w.managed(tool).is_file(), "{tool}");
    }
    let mut top: Vec<String> = fs::read_dir(w.tools())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    top.sort();
    assert_eq!(
        top,
        ["bandit", "checkov", "python", "semgrep", "uv", "uv-cache"]
    );

    let env = fs::read_to_string(w.rec().join("uv-env.log")).unwrap();
    let staging = w.tools().join(STAGING_DIR);
    let mut calls = 0;
    for call in env.split("--- ").skip(1) {
        calls += 1;
        let var = |name: &str| {
            call.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}=")))
                .map(str::to_string)
        };
        assert!(
            var("HOME").is_some_and(|h| Path::new(&h).starts_with(&staging)),
            "{call}"
        );
        assert!(
            var("TMPDIR").is_some_and(|t| Path::new(&t).starts_with(&staging)),
            "{call}"
        );
        assert_eq!(
            var("UV_PYTHON_INSTALL_DIR"),
            Some(w.tools().join(PYTHON_DIR).display().to_string())
        );
        assert_eq!(
            var("UV_PYTHON_CACHE_DIR"),
            Some(
                w.tools()
                    .join(UV_CACHE_DIR)
                    .join(PYTHON_DIR)
                    .display()
                    .to_string()
            )
        );
        assert_eq!(
            var("HTTPS_PROXY").as_deref(),
            Some("http://proxy.invalid:3128"),
            "a proxy passes through"
        );
        for leaked in [
            "UV_CACHE_DIR",
            "UV_PYTHON_BIN_DIR",
            "UV_INDEX_URL",
            "PYTHONPATH",
            SANDBOX_CHILD_ENV,
        ] {
            assert_eq!(var(leaked), None, "{leaked} reached uv");
        }
    }
    assert_eq!(calls, 9);
}

/// The child half of the sandbox test: an install of the three, from the
/// parent's world.
#[test]
#[ignore = "run as a child process by the sandbox test"]
fn sandbox_child() {
    let Some(base) = std::env::var_os(SANDBOX_CHILD_ENV) else {
        return;
    };
    // The parent's world, stubs, index and packed `uv` included.
    let w = World {
        _tmp: None,
        base: PathBuf::from(base),
    };
    let report = w
        .installer(&w.pinned(None))
        .install(&PYTHON_TOOLS, &Source::Network)
        .unwrap();
    for t in &report.tools {
        installed(&t.outcome);
    }
}

/// [FR-SC-23] AC 4 / CRA-03: with `--from`, the offline directory's uv cache
/// installs all three with the network denied — `curl` fails, the index is
/// gone, and every `uv` step runs `--offline`.
///
/// [FR-SC-23]: ../../docs/specs/requirements/FR-SC-23.md
#[test]
fn an_offline_directory_carrying_the_uv_cache_installs_all_three_without_the_network() {
    let w = World::new();
    let manifest = w.manifest(None);
    // A connected machine's install leaves the cache the offline directory
    // carries.
    w.installer_at(&manifest, w.base.join("connected"))
        .install(&PYTHON_TOOLS, &Source::Network)
        .unwrap();
    let from = w.base.join("offline");
    fs::create_dir_all(&from).unwrap();
    fs::copy(w.server().join("uv.tar.gz"), from.join("uv.tar.gz")).unwrap();
    let status = Command::new("cp")
        .arg("-R")
        .arg(w.base.join("connected").join(UV_CACHE_DIR))
        .arg(from.join(UV_CACHE_DIR))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        fs::symlink_metadata(from.join(UV_CACHE_DIR).join("links").join(DEP))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the cache holds links, as uv's does"
    );

    // The network is denied from here on.
    w.curl_offline();
    fs::rename(w.index(), w.base.join("index-unreachable")).unwrap();
    fs::rename(w.server(), w.base.join("server-unreachable")).unwrap();
    let calls_before = w.lines("uv.log").len();
    let net_before = w.lines("net.log").len();

    let report = w
        .installer(&manifest)
        .install(&PYTHON_TOOLS, &Source::Directory(from.clone()))
        .unwrap();
    for (tool, name) in report.tools.iter().zip(PYTHON_TOOLS) {
        assert_eq!(installed(&tool.outcome), w.managed(name), "{name}");
    }
    assert!(
        w.lines("curl.log").len() == 1,
        "only the connected install fetched uv: {:?}",
        w.lines("curl.log")
    );
    assert_eq!(
        w.lines("net.log").len(),
        net_before,
        "nothing reached the index or the Python host"
    );
    let offline_calls = &w.lines("uv.log")[calls_before..];
    assert_eq!(offline_calls.len(), 9);
    assert!(
        offline_calls.iter().all(|c| c.contains("--offline")),
        "{offline_calls:#?}"
    );
    assert!(w
        .tools()
        .join(UV_CACHE_DIR)
        .join(PYTHON_DIR)
        .join("cpython-3.12.14.tar.gz")
        .is_file());

    // Without the cache, an offline install of a Python scanner says what is
    // missing, and installs nothing.
    let bare = w.base.join("offline-without-cache");
    fs::create_dir_all(&bare).unwrap();
    fs::copy(from.join("uv.tar.gz"), bare.join("uv.tar.gz")).unwrap();
    let elsewhere = w.base.join("tools-2");
    let report = w
        .installer_at(&manifest, elsewhere.clone())
        .install(&["bandit"], &Source::Directory(bare))
        .unwrap();
    match &report.tools[0].outcome {
        InstallOutcome::Failed {
            stage: FailStage::Download,
            reason,
        } => {
            assert!(
                reason.contains(UV_CACHE_DIR) && reason.contains("is not a directory"),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!elsewhere.join("bandit").exists());

    // A link in the offline cache that leaves it is refused.
    let escaping = w.base.join("offline-escaping");
    fs::create_dir_all(escaping.join(UV_CACHE_DIR)).unwrap();
    fs::copy(from.join("uv.tar.gz"), escaping.join("uv.tar.gz")).unwrap();
    std::os::unix::fs::symlink("../../outside", escaping.join(UV_CACHE_DIR).join("escape"))
        .unwrap();
    let elsewhere = w.base.join("tools-3");
    let report = w
        .installer_at(&manifest, elsewhere.clone())
        .install(&["bandit"], &Source::Directory(escaping))
        .unwrap();
    match &report.tools[0].outcome {
        InstallOutcome::Failed {
            stage: FailStage::Download,
            reason,
        } => {
            assert!(reason.contains("links outside the cache"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!elsewhere.join(UV_CACHE_DIR).join("escape").exists());
}

/// Removing a managed Python tool removes its environment; once the last one
/// goes, the managed Python, `uv`'s cache and `uv` go with it.
#[test]
fn uninstalling_the_last_python_scanner_reclaims_python_uv_and_the_cache() {
    let w = World::new();
    let manifest = w.manifest(None);
    let installer = w.installer(&manifest);
    installer.install(&PYTHON_TOOLS, &Source::Network).unwrap();

    let removed = installer.uninstall(&["bandit"]).unwrap();
    assert_eq!(removed[0].outcome, UninstallOutcome::Removed);
    assert!(!w.tools().join("bandit").exists());
    for kept in [PYTHON_DIR, UV_CACHE_DIR, "uv", "semgrep", "checkov"] {
        assert!(
            w.tools().join(kept).exists(),
            "{kept} stays while a Python tool needs it"
        );
    }

    installer.uninstall(&["semgrep", "checkov"]).unwrap();
    let left: Vec<_> = fs::read_dir(w.tools())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// The reclaim looks only at Python tools: an installed release binary
/// (Trivy here) keeps neither `uv`, the managed Python nor the cache alive,
/// and is itself left alone.
#[test]
fn a_release_binary_beside_the_python_scanners_does_not_keep_their_support() {
    let w = World::new();
    let mut manifest = w.manifest(None);
    manifest
        .tools
        .extend(Manifest::builtin().tool("trivy").cloned());
    let trivy = w.tools().join("trivy/0.74.0/trivy");
    fs::create_dir_all(trivy.parent().unwrap()).unwrap();
    script(&trivy, "#!/bin/sh\necho 0.74.0\n");
    let installer = w.installer(&manifest);
    installer.install(&["bandit"], &Source::Network).unwrap();
    assert!(w.tools().join(PYTHON_DIR).exists());

    installer.uninstall(&["bandit"]).unwrap();
    let mut left: Vec<String> = fs::read_dir(w.tools())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["trivy"]);
    assert!(trivy.is_file());
}

/// A Python scanner installs only where its package, the managed Python and
/// `uv` all have a build for the platform; otherwise it is `unavailable`,
/// and nothing is fetched or run.
#[test]
fn a_python_scanner_is_unavailable_where_any_part_has_no_build() {
    let w = World::new();
    let platform = Platform::current().unwrap();
    // Removes one part's build for the platform.
    type Strip = fn(&mut Manifest, Platform);
    let cases: [(&str, Strip); 3] = [
        ("the package", |m, p| {
            m.tools
                .iter_mut()
                .for_each(|t| t.python.as_mut().unwrap().sizes.retain(|(q, _)| *q != p))
        }),
        ("the managed Python", |m, p| {
            m.python.sizes.retain(|(q, _)| *q != p)
        }),
        ("uv", |m, p| m.uv.artifacts.retain(|a| a.platform != p)),
    ];
    for (missing, strip) in cases {
        let mut manifest = w.manifest(None);
        strip(&mut manifest, platform);
        let report = w
            .installer(&manifest)
            .install(&["bandit"], &Source::Network)
            .unwrap();
        assert!(
            matches!(report.tools[0].outcome, InstallOutcome::Unavailable { .. }),
            "{missing}: {report:?}"
        );
    }
    assert!(
        w.lines("curl.log").is_empty() && w.lines("uv.log").is_empty(),
        "nothing was fetched or run"
    );
    assert!(!w.tools().exists() || tree(&w.tools()).is_empty());
}

/// An environment that ends up without the scanner's entry point is not
/// moved into place: the install fails and leaves nothing.
#[test]
fn an_environment_without_its_entry_point_is_not_installed() {
    let w = World::new();
    // A "wheel" that is not a script installs no `bin/bandit`.
    fs::write(w.wheel("bandit"), "a package with no console script").unwrap();
    let manifest = w.manifest(None);
    let report = w
        .installer(&manifest)
        .install(&["bandit"], &Source::Network)
        .unwrap();
    match &report.tools[0].outcome {
        InstallOutcome::Failed {
            stage: FailStage::Install,
            reason,
        } => {
            assert!(
                reason.contains("holds no regular file bin/bandit"),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!w.tools().join("bandit").exists());
    assert!(!w.tools().join(STAGING_DIR).exists());
}

/// A Python environment whose interpreter is gone is not a copy: the next
/// install provisions Python again and rebuilds it.
#[test]
fn an_environment_whose_managed_python_was_removed_is_installed_again() {
    let w = World::new();
    let manifest = w.manifest(None);
    let installer = w.installer(&manifest);
    installer.install(&["bandit"], &Source::Network).unwrap();
    fs::remove_dir_all(w.tools().join(PYTHON_DIR)).unwrap();
    assert_eq!(installer.resolve("bandit"), None);

    let report = installer.install(&["bandit"], &Source::Network).unwrap();
    assert_eq!(installed(&report.tools[0].outcome), w.managed("bandit"));
    assert!(installer.resolve("bandit").is_some());
}

/// Through the engine: the consent names each Python scanner from the index,
/// and `uv` and the managed Python each as a line of their own; with the
/// consent, `scan --install` installs the three and each one's SARIF, written
/// through the registry's fixed template, ingests.
#[test]
fn scan_install_consents_to_uv_and_python_installs_the_three_and_ingests_their_sarif() {
    let w = World::new();
    fs::write(w.root().join("main.py"), "import subprocess\n").unwrap();
    git(&w.root(), &["init", "-q", "-b", "main"]);
    git(&w.root(), &["add", "."]);
    git(&w.root(), &["commit", "-q", "-m", "seed"]);
    Engine::start(w.root()).expect("engine").index();
    // Only the Python scanners are under test: the others are switched off.
    fs::write(
        w.root().join(".logos/config.toml"),
        [
            "trivy",
            "gitleaks",
            "osv-scanner",
            "gosec",
            "codeql",
            "dependency-check",
        ]
        .map(|s| format!("[security_scan.{s}]\nenabled = false\n"))
        .concat(),
    )
    .unwrap();
    let manifest = w.manifest(None);
    let engine = Engine::start(w.root())
        .expect("engine")
        .with_scan_environment(w.env())
        .with_tools_manifest(manifest);

    let refused = engine
        .security_install(&InstallRequest {
            tools: Selection::Only(vec!["bandit".to_string()]),
            update: false,
            from: None,
            consent: false,
            accept_licences: Vec::new(),
        })
        .expect_err("no consent is recorded")
        .to_string();
    for line in [
        "bandit 1.7.10 from index.invalid (",
        "uv 0.12.21 from fixtures.invalid (",
        "python 3.12.14 from github.com (",
    ] {
        assert!(refused.contains(line), "names `{line}`: {refused}");
    }
    assert!(
        w.lines("curl.log").is_empty() && w.lines("uv.log").is_empty(),
        "a refusal downloads and runs nothing"
    );
    assert!(!w.user().join(INSTALL_CONSENT_FILE).exists());

    // Offline, every line names the directory instead of a host.
    let from = w.base.join("offline-dir");
    let refused = engine
        .security_install(&InstallRequest {
            tools: Selection::Only(vec!["bandit".to_string()]),
            update: false,
            from: Some(from.clone()),
            consent: false,
            accept_licences: Vec::new(),
        })
        .expect_err("no consent is recorded")
        .to_string();
    for tool in ["bandit 1.7.10", "uv 0.12.21", "python 3.12.14"] {
        let line = format!("{tool} from {} (", from.display());
        assert!(refused.contains(&line), "names `{line}`: {refused}");
    }
    for host in ["index.invalid", "fixtures.invalid", "github.com"] {
        assert!(
            !refused.contains(host),
            "an offline install names no host: {refused}"
        );
    }

    // Once bandit brought `uv` and Python in, the next scanner's consent asks
    // for that scanner alone.
    let request = |tool: &str, consent| InstallRequest {
        tools: Selection::Only(vec![tool.to_string()]),
        update: false,
        from: None,
        consent,
        accept_licences: Vec::new(),
    };
    engine.security_install(&request("bandit", true)).unwrap();
    let consent_file = w.user().join(INSTALL_CONSENT_FILE);
    let consent = fs::read_to_string(&consent_file).unwrap();
    for line in [
        "uv = [\"0.12.21\"]",
        "python = [\"3.12.14\"]",
        "bandit = [\"1.7.10\"]",
    ] {
        assert!(consent.contains(line), "{line}: {consent}");
    }
    // Only bandit stays recorded, so a consent line for the installed `uv` or
    // Python would show in the refusal below.
    fs::write(&consent_file, "[tool]\nbandit = [\"1.7.10\"]\n").unwrap();
    let refused = engine
        .security_install(&request("semgrep", false))
        .expect_err("semgrep is not consented to")
        .to_string();
    assert!(
        refused.contains("semgrep 1.85.0 from index.invalid ("),
        "{refused}"
    );
    for line in ["uv 0.12.21", "python 3.12.14"] {
        assert!(
            !refused.contains(line),
            "{line} is installed and not asked for again: {refused}"
        );
    }

    let done: Value = serde_json::to_value(engine.security_scan_install(true).unwrap()).unwrap();
    let installed_tools: Vec<&str> = done["install"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tool"].as_str().unwrap())
        .collect();
    assert_eq!(installed_tools, ["semgrep", "checkov"]);
    assert_eq!(done["install"]["kept"][0]["tool"], "bandit", "{done}");
    assert!(
        done["install"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["outcome"] == "installed"),
        "{done}"
    );
    let consent = fs::read_to_string(&consent_file).unwrap();
    for line in ["semgrep = [\"1.85.0\"]", "checkov = [\"3.2.533\"]"] {
        assert!(consent.contains(line), "{line}: {consent}");
    }
    let scan = &done["scan"];
    for tool in PYTHON_TOOLS {
        let run = scan["scanners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["scanner"] == tool)
            .unwrap();
        assert_eq!(run["outcome"], "ran", "{tool}: {run}");
    }
    assert_eq!(scan["findings"], 3, "one finding from each report: {scan}");
}

/// A `serve` stopped while `uv` installs a Python scanner shuts its job runner
/// down (S-759): the `uv` hanging in `pip install` is killed, and the half-built
/// environment in staging is gone by the time the shutdown returns.
#[test]
fn a_shutdown_mid_pip_install_kills_uv_and_leaves_no_staging() {
    let w = World::new();
    let manifest = w.manifest(None);
    fs::write(w.rec().join("hang-pip"), "").unwrap();
    let jobs = AnalysisJobs::new();
    let installer = w.installer(&manifest);
    jobs.start(JobKind::ToolInstall, &w.root(), move |_| {
        installer.install(&["bandit"], &Source::Network)?;
        Ok(Value::Null)
    })
    .unwrap();
    let deadline = Instant::now() + logos_core::perf::scaled(Duration::from_secs(60));
    let pid = loop {
        if let Ok(pid) = fs::read_to_string(w.rec().join("uv.pid")) {
            break pid.trim().to_string();
        }
        assert!(Instant::now() < deadline, "uv never reached pip install");
        std::thread::sleep(Duration::from_millis(20));
    };
    let staging = w.tools().join(STAGING_DIR);
    assert!(!tree(&staging).is_empty(), "the environment is being built in staging");

    assert!(jobs.shutdown(Duration::from_secs(10)), "the install ended within the grace");
    let alive = || Command::new("/bin/kill").args(["-0", &pid]).stderr(Stdio::null()).status().unwrap().success();
    let deadline = Instant::now() + logos_core::perf::scaled(Duration::from_secs(10));
    while alive() {
        assert!(Instant::now() < deadline, "uv {pid} outlived the shutdown");
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(tree(&staging).is_empty(), "no partial environment is left: {:?}", tree(&staging));
    assert!(!w.managed("bandit").exists(), "no bandit was installed");
}
