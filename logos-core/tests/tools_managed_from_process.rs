//! The shipped detection path finds managed copies (S-743): every surface
//! opens the orchestrator through `ScanEnvironment::from_process`, so the
//! tools directory must come from `$LOGOS_DATA_HOME` there too.
//!
//! This file holds one test because it sets process environment variables;
//! no other test in this binary can observe them.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;

use logos_core::security::{CopyKind, Detection, Orchestrator, ScanEnvironment};
use logos_core::tools::Manifest;

#[test]
fn the_process_environment_names_the_tools_directory_detection_uses() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (tools, user, root, empty) = (
        base.join("tools"),
        base.join("user"),
        base.join("project"),
        base.join("empty-bin"),
    );
    for dir in [&user, &root, &empty] {
        fs::create_dir_all(dir).unwrap();
    }
    let entry = Manifest::builtin().tool("osv-scanner").unwrap();
    let binary = tools
        .join("osv-scanner")
        .join(&entry.version)
        .join(&entry.binary);
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(
        &binary,
        format!("#!/bin/sh\necho 'osv-scanner version: {}'\n", entry.version),
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

    std::env::set_var("LOGOS_DATA_HOME", &tools);
    std::env::set_var("LOGOS_CONFIG_HOME", &user);
    std::env::set_var("PATH", &empty);

    assert_eq!(ScanEnvironment::from_process().tools_dir, Some(tools));
    let states = Orchestrator::open(&root).unwrap().detect();
    let osv = states.iter().find(|s| s.scanner == "osv-scanner").unwrap();
    assert_eq!(
        osv.detection,
        Detection::Installed {
            binary,
            version: Some(entry.version.clone()),
            copy: CopyKind::Managed,
        }
    );
}
