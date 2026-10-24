//! The install event ([tool-installer] Observability, S-743): one `info`
//! event on `logos::tools` per tool, naming the source host and never the
//! URL's path or query.
//!
//! This file holds one test: an event's callsite interest is cached
//! process-wide, so a capturing subscriber is reliable only when no other
//! test in the binary emits the event first.
//!
//! [tool-installer]: ../../docs/specs/architecture/components/tool-installer.md
#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use logos_core::tools::{InstallOutcome, Installer, Layout, Manifest, Platform, Source};

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

#[test]
fn each_install_emits_one_event_with_the_host_and_never_the_url_path_or_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools/osv-scanner");
    // A curl that serves the osv-scanner fixture whatever the URL says.
    let curl = bin.join("curl");
    fs::write(
        &curl,
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in --output) out=\"$2\"; shift 2 ;; *) shift ;; esac; done\n/bin/cp '{}' \"$out\"\n",
            fixture.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();

    let platform = Platform::current().unwrap();
    let mut manifest = Manifest::builtin().clone();
    manifest.tools.retain(|t| t.tool == "osv-scanner");
    let artifact = manifest.tools[0]
        .artifacts
        .iter_mut()
        .find(|a| a.platform == platform)
        .unwrap();
    artifact.url = "https://fixtures.invalid/secret/path/osv-scanner?token=SEKRIT".to_string();
    artifact.sha256 =
        "2e3b29ec02f47474da838f7db433924559c3bb2ce72a3b30c293ddf78069091e".to_string();
    assert_eq!(artifact.layout, Layout::Binary);

    let captured = Captured(Arc::new(Mutex::new(Vec::new())));
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    let report = tracing::subscriber::with_default(subscriber, || {
        Installer::new(base.join("tools"))
            .with_manifest(manifest)
            .with_search_path(Some(bin.into_os_string()))
            .install(&["osv-scanner"], &Source::Network)
            .unwrap()
    });
    assert!(
        matches!(report.tools[0].outcome, InstallOutcome::Installed { .. }),
        "{report:?}"
    );

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let events: Vec<&str> = log.lines().filter(|l| l.contains("tool install")).collect();
    assert_eq!(events.len(), 1, "{log}");
    let event = events[0];
    for field in [
        "logos::tools",
        "tool=\"osv-scanner\"",
        "version=\"2.7.0\"",
        "host=\"fixtures.invalid\"",
        "bytes=104",
        "duration_ms=",
        "outcome=\"installed\"",
    ] {
        assert!(event.contains(field), "{field} missing from: {event}");
    }
    for leak in ["secret", "SEKRIT", "token", "/path/"] {
        assert!(!log.contains(leak), "{leak} leaked into: {log}");
    }
}
