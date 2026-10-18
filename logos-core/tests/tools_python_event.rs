//! The managed Python's install event ([tool-installer] Observability,
//! S-745): installing the first Python scanner emits one `info` event on
//! `logos::tools` each for `uv`, the managed Python and the scanner, and the
//! Python's names its host and size. A second scanner, installed while that
//! Python is in place, emits none for it.
//!
//! This file holds one test, as `tools_install_event.rs` does: an event's
//! callsite interest is cached process-wide, so a capturing subscriber is
//! reliable only when no other test in the binary emits the event first. The
//! stub `uv` here does only what an install needs; the full one is
//! `tools_python.rs`'s.
//!
//! [tool-installer]: ../../docs/specs/architecture/components/tool-installer.md
#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use logos_core::tools::{
    sha256_file, InstallOutcome, Installer, Layout, Manifest, Platform, Source,
};

#[derive(Clone)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const UV_STUB: &str = r#"#!/bin/sh
sub="$1"; shift
case "$sub" in python|pip) shift ;; esac
pos=''; py=''; idir=''; req=''
while [ $# -gt 0 ]; do
  case "$1" in
    --python) py="$2"; shift 2 ;;
    --install-dir) idir="$2"; shift 2 ;;
    -r) req="$2"; shift 2 ;;
    --default-index|--cache-dir|--only-binary) shift 2 ;;
    -*) shift ;;
    *) pos="$1"; shift ;;
  esac
done
case "$sub" in
python)
  [ -d "$idir/cpython-$pos-stub" ] && exit 0
  mkdir -p "$idir/cpython-$pos-stub/bin"
  printf '#!/bin/sh\n' > "$idir/cpython-$pos-stub/bin/python3.12"
  chmod 755 "$idir/cpython-$pos-stub/bin/python3.12" ;;
venv)
  mkdir -p "$pos/bin"
  ln -s "$UV_PYTHON_INSTALL_DIR/cpython-$py-stub/bin/python3.12" "$pos/bin/python" ;;
pip)
  d=$(dirname "$py"); name=$(head -n 1 "$req" | cut -d = -f 1)
  printf '#!/bin/sh\n' > "$d/$name"
  chmod 755 "$d/$name" ;;
esac
"#;

fn script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn on_process_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

#[test]
fn the_managed_python_install_emits_one_event_with_its_host_and_size() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (bin, server, src) = (base.join("bin"), base.join("server"), base.join("src"));
    for dir in [&bin, &server, &src] {
        fs::create_dir_all(dir).unwrap();
    }
    for program in [
        "tar", "gzip", "mkdir", "chmod", "ln", "dirname", "head", "cut",
    ] {
        if let Some(found) = on_process_path(program) {
            std::os::unix::fs::symlink(found, bin.join(program)).unwrap();
        }
    }
    script(
        &bin.join("curl"),
        &format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in --output) out=\"$2\"; shift 2 ;; *) shift ;; esac; done\n/bin/cp '{}' \"$out\"\n",
            server.join("uv.tar.gz").display()
        ),
    );

    let platform = Platform::current().unwrap();
    let mut manifest = Manifest::builtin().clone();
    manifest
        .tools
        .retain(|t| t.tool == "bandit" || t.tool == "semgrep");
    for entry in &mut manifest.tools {
        entry.python.as_mut().unwrap().requirements = format!(
            "{}=={} --hash=sha256:{}\n",
            entry.tool,
            entry.version,
            "0".repeat(64)
        );
    }
    manifest.python.index = "https://index.invalid/simple".to_string();
    let mut artifact = manifest.uv.artifact(platform).unwrap().clone();
    let Layout::TarGz { member } = artifact.layout.clone() else {
        panic!("uv ships as a .tar.gz");
    };
    fs::create_dir_all(src.join(&member).parent().unwrap()).unwrap();
    script(&src.join(&member), UV_STUB);
    let archive = server.join("uv.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&src)
        .arg(&member)
        .status()
        .unwrap();
    assert!(status.success());
    artifact.url = "https://fixtures.invalid/uv.tar.gz".to_string();
    artifact.sha256 = sha256_file(&archive).unwrap();
    artifact.size = fs::metadata(&archive).unwrap().len();
    manifest.uv.artifacts = vec![artifact];
    let python_size = manifest.python.size(platform).unwrap();

    let installer = Installer::new(base.join("tools"))
        .with_manifest(manifest)
        .with_search_path(Some(bin.into_os_string()));
    let captured = Captured(Arc::new(Mutex::new(Vec::new())));
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let (first, second) = tracing::subscriber::with_default(subscriber, || {
        let first = installer.install(&["bandit"], &Source::Network).unwrap();
        let second = installer.install(&["semgrep"], &Source::Network).unwrap();
        (first, second)
    });
    assert!(
        matches!(first.tools[0].outcome, InstallOutcome::Installed { .. }),
        "{first:?}"
    );
    assert!(
        matches!(second.tools[0].outcome, InstallOutcome::Installed { .. }),
        "{second:?}"
    );

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let events: Vec<&str> = log.lines().filter(|l| l.contains("tool install")).collect();
    let python: Vec<&&str> = events
        .iter()
        .filter(|e| e.contains("tool=\"python\""))
        .collect();
    assert_eq!(
        python.len(),
        1,
        "one event, from the install that fetched it: {log}"
    );
    for field in [
        "logos::tools",
        "version=\"3.12.14\"",
        "host=\"github.com\"",
        &format!("bytes={python_size}"),
        "duration_ms=",
        "outcome=\"installed\"",
    ] {
        assert!(
            python[0].contains(field),
            "{field} missing from: {}",
            python[0]
        );
    }
    for tool in ["uv", "bandit", "semgrep"] {
        assert!(
            events
                .iter()
                .any(|e| e.contains(&format!("tool=\"{tool}\""))
                    && e.contains("outcome=\"installed\"")),
            "{tool}: {log}"
        );
    }
}
